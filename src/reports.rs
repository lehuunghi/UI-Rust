use crate::{
    auth, crypto,
    error::{Error, Result},
    resources, App,
};
use axum::{
    extract::{Query, State},
    http::{header, HeaderMap},
    response::{IntoResponse, Response},
    Json,
};
use base64::Engine;
use chrono::{DateTime, Datelike, NaiveDate, TimeZone, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgConnection;
use std::io::Read;
const MAX: usize = 8 * 1024 * 1024;
fn bad() -> Error {
    Error::bad("Báo cáo không hợp lệ hoặc vượt giới hạn")
}
fn count(v: &Value) -> Result<i64> {
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .filter(|n| *n >= 0 && *n <= 1_000_000_000_000)
        .ok_or_else(bad)
}
fn time(v: &Value) -> Result<DateTime<Utc>> {
    let value = if let Some(n) = v
        .as_i64()
        .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
    {
        DateTime::from_timestamp(n, 0)
    } else {
        v.as_str()
            .and_then(|s| {
                DateTime::parse_from_rfc3339(s)
                    .or_else(|_| DateTime::parse_from_rfc2822(s))
                    .ok()
            })
            .map(|d| d.with_timezone(&Utc))
    };
    value
        .filter(|d| d.timestamp() >= 946684800 && *d <= Utc::now() + chrono::Duration::days(1))
        .ok_or_else(bad)
}
fn short(v: &Value, max: usize) -> String {
    v.as_str()
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .take(max)
        .collect()
}
fn summary(
    kind: &str,
    domain: &Value,
    report: &Value,
    start: &Value,
    end: &Value,
    org: &Value,
    passed: i64,
    failed: i64,
    details: Value,
) -> Result<Value> {
    let start = time(start)?;
    let end = time(end)?;
    if end < start || end - start > chrono::Duration::days(366) {
        return Err(bad());
    }
    let domain = short(domain, 254).trim().to_ascii_lowercase();
    if !resources::domain_name(&domain) {
        return Err(bad());
    }
    let id = short(report, 190);
    if id.is_empty() {
        return Err(bad());
    }
    let total = passed
        .checked_add(failed)
        .filter(|n| *n <= 5_000_000_000_000_000)
        .ok_or_else(bad)?;
    Ok(
        json!({"kind":kind,"domain":domain,"report_id":id,"period_start":start.to_rfc3339(),"period_end":end.to_rfc3339(),"total":total,"passed":passed,"failed":failed,"organization":short(org,190),"details":details}),
    )
}
pub fn native(kind: &str, r: &Value, received: &Value) -> Result<Vec<Value>> {
    match kind {
        "dmarc_reports" => {
            let rows = r["records"]
                .as_array()
                .filter(|a| a.len() <= 5000)
                .ok_or_else(bad)?;
            let mut passed = 0_i64;
            let mut failed = 0_i64;
            let mut sources = Vec::new();
            for row in rows {
                if !row.is_object() {
                    return Err(bad());
                }
                let n = count(&row["count"])?;
                if row["evaluatedDkim"] == "pass" || row["evaluatedSpf"] == "pass" {
                    passed = passed.checked_add(n).ok_or_else(bad)?;
                } else {
                    failed = failed.checked_add(n).ok_or_else(bad)?;
                }
                if sources.len() < 100 {
                    let ip = row["sourceIp"].as_str().unwrap_or("");
                    sources.push(json!({"ip":ip.parse::<std::net::IpAddr>().map(|v|v.to_string()).unwrap_or_default(),"count":n,"dkim":short(&row["evaluatedDkim"],30),"spf":short(&row["evaluatedSpf"],30),"disposition":short(&row["evaluatedDisposition"],30)}));
                }
            }
            Ok(vec![summary(
                "dmarc",
                &r["policyDomain"],
                &r["reportId"],
                &r["dateRangeBegin"],
                &r["dateRangeEnd"],
                &r["orgName"],
                passed,
                failed,
                json!({"sources":sources,"source_rows":rows.len(),"display_truncated":rows.len()>100}),
            )?])
        }
        "tls_reports" => {
            let policies = r["policies"]
                .as_array()
                .filter(|a| a.len() <= 1000 && !a.is_empty())
                .ok_or_else(bad)?;
            let mut out = Vec::new();
            for p in policies {
                let rows = p
                    .get("failureDetails")
                    .and_then(Value::as_array)
                    .filter(|a| a.len() <= 5000)
                    .ok_or_else(bad)?;
                let mut errors = Vec::new();
                for f in rows {
                    let count = count(&f["failedSessionCount"])?;
                    if errors.len() < 100 {
                        errors.push(json!({"type":short(&f["resultType"],100),"count":count,"receiving_mx":short(&f["receivingMxHostname"],253)}));
                    }
                }
                out.push(summary("tls",&p["policyDomain"],&r["reportId"],&r["dateRangeStart"],&r["dateRangeEnd"],&r["organizationName"],count(&p["totalSuccessfulSessions"] )?,count(&p["totalFailedSessions"] )?,json!({"policy":short(&p["policyType"],50),"errors":errors,"display_truncated":rows.len()>100}))?);
            }
            Ok(out)
        }
        "arf_reports" => {
            let domains: Vec<Value> = if let Some(a) = r["reportedDomains"].as_array() {
                a.clone()
            } else {
                r["reportedDomains"]
                    .as_object()
                    .ok_or_else(bad)?
                    .iter()
                    .filter(|(_, v)| v.as_bool() == Some(true))
                    .map(|(k, _)| json!(k))
                    .collect()
            };
            if domains.is_empty() || domains.len() > 100 {
                return Err(bad());
            }
            let date = r.get("arrivalDate").unwrap_or(received);
            let identity = json!({"arrivalDate":date,"feedbackType":r["feedbackType"],"sourceIp":r["sourceIp"],"incidents":r["incidents"],"reportedDomains":r["reportedDomains"]});
            let id = json!(crypto::hash(&identity.to_string()));
            domains.iter().map(|d|summary("arf",d,&id,date,date,&json!(""),0,count(r.get("incidents").unwrap_or(&json!(1)))?,json!({"feedback_type":short(&r["feedbackType"],50),"source_ip":r["sourceIp"].as_str().and_then(|s|s.parse::<std::net::IpAddr>().ok()).map(|v|v.to_string()).unwrap_or_default()}))).collect()
        }
        "metrics" => {
            let metric = short(&r["metric"], 101);
            if metric.is_empty()
                || metric.len() > 100
                || !metric
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
                || !["Counter", "Gauge", "Histogram"]
                    .iter()
                    .any(|v| r["@type"] == *v)
            {
                return Err(bad());
            }
            let timestamp = time(&r["timestamp"])?.to_rfc3339();
            Ok(vec![
                json!({"kind":"metric","domain":"","report_id":metric,"period_start":timestamp,"period_end":timestamp,"total":count(&r["count"] )?,"passed":0,"failed":0,"metric_type":r["@type"],"sum":count(&r["sum"] )?}),
            ])
        }
        _ => Err(bad()),
    }
}
fn xml_text(node: roxmltree::Node<'_, '_>, path: &[&str]) -> String {
    let mut node = node;
    for part in path {
        let Some(next) = node
            .children()
            .find(|n| n.is_element() && n.tag_name().name() == *part)
        else {
            return String::new();
        };
        node = next;
    }
    node.text().unwrap_or("").trim().into()
}
pub fn parse(kind: &str, bytes: &[u8]) -> Result<Vec<Value>> {
    if bytes.len() > MAX {
        return Err(bad());
    }
    let mut bytes = bytes.to_vec();
    if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut expanded = Vec::new();
        flate2::read::GzDecoder::new(bytes.as_slice())
            .take((MAX + 1) as u64)
            .read_to_end(&mut expanded)
            .map_err(|_| bad())?;
        if expanded.len() > MAX {
            return Err(bad());
        }
        bytes = expanded;
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| bad())?;
    match kind {
        "dmarc" => {
            let upper = text.to_ascii_uppercase();
            if upper.contains("<!DOCTYPE") || upper.contains("<!ENTITY") {
                return Err(bad());
            }
            let doc = roxmltree::Document::parse(text).map_err(|_| bad())?;
            let root = doc.root_element();
            if root.tag_name().name() != "feedback" {
                return Err(bad());
            }
            let records:Vec<_>=root.children().filter(|n|n.is_element()&&n.tag_name().name()=="record").map(|n|json!({"sourceIp":xml_text(n,&["row","source_ip"]),"count":xml_text(n,&["row","count"]),"evaluatedDisposition":xml_text(n,&["row","policy_evaluated","disposition"]),"evaluatedDkim":xml_text(n,&["row","policy_evaluated","dkim"]),"evaluatedSpf":xml_text(n,&["row","policy_evaluated","spf"])})).take(5001).collect();
            native(
                "dmarc_reports",
                &json!({"reportId":xml_text(root,&["report_metadata","report_id"]),"orgName":xml_text(root,&["report_metadata","org_name"]),"dateRangeBegin":xml_text(root,&["report_metadata","date_range","begin"]),"dateRangeEnd":xml_text(root,&["report_metadata","date_range","end"]),"policyDomain":xml_text(root,&["policy_published","domain"]),"records":records}),
                &json!(Utc::now().to_rfc3339()),
            )
        }
        "tls" => {
            let r: Value = serde_json::from_str(text).map_err(|_| bad())?;
            let policies = r["policies"]
                .as_array()
                .filter(|a| a.len() <= 1000)
                .ok_or_else(bad)?;
            let mut translated = Vec::new();
            for p in policies {
                let failures = p.get("failure-details").cloned().unwrap_or(json!([]));
                let failures = failures
                    .as_array()
                    .filter(|a| a.len() <= 5000)
                    .ok_or_else(bad)?;
                translated.push(json!({"policyDomain":p["policy"]["policy-domain"],"policyType":p["policy"]["policy-type"],"totalSuccessfulSessions":p["summary"]["total-successful-session-count"],"totalFailedSessions":p["summary"]["total-failure-session-count"],"failureDetails":failures.iter().map(|f|json!({"resultType":f["result-type"],"failedSessionCount":f["failed-session-count"],"receivingMxHostname":f["receiving-mx-hostname"]})).collect::<Vec<_>>()}));
            }
            native(
                "tls_reports",
                &json!({"reportId":r["report-id"],"dateRangeStart":r["date-range"]["start-datetime"],"dateRangeEnd":r["date-range"]["end-datetime"],"organizationName":r["organization-name"],"policies":translated}),
                &json!(Utc::now().to_rfc3339()),
            )
        }
        "arf" => {
            let r: Value = serde_json::from_str(text).map_err(|_| bad())?;
            native("arf_reports", &r, &json!(Utc::now().to_rfc3339()))
        }
        "dsn" => parse_dsn(text),
        _ => Err(bad()),
    }
}
fn parse_dsn(text: &str) -> Result<Vec<Value>> {
    if text.len() > 1024 * 1024 {
        return Err(bad());
    }
    let text = text.replace("\r\n", "\n");
    let folded = regex::Regex::new(r"\n[ \t]+")
        .unwrap()
        .replace_all(&text, " ");
    let mut groups: std::collections::BTreeMap<String, Vec<Value>> = Default::default();
    let mut date = Utc::now();
    let mut total = 0;
    for block in regex::Regex::new(r"\n\s*\n").unwrap().split(&folded) {
        let fields: std::collections::HashMap<String, String> = block
            .lines()
            .filter_map(|line| {
                line.split_once(':')
                    .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
            })
            .collect();
        if let Some(v) = fields.get("arrival-date") {
            date = time(&json!(v))?;
        }
        let Some(recipient) = fields.get("final-recipient") else {
            continue;
        };
        let recipient = recipient
            .split_once(';')
            .map(|(_, v)| v.trim())
            .unwrap_or("");
        let action = fields.get("action").map(String::as_str).unwrap_or("");
        let status = fields.get("status").map(String::as_str).unwrap_or("");
        if !auth::valid_email(recipient)
            || !["failed", "delayed", "delivered", "relayed", "expanded"].contains(&action)
            || !regex::Regex::new(r"^[245]\.\d{1,3}\.\d{1,3}$")
                .unwrap()
                .is_match(status)
        {
            return Err(bad());
        }
        let domain = recipient.rsplit_once('@').unwrap().1.to_ascii_lowercase();
        groups.entry(domain).or_default().push(json!({"recipient":recipient,"action":action,"status":status,"diagnostic":short(&json!(fields.get("diagnostic-code").cloned().unwrap_or_default()),300)}));
        total += 1;
        if total > 1000 {
            return Err(bad());
        }
    }
    if groups.is_empty() {
        return Err(bad());
    }
    groups
        .into_iter()
        .map(|(domain, rows)| {
            let passed = rows
                .iter()
                .filter(|r| {
                    ["delivered", "relayed", "expanded"]
                        .iter()
                        .any(|s| r["action"] == *s)
                })
                .count() as i64;
            summary(
                "dsn",
                &json!(domain),
                &json!(crypto::hash(&text)),
                &json!(date.to_rfc3339()),
                &json!(date.to_rfc3339()),
                &json!(""),
                passed,
                rows.len() as i64 - passed,
                json!({"recipients":rows}),
            )
        })
        .collect()
}
pub async fn store(
    db: &mut PgConnection,
    server: i64,
    summary: &Value,
    source: &str,
) -> Result<u64> {
    let identity = if summary["kind"] == "metric" {
        json!([
            summary["report_id"],
            summary["period_start"],
            summary["metric_type"]
        ])
    } else {
        json!([
            summary["kind"],
            summary["domain"],
            summary["report_id"],
            summary["period_start"],
            summary["period_end"],
            summary["organization"]
        ])
    };
    Ok(sqlx::query("INSERT INTO operational_report_history(server_id,kind,fingerprint,domain_name,report_id,period_start,period_end,total,passed,failed,summary,source) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12) ON CONFLICT(server_id,kind,fingerprint) DO NOTHING").bind(server).bind(summary["kind"].as_str().unwrap()).bind(crypto::hash(&identity.to_string())).bind(summary["domain"].as_str().unwrap()).bind(summary["report_id"].as_str().unwrap()).bind(time(&summary["period_start"] )?).bind(time(&summary["period_end"] )?).bind(summary["total"].as_i64().unwrap()).bind(summary["passed"].as_i64().unwrap()).bind(summary["failed"].as_i64().unwrap()).bind(summary).bind(source).execute(db).await?.rows_affected())
}
pub async fn import(
    State(s): State<App>,
    h: HeaderMap,
    Json(v): Json<Value>,
) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "operations", true, true).await?;
    u.super_admin()?;
    let server = resources::number(&v, "server_id")?;
    let encoded = resources::text(&v, "file", 12 * 1024 * 1024)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| bad())?;
    let summaries = parse(resources::text(&v, "kind", 10)?, &bytes)?;
    let mut tx = s.db.begin().await?;
    sqlx::query("SELECT id FROM stalwart_servers WHERE id=$1 FOR SHARE")
        .bind(server)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(Error::missing)?;
    let mut inserted = 0;
    for summary in &summaries {
        inserted += store(&mut tx, server, summary, "upload").await?;
    }
    sqlx::query(
        "INSERT INTO audit_logs(user_id,action,context) VALUES($1,'mail_reports_imported',$2)",
    )
    .bind(u.id)
    .bind(json!({"server_id":server,"kind":v["kind"],"inserted":inserted}))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(
        json!({"inserted":inserted,"duplicates":summaries.len()as u64-inserted}),
    ))
}
#[derive(Deserialize)]
pub struct History {
    server_id: i64,
    month: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    domain: String,
    #[serde(default)]
    format: String,
}
async fn history(s: &App, h: &HeaderMap, q: &History) -> Result<Vec<Value>> {
    let u = auth::require(s, h, "operations", false, false).await?;
    if u.role == "sub_admin" {
        return Err(Error::forbidden());
    }
    if u.role == "admin" {
        u.super_admin()?;
    } else {
        let owns:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM domains WHERE customer_id=$1 AND server_id=$2 AND domain_name=$3 AND status<>'deleted')").bind(u.id).bind(q.server_id).bind(&q.domain).fetch_one(&s.db).await?;
        if !owns || q.kind == "metric" {
            return Err(Error::forbidden());
        }
    }
    if !regex::Regex::new(r"^20\d{2}-(0[1-9]|1[0-2])$")
        .unwrap()
        .is_match(&q.month)
        || !["", "reports", "dmarc", "tls", "arf", "dsn", "metric"].contains(&q.kind.as_str())
    {
        return Err(bad());
    }
    let date =
        NaiveDate::parse_from_str(&format!("{}-01", q.month), "%Y-%m-%d").map_err(|_| bad())?;
    let end = NaiveDate::from_ymd_opt(
        date.year() + if date.month() == 12 { 1 } else { 0 },
        if date.month() == 12 {
            1
        } else {
            date.month() + 1
        },
        1,
    )
    .ok_or_else(bad)?;
    let rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(r) FROM operational_report_history r WHERE server_id=$1 AND period_start>=$2 AND period_start<$3 AND ($4='' OR kind=$4 OR ($4='reports' AND kind IN ('dmarc','tls','arf','dsn'))) AND ($5='' OR domain_name=$5) ORDER BY period_start DESC,id DESC LIMIT 10001").bind(q.server_id).bind(Utc.from_utc_datetime(&date.and_hms_opt(0,0,0).unwrap())).bind(Utc.from_utc_datetime(&end.and_hms_opt(0,0,0).unwrap())).bind(&q.kind).bind(&q.domain).fetch_all(&s.db).await?;
    if rows.len() > 10000 {
        return Err(Error::bad("Quá 10.000 báo cáo; lọc theo loại hoặc domain"));
    }
    Ok(rows)
}
pub async fn list(
    State(s): State<App>,
    h: HeaderMap,
    Query(q): Query<History>,
) -> Result<Response> {
    let rows = history(&s, &h, &q).await?;
    match q.format.as_str() {
        "" | "json" => Ok(Json(json!({"items":rows})).into_response()),
        "ndjson" => {
            let text = rows
                .iter()
                .map(|r| r.to_string() + "\n")
                .collect::<String>();
            Ok((
                [
                    (header::CONTENT_TYPE, "application/x-ndjson"),
                    (
                        header::CONTENT_DISPOSITION,
                        "attachment; filename=mail-reports.ndjson",
                    ),
                ],
                text,
            )
                .into_response())
        }
        "csv" => {
            let mut writer = csv::Writer::from_writer(Vec::new());
            let columns = [
                "kind",
                "domain_name",
                "report_id",
                "period_start",
                "period_end",
                "total",
                "passed",
                "failed",
                "source",
            ];
            writer.write_record(columns).map_err(|_| bad())?;
            for r in rows {
                writer
                    .write_record(columns.iter().map(|k| crate::bulk::cell(&r[*k])))
                    .map_err(|_| bad())?;
            }
            let bytes = writer.into_inner().map_err(|_| bad())?;
            Ok((
                [
                    (header::CONTENT_TYPE, "text/csv; charset=utf-8"),
                    (
                        header::CONTENT_DISPOSITION,
                        "attachment; filename=mail-reports.csv",
                    ),
                ],
                bytes,
            )
                .into_response())
        }
        _ => Err(bad()),
    }
}
