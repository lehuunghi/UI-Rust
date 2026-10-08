use crate::{
    auth, crypto,
    error::{Error, Result},
    App,
};
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use serde_json::{json, Value};
use sqlx::Row;
pub async fn instructions(
    State(s): State<App>,
    h: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "domains", false, false).await?;
    if u.role == "sub_admin" {
        return Err(Error::forbidden());
    }
    let r = sqlx::query(
        "SELECT * FROM domains WHERE id=$1 AND status<>'deleted' AND (customer_id=$2 OR $3)",
    )
    .bind(id)
    .bind(u.owner)
    .bind(u.role == "admin")
    .fetch_optional(&s.db)
    .await?
    .ok_or_else(Error::missing)?;
    let token = r.get::<Option<String>, _>("verification_token");
    let token = if let Some(token) = token {
        token
    } else {
        let t = crypto::token();
        sqlx::query_scalar::<_,String>("UPDATE domains SET verification_token=COALESCE(verification_token,$1) WHERE id=$2 RETURNING verification_token").bind(t).bind(id).fetch_one(&s.db).await?
    };
    Ok(Json(
        json!({"name":format!("_ui-verification.{}",r.get::<String,_>("domain_name")),"type":"TXT","value":format!("ui-verification={token}"),"mail_dns":r.get::<Option<String>,_>("dns_records"),"verified":r.get::<String,_>("verification_status")=="verified"}),
    ))
}
pub async fn verify(
    State(s): State<App>,
    h: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "domains", false, true).await?;
    if u.role == "sub_admin" {
        return Err(Error::forbidden());
    }
    let r=sqlx::query("SELECT domain_name,verification_token FROM domains WHERE id=$1 AND status<>'deleted' AND (customer_id=$2 OR $3)").bind(id).bind(u.owner).bind(u.role=="admin").fetch_optional(&s.db).await?.ok_or_else(Error::missing)?;
    let token = r
        .get::<Option<String>, _>("verification_token")
        .ok_or_else(|| Error::bad("Mở hướng dẫn DNS trước"))?;
    let name = format!("_ui-verification.{}", r.get::<String, _>("domain_name"));
    let expected = format!("ui-verification={token}");
    let response = s
        .http
        .get("https://cloudflare-dns.com/dns-query")
        .header("accept", "application/dns-json")
        .query(&[("name", name.as_str()), ("type", "TXT")])
        .send()
        .await
        .map_err(|e| anyhow::anyhow!(e))?
        .error_for_status()
        .map_err(|e| anyhow::anyhow!(e))?;
    let mut response = response;
    let mut raw = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| anyhow::anyhow!(e))? {
        if raw.len() + chunk.len() > 65536 {
            return Err(Error::bad("DNS response too large"));
        }
        raw.extend_from_slice(&chunk)
    }
    let data: Value = serde_json::from_slice(&raw).map_err(|e| anyhow::anyhow!(e))?;
    let matched = data["Status"] == 0
        && data["Answer"].as_array().is_some_and(|a| {
            a.iter().any(|r| {
                r["type"] == 16
                    && r["data"]
                        .as_str()
                        .is_some_and(|s| s.trim_matches('"') == expected)
            })
        });
    sqlx::query("UPDATE domains SET verification_status=$1,verified_at=CASE WHEN $2 THEN now() ELSE verified_at END,dns_check_result=$3,updated_at=now() WHERE id=$4").bind(if matched{"verified"}else{"failed"}).bind(matched).bind(json!({"txt_verified":matched,"resolver":"cloudflare-dns.com"}).to_string()).bind(id).execute(&s.db).await?;
    Ok(Json(json!({"verified":matched})))
}
