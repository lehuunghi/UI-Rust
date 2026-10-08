use crate::{
    auth::{self, User},
    crypto,
    error::{Error, Result},
    App,
};
use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{Postgres, Row, Transaction};
#[derive(Deserialize)]
pub struct Page {
    #[serde(default)]
    pub offset: i64,
    #[serde(default)]
    pub q: String,
}
pub fn resource(kind: &str) -> Result<(&'static str, &'static str, bool)> {
    Ok(match kind {
        "customers" => ("users", "customers", true),
        "admins" => ("users", "admins", true),
        "packages" => ("packages", "packages", true),
        "domains" => ("domains", "domains", false),
        "accounts" => ("email_accounts", "accounts", false),
        "groups" => ("email_groups", "groups", false),
        "aliases" => ("email_aliases", "accounts", false),
        "subadmins" => ("sub_admins", "admins", false),
        "subscriptions" => ("subscriptions", "packages", false),
        "payments" => ("sepay_reconciliation", "subscriptions", true),
        "servers" => ("stalwart_servers", "settings", true),
        "jobs" => ("api_sync_jobs", "operations", true),
        "logs" => ("audit_logs", "logs", true),
        "trials" => ("trial_requests", "customers", true),
        "incidents" => ("support_incidents", "customers", true),
        "alerts" => ("operational_alerts", "operations", true),
        "health" => ("server_health_samples", "operations", true),
        "changes" => ("rust_change_plans", "operations", true),
        _ => return Err(Error::missing()),
    })
}
pub async fn list(
    State(s): State<App>,
    h: HeaderMap,
    Path(kind): Path<String>,
    Query(page): Query<Page>,
) -> Result<Json<Value>> {
    let (table, perm, admin) = resource(&kind)?;
    let u = auth::require(&s, &h, perm, admin, false).await?;
    let mut filters = vec!["TRUE".to_string()];
    if kind == "customers" {
        filters.push("r.role='customer'".into())
    }
    if kind == "admins" {
        filters.push("r.role='admin'".into())
    }
    if !admin && u.role != "admin" {
        filters.push("r.customer_id=$1".into());
        if u.role == "sub_admin" {
            match kind.as_str(){"domains"=>filters.push("EXISTS(SELECT 1 FROM sub_admin_domain_access a WHERE a.user_id=$2 AND a.domain_id=r.id)".into()),"groups"=>filters.push("EXISTS(SELECT 1 FROM sub_admin_group_access a WHERE a.user_id=$2 AND a.group_id=r.id)".into()),"accounts"=>filters.push("EXISTS(SELECT 1 FROM sub_admin_domain_access a WHERE a.user_id=$2 AND a.domain_id=r.domain_id) AND EXISTS(SELECT 1 FROM sub_admin_group_access a WHERE a.user_id=$2 AND a.group_id=r.group_id)".into()),"aliases"|"subadmins"|"subscriptions"=>return Err(Error::forbidden()),_=>{}}
        }
    }
    // Bind stable parameter types even on admin-only queries. Sensitive values never enter the JSON projection.
    let projection="to_jsonb(r)-ARRAY['password_hash','two_factor_secret','token_encrypted','payload_encrypted','payload','calls','result','csrf','password_fingerprint','citizen_id','citizen_file','business_license_file']";
    let q=format!("SELECT {projection} FROM {table} r WHERE {} AND $1::bigint IS NOT NULL AND $2::bigint IS NOT NULL AND ($3='' OR ({projection})::text ILIKE '%'||$3||'%') ORDER BY r.id DESC LIMIT 50 OFFSET $4",filters.join(" AND "));
    let rows: Vec<Value> = sqlx::query_scalar(&q)
        .bind(u.owner)
        .bind(u.id)
        .bind(page.q.chars().take(100).collect::<String>())
        .bind(page.offset.clamp(0, 1_000_000))
        .fetch_all(&s.db)
        .await?;
    Ok(Json(
        json!({"items":rows,"offset":page.offset,"next":if rows.len()==50{Some(page.offset+50)}else{None}}),
    ))
}
pub fn text<'a>(v: &'a Value, key: &str, max: usize) -> Result<&'a str> {
    let s = v[key].as_str().unwrap_or("").trim();
    if s.is_empty() || s.chars().count() > max || s.chars().any(|c| c == '\0') {
        return Err(Error::bad(format!("{key} không hợp lệ")));
    }
    Ok(s)
}
pub fn number(v: &Value, k: &str) -> Result<i64> {
    v[k].as_i64()
        .filter(|n| *n > 0)
        .ok_or_else(|| Error::bad(format!("{k} không hợp lệ")))
}
pub fn domain_name(s: &str) -> bool {
    s.len() <= 190
        && s.contains('.')
        && s.split('.').all(|p| {
            !p.is_empty()
                && p.len() <= 63
                && !p.starts_with('-')
                && !p.ends_with('-')
                && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}
pub async fn enqueue(
    s: &App,
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    kind: &str,
    id: i64,
    payload: Value,
) -> Result<()> {
    let encrypted = crypto::seal(&s.config.key, &payload.to_string())?;
    let version: i64 = sqlx::query_scalar(
        "SELECT config_version FROM stalwart_servers WHERE id=$1 AND active=1 FOR SHARE",
    )
    .bind(server)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(Error::missing)?;
    sqlx::query("INSERT INTO api_sync_jobs(job_key,job_type,resource_id,server_id,payload_encrypted,server_version,run_after) VALUES($1,$2,$3,$4,$5,$6,now())").bind(crypto::token()).bind(kind).bind(id).bind(server).bind(encrypted).bind(version).execute(&mut **tx).await?;
    Ok(())
}
async fn owner_lock(tx: &mut Transaction<'_, Postgres>, id: i64) -> Result<()> {
    sqlx::query("SELECT id FROM users WHERE id=$1 AND status='active' FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(Error::missing)?;
    Ok(())
}
async fn scope(
    tx: &mut Transaction<'_, Postgres>,
    u: &User,
    domain: i64,
    group: Option<i64>,
) -> Result<()> {
    if u.role != "sub_admin" {
        return Ok(());
    }
    let d: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sub_admin_domain_access WHERE user_id=$1 AND domain_id=$2)",
    )
    .bind(u.id)
    .bind(domain)
    .fetch_one(&mut **tx)
    .await?;
    let g = if let Some(group) = group {
        sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sub_admin_group_access WHERE user_id=$1 AND group_id=$2)",
        )
        .bind(u.id)
        .bind(group)
        .fetch_one(&mut **tx)
        .await?
    } else {
        true
    };
    if !d || !g {
        return Err(Error::forbidden());
    }
    Ok(())
}
async fn plan(
    tx: &mut Transaction<'_, Postgres>,
    owner: i64,
    kind: &str,
) -> Result<sqlx::postgres::PgRow> {
    let r=sqlx::query("SELECT p.*,s.selected_email_accounts,s.selected_domains FROM subscriptions s JOIN packages p ON p.id=s.package_id WHERE s.customer_id=$1 AND s.status='active' AND current_date BETWEEN s.start_date AND s.end_date ORDER BY s.id DESC LIMIT 1").bind(owner).fetch_optional(&mut **tx).await?.ok_or_else(||Error::bad("Chưa có gói đang hoạt động"))?;
    let (table, limit) = match kind {
        "domains" => ("domains", r.get::<i32, _>("selected_domains")),
        "accounts" => ("email_accounts", r.get::<i32, _>("selected_email_accounts")),
        "subadmins" => ("sub_admins", r.get::<i32, _>("max_admin_users")),
        _ => ("email_groups", i32::MAX),
    };
    let count: i64 = sqlx::query_scalar(&format!(
        "SELECT count(*) FROM {table} WHERE customer_id=$1 AND status<>'deleted'"
    ))
    .bind(owner)
    .fetch_one(&mut **tx)
    .await?;
    if count >= i64::from(limit) {
        return Err(Error::bad("Đã đạt hạn mức của gói"));
    }
    Ok(r)
}
pub async fn save(
    State(s): State<App>,
    h: HeaderMap,
    Path(kind): Path<String>,
    Json(v): Json<Value>,
) -> Result<Json<Value>> {
    let (_, perm, admin) = resource(&kind)?;
    let u = auth::require(&s, &h, perm, admin, true).await?;
    let action = v["action"].as_str().unwrap_or("create");
    let mut tx = s.db.begin().await?;
    let owner = if u.role == "admin" {
        v["customer_id"].as_i64().unwrap_or(u.id)
    } else {
        u.owner
    };
    let mut id = v["id"].as_i64().unwrap_or(0);
    match kind.as_str() {
        "packages" => {
            let mut data = v.clone();
            data.as_object_mut()
                .ok_or_else(|| Error::bad("JSON object required"))?
                .remove("id");
            let allowed = [
                "name",
                "price",
                "promo_price",
                "billing_months",
                "duration_days",
                "min_email_accounts",
                "max_email_accounts",
                "min_domains",
                "max_domains",
                "extra_domain_price",
                "max_admin_users",
                "storage_per_account_mb",
                "allow_alias",
                "allow_sub_admin",
                "status",
            ];
            id = write_record(&mut tx, "packages", id, &data, &allowed).await?;
        }
        "customers" | "admins" => {
            if kind == "admins" {
                u.super_admin()?
            }
            if action == "create" {
                let email = text(&v, "email", 190)?.to_lowercase();
                if !auth::valid_email(&email) {
                    return Err(Error::bad("Email không hợp lệ"));
                }
                let hash = crypto::password(text(&v, "password", 256)?)?;
                id=sqlx::query_scalar("INSERT INTO users(name,email,password_hash,role,admin_level,admin_permissions,two_factor_enabled) VALUES($1,$2,$3,$4,$5,$6,0) RETURNING id").bind(text(&v,"name",190)?).bind(email).bind(hash).bind(if kind=="admins"{"admin"}else{"customer"}).bind(if kind=="admins"{Some("manager")}else{None}).bind(v.get("permissions").cloned().unwrap_or(json!([]))).fetch_one(&mut *tx).await?;
            } else {
                if id == u.id {
                    return Err(Error::bad("Không thể khóa tài khoản của chính mình"));
                }
                let status = text(&v, "status", 20)?;
                if !["active", "disabled", "deleted"].contains(&status) {
                    return Err(Error::bad("Trạng thái không hợp lệ"));
                }
                let role = if kind == "admins" {
                    "admin"
                } else {
                    "customer"
                };
                let affected = sqlx::query(
                    "UPDATE users SET status=$1,updated_at=now() WHERE id=$2 AND role=$3",
                )
                .bind(status)
                .bind(id)
                .bind(role)
                .execute(&mut *tx)
                .await?
                .rows_affected();
                if affected == 0 {
                    return Err(Error::missing());
                }
                sqlx::query("DELETE FROM panel_sessions WHERE user_id=$1 OR user_id IN(SELECT id FROM users WHERE parent_customer_id=$1)").bind(id).execute(&mut *tx).await?;
            }
        }
        "domains" => {
            owner_lock(&mut tx, owner).await?;
            if action == "create" {
                if u.role == "sub_admin" {
                    return Err(Error::forbidden());
                }
                plan(&mut tx, owner, "domains").await?;
                let name = text(&v, "domain_name", 190)?.to_lowercase();
                if !domain_name(&name) {
                    return Err(Error::bad("Domain không hợp lệ; dùng tên ASCII/Punycode"));
                }
                let server: i64 = if let Some(n) = v["server_id"].as_i64() {
                    if u.role != "admin" {
                        return Err(Error::forbidden());
                    }
                    sqlx::query_scalar("SELECT id FROM stalwart_servers WHERE id=$1 AND active=1")
                        .bind(n)
                        .fetch_optional(&mut *tx)
                        .await?
                        .ok_or_else(Error::missing)?
                } else {
                    sqlx::query_scalar(
                        "SELECT id FROM stalwart_servers WHERE is_primary=1 AND active=1",
                    )
                    .fetch_optional(&mut *tx)
                    .await?
                    .ok_or_else(|| Error::bad("Chưa cấu hình server chính"))?
                };
                id=sqlx::query_scalar("INSERT INTO domains(customer_id,domain_name,server_id) VALUES($1,$2,$3) RETURNING id").bind(owner).bind(&name).bind(server).fetch_one(&mut *tx).await?;
                enqueue(
                    &s,
                    &mut tx,
                    server,
                    "domain_create",
                    id,
                    json!({"name":name}),
                )
                .await?;
            } else {
                scope(&mut tx, &u, id, None).await?;
                let d=sqlx::query("SELECT * FROM domains WHERE id=$1 AND customer_id=$2 AND status<>'deleted' FOR UPDATE").bind(id).bind(owner).fetch_optional(&mut *tx).await?.ok_or_else(Error::missing)?;
                let remote = d
                    .get::<Option<String>, _>("stalwart_domain_id")
                    .ok_or_else(|| Error::bad("Chờ domain đồng bộ trước"))?;
                let server = d
                    .get::<Option<i64>, _>("server_id")
                    .ok_or_else(|| Error::bad("Domain chưa được gán server"))?;
                if action == "delete" {
                    let n:i64=sqlx::query_scalar("SELECT count(*) FROM email_accounts WHERE domain_id=$1 AND status<>'deleted'").bind(id).fetch_one(&mut *tx).await?;
                    if n > 0 {
                        return Err(Error::bad("Xóa các hộp thư trước"));
                    }
                    sqlx::query("UPDATE domains SET status='deleted',sync_status='pending',deleted_at=now() WHERE id=$1").bind(id).execute(&mut *tx).await?;
                    enqueue(
                        &s,
                        &mut tx,
                        server,
                        "domain_delete",
                        id,
                        json!({"remote_id":remote}),
                    )
                    .await?;
                } else if action == "status" {
                    let status = text(&v, "status", 20)?;
                    if !["active", "disabled"].contains(&status) {
                        return Err(Error::bad("Trạng thái không hợp lệ"));
                    }
                    sqlx::query("UPDATE domains SET status=$1,sync_status='pending',updated_at=now() WHERE id=$2").bind(status).bind(id).execute(&mut *tx).await?;
                    enqueue(
                        &s,
                        &mut tx,
                        server,
                        "domain_status",
                        id,
                        json!({"remote_id":remote,"enabled":status=="active"}),
                    )
                    .await?;
                } else {
                    return Err(Error::bad("Thao tác domain không hợp lệ"));
                }
            }
        }
        "accounts" => {
            owner_lock(&mut tx, owner).await?;
            if action == "create" {
                let p = plan(&mut tx, owner, "accounts").await?;
                let domain = number(&v, "domain_id")?;
                let group = number(&v, "group_id")?;
                scope(&mut tx, &u, domain, Some(group)).await?;
                let d=sqlx::query("SELECT domain_name,stalwart_domain_id,server_id FROM domains WHERE id=$1 AND customer_id=$2 AND status='active' AND sync_status='synced'").bind(domain).bind(owner).fetch_optional(&mut *tx).await?.ok_or_else(||Error::bad("Domain chưa sẵn sàng"))?;
                let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM email_groups WHERE id=$1 AND customer_id=$2 AND status='active')").bind(group).bind(owner).fetch_one(&mut *tx).await?;
                if !exists {
                    return Err(Error::bad("Nhóm không hợp lệ"));
                }
                let local = text(&v, "local_part", 100)?.to_lowercase();
                if !local
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-+".contains(&b))
                {
                    return Err(Error::bad("Tên hộp thư không hợp lệ"));
                }
                let password = text(&v, "password", 256)?;
                if password.len() < 12 {
                    return Err(Error::bad("Mật khẩu tối thiểu 12 ký tự"));
                }
                let email = format!("{local}@{}", d.get::<String, _>("domain_name"));
                let conflict:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM email_aliases WHERE alias_email=$1 AND status<>'deleted')").bind(&email).fetch_one(&mut *tx).await?;
                if conflict {
                    return Err(Error::conflict("Địa chỉ đã dùng làm alias"));
                }
                let server: i64 = d
                    .get::<Option<i64>, _>("server_id")
                    .ok_or_else(Error::missing)?;
                let quota: i32 = p.get("storage_per_account_mb");
                id=sqlx::query_scalar("INSERT INTO email_accounts(customer_id,domain_id,group_id,email,local_part,display_name,storage_limit_mb,server_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8) RETURNING id").bind(owner).bind(domain).bind(group).bind(email).bind(&local).bind(v["display_name"].as_str().unwrap_or("")).bind(quota).bind(server).fetch_one(&mut *tx).await?;
                enqueue(&s,&mut tx,server,"account_create",id,json!({"local":local,"domain_id":d.get::<String,_>("stalwart_domain_id"),"password":password,"quota":i64::from(quota)*1048576})).await?;
            } else {
                let a=sqlx::query("SELECT * FROM email_accounts WHERE id=$1 AND customer_id=$2 AND status<>'deleted' FOR UPDATE").bind(id).bind(owner).fetch_optional(&mut *tx).await?.ok_or_else(Error::missing)?;
                scope(&mut tx, &u, a.get("domain_id"), a.get("group_id")).await?;
                let remote = a
                    .get::<Option<String>, _>("stalwart_account_id")
                    .ok_or_else(|| Error::bad("Chờ hộp thư đồng bộ trước"))?;
                let server = a
                    .get::<Option<i64>, _>("server_id")
                    .ok_or_else(Error::missing)?;
                match action {
                    "delete" => {
                        sqlx::query("UPDATE email_accounts SET status='deleted',deleted_at=now(),sync_status='pending' WHERE id=$1").bind(id).execute(&mut *tx).await?;
                        sqlx::query(
                            "UPDATE email_aliases SET status='deleted' WHERE email_account_id=$1",
                        )
                        .bind(id)
                        .execute(&mut *tx)
                        .await?;
                        enqueue(
                            &s,
                            &mut tx,
                            server,
                            "account_delete",
                            id,
                            json!({"remote_id":remote}),
                        )
                        .await?;
                    }
                    "password" => {
                        let password = text(&v, "password", 256)?;
                        if password.len() < 12 {
                            return Err(Error::bad("Mật khẩu tối thiểu 12 ký tự"));
                        }
                        enqueue(
                            &s,
                            &mut tx,
                            server,
                            "account_password",
                            id,
                            json!({"remote_id":remote,"password":password}),
                        )
                        .await?;
                    }
                    "status" => {
                        let status = text(&v, "status", 20)?;
                        if !["active", "disabled"].contains(&status) {
                            return Err(Error::bad("Trạng thái không hợp lệ"));
                        }
                        let password = if status == "active" {
                            let p = text(&v, "password", 256)?;
                            if p.len() < 12 {
                                return Err(Error::bad("Cần mật khẩu mới khi bật lại hộp thư"));
                            }
                            Some(p)
                        } else {
                            None
                        };
                        sqlx::query(
                            "UPDATE email_accounts SET status=$1,sync_status='pending' WHERE id=$2",
                        )
                        .bind(status)
                        .bind(id)
                        .execute(&mut *tx)
                        .await?;
                        enqueue(
                            &s,
                            &mut tx,
                            server,
                            "account_status",
                            id,
                            json!({"remote_id":remote,"password":password}),
                        )
                        .await?;
                    }
                    "update" => {
                        sqlx::query("UPDATE email_accounts SET display_name=$1,updated_at=now() WHERE id=$2").bind(text(&v,"display_name",190)?).bind(id).execute(&mut *tx).await?;
                    }
                    _ => return Err(Error::bad("Thao tác hộp thư không hợp lệ")),
                }
            }
        }
        "groups" => {
            if u.role == "sub_admin" {
                return Err(Error::forbidden());
            }
            owner_lock(&mut tx, owner).await?;
            if action == "create" {
                id = sqlx::query_scalar(
                    "INSERT INTO email_groups(customer_id,name) VALUES($1,$2) RETURNING id",
                )
                .bind(owner)
                .bind(text(&v, "name", 190)?)
                .fetch_one(&mut *tx)
                .await?;
            } else {
                let count: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM email_accounts WHERE group_id=$1 AND status<>'deleted'",
                )
                .bind(id)
                .fetch_one(&mut *tx)
                .await?;
                if action == "delete" && count > 0 {
                    return Err(Error::bad("Nhóm còn hộp thư"));
                }
                let affected = if action == "delete" {
                    sqlx::query(
                        "UPDATE email_groups SET status='deleted' WHERE id=$1 AND customer_id=$2",
                    )
                    .bind(id)
                    .bind(owner)
                    .execute(&mut *tx)
                    .await?
                } else {
                    sqlx::query("UPDATE email_groups SET name=$1 WHERE id=$2 AND customer_id=$3")
                        .bind(text(&v, "name", 190)?)
                        .bind(id)
                        .bind(owner)
                        .execute(&mut *tx)
                        .await?
                };
                if affected.rows_affected() == 0 {
                    return Err(Error::missing());
                }
            }
        }
        "aliases" => {
            if u.role == "sub_admin" {
                return Err(Error::forbidden());
            }
            owner_lock(&mut tx, owner).await?;
            let account = number(&v, "email_account_id")?;
            let a=sqlx::query("SELECT * FROM email_accounts WHERE id=$1 AND customer_id=$2 AND status='active' AND sync_status='synced'").bind(account).bind(owner).fetch_optional(&mut *tx).await?.ok_or_else(Error::missing)?;
            if action == "create" {
                let p = plan(&mut tx, owner, "groups").await?;
                if p.get::<i16, _>("allow_alias") == 0 {
                    return Err(Error::forbidden());
                }
                let domain = number(&v, "domain_id")?;
                let d=sqlx::query("SELECT * FROM domains WHERE id=$1 AND customer_id=$2 AND status='active' AND sync_status='synced'").bind(domain).bind(owner).fetch_optional(&mut *tx).await?.ok_or_else(Error::missing)?;
                if d.get::<Option<i64>, _>("server_id") != a.get::<Option<i64>, _>("server_id") {
                    return Err(Error::bad("Alias phải cùng server"));
                }
                let local = text(&v, "local_part", 100)?.to_lowercase();
                if !local
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-+".contains(&b))
                {
                    return Err(Error::bad("Alias không hợp lệ"));
                }
                let email = format!("{local}@{}", d.get::<String, _>("domain_name"));
                let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM email_accounts WHERE email=$1 AND status<>'deleted')").bind(&email).fetch_one(&mut *tx).await?;
                if exists {
                    return Err(Error::conflict("Địa chỉ đã dùng"));
                }
                id=sqlx::query_scalar("INSERT INTO email_aliases(email_account_id,customer_id,domain_id,alias_email,local_part,description) VALUES($1,$2,$3,$4,$5,$6) RETURNING id").bind(account).bind(owner).bind(domain).bind(email).bind(local).bind(v["description"].as_str().unwrap_or("")).fetch_one(&mut *tx).await?;
            } else {
                let status = if action == "delete" {
                    "deleted"
                } else {
                    text(&v, "status", 20)?
                };
                sqlx::query("UPDATE email_aliases SET status=$1,sync_status='pending' WHERE id=$2 AND customer_id=$3 AND email_account_id=$4").bind(status).bind(id).bind(owner).bind(account).execute(&mut *tx).await?;
            }
            let aliases:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('name',a.local_part,'domainId',d.stalwart_domain_id,'enabled',a.status='active','description',COALESCE(a.description,'')) FROM email_aliases a JOIN domains d ON d.id=a.domain_id WHERE a.email_account_id=$1 AND a.status<>'deleted'").bind(account).fetch_all(&mut *tx).await?;
            let map: serde_json::Map<String, Value> = aliases
                .into_iter()
                .enumerate()
                .map(|(i, v)| (i.to_string(), v))
                .collect();
            enqueue(
                &s,
                &mut tx,
                a.get::<Option<i64>, _>("server_id")
                    .ok_or_else(Error::missing)?,
                "account_aliases",
                account,
                json!({"remote_id":a.get::<String,_>("stalwart_account_id"),"aliases":map}),
            )
            .await?;
        }
        "subadmins" => {
            if u.role == "sub_admin" {
                return Err(Error::forbidden());
            }
            owner_lock(&mut tx, owner).await?;
            if action == "create" {
                let p = plan(&mut tx, owner, "subadmins").await?;
                if p.get::<i16, _>("allow_sub_admin") == 0 {
                    return Err(Error::forbidden());
                }
                let email = text(&v, "email", 190)?.to_lowercase();
                if !auth::valid_email(&email) {
                    return Err(Error::bad("Email không hợp lệ"));
                }
                let uid:i64=sqlx::query_scalar("INSERT INTO users(name,email,password_hash,role,parent_customer_id,two_factor_enabled) VALUES($1,$2,$3,'sub_admin',$4,0) RETURNING id").bind(text(&v,"name",190)?).bind(email).bind(crypto::password(text(&v,"password",256)?)?).bind(owner).fetch_one(&mut *tx).await?;
                let perms = v
                    .get("permissions")
                    .cloned()
                    .unwrap_or(json!(["accounts", "domains", "groups"]));
                id=sqlx::query_scalar("INSERT INTO sub_admins(customer_id,user_id,permissions) VALUES($1,$2,$3) RETURNING id").bind(owner).bind(uid).bind(perms).fetch_one(&mut *tx).await?;
                for (key, table, col) in [
                    ("domain_ids", "sub_admin_domain_access", "domain_id"),
                    ("group_ids", "sub_admin_group_access", "group_id"),
                ] {
                    for rid in v[key].as_array().into_iter().flatten() {
                        let rid = rid.as_i64().ok_or_else(|| Error::bad("ID không hợp lệ"))?;
                        let resource = if key == "domain_ids" {
                            "domains"
                        } else {
                            "email_groups"
                        };
                        let belongs:bool=sqlx::query_scalar(&format!("SELECT EXISTS(SELECT 1 FROM {resource} WHERE id=$1 AND customer_id=$2 AND status='active')")).bind(rid).bind(owner).fetch_one(&mut *tx).await?;
                        if !belongs {
                            return Err(Error::forbidden());
                        }
                        sqlx::query(&format!(
                            "INSERT INTO {table}(sub_admin_id,user_id,{col}) VALUES($1,$2,$3)"
                        ))
                        .bind(id)
                        .bind(uid)
                        .bind(rid)
                        .execute(&mut *tx)
                        .await?;
                    }
                }
            } else {
                let uid:i64=sqlx::query_scalar("UPDATE sub_admins SET status='deleted' WHERE id=$1 AND customer_id=$2 RETURNING user_id").bind(id).bind(owner).fetch_optional(&mut *tx).await?.ok_or_else(Error::missing)?;
                sqlx::query("UPDATE users SET status='deleted' WHERE id=$1")
                    .bind(uid)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        "servers" => {
            u.super_admin()?;
            let mut data = v.clone();
            let obj = data
                .as_object_mut()
                .ok_or_else(|| Error::bad("JSON object required"))?;
            if let Some(token) = obj.remove("token") {
                let token = token
                    .as_str()
                    .ok_or_else(|| Error::bad("Token không hợp lệ"))?;
                if !token.is_empty() {
                    obj.insert(
                        "token_encrypted".into(),
                        json!(crypto::seal(&s.config.key, token)?),
                    );
                }
            }
            if let Some(base) = obj.get("base_url") {
                let url = url::Url::parse(base.as_str().unwrap_or(""))
                    .map_err(|_| Error::bad("URL không hợp lệ"))?;
                if !["http", "https"].contains(&url.scheme())
                    || !url.username().is_empty()
                    || url.password().is_some()
                    || url.query().is_some()
                    || url.fragment().is_some()
                {
                    return Err(Error::bad("URL server không hợp lệ"));
                }
            }
            if obj.get("is_primary") == Some(&json!(1)) {
                sqlx::query("UPDATE stalwart_servers SET is_primary=0 WHERE is_primary=1")
                    .execute(&mut *tx)
                    .await?;
            }
            id = write_record(
                &mut tx,
                "stalwart_servers",
                id,
                &data,
                &[
                    "name",
                    "base_url",
                    "token_encrypted",
                    "dry_run",
                    "active",
                    "is_primary",
                ],
            )
            .await?;
            sqlx::query("UPDATE stalwart_servers SET config_version=config_version+1 WHERE id=$1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        "jobs" => {
            u.super_admin()?;
            if action != "retry" {
                return Err(Error::bad("Chỉ hỗ trợ retry"));
            }
            sqlx::query("UPDATE api_sync_jobs j SET status='pending',attempts=0,run_after=now(),last_error=NULL,server_version=s.config_version FROM stalwart_servers s WHERE j.id=$1 AND j.status='failed' AND s.id=j.server_id AND s.active=1").bind(id).execute(&mut *tx).await?;
        }
        "incidents" => {
            let mut data = v.clone();
            if id == 0 {
                data["created_by"] = json!(u.id);
                data["customer_id"] = json!(owner);
            }
            id = write_record(
                &mut tx,
                "support_incidents",
                id,
                &data,
                &[
                    "customer_id",
                    "subject",
                    "severity",
                    "status",
                    "root_cause",
                    "resolution",
                    "assigned_to",
                    "created_by",
                ],
            )
            .await?;
        }
        "alerts" => {
            sqlx::query("UPDATE operational_alerts SET status='acknowledged',acknowledged_by=$1,acknowledged_at=now() WHERE id=$2").bind(u.id).bind(id).execute(&mut *tx).await?;
        }
        _ => {
            return Err(Error::bad(
                "Tài nguyên này chỉ đọc hoặc có API nghiệp vụ riêng",
            ))
        }
    }
    tx.commit().await?;
    auth::audit(
        &s,
        Some(u.id),
        "resource_change",
        json!({"kind":kind,"action":action,"id":id}),
    )
    .await?;
    Ok(Json(json!({"ok":true,"id":id})))
}
// Identifiers come only from the static allowlists above. Values always use a bound JSON parameter.
async fn write_record(
    tx: &mut Transaction<'_, Postgres>,
    table: &str,
    id: i64,
    v: &Value,
    allowed: &[&str],
) -> Result<i64> {
    let obj = v
        .as_object()
        .ok_or_else(|| Error::bad("JSON object required"))?;
    let cols: Vec<&str> = allowed
        .iter()
        .copied()
        .filter(|c| obj.contains_key(*c))
        .collect();
    if cols.is_empty() {
        return Err(Error::bad("Không có trường cần lưu"));
    }
    let q = if id == 0 {
        format!("INSERT INTO {table} ({}) SELECT {} FROM jsonb_populate_record(NULL::{table},$1) r RETURNING id",cols.join(","),cols.iter().map(|c|format!("r.{c}")).collect::<Vec<_>>().join(","))
    } else {
        format!("UPDATE {table} t SET {} FROM jsonb_populate_record(NULL::{table},$1) r WHERE t.id=$2 RETURNING t.id",cols.iter().map(|c|format!("{c}=r.{c}")).collect::<Vec<_>>().join(","))
    };
    let query = sqlx::query_scalar(&q).bind(v);
    let query = if id == 0 { query } else { query.bind(id) };
    query
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(Error::missing)
}
pub async fn dashboard(State(s): State<App>, h: HeaderMap) -> Result<Json<Value>> {
    let u = auth::user(&s, &h, true).await?;
    let all = u.role == "admin" && u.can("customers");
    let result:Value=sqlx::query_scalar("SELECT jsonb_build_object('domains',(SELECT count(*) FROM domains WHERE status<>'deleted' AND ($2 OR customer_id=$1)),'accounts',(SELECT count(*) FROM email_accounts WHERE status<>'deleted' AND ($2 OR customer_id=$1)),'subscriptions',(SELECT count(*) FROM subscriptions WHERE status='active' AND ($2 OR customer_id=$1)),'pending_invoices',(SELECT count(*) FROM subscriptions WHERE status='pending' AND ($2 OR customer_id=$1)))").bind(u.owner).bind(all).fetch_one(&s.db).await?;
    Ok(Json(result))
}
