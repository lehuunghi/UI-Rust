use crate::{
    auth,
    error::{Error, Result},
    resources, App,
};
use axum::{extract::State, http::HeaderMap, Json};
use serde_json::{json, Value};
use sqlx::PgConnection;
pub async fn capacity(db: &mut PgConnection, server: i64, extra: i64) -> Result<()> {
    let limit: i32 = sqlx::query_scalar(
        "SELECT max_accounts FROM server_capacity WHERE server_id=$1 FOR UPDATE",
    )
    .bind(server)
    .fetch_optional(&mut *db)
    .await?
    .ok_or_else(Error::missing)?;
    if limit > 0 {
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM email_accounts WHERE server_id=$1 AND status<>'deleted'",
        )
        .bind(server)
        .fetch_one(db)
        .await?;
        if count
            .checked_add(extra)
            .is_none_or(|n| n > i64::from(limit))
        {
            return Err(Error::conflict("Server đã đạt giới hạn tài khoản"));
        }
    }
    Ok(())
}
pub async fn list(State(s): State<App>, h: HeaderMap) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "operations", true, false).await?;
    u.super_admin()?;
    let placements:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(p) FROM resource_placements p WHERE scope='customer' ORDER BY resource_id LIMIT 1000").fetch_all(&s.db).await?;
    let capacities:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(c)||jsonb_build_object('name',s.name,'active',s.active,'accounts',(SELECT count(*) FROM email_accounts a WHERE a.server_id=c.server_id AND a.status<>'deleted'),'domains',(SELECT count(*) FROM domains d WHERE d.server_id=c.server_id AND d.status<>'deleted'),'used_bytes',(SELECT COALESCE(sum(storage_used_bytes),0) FROM email_accounts a WHERE a.server_id=c.server_id AND a.status<>'deleted' AND a.storage_used_synced_at>=now()-interval '24 hours'),'stale_accounts',(SELECT count(*) FROM email_accounts a WHERE a.server_id=c.server_id AND a.status<>'deleted' AND (a.storage_used_synced_at IS NULL OR a.storage_used_synced_at<now()-interval '24 hours'))) FROM server_capacity c JOIN stalwart_servers s ON s.id=c.server_id ORDER BY c.server_id").fetch_all(&s.db).await?;
    Ok(Json(
        json!({"placements":placements,"capacities":capacities}),
    ))
}
pub async fn save(State(s): State<App>, h: HeaderMap, Json(v): Json<Value>) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "operations", true, true).await?;
    u.super_admin()?;
    let server = resources::number(&v, "server_id")?;
    let mut tx = s.db.begin().await?;
    sqlx::query("SELECT id FROM stalwart_servers WHERE id=$1 AND active=1 FOR SHARE")
        .bind(server)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(Error::missing)?;
    let kind = v["kind"].as_str().unwrap_or("customer");
    if kind == "customer" {
        let customer = resources::number(&v, "customer_id")?;
        sqlx::query(
            "SELECT id FROM users WHERE id=$1 AND role='customer' AND status='active' FOR UPDATE",
        )
        .bind(customer)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(Error::missing)?;
        let current:Option<i32>=sqlx::query_scalar("SELECT version FROM resource_placements WHERE scope='customer' AND resource_id=$1 FOR UPDATE").bind(customer).fetch_optional(&mut *tx).await?;
        if i64::from(current.unwrap_or(0)) != v["version"].as_i64().unwrap_or(-1) {
            return Err(Error::conflict("Phân bổ đã thay đổi; tải lại trang"));
        }
        sqlx::query("INSERT INTO resource_placements(scope,resource_id,server_id) VALUES('customer',$1,$2) ON CONFLICT(scope,resource_id) DO UPDATE SET server_id=excluded.server_id,version=resource_placements.version+1,updated_at=now()").bind(customer).bind(server).execute(&mut *tx).await?;
    } else if kind == "capacity" {
        let max = v["max_accounts"]
            .as_i64()
            .filter(|n| *n >= 0 && *n <= 1_000_000_000)
            .ok_or_else(|| Error::bad("Hạn mức tài khoản không hợp lệ"))?;
        let mut decimals = Vec::new();
        for k in ["storage_budget_gb", "cost_per_gb", "fixed_cost"] {
            let value = v[k]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| v[k].to_string());
            let n: rust_decimal::Decimal = value
                .parse()
                .map_err(|_| Error::bad("Chi phí/dung lượng không hợp lệ"))?;
            if n < rust_decimal::Decimal::ZERO || n > rust_decimal::Decimal::from(1_000_000_000_i64)
            {
                return Err(Error::bad("Chi phí/dung lượng ngoài giới hạn"));
            }
            decimals.push(n);
        }
        sqlx::query("UPDATE server_capacity SET max_accounts=$1,storage_budget_gb=$2,cost_per_gb=$3,fixed_cost=$4,updated_at=now() WHERE server_id=$5").bind(max as i32).bind(decimals[0]).bind(decimals[1]).bind(decimals[2]).bind(server).execute(&mut *tx).await?;
    } else {
        return Err(Error::bad("Loại phân bổ không hợp lệ"));
    }
    sqlx::query(
        "INSERT INTO audit_logs(user_id,action,context) VALUES($1,'placement_configured',$2)",
    )
    .bind(u.id)
    .bind(json!({"kind":kind,"server_id":server,"customer_id":v["customer_id"]}))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"ok":true})))
}
