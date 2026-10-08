use crate::{
    auth,
    error::{Error, Result},
    App,
};
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use lettre::{AsyncSmtpTransport, Tokio1Executor};
use serde_json::{json, Value};
use sqlx::Row;
use std::collections::BTreeMap;

fn unquote(text: &str) -> String {
    let pattern = regex::Regex::new(r#""((?:[^"\\]|\\.)*)""#).unwrap();
    let parts: Vec<_> = pattern
        .captures_iter(text)
        .map(|c| c[1].replace("\\\"", "\"").replace("\\\\", "\\"))
        .collect();
    if parts.is_empty() {
        text.trim().to_string()
    } else {
        parts.join("")
    }
}
pub fn zone(domain: &str, source: &str) -> BTreeMap<(String, String), Vec<String>> {
    let record = regex::Regex::new(r"(?i)^(\S+)\s+(?:\d+\s+)?(?:IN\s+)?(MX|TXT)\s+(.+)$").unwrap();
    let mut out = BTreeMap::<_, Vec<_>>::new();
    let mut logical = String::new();
    let mut parens = 0;
    for line in source.lines() {
        let mut quoted = false;
        let mut escaped = false;
        let mut clean = String::new();
        for c in line.chars() {
            if escaped {
                clean.push(c);
                escaped = false;
                continue;
            }
            if c == '\\' {
                clean.push(c);
                escaped = true;
                continue;
            }
            if c == '"' {
                quoted = !quoted;
            }
            if !quoted {
                if c == ';' {
                    break;
                }
                if c == '(' {
                    parens += 1;
                    continue;
                }
                if c == ')' {
                    parens -= 1;
                    continue;
                }
            }
            clean.push(c);
        }
        if !logical.is_empty() {
            logical.push(' ');
        }
        logical.push_str(clean.trim());
        if parens > 0 {
            continue;
        }
        if let Some(c) = record.captures(logical.trim()) {
            let raw = c[1].trim_end_matches('.').to_ascii_lowercase();
            let name = if raw == "@" {
                domain.to_string()
            } else if raw == domain || raw.ends_with(&format!(".{domain}")) {
                raw
            } else {
                format!("{raw}.{domain}")
            };
            let kind = c[2].to_ascii_uppercase();
            let value = if kind == "TXT" {
                unquote(&c[3])
            } else {
                c[3].trim().trim_end_matches('.').to_string()
            };
            out.entry((kind, name)).or_default().push(value);
        }
        logical.clear();
        parens = 0;
    }
    out
}
fn normalized(kind: &str, value: &str) -> String {
    let value = value.trim();
    if kind == "MX" {
        return value
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .trim_end_matches('.')
            .to_ascii_lowercase();
    }
    if value.to_ascii_lowercase().contains("v=dkim1")
        || value.to_ascii_lowercase().contains("v=dmarc1")
    {
        let mut tags = BTreeMap::new();
        for part in value.split(';') {
            if let Some((k, v)) = part.trim().split_once('=') {
                tags.insert(
                    k.trim().to_ascii_lowercase(),
                    if k.trim() == "p" && value.to_ascii_lowercase().contains("v=dkim1") {
                        v.split_whitespace().collect::<String>()
                    } else {
                        v.trim().to_string()
                    },
                );
            }
        }
        serde_json::to_string(&tags).unwrap()
    } else {
        value.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}
pub fn compare(
    kind: &str,
    name: &str,
    expected: &[String],
    response: &Value,
    prefix: &str,
) -> Value {
    let actual: Vec<String> = response["values"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|v| prefix.is_empty() || v.to_ascii_lowercase().starts_with(prefix))
        .map(str::to_string)
        .collect();
    let status = if response["known"] != true {
        "unknown"
    } else if actual.is_empty() || (!prefix.is_empty() && actual.len() != 1) {
        "failed"
    } else if expected.is_empty() {
        "unknown"
    } else if expected.iter().all(|e| {
        actual
            .iter()
            .any(|a| normalized(kind, a) == normalized(kind, e))
    }) {
        "passed"
    } else {
        "failed"
    };
    json!({"type":kind,"name":name,"expected":expected,"actual":actual,"status":status})
}
pub(crate) async fn lookup(s: &App, name: &str, kind: &str) -> Value {
    let code = match kind {
        "MX" => 15,
        "TXT" => 16,
        "A" => 1,
        "AAAA" => 28,
        "PTR" => 12,
        _ => 0,
    };
    let result = async {
        let response = s
            .http
            .get("https://cloudflare-dns.com/dns-query")
            .header("accept", "application/dns-json")
            .query(&[("name", name), ("type", kind)])
            .timeout(std::time::Duration::from_secs(3))
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        crate::sepay::bounded_json(response, 262144).await.ok()
    }
    .await;
    let Some(data) = result else {
        return json!({"known":false,"values":[]});
    };
    if ![json!(0), json!(3)].contains(&data["Status"]) {
        return json!({"known":false,"values":[]});
    }
    let values: Vec<String> = data["Answer"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|v| v["type"] == code)
        .filter_map(|v| v["data"].as_str())
        .map(|v| {
            if kind == "TXT" {
                unquote(v)
            } else {
                v.to_string()
            }
        })
        .collect();
    json!({"known":true,"values":values})
}
pub fn reverse(ip: std::net::IpAddr) -> String {
    match ip {
        std::net::IpAddr::V4(ip) => {
            let bytes = ip.octets();
            format!(
                "{}.{}.{}.{}.in-addr.arpa",
                bytes[3], bytes[2], bytes[1], bytes[0]
            )
        }
        std::net::IpAddr::V6(ip) => {
            let s = hex::encode(ip.octets());
            format!(
                "{}.ip6.arpa",
                s.chars()
                    .rev()
                    .map(|c| c.to_string())
                    .collect::<Vec<_>>()
                    .join(".")
            )
        }
    }
}
pub async fn inspect(
    State(s): State<App>,
    h: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "domains", false, true).await?;
    if u.role == "sub_admin" {
        return Err(Error::forbidden());
    }
    let r=sqlx::query("SELECT d.domain_name,d.dns_records,d.server_id,p.base_url,p.config_version FROM domains d JOIN stalwart_servers p ON p.id=d.server_id WHERE d.id=$1 AND d.status<>'deleted' AND (d.customer_id=$2 OR $3)").bind(id).bind(u.owner).bind(u.role=="admin").fetch_optional(&s.db).await?.ok_or_else(Error::missing)?;
    let domain: String = r.get("domain_name");
    let server: i64 = r.get("server_id");
    let version: i64 = r.get("config_version");
    let url = url::Url::parse(&r.get::<String, _>("base_url"))
        .map_err(|_| Error::bad("URL server không hợp lệ"))?;
    let host = url.host_str().ok_or_else(Error::missing)?;
    let records = zone(
        &domain,
        &r.get::<Option<String>, _>("dns_records")
            .unwrap_or_default(),
    );
    let mx = records
        .get(&("MX".into(), domain.clone()))
        .cloned()
        .unwrap_or_else(|| vec![format!("10 {host}")]);
    let txt = records
        .get(&("TXT".into(), domain.clone()))
        .cloned()
        .unwrap_or_default();
    let spf: Vec<_> = txt
        .into_iter()
        .filter(|v| v.to_ascii_lowercase().starts_with("v=spf1"))
        .collect();
    let dmarc = format!("_dmarc.{domain}");
    let mut checks = vec![
        compare("MX", &domain, &mx, &lookup(&s, &domain, "MX").await, ""),
        compare(
            "TXT",
            &domain,
            &spf,
            &lookup(&s, &domain, "TXT").await,
            "v=spf1",
        ),
        compare(
            "TXT",
            &dmarc,
            records
                .get(&("TXT".into(), dmarc.clone()))
                .map(Vec::as_slice)
                .unwrap_or(&[]),
            &lookup(&s, &dmarc, "TXT").await,
            "v=dmarc1",
        ),
    ];
    let selectors: Vec<_> = records
        .iter()
        .filter(|((kind, name), _)| {
            kind == "TXT" && name.ends_with(&format!("._domainkey.{domain}"))
        })
        .collect();
    if selectors.is_empty() {
        checks.push(json!({"type":"DKIM","name":domain,"status":"unknown","message":"Chưa có selector từ zone server"}));
    }
    for ((_, name), values) in selectors.iter().take(8) {
        checks.push(compare(
            "TXT",
            name,
            values,
            &lookup(&s, name, "TXT").await,
            "",
        ));
    }
    if selectors.len() > 8 {
        checks.push(json!({"type":"DKIM","status":"unknown","message":"Có hơn 8 selector; cần kiểm tra bổ sung"}));
    }
    let mut ips = Vec::new();
    for kind in ["A", "AAAA"] {
        let values = lookup(&s, host, kind).await;
        for v in values["values"].as_array().into_iter().flatten() {
            if let Some(ip) = v.as_str().and_then(|v| v.parse::<std::net::IpAddr>().ok()) {
                if !ips.contains(&ip) {
                    ips.push(ip);
                }
            }
        }
    }
    for ip in ips.iter().take(4) {
        let response = lookup(&s, &reverse(*ip), "PTR").await;
        let actual: Vec<_> = response["values"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|s| s.trim_end_matches('.').to_ascii_lowercase())
            .collect();
        checks.push(json!({"type":"PTR","name":ip.to_string(),"expected":[host],"actual":actual,"status":if response["known"]!=true{"unknown"}else if actual.contains(&host.to_ascii_lowercase()){"passed"}else{"failed"}}));
    }
    if ips.is_empty() || ips.len() > 4 {
        checks.push(json!({"type":"PTR","name":host,"status":"unknown","message":"Không đủ dữ liệu IP để xác minh PTR"}));
    }
    let tls = match AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host) {
        Ok(builder) => match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            builder
                .port(25)
                .timeout(Some(std::time::Duration::from_secs(5)))
                .build::<Tokio1Executor>()
                .test_connection(),
        )
        .await
        {
            Ok(Ok(true)) => "passed",
            _ => "unknown",
        },
        Err(_) => "unknown",
    };
    checks.push(json!({"type":"STARTTLS","name":host,"port":25,"status":tls,"message":if tls=="passed"{"STARTTLS và chứng chỉ đã xác minh"}else{"Chưa xác minh được STARTTLS; kiểm tra kết nối và chứng chỉ server"}}));
    let status = if checks.iter().any(|c| c["status"] == "failed") {
        "failed"
    } else if checks.iter().any(|c| c["status"] == "unknown") {
        "unknown"
    } else {
        "passed"
    };
    let report = json!({"status":status,"checks":checks,"mail_host":host,"resolver":"Cloudflare DNS-over-HTTPS"});
    sqlx::query("INSERT INTO operational_dns_checks(server_id,server_version,domain_id,local_hash,status,report,checked_at,next_check_at) VALUES($1,$2,$3,$4,$5,$6,now(),now()+interval '1 hour') ON CONFLICT(server_id,domain_id) DO UPDATE SET server_version=EXCLUDED.server_version,local_hash=EXCLUDED.local_hash,status=EXCLUDED.status,report=EXCLUDED.report,checked_at=now(),next_check_at=EXCLUDED.next_check_at").bind(server).bind(version).bind(id).bind(crate::crypto::hash(&records.iter().map(|r|format!("{r:?}")).collect::<String>())).bind(status).bind(&report).execute(&s.db).await?;
    auth::audit(
        &s,
        Some(u.impersonator.unwrap_or(u.id)),
        "mail_dns_check",
        json!({"domain_id":id,"status":status}),
    )
    .await?;
    Ok(Json(report))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn multiline_zone_and_policy_comparison() {
        let z=zone("example.com","@ 3600 IN MX 10 mail.example.com.\n@ IN TXT \"v=spf1 mx -all\"\nkey._domainkey IN TXT (\n\"v=DKIM1; p=AAA\"\n\"BBB\"\n)\n_dmarc IN TXT \"v=DMARC1; p=reject\"");
        assert_eq!(
            z[&("TXT".into(), "key._domainkey.example.com".into())],
            vec!["v=DKIM1; p=AAABBB"]
        );
        let r = json!({"known":true,"values":["v=DMARC1;p=reject;"]});
        assert_eq!(
            compare(
                "TXT",
                "_dmarc.example.com",
                &["p=reject;v=DMARC1".into()],
                &r,
                "v=dmarc1"
            )["status"],
            "passed"
        );
        assert_eq!(
            compare(
                "TXT",
                "example.com",
                &["v=spf1 mx -all".into()],
                &json!({"known":false,"values":[]}),
                "v=spf1"
            )["status"],
            "unknown"
        );
        assert_eq!(
            compare(
                "TXT",
                "example.com",
                &[],
                &json!({"known":true,"values":["v=spf1 mx -all","v=spf1 -all"]}),
                "v=spf1"
            )["status"],
            "failed"
        );
    }
    #[test]
    fn reverse_dns_names() {
        assert_eq!(
            reverse("192.0.2.1".parse().unwrap()),
            "1.2.0.192.in-addr.arpa"
        );
        assert!(reverse("2001:db8::1".parse().unwrap()).starts_with("1.0.0.0."));
    }
}
