use crate::{
    auth, crypto,
    error::{Error, Result},
    resources, App,
};
use axum::{extract::State, http::HeaderMap, Json};
use serde_json::{json, Value};
use sqlx::{Postgres, Row, Transaction};

pub fn render(template: &str, vars: &Value) -> String {
    regex::Regex::new(r"\{\{([a-zA-Z0-9_]+)\}\}")
        .unwrap()
        .replace_all(template, |caps: &regex::Captures| match &vars[&caps[1]] {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            v => v.to_string(),
        })
        .into_owned()
}
pub async fn queue(
    s: &App,
    tx: &mut Transaction<'_, Postgres>,
    user: i64,
    key: &str,
    mut vars: Value,
    dedupe: Option<&str>,
) -> Result<bool> {
    if s.config.smtp_host.is_empty() {
        return Ok(false);
    }
    let template =
        sqlx::query("SELECT subject,body FROM email_templates WHERE template_key=$1 AND enabled=1")
            .bind(key)
            .fetch_optional(&mut **tx)
            .await?;
    let Some(t) = template else { return Ok(false) };
    let u = sqlx::query("SELECT name,email FROM users WHERE id=$1 AND status='active'")
        .bind(user)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(Error::missing)?;
    let site: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key='landing_brand_name'")
            .fetch_optional(&mut **tx)
            .await?;
    let map = vars
        .as_object_mut()
        .ok_or_else(|| Error::bad("Biến mẫu không hợp lệ"))?;
    for (k, v) in [
        ("name", u.get::<String, _>("name")),
        ("email", u.get("email")),
        ("site_name", site.unwrap_or("Email Business".into())),
        ("site_url", s.config.app_url.clone()),
        ("login_url", format!("{}/dang-nhap", s.config.app_url)),
    ] {
        map.entry(k).or_insert(json!(v));
    }
    let subject = render(&t.get::<String, _>("subject"), &vars);
    let body = render(&t.get::<String, _>("body"), &vars);
    let added=sqlx::query("INSERT INTO notifications(user_id,recipient,subject,body_encrypted,dedupe_key) VALUES($1,$2,$3,$4,$5) ON CONFLICT(dedupe_key) DO NOTHING").bind(user).bind(u.get::<String,_>("email")).bind(subject).bind(crypto::seal(&s.config.key,&body)?).bind(dedupe).execute(&mut **tx).await?.rows_affected();
    Ok(added == 1)
}
pub async fn send(
    s: &App,
    user: i64,
    key: &str,
    vars: Value,
    dedupe: Option<&str>,
) -> Result<bool> {
    let mut tx = s.db.begin().await?;
    let queued = queue(s, &mut tx, user, key, vars, dedupe).await?;
    tx.commit().await?;
    Ok(queued)
}
pub async fn list(State(s): State<App>, h: HeaderMap) -> Result<Json<Value>> {
    auth::require(&s, &h, "settings", true, false).await?;
    let templates: Vec<Value> =
        sqlx::query_scalar("SELECT to_jsonb(t) FROM email_templates t ORDER BY template_key")
            .fetch_all(&s.db)
            .await?;
    let queue:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('status',status,'count',count(*)) FROM notifications GROUP BY status").fetch_all(&s.db).await?;
    Ok(Json(
        json!({"templates":templates,"queue":queue,"smtp_configured":!s.config.smtp_host.is_empty()}),
    ))
}
pub async fn save(State(s): State<App>, h: HeaderMap, Json(v): Json<Value>) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "settings", true, true).await?;
    let key = resources::text(&v, "template_key", 80)?;
    let enabled = if v["enabled"] == false { 0_i16 } else { 1 };
    if enabled == 0 && ["login_otp", "password_reset"].contains(&key) {
        return Err(Error::bad(
            "Không thể tắt mẫu xác thực hoặc khôi phục mật khẩu",
        ));
    }
    let subject = resources::text(&v, "subject", 255)?;
    if subject.contains(['\r', '\n']) {
        return Err(Error::bad("Tiêu đề không được xuống dòng"));
    }
    let changed=sqlx::query("UPDATE email_templates SET name=$1,subject=$2,body=$3,enabled=$4,updated_at=now() WHERE template_key=$5").bind(resources::text(&v,"name",190)?).bind(subject).bind(resources::text(&v,"body",20_000)?).bind(enabled).bind(key).execute(&s.db).await?.rows_affected();
    if changed == 0 {
        return Err(Error::missing());
    }
    auth::audit(&s, Some(u.id), "email_template_update", json!({"key":key})).await?;
    Ok(Json(json!({"ok":true})))
}
pub async fn broadcast(
    State(s): State<App>,
    h: HeaderMap,
    Json(v): Json<Value>,
) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "settings", true, true).await?;
    u.super_admin()?;
    if s.config.smtp_host.is_empty() {
        return Err(Error::bad("SMTP chưa được cấu hình"));
    }
    let subject = resources::text(&v, "subject", 255)?;
    let message = resources::text(&v, "message", 20_000)?;
    if subject.contains(['\r', '\n']) {
        return Err(Error::bad("Tiêu đề không được xuống dòng"));
    }
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id=$1")
        .bind(u.id)
        .fetch_one(&s.db)
        .await?;
    if !crypto::verify(v["password"].as_str().unwrap_or(""), &hash) {
        return Err(Error::unauthorized());
    }
    let request_id = resources::text(&v, "request_id", 64)?;
    if !request_id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(Error::bad("Mã gửi không hợp lệ"));
    }
    let mut tx = s.db.begin().await?;
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM users WHERE role='customer' AND status='active' ORDER BY id",
    )
    .fetch_all(&mut *tx)
    .await?;
    let mut queued = 0;
    for id in ids {
        if queue(
            &s,
            &mut tx,
            id,
            "notice",
            json!({"subject":subject,"message":message}),
            Some(&format!("broadcast:{}:{request_id}:{id}", u.id)),
        )
        .await?
        {
            queued += 1;
        }
    }
    sqlx::query("INSERT INTO audit_logs(user_id,action,context) VALUES($1,'email_broadcast',$2)")
        .bind(u.id)
        .bind(json!({"queued":queued,"request_id":request_id}))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({"queued":queued})))
}
pub async fn reminders(s: &App) -> Result<()> {
    if s.config.smtp_host.is_empty() {
        return Ok(());
    }
    let owners:Vec<i64>=sqlx::query_scalar("SELECT DISTINCT customer_id FROM subscriptions WHERE customer_id IN (SELECT id FROM users WHERE status='active') AND status='active' AND end_date BETWEEN current_date AND current_date+45 AND start_date<=current_date AND (last_renewal_reminder_at IS NULL OR last_renewal_reminder_at<now()-interval '7 days')").fetch_all(&s.db).await?;
    for owner in owners {
        let mut tx = s.db.begin().await?;
        if sqlx::query("SELECT id FROM users WHERE id=$1 AND status='active' FOR UPDATE")
            .bind(owner)
            .fetch_optional(&mut *tx)
            .await?
            .is_none()
        {
            continue;
        }
        let rows=sqlx::query("SELECT s.*,p.name package_name FROM subscriptions s JOIN packages p ON p.id=s.package_id WHERE s.customer_id=$1 AND s.status='active' AND s.end_date BETWEEN current_date AND current_date+45 AND s.start_date<=current_date AND (s.last_renewal_reminder_at IS NULL OR s.last_renewal_reminder_at<now()-interval '7 days') FOR UPDATE OF s").bind(owner).fetch_all(&mut *tx).await?;
        for row in rows {
            let id: i64 = row.get("id");
            let existing:Option<i64>=sqlx::query_scalar("SELECT id FROM subscriptions WHERE renewal_for_subscription_id=$1 AND status='pending' ORDER BY id DESC LIMIT 1").bind(id).fetch_optional(&mut *tx).await?;
            let renewal = if let Some(id) = existing {
                id
            } else {
                let p = sqlx::query_as::<_, crate::billing::Package>(
                    "SELECT * FROM packages WHERE id=$1 AND status='active'",
                )
                .bind(row.get::<i64, _>("package_id"))
                .fetch_optional(&mut *tx)
                .await?;
                let Some(p) = p else { continue };
                let q = crate::billing::quote(
                    &p,
                    row.get("selected_email_accounts"),
                    row.get("selected_domains"),
                );
                let invoice = format!(
                    "INV{}{:04}",
                    chrono::Utc::now().timestamp_micros(),
                    rand::random::<u16>() % 10000
                );
                sqlx::query_scalar("INSERT INTO subscriptions(customer_id,package_id,price,invoice_code,bank_transfer_note,selected_email_accounts,selected_domains,renewal_for_subscription_id,start_date,end_date) VALUES($1,$2,$3,$4,$4,$5,$6,$7,current_date,current_date+$8-1) RETURNING id").bind(owner).bind(p.id).bind(q.total).bind(invoice).bind(q.emails).bind(q.domains).bind(id).bind(p.duration_days).fetch_one(&mut *tx).await?
            };
            let end: chrono::NaiveDate = row.get("end_date");
            let dedupe = format!("renewal:{id}:{}", chrono::Utc::now().date_naive());
            if queue(s,&mut tx,owner,"renewal_reminder",json!({"package_name":row.get::<String,_>("package_name"),"end_date":end.to_string(),"days_left":(end-chrono::Utc::now().date_naive()).num_days(),"renewal_url":format!("{}/customer/invoice?id={renewal}",s.config.app_url)}),Some(&dedupe)).await?{sqlx::query("UPDATE subscriptions SET last_renewal_reminder_at=now() WHERE id=$1").bind(id).execute(&mut *tx).await?;}
        }
        tx.commit().await?;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn substitution_is_single_pass() {
        assert_eq!(
            render(
                "Hello {{name}} {{code}}",
                &json!({"name":"{{code}}","code":"123"})
            ),
            "Hello {{code}} 123"
        );
    }
}
