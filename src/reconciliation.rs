use crate::{
    auth, crypto,
    error::{Error, Result},
    management, resources, stalwart, App,
};
use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    Json,
};
use serde_json::{json, Value};
use sqlx::{PgConnection, Row};

fn normalized_aliases(value: &Value) -> Result<Value> {
    let rows: Vec<&Value> = if let Some(a) = value.as_array() {
        a.iter().collect()
    } else if let Some(o) = value.as_object() {
        o.values().collect()
    } else {
        return Err(Error::bad("Schema alias không tương thích"));
    };
    if rows.len() > 1000 {
        return Err(Error::bad("Quá nhiều alias"));
    }
    let mut out = Vec::new();
    for row in rows {
        let name = row["name"]
            .as_str()
            .ok_or_else(|| Error::bad("Alias thiếu tên"))?;
        let domain = row["domainId"]
            .as_str()
            .ok_or_else(|| Error::bad("Alias thiếu domain"))?;
        if row.get("enabled").is_some_and(|v| !v.is_boolean()) {
            return Err(Error::bad("Alias enabled không hợp lệ"));
        }
        out.push(json!({"name":name,"domainId":domain,"enabled":row["enabled"].as_bool().unwrap_or(true),"description":row["description"].as_str().unwrap_or("")}));
    }
    out.sort_by_key(Value::to_string);
    Ok(json!(out))
}
fn projection(kind: &str, row: &Value) -> Result<Value> {
    let name = row["name"]
        .as_str()
        .ok_or_else(|| Error::bad("Schema remote thiếu tên"))?;
    if kind == "domain" {
        Ok(
            json!({"name":name,"isEnabled":row["isEnabled"].as_bool().ok_or_else(||Error::bad("Schema Domain không tương thích"))?}),
        )
    } else {
        Ok(
            json!({"name":name,"domainId":row["domainId"].as_str().ok_or_else(||Error::bad("Account thiếu domain"))?,"maxDiskQuota":row["quotas"]["maxDiskQuota"].as_i64().filter(|n|*n>=0).ok_or_else(||Error::bad("Account thiếu quota"))?,"aliases":normalized_aliases(&row["aliases"])?}),
        )
    }
}
async fn local(db: &mut PgConnection, kind: &str, id: i64) -> Result<Value> {
    let value: Option<Value> = if kind == "domain" {
        sqlx::query_scalar("SELECT jsonb_build_object('id',d.id,'owner',d.customer_id,'server',d.server_id,'remote',d.stalwart_domain_id,'label',d.domain_name,'status',d.status,'customer_status',u.status,'desired',jsonb_build_object('name',d.domain_name,'isEnabled',d.status='active' AND u.status='active'),'eligible',d.sync_status='synced') FROM domains d JOIN users u ON u.id=d.customer_id WHERE d.id=$1 AND d.status<>'deleted'").bind(id).fetch_optional(&mut *db).await?
    } else {
        sqlx::query_scalar("SELECT jsonb_build_object('id',a.id,'owner',a.customer_id,'server',a.server_id,'remote',a.stalwart_account_id,'label',a.email,'status',a.status,'customer_status',u.status,'domain_status',d.status,'desired',jsonb_build_object('name',a.local_part,'domainId',d.stalwart_domain_id,'maxDiskQuota',a.storage_limit_mb::bigint*1048576,'aliases',(SELECT COALESCE(jsonb_agg(jsonb_build_object('name',x.local_part,'domainId',xd.stalwart_domain_id,'enabled',x.status='active','description',COALESCE(x.description,'')) ORDER BY x.id),'[]'::jsonb) FROM email_aliases x JOIN domains xd ON xd.id=x.domain_id WHERE x.email_account_id=a.id AND x.status<>'deleted')),'eligible',a.status='active' AND u.status='active' AND d.status='active' AND a.sync_status='synced' AND d.sync_status='synced') FROM email_accounts a JOIN domains d ON d.id=a.domain_id JOIN users u ON u.id=a.customer_id WHERE a.id=$1 AND a.status<>'deleted'").bind(id).fetch_optional(&mut *db).await?
    };
    let mut value = value.ok_or_else(Error::missing)?;
    if kind == "account" {
        value["desired"]["aliases"] = normalized_aliases(&value["desired"]["aliases"])?;
    }
    let hash = crypto::hash(&value.to_string());
    value["hash"] = json!(hash);
    Ok(value)
}
fn patch(kind: &str, before: &Value, desired: &Value) -> Result<Value> {
    if kind == "domain" {
        if before["name"] != desired["name"] {
            return Err(Error::conflict(
                "Domain không khớp tên; kiểm tra mapping thủ công",
            ));
        }
        return Ok(json!({"isEnabled":desired["isEnabled"]}));
    }
    if before["domainId"] != desired["domainId"] {
        return Err(Error::conflict(
            "Account không khớp domain; kiểm tra mapping thủ công",
        ));
    }
    let mut out = json!({});
    if before["name"] != desired["name"] {
        out["name"] = desired["name"].clone();
    }
    if before["maxDiskQuota"] != desired["maxDiskQuota"] {
        out["quotas/maxDiskQuota"] = desired["maxDiskQuota"].clone();
    }
    if before["aliases"] != desired["aliases"] {
        out["aliases"] = Value::Object(
            desired["aliases"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
                .map(|(i, v)| (i.to_string(), v.clone()))
                .collect(),
        );
    }
    Ok(out)
}
async fn inventory(
    s: &App,
    server: i64,
    kind: &str,
) -> Result<std::collections::BTreeMap<String, Value>> {
    let mut all = std::collections::BTreeMap::new();
    let mut total = None;
    for offset in (0..5000_i64).step_by(50) {
        let ty = if kind == "domain" {
            "Domain"
        } else {
            "Account"
        };
        let query = management::method(
            s,
            server,
            &format!("x:{ty}/query"),
            json!({"position":offset,"limit":50,"calculateTotal":true}),
        )
        .await?;
        let ids = query["ids"]
            .as_array()
            .filter(|a| {
                a.len() <= 50
                    && a.iter()
                        .all(|id| id.as_str().is_some_and(|s| !s.is_empty() && s.len() <= 190))
            })
            .ok_or_else(|| Error::bad("Inventory ID không hợp lệ"))?;
        let properties = if kind == "domain" {
            json!(["id", "name", "isEnabled"])
        } else {
            json!([
                "id",
                "name",
                "domainId",
                "quotas",
                "aliases",
                "usedDiskQuota"
            ])
        };
        let rows = if ids.is_empty() {
            json!([])
        } else {
            management::method(
                s,
                server,
                &format!("x:{ty}/get"),
                json!({"ids":ids,"properties":properties}),
            )
            .await?["list"]
                .clone()
        };
        let rows = rows
            .as_array()
            .filter(|a| a.len() == ids.len() && a.iter().all(|row| ids.contains(&row["id"])))
            .ok_or_else(|| Error::conflict("Inventory get thiếu object hoặc ngoài yêu cầu"))?;
        let page = json!({"total":query["total"],"items":rows,"next":if ids.len()==50{Some(offset+50)}else{None}});
        let count = page["total"]
            .as_u64()
            .filter(|n| *n <= 5000)
            .ok_or_else(|| Error::bad("Inventory thiếu total hoặc vượt 5000"))?;
        if total.is_some_and(|n| n != count) {
            return Err(Error::conflict("Inventory thay đổi; xem trước lại"));
        }
        total = Some(count);
        for row in page["items"].as_array().unwrap() {
            let id = row["id"].as_str().unwrap().to_owned();
            if all.insert(id, row.clone()).is_some() {
                return Err(Error::conflict("Inventory lặp ID"));
            }
        }
        if page["next"].is_null() {
            if all.len() as u64 != count {
                return Err(Error::conflict("Inventory chưa đủ object"));
            }
            return Ok(all);
        }
    }
    Err(Error::bad("Inventory vượt giới hạn"))
}
pub async fn preview(
    State(s): State<App>,
    h: HeaderMap,
    Json(v): Json<Value>,
) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "operations", true, true).await?;
    u.super_admin()?;
    let server = resources::number(&v, "server_id")?;
    let mut tx = s.db.begin().await?;
    let row = sqlx::query(
        "SELECT config_version,dry_run FROM stalwart_servers WHERE id=$1 AND active=1 FOR SHARE",
    )
    .bind(server)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(Error::missing)?;
    let dry = row.get::<i16, _>("dry_run") == 1;
    let run:i64=sqlx::query_scalar("INSERT INTO reconciliation_runs(server_id,server_version,mode,status,actor_id) VALUES($1,$2,$3,'running',$4) RETURNING id").bind(server).bind(row.get::<i64,_>("config_version")).bind(if dry{"dry_run"}else{"live"}).bind(u.id).fetch_one(&mut *tx).await?;
    let mut summary = json!({"equal":0,"diff":0,"manual":0});
    for kind in ["domain", "account"] {
        let mut remote = if dry {
            std::collections::BTreeMap::new()
        } else {
            inventory(&s, server, kind).await?
        };
        let table = if kind == "domain" {
            "domains"
        } else {
            "email_accounts"
        };
        let ids:Vec<i64>=sqlx::query_scalar(&format!("SELECT id FROM {table} WHERE server_id=$1 AND status<>'deleted' ORDER BY id LIMIT 5001")).bind(server).fetch_all(&mut *tx).await?;
        if ids.len() > 5000 {
            return Err(Error::bad("Panel vượt 5000 tài nguyên mỗi loại"));
        }
        for id in ids {
            let current = local(&mut tx, kind, id).await?;
            let remote_id = current["remote"].as_str().unwrap_or("");
            let row = remote.remove(remote_id);
            let before = row.as_ref().and_then(|row| projection(kind, row).ok());
            let (status, detail) = if dry {
                ("manual", Some("Dry-run: chưa đọc inventory server thật"))
            } else if before.as_ref() == Some(&current["desired"]) {
                ("equal", None)
            } else if current["eligible"] == true
                && before
                    .as_ref()
                    .is_some_and(|b| patch(kind, b, &current["desired"]).is_ok())
            {
                ("diff", None)
            } else {
                (
                    "manual",
                    Some("Thiếu mapping, schema không khớp hoặc tài nguyên chưa đủ điều kiện"),
                )
            };
            summary[status] = json!(summary[status].as_i64().unwrap() + 1);
            sqlx::query("INSERT INTO reconciliation_items(run_id,kind,local_id,remote_id,label,local_hash,before_data,desired_data,status,detail) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)").bind(run).bind(kind).bind(id).bind(remote_id).bind(current["label"].as_str().unwrap()).bind(current["hash"].as_str().unwrap()).bind(before).bind(&current["desired"]).bind(status).bind(detail).execute(&mut *tx).await?;
        }
        for (id, row) in remote {
            summary["manual"] = json!(summary["manual"].as_i64().unwrap() + 1);
            sqlx::query("INSERT INTO reconciliation_items(run_id,kind,remote_id,label,status,detail) VALUES($1,$2,$3,$4,'manual','Object chỉ có trên remote; không tự nhận quyền quản lý hoặc xóa')").bind(run).bind(kind).bind(&id).bind(row["name"].as_str().unwrap_or(&id).chars().take(190).collect::<String>()).execute(&mut *tx).await?;
        }
    }
    sqlx::query(
        "UPDATE reconciliation_runs SET status='ready',summary=$1,finished_at=now() WHERE id=$2",
    )
    .bind(&summary)
    .bind(run)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    auth::audit(
        &s,
        Some(u.id),
        "reconciliation_preview",
        json!({"run_id":run,"server_id":server}),
    )
    .await?;
    Ok(Json(json!({"run_id":run,"summary":summary})))
}
#[derive(serde::Deserialize)]
pub struct Page {
    #[serde(default)]
    offset: i64,
}
pub async fn list(
    State(s): State<App>,
    h: HeaderMap,
    Path(id): Path<i64>,
    Query(page): Query<Page>,
) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "operations", true, false).await?;
    u.super_admin()?;
    let run: Value = sqlx::query_scalar(
        "SELECT to_jsonb(r) FROM reconciliation_runs r WHERE id=$1 AND actor_id=$2",
    )
    .bind(id)
    .bind(u.id)
    .fetch_optional(&s.db)
    .await?
    .ok_or_else(Error::missing)?;
    if !(0..=20000).contains(&page.offset) {
        return Err(Error::bad("Trang không hợp lệ"));
    }
    let total: i64 =
        sqlx::query_scalar("SELECT count(*) FROM reconciliation_items WHERE run_id=$1")
            .bind(id)
            .fetch_one(&s.db)
            .await?;
    let items:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(i) FROM reconciliation_items i WHERE run_id=$1 ORDER BY id LIMIT 100 OFFSET $2").bind(id).bind(page.offset).fetch_all(&s.db).await?;
    Ok(Json(
        json!({"run":run,"items":items,"total":total,"next":if page.offset+100<total {Some(page.offset+100)}else{None}}),
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
    if v["confirmation"] != "RECONCILE" {
        return Err(Error::bad("Nhập RECONCILE để áp dụng"));
    }
    let ids = v["item_ids"]
        .as_array()
        .filter(|a| !a.is_empty() && a.len() <= 50)
        .ok_or_else(|| Error::bad("Chọn 1 đến 50 thay đổi"))?;
    let mut seen = std::collections::HashSet::new();
    let mut selected = Vec::new();
    for item in ids {
        let item = item
            .as_i64()
            .filter(|n| *n > 0 && seen.insert(*n))
            .ok_or_else(|| Error::bad("ID không hợp lệ hoặc trùng"))?;
        selected.push(item);
    }
    let run=sqlx::query("SELECT * FROM reconciliation_runs WHERE id=$1 AND actor_id=$2 AND mode='live' AND status='ready' AND created_at>now()-interval '30 minutes'").bind(id).bind(u.id).fetch_optional(&s.db).await?.ok_or_else(||Error::conflict("Bản đối chiếu đã cũ hoặc không thuộc người tạo"))?;
    let server: i64 = run.get("server_id");
    let version: i64 = run.get("server_version");
    let mut summary = json!({"applied":0,"stale":0,"uncertain":0,"skipped":0});
    for item in selected {
        let mut tx = s.db.begin().await?;
        let row=sqlx::query("SELECT * FROM reconciliation_items WHERE id=$1 AND run_id=$2 AND status='diff' FOR UPDATE").bind(item).bind(id).fetch_optional(&mut *tx).await?;
        let Some(row) = row else {
            summary["skipped"] = json!(summary["skipped"].as_i64().unwrap() + 1);
            continue;
        };
        sqlx::query("UPDATE reconciliation_items SET status='applying' WHERE id=$1")
            .bind(item)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        let mut tx = s.db.begin().await?;
        sqlx::query("SELECT id FROM reconciliation_items WHERE id=$1 FOR UPDATE")
            .bind(item)
            .fetch_one(&mut *tx)
            .await?;
        let mut wrote = false;
        let result:Result<()>=async{
            sqlx::query("SELECT pg_advisory_xact_lock(88261732)").execute(&mut *tx).await?;
            let fresh:Option<i64>=sqlx::query_scalar("SELECT config_version FROM stalwart_servers WHERE id=$1 AND active=1 AND dry_run=0 FOR SHARE").bind(server).fetch_optional(&mut *tx).await?;
            if fresh!=Some(version){return Err(Error::conflict("Server thay đổi"));}
            auth::require(&s,&h,"operations",true,true).await?.super_admin()?;
            sqlx::query("SELECT id FROM users WHERE id=$1 AND role='admin' AND admin_level='super' AND status='active' FOR SHARE").bind(u.id).fetch_optional(&mut *tx).await?.ok_or_else(Error::forbidden)?;
            let kind:String=row.get("kind");let local_id:i64=row.get("local_id");let table=if kind=="domain"{"domains"}else{"email_accounts"};
            let owner:i64=sqlx::query_scalar(&format!("SELECT customer_id FROM {table} WHERE id=$1")).bind(local_id).fetch_optional(&mut *tx).await?.ok_or_else(Error::missing)?;
            sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE").bind(owner).fetch_one(&mut *tx).await?;
            sqlx::query(&format!("SELECT id FROM {table} WHERE id=$1 FOR SHARE")).bind(local_id).fetch_one(&mut *tx).await?;
            if kind=="account"{sqlx::query("SELECT d.id FROM domains d JOIN email_accounts a ON a.domain_id=d.id WHERE a.id=$1 FOR SHARE OF d").bind(local_id).fetch_one(&mut *tx).await?;}
            let current=local(&mut tx,&kind,local_id).await?;let remote:String=row.get("remote_id");
            if current["server"]!=server || current["remote"]!=remote || current["eligible"]!=true || current["hash"]!=row.get::<String,_>("local_hash"){return Err(Error::conflict("Dữ liệu panel thay đổi"));}
            let busy:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_sync_jobs WHERE resource_id=$1 AND ((job_type LIKE 'domain_%' AND $2='domain') OR (job_type LIKE 'account_%' AND $2='account')) AND status IN ('pending','running'))").bind(local_id).bind(&kind).fetch_one(&mut *tx).await?;
            if busy{return Err(Error::conflict("Có tác vụ đồng bộ chưa hoàn tất"));}
            let ty=if kind=="domain"{"Domain"}else{"Account"};
            let metadata=stalwart::metadata(&s,server,"/api/account").await?;
            for suffix in ["Get","Update"]{if !metadata["data"]["permissions"].as_array().is_some_and(|a|a.iter().any(|p|p==&format!("sys{ty}{suffix}"))){return Err(Error::forbidden());}}
            let properties=if kind=="domain"{json!(["id","name","isEnabled"])}else{json!(["id","name","domainId","quotas","aliases","usedDiskQuota"])};
            let before=management::method(&s,server,&format!("x:{ty}/get"),json!({"ids":[remote],"properties":properties})).await?;
            let list=before["list"].as_array().filter(|a|a.len()==1&&a[0]["id"]==remote).ok_or_else(||Error::conflict("Remote mapping không còn tồn tại"))?;
            let projected=projection(&kind,&list[0])?;let desired:Value=row.get("desired_data");
            if projected==desired{return Ok(());}
            if projected!=row.get::<Value,_>("before_data"){return Err(Error::conflict("Remote đã thay đổi"));}
            if kind=="account" && list[0]["usedDiskQuota"].as_i64().is_some_and(|used|used>desired["maxDiskQuota"].as_i64().unwrap_or(0)){return Err(Error::conflict("Quota nhỏ hơn dung lượng đang dùng"));}
            let state=before["state"].as_str().filter(|s|!s.is_empty()).ok_or_else(||Error::conflict("Remote thiếu state"))?;
            let patch=patch(&kind,&projected,&desired)?;wrote=true;
            management::method(&s,server,&format!("x:{ty}/set"),json!({"ifInState":state,"update":{remote.clone():patch}})).await?;
            let verify=management::method(&s,server,&format!("x:{ty}/get"),json!({"ids":[remote],"properties":properties})).await?;
            if !verify["list"].as_array().is_some_and(|a|a.len()==1&&a[0]["id"]==remote&&projection(&kind,&a[0]).ok().as_ref()==Some(&desired)){return Err(Error::conflict("Đối chiếu sau ghi chưa khớp"));}
            Ok(())
        }.await;
        let status = if result.is_ok() {
            "applied"
        } else if wrote {
            "uncertain"
        } else {
            "stale"
        };
        sqlx::query("UPDATE reconciliation_items SET status=$1,detail=$2,applied_at=CASE WHEN $1='applied' THEN now() ELSE NULL END WHERE id=$3").bind(status).bind(if result.is_ok(){None}else{Some(if wrote{"Kết quả ghi chưa được xác minh; kiểm tra remote, không tự chạy lại"}else{"Điều kiện panel/remote/quyền thay đổi; xem trước lại"})}).bind(item).execute(&mut *tx).await?;
        tx.commit().await?;
        summary[status] = json!(summary[status].as_i64().unwrap() + 1);
    }
    auth::audit(
        &s,
        Some(u.id),
        "reconciliation_applied",
        json!({"run_id":id,"summary":summary}),
    )
    .await?;
    Ok(Json(summary))
}
