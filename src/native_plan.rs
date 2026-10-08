use crate::error::{Error, Result};
use serde_json::{json, Value};
pub const TYPES: &[&str] = &[
    "Account",
    "AccountPassword",
    "ApiKey",
    "AppPassword",
    "Role",
    "Tenant",
    "Domain",
    "Directory",
    "DataStore",
    "BlobStore",
    "SearchStore",
    "InMemoryStore",
    "DkimSignature",
    "Certificate",
    "AcmeProvider",
    "DnsServer",
    "NetworkListener",
    "SystemSettings",
    "Authentication",
    "Security",
    "Jmap",
    "Imap",
    "ReportSettings",
    "SenderAuth",
    "MtaInboundSession",
    "MtaOutboundStrategy",
    "MtaRoute",
    "MtaTlsStrategy",
    "MtaOutboundThrottle",
    "MtaInboundThrottle",
    "MtaQueueQuota",
    "SpamSettings",
    "SpamRule",
    "PublicKey",
    "OAuthClient",
];
pub fn types(value: &str) -> Result<Vec<String>> {
    if value.len() > 3000 {
        return Err(Error::bad("Danh sách object quá dài"));
    }
    let mut types = Vec::new();
    for ty in value
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
    {
        if !TYPES.contains(&ty) {
            return Err(Error::bad("Loại object không được hỗ trợ"));
        }
        if !types.iter().any(|t| t == ty) {
            types.push(ty.into());
        }
    }
    if types.is_empty() {
        return Err(Error::bad("Chọn object cần backup"));
    }
    Ok(types)
}
pub fn vault(value: &str) -> Result<Value> {
    if value.len() > 1024 * 1024 {
        return Err(Error::bad("Vault tối đa 1 MB"));
    }
    let data: Value =
        serde_json::from_str(value).map_err(|_| Error::bad("Vault JSON không hợp lệ"))?;
    let object = data
        .as_object()
        .ok_or_else(|| Error::bad("Vault phải là object"))?;
    let secret=regex::Regex::new("(?i)password|secret|token|credential|privateKey|accessKey|apiKey|connectionString|dsn|encryptionKey").unwrap();
    for (ty, objects) in object {
        if !TYPES.contains(&ty.as_str()) {
            return Err(Error::bad("Loại object vault không hỗ trợ"));
        }
        for (id, fields) in objects
            .as_object()
            .ok_or_else(|| Error::bad("Object vault không hợp lệ"))?
        {
            if !regex::Regex::new(r"^[A-Za-z0-9_.:@+-]{1,190}$")
                .unwrap()
                .is_match(id)
                || !fields
                    .as_object()
                    .is_some_and(|f| f.keys().all(|k| secret.is_match(k)))
            {
                return Err(Error::bad("Vault chỉ nhận trường bí mật cho ID hợp lệ"));
            }
        }
    }
    Ok(data)
}
fn masked(v: &Value) -> bool {
    match v {
        Value::String(s) => s.len() >= 3 && s.bytes().all(|b| b == b'*'),
        Value::Array(a) => a.iter().any(masked),
        Value::Object(o) => o.values().any(masked),
        _ => false,
    }
}
pub fn build(input: &str, types: &[String], vault: &Value, lab: bool) -> Result<(String, Value)> {
    if input.len() > 64 * 1024 * 1024 {
        return Err(Error::bad("Plan tối đa 64 MB"));
    }
    let mut output = String::new();
    let mut records = 0;
    let mut counts = serde_json::Map::new();
    let mut used = std::collections::HashSet::new();
    let mut excluded = std::collections::BTreeSet::new();
    for line in input.lines().filter(|l| !l.trim().is_empty()) {
        if line.len() > 8 * 1024 * 1024 {
            return Err(Error::bad("Một dòng plan vượt 8 MB"));
        }
        let mut row: Value =
            serde_json::from_str(line).map_err(|_| Error::bad("Plan JSON không hợp lệ"))?;
        let ty = row["object"]
            .as_str()
            .filter(|t| types.iter().any(|v| v == *t))
            .ok_or_else(|| Error::bad("Plan chứa object ngoài phạm vi"))?
            .to_owned();
        let singleton = row["@type"] == "update";
        if !singleton && row["@type"] != "upsert" {
            return Err(Error::bad("Plan chỉ cho phép upsert/update"));
        }
        if !row["value"].is_object() {
            return Err(Error::bad("Plan value không hợp lệ"));
        }
        if lab
            && !matches!(
                ty.as_str(),
                "Role" | "Tenant" | "Domain" | "Account" | "DkimSignature"
            )
        {
            excluded.insert(ty);
            continue;
        }
        if lab && singleton {
            return Err(Error::bad("Diễn tập không áp dụng singleton"));
        }
        if !singleton && row["matchOn"].is_null() {
            return Err(Error::bad("Plan upsert cần matchOn"));
        }
        let mut bodies = if singleton {
            serde_json::Map::from_iter([("singleton".into(), row["value"].clone())])
        } else {
            row["value"].as_object().unwrap().clone()
        };
        for (id, body) in &mut bodies {
            let obj = body
                .as_object_mut()
                .ok_or_else(|| Error::bad("Body object không hợp lệ"))?;
            if let Some(fields) = vault[&ty][id].as_object() {
                obj.extend(fields.clone());
                used.insert((ty.clone(), id.clone()));
            }
            if masked(body) {
                return Err(Error::bad("Snapshot còn credential bị mask; bổ sung vault"));
            }
            if lab && ty == "Domain" {
                body["isEnabled"] = json!(false);
                for key in ["dnsManagement", "dkimManagement", "certificateManagement"] {
                    body[key] = json!({"@type":"Manual"});
                }
            }
            let n = counts.get(&ty).and_then(Value::as_u64).unwrap_or(0) + 1;
            counts.insert(ty.clone(), json!(n));
        }
        row["value"] = if singleton {
            bodies.remove("singleton").unwrap()
        } else {
            json!(bodies)
        };
        records += 1;
        if records > 100000 {
            return Err(Error::bad("Plan vượt 100.000 operation"));
        }
        output.push_str(&row.to_string());
        output.push('\n');
        if output.len() > 64 * 1024 * 1024 {
            return Err(Error::bad("Plan sau hợp nhất vượt giới hạn"));
        }
    }
    if records == 0 {
        return Err(Error::bad("Plan rỗng"));
    }
    if !lab {
        for (ty, objects) in vault.as_object().into_iter().flat_map(|o| o.iter()) {
            for id in objects.as_object().into_iter().flat_map(|o| o.keys()) {
                if !used.contains(&(ty.clone(), id.clone())) {
                    return Err(Error::bad("Vault không khớp object snapshot"));
                }
            }
        }
    }
    Ok((
        output,
        json!({"records":records,"objects":counts,"vault_entries":used.len(),"excluded_lab_types":excluded}),
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn masked_and_lab_settings() {
        let types = types("Domain,SystemSettings").unwrap();
        let plan=json!({"@type":"upsert","object":"Domain","matchOn":["name"],"value":{"domain-1":{"name":"example.test","privateKey":"***"}}}).to_string();
        assert!(build(&plan, &types, &json!({}), false).is_err());
        let secrets = vault(r#"{"Domain":{"domain-1":{"privateKey":"fixture-key"}}}"#).unwrap();
        let (full, _) = build(&plan, &types, &secrets, false).unwrap();
        assert!(full.contains("fixture-key"));
        let (lab, stats) = build(&full, &types, &json!({}), true).unwrap();
        let row: Value = serde_json::from_str(lab.trim()).unwrap();
        assert_eq!(row["value"]["domain-1"]["isEnabled"], false);
        assert_eq!(row["value"]["domain-1"]["dnsManagement"]["@type"], "Manual");
        assert_eq!(stats["records"], 1);
        assert!(vault(r#"{"Domain":{"id":{"name":"bad"}}}"#).is_err());
    }
}
