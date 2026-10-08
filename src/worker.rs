use crate::{crypto, stalwart, App};
use anyhow::{anyhow, Result};
use lettre::{
    transport::smtp::authentication::Credentials, AsyncSmtpTransport, AsyncTransport, Message,
    Tokio1Executor,
};
use serde_json::{json, Value};
use sqlx::Row;
pub async fn notify(s: &App, user: i64, email: &str, subject: &str, body: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO notifications(user_id,recipient,subject,body_encrypted) VALUES($1,$2,$3,$4)",
    )
    .bind(user)
    .bind(email)
    .bind(subject)
    .bind(crypto::seal(&s.config.key, body)?)
    .execute(&s.db)
    .await?;
    Ok(())
}
fn credentials(p: &Value) -> Value {
    if p.is_string() {
        json!({"0":{"@type":"Password","secret":p,"expiresAt":null,"allowedIps":{}}})
    } else {
        json!({})
    }
}
pub async fn tick(s: &App) -> Result<()> {
    sync_one(s).await?;
    mail_one(s).await?;
    sqlx::query("UPDATE subscriptions SET status='expired',updated_at=now() WHERE status='active' AND end_date<current_date").execute(&s.db).await?;
    sqlx::query("DELETE FROM panel_sessions WHERE expires_at<now()-interval '1 day'")
        .execute(&s.db)
        .await?;
    sqlx::query("DELETE FROM auth_rate_limits WHERE expires_at<now()")
        .execute(&s.db)
        .await?;
    Ok(())
}
async fn sync_one(s: &App) -> Result<()> {
    let mut tx = s.db.begin().await?;
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(88261732)")
        .fetch_one(&mut *tx)
        .await?;
    if !locked {
        return Ok(());
    }
    let r=sqlx::query("SELECT * FROM api_sync_jobs WHERE status='pending' AND run_after<=now() ORDER BY id LIMIT 1 FOR UPDATE SKIP LOCKED").fetch_optional(&mut *tx).await?;
    let Some(r) = r else { return Ok(()) };
    let id: i64 = r.get("id");
    let resource: i64 = r.get("resource_id");
    let server = r
        .get::<Option<i64>, _>("server_id")
        .ok_or_else(|| anyhow!("Missing placement"))?;
    let kind: String = r.get("job_type");
    let p: Value = serde_json::from_str(&crypto::open(
        &s.config.key,
        &r.get::<String, _>("payload_encrypted"),
    )?)?;
    let (method, args) = match kind.as_str() {
        "domain_create" => (
            "x:Domain/set",
            json!({"create":{"item":{"name":p["name"],"aliases":{},"isEnabled":true,"certificateManagement":{"@type":"Manual"},"dkimManagement":{"@type":"Automatic"},"dnsManagement":{"@type":"Manual"},"subAddressing":{"@type":"Enabled"}}}}),
        ),
        "account_create" => (
            "x:Account/set",
            json!({"create":{"item":{"@type":"User","name":p["local"],"domainId":p["domain_id"],"credentials":credentials(&p["password"]),"quotas":{"maxDiskQuota":p["quota"]},"roles":{"@type":"User"},"permissions":{"@type":"Inherit"},"aliases":{},"memberGroupIds":{},"encryptionAtRest":{"@type":"Disabled"}}}}),
        ),
        "domain_delete" => ("x:Domain/set", json!({"destroy":[p["remote_id"]]})),
        "account_delete" => ("x:Account/set", json!({"destroy":[p["remote_id"]]})),
        "domain_status" => (
            "x:Domain/set",
            json!({"update":{p["remote_id"].as_str().unwrap_or(""):{"isEnabled":p["enabled"]}}}),
        ),
        "account_password" | "account_status" => (
            "x:Account/set",
            json!({"update":{p["remote_id"].as_str().unwrap_or(""):{"credentials":credentials(&p["password"])}}}),
        ),
        "account_aliases" => (
            "x:Account/set",
            json!({"update":{p["remote_id"].as_str().unwrap_or(""):{"aliases":p["aliases"]}}}),
        ),
        _ => return Err(anyhow!("Unknown sync job")),
    };
    let result = stalwart::call(s, server, json!([[method, args, "c1"]])).await;
    let table = if kind.starts_with("domain_") {
        "domains"
    } else {
        "email_accounts"
    };
    match result {
        Ok(result) => {
            if kind.ends_with("_create") {
                let remote = result["methodResponses"][0][1]["created"]["item"]["id"]
                    .as_str()
                    .ok_or_else(|| anyhow!("Missing remote id"))?;
                if table == "domains" {
                    let zone =
                        result["methodResponses"][0][1]["created"]["item"]["dnsZoneFile"].as_str();
                    sqlx::query("UPDATE domains SET dns_records=$1 WHERE id=$2")
                        .bind(zone)
                        .bind(resource)
                        .execute(&mut *tx)
                        .await?;
                }
                let field = if table == "domains" {
                    "stalwart_domain_id"
                } else {
                    "stalwart_account_id"
                };
                sqlx::query(&format!("UPDATE {table} SET {field}=$1,sync_status='synced',updated_at=now() WHERE id=$2 AND server_id=$3")).bind(remote).bind(resource).bind(server).execute(&mut *tx).await?;
            } else {
                sqlx::query(&format!("UPDATE {table} SET sync_status='synced',updated_at=now() WHERE id=$1 AND server_id=$2")).bind(resource).bind(server).execute(&mut *tx).await?;
            }
            if kind == "account_aliases" {
                sqlx::query("UPDATE email_aliases SET sync_status='synced',last_sync_error=NULL WHERE email_account_id=$1").bind(resource).execute(&mut *tx).await?;
            }
            sqlx::query("UPDATE api_sync_jobs SET status='done',payload_encrypted=NULL,updated_at=now() WHERE id=$1").bind(id).execute(&mut *tx).await?;
        }
        Err(_e) => {
            sqlx::query("UPDATE api_sync_jobs SET status='failed',attempts=attempts+1,last_error='Remote result unconfirmed. Inspect Stalwart before retrying; creates may already exist.',updated_at=now() WHERE id=$1").bind(id).execute(&mut *tx).await?;
            sqlx::query(&format!(
                "UPDATE {table} SET sync_status='failed' WHERE id=$1"
            ))
            .bind(resource)
            .execute(&mut *tx)
            .await?;
            tracing::warn!(
                job_id = id,
                "Stalwart sync failed; no automatic replay of uncertain writes"
            );
        }
    }
    tx.commit().await?;
    Ok(())
}
async fn mail_one(s: &App) -> Result<()> {
    if s.config.smtp_host.is_empty() {
        return Ok(());
    }
    let mut tx = s.db.begin().await?;
    let row=sqlx::query("SELECT * FROM notifications WHERE status='pending' AND attempts<5 AND run_after<=now() ORDER BY id LIMIT 1 FOR UPDATE SKIP LOCKED").fetch_optional(&mut *tx).await?;
    let Some(r) = row else { return Ok(()) };
    let id: i64 = r.get("id");
    let body = crypto::open(&s.config.key, &r.get::<String, _>("body_encrypted"))?;
    let message = Message::builder()
        .from(s.config.smtp_from.parse()?)
        .to(r.get::<String, _>("recipient").parse()?)
        .subject(r.get::<String, _>("subject"))
        .body(body)?;
    let transport = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&s.config.smtp_host)?
        .credentials(Credentials::new(
            s.config.smtp_user.clone(),
            s.config.smtp_password.clone(),
        ))
        .timeout(Some(std::time::Duration::from_secs(15)))
        .build();
    let sent = transport.send(message).await.is_ok();
    sqlx::query("UPDATE notifications SET status=CASE WHEN $2 THEN 'sent' WHEN attempts>=4 THEN 'failed' ELSE 'pending' END,attempts=attempts+1,body_encrypted=CASE WHEN $2 THEN '' ELSE body_encrypted END,run_after=now()+interval '2 minutes' WHERE id=$1").bind(id).bind(sent).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}
