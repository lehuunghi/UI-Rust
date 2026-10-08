use crate::{
    auth, crypto,
    error::{Error, Result},
    stalwart, App,
};
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use serde_json::{json, Value};
use sqlx::Row;
pub async fn read(State(s): State<App>, h: HeaderMap, Json(v): Json<Value>) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "operations", true, true).await?;
    u.super_admin()?;
    let server = crate::resources::number(&v, "server_id")?;
    let method = crate::resources::text(&v, "method", 100)?;
    if !stalwart::allowed(method, false) {
        return Err(Error::forbidden());
    }
    let args = v.get("args").cloned().unwrap_or(json!({"limit":50}));
    let mut result = stalwart::call(&s, server, json!([[method, args, "c1"]])).await?;
    stalwart::redact(&mut result);
    Ok(Json(result))
}
pub async fn preview(
    State(s): State<App>,
    h: HeaderMap,
    Json(v): Json<Value>,
) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "operations", true, true).await?;
    u.super_admin()?;
    let server = crate::resources::number(&v, "server_id")?;
    let calls = v["calls"]
        .as_array()
        .filter(|v| !v.is_empty() && v.len() <= 20)
        .ok_or_else(|| Error::bad("Danh sách methodCalls không hợp lệ"))?;
    for c in calls {
        if !stalwart::allowed(c[0].as_str().unwrap_or(""), true)
            || !c[1].is_object()
            || !c[2].is_string()
        {
            return Err(Error::bad("Lệnh JMAP không hợp lệ"));
        }
    }
    let version: i64 =
        sqlx::query_scalar("SELECT config_version FROM stalwart_servers WHERE id=$1 AND active=1")
            .bind(server)
            .fetch_optional(&s.db)
            .await?
            .ok_or_else(Error::missing)?;
    let encrypted = crypto::seal(&s.config.key, &json!(calls).to_string())?;
    let id:i64=sqlx::query_scalar("INSERT INTO rust_change_plans(server_id,server_version,actor_id,calls) VALUES($1,$2,$3,$4) RETURNING id").bind(server).bind(version).bind(u.id).bind(json!({"encrypted":encrypted})).fetch_one(&s.db).await?;
    let mut review = json!(calls);
    stalwart::redact(&mut review);
    Ok(Json(
        json!({"plan_id":id,"calls":review,"expires_in_seconds":900,"warning":"Đọc kỹ danh sách thay đổi trước khi xác nhận; lệnh có thể xóa dữ liệu trên Stalwart."}),
    ))
}
pub async fn execute(
    State(s): State<App>,
    h: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "operations", true, true).await?;
    u.super_admin()?;
    let mut tx = s.db.begin().await?;
    let r=sqlx::query("SELECT p.*,s.config_version FROM rust_change_plans p JOIN stalwart_servers s ON s.id=p.server_id WHERE p.id=$1 AND p.actor_id=$2 AND p.status='ready' AND p.expires_at>now() AND s.active=1 FOR UPDATE OF p FOR SHARE OF s").bind(id).bind(u.id).fetch_optional(&mut *tx).await?.ok_or_else(||Error::conflict("Kế hoạch hết hạn hoặc đã thực hiện"))?;
    if r.get::<i64, _>("server_version") != r.get::<i64, _>("config_version") {
        return Err(Error::conflict(
            "Cấu hình server đã thay đổi; tạo lại bản xem trước",
        ));
    }
    let server: i64 = r.get("server_id");
    let payload: Value = r.get("calls");
    let calls: Value = serde_json::from_str(&crypto::open(
        &s.config.key,
        payload["encrypted"].as_str().ok_or_else(Error::missing)?,
    )?)
    .map_err(|e| anyhow::anyhow!(e))?;
    sqlx::query("UPDATE rust_change_plans SET status='executing' WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let (status, mut result) = match stalwart::call(&s, server, calls).await {
        Ok(v) => ("done", v),
        Err(_) => (
            "unconfirmed",
            json!({"error":"Không xác nhận được kết quả. Kiểm tra trạng thái trên Stalwart trước khi thực hiện lại."}),
        ),
    };
    stalwart::redact(&mut result);
    sqlx::query("UPDATE rust_change_plans SET status=$1,result=$2,calls='{}'::jsonb WHERE id=$3")
        .bind(status)
        .bind(&result)
        .bind(id)
        .execute(&s.db)
        .await?;
    auth::audit(
        &s,
        Some(u.id),
        "stalwart_change",
        json!({"plan":id,"status":status}),
    )
    .await?;
    Ok(Json(json!({"status":status,"result":result})))
}
pub async fn trial(
    State(s): State<App>,
    h: HeaderMap,
    Json(v): Json<Value>,
) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "packages", false, true).await?;
    if u.role != "customer" {
        return Err(Error::forbidden());
    }
    let mut tx = s.db.begin().await?;
    let r = sqlx::query("SELECT name,email FROM users WHERE id=$1 FOR UPDATE")
        .bind(u.id)
        .fetch_one(&mut *tx)
        .await?;
    let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM trial_requests WHERE customer_id=$1 AND status IN ('pending','approved'))").bind(u.id).fetch_one(&mut *tx).await?;
    if exists {
        return Err(Error::conflict("Đã có yêu cầu dùng thử"));
    }
    let id:i64=sqlx::query_scalar("INSERT INTO trial_requests(package_id,customer_id,name,email,phone,address,company_name) VALUES($1,$2,$3,$4,$5,$6,$7) RETURNING id").bind(crate::resources::number(&v,"package_id")?).bind(u.id).bind(r.get::<String,_>("name")).bind(r.get::<String,_>("email")).bind(crate::resources::text(&v,"phone",50)?).bind(crate::resources::text(&v,"address",255)?).bind(v["company_name"].as_str().unwrap_or("")).fetch_one(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(json!({"id":id})))
}
pub async fn trial_review(
    State(s): State<App>,
    h: HeaderMap,
    Path(id): Path<i64>,
    Json(v): Json<Value>,
) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "customers", true, true).await?;
    let mut tx = s.db.begin().await?;
    let owner: i64 = sqlx::query_scalar("SELECT customer_id FROM trial_requests WHERE id=$1")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(Error::missing)?;
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(owner)
        .fetch_one(&mut *tx)
        .await?;
    let r=sqlx::query("SELECT t.*,p.min_email_accounts,p.min_domains FROM trial_requests t JOIN packages p ON p.id=t.package_id WHERE t.id=$1 AND t.status='pending' FOR UPDATE OF t").bind(id).fetch_optional(&mut *tx).await?.ok_or_else(||Error::conflict("Yêu cầu đã được xử lý"))?;
    let approve = v["approve"] == true;
    let sid = if approve {
        let active: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM subscriptions WHERE customer_id=$1 AND status='active')",
        )
        .bind(owner)
        .fetch_one(&mut *tx)
        .await?;
        if active {
            return Err(Error::conflict("Khách hàng đã có gói hoạt động"));
        }
        let sid:i64=sqlx::query_scalar("INSERT INTO subscriptions(customer_id,package_id,price,selected_email_accounts,selected_domains,is_trial,status,payment_method,payment_status,activated_at,start_date,end_date) VALUES($1,$2,0,$3,$4,1,'active','manual','paid',now(),current_date,current_date+13) RETURNING id").bind(owner).bind(r.get::<i64,_>("package_id")).bind(r.get::<i32,_>("min_email_accounts")).bind(r.get::<i32,_>("min_domains")).fetch_one(&mut *tx).await?;
        sqlx::query("INSERT INTO email_groups(customer_id,name) VALUES($1,'Mặc định') ON CONFLICT DO NOTHING").bind(owner).execute(&mut *tx).await?;
        Some(sid)
    } else {
        None
    };
    sqlx::query("UPDATE trial_requests SET status=$1,subscription_id=$2,admin_note=$3,updated_at=now() WHERE id=$4").bind(if approve{"approved"}else{"rejected"}).bind(sid).bind(v["note"].as_str().unwrap_or("")).bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    auth::audit(
        &s,
        Some(u.id),
        "trial_review",
        json!({"id":id,"approved":approve}),
    )
    .await?;
    Ok(Json(json!({"ok":true,"subscription_id":sid})))
}
