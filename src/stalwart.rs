use crate::{crypto, App};
use anyhow::{anyhow, bail, Result};
use base64::Engine;
use serde_json::{json, Value};
use sqlx::Row;
pub fn validate(calls: &Value, response: &Value) -> Result<()> {
    let calls = calls.as_array().ok_or_else(|| anyhow!("Invalid calls"))?;
    let responses = response["methodResponses"]
        .as_array()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| anyhow!("Missing method responses"))?;
    let mut ids = std::collections::HashSet::new();
    for call in calls {
        let id = call[2].as_str().ok_or_else(|| anyhow!("Invalid call ID"))?;
        if !ids.insert(id) {
            bail!("Duplicate call ID")
        }
        let matches: Vec<_> = responses
            .iter()
            .filter(|r| r[2] == call[2] && (r[0] == call[0] || r[0] == "error"))
            .collect();
        if matches.len() != 1 {
            bail!("Missing or duplicate method response")
        }
        let r = matches[0];
        if r.as_array().map(Vec::len) != Some(3) || !r[1].is_object() || r[0] == "error" {
            bail!("JMAP request rejected")
        }
        let args = &call[1];
        let body = &r[1];
        for key in ["notCreated", "notUpdated", "notDestroyed"] {
            if body
                .get(key)
                .is_some_and(|v| v.as_object().is_some_and(|o| !o.is_empty()))
            {
                bail!("JMAP object change rejected")
            }
        }
        if call[0].as_str().unwrap_or("").ends_with("/set") {
            for (action, field) in [("create", "created"), ("update", "updated")] {
                if let Some(items) = args[action].as_object() {
                    for (k, _) in items {
                        if body[field].get(k).is_none() {
                            bail!("Incomplete JMAP change result")
                        }
                        if action == "create"
                            && body[field][k]["id"].as_str().is_none_or(str::is_empty)
                        {
                            bail!("Missing created id")
                        }
                    }
                }
            }
            if let Some(items) = args["destroy"].as_array() {
                for id in items {
                    if !body["destroyed"].as_array().is_some_and(|v| v.contains(id)) {
                        bail!("Incomplete destroy result")
                    }
                }
            }
        }
    }
    for r in responses {
        if !r[2].as_str().is_some_and(|id| ids.contains(id)) {
            bail!("Unknown call id")
        }
    }
    Ok(())
}
pub fn allowed(method: &str, write: bool) -> bool {
    let Some((kind, action)) = method.strip_prefix("x:").and_then(|s| s.split_once('/')) else {
        return false;
    };
    let mutable = [
        "Domain",
        "Account",
        "QueuedMessage",
        "Task",
        "Action",
        "ApiKey",
        "AppPassword",
        "Role",
        "Tenant",
        "DkimSignature",
        "MtaOutboundThrottle",
        "MtaInboundThrottle",
        "BlockedIp",
        "AllowedIp",
    ];
    let readonly = [
        "Certificate",
        "Log",
        "Trace",
        "DmarcExternalReport",
        "TlsExternalReport",
        "ArfExternalReport",
        "Metric",
        "DnsServer",
    ];
    (mutable.contains(&kind) || readonly.contains(&kind))
        && (["query", "get"].contains(&action)
            || (write && action == "set" && mutable.contains(&kind)))
}
pub async fn call(s: &App, server: i64, calls: Value) -> Result<Value> {
    let r = sqlx::query("SELECT * FROM stalwart_servers WHERE id=$1 AND active=1")
        .bind(server)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(|| anyhow!("Selected server unavailable"))?;
    let list = calls
        .as_array()
        .filter(|v| !v.is_empty() && v.len() <= 20)
        .ok_or_else(|| anyhow!("Invalid calls"))?;
    for call in list {
        if call.as_array().map(Vec::len) != Some(3)
            || !call[1].is_object()
            || !call[2].is_string()
            || !allowed(call[0].as_str().unwrap_or(""), true)
        {
            bail!("Unsupported JMAP method")
        }
    }
    if r.get::<i16, _>("dry_run") == 1 {
        let mut responses = Vec::new();
        for c in list {
            let method = c[0].as_str().unwrap();
            let args = &c[1];
            let mut body = json!({});
            if method.ends_with("/set") {
                body = json!({"created":{},"updated":{},"destroyed":args.get("destroy").cloned().unwrap_or(json!([]))});
                if let Some(create) = args["create"].as_object() {
                    for (k, v) in create {
                        body["created"][k] =
                            json!({"id":format!("dry-{}",&crypto::hash(&v.to_string())[..16])});
                    }
                }
                if let Some(update) = args["update"].as_object() {
                    for (k, _) in update {
                        body["updated"][k] = Value::Null;
                    }
                }
            } else if method.ends_with("/query") {
                body = json!({"ids":[],"total":0})
            } else if method.ends_with("/get") {
                body = json!({"list":[],"notFound":[]})
            }
            responses.push(json!([method, body, c[2]]));
        }
        let v = json!({"methodResponses":responses,"dry_run":true});
        validate(&calls, &v)?;
        return Ok(v);
    }
    let base = r
        .get::<String, _>("base_url")
        .trim_end_matches('/')
        .to_string();
    let endpoint = if base.ends_with("/api") {
        base
    } else if base.ends_with("/jmap") {
        format!("{base}/")
    } else {
        format!("{base}/jmap/")
    };
    let token = r
        .get::<Option<String>, _>("token_encrypted")
        .ok_or_else(|| anyhow!("Missing server credentials"))?;
    let token = crypto::open(&s.config.key, &token)?;
    let authorization = if let Some(v) = token.strip_prefix("basic:") {
        format!("Basic {}", v.trim())
    } else if token.contains(':') && !token.starts_with("API_") {
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(token.as_bytes())
        )
    } else {
        format!("Bearer {token}")
    };
    let mut response = s
        .http
        .post(endpoint)
        .header("Authorization", authorization)
        .json(
            &json!({"using":["urn:ietf:params:jmap:core","urn:stalwart:jmap"],"methodCalls":calls}),
        )
        .send()
        .await?
        .error_for_status()?;
    let mut bytes = Vec::new();
    while let Some(c) = response.chunk().await? {
        if bytes.len() + c.len() > 4 * 1024 * 1024 {
            bail!("Response exceeds 4 MB")
        }
        bytes.extend_from_slice(&c)
    }
    let response: Value = serde_json::from_slice(&bytes)?;
    validate(&calls, &response)?;
    Ok(response)
}
pub fn safe_text(value: &str) -> String {
    static PATTERNS: std::sync::LazyLock<Vec<(regex::Regex, &str)>> = std::sync::LazyLock::new(
        || {
            vec![
        (regex::Regex::new(r"(?s)-----BEGIN [^-]*PRIVATE KEY-----.*?-----END [^-]*PRIVATE KEY-----").unwrap(),"[REDACTED]"),
        (regex::Regex::new(r"(?i)\b(Bearer|Basic)\s+[A-Za-z0-9+/_=.-]+").unwrap(),"$1 [REDACTED]"),
        (regex::Regex::new(r#"(?i)\b(password|passwd|secret|token|authorization|credential|api[-_]?key|private[-_]?key|connectionString|dsn|encryptionKey)["']?\s*[:=]\s*(?:"[^"]*"|'[^']*'|\S+)"#).unwrap(),"$1=[REDACTED]"),
        (regex::Regex::new(r"(?i)(https?://)[^\s/@]+:[^\s/@]+@").unwrap(),"$1[REDACTED]@")]
        },
    );
    let mut text = value
        .chars()
        .filter(|c| !c.is_control() || ['\n', '\r', '\t'].contains(c))
        .collect::<String>();
    for (pattern, replacement) in PATTERNS.iter() {
        text = pattern.replace_all(&text, *replacement).into_owned();
    }
    text
}
pub fn redact(v: &mut Value) {
    match v {
        Value::Object(obj) => {
            for (k, v) in obj {
                if [
                    "secret",
                    "credentials",
                    "password",
                    "token",
                    "privateKey",
                    "key",
                    "passwordHash",
                    "apiKey",
                    "accessKey",
                    "connectionString",
                    "dsn",
                    "encryptionKey",
                    "authorization",
                    "body",
                    "blob",
                ]
                .iter()
                .any(|x| k.eq_ignore_ascii_case(x))
                {
                    *v = json!("[REDACTED]")
                } else {
                    redact(v)
                }
            }
        }
        Value::Array(a) => {
            for v in a {
                redact(v)
            }
        }
        Value::String(value) => *value = safe_text(value),
        _ => {}
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secrets_in_logs_and_urls_are_redacted() {
        let mut row = json!({"details":"password=\"value with spaces\" Authorization: Bearer abc123 URL https://user:pass@example.test/", "privateKey":"key", "dsn":"db-secret","publicKey":"public"});
        redact(&mut row);
        let text = row.to_string();
        assert!(!text.contains("value with spaces"));
        assert!(!text.contains("abc123"));
        assert!(!text.contains("user:pass"));
        assert!(!text.contains("db-secret"));
        assert_eq!(row["publicKey"], "public");
    }
    #[test]
    fn reject_missing_result() {
        let c = json!([["x:Domain/set",{"create":{"d1":{}}},"c1"]]);
        assert!(validate(
            &c,
            &json!({"methodResponses":[["x:Domain/set",{"created":{}},"c1"]]})
        )
        .is_err());
        assert!(validate(
            &c,
            &json!({"methodResponses":[["x:Domain/set",{"created":{"d1":{"id":"d"}}},"c1"]]})
        )
        .is_ok());
    }
}

pub async fn metadata(s: &App, server: i64, path: &str) -> Result<Value> {
    if !["/api/account", "/api/schema"].contains(&path) {
        bail!("Unsupported metadata path");
    }
    let r = sqlx::query(
        "SELECT base_url,token_encrypted,dry_run FROM stalwart_servers WHERE id=$1 AND active=1",
    )
    .bind(server)
    .fetch_optional(&s.db)
    .await?
    .ok_or_else(|| anyhow!("Server unavailable"))?;
    if r.get::<i16, _>("dry_run") == 1 {
        bail!("Dry-run does not query live metadata");
    }
    let mut url = url::Url::parse(&r.get::<String, _>("base_url"))?;
    url.set_path(path);
    url.set_query(None);
    url.set_fragment(None);
    let token = crypto::open(
        &s.config.key,
        &r.get::<Option<String>, _>("token_encrypted")
            .ok_or_else(|| anyhow!("Missing token"))?,
    )?;
    let authorization = if let Some(v) = token.strip_prefix("basic:") {
        format!("Basic {}", v.trim())
    } else if token.contains(':') && !token.starts_with("API_") {
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(token.as_bytes())
        )
    } else {
        format!("Bearer {token}")
    };
    let response = s
        .http
        .get(url)
        .header("authorization", authorization)
        .send()
        .await?
        .error_for_status()?;
    crate::sepay::bounded_json(response, 4 * 1024 * 1024)
        .await
        .map_err(|e| anyhow!(e.1))
}
