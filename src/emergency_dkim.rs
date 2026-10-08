use crate::{
    auth, backup_admin, backup_tools as files, crypto,
    error::{Error, Result},
    mail_dns, management, resources, stalwart, App,
};
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use serde_json::{json, Value};
use sqlx::PgConnection;
async fn domain(db: &mut PgConnection, id: i64, server: i64) -> Result<Value> {
    sqlx::query_scalar("SELECT jsonb_build_object('id',id,'customer_id',customer_id,'domain_name',domain_name,'status',status,'stalwart_domain_id',stalwart_domain_id,'server_id',server_id) FROM domains WHERE id=$1 AND server_id=$2 AND status='active' AND sync_status='synced' AND stalwart_domain_id IS NOT NULL FOR SHARE").bind(id).bind(server).fetch_optional(db).await?.ok_or_else(Error::missing)
}
async fn permissions(s: &App, server: i64, required: &[&str]) -> Result<()> {
    let meta = stalwart::metadata(s, server, "/api/account").await?;
    if !required.iter().all(|p| {
        meta["data"]["permissions"]
            .as_array()
            .is_some_and(|a| a.iter().any(|v| v == *p))
    }) {
        return Err(Error::forbidden());
    }
    Ok(())
}
async fn vault(db: &mut PgConnection, server: i64) -> Result<Value> {
    sqlx::query_scalar("SELECT to_jsonb(p) FROM recovery_profiles p WHERE server_id=$1 AND restore_target=0 AND object_types @> '[\"DkimSignature\"]'::jsonb FOR UPDATE").bind(server).fetch_optional(db).await?.ok_or_else(||Error::bad("Cần hồ sơ recovery chứa DkimSignature để lưu khóa khẩn cấp"))
}
fn project(mut row: Value) -> Result<Value> {
    let fields = row
        .as_object_mut()
        .ok_or_else(|| Error::bad("Khóa DKIM không hợp lệ"))?;
    fields.retain(|key, _| {
        management::spec("dkim_keys").unwrap()["properties"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == key)
    });
    stalwart::redact(&mut row);
    Ok(row)
}
fn public_key(value: &str) -> Option<String> {
    let mut key = None;
    for part in value.split(';') {
        if let Some((name, value)) = part.trim().split_once('=') {
            if name.eq_ignore_ascii_case("p") {
                if key.is_some() {
                    return None;
                }
                let value: String = value.chars().filter(|c| !c.is_whitespace()).collect();
                if value.is_empty()
                    || !value
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
                {
                    return None;
                }
                key = Some(value);
            }
        }
    }
    key
}
fn dns_matches(dns: &Value, expected: &str) -> bool {
    let Some(expected) = public_key(expected) else {
        return false;
    };
    dns["known"] == true
        && dns["values"].as_array().is_some_and(|a| {
            a.iter().any(|v| {
                v.as_str()
                    .and_then(public_key)
                    .is_some_and(|value| value == expected)
            })
        })
}
pub async fn list(State(s): State<App>, h: HeaderMap) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "operations", true, false).await?;
    u.super_admin()?;
    let rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(p)-'private_encrypted' FROM emergency_dkim_plans p WHERE actor_id=$1 ORDER BY id DESC LIMIT 100").bind(u.id).fetch_all(&s.db).await?;
    Ok(Json(json!({"items":rows})))
}
pub async fn create(
    State(s): State<App>,
    h: HeaderMap,
    Json(v): Json<Value>,
) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "operations", true, true).await?;
    u.super_admin()?;
    let server = resources::number(&v, "server_id")?;
    let local = resources::number(&v, "domain_id")?;
    let old_id = resources::text(&v, "old_key_id", 190)?;
    let selector = resources::text(&v, "selector", 63)?;
    if !regex::Regex::new(r"^[a-z0-9][a-z0-9-]{0,62}$")
        .unwrap()
        .is_match(selector)
    {
        return Err(Error::bad("Selector không hợp lệ"));
    }
    let mut tx = s.db.begin().await?;
    let version:i64=sqlx::query_scalar("SELECT config_version FROM stalwart_servers WHERE id=$1 AND active=1 AND dry_run=0 FOR SHARE").bind(server).fetch_optional(&mut *tx).await?.ok_or_else(Error::missing)?;
    let d = domain(&mut tx, local, server).await?;
    vault(&mut tx, server).await?;
    permissions(
        &s,
        server,
        &[
            "sysDomainGet",
            "sysDkimSignatureGet",
            "sysDkimSignatureQuery",
            "sysDkimSignatureCreate",
            "sysDkimSignatureUpdate",
        ],
    )
    .await?;
    let response = management::method(
        &s,
        server,
        "x:DkimSignature/get",
        json!({"ids":[old_id],"properties":management::spec("dkim_keys")?["properties"]}),
    )
    .await?;
    let rows = response["list"]
        .as_array()
        .filter(|r| r.len() == 1 && r[0]["id"] == old_id)
        .ok_or_else(Error::missing)?;
    let old = project(rows[0].clone())?;
    if old["domainId"] != d["stalwart_domain_id"]
        || old["stage"] != "active"
        || old["selector"] == selector
        || !old["publicKey"].is_string()
    {
        return Err(Error::bad("Chọn khóa active của domain và selector mới"));
    }
    let work = files::work(&backup_admin::directory(&s).join("keys")).await?;
    let generated = async {
        let private = files::command(
            "openssl",
            &[
                "genpkey".into(),
                "-algorithm".into(),
                "RSA".into(),
                "-pkeyopt".into(),
                "rsa_keygen_bits:2048".into(),
            ],
            &[],
            &work,
        )
        .await?;
        let path = work.join("private.pem");
        files::write(&path, private.as_bytes()).await?;
        let public = files::command(
            "openssl",
            &[
                "pkey".into(),
                "-in".into(),
                path.to_string_lossy().into(),
                "-pubout".into(),
            ],
            &[],
            &work,
        )
        .await?;
        let public: String = public.lines().filter(|l| !l.starts_with("-----")).collect();
        if public.is_empty() {
            return Err(Error::bad("Không tạo được public key"));
        }
        Ok((
            crypto::seal(&s.config.key, &private)?,
            format!("v=DKIM1; k=rsa; p={public}"),
        ))
    }
    .await;
    tokio::fs::remove_dir_all(work)
        .await
        .map_err(|_| Error::bad("Không dọn được private key tạm"))?;
    let (private, public) = generated?;
    let id:i64=sqlx::query_scalar("INSERT INTO emergency_dkim_plans(server_id,domain_id,actor_id,server_version,local_hash,old_key_id,old_key,selector,public_value,private_encrypted) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) RETURNING id").bind(server).bind(local).bind(u.id).bind(version).bind(crypto::hash(&d.to_string())).bind(old_id).bind(old).bind(selector).bind(&public).bind(private).fetch_one(&mut *tx).await?;
    sqlx::query(
        "INSERT INTO audit_logs(user_id,action,context) VALUES($1,'emergency_dkim_preview',$2)",
    )
    .bind(u.id)
    .bind(json!({"plan_id":id,"domain_id":local,"server_id":server}))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(
        json!({"plan_id":id,"status":"ready","dns_name":format!("{selector}._domainkey.{}",d["domain_name"].as_str().unwrap()),"public_value":public}),
    ))
}
pub async fn apply(
    State(s): State<App>,
    h: HeaderMap,
    Path(id): Path<i64>,
    Json(v): Json<Value>,
) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "operations", true, true).await?;
    u.super_admin()?;
    let phase = resources::text(&v, "phase", 20)?;
    let state = match phase {
        "stage" => "ready",
        "activate" => "staged",
        "verify" => "active",
        _ => return Err(Error::bad("Bước DKIM không hợp lệ")),
    };
    let confirm = match phase {
        "stage" => "STAGE_DKIM",
        "activate" => "ACTIVATE_DKIM",
        _ => "VERIFY_DKIM",
    };
    if v["confirmation"] != confirm {
        return Err(Error::bad(format!("Nhập {confirm} để xác nhận")));
    }
    let mut tx = s.db.begin().await?;
    let p:Value=sqlx::query_scalar("SELECT to_jsonb(p) FROM emergency_dkim_plans p WHERE id=$1 AND actor_id=$2 AND status=$3 AND phase IS NULL AND created_at>now()-interval '30 days' FOR UPDATE").bind(id).bind(u.id).bind(state).fetch_optional(&mut *tx).await?.ok_or_else(||Error::conflict("Kế hoạch đã cũ hoặc bước đã thực hiện"))?;
    let server = p["server_id"].as_i64().unwrap();
    let version:Option<i64>=sqlx::query_scalar("SELECT config_version FROM stalwart_servers WHERE id=$1 AND active=1 AND dry_run=0 FOR SHARE").bind(server).fetch_optional(&mut *tx).await?;
    if version != p["server_version"].as_i64() {
        return Err(Error::conflict("Cấu hình server đã đổi"));
    }
    let d = domain(&mut tx, p["domain_id"].as_i64().unwrap(), server).await?;
    if crypto::hash(&d.to_string()) != p["local_hash"] {
        return Err(Error::conflict("Liên kết domain đã đổi"));
    }
    let mut required = vec![
        "sysDomainGet",
        "sysDkimSignatureGet",
        "sysDkimSignatureQuery",
    ];
    if phase == "stage" {
        required.push("sysDkimSignatureCreate");
    }
    if phase == "activate" {
        required.push("sysDkimSignatureUpdate");
    }
    permissions(&s, server, &required).await?;
    let remote_domain = management::method(
        &s,
        server,
        "x:Domain/get",
        json!({"ids":[d["stalwart_domain_id"]],"properties":["id","dkimManagement"]}),
    )
    .await?;
    if remote_domain["list"][0]["id"] != d["stalwart_domain_id"]
        || remote_domain["list"][0]["dkimManagement"]["@type"] != "Manual"
    {
        return Err(Error::bad(
            "Chuyển DKIM domain sang Manual trước khi xử lý khẩn cấp",
        ));
    }
    let mut ids = vec![p["old_key_id"].clone()];
    if p["new_key_id"].is_string() {
        ids.push(p["new_key_id"].clone());
    }
    let response = management::method(
        &s,
        server,
        "x:DkimSignature/get",
        json!({"ids":ids,"properties":management::spec("dkim_keys")?["properties"]}),
    )
    .await?;
    let state = resources::text(&response, "state", 190)?.to_owned();
    let rows = response["list"]
        .as_array()
        .ok_or_else(|| Error::bad("Không đọc được trạng thái DKIM"))?;
    let mut keys = std::collections::HashMap::new();
    for row in rows {
        let row = project(row.clone())?;
        let id = row["id"].as_str().ok_or_else(Error::missing)?.to_owned();
        if keys.insert(id, row).is_some() {
            return Err(Error::bad("Server trả ID trùng"));
        }
    }
    let old = keys
        .get(p["old_key_id"].as_str().unwrap())
        .ok_or_else(Error::missing)?;
    let (calls, profile) = if phase == "stage" {
        if *old != p["old_key"] {
            return Err(Error::conflict("Khóa cũ đã thay đổi"));
        }
        let page = management::page(
            &s,
            server,
            "dkim_keys",
            0,
            json!({"domainId":d["stalwart_domain_id"]}),
        )
        .await?;
        if !page["next"].is_null()
            || page["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["selector"] == p["selector"])
        {
            return Err(Error::bad("Selector tồn tại hoặc chưa kiểm tra hết khóa"));
        }
        let profile = vault(&mut tx, server).await?;
        let private = crypto::open(
            &s.config.key,
            p["private_encrypted"].as_str().ok_or_else(Error::missing)?,
        )?;
        let calls = json!({"ifInState":state,"create":{"emergency":{"@type":"Dkim1RsaSha256","domainId":d["stalwart_domain_id"],"selector":p["selector"],"stage":"pending","nextTransitionAt":null,"privateKey":{"@type":"Text","secret":private}}}});
        (Some(calls), Some(profile))
    } else {
        let new = keys
            .get(p["new_key_id"].as_str().ok_or_else(Error::missing)?)
            .ok_or_else(Error::missing)?;
        if new["domainId"] != d["stalwart_domain_id"] || new["selector"] != p["selector"] {
            return Err(Error::conflict("Khóa thay thế không khớp"));
        }
        let dns = mail_dns::lookup(
            &s,
            &format!(
                "{}._domainkey.{}",
                p["selector"].as_str().unwrap(),
                d["domain_name"].as_str().unwrap()
            ),
            "TXT",
        )
        .await;
        if !dns_matches(&dns, p["public_value"].as_str().unwrap()) {
            return Err(Error::conflict(
                "Chưa xác minh được public key mới trên DNS công khai",
            ));
        }
        if phase == "activate" {
            if new["stage"] != "pending" || *old != p["old_key"] {
                return Err(Error::conflict("Trạng thái khóa đã đổi"));
            }
            (
                Some(
                    json!({"ifInState":state,"update":{p["new_key_id"].as_str().unwrap():{"stage":"active","nextTransitionAt":null},p["old_key_id"].as_str().unwrap():{"stage":"retired","nextTransitionAt":null}}}),
                ),
                None,
            )
        } else {
            let dns = mail_dns::lookup(
                &s,
                &format!(
                    "{}._domainkey.{}",
                    old["selector"].as_str().ok_or_else(Error::missing)?,
                    d["domain_name"].as_str().unwrap()
                ),
                "TXT",
            )
            .await;
            if new["stage"] != "active"
                || old["stage"] != "retired"
                || dns["known"] != true
                || dns["values"].as_array().is_none_or(|a| {
                    a.iter().any(|v| {
                        v.as_str().is_some_and(|value| {
                            value.split(';').any(|tag| {
                                tag.trim().split_once('=').is_some_and(|(key, value)| {
                                    key.eq_ignore_ascii_case("p") && !value.trim().is_empty()
                                })
                            })
                        })
                    })
                })
            {
                return Err(Error::conflict(
                    "Gỡ public key cũ khỏi DNS và xác minh trạng thái khóa",
                ));
            }
            (None, None)
        }
    };
    if let Some(calls) = calls {
        sqlx::query("UPDATE emergency_dkim_plans SET status='applying',phase=$1,phase_expires_at=now()+interval '15 minutes',updated_at=now() WHERE id=$2").bind(phase).bind(id).execute(&mut *tx).await?;
        tx.commit().await?;
        let mut tx = s.db.begin().await?;
        let fresh:Option<i64>=sqlx::query_scalar("SELECT config_version FROM stalwart_servers WHERE id=$1 AND active=1 AND dry_run=0 FOR SHARE").bind(server).fetch_optional(&mut *tx).await?;
        let current = domain(&mut tx, p["domain_id"].as_i64().unwrap(), server).await?;
        let locked_profile = if phase == "stage" {
            Some(vault(&mut tx, server).await?)
        } else {
            None
        };
        if fresh != p["server_version"].as_i64()
            || crypto::hash(&current.to_string()) != p["local_hash"]
            || locked_profile.as_ref().is_some_and(|locked| {
                profile
                    .as_ref()
                    .is_none_or(|profile| profile["version"] != locked["version"])
            })
        {
            sqlx::query("UPDATE emergency_dkim_plans SET status='stale',phase=NULL,error='Configuration changed before remote write' WHERE id=$1").bind(id).execute(&mut *tx).await?;
            tx.commit().await?;
            return Err(Error::conflict(
                "Cấu hình đã thay đổi trước khi ghi lên server; tạo kế hoạch mới",
            ));
        }
        let response = management::method(&s, server, "x:DkimSignature/set", calls).await;
        sqlx::query("SELECT id FROM emergency_dkim_plans WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
        let (status, error) = match response {
            Ok(response) => {
                if phase == "stage" {
                    let new = response["created"]["emergency"]["id"]
                        .as_str()
                        .ok_or_else(|| Error::conflict("Không xác định được ID khóa mới"))?;
                    let profile = profile.unwrap();
                    let locked = locked_profile.unwrap();
                    if profile["version"] != locked["version"] {
                        sqlx::query("UPDATE emergency_dkim_plans SET status='uncertain',error='Recovery profile changed after remote write',phase=NULL WHERE id=$1").bind(id).execute(&mut *tx).await?;
                        tx.commit().await?;
                        return Err(Error::conflict(
                            "Hồ sơ recovery đã đổi sau khi tạo khóa; đối chiếu Stalwart",
                        ));
                    }
                    let mut secrets = if let Some(encrypted) = locked["vault_encrypted"].as_str() {
                        crate::native_plan::vault(&crypto::open(&s.config.key, encrypted)?)?
                    } else {
                        json!({})
                    };
                    if secrets["DkimSignature"].is_null() {
                        secrets["DkimSignature"] = json!({});
                    }
                    secrets["DkimSignature"][format!("dkimsignature-{new}")] = json!({"privateKey":{"@type":"Text","secret":crypto::open(&s.config.key,p["private_encrypted"].as_str().unwrap())?}});
                    sqlx::query("UPDATE recovery_profiles SET vault_encrypted=$1,version=version+1,updated_at=now() WHERE id=$2").bind(crypto::seal(&s.config.key,&secrets.to_string())?).bind(locked["id"].as_i64().unwrap()).execute(&mut *tx).await?;
                    sqlx::query("UPDATE emergency_dkim_plans SET new_key_id=$1,private_encrypted=NULL WHERE id=$2").bind(new).bind(id).execute(&mut *tx).await?;
                }
                (if phase == "stage" { "staged" } else { "active" }, None)
            }
            Err(_) => (
                "uncertain",
                Some("Kết quả chưa xác định; đối chiếu các khóa trên Stalwart trước khi tiếp tục"),
            ),
        };
        sqlx::query("UPDATE emergency_dkim_plans SET status=$1,phase=NULL,error=$2,updated_at=now() WHERE id=$3").bind(status).bind(error).bind(id).execute(&mut *tx).await?;
        tx.commit().await?;
        auth::audit(
            &s,
            Some(u.id),
            "emergency_dkim_result",
            json!({"plan_id":id,"phase":phase,"status":status}),
        )
        .await?;
        return Ok(Json(json!({"status":status,"plan_id":id})));
    }
    sqlx::query("UPDATE emergency_dkim_plans SET status='done',phase=NULL,error=NULL,updated_at=now() WHERE id=$1").bind(id).execute(&mut *tx).await?;
    sqlx::query(
        "INSERT INTO audit_logs(user_id,action,context) VALUES($1,'emergency_dkim_verified',$2)",
    )
    .bind(u.id)
    .bind(json!({"plan_id":id}))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"status":"done","plan_id":id})))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dns_requires_exact_single_public_key() {
        assert!(dns_matches(
            &json!({"known":true,"values":["k=rsa; p=YWJj; v=DKIM1"]}),
            "v=DKIM1; p=YWJj"
        ));
        assert!(!dns_matches(
            &json!({"known":false,"values":["p=YWJj"]}),
            "p=YWJj"
        ));
        assert!(!dns_matches(
            &json!({"known":true,"values":["p=YWJj;p=ZGVm"]}),
            "p=YWJj"
        ));
    }
}
