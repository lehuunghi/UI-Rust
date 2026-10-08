use crate::{
    auth, crypto,
    error::{Error, Result},
    resources, App,
};
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap},
    response::{IntoResponse, Response},
    Json,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use sqlx::Row;

fn document(v: &Value) -> Result<(&str, String)> {
    let mime = resources::text(v, "mime", 100)?;
    let encoded = resources::text(v, "data", 7 * 1024 * 1024)?;
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| Error::bad("Dữ liệu tệp không hợp lệ"))?;
    if bytes.is_empty() || bytes.len() > 5 * 1024 * 1024 {
        return Err(Error::bad("Tệp tối đa 5 MB"));
    }
    let valid = match mime {
        "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" => bytes.starts_with(&[0xff, 0xd8, 0xff]),
        "application/pdf" => bytes.starts_with(b"%PDF-"),
        _ => false,
    };
    if !valid {
        return Err(Error::bad("Chỉ hỗ trợ nội dung JPG, PNG hoặc PDF"));
    }
    Ok((mime, STANDARD.encode(bytes)))
}
pub async fn submit(
    State(s): State<App>,
    h: HeaderMap,
    Json(v): Json<Value>,
) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "packages", false, true).await?;
    if u.role != "customer" || u.impersonator.is_some() {
        return Err(Error::forbidden());
    }
    let r = sqlx::query("SELECT name,email FROM users WHERE id=$1")
        .bind(u.id)
        .fetch_one(&s.db)
        .await?;
    create(&s, Some(u.id), r.get("name"), r.get("email"), &v).await
}
pub async fn public_submit(
    State(s): State<App>,
    h: HeaderMap,
    Json(v): Json<Value>,
) -> Result<Json<Value>> {
    auth::origin(&s, &h)?;
    let name = resources::text(&v, "name", 190)?.trim().to_owned();
    let email = resources::text(&v, "email", 190)?.trim().to_lowercase();
    if name.is_empty() || !auth::valid_email(&email) {
        return Err(Error::bad("Tên hoặc email không hợp lệ"));
    }
    auth::limit(&s, &format!("trial:{email}")).await?;
    let _ = create(&s, None, name, email, &v).await?;
    Ok(Json(
        json!({"ok":true,"message":"Đã nhận hồ sơ dùng thử; quản trị viên sẽ xem xét."}),
    ))
}
async fn create(
    s: &App,
    owner: Option<i64>,
    name: String,
    email: String,
    v: &Value,
) -> Result<Json<Value>> {
    let mut tx = s.db.begin().await?;
    // Serialize public daily admission and email deduplication before storing identity documents.
    sqlx::query("SELECT pg_advisory_xact_lock(88261735)")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,35))")
        .bind(&email)
        .execute(&mut *tx)
        .await?;
    if let Some(owner) = owner {
        sqlx::query("SELECT id FROM users WHERE id=$1 AND status='active' FOR UPDATE")
            .bind(owner)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(Error::missing)?;
    }
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM trial_requests WHERE (lower(email)=$1 OR customer_id=$2) AND status IN ('pending','approved'))").bind(&email).bind(owner).fetch_one(&mut *tx).await?;
    if exists {
        if owner.is_none() {
            return Ok(Json(json!({"ok":true})));
        }
        return Err(Error::conflict("Đã có yêu cầu dùng thử"));
    }
    if owner.is_none() {
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM trial_requests WHERE created_at>=current_date",
        )
        .fetch_one(&mut *tx)
        .await?;
        if count >= 200 {
            return Err(Error(
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                "Hệ thống tạm giới hạn hồ sơ mới; vui lòng thử lại sau".into(),
            ));
        }
    }
    let package = resources::number(&v, "package_id")?;
    let active: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM packages WHERE id=$1 AND status='active')")
            .bind(package)
            .fetch_one(&mut *tx)
            .await?;
    if !active {
        return Err(Error::bad("Gói không còn hoạt động"));
    }
    let required: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key='trial_require_identity'")
            .fetch_optional(&mut *tx)
            .await?;
    if required.as_deref() != Some("0") && v["documents"]["citizen"].is_null() {
        return Err(Error::bad("Vui lòng đính kèm hồ sơ định danh"));
    }
    let mut docs = Vec::new();
    for kind in ["citizen", "business_license"] {
        if !v["documents"][kind].is_null() {
            let (mime, data) = document(&v["documents"][kind])?;
            docs.push((kind, mime, crypto::seal(&s.config.key, &data)?));
        }
    }
    let id:i64=sqlx::query_scalar("INSERT INTO trial_requests(package_id,customer_id,name,email,phone,address,company_name) VALUES($1,$2,$3,$4,$5,$6,$7) RETURNING id").bind(package).bind(owner).bind(name).bind(email).bind(resources::text(&v,"phone",50)?).bind(resources::text(&v,"address",255)?).bind(v["company_name"].as_str().unwrap_or("").chars().take(190).collect::<String>()).fetch_one(&mut *tx).await?;
    if let Some(identity) = v["citizen_id"].as_str().filter(|v| !v.is_empty()) {
        if identity.len() > 80 {
            return Err(Error::bad("Số định danh quá dài"));
        }
        sqlx::query("INSERT INTO trial_identity(trial_id,encrypted) VALUES($1,$2)")
            .bind(id)
            .bind(crypto::seal(&s.config.key, identity)?)
            .execute(&mut *tx)
            .await?;
    }
    for (kind, mime, encrypted) in docs {
        sqlx::query(
            "INSERT INTO trial_documents(trial_id,kind,mime,encrypted) VALUES($1,$2,$3,$4)",
        )
        .bind(id)
        .bind(kind)
        .bind(mime)
        .bind(encrypted)
        .execute(&mut *tx)
        .await?;
    }
    let package_name: String = sqlx::query_scalar("SELECT name FROM packages WHERE id=$1")
        .bind(package)
        .fetch_one(&mut *tx)
        .await?;
    if let Some(owner) = owner {
        crate::notifications::queue(
            s,
            &mut tx,
            owner,
            "trial_request_created",
            json!({"package_name":package_name}),
            Some(&format!("trial-request:{id}")),
        )
        .await?;
    }
    tx.commit().await?;
    auth::audit(
        &s,
        owner,
        "trial_request_create",
        json!({"id":id,"package_id":package}),
    )
    .await?;
    Ok(Json(json!({"id":id})))
}
pub async fn download(
    State(s): State<App>,
    h: HeaderMap,
    Path((id, kind)): Path<(i64, String)>,
) -> Result<Response> {
    let u = auth::require(&s, &h, "customers", true, false).await?;
    let r = sqlx::query("SELECT mime,encrypted FROM trial_documents WHERE trial_id=$1 AND kind=$2")
        .bind(id)
        .bind(&kind)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(Error::missing)?;
    let encoded = crypto::open(&s.config.key, &r.get::<String, _>("encrypted"))?;
    let bytes = STANDARD.decode(encoded).map_err(|e| anyhow::anyhow!(e))?;
    let mime = r.get::<String, _>("mime");
    let ext = match mime.as_str() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        _ => "pdf",
    };
    auth::audit(
        &s,
        Some(u.id),
        "trial_document_download",
        json!({"trial_id":id,"kind":kind}),
    )
    .await?;
    Ok((
        [
            (header::CONTENT_TYPE, mime),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"trial-{id}-{kind}.{ext}\""),
            ),
        ],
        bytes,
    )
        .into_response())
}
pub async fn retain(s: &App) -> anyhow::Result<()> {
    // Only completed requests expire; pending review evidence remains available.
    let days: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key='trial_retention_days'")
            .fetch_optional(&s.db)
            .await?;
    let days = days
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(90)
        .clamp(1, 3650);
    let mut tx = s.db.begin().await?;
    for table in ["trial_documents", "trial_identity"] {
        sqlx::query(&format!("DELETE FROM {table} d USING trial_requests t WHERE d.trial_id=t.id AND t.status<>'pending' AND COALESCE(t.updated_at,t.created_at)<now()-make_interval(days=>$1)")).bind(days).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}
