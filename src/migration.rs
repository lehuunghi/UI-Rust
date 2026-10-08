use crate::{
    auth, crypto,
    error::{Error, Result},
    management, resources, stalwart, App,
};
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use serde_json::{json, Value};
use sqlx::{PgConnection, Row};

async fn inventory(db: &mut PgConnection, id: i64) -> Result<Value> {
    let owner: i64 = sqlx::query_scalar("SELECT customer_id FROM domains WHERE id=$1")
        .bind(id)
        .fetch_optional(&mut *db)
        .await?
        .ok_or_else(Error::missing)?;
    let active: bool =
        sqlx::query_scalar("SELECT status='active' FROM users WHERE id=$1 FOR UPDATE")
            .bind(owner)
            .fetch_one(&mut *db)
            .await?;
    let domain:Value=sqlx::query_scalar("SELECT jsonb_build_object('id',id,'customer_id',customer_id,'domain_name',domain_name,'status',status,'stalwart_domain_id',stalwart_domain_id,'server_id',server_id,'sync_status',sync_status) FROM domains WHERE id=$1 AND status='active' FOR UPDATE").bind(id).fetch_optional(&mut *db).await?.ok_or_else(Error::missing)?;
    if domain["customer_id"] != owner {
        return Err(Error::conflict("Sở hữu domain đã thay đổi"));
    }
    if !active {
        return Err(Error::conflict("Khách hàng không hoạt động"));
    }
    if domain["sync_status"] != "synced" || domain["stalwart_domain_id"].as_str().is_none() {
        return Err(Error::conflict("Domain chưa đồng bộ"));
    }
    let accounts:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',id,'customer_id',customer_id,'domain_id',domain_id,'local_part',local_part,'stalwart_account_id',stalwart_account_id,'storage_limit_mb',storage_limit_mb,'status',status,'server_id',server_id,'sync_status',sync_status) FROM email_accounts WHERE domain_id=$1 AND status<>'deleted' ORDER BY id FOR UPDATE").bind(id).fetch_all(&mut *db).await?;
    if accounts.len() > 200
        || accounts.iter().any(|a| {
            a["sync_status"] != "synced"
                || a["server_id"] != domain["server_id"]
                || a["customer_id"] != domain["customer_id"]
                || a["stalwart_account_id"].as_str().is_none()
        })
    {
        return Err(Error::conflict(
            "Tối đa 200 tài khoản đã đồng bộ trên server nguồn",
        ));
    }
    let aliases:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',a.id,'email_account_id',a.email_account_id,'domain_id',a.domain_id,'local_part',a.local_part,'status',a.status,'sync_status',a.sync_status) FROM email_aliases a JOIN email_accounts e ON e.id=a.email_account_id WHERE a.status<>'deleted' AND (a.domain_id=$1 OR e.domain_id=$1) ORDER BY a.id FOR UPDATE OF a").bind(id).fetch_all(&mut *db).await?;
    if aliases.iter().any(|a| {
        a["domain_id"] != id
            || a["sync_status"] != "synced"
            || !accounts.iter().any(|e| e["id"] == a["email_account_id"])
    }) {
        return Err(Error::conflict(
            "Bí danh liên kết chéo domain hoặc chưa đồng bộ; xử lý trước khi chuyển",
        ));
    }
    Ok(json!({"domain":domain,"accounts":accounts,"aliases":aliases}))
}
fn remote_id(v: &Value) -> Result<&str> {
    v.as_str()
        .filter(|s| !s.is_empty() && s.len() <= 190 && !s.chars().any(char::is_control))
        .ok_or_else(|| Error::bad("ID đích không hợp lệ"))
}
async fn verify_target(s: &App, server: i64, local: &Value, map: &Value) -> Result<()> {
    let permissions = stalwart::metadata(s, server, "/api/account").await?;
    for p in ["sysDomainGet", "sysAccountGet"] {
        if !permissions["data"]["permissions"]
            .as_array()
            .is_some_and(|a| a.iter().any(|v| v == p))
        {
            return Err(Error::forbidden());
        }
    }
    let result = management::method(
        s,
        server,
        "x:Domain/get",
        json!({"ids":[map["domain_id"]],"properties":["id","name","isEnabled"]}),
    )
    .await?;
    let rows = result["list"]
        .as_array()
        .filter(|r| r.len() == 1)
        .ok_or_else(|| Error::conflict("Domain đích không tồn tại"))?;
    if rows[0]["id"] != map["domain_id"]
        || rows[0]["name"] != local["domain"]["domain_name"]
        || rows[0]["isEnabled"] != true
    {
        return Err(Error::conflict("Domain đích không khớp hoặc chưa bật"));
    }
    for chunk in local["accounts"].as_array().unwrap().chunks(50) {
        let ids: Vec<_> = chunk
            .iter()
            .map(|a| map["accounts"][a["id"].to_string()].clone())
            .collect();
        let result = management::method(
            s,
            server,
            "x:Account/get",
            json!({"ids":ids,"properties":["id","name","domainId","quotas","aliases"]}),
        )
        .await?;
        let rows = result["list"]
            .as_array()
            .filter(|r| r.len() == chunk.len())
            .ok_or_else(|| Error::conflict("Không đủ tài khoản trên server đích"))?;
        for account in chunk {
            let remote = &map["accounts"][account["id"].to_string()];
            let matched: Vec<_> = rows.iter().filter(|r| r["id"] == *remote).collect();
            if matched.len() != 1 {
                return Err(Error::conflict("ID tài khoản đích thiếu hoặc trùng"));
            }
            let r = matched[0];
            let quota = r["quotas"]["maxDiskQuota"]
                .as_i64()
                .filter(|q| *q >= 0)
                .ok_or_else(|| Error::conflict("Không xác nhận được quota đích"))?;
            if r["name"] != account["local_part"]
                || r["domainId"] != map["domain_id"]
                || (quota != 0 && quota < account["storage_limit_mb"].as_i64().unwrap() * 1048576)
            {
                return Err(Error::conflict("Tài khoản hoặc quota đích không khớp"));
            }
            let mut expected:Vec<_>=local["aliases"].as_array().unwrap().iter().filter(|a|a["email_account_id"]==account["id"]).map(|a|json!({"name":a["local_part"],"domainId":map["domain_id"],"enabled":a["status"]=="active"})).collect();
            let mut actual:Vec<_>=r["aliases"].as_array().ok_or_else(||Error::conflict("Không đọc được bí danh đích"))?.iter().map(|a|json!({"name":a["name"],"domainId":a["domainId"],"enabled":a.get("enabled").cloned().unwrap_or(json!(true))})).collect();
            expected.sort_by_key(Value::to_string);
            actual.sort_by_key(Value::to_string);
            if expected != actual {
                return Err(Error::conflict("Bí danh trên server đích chưa khớp"));
            }
        }
    }
    Ok(())
}
pub async fn preview(
    State(s): State<App>,
    h: HeaderMap,
    Json(v): Json<Value>,
) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "operations", true, true).await?;
    u.super_admin()?;
    let domain = resources::number(&v, "domain_id")?;
    let target = resources::number(&v, "target_server_id")?;
    for key in ["backup", "restore", "delivery", "dns"] {
        if v["checklist"][key] != true {
            return Err(Error::bad(
                "Hoàn tất backup, phục hồi thử, giao thư và DNS trước khi chuyển",
            ));
        }
    }
    let mut tx = s.db.begin().await?;
    let local = inventory(&mut tx, domain).await?;
    let source = local["domain"]["server_id"]
        .as_i64()
        .ok_or_else(Error::missing)?;
    if source == target {
        return Err(Error::bad("Chọn server đích khác server nguồn"));
    }
    let source_version: i64 = sqlx::query_scalar(
        "SELECT config_version FROM stalwart_servers WHERE id=$1 AND active=1 FOR SHARE",
    )
    .bind(source)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(Error::missing)?;
    let target_version:i64=sqlx::query_scalar("SELECT config_version FROM stalwart_servers WHERE id=$1 AND active=1 AND dry_run=0 FOR SHARE").bind(target).fetch_optional(&mut *tx).await?.ok_or_else(||Error::bad("Server đích phải hoạt động ở chế độ thật"))?;
    let map = &v["mappings"];
    remote_id(&map["domain_id"])?;
    let maps = map["accounts"]
        .as_object()
        .ok_or_else(|| Error::bad("Thiếu ánh xạ tài khoản"))?;
    if maps.len() != local["accounts"].as_array().unwrap().len() {
        return Err(Error::bad(
            "Ánh xạ phải đủ và chỉ gồm các tài khoản hiện tại",
        ));
    }
    let mut unique = std::collections::HashSet::new();
    for a in local["accounts"].as_array().unwrap() {
        if !unique.insert(remote_id(&map["accounts"][a["id"].to_string()])?) {
            return Err(Error::bad("ID tài khoản đích bị trùng"));
        }
    }
    verify_target(&s, target, &local, map).await?;
    let id:i64=sqlx::query_scalar("INSERT INTO mailbox_migration_plans(domain_id,source_server_id,target_server_id,actor_id,status,inventory_hash,target_version,source_version,mappings,checklist,expires_at) VALUES($1,$2,$3,$4,'ready',$5,$6,$7,$8,$9,now()+interval '30 minutes') RETURNING id").bind(domain).bind(source).bind(target).bind(u.id).bind(crypto::hash(&local.to_string())).bind(i32::try_from(target_version).map_err(|_|Error::bad("Phiên bản cấu hình không hợp lệ"))?).bind(source_version).bind(map).bind(&v["checklist"]).fetch_one(&mut *tx).await?;
    sqlx::query("INSERT INTO audit_logs(user_id,action,context) VALUES($1,'migration_preview',$2)")
        .bind(u.id)
        .bind(json!({"plan_id":id,"domain_id":domain,"target_server_id":target}))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(
        json!({"plan_id":id,"status":"ready","inventory":local,"mappings":map,"expires_in_seconds":1800}),
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
    if v["confirmation"] != "MIGRATE" {
        return Err(Error::bad("Nhập MIGRATE để xác nhận chuyển liên kết"));
    }
    let mut tx = s.db.begin().await?;
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(88261732)")
        .fetch_one(&mut *tx)
        .await?;
    if !locked {
        return Err(Error::conflict("Worker đang đồng bộ; thử lại sau"));
    }
    let p=sqlx::query("SELECT * FROM mailbox_migration_plans WHERE id=$1 AND actor_id=$2 AND status='ready' AND expires_at>now() FOR UPDATE").bind(id).bind(u.id).fetch_optional(&mut *tx).await?.ok_or_else(||Error::conflict("Kế hoạch đã thực hiện hoặc hết hạn"))?;
    let owner: i64 = sqlx::query_scalar("SELECT customer_id FROM domains WHERE id=$1")
        .bind(p.get::<i64, _>("domain_id"))
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(owner)
        .fetch_one(&mut *tx)
        .await?;
    let target_id: i64 = p.get("target_server_id");
    sqlx::query("SELECT server_id FROM server_capacity WHERE server_id=$1 FOR UPDATE")
        .bind(target_id)
        .fetch_one(&mut *tx)
        .await?;
    let domain: i64 = p.get("domain_id");
    let local = inventory(&mut tx, domain).await?;
    if crypto::hash(&local.to_string()) != p.get::<String, _>("inventory_hash") {
        return Err(Error::conflict("Dữ liệu đã thay đổi; lập kế hoạch mới"));
    }
    for (server, version, live) in [
        (
            p.get::<i64, _>("source_server_id"),
            p.get::<i64, _>("source_version"),
            false,
        ),
        (
            p.get::<i64, _>("target_server_id"),
            i64::from(p.get::<i32, _>("target_version")),
            true,
        ),
    ] {
        let r=sqlx::query("SELECT config_version,dry_run FROM stalwart_servers WHERE id=$1 AND active=1 FOR SHARE").bind(server).fetch_optional(&mut *tx).await?.ok_or_else(Error::missing)?;
        if r.get::<i64, _>("config_version") != version || (live && r.get::<i16, _>("dry_run") != 0)
        {
            return Err(Error::conflict("Cấu hình server đã thay đổi"));
        }
    }
    let account_ids: Vec<i64> = local["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_i64().unwrap())
        .collect();
    let pending:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_sync_jobs WHERE status<>'done' AND ((job_type LIKE 'domain_%' AND resource_id=$1) OR (job_type LIKE 'account_%' AND resource_id=ANY($2))))").bind(domain).bind(&account_ids).fetch_one(&mut *tx).await?;
    if pending {
        return Err(Error::conflict(
            "Cần xử lý hết tác vụ đồng bộ trước khi chuyển",
        ));
    }
    crate::placement::capacity(&mut tx, target_id, account_ids.len() as i64).await?;
    let map: Value = p.get("mappings");
    let target: i64 = p.get("target_server_id");
    verify_target(&s, target, &local, &map).await?;
    sqlx::query(
        "UPDATE domains SET server_id=$1,stalwart_domain_id=$2,updated_at=now() WHERE id=$3",
    )
    .bind(target)
    .bind(remote_id(&map["domain_id"])?)
    .bind(domain)
    .execute(&mut *tx)
    .await?;
    for account in account_ids {
        sqlx::query("UPDATE email_accounts SET server_id=$1,stalwart_account_id=$2,updated_at=now() WHERE id=$3").bind(target).bind(remote_id(&map["accounts"][account.to_string()])?).bind(account).execute(&mut *tx).await?;
    }
    sqlx::query("UPDATE mailbox_migration_plans SET status='applied',applied_at=now() WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO audit_logs(user_id,action,context) VALUES($1,'migration_links_applied',$2)",
    )
    .bind(u.id)
    .bind(json!({"plan_id":id,"domain_id":domain,"target_server_id":target}))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"status":"applied","plan_id":id})))
}
