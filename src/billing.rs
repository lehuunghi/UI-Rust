use crate::{
    auth, crypto,
    error::{Error, Result},
    App,
};
use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use chrono::{Duration, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{Postgres, Row, Transaction};
#[derive(Debug, sqlx::FromRow)]
pub struct Package {
    pub id: i64,
    pub price: Decimal,
    pub promo_price: Option<Decimal>,
    pub billing_months: i32,
    pub duration_days: i32,
    pub min_email_accounts: i32,
    pub max_email_accounts: i32,
    pub min_domains: i32,
    pub max_domains: i32,
    pub extra_domain_price: Decimal,
}
#[derive(Debug, Serialize)]
pub struct Quote {
    pub emails: i32,
    pub domains: i32,
    pub months: i32,
    #[serde(with = "rust_decimal::serde::str")]
    pub total: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub email_total: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub unit: Decimal,
}
pub fn quote(p: &Package, emails: i32, domains: i32) -> Quote {
    let emails = emails.clamp(p.min_email_accounts, p.max_email_accounts);
    let domains = domains.clamp(p.min_domains, p.max_domains);
    let unit = p
        .promo_price
        .filter(|v| *v > Decimal::ZERO)
        .unwrap_or(p.price);
    let email_total = unit * Decimal::from(emails) * Decimal::from(p.billing_months);
    Quote {
        emails,
        domains,
        months: p.billing_months,
        total: email_total + Decimal::from(domains - p.min_domains) * p.extra_domain_price,
        email_total,
        unit,
    }
}
#[derive(Deserialize)]
pub struct Order {
    pub package_id: i64,
    #[serde(default = "one")]
    pub emails: i32,
    #[serde(default = "one")]
    pub domains: i32,
    #[serde(default)]
    pub renewal_for: Option<i64>,
}
fn one() -> i32 {
    1
}
pub async fn packages(State(s): State<App>) -> Result<Json<Value>> {
    let rows: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(p) FROM packages p WHERE status='active' ORDER BY price,id",
    )
    .fetch_all(&s.db)
    .await?;
    Ok(Json(json!(rows)))
}
pub async fn pricing(State(s): State<App>, Json(d): Json<Order>) -> Result<Json<Value>> {
    let p = sqlx::query_as::<_, Package>("SELECT * FROM packages WHERE id=$1 AND status='active'")
        .bind(d.package_id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(Error::missing)?;
    Ok(Json(json!(quote(&p, d.emails, d.domains))))
}
pub async fn order(
    State(s): State<App>,
    h: HeaderMap,
    Json(d): Json<Order>,
) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "packages", false, true).await?;
    if u.role != "customer" {
        return Err(Error::forbidden());
    }
    let mut tx = s.db.begin().await?;
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(u.owner)
        .fetch_one(&mut *tx)
        .await?;
    let p = sqlx::query_as::<_, Package>(
        "SELECT * FROM packages WHERE id=$1 AND status='active' FOR SHARE",
    )
    .bind(d.package_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(Error::missing)?;
    let q = quote(&p, d.emails, d.domains);
    if let Some(parent) = d.renewal_for {
        let r=sqlx::query("SELECT id FROM subscriptions WHERE id=$1 AND customer_id=$2 AND status IN ('active','expired') FOR UPDATE").bind(parent).bind(u.owner).fetch_optional(&mut *tx).await?;
        if r.is_none() {
            return Err(Error::missing());
        }
        let pending:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM subscriptions WHERE renewal_for_subscription_id=$1 AND status='pending')").bind(parent).fetch_one(&mut *tx).await?;
        if pending {
            return Err(Error::conflict("Đã có hóa đơn gia hạn đang chờ"));
        }
    }
    let invoice = format!(
        "INV{}{:04}",
        Utc::now().timestamp_micros(),
        rand::random::<u16>() % 10000
    );
    let id:i64=sqlx::query_scalar("INSERT INTO subscriptions(customer_id,package_id,price,invoice_code,bank_transfer_note,selected_email_accounts,selected_domains,base_price,extra_email_count,extra_domain_count,extra_email_price,extra_domain_price,start_date,end_date,renewal_for_subscription_id) VALUES($1,$2,$3,$4,$4,$5,$6,$7,$5,$8,$9,$10,current_date,current_date+$11-1,$12) RETURNING id").bind(u.owner).bind(p.id).bind(q.total).bind(&invoice).bind(q.emails).bind(q.domains).bind(q.email_total).bind(q.domains-p.min_domains).bind(q.unit).bind(p.extra_domain_price).bind(p.duration_days).bind(d.renewal_for).fetch_one(&mut *tx).await?;
    tx.commit().await?;
    auth::audit(&s, Some(u.id), "invoice_created", json!({"id":id})).await?;
    Ok(Json(json!({"id":id,"invoice_code":invoice,"quote":q})))
}
pub async fn invoice(
    State(s): State<App>,
    h: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "subscriptions", false, false).await?;
    let v:Value=sqlx::query_scalar("SELECT to_jsonb(s)||jsonb_build_object('package_name',p.name,'billing_months',p.billing_months) FROM subscriptions s JOIN packages p ON p.id=s.package_id WHERE s.id=$1 AND (s.customer_id=$2 OR $3)").bind(id).bind(u.owner).bind(u.role=="admin"&&u.can("subscriptions")).fetch_optional(&s.db).await?.ok_or_else(Error::missing)?;
    let mut qr = String::new();
    if !s.config.sepay_account.is_empty()
        && v["status"] == "pending"
        && v["payment_status"] != "paid"
    {
        let mut url = url::Url::parse("https://qr.sepay.vn/img").unwrap();
        url.query_pairs_mut()
            .append_pair("acc", &s.config.sepay_account)
            .append_pair("bank", &s.config.sepay_bank)
            .append_pair("amount", &v["price"].to_string())
            .append_pair("des", v["invoice_code"].as_str().unwrap_or(""));
        qr = url.into();
    }
    Ok(Json(json!({"invoice":v,"qr_url":qr})))
}
pub async fn activate(tx: &mut Transaction<'_, Postgres>, id: i64) -> Result<()> {
    let r=sqlx::query("SELECT s.*,p.duration_days FROM subscriptions s JOIN packages p ON p.id=s.package_id WHERE s.id=$1 FOR UPDATE OF s").bind(id).fetch_optional(&mut **tx).await?.ok_or_else(Error::missing)?;
    if r.get::<Option<chrono::DateTime<Utc>>, _>("activated_at")
        .is_some()
    {
        return Ok(());
    }
    let status: String = r.get("status");
    if status == "cancelled" || status == "expired" {
        return Err(Error::conflict("Hóa đơn không còn hiệu lực"));
    }
    let owner: i64 = r.get("customer_id");
    let mut start = Utc::now().date_naive();
    if let Some(parent) = r.get::<Option<i64>, _>("renewal_for_subscription_id") {
        let end: NaiveDate = sqlx::query_scalar(
            "SELECT end_date FROM subscriptions WHERE id=$1 AND customer_id=$2 FOR UPDATE",
        )
        .bind(parent)
        .bind(owner)
        .fetch_one(&mut **tx)
        .await?;
        if end >= start {
            start = end + Duration::days(1)
        }
    } else {
        sqlx::query("UPDATE subscriptions SET status='expired',updated_at=now() WHERE customer_id=$1 AND status='active' AND id<>$2").bind(owner).bind(id).execute(&mut **tx).await?;
    }
    let end = start + Duration::days(i64::from(r.get::<i32, _>("duration_days")) - 1);
    sqlx::query("UPDATE subscriptions SET status='active',payment_status='paid',paid_at=COALESCE(paid_at,now()),activated_at=now(),start_date=$1,end_date=$2,updated_at=now() WHERE id=$3").bind(start).bind(end).bind(id).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO email_groups(customer_id,name) SELECT $1,'Mặc định' WHERE NOT EXISTS(SELECT 1 FROM email_groups WHERE customer_id=$1 AND status='active') ON CONFLICT DO NOTHING").bind(owner).execute(&mut **tx).await?;
    Ok(())
}
#[derive(Deserialize)]
pub struct Manual {
    pub action: String,
}
pub async fn manual(
    State(s): State<App>,
    h: HeaderMap,
    Path(id): Path<i64>,
    Json(d): Json<Manual>,
) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "subscriptions", true, true).await?;
    let mut tx = s.db.begin().await?;
    let owner: i64 = sqlx::query_scalar("SELECT customer_id FROM subscriptions WHERE id=$1")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(Error::missing)?;
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(owner)
        .fetch_one(&mut *tx)
        .await?;
    match d.action.as_str() {
        "activate" | "paid" => activate(&mut tx, id).await?,
        "cancel" => {
            sqlx::query("UPDATE subscriptions SET status='cancelled',updated_at=now() WHERE id=$1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        _ => return Err(Error::bad("Thao tác không hợp lệ")),
    }
    tx.commit().await?;
    auth::audit(
        &s,
        Some(u.id),
        "subscription_change",
        json!({"id":id,"action":d.action}),
    )
    .await?;
    Ok(Json(json!({"ok":true})))
}
pub fn invoice_code(code: &str, content: &str) -> Result<String> {
    let mut codes = std::collections::BTreeSet::new();
    for text in [code, content] {
        for word in text.split(|c: char| !c.is_ascii_alphanumeric()) {
            let word = word.to_ascii_uppercase();
            if let Some(digits) = word.strip_prefix("INV") {
                if (digits.len() == 8 || (12..=25).contains(&digits.len()))
                    && digits.bytes().all(|b| b.is_ascii_digit())
                {
                    codes.insert(word);
                }
            }
        }
    }
    if codes.len() > 1 {
        return Err(Error::bad("Nhiều mã hóa đơn trong một giao dịch"));
    }
    Ok(codes.into_iter().next().unwrap_or_default())
}

fn integer(v: &Value) -> Result<i64> {
    let s = if let Some(s) = v.as_str() {
        s.to_owned()
    } else {
        v.to_string()
    };
    if s.starts_with('0') || !s.bytes().all(|c| c.is_ascii_digit()) {
        return Err(Error::bad("Số giao dịch hoặc số tiền không hợp lệ"));
    }
    s.parse::<i64>()
        .ok()
        .filter(|v| *v > 0)
        .ok_or_else(|| Error::bad("Số ngoài phạm vi"))
}
pub async fn webhook(State(s): State<App>, h: HeaderMap, body: Bytes) -> Result<Json<Value>> {
    if body.len() > 65536 {
        return Err(Error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Payload too large".into(),
        ));
    }
    if s.config.sepay_account.is_empty()
        || (s.config.sepay_key.is_empty() && s.config.sepay_hmac.is_empty())
    {
        return Err(Error(
            StatusCode::SERVICE_UNAVAILABLE,
            "SePay chưa bật".into(),
        ));
    }
    if !s.config.sepay_hmac.is_empty() {
        use hmac::{Hmac, Mac};
        let ts = h
            .get("x-sepay-timestamp")
            .and_then(|s| s.to_str().ok())
            .unwrap_or("");
        let time = ts.parse::<i64>().unwrap_or(0);
        if Utc::now().timestamp().abs_diff(time) > 300 {
            return Err(Error::unauthorized());
        }
        let mut mac = Hmac::<sha2::Sha256>::new_from_slice(s.config.sepay_hmac.as_bytes()).unwrap();
        mac.update(ts.as_bytes());
        mac.update(b".");
        mac.update(&body);
        let expected = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
        if !crypto::equal(
            &expected,
            h.get("x-sepay-signature")
                .and_then(|s| s.to_str().ok())
                .unwrap_or(""),
        ) {
            return Err(Error::unauthorized());
        }
    } else if !crypto::equal(
        &format!("Apikey {}", s.config.sepay_key),
        h.get("authorization")
            .and_then(|s| s.to_str().ok())
            .unwrap_or(""),
    ) {
        return Err(Error::unauthorized());
    }
    if h.get("content-type")
        .and_then(|s| s.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        != Some("application/json")
    {
        return Err(Error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Use application/json".into(),
        ));
    }
    let p: Value = serde_json::from_slice(&body).map_err(|_| Error::bad("JSON không hợp lệ"))?;
    if !p.is_object() {
        return Err(Error::bad("Cần JSON object"));
    }
    if p["id"] == 0 || p["id"] == "0" {
        return Ok(Json(json!({"success":true,"test":true})));
    }
    let tid = integer(&p["id"])?;
    let amount = integer(&p["transferAmount"])?;
    if amount > 9_999_999_999 {
        return Err(Error::bad("Số tiền ngoài phạm vi"));
    }
    for (k, n) in [
        ("accountNumber", 100),
        ("referenceCode", 100),
        ("content", 500),
        ("code", 190),
        ("gateway", 100),
    ] {
        if let Some(v) = p.get(k) {
            if !v.is_string() || v.as_str().unwrap().chars().count() > n {
                return Err(Error::bad("Trường giao dịch không hợp lệ"));
            }
        }
    }
    if p["transferType"] == "out" {
        return Ok(Json(json!({"success":true,"ignored":true})));
    }
    if p["transferType"] != "in" {
        return Err(Error::bad("transferType không hợp lệ"));
    }
    if p["accountNumber"].as_str() != Some(s.config.sepay_account.as_str()) {
        return Err(Error::bad("Sai tài khoản nhận"));
    }
    let code = invoice_code(
        p["code"].as_str().unwrap_or(""),
        p["content"].as_str().unwrap_or(""),
    )?;
    let reference = p["referenceCode"].as_str().unwrap_or("");
    let source = format!("live:webhook:{tid}");
    let bank = if reference.is_empty() {
        None
    } else {
        Some(crypto::hash(&format!(
            "{}\0{}",
            s.config.sepay_account, reference
        )))
    };
    let mut tx = s.db.begin().await?;
    // One lock orders idempotency across both provider ids and bank references, even when invoice codes differ.
    sqlx::query("SELECT pg_advisory_xact_lock(88261731)")
        .execute(&mut *tx)
        .await?;
    let saved=sqlx::query("SELECT r.* FROM sepay_reconciliation r WHERE r.source_key=$1 OR (r.mode='live' AND r.bank_identity=$2) OR r.id IN (SELECT reconciliation_id FROM sepay_receipts WHERE source_key=$1)").bind(&source).bind(&bank).fetch_all(&mut *tx).await?;
    if !saved.is_empty() {
        if saved.len() != 1 {
            return Err(Error::conflict("Conflicting transaction identities"));
        }
        let r = &saved[0];
        if r.get::<Decimal, _>("amount") != Decimal::from(amount)
            || r.get::<String, _>("account_number") != s.config.sepay_account
            || r.get::<String, _>("reference_code") != reference
            || r.get::<String, _>("invoice_code") != code
        {
            return Err(Error::conflict("Transaction id conflict"));
        }
        sqlx::query("INSERT INTO sepay_receipts(source_key,reconciliation_id) VALUES($1,$2) ON CONFLICT DO NOTHING").bind(&source).bind(r.get::<i64,_>("id")).execute(&mut *tx).await?;
        tx.commit().await?;
        return Ok(Json(json!({"success":true,"duplicate":true})));
    }
    let sub = sqlx::query("SELECT id,customer_id FROM subscriptions WHERE invoice_code=$1")
        .bind(&code)
        .fetch_optional(&mut *tx)
        .await?;
    let sid = sub.as_ref().map(|r| r.get::<i64, _>("id"));
    if let Some(r) = &sub {
        sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
            .bind(r.get::<i64, _>("customer_id"))
            .fetch_one(&mut *tx)
            .await?;
    }
    let rid:i64=sqlx::query_scalar("INSERT INTO sepay_reconciliation(source_key,mode,source,provider_id,bank_identity,account_number,reference_code,amount,content,invoice_code,subscription_id,state) VALUES($1,'live','webhook',$2,$3,$4,$5,$6,$7,$8,$9,'unmatched') RETURNING id").bind(&source).bind(tid.to_string()).bind(bank).bind(&s.config.sepay_account).bind(reference).bind(Decimal::from(amount)).bind(p["content"].as_str().unwrap_or("")).bind(&code).bind(sid).fetch_one(&mut *tx).await?;
    sqlx::query("INSERT INTO sepay_receipts(source_key,reconciliation_id) VALUES($1,$2)")
        .bind(&source)
        .bind(rid)
        .execute(&mut *tx)
        .await?;
    let mut state = "unmatched";
    if let Some(id) = sid {
        let r = sqlx::query("SELECT * FROM subscriptions WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
        let lid:i64=sqlx::query_scalar("INSERT INTO payment_transactions(subscription_id,provider_transaction_id,account_number,reference_code,content,transfer_type,amount,raw_payload) VALUES($1,$2,$3,$4,$5,'in',$6,$7) RETURNING id").bind(id).bind(tid).bind(&s.config.sepay_account).bind(reference).bind(p["content"].as_str().unwrap_or("")).bind(Decimal::from(amount)).bind(&p).fetch_one(&mut *tx).await?;
        let status: String = r.get("status");
        let paid: String = r.get("payment_status");
        if ["cancelled", "expired"].contains(&status.as_str())
            || paid == "refunded"
            || r.get::<i16, _>("is_trial") == 1
            || r.get::<String, _>("payment_method") != "sepay"
        {
            state = "late"
        } else if paid == "paid"
            && r.get::<Option<chrono::DateTime<Utc>>, _>("activated_at")
                .is_some()
        {
            state = "additional"
        } else {
            let total:Decimal=sqlx::query_scalar("SELECT COALESCE(sum(amount),0) FROM payment_transactions WHERE subscription_id=$1 AND provider='sepay' AND transfer_type='in'").bind(id).fetch_one(&mut *tx).await?;
            let due: Decimal = r.get("price");
            if total >= due {
                activate(&mut tx, id).await?;
                state = if total > due { "overpaid" } else { "paid" }
            } else {
                state = "partial"
            }
        }
        sqlx::query("UPDATE sepay_reconciliation SET ledger_id=$1,state=$2 WHERE id=$3")
            .bind(lid)
            .bind(state)
            .bind(rid)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(Json(json!({"success":true,"state":state})))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pricing_matches_php() {
        let p = Package {
            id: 1,
            price: Decimal::from(99),
            promo_price: Some(Decimal::from(89)),
            billing_months: 12,
            duration_days: 365,
            min_email_accounts: 5,
            max_email_accounts: 20,
            min_domains: 1,
            max_domains: 3,
            extra_domain_price: Decimal::from(50),
        };
        assert_eq!(quote(&p, 0, 0).total, Decimal::from(5340));
        assert_eq!(quote(&p, 30, 8).total, Decimal::from(21460));
    }
    #[test]
    fn codes() {
        assert_eq!(
            invoice_code("", "TT INV123456789012").unwrap(),
            "INV123456789012"
        );
        assert!(invoice_code("INV123456789012", "INV123456789013").is_err());
        assert!(invoice_code("", "INV123456789012 INV123456789013").is_err());
        assert_eq!(invoice_code("XINV123456789012", "").unwrap(), "");
    }
}

pub async fn reconcile(
    State(s): State<App>,
    h: HeaderMap,
    Path(id): Path<i64>,
    Json(v): Json<Value>,
) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "subscriptions", true, true).await?;
    let code = crate::resources::text(&v, "invoice_code", 40)?;
    let note = crate::resources::text(&v, "note", 500)?;
    let mut tx = s.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(88261731)")
        .execute(&mut *tx)
        .await?;
    let sub = sqlx::query("SELECT id,customer_id FROM subscriptions WHERE invoice_code=$1")
        .bind(code)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(Error::missing)?;
    let sid: i64 = sub.get("id");
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(sub.get::<i64, _>("customer_id"))
        .fetch_one(&mut *tx)
        .await?;
    let invoice = sqlx::query("SELECT * FROM subscriptions WHERE id=$1 FOR UPDATE")
        .bind(sid)
        .fetch_one(&mut *tx)
        .await?;
    if invoice.get::<String, _>("status") != "pending"
        || invoice.get::<i16, _>("is_trial") == 1
        || invoice.get::<String, _>("payment_method") != "sepay"
    {
        return Err(Error::conflict("Hóa đơn không còn nhận thanh toán"));
    }
    let r=sqlx::query("SELECT * FROM sepay_reconciliation WHERE id=$1 AND mode='live' AND ledger_id IS NULL AND state IN ('unmatched','needs_review') FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?.ok_or_else(||Error::conflict("Giao dịch không thể gán lại"))?;
    let tid = r
        .get::<String, _>("provider_id")
        .parse::<i64>()
        .map_err(|_| Error::bad("ID giao dịch không hợp lệ"))?;
    let lid:i64=sqlx::query_scalar("INSERT INTO payment_transactions(subscription_id,provider_transaction_id,account_number,reference_code,content,transfer_type,amount,raw_payload) VALUES($1,$2,$3,$4,$5,'in',$6,$7) RETURNING id").bind(sid).bind(tid).bind(r.get::<String,_>("account_number")).bind(r.get::<String,_>("reference_code")).bind(r.get::<String,_>("content")).bind(r.get::<Decimal,_>("amount")).bind(json!({"reconciliation_id":id,"actor":u.id})).fetch_one(&mut *tx).await?;
    let total:Decimal=sqlx::query_scalar("SELECT COALESCE(sum(amount),0) FROM payment_transactions WHERE subscription_id=$1 AND provider='sepay' AND transfer_type='in'").bind(sid).fetch_one(&mut *tx).await?;
    let due: Decimal = invoice.get("price");
    let state = if total >= due {
        activate(&mut tx, sid).await?;
        if total > due {
            "overpaid"
        } else {
            "paid"
        }
    } else {
        "partial"
    };
    // Preserve the original invoice_code used for replay comparison; store the resolved subscription separately.
    sqlx::query("UPDATE sepay_reconciliation SET subscription_id=$1,ledger_id=$2,state=$3,note=$4,reviewed_by=$5,reviewed_at=now() WHERE id=$6").bind(sid).bind(lid).bind(state).bind(note).bind(u.id).bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    auth::audit(
        &s,
        Some(u.id),
        "manual_reconciliation",
        json!({"id":id,"subscription":sid}),
    )
    .await?;
    Ok(Json(json!({"ok":true,"state":state})))
}
