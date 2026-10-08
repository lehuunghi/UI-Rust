use crate::{
    auth,
    error::{Error, Result},
    resources, App,
};
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

pub async fn import(
    State(s): State<App>,
    h: HeaderMap,
    Json(v): Json<Value>,
) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "accounts", false, true).await?;
    let owner = if u.role == "admin" {
        resources::number(&v, "customer_id")?
    } else {
        u.owner
    };
    let text = resources::text(&v, "text", 60_000)?.trim_start_matches('\u{feff}');
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(text.as_bytes());
    let rows: Vec<_> = reader.records().collect();
    if rows.is_empty() || rows.len() > 200 {
        return Err(Error::bad("Mỗi lần import từ 1 đến 200 dòng"));
    }
    let mut created = 0;
    let mut errors = Vec::new();
    for (index, row) in rows.into_iter().enumerate() {
        let result=async {
            let row=row.map_err(|_|Error::bad("CSV không hợp lệ"))?;
            if row.len()!=5 {return Err(Error::bad("Cần 5 cột: tên, họ, email, mật khẩu, ID nhóm"));}
            let email=row[2].to_lowercase();
            if !auth::valid_email(&email){return Err(Error::bad("Email không hợp lệ"));}
            let (local,domain)=email.split_once('@').ok_or_else(||Error::bad("Email không hợp lệ"))?;
            let domain_id: i64=sqlx::query_scalar("SELECT id FROM domains WHERE customer_id=$1 AND domain_name=$2 AND status='active' AND verification_status='verified' AND sync_status='synced'").bind(owner).bind(domain).fetch_optional(&s.db).await?.ok_or_else(||Error::bad("Domain chưa được xác minh hoặc đồng bộ"))?;
            let group=row[4].parse::<i64>().map_err(|_|Error::bad("ID nhóm không hợp lệ"))?;
            let _ = resources::save(State(s.clone()),h.clone(),Path("accounts".into()),Json(json!({"customer_id":owner,"domain_id":domain_id,"group_id":group,"local_part":local,"display_name":format!("{} {}",&row[0],&row[1]).trim(),"password":&row[3]}))).await?;
            Ok::<_,Error>(())
        }.await;
        match result {
            Ok(()) => created += 1,
            Err(e) => errors.push(json!({"row":index+1,"error":e.1})),
        }
    }
    auth::audit(
        &s,
        Some(u.impersonator.unwrap_or(u.id)),
        "account_import",
        json!({"customer_id":owner,"created":created,"failed":errors.len()}),
    )
    .await?;
    Ok(Json(
        json!({"created":created,"failed":errors.len(),"errors":errors}),
    ))
}

// Protect spreadsheets from formulas while preserving quoting, Unicode, commas and newlines.
pub(crate) fn cell(v: &Value) -> String {
    let value = match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        _ => v.to_string(),
    };
    if value.trim_start().starts_with(['=', '+', '-', '@']) || value.starts_with(['\t', '\r']) {
        format!("'{value}")
    } else {
        value
    }
}
pub async fn export(
    State(s): State<App>,
    h: HeaderMap,
    Path(kind): Path<String>,
    Query(page): Query<resources::Page>,
) -> Result<Response> {
    let u = auth::user(&s, &h, true).await?;
    let columns: &[&str] = match kind.as_str() {
        "accounts" => &[
            "id",
            "customer_id",
            "email",
            "display_name",
            "domain_id",
            "group_id",
            "status",
            "sync_status",
            "storage_limit_mb",
        ],
        "domains" => &[
            "id",
            "customer_id",
            "domain_name",
            "status",
            "verification_status",
            "sync_status",
            "server_id",
        ],
        "customers" => &["id", "name", "email", "status"],
        "subscriptions" => &[
            "id",
            "customer_id",
            "invoice_code",
            "price",
            "payment_status",
            "status",
            "start_date",
            "end_date",
        ],
        "payments" => &[
            "id",
            "invoice_code",
            "amount",
            "reference_code",
            "state",
            "mode",
            "source",
            "created_at",
        ],
        "logs" => &["id", "user_id", "action", "created_at", "context"],
        "jobs" => &[
            "id",
            "server_id",
            "job_type",
            "resource_id",
            "status",
            "attempts",
            "last_error",
        ],
        _ => return Err(Error::bad("Loại dữ liệu chưa hỗ trợ CSV")),
    };
    let mut writer = csv::Writer::from_writer(Vec::new());
    writer
        .write_record(columns)
        .map_err(|e| anyhow::anyhow!(e))?;
    let mut count = 0;
    loop {
        let Json(result) = resources::list(
            State(s.clone()),
            h.clone(),
            Path(kind.clone()),
            Query(resources::Page {
                offset: count,
                q: page.q.clone(),
            }),
        )
        .await?;
        let items = result["items"].as_array().ok_or_else(Error::missing)?;
        for item in items {
            writer
                .write_record(columns.iter().map(|key| cell(&item[*key])))
                .map_err(|e| anyhow::anyhow!(e))?;
        }
        count += items.len() as i64;
        if result["next"].is_null() {
            break;
        }
        if count >= 10_000 {
            return Err(Error::bad(
                "Trên 10.000 dòng; thu hẹp bộ lọc trước khi xuất",
            ));
        }
    }
    auth::audit(
        &s,
        Some(u.impersonator.unwrap_or(u.id)),
        "csv_export",
        json!({"kind":kind,"rows":count}),
    )
    .await?;
    let bytes = writer.into_inner().map_err(|e| anyhow::anyhow!(e))?;
    Ok((
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                &format!("attachment; filename=\"{kind}.csv\""),
            ),
        ],
        bytes,
    )
        .into_response())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn spreadsheet_formulas_are_inert() {
        for s in ["=SUM(A1)", " +1", "\tcmd", "@SUM(1)", "-2"] {
            assert!(cell(&json!(s)).starts_with('\''));
        }
        assert_eq!(cell(&json!("Nguyễn, An")), "Nguyễn, An");
    }
}
