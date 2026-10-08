use crate::{
    auth,
    error::{Error, Result},
    management, resources, stalwart, App,
};
use axum::{extract::State, http::HeaderMap, Json};
use serde_json::{json, Value};
use sqlx::Row;
pub async fn list(State(s): State<App>, h: HeaderMap) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "operations", true, false).await?;
    u.super_admin()?;
    let rows: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(b) FROM native_tenant_bindings b ORDER BY id DESC LIMIT 200",
    )
    .fetch_all(&s.db)
    .await?;
    Ok(Json(json!({"items":rows})))
}
pub async fn bind(State(s): State<App>, h: HeaderMap, Json(v): Json<Value>) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "operations", true, true).await?;
    u.super_admin()?;
    let server = resources::number(&v, "server_id")?;
    let customer = resources::number(&v, "customer_id")?;
    let tenant = resources::text(&v, "tenant_id", 190)?;
    let version = v["version"]
        .as_i64()
        .filter(|v| *v >= 0)
        .ok_or_else(|| Error::bad("Thiếu phiên bản liên kết"))?;
    let mut tx = s.db.begin().await?;
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(88261732)")
        .fetch_one(&mut *tx)
        .await?;
    if !locked {
        return Err(Error::conflict("Worker đang đồng bộ; thử lại sau"));
    }
    sqlx::query(
        "SELECT id FROM users WHERE id=$1 AND role='customer' AND status='active' FOR UPDATE",
    )
    .bind(customer)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(Error::missing)?;
    sqlx::query("SELECT id FROM stalwart_servers WHERE id=$1 AND active=1 AND dry_run=0 FOR SHARE")
        .bind(server)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| Error::bad("Chọn server thật đang hoạt động"))?;
    let perms = stalwart::metadata(&s, server, "/api/account").await?;
    for p in ["sysTenantGet", "sysDomainGet"] {
        if !perms["data"]["permissions"]
            .as_array()
            .is_some_and(|a| a.iter().any(|v| v == p))
        {
            return Err(Error::forbidden());
        }
    }
    let remote = management::method(
        &s,
        server,
        "x:Tenant/get",
        json!({"ids":[tenant],"properties":["id","name"]}),
    )
    .await?;
    if !remote["list"]
        .as_array()
        .is_some_and(|a| a.len() == 1 && a[0]["id"] == tenant)
    {
        return Err(Error::bad("Tenant không tồn tại trên server đã chọn"));
    }
    let rows=sqlx::query("SELECT id,stalwart_domain_id FROM domains WHERE customer_id=$1 AND server_id=$2 AND status<>'deleted' ORDER BY id LIMIT 201 FOR UPDATE").bind(customer).bind(server).fetch_all(&mut *tx).await?;
    if rows.len() > 200 {
        return Err(Error::bad("Tối đa 200 domain cho một lượt kiểm tra"));
    }
    let ids: Vec<String> = rows
        .iter()
        .filter_map(|r| r.get::<Option<String>, _>("stalwart_domain_id"))
        .collect();
    for chunk in ids.chunks(50) {
        let response = management::method(
            &s,
            server,
            "x:Domain/get",
            json!({"ids":chunk,"properties":["id","memberTenantId"]}),
        )
        .await?;
        let domains = response["list"]
            .as_array()
            .ok_or_else(|| Error::conflict("Không đọc được domain hiện tại"))?;
        if domains.len() != chunk.len()
            || chunk.iter().any(|id| {
                domains
                    .iter()
                    .filter(|d| d["id"] == *id && d["memberTenantId"] == tenant)
                    .count()
                    != 1
            })
        {
            return Err(Error::conflict("Domain hiện có chưa thuộc tenant đã chọn"));
        }
    }
    let old=sqlx::query("SELECT id,version FROM native_tenant_bindings WHERE server_id=$1 AND customer_id=$2 FOR UPDATE").bind(server).bind(customer).fetch_optional(&mut *tx).await?;
    if old
        .as_ref()
        .map(|r| r.get::<i64, _>("version"))
        .unwrap_or(0)
        != version
    {
        return Err(Error::conflict("Liên kết đã thay đổi; tải lại trang"));
    }
    sqlx::query("INSERT INTO native_tenant_bindings(server_id,customer_id,tenant_id) VALUES($1,$2,$3) ON CONFLICT(server_id,customer_id) DO UPDATE SET tenant_id=excluded.tenant_id,version=native_tenant_bindings.version+1,updated_at=now()").bind(server).bind(customer).bind(tenant).execute(&mut *tx).await?;
    sqlx::query(
        "INSERT INTO audit_logs(user_id,action,context) VALUES($1,'native_tenant_binding',$2)",
    )
    .bind(u.id)
    .bind(json!({"server_id":server,"customer_id":customer,"tenant_id":tenant}))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"ok":true,"version":version+1})))
}
