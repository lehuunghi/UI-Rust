use crate::{
    auth,
    error::{Error, Result},
    operations, resources, stalwart, App,
};
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use serde_json::{json, Value};
use sqlx::Row;
use std::sync::LazyLock;
static SPECS: LazyLock<Value> =
    LazyLock::new(|| serde_json::from_str(include_str!("../locales/management.json")).unwrap());
pub fn spec(kind: &str) -> Result<&'static Value> {
    SPECS.get(kind).ok_or_else(Error::missing)
}
pub async fn method(s: &App, server: i64, kind: &str, args: Value) -> Result<Value> {
    let result = stalwart::call(s, server, json!([[kind, args, "manage"]])).await?;
    Ok(result["methodResponses"][0][1].clone())
}
pub async fn page(s: &App, server: i64, kind: &str, offset: i64, filter: Value) -> Result<Value> {
    let spec = spec(kind)?;
    let ty = spec["type"].as_str().unwrap();
    let q = method(
        s,
        server,
        &format!("x:{ty}/query"),
        json!({"position":offset,"limit":50,"calculateTotal":true,"filter":filter}),
    )
    .await?;
    let ids = q["ids"]
        .as_array()
        .filter(|a| {
            a.len() <= 50
                && a.iter()
                    .all(|id| id.as_str().is_some_and(|s| !s.is_empty() && s.len() <= 190))
        })
        .ok_or_else(|| Error::bad("Danh sách ID từ server không hợp lệ"))?;
    let mut rows = if ids.is_empty() {
        json!([])
    } else {
        method(
            s,
            server,
            &format!("x:{ty}/get"),
            json!({"ids":ids,"properties":spec["properties"]}),
        )
        .await?["list"]
            .clone()
    };
    let a = rows
        .as_array_mut()
        .filter(|a| a.len() <= 50)
        .ok_or_else(|| Error::bad("Phản hồi server không hợp lệ"))?;
    let mut seen = std::collections::HashSet::new();
    for row in a {
        let id = row["id"]
            .as_str()
            .filter(|id| seen.insert((*id).to_string()))
            .ok_or_else(|| Error::bad("Server trả ID trùng hoặc không hợp lệ"))?;
        if !ids.iter().any(|expected| expected == id) {
            return Err(Error::bad("Server trả đối tượng ngoài yêu cầu"));
        }
        let obj = row.as_object_mut().unwrap();
        obj.retain(|key, _| {
            spec["properties"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == key)
        });
        stalwart::redact(row);
    }
    Ok(json!({"items":rows,"total":q["total"],"next":if ids.len()==50 {Some(offset+50)}else{None}}))
}
pub async fn list(State(s): State<App>, h: HeaderMap, Json(v): Json<Value>) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "operations", true, true).await?;
    u.super_admin()?;
    let server = resources::number(&v, "server_id")?;
    let mut version_guard = s.db.begin().await?;
    sqlx::query("SELECT id FROM stalwart_servers WHERE id=$1 AND active=1 FOR SHARE")
        .bind(server)
        .fetch_optional(&mut *version_guard)
        .await?
        .ok_or_else(Error::missing)?;

    let kind = resources::text(&v, "kind", 30)?;
    let offset = v["offset"].as_i64().unwrap_or(0).clamp(0, 100_000);
    let filter = v.get("filter").cloned().unwrap_or(json!({}));
    if !filter.is_object() {
        return Err(Error::bad("Bộ lọc không hợp lệ"));
    }
    let result = page(&s, server, kind, offset, filter.clone()).await?;
    if ["dmarc_reports", "tls_reports", "arf_reports", "metrics"].contains(&kind) {
        let mut report_tx = s.db.begin().await?;
        for row in result["items"].as_array().unwrap() {
            let summaries = if kind == "metrics" {
                crate::reports::native(kind, row, &json!(""))?
            } else {
                crate::reports::native(kind, &row["report"], &row["receivedAt"])?
            };
            for summary in summaries {
                crate::reports::store(&mut report_tx, server, &summary, "native").await?;
            }
        }
        report_tx.commit().await?;
    }

    let r =
        sqlx::query("SELECT config_version,dry_run FROM stalwart_servers WHERE id=$1 AND active=1")
            .bind(server)
            .fetch_one(&s.db)
            .await?;
    let mode = if r.get::<i16, _>("dry_run") == 1 {
        "dry_run"
    } else {
        "live"
    };
    let id:i64=sqlx::query_scalar("INSERT INTO operational_collections(server_id,server_version,kind,mode,filter_data,page_position,data,total,has_more,status) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,'ready') RETURNING id").bind(server).bind(r.get::<i64,_>("config_version")).bind(kind).bind(mode).bind(filter).bind(offset as i32).bind(&result["items"]).bind(result["total"].as_i64().and_then(|n|i32::try_from(n).ok())).bind(if result["next"].is_null(){0_i16}else{1}).fetch_one(&s.db).await?;
    auth::audit(
        &s,
        Some(u.id),
        "management_read",
        json!({"server_id":server,"kind":kind,"collection_id":id}),
    )
    .await?;
    Ok(Json(
        json!({"collection_id":id,"mode":mode,"items":result["items"],"total":result["total"],"next":result["next"]}),
    ))
}
fn id(v: &Value) -> Result<&str> {
    v.as_str()
        .filter(|s| !s.is_empty() && s.len() <= 190 && !s.chars().any(char::is_control))
        .ok_or_else(|| Error::bad("ID server không hợp lệ"))
}
fn expiry(v: &Value) -> Result<String> {
    let time = chrono::DateTime::parse_from_rfc3339(v.as_str().unwrap_or(""))
        .map_err(|_| Error::bad("Thời hạn cần RFC3339 có múi giờ"))?;
    let now = chrono::Utc::now();
    if time < now + chrono::Duration::minutes(5) || time > now + chrono::Duration::days(366) {
        return Err(Error::bad("Thời hạn từ 5 phút đến 366 ngày"));
    }
    Ok(time.to_rfc3339())
}
fn ips(v: &Value) -> Result<Value> {
    let mut map = serde_json::Map::new();
    for value in v
        .as_str()
        .unwrap_or("")
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
    {
        let (ip, mask) = value
            .split_once('/')
            .map(|(ip, mask)| (ip, Some(mask)))
            .unwrap_or((value, None));
        let address: std::net::IpAddr = ip.parse().map_err(|_| Error::bad("IP không hợp lệ"))?;
        if mask.is_some_and(|m| {
            m.parse::<u8>().is_err()
                || m.parse::<u8>().unwrap() > if address.is_ipv4() { 32 } else { 128 }
        }) {
            return Err(Error::bad("CIDR không hợp lệ"));
        }
        map.insert(value.into(), json!(true));
    }
    if map.len() > 20 {
        return Err(Error::bad("Tối đa 20 IP/CIDR"));
    }
    Ok(json!(map))
}
async fn grants(s: &App, server: i64, v: &Value) -> Result<Value> {
    let perms: Vec<_> = v
        .as_str()
        .unwrap_or("")
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .collect();
    if perms.len() > 100
        || perms.iter().any(|p| {
            p.len() > 100
                || !p
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        })
    {
        return Err(Error::bad("Danh sách quyền không hợp lệ"));
    }
    let dry: i16 =
        sqlx::query_scalar("SELECT dry_run FROM stalwart_servers WHERE id=$1 AND active=1")
            .bind(server)
            .fetch_one(&s.db)
            .await?;
    if dry == 0 {
        let metadata = stalwart::metadata(s, server, "/api/account").await?;
        let available = metadata["data"]["permissions"]
            .as_array()
            .ok_or_else(|| Error::bad("Không đọc được quyền token thực tế"))?;
        if perms.iter().any(|p| !available.iter().any(|v| v == *p)) {
            return Err(Error::forbidden());
        }
    }
    Ok(Value::Object(
        perms.into_iter().map(|p| (p.into(), json!(true))).collect(),
    ))
}
pub async fn preview(
    State(s): State<App>,
    h: HeaderMap,
    Json(v): Json<Value>,
) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "operations", true, true).await?;
    u.super_admin()?;
    let server = resources::number(&v, "server_id")?;
    let mut version_guard = s.db.begin().await?;
    sqlx::query("SELECT id FROM stalwart_servers WHERE id=$1 AND active=1 FOR SHARE")
        .bind(server)
        .fetch_optional(&mut *version_guard)
        .await?
        .ok_or_else(Error::missing)?;

    let kind = resources::text(&v, "kind", 30)?;
    let spec = spec(kind)?;
    let ty = spec["type"].as_str().unwrap();
    let action = resources::text(&v, "action", 40)?;
    let mut args = json!({});
    let mut guards = Vec::new();
    let selected = v
        .get("ids")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let fields = &v["fields"];
    let mut before = json!([]);
    if !selected.is_empty() {
        if selected.len() > 50 {
            return Err(Error::bad("Chọn tối đa 50 mục"));
        }
        for value in &selected {
            id(value)?;
        }
        let response = method(
            &s,
            server,
            &format!("x:{ty}/get"),
            json!({"ids":selected,"properties":spec["properties"]}),
        )
        .await?;
        before = response["list"].clone();
        if let Some(rows) = before.as_array_mut() {
            for row in rows {
                let fields = row
                    .as_object_mut()
                    .ok_or_else(|| Error::bad("Đối tượng server không hợp lệ"))?;
                fields.retain(|key, _| {
                    spec["properties"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|v| v == key)
                });
                stalwart::redact(row);
            }
        }

        let rows = before
            .as_array()
            .ok_or_else(|| Error::bad("Phản hồi không hợp lệ"))?;
        if rows.len() != selected.len()
            || selected
                .iter()
                .any(|id| rows.iter().filter(|r| r["id"] == *id).count() != 1)
        {
            return Err(Error::conflict("Một mục đã thay đổi hoặc không tồn tại"));
        }
        args["ifInState"] = json!(id(&response["state"])?);
    }
    let method_type = match (kind, action) {
        ("queue", "retry" | "defer") => {
            let mut update = serde_json::Map::new();
            let due = (chrono::Utc::now()
                + chrono::Duration::minutes(if action == "defer" { 60 } else { 0 }))
            .to_rfc3339();
            for row in before.as_array().unwrap() {
                let mut patch = json!({"nextRetry":due});
                let recipients = row["recipients"]
                    .as_object()
                    .ok_or_else(|| Error::bad("Không có người nhận"))?;
                let mut count = 0;
                for (address, status) in recipients {
                    if ["Scheduled", "TemporaryFailure"]
                        .iter()
                        .any(|s| status["status"]["@type"] == *s)
                    {
                        patch[format!(
                            "recipients/{}/retryDue",
                            address.replace('~', "~0").replace('/', "~1")
                        )] = json!(due);
                        count += 1;
                    }
                }
                if count == 0 {
                    return Err(Error::bad("Thư không còn người nhận có thể thử lại"));
                }
                update.insert(id(&row["id"])?.to_string(), patch);
            }
            args["update"] = json!(update);
            ty
        }
        ("queue", "cancel") => {
            if selected.is_empty() {
                return Err(Error::bad("Chọn thư cần hủy"));
            }
            args["destroy"] = json!(selected);
            ty
        }
        ("tasks", "retry") => {
            let mut update = serde_json::Map::new();
            for row in before.as_array().unwrap() {
                if !["Failed", "Retry"]
                    .iter()
                    .any(|s| row["status"]["@type"] == *s)
                    || ![
                        "IndexDocument",
                        "IndexTrace",
                        "AccountMaintenance",
                        "TenantMaintenance",
                        "SpamFilterMaintenance",
                        "AcmeRenewal",
                        "DkimManagement",
                    ]
                    .iter()
                    .any(|s| row["@type"] == *s)
                {
                    return Err(Error::bad("Tác vụ không đủ điều kiện thử lại"));
                }
                update.insert(
                    id(&row["id"])?.to_string(),
                    json!({"status":{"@type":"Pending","due":chrono::Utc::now().to_rfc3339()}}),
                );
            }
            args["update"] = json!(update);
            ty
        }
        ("queue", "pause" | "resume") => {
            args = json!({"create":{"item":{"@type":if action=="pause"{"PauseMtaQueue"}else{"ResumeMtaQueue"}}}});
            "Action"
        }
        ("tasks", "recalculate_quota" | "reindex_account" | "recalculate_imap_uid") => {
            let local = resources::number(fields, "account_id")?;
            let row:Value=sqlx::query_scalar("SELECT jsonb_build_object('id',id,'server_id',server_id,'stalwart_account_id',stalwart_account_id,'status',status,'domain_id',domain_id,'local_part',local_part) FROM email_accounts WHERE id=$1 AND server_id=$2 AND status='active' AND sync_status='synced'").bind(local).bind(server).fetch_optional(&s.db).await?.ok_or_else(Error::missing)?;
            guards.push(json!({"kind":"accounts","id":local,"expected":row}));
            args = json!({"create":{"item":{"@type":"AccountMaintenance","accountId":row["stalwart_account_id"],"maintenanceType":match action{"recalculate_quota"=>"recalculateQuota","recalculate_imap_uid"=>"recalculateImapUid",_=>"reindex"},"status":{"@type":"Pending","due":chrono::Utc::now().to_rfc3339()}}}});
            "Task"
        }
        ("tasks", "reload_tls" | "reload_settings" | "reload_lookup" | "update_spam_rules") => {
            if action == "update_spam_rules" {
                args = json!({"create":{"item":{"@type":"SpamFilterMaintenance","maintenanceType":"updateRules","status":{"@type":"Pending","due":chrono::Utc::now().to_rfc3339()}}}});
                "Task"
            } else {
                args = json!({"create":{"item":{"@type":match action{"reload_tls"=>"ReloadTlsCertificates","reload_settings"=>"ReloadSettings",_=>"ReloadLookupStores"}}}});
                "Action"
            }
        }
        ("dns_domains", "dns_publish" | "renew_certificate" | "dkim_cycle") => {
            if selected.len() != 1 {
                return Err(Error::bad("Chọn một domain"));
            }
            let row = &before[0];
            let key = match action {
                "dns_publish" => "dnsManagement",
                "renew_certificate" => "certificateManagement",
                _ => "dkimManagement",
            };
            if row[key]["@type"] != "Automatic" {
                return Err(Error::bad(
                    "Domain chưa bật chế độ quản lý tự động tương ứng",
                ));
            }
            if action == "dkim_cycle"
                && (row["dnsManagement"]["@type"] != "Automatic"
                    || row[key]["retireAfter"].as_i64().unwrap_or(604800000) < 604800000
                    || row[key]["deleteAfter"].as_i64().unwrap_or(2592000000) < 604800000)
            {
                return Err(Error::bad(
                    "DKIM cần DNS tự động và giữ khóa cũ ít nhất 7 ngày",
                ));
            }
            args = json!({"create":{"item":{"@type":match action{"dns_publish"=>"DnsManagement","renew_certificate"=>"AcmeRenewal",_=>"DkimManagement"},"domainId":selected[0],"status":{"@type":"Pending","due":chrono::Utc::now().to_rfc3339()}}}});
            if action == "dns_publish" {
                args["create"]["item"]["updateRecords"] =
                    json!({"mx":true,"spf":true,"dkim":true,"dmarc":true});
                args["create"]["item"]["onSuccessRenewCertificate"] = json!(false);
            }
            "Task"
        }
        (
            "roles" | "tenants" | "api_keys" | "app_passwords" | "outbound" | "inbound" | "blocked"
            | "allowed",
            "create" | "update" | "rotate" | "disable" | "destroy",
        ) => {
            if !["create"].contains(&action) && selected.len() != 1 {
                return Err(Error::bad("Chọn một mục"));
            }
            if action == "destroy" {
                if kind == "tenants" {
                    let bound:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM native_tenant_bindings WHERE server_id=$1 AND tenant_id=$2)").bind(server).bind(id(&selected[0])?).fetch_one(&s.db).await?;
                    if bound {
                        return Err(Error::conflict("Tenant đang gắn với khách hàng"));
                    }
                    for k in ["dns_domains", "accounts", "roles"] {
                        let type_name = match k {
                            "accounts" => "Account",
                            "roles" => "Role",
                            _ => "Domain",
                        };
                        let query = method(
                            &s,
                            server,
                            &format!("x:{type_name}/query"),
                            json!({"limit":1,"filter":{"memberTenantId":selected[0]},"calculateTotal":true}),
                        )
                        .await?;
                        if query["total"] != 0
                            || !query["ids"].as_array().is_some_and(Vec::is_empty)
                        {
                            return Err(Error::bad("Tenant còn domain hoặc tài khoản"));
                        }
                    }
                }
                args["destroy"] = json!(selected);
            } else {
                let data = if action == "disable" {
                    if !["inbound", "outbound"].contains(&kind) {
                        return Err(Error::bad("Thao tác không hỗ trợ"));
                    }
                    json!({"enable":false})
                } else {
                    build_fields(&s, server, kind, fields, &before[0]).await?
                };
                if action == "create" || action == "rotate" {
                    if action == "rotate" && !["api_keys", "app_passwords"].contains(&kind) {
                        return Err(Error::bad("Chỉ luân chuyển credential"));
                    }
                    args["create"] = json!({"item":data});
                } else {
                    args["update"] = json!({id(&selected[0])?:data});
                }
            }
            ty
        }
        _ => return Err(Error::bad("Thao tác không được hỗ trợ")),
    };
    if args
        .get("update")
        .and_then(Value::as_object)
        .is_some_and(|o| o.is_empty())
    {
        return Err(Error::bad("Chọn ít nhất một mục"));
    }
    let Json(mut plan)=operations::preview(State(s.clone()),h,Json(json!({"server_id":server,"calls":[[format!("x:{method_type}/set"),args,"c1"]],"preconditions":guards,"remote_preconditions":if selected.is_empty(){json!([])}else{json!([{"method":format!("x:{ty}/get"),"ids":selected,"properties":spec["properties"],"expected":before}])}}))).await?;
    let mut review = before;
    stalwart::redact(&mut review);
    plan["before"] = review;
    plan["kind"] = json!(kind);
    plan["action"] = json!(action);
    Ok(Json(plan))
}
async fn build_fields(
    s: &App,
    server: i64,
    kind: &str,
    v: &Value,
    before: &Value,
) -> Result<Value> {
    match kind {
        "roles" => {
            let mut fields = json!({"description":resources::text(v,"description",190)?,"roleIds":{},"enabledPermissions":grants(s,server,&v["permissions"]).await?,"disabledPermissions":{}});
            if let Some(tenant) = before["memberTenantId"]
                .as_str()
                .or_else(|| v["tenant_id"].as_str())
                .filter(|s| !s.is_empty())
            {
                fields["memberTenantId"] = json!(tenant);
            }
            Ok(fields)
        }
        "tenants" => {
            let mut quotas = serde_json::Map::new();
            for key in [
                "maxAccounts",
                "maxDomains",
                "maxGroups",
                "maxRoles",
                "maxDiskQuota",
            ] {
                let number = v[key]
                    .as_i64()
                    .filter(|n| *n >= 0 && *n <= 9_000_000_000_000_000)
                    .ok_or_else(|| Error::bad("Quota không hợp lệ"))?;
                if key == "maxDiskQuota"
                    && number > 0
                    && number < before["usedDiskQuota"].as_i64().unwrap_or(0)
                {
                    return Err(Error::bad("Quota thấp hơn dung lượng đã dùng"));
                }
                quotas.insert(key.into(), json!(number));
            }
            Ok(
                json!({"name":resources::text(v,"name",190)?,"roles":{"@type":"Custom","roleIds":{}},"permissions":{"@type":"Replace","enabledPermissions":grants(s,server,&v["permissions"]).await?,"disabledPermissions":{}},"quotas":quotas}),
            )
        }
        "api_keys" | "app_passwords" => {
            let permissions = grants(s, server, &v["permissions"]).await?;
            if permissions.as_object().is_some_and(|o| o.is_empty()) {
                return Err(Error::bad("Cần quyền API cụ thể"));
            }
            Ok(
                json!({"description":resources::text(v,"description",190)?,"expiresAt":expiry(&v["expires_at"] )?,"permissions":{"@type":"Replace","permissions":permissions},"allowedIps":ips(&v["allowed_ips"])?}),
            )
        }
        "inbound" | "outbound" => {
            let scope = resources::text(v, "scope", 30)?;
            let scopes = if kind == "outbound" {
                ["sender", "senderDomain", "rcptDomain"]
            } else {
                ["authenticatedAs", "senderDomain", "rcptDomain"]
            };
            if !scopes.contains(&scope) {
                return Err(Error::bad("Phạm vi không hợp lệ"));
            }
            let value = resources::text(v, "value", 253)?.to_ascii_lowercase();
            if if scope.ends_with("Domain") {
                !resources::domain_name(&value)
            } else {
                !auth::valid_email(&value)
            } {
                return Err(Error::bad("Email/domain không hợp lệ"));
            }
            let count = resources::number(v, "count")?;
            let minutes = resources::number(v, "minutes")?;
            if count > 1_000_000 || minutes > 1440 {
                return Err(Error::bad("Giới hạn gửi không hợp lệ"));
            }
            let variable = match scope {
                "senderDomain" => "sender_domain",
                "rcptDomain" => "rcpt_domain",
                "authenticatedAs" => "authenticated_as",
                _ => "sender",
            };
            let mut data = json!({"enable":true,"key":{scope:true},"match":{"else":format!("{variable} == {}",json!(value))},"rate":{"count":count,"period":minutes*60000}});
            if kind == "outbound" {
                data["description"] = json!(v["description"].as_str().unwrap_or("Panel policy"));
            }
            Ok(data)
        }
        "blocked" | "allowed" => {
            let value = resources::text(v, "value", 100)?;
            if ips(&json!(value))?.as_object().unwrap().len() != 1 {
                return Err(Error::bad("Nhập một IP/CIDR"));
            }
            Ok(
                json!({"address":value,"reason":if kind=="blocked"{"manual"}else{v["description"].as_str().unwrap_or("")},"expiresAt":expiry(&v["expires_at"])?}),
            )
        }
        _ => Err(Error::bad("Loại cấu hình không hỗ trợ")),
    }
}
pub async fn reveal(
    State(s): State<App>,
    h: HeaderMap,
    Path(id): Path<i64>,
    Json(v): Json<Value>,
) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "operations", true, true).await?;
    u.super_admin()?;
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id=$1")
        .bind(u.id)
        .fetch_one(&s.db)
        .await?;
    if !crate::crypto::verify(v["password"].as_str().unwrap_or(""), &hash) {
        return Err(Error::forbidden());
    }
    let mut tx = s.db.begin().await?;
    let encrypted:String=sqlx::query_scalar("SELECT result_secret FROM rust_change_plans WHERE id=$1 AND actor_id=$2 AND status='done' AND finished_at>now()-interval '15 minutes' AND result_secret IS NOT NULL FOR UPDATE").bind(id).bind(u.id).fetch_optional(&mut *tx).await?.ok_or_else(||Error::conflict("Secret đã lấy hoặc hết hạn"))?;
    let secret = crate::crypto::open(&s.config.key, &encrypted)?;
    sqlx::query("UPDATE rust_change_plans SET result_secret=NULL WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO audit_logs(user_id,action,context) VALUES($1,'credential_revealed',$2)",
    )
    .bind(u.id)
    .bind(json!({"plan_id":id}))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"secret":secret})))
}
