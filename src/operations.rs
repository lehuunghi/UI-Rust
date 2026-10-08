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
    let mut call_ids = std::collections::HashSet::new();
    for c in calls {
        if !c.as_array().is_some_and(|a| a.len() == 3)
            || !c[2]
                .as_str()
                .is_some_and(|id| !id.is_empty() && id.len() <= 190 && call_ids.insert(id))
            || !stalwart::allowed(c[0].as_str().unwrap_or(""), true)
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
    let guards = v.get("preconditions").cloned().unwrap_or(json!([]));
    let conditions = guards
        .as_array()
        .filter(|g| g.len() <= 50)
        .ok_or_else(|| Error::bad("Điều kiện kế hoạch không hợp lệ"))?;
    for g in conditions {
        if !matches!(g["kind"].as_str(), Some("accounts" | "domains"))
            || !g["id"].as_i64().is_some_and(|id| id > 0)
            || !g["expected"]
                .as_object()
                .is_some_and(|o| !o.is_empty() && o.len() <= 30)
        {
            return Err(Error::bad("Điều kiện kế hoạch không hợp lệ"));
        }
    }
    let mut remote = v.get("remote_preconditions").cloned().unwrap_or(json!([]));
    for condition in remote
        .as_array()
        .filter(|a| a.len() <= 20)
        .ok_or_else(|| Error::bad("Điều kiện server không hợp lệ"))?
    {
        if !condition["method"]
            .as_str()
            .is_some_and(|m| m.ends_with("/get") && stalwart::allowed(m, false))
            || !condition["ids"].as_array().is_some_and(|a| {
                !a.is_empty()
                    && a.len() <= 50
                    && a.iter()
                        .all(|id| id.as_str().is_some_and(|s| !s.is_empty() && s.len() <= 190))
            })
            || !condition["properties"]
                .as_array()
                .is_some_and(|a| a.len() <= 50 && a.iter().all(|v| v.is_string()))
            || !condition["expected"].is_array()
        {
            return Err(Error::bad("Điều kiện server không hợp lệ"));
        }
    }
    stalwart::redact(&mut remote);
    let remote = crypto::seal(&s.config.key, &remote.to_string())?;
    let encrypted = crypto::seal(&s.config.key, &json!(calls).to_string())?;
    let id:i64=sqlx::query_scalar("INSERT INTO rust_change_plans(server_id,server_version,actor_id,calls,preconditions,remote_preconditions) VALUES($1,$2,$3,$4,$5,$6) RETURNING id").bind(server).bind(version).bind(u.id).bind(json!({"encrypted":encrypted})).bind(guards).bind(remote).fetch_one(&s.db).await?;
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
    let dry: i16 = sqlx::query_scalar("SELECT dry_run FROM stalwart_servers WHERE id=$1")
        .bind(server)
        .fetch_one(&mut *tx)
        .await?;
    if dry == 0 {
        let metadata = stalwart::metadata(&s, server, "/api/account").await?;
        let permissions = metadata["data"]["permissions"]
            .as_array()
            .ok_or_else(Error::forbidden)?;
        for call in calls.as_array().ok_or_else(Error::missing)? {
            let method = call[0].as_str().unwrap_or("");
            if let Some((kind, "set")) = method.strip_prefix("x:").and_then(|v| v.split_once('/')) {
                for (key, suffix) in [
                    ("create", "Create"),
                    ("update", "Update"),
                    ("destroy", "Destroy"),
                ] {
                    if call[1].get(key).is_some()
                        && !permissions
                            .iter()
                            .any(|v| v == &format!("sys{kind}{suffix}"))
                    {
                        return Err(Error::forbidden());
                    }
                }
                if matches!(kind, "ApiKey" | "AppPassword") {
                    for item in call[1]["create"]
                        .as_object()
                        .into_iter()
                        .flat_map(|o| o.values())
                    {
                        for grant in item["permissions"]["permissions"]
                            .as_object()
                            .into_iter()
                            .flat_map(|o| o.keys())
                        {
                            if !permissions.iter().any(|v| v == grant) {
                                return Err(Error::forbidden());
                            }
                        }
                    }
                }
            }
        }
    }
    sqlx::query("UPDATE rust_change_plans SET status='executing' WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let mut connection_guard = s.db.begin().await?;
    let version: Option<i64> = sqlx::query_scalar(
        "SELECT config_version FROM stalwart_servers WHERE id=$1 AND active=1 FOR SHARE",
    )
    .bind(server)
    .fetch_optional(&mut *connection_guard)
    .await?;
    if version != Some(r.get::<i64, _>("server_version")) {
        sqlx::query("UPDATE rust_change_plans SET status='stale' WHERE id=$1")
            .bind(id)
            .execute(&mut *connection_guard)
            .await?;
        connection_guard.commit().await?;
        return Err(Error::conflict(
            "Cấu hình server đã thay đổi; tạo lại kế hoạch",
        ));
    }
    let guards: Value = r.get("preconditions");
    for guard in guards
        .as_array()
        .ok_or_else(|| Error::bad("Điều kiện kế hoạch không hợp lệ"))?
    {
        let table = match guard["kind"].as_str() {
            Some("accounts") => "email_accounts",
            Some("domains") => "domains",
            _ => return Err(Error::bad("Điều kiện không hợp lệ")),
        };
        let current: Option<Value> = sqlx::query_scalar(&format!(
            "SELECT to_jsonb(r)-ARRAY['password_hash'] FROM {table} r WHERE id=$1 FOR SHARE"
        ))
        .bind(
            guard["id"]
                .as_i64()
                .ok_or_else(|| Error::bad("ID điều kiện không hợp lệ"))?,
        )
        .fetch_optional(&mut *connection_guard)
        .await?;
        let matches = current.as_ref().is_some_and(|current| {
            guard["expected"]
                .as_object()
                .is_some_and(|fields| fields.iter().all(|(key, value)| current[key] == *value))
        });
        if !matches {
            sqlx::query(
                "UPDATE rust_change_plans SET status='stale',calls='{}'::jsonb WHERE id=$1",
            )
            .bind(id)
            .execute(&mut *connection_guard)
            .await?;
            connection_guard.commit().await?;
            return Err(Error::conflict(
                "Liên kết hoặc tài nguyên đã thay đổi; xem trước lại",
            ));
        }
    }
    let verification: Result<()> = async {
        if let Some(encrypted) = r.get::<Option<String>, _>("remote_preconditions") {
            let remote: Value = serde_json::from_str(&crypto::open(&s.config.key, &encrypted)?)
                .map_err(|_| Error::bad("Điều kiện server không hợp lệ"))?;
            for condition in remote.as_array().ok_or_else(Error::missing)? {
                let mut response = crate::management::method(
                    &s,
                    server,
                    condition["method"].as_str().unwrap(),
                    json!({"ids":condition["ids"],"properties":condition["properties"]}),
                )
                .await?;
                let rows = response["list"]
                    .as_array_mut()
                    .ok_or_else(|| Error::conflict("Không đọc được tài nguyên hiện tại"))?;
                for row in rows.iter_mut() {
                    let obj = row
                        .as_object_mut()
                        .ok_or_else(|| Error::conflict("Phản hồi không hợp lệ"))?;
                    obj.retain(|k, _| {
                        condition["properties"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|v| v == k)
                    });
                    stalwart::redact(row);
                }
                let mut current = rows.clone();
                let mut expected = condition["expected"]
                    .as_array()
                    .ok_or_else(Error::missing)?
                    .clone();
                current.sort_by_key(Value::to_string);
                expected.sort_by_key(Value::to_string);
                if current != expected {
                    return Err(Error::conflict("Dữ liệu server đã thay đổi; xem trước lại"));
                }
            }
        }
        Ok(())
    }
    .await;
    if verification.is_err() {
        sqlx::query("UPDATE rust_change_plans SET status='stale',calls='{}'::jsonb,remote_preconditions=NULL WHERE id=$1").bind(id).execute(&mut *connection_guard).await?;
        connection_guard.commit().await?;
        return Err(Error::conflict(
            "Không xác minh được dữ liệu server còn khớp; xem trước lại",
        ));
    }
    let (status, mut result) = match stalwart::call(&s, server, calls).await {
        Ok(v) => ("done", v),
        Err(_) => (
            "unconfirmed",
            json!({"error":"Không xác nhận được kết quả. Kiểm tra trạng thái trên Stalwart trước khi thực hiện lại."}),
        ),
    };
    let secrets: Vec<Value> = result["methodResponses"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| matches!(r[0].as_str(), Some("x:ApiKey/set" | "x:AppPassword/set")))
        .flat_map(|r| {
            r[1]["created"]
                .as_object()
                .into_iter()
                .flat_map(|o| o.values())
        })
        .filter_map(|v| {
            v["secret"]
                .as_str()
                .map(|secret| json!({"id":v["id"],"secret":secret}))
        })
        .collect();
    let encrypted = if secrets.is_empty() {
        None
    } else {
        Some(crypto::seal(&s.config.key, &json!(secrets).to_string())?)
    };
    stalwart::redact(&mut result);
    sqlx::query("UPDATE rust_change_plans SET status=$1,result=$2,calls='{}'::jsonb,result_secret=$4,finished_at=now(),remote_preconditions=NULL WHERE id=$3")
        .bind(status)
        .bind(&result)
        .bind(id)
        .bind(&encrypted)
        .execute(&mut *connection_guard)
        .await?;
    connection_guard.commit().await?;
    auth::audit(
        &s,
        Some(u.id),
        "stalwart_change",
        json!({"plan":id,"status":status}),
    )
    .await?;
    Ok(Json(
        json!({"status":status,"result":result,"secret_available":encrypted.is_some()}),
    ))
}
pub async fn trial_review(
    State(s): State<App>,
    h: HeaderMap,
    Path(id): Path<i64>,
    Json(v): Json<Value>,
) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "customers", true, true).await?;
    let mut tx = s.db.begin().await?;
    let preliminary = sqlx::query("SELECT customer_id,email FROM trial_requests WHERE id=$1")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(Error::missing)?;
    let mut owner: Option<i64> = preliminary.get("customer_id");
    let email: String = preliminary.get("email");
    if owner.is_none() {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,35))")
            .bind(email.to_lowercase())
            .execute(&mut *tx)
            .await?;
        owner = sqlx::query_scalar("SELECT id FROM users WHERE lower(email)=lower($1) AND role='customer' AND status='active' FOR UPDATE").bind(&email).fetch_optional(&mut *tx).await?;
    } else {
        sqlx::query(
            "SELECT id FROM users WHERE id=$1 AND role='customer' AND status='active' FOR UPDATE",
        )
        .bind(owner)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(Error::missing)?;
    }
    let r=sqlx::query("SELECT t.*,p.min_email_accounts,p.min_domains FROM trial_requests t JOIN packages p ON p.id=t.package_id WHERE t.id=$1 AND t.status='pending' AND p.status='active' FOR UPDATE OF t").bind(id).fetch_optional(&mut *tx).await?.ok_or_else(||Error::conflict("Yêu cầu đã được xử lý hoặc gói đã tắt"))?;
    let approve = v["approve"]
        .as_bool()
        .ok_or_else(|| Error::bad("Cần chọn duyệt hoặc từ chối"))?;
    if approve && owner.is_none() {
        if s.config.smtp_host.is_empty() {
            return Err(Error::bad(
                "Cần cấu hình SMTP để khách mới nhận liên kết đặt mật khẩu",
            ));
        }
        let occupied: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE lower(email)=lower($1))")
                .bind(&email)
                .fetch_one(&mut *tx)
                .await?;
        if occupied {
            return Err(Error::conflict(
                "Email đã thuộc tài khoản không phù hợp; cần quản trị viên xem xét",
            ));
        }
        let password = crypto::token();
        let hash = tokio::task::spawn_blocking(move || crypto::password(&password))
            .await
            .map_err(|e| anyhow::anyhow!(e))??;
        let uid:i64=sqlx::query_scalar("INSERT INTO users(name,email,password_hash,role,two_factor_enabled) VALUES($1,$2,$3,'customer',0) RETURNING id").bind(r.get::<String,_>("name")).bind(email.to_lowercase()).bind(hash).fetch_one(&mut *tx).await?;
        let token = crypto::token();
        sqlx::query("INSERT INTO rust_reset_tokens(token_hash,user_id,expires_at) VALUES($1,$2,now()+interval '30 minutes')").bind(crypto::hash(&token)).bind(uid).execute(&mut *tx).await?;
        if !crate::notifications::queue(
            &s,
            &mut tx,
            uid,
            "password_reset",
            json!({"reset_url":format!("{}/reset-password#{}",s.config.app_url,token)}),
            Some(&format!("trial-welcome:{id}")),
        )
        .await?
        {
            return Err(Error::bad(
                "Bật mẫu email đặt mật khẩu trước khi duyệt khách mới",
            ));
        }
        owner = Some(uid);
    }
    let sid = if approve {
        let used: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM trial_requests WHERE customer_id=$1 AND status='approved' AND id<>$2)").bind(owner).bind(id).fetch_one(&mut *tx).await?;
        if used {
            return Err(Error::conflict(
                "Khách hàng đã sử dụng chương trình dùng thử",
            ));
        }

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
    sqlx::query("UPDATE trial_requests SET status=$1,subscription_id=$2,admin_note=$3,updated_at=now(),customer_id=$5 WHERE id=$4").bind(if approve{"approved"}else{"rejected"}).bind(sid).bind(v["note"].as_str().unwrap_or("")).bind(id).bind(owner).execute(&mut *tx).await?;
    if approve {
        let package_name: String = sqlx::query_scalar("SELECT name FROM packages WHERE id=$1")
            .bind(r.get::<i64, _>("package_id"))
            .fetch_one(&mut *tx)
            .await?;
        crate::notifications::queue(&s,&mut tx,owner.ok_or_else(Error::missing)?,"trial_activated",json!({"package_name":package_name,"end_date":(chrono::Utc::now().date_naive()+chrono::Duration::days(13)).to_string()}),Some(&format!("trial-activate:{id}"))).await?;
    }
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
