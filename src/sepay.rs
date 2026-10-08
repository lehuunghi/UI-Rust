use crate::{
    auth, billing,
    error::{Error, Result},
    App,
};
use axum::{extract::State, http::HeaderMap, Json};
use chrono::{Duration, NaiveDate, Utc};
use serde_json::{json, Value};

fn dates(from: &str, to: &str) -> Result<()> {
    let a =
        NaiveDate::parse_from_str(from, "%Y-%m-%d").map_err(|_| Error::bad("Ngày không hợp lệ"))?;
    let b =
        NaiveDate::parse_from_str(to, "%Y-%m-%d").map_err(|_| Error::bad("Ngày không hợp lệ"))?;
    if a.format("%Y-%m-%d").to_string() != from
        || b.format("%Y-%m-%d").to_string() != to
        || b < a
        || (b - a).num_days() > 30
    {
        return Err(Error::bad("Chọn khoảng tối đa 31 ngày"));
    }
    Ok(())
}
pub fn payload(row: &Value) -> Value {
    json!({"id":row["id"],"transferAmount":row["amount_in"],"transferType":row["transfer_type"],"accountNumber":row["account_number"],"referenceCode":row.get("reference_number").cloned().unwrap_or(json!("")),"content":row.get("transaction_content").cloned().unwrap_or(json!("")),"code":row.get("code").cloned().unwrap_or(json!("")),"gateway":row.get("bank_brand_name").cloned().unwrap_or(json!(""))})
}
pub async fn import(
    State(s): State<App>,
    h: HeaderMap,
    Json(v): Json<Value>,
) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "subscriptions", true, true).await?;
    let result = import_range(
        &s,
        crate::resources::text(&v, "from", 10)?,
        crate::resources::text(&v, "to", 10)?,
    )
    .await?;
    auth::audit(&s, Some(u.id), "sepay_api_import", result.clone()).await?;
    Ok(Json(result))
}
pub async fn import_range(s: &App, from: &str, to: &str) -> Result<Value> {
    dates(from, to)?;
    let mode = s.config.sepay_api_mode.as_str();
    let origin = match mode {
        "live" => "https://userapi.sepay.vn",
        "sandbox" => "https://userapi-sandbox.sepay.vn",
        _ => return Err(Error::bad("SEPAY_API_MODE phải là live hoặc sandbox")),
    };
    if s.config.sepay_api_token.is_empty() || s.config.sepay_account.is_empty() {
        return Err(Error::bad(
            "Cần SEPAY_USER_API_TOKEN và SEPAY_ACCOUNT_NUMBER",
        ));
    }
    let mut totals = json!({"read":0,"recorded":0,"duplicates":0,"skipped":0,"complete":false});
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(45);
    for page in 1..=20 {
        if tokio::time::Instant::now() > deadline {
            break;
        }
        let response = s
            .http
            .get(format!("{origin}/v2/transactions"))
            .bearer_auth(&s.config.sepay_api_token)
            .header("accept", "application/json")
            .query(&[
                ("page", page.to_string()),
                ("per_page", "100".into()),
                ("transfer_type", "in".into()),
                ("transaction_date_sort", "asc".into()),
                ("transaction_date_from", format!("{from} 00:00:00")),
                ("transaction_date_to", format!("{to} 23:59:59")),
            ])
            .send()
            .await
            .map_err(|_| Error::bad("Không kết nối được SePay API"))?;
        if !response.status().is_success() {
            return Err(Error::bad(format!(
                "SePay API trả HTTP {}; các giao dịch đã nhận được giữ lại",
                response.status().as_u16()
            )));
        }
        let data = bounded_json(response, 2 * 1024 * 1024).await?;
        apply_page(s, &data, mode, page, &mut totals).await?;
        if totals["complete"] == true {
            break;
        }
    }
    Ok(totals)
}
pub async fn bounded_json(mut response: reqwest::Response, limit: usize) -> Result<Value> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| Error::bad("Không đọc được phản hồi"))?
    {
        if body.len() + chunk.len() > limit {
            return Err(Error::bad("Phản hồi quá lớn"));
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| Error::bad("Phản hồi JSON không hợp lệ"))
}
pub async fn apply_page(
    s: &App,
    data: &Value,
    mode: &str,
    page: i32,
    totals: &mut Value,
) -> Result<()> {
    if data["status"] != "success" {
        return Err(Error::bad("Phản hồi SePay không hợp lệ"));
    }
    let rows = data["data"]
        .as_array()
        .filter(|r| r.len() <= 100)
        .ok_or_else(|| Error::bad("Phản hồi SePay không hợp lệ"))?;
    for row in rows {
        increment(totals, "read");
        if !row.is_object() {
            return Err(Error::bad("Giao dịch SePay không hợp lệ"));
        }
        if row["transfer_type"] != "in"
            || row["account_number"].as_str() != Some(&s.config.sepay_account)
        {
            increment(totals, "skipped");
            continue;
        }
        let Json(result) = billing::record(s, payload(row), mode, "api").await?;
        increment(
            totals,
            if result["duplicate"] == true {
                "duplicates"
            } else {
                "recorded"
            },
        );
    }
    let pagination = data["meta"]
        .get("pagination")
        .or_else(|| data.get("pagination"))
        .ok_or_else(|| Error::bad("Thiếu phân trang SePay; các giao dịch đã nhận được giữ lại"))?;
    if pagination["current_page"].as_i64() != Some(i64::from(page))
        || !pagination["has_more"].is_boolean()
    {
        return Err(Error::bad("Phân trang SePay không hợp lệ"));
    }
    totals["complete"] = json!(pagination["has_more"] == false);
    Ok(())
}
fn increment(v: &mut Value, key: &str) {
    v[key] = json!(v[key].as_u64().unwrap_or(0) + 1);
}
pub async fn poll(s: &App) -> anyhow::Result<()> {
    if s.config.sepay_poll_seconds == 0 {
        return Ok(());
    }
    let seconds = s.config.sepay_poll_seconds.clamp(300, 86400) as i64;
    let claimed=sqlx::query("UPDATE rust_poll_state SET next_run_at=now()+make_interval(secs=>$1::double precision) WHERE kind='sepay' AND next_run_at<=now()").bind(seconds).execute(&s.db).await?.rows_affected();
    if claimed == 0 {
        return Ok(());
    }
    let to = Utc::now().date_naive();
    let from = to - Duration::days(6);
    let error = match import_range(s, &from.to_string(), &to.to_string()).await {
        Ok(result) => {
            auth::audit(s, None, "sepay_periodic_import", result)
                .await
                .map_err(|e| anyhow::anyhow!(e.1))?;
            None
        }
        Err(e) => Some(e.1),
    };
    sqlx::query("UPDATE rust_poll_state SET last_error=$1 WHERE kind='sepay'")
        .bind(error)
        .execute(&s.db)
        .await?;
    Ok(())
}
