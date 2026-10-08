use crate::{
    auth, backup, backup_admin, backup_tools as files, crypto,
    error::{Error, Result},
    management, native_plan, recovery_sources as sources, resources, stalwart, App,
};
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap},
    response::{IntoResponse, Response},
    Json,
};
use base64::Engine;
use serde_json::{json, Value};
use sqlx::{PgConnection, Row};
use std::path::{Path as FilePath, PathBuf};
fn directory(s: &App) -> PathBuf {
    backup_admin::directory(s).join("recovery")
}
fn archive_path(s: &App, name: &str) -> Result<PathBuf> {
    if !regex::Regex::new(r"^recovery-[0-9]+-[0-9]+-[a-f0-9]{16}\.rcenc$")
        .unwrap()
        .is_match(name)
    {
        return Err(Error::bad("Tên archive recovery không hợp lệ"));
    }
    Ok(directory(s).join(name))
}
async fn profile(db: &mut PgConnection, id: i64) -> Result<Value> {
    sqlx::query_scalar("SELECT to_jsonb(p) FROM recovery_profiles p WHERE id=$1 FOR SHARE")
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or_else(Error::missing)
}
async fn context(db: &mut PgConnection, p: &Value) -> Result<(Value, Value, String)> {
    let server:Value=sqlx::query_scalar("SELECT to_jsonb(s) FROM stalwart_servers s WHERE id=$1 AND active=1 AND dry_run=0 FOR SHARE").bind(p["server_id"].as_i64().ok_or_else(Error::missing)?).fetch_optional(db).await?.ok_or_else(||Error::bad("Recovery cần server thật đang hoạt động"))?;
    let config = sources::get(
        p["source_key"].as_str().unwrap_or(""),
        server["base_url"].as_str().unwrap(),
    )
    .await?;
    let hash = crypto::hash(&json!([server["id"], server["config_version"], config]).to_string());
    Ok((server, config, hash))
}
pub async fn list(State(s): State<App>, h: HeaderMap) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "operations", true, false).await?;
    u.super_admin()?;
    let profiles: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(p)-'vault_encrypted' FROM recovery_profiles p ORDER BY id DESC LIMIT 100",
    )
    .fetch_all(&s.db)
    .await?;
    let artifacts:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(a)-'password_encrypted' FROM recovery_artifacts a ORDER BY id DESC LIMIT 100").fetch_all(&s.db).await?;
    let jobs: Vec<Value> =
        sqlx::query_scalar("SELECT to_jsonb(j) FROM recovery_jobs j ORDER BY id DESC LIMIT 100")
            .fetch_all(&s.db)
            .await?;
    let keys: Vec<String> = sources::all()
        .await?
        .as_object()
        .into_iter()
        .flat_map(|o| o.keys().cloned())
        .collect();
    Ok(Json(
        json!({"profiles":profiles,"artifacts":artifacts,"jobs":jobs,"source_keys":keys}),
    ))
}
pub async fn save(State(s): State<App>, h: HeaderMap, Json(v): Json<Value>) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "operations", true, true).await?;
    u.super_admin()?;
    let server = resources::number(&v, "server_id")?;
    let key = resources::text(&v, "source_key", 100)?;
    let types = native_plan::types(resources::text(&v, "object_types", 3000)?)?;
    let hours = v["interval_hours"]
        .as_i64()
        .filter(|n| (1..=168).contains(n))
        .ok_or_else(|| Error::bad("Chu kỳ 1 đến 168 giờ"))?;
    let keep = v["keep_local"]
        .as_i64()
        .filter(|n| (1..=90).contains(n))
        .ok_or_else(|| Error::bad("Giữ 1 đến 90 bản"))?;
    let target = v["restore_target"] == true;
    let vault = v["vault_json"].as_str().unwrap_or("");
    let mut tx = s.db.begin().await?;
    let base: String = sqlx::query_scalar(
        "SELECT base_url FROM stalwart_servers WHERE id=$1 AND active=1 AND dry_run=0 FOR SHARE",
    )
    .bind(server)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(Error::missing)?;
    let config = sources::get(key, &base).await?;
    if target {
        sources::target(&config).await?;
    }
    let old: Option<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(p) FROM recovery_profiles p WHERE server_id=$1 FOR UPDATE",
    )
    .bind(server)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(p) = &old {
        if p["version"] != v["version"] {
            return Err(Error::conflict("Hồ sơ đã thay đổi; tải lại trang"));
        }
        let busy:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM recovery_jobs WHERE (profile_id=$1 OR target_profile_id=$1) AND status IN ('pending','running'))").bind(p["id"].as_i64().unwrap()).fetch_one(&mut *tx).await?;
        if busy {
            return Err(Error::conflict("Hồ sơ đang có tác vụ"));
        }
    }
    let encrypted = if vault.trim().is_empty() {
        old.as_ref()
            .and_then(|p| p["vault_encrypted"].as_str())
            .map(str::to_owned)
    } else {
        Some(crypto::seal(
            &s.config.key,
            &native_plan::vault(vault)?.to_string(),
        )?)
    };
    let id:i64=sqlx::query_scalar("INSERT INTO recovery_profiles(server_id,name,source_key,object_types,vault_encrypted,enabled,interval_hours,keep_local,keep_cloud,restore_target,created_by) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$8,$9,$10) ON CONFLICT(server_id) DO UPDATE SET name=excluded.name,source_key=excluded.source_key,object_types=excluded.object_types,vault_encrypted=excluded.vault_encrypted,enabled=excluded.enabled,interval_hours=excluded.interval_hours,keep_local=excluded.keep_local,restore_target=excluded.restore_target,created_by=excluded.created_by,version=recovery_profiles.version+1,next_run_at=NULL,updated_at=now() RETURNING id").bind(server).bind(resources::text(&v,"name",190)?).bind(key).bind(json!(types)).bind(encrypted).bind(if v["enabled"]==true&&!target{1_i16}else{0}).bind(hours as i32).bind(keep as i32).bind(if target{1_i16}else{0}).bind(u.id).fetch_one(&mut *tx).await?;
    sqlx::query(
        "INSERT INTO audit_logs(user_id,action,context) VALUES($1,'recovery_profile_saved',$2)",
    )
    .bind(u.id)
    .bind(json!({"profile_id":id,"server_id":server}))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"id":id})))
}
async fn artifact(db: &mut PgConnection, id: i64) -> Result<Value> {
    sqlx::query_scalar("SELECT to_jsonb(a) FROM recovery_artifacts a WHERE id=$1 FOR SHARE")
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or_else(Error::missing)
}
async fn target(
    db: &mut PgConnection,
    source: &Value,
    target: &Value,
    config: &Value,
) -> Result<PathBuf> {
    if target["restore_target"] != 1 || source["server_id"] == target["server_id"] {
        return Err(Error::bad("Chọn server diễn tập khác"));
    }
    let t: Value = sqlx::query_scalar(
        "SELECT to_jsonb(s)-'token_encrypted' FROM stalwart_servers s WHERE id=$1 FOR SHARE",
    )
    .bind(target["server_id"].as_i64().unwrap())
    .fetch_one(&mut *db)
    .await?;
    let source_base: String =
        sqlx::query_scalar("SELECT base_url FROM stalwart_servers WHERE id=$1 FOR SHARE")
            .bind(source["server_id"].as_i64().unwrap())
            .fetch_one(&mut *db)
            .await?;
    if t["is_primary"] == 1
        || url::Url::parse(t["base_url"].as_str().unwrap())
            .map_err(|_| Error::missing())?
            .origin()
            == url::Url::parse(&source_base)
                .map_err(|_| Error::missing())?
                .origin()
    {
        return Err(Error::bad(
            "Server diễn tập không được là primary hoặc cùng origin nguồn",
        ));
    }
    let assigned:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM domains WHERE server_id=$1 AND status<>'deleted') OR EXISTS(SELECT 1 FROM email_accounts WHERE server_id=$1 AND status<>'deleted')").bind(target["server_id"].as_i64().unwrap()).fetch_one(db).await?;
    if assigned {
        return Err(Error::bad(
            "Server diễn tập đang được phân bổ tài nguyên panel",
        ));
    }
    sources::target(config).await
}
async fn valid_preview(
    db: &mut PgConnection,
    id: i64,
    actor: i64,
    artifact: i64,
    target: &Value,
    hash: &str,
) -> Result<()> {
    let ok:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM recovery_jobs WHERE id=$1 AND actor_id=$2 AND artifact_id=$3 AND target_profile_id=$4 AND target_version=$5 AND target_config_hash=$6 AND kind='restore_preview' AND status='done' AND finished_at>now()-interval '15 minutes')").bind(id).bind(actor).bind(artifact).bind(target["id"].as_i64().unwrap()).bind(target["version"].as_i64().unwrap()).bind(hash).fetch_one(db).await?;
    if !ok {
        return Err(Error::conflict(
            "Bản xem trước đã cũ hoặc không khớp thao tác",
        ));
    }
    Ok(())
}
pub async fn queue(
    State(s): State<App>,
    h: HeaderMap,
    Json(v): Json<Value>,
) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "operations", true, true).await?;
    u.super_admin()?;
    let id = resources::number(&v, "profile_id")?;
    let kind = resources::text(&v, "kind", 20)?;
    if !["backup", "upload", "retrieve", "restore_preview", "restore"].contains(&kind) {
        return Err(Error::bad("Tác vụ recovery không hỗ trợ"));
    }
    let target_id = v["target_profile_id"].as_i64();
    let artifact_id = v["artifact_id"].as_i64();
    let preview = v["preview_id"].as_i64();
    let mut tx = s.db.begin().await?;
    let mut locks = vec![id];
    if let Some(id) = target_id {
        locks.push(id);
    }
    locks.sort();
    locks.dedup();
    for id in locks {
        sqlx::query("SELECT id FROM recovery_profiles WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(Error::missing)?;
        let busy:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM recovery_jobs WHERE (profile_id=$1 OR target_profile_id=$1) AND status IN ('pending','running'))").bind(id).fetch_one(&mut *tx).await?;
        if busy {
            return Err(Error::conflict("Hồ sơ đang có tác vụ"));
        }
    }
    let p = profile(&mut tx, id).await?;
    let (_, _, source_hash) = context(&mut tx, &p).await?;
    let mut target_version = None;
    let mut target_hash = None;
    if kind != "backup" {
        let a = artifact(&mut tx, artifact_id.ok_or_else(Error::missing)?).await?;
        if a["profile_id"] != id {
            return Err(Error::missing());
        }
        if kind != "retrieve" {
            verified(&s, &a).await?;
        }
    }
    if kind.starts_with("restore") {
        let t = profile(&mut tx, target_id.ok_or_else(Error::missing)?).await?;
        let (_, config, hash) = context(&mut tx, &t).await?;
        target(&mut tx, &p, &t, &config).await?;
        if kind == "restore" {
            if v["confirmation"] != "RESTORE_SERVER" {
                return Err(Error::bad("Nhập RESTORE_SERVER để xác nhận diễn tập"));
            }
            valid_preview(
                &mut tx,
                preview.ok_or_else(Error::missing)?,
                u.id,
                artifact_id.unwrap(),
                &t,
                &hash,
            )
            .await?;
        }
        target_version = t["version"].as_i64();
        target_hash = Some(hash);
    }
    let id:i64=sqlx::query_scalar("INSERT INTO recovery_jobs(profile_id,target_profile_id,artifact_id,preview_id,actor_id,kind,profile_version,target_version,source_config_hash,target_config_hash) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) RETURNING id").bind(id).bind(target_id).bind(artifact_id).bind(preview).bind(u.id).bind(kind).bind(p["version"].as_i64().unwrap()).bind(target_version).bind(source_hash).bind(target_hash).fetch_one(&mut *tx).await?;
    sqlx::query(
        "INSERT INTO audit_logs(user_id,action,context) VALUES($1,'recovery_job_queued',$2)",
    )
    .bind(u.id)
    .bind(json!({"job_id":id,"kind":kind}))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"job_id":id})))
}
async fn cli(s: &App, server: &Value, args: &[String], work: &FilePath) -> Result<String> {
    let token = crypto::open(
        &s.config.key,
        server["token_encrypted"]
            .as_str()
            .ok_or_else(|| Error::bad("Thiếu token Stalwart"))?,
    )?;
    let mut env = vec![
        ("STALWART_URL", server["base_url"].as_str().unwrap().into()),
        ("STALWART_TOKEN", String::new()),
        ("STALWART_USER", String::new()),
        ("STALWART_PASSWORD", String::new()),
        ("NO_COLOR", "1".into()),
    ];
    let token = if let Some(raw) = token.strip_prefix("basic:") {
        String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(raw.trim())
                .map_err(|_| Error::bad("Basic credential không hợp lệ"))?,
        )
        .map_err(|_| Error::bad("Basic credential không hợp lệ"))?
    } else {
        token
    };
    if token.contains(':') && !token.starts_with("API_") {
        let (user, password) = token.split_once(':').unwrap();
        env[2].1 = user.into();
        env[3].1 = password.into();
    } else {
        env[1].1 = token;
    }
    files::command_with_limit(
        &std::env::var("RECOVERY_STALWART_CLI_BIN").unwrap_or("stalwart-cli".into()),
        args,
        &env,
        work,
        if args.first().is_some_and(|v| v == "snapshot") {
            64 * 1024 * 1024
        } else {
            1024 * 1024
        },
    )
    .await
}
async fn capture(
    s: &App,
    p: &Value,
    job: &Value,
    server: &Value,
    config: &Value,
    work: &FilePath,
) -> Result<Value> {
    sources::adapter(config, "snapshot", work, None, None).await?;
    let marker = sources::marker(config).await?;
    let inventory = sources::inventory(config).await?;
    let types: Vec<String> = serde_json::from_value(p["object_types"].clone())
        .map_err(|_| Error::bad("Object types không hợp lệ"))?;
    let raw = if let Some(path) = config["config_plan_path"].as_str() {
        let path = sources::path(path).await?;
        let bytes = files::read(&path, 64 * 1024 * 1024).await?;
        if marker["config_sha256"] != files::hash(&bytes) {
            return Err(Error::bad("Plan cấu hình không khớp marker snapshot"));
        }
        String::from_utf8(bytes).map_err(|_| Error::bad("Plan không phải UTF-8"))?
    } else {
        cli(
            s,
            server,
            &[
                "snapshot".into(),
                types.join(","),
                "--include-secrets".into(),
                "--quiet".into(),
            ],
            work,
        )
        .await?
    };
    let vault = if let Some(v) = p["vault_encrypted"].as_str() {
        native_plan::vault(&crypto::open(&s.config.key, v)?)?
    } else {
        json!({})
    };
    let (plan, stats) = native_plan::build(&raw, &types, &vault, false)?;
    let mut entries = vec![("configuration.ndjson".into(), plan.into_bytes())];
    let mut total = entries[0].1.len() as u64;
    for (name, path) in &inventory {
        let bytes = files::read(path, files::MAX_ARCHIVE).await?;
        total = total
            .checked_add(bytes.len() as u64)
            .filter(|n| *n <= files::MAX_ARCHIVE - 2 * 1024 * 1024)
            .ok_or_else(|| Error::bad("Recovery archive vượt 256 MB"))?;
        entries.push((name.clone(), bytes));
    }
    for ((_, path), (_, bytes)) in inventory.iter().zip(entries.iter().skip(1)) {
        if files::hash(&files::read(path, files::MAX_ARCHIVE).await?) != files::hash(bytes) {
            return Err(Error::conflict("Snapshot thay đổi trong khi sao lưu"));
        }
    }
    if sources::marker(config).await? != marker {
        return Err(Error::conflict("Marker snapshot đã thay đổi"));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut metadata = serde_json::Map::new();
    for (name, bytes) in &entries {
        metadata.insert(
            name.clone(),
            json!({"bytes":bytes.len(),"sha256":files::hash(bytes)}),
        );
    }
    let manifest = json!({"format":"ui-rust-recovery-v1","server_id":server["id"],"profile_id":p["id"],"snapshot_id":marker["snapshot_id"],"snapshot_at":marker["created_at"],"created_at":chrono::Utc::now().to_rfc3339(),"object_types":types,"configuration":stats,"consistency":if config["config_plan_path"].is_string(){"backend-and-configuration-checkpoint"}else{"backend-checkpoint-plus-live-management"},"files":metadata});
    let header = manifest.to_string().into_bytes();
    if header.len() > 2 * 1024 * 1024 {
        return Err(Error::bad("Manifest quá lớn"));
    }
    let mut data = (header.len() as u32).to_be_bytes().to_vec();
    data.extend(header);
    for (_, bytes) in entries {
        data.extend(bytes);
    }
    let encrypted = backup::encrypt(&s.config.key, &data)?;
    if encrypted.len() as u64 > files::MAX_ARCHIVE {
        return Err(Error::bad("Recovery archive vượt giới hạn"));
    }
    let name = format!(
        "recovery-{}-{}-{}.rcenc",
        p["id"],
        job["id"],
        &crypto::token()[..16]
    );
    files::write(&archive_path(s, &name)?, &encrypted).await?;
    let id:i64=sqlx::query_scalar("INSERT INTO recovery_artifacts(profile_id,job_id,filename,sha256,bytes,manifest,password_encrypted,destinations) VALUES($1,$2,$3,$4,$5,$6,'APP_KEY','[]') RETURNING id").bind(p["id"].as_i64().unwrap()).bind(job["id"].as_i64().unwrap()).bind(name).bind(files::hash(&encrypted)).bind(encrypted.len()as i64).bind(manifest).fetch_one(&s.db).await?;
    Ok(json!({"artifact_id":id,"snapshot_id":marker["snapshot_id"]}))
}
async fn verified(s: &App, a: &Value) -> Result<Vec<u8>> {
    if a["local_status"] != "ready" {
        return Err(Error::missing());
    }
    let bytes = files::read(
        &archive_path(s, a["filename"].as_str().ok_or_else(Error::missing)?)?,
        files::MAX_ARCHIVE,
    )
    .await?;
    if files::hash(&bytes) != a["sha256"] || bytes.len() as i64 != a["bytes"] {
        return Err(Error::bad("Checksum recovery không khớp"));
    }
    backup::decrypt(&s.config.key, &bytes)?;
    Ok(bytes)
}
async fn extract(
    s: &App,
    a: &Value,
    work: &FilePath,
) -> Result<std::collections::BTreeMap<String, PathBuf>> {
    let data = backup::decrypt(&s.config.key, &verified(s, a).await?)?;
    if data.len() < 4 {
        return Err(Error::bad("Archive recovery không hợp lệ"));
    }
    let size = u32::from_be_bytes(data[..4].try_into().unwrap()) as usize;
    if size > 2 * 1024 * 1024 || size + 4 > data.len() {
        return Err(Error::bad("Manifest recovery không hợp lệ"));
    }
    let manifest: Value = serde_json::from_slice(&data[4..4 + size])
        .map_err(|_| Error::bad("Manifest recovery không hợp lệ"))?;
    if manifest != a["manifest"] || manifest["format"] != "ui-rust-recovery-v1" {
        return Err(Error::bad("Manifest recovery không khớp"));
    }
    let mut offset = 4 + size;
    let mut output = std::collections::BTreeMap::new();
    for (name, meta) in manifest["files"]
        .as_object()
        .filter(|o| o.len() <= 20001)
        .ok_or_else(|| Error::bad("Manifest quá nhiều tệp"))?
    {
        if !sources::entry(name) {
            return Err(Error::bad("Manifest chứa đường dẫn không hợp lệ"));
        }
        let length = meta["bytes"]
            .as_u64()
            .filter(|n| *n <= files::MAX_ARCHIVE)
            .ok_or_else(|| Error::bad("Kích thước entry không hợp lệ"))?
            as usize;
        let end = offset
            .checked_add(length)
            .filter(|n| *n <= data.len())
            .ok_or_else(|| Error::bad("Kích thước recovery không khớp"))?;
        let bytes = &data[offset..end];
        if files::hash(bytes) != meta["sha256"] {
            return Err(Error::bad("Checksum entry không khớp"));
        }
        let path = work.join("decoded").join(name);
        files::private_dir(path.parent().unwrap()).await?;
        files::write(&path, bytes).await?;
        output.insert(name.clone(), path);
        offset = end;
    }
    if offset != data.len() {
        return Err(Error::bad("Archive có dữ liệu ngoài manifest"));
    }
    Ok(output)
}
async fn empty_lab(s: &App, server: i64, config: &Value) -> Result<()> {
    let metadata = stalwart::metadata(s, server, "/api/account").await?;
    for permission in ["sysDomainQuery", "sysAccountQuery", "sysAccountGet"] {
        if !metadata["data"]["permissions"]
            .as_array()
            .is_some_and(|a| a.iter().any(|v| v == permission))
        {
            return Err(Error::forbidden());
        }
    }
    let domains = management::method(
        s,
        server,
        "x:Domain/query",
        json!({"limit":1,"calculateTotal":true}),
    )
    .await?;
    if domains["total"] != 0 || !domains["ids"].as_array().is_some_and(Vec::is_empty) {
        return Err(Error::bad("Server diễn tập cần domain rỗng"));
    }
    let seed = config.get("seed_account_ids").cloned().unwrap_or(json!([]));
    if !seed.as_array().is_some_and(|a| {
        a.len() <= 5
            && a.iter()
                .all(|v| v.as_str().is_some_and(|s| !s.is_empty() && s.len() <= 190))
    }) {
        return Err(Error::bad("Seed account không hợp lệ"));
    }
    let accounts = management::method(
        s,
        server,
        "x:Account/query",
        json!({"limit":6,"calculateTotal":true}),
    )
    .await?;
    let ids = accounts["ids"]
        .as_array()
        .filter(|a| {
            a.len() <= 5
                && accounts["total"] == a.len()
                && a.iter().all(|id| seed.as_array().unwrap().contains(id))
        })
        .ok_or_else(|| Error::bad("Server diễn tập còn account ngoài seed"))?;
    if !ids.is_empty() {
        let accounts = management::method(
            s,
            server,
            "x:Account/get",
            json!({"ids":ids,"properties":["id","domainId"]}),
        )
        .await?;
        let rows = accounts["list"]
            .as_array()
            .ok_or_else(|| Error::bad("Không đọc được seed"))?;
        if rows.len() != ids.len()
            || ids.iter().any(|id| {
                rows.iter()
                    .filter(|r| r["id"] == *id && (r["domainId"].is_null() || r["domainId"] == ""))
                    .count()
                    != 1
            })
        {
            return Err(Error::bad("Seed account phải là admin không gắn domain"));
        }
    }
    Ok(())
}
async fn restore(
    s: &App,
    p: &Value,
    t: &Value,
    a: &Value,
    job: &Value,
    server: &Value,
    config: &Value,
    hash: &str,
    work: &FilePath,
    db: &mut PgConnection,
) -> Result<Value> {
    let root = target(db, p, t, config).await?;
    empty_lab(s, server["id"].as_i64().unwrap(), config).await?;
    let files = extract(s, a, work).await?;
    let input = std::fs::read_to_string(
        files
            .get("configuration.ndjson")
            .ok_or_else(|| Error::bad("Thiếu plan cấu hình"))?,
    )
    .map_err(|_| Error::bad("Không đọc được plan"))?;
    let types: Vec<String> = serde_json::from_value(a["manifest"]["object_types"].clone())
        .map_err(|_| Error::bad("Object types không hợp lệ"))?;
    let (plan, stats) = native_plan::build(&input, &types, &json!({}), true)?;
    let metadata = stalwart::metadata(s, server["id"].as_i64().unwrap(), "/api/account").await?;
    for ty in stats["objects"].as_object().unwrap().keys() {
        for action in ["Get", "Query", "Create", "Update"] {
            let permission = format!("sys{ty}{action}");
            if !metadata["data"]["permissions"]
                .as_array()
                .is_some_and(|a| a.iter().any(|v| v == &permission))
            {
                return Err(Error::forbidden());
            }
        }
    }
    let plan_path = work.join("lab.ndjson");
    files::write(&plan_path, plan.as_bytes()).await?;
    cli(
        s,
        server,
        &[
            "apply".into(),
            "--file".into(),
            plan_path.to_string_lossy().into(),
            "--dry-run".into(),
            "--quiet".into(),
        ],
        work,
    )
    .await?;
    let dry = job["kind"] == "restore_preview";
    let snapshot =
        chrono::DateTime::parse_from_rfc3339(a["manifest"]["snapshot_at"].as_str().unwrap_or(""))
            .map_err(|_| Error::bad("Thời gian snapshot không hợp lệ"))?;
    let mut result = json!({"artifact_id":a["id"],"snapshot_id":a["manifest"]["snapshot_id"],"configuration":stats,"dry_run":dry,"backend_staged":false,"full_restore_verified":false,"rpo_seconds":chrono::Utc::now().signed_duration_since(snapshot).num_seconds().max(0)});
    if dry {
        return Ok(result);
    }
    valid_preview(
        db,
        job["preview_id"].as_i64().unwrap(),
        job["actor_id"].as_i64().unwrap(),
        a["id"].as_i64().unwrap(),
        t,
        hash,
    )
    .await?;
    let stage = root.join(format!("rehearsal-{}", job["id"]));
    tokio::fs::create_dir(&stage)
        .await
        .map_err(|_| Error::conflict("Thư mục diễn tập đã tồn tại hoặc không ghi được"))?;
    files::private_dir(&stage).await?;
    for (entry, path) in files {
        let bytes = files::read(&path, files::MAX_ARCHIVE).await?;
        let output = stage.join(&entry);
        files::private_dir(output.parent().unwrap()).await?;
        files::write(&output, &bytes).await?;
    }
    files::write(
        &stage.join("manifest.json"),
        a["manifest"].to_string().as_bytes(),
    )
    .await?;
    result["backend_staged"] = json!(true);
    result["stage_id"] = json!(stage.file_name().unwrap().to_string_lossy());
    if let Some(adapter) = sources::adapter(config, "restore", work, Some(&stage), Some(a)).await? {
        result
            .as_object_mut()
            .unwrap()
            .extend(adapter.as_object().unwrap().clone());
    }
    cli(
        s,
        server,
        &[
            "apply".into(),
            "--file".into(),
            plan_path.to_string_lossy().into(),
            "--quiet".into(),
        ],
        work,
    )
    .await?;
    result["configuration_applied"] = json!(true);
    let started = chrono::DateTime::parse_from_rfc3339(job["started_at"].as_str().unwrap_or(""))
        .map_err(|_| Error::bad("Thời gian tác vụ không hợp lệ"))?;
    result["rto_seconds"] = json!(chrono::Utc::now()
        .signed_duration_since(started)
        .num_seconds()
        .max(0));
    Ok(result)
}
async fn run(s: &App, job: &Value, work: &FilePath) -> Result<Value> {
    let mut tx = s.db.begin().await?;
    let p = profile(&mut tx, job["profile_id"].as_i64().unwrap()).await?;
    let (server, config, hash) = context(&mut tx, &p).await?;
    if p["version"] != job["profile_version"] || hash != job["source_config_hash"] {
        return Err(Error::conflict("Hồ sơ hoặc nguồn recovery đã thay đổi"));
    }
    let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id=$1 AND role='admin' AND admin_level='super' AND status='active')").bind(job["actor_id"].as_i64().unwrap()).fetch_one(&mut *tx).await?;
    if !valid {
        return Err(Error::forbidden());
    }
    if job["kind"] == "backup" {
        let mut result = capture(s, &p, job, &server, &config, work).await?;
        tx.commit().await?;
        let mut db = s.db.acquire().await?;
        let a = artifact(&mut db, result["artifact_id"].as_i64().unwrap()).await?;
        result["cloud_failed"] = json!(if upload(s, &a, work).await? { 0 } else { 1 });
        return Ok(result);
    }
    let a = artifact(&mut tx, job["artifact_id"].as_i64().unwrap()).await?;
    if a["profile_id"] != p["id"] {
        return Err(Error::missing());
    }
    if job["kind"] == "upload" {
        tx.commit().await?;
        return Ok(
            json!({"artifact_id":a["id"],"cloud_failed":if upload(s,&a,work).await?{0}else{1}}),
        );
    }
    if job["kind"] == "retrieve" {
        tx.commit().await?;
        retrieve(s, &a, work).await?;
        return Ok(json!({"artifact_id":a["id"],"retrieved":true}));
    }
    let t = profile(
        &mut tx,
        job["target_profile_id"]
            .as_i64()
            .ok_or_else(|| Error::bad("Thiếu hồ sơ đích"))?,
    )
    .await?;
    let (target, config, hash) = context(&mut tx, &t).await?;
    if t["version"] != job["target_version"] || hash != job["target_config_hash"] {
        return Err(Error::conflict("Hồ sơ đích đã thay đổi"));
    }
    let result = restore(s, &p, &t, &a, job, &target, &config, &hash, work, &mut tx).await?;
    tx.commit().await?;
    Ok(result)
}
pub async fn tick(s: &App) -> Result<()> {
    let mut lease = s.db.begin().await?;
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(88261734)")
        .fetch_one(&mut *lease)
        .await?;
    if !locked {
        return Ok(());
    }
    sqlx::query("UPDATE recovery_jobs SET status=CASE WHEN kind='restore' THEN 'uncertain' ELSE 'failed' END,error='Worker interrupted; inspect target before retry',finished_at=now() WHERE status='running'").execute(&s.db).await?;
    files::clean_orphans(&directory(s)).await?;
    let mut tx = s.db.begin().await?;
    let due=sqlx::query("SELECT * FROM recovery_profiles WHERE enabled=1 AND restore_target=0 AND (next_run_at IS NULL OR next_run_at<=now()) ORDER BY next_run_at LIMIT 2 FOR UPDATE SKIP LOCKED").fetch_all(&mut *tx).await?;
    for row in due {
        let id: i64 = row.get("id");
        let p = profile(&mut tx, id).await?;
        if let Ok((_, _, hash)) = context(&mut tx, &p).await {
            sqlx::query("INSERT INTO recovery_jobs(profile_id,actor_id,kind,profile_version,source_config_hash) SELECT $1,$2,'backup',$3,$4 WHERE NOT EXISTS(SELECT 1 FROM recovery_jobs WHERE (profile_id=$1 OR target_profile_id=$1) AND status IN ('pending','running'))").bind(id).bind(row.get::<i64,_>("created_by")).bind(row.get::<i64,_>("version")).bind(hash).execute(&mut *tx).await?;
        }
        sqlx::query(
            "UPDATE recovery_profiles SET next_run_at=now()+make_interval(hours=>$1) WHERE id=$2",
        )
        .bind(row.get::<i32, _>("interval_hours"))
        .bind(id)
        .execute(&mut *tx)
        .await?;
    }
    let job:Option<Value>=sqlx::query_scalar("UPDATE recovery_jobs SET status='running',started_at=now() WHERE id=(SELECT id FROM recovery_jobs WHERE status='pending' ORDER BY id LIMIT 1 FOR UPDATE SKIP LOCKED) RETURNING to_jsonb(recovery_jobs)").fetch_optional(&mut *tx).await?;
    tx.commit().await?;
    if let Some(job) = job {
        let work = files::work(&directory(s)).await;
        let result = match &work {
            Ok(path) => run(s, &job, path).await,
            Err(e) => Err(Error::bad(&e.1)),
        };
        let (status, result, error) = match result {
            Ok(v) => (
                if v["cloud_failed"].as_i64().unwrap_or(0) > 0 {
                    "partial"
                } else {
                    "done"
                },
                Some(v),
                None,
            ),
            Err(e) => (
                if job["kind"] == "restore" {
                    "uncertain"
                } else {
                    "failed"
                },
                None,
                Some(e.1),
            ),
        };
        sqlx::query(
            "UPDATE recovery_jobs SET status=$1,result=$2,error=$3,finished_at=now() WHERE id=$4",
        )
        .bind(status)
        .bind(result)
        .bind(error)
        .bind(job["id"].as_i64().unwrap())
        .execute(&s.db)
        .await?;
        if let Ok(path) = work {
            tokio::fs::remove_dir_all(path)
                .await
                .map_err(|_| Error::bad("Không dọn được recovery tạm"))?;
        }
        auth::audit(
            s,
            job["actor_id"].as_i64(),
            "recovery_job_finished",
            json!({"job_id":job["id"],"status":status}),
        )
        .await?;
    }
    retain(s).await?;
    lease.commit().await?;
    Ok(())
}
pub async fn download(State(s): State<App>, h: HeaderMap, Path(id): Path<i64>) -> Result<Response> {
    let u = auth::require(&s, &h, "operations", true, false).await?;
    u.super_admin()?;
    let mut db = s.db.acquire().await?;
    let a = artifact(&mut db, id).await?;
    let bytes = verified(&s, &a).await?;
    auth::audit(
        &s,
        Some(u.id),
        "recovery_downloaded",
        json!({"artifact_id":id}),
    )
    .await?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=recovery.rcenc",
            ),
        ],
        bytes,
    )
        .into_response())
}
async fn upload(s: &App, a: &Value, work: &FilePath) -> Result<bool> {
    let destination: String = sqlx::query_scalar("SELECT destination FROM rust_backup_policy")
        .fetch_one(&s.db)
        .await?;
    if destination.is_empty() {
        return Ok(true);
    }
    verified(s, a).await?;
    let remote = format!(
        "{}/recovery/{}/{}",
        destination.trim_end_matches('/'),
        a["profile_id"],
        a["filename"].as_str().unwrap()
    );
    let path = archive_path(s, a["filename"].as_str().unwrap())?;
    let ok = files::command(
        "rclone",
        &[
            "copyto".into(),
            path.to_string_lossy().into(),
            remote.clone(),
            "--retries".into(),
            "1".into(),
            "--timeout".into(),
            "30s".into(),
        ],
        &[],
        work,
    )
    .await
    .is_ok();
    sqlx::query("UPDATE recovery_artifacts SET destinations=$1 WHERE id=$2").bind(json!([{"destination":destination,"remote":remote,"status":if ok{"uploaded"}else{"failed"}}])).bind(a["id"].as_i64().unwrap()).execute(&s.db).await?;
    Ok(ok)
}
async fn retrieve(s: &App, a: &Value, work: &FilePath) -> Result<()> {
    let destination: String = sqlx::query_scalar("SELECT destination FROM rust_backup_policy")
        .fetch_one(&s.db)
        .await?;
    let expected = format!(
        "{}/recovery/{}/{}",
        destination.trim_end_matches('/'),
        a["profile_id"],
        a["filename"].as_str().unwrap()
    );
    if destination.is_empty()
        || !a["destinations"].as_array().is_some_and(|rows| {
            rows.iter().any(|r| {
                r["destination"] == destination
                    && r["remote"] == expected
                    && r["status"] == "uploaded"
            })
        })
    {
        return Err(Error::bad("Không có recovery cloud khớp cấu hình hiện tại"));
    }
    let path = work.join("retrieved.rcenc");
    files::command(
        "rclone",
        &[
            "copyto".into(),
            expected,
            path.to_string_lossy().into(),
            "--max-transfer".into(),
            files::MAX_ARCHIVE.to_string(),
            "--retries".into(),
            "1".into(),
            "--timeout".into(),
            "30s".into(),
        ],
        &[],
        work,
    )
    .await?;
    let bytes = files::read(&path, files::MAX_ARCHIVE).await?;
    if files::hash(&bytes) != a["sha256"] || bytes.len() as i64 != a["bytes"] {
        return Err(Error::bad("Checksum recovery cloud không khớp"));
    }
    backup::decrypt(&s.config.key, &bytes)?;
    files::write(&archive_path(s, a["filename"].as_str().unwrap())?, &bytes).await?;
    sqlx::query("UPDATE recovery_artifacts SET local_status='ready' WHERE id=$1")
        .bind(a["id"].as_i64().unwrap())
        .execute(&s.db)
        .await?;
    Ok(())
}
async fn retain(s: &App) -> Result<()> {
    let mut tx = s.db.begin().await?;
    let rows=sqlx::query("SELECT a.id,a.filename,a.destinations FROM recovery_artifacts a JOIN recovery_profiles p ON p.id=a.profile_id WHERE a.local_status='ready' AND (SELECT count(*) FROM recovery_artifacts newer WHERE newer.profile_id=a.profile_id AND newer.id>a.id)>=p.keep_local AND NOT EXISTS(SELECT 1 FROM recovery_jobs j WHERE j.artifact_id=a.id AND j.status IN ('pending','running')) FOR UPDATE OF a SKIP LOCKED").fetch_all(&mut *tx).await?;
    for a in rows {
        let destinations: Value = a.get("destinations");
        if destinations
            .as_array()
            .is_some_and(|r| r.iter().any(|r| r["status"] == "failed"))
        {
            continue;
        }
        let path = archive_path(s, &a.get::<String, _>("filename"))?;
        match tokio::fs::remove_file(path).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(Error::bad("Không xóa được recovery hết hạn")),
        };
        sqlx::query("UPDATE recovery_artifacts SET local_status='pruned' WHERE id=$1")
            .bind(a.get::<i64, _>("id"))
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}
pub async fn attest(
    State(s): State<App>,
    h: HeaderMap,
    Path(id): Path<i64>,
    Json(v): Json<Value>,
) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "operations", true, true).await?;
    u.super_admin()?;
    for key in [
        "backend_loaded",
        "credentials_checked",
        "mail_folders_checked",
        "attachments_checked",
        "queue_checked",
        "outbound_isolated",
    ] {
        if v[key] != true {
            return Err(Error::bad("Hoàn tất toàn bộ kiểm chứng phục hồi"));
        }
    }
    let note = resources::text(&v, "note", 2000)?;
    if note.chars().count() < 20 {
        return Err(Error::bad(
            "Ghi rõ bằng chứng và cách kiểm tra; tối thiểu 20 ký tự",
        ));
    }
    let mut tx = s.db.begin().await?;
    let job:Value=sqlx::query_scalar("SELECT to_jsonb(j) FROM recovery_jobs j WHERE id=$1 AND actor_id=$2 AND kind='restore' AND status='done' FOR UPDATE").bind(id).bind(u.id).fetch_optional(&mut *tx).await?.ok_or_else(Error::missing)?;
    let p = profile(&mut tx, job["target_profile_id"].as_i64().unwrap()).await?;
    let (_, _, hash) = context(&mut tx, &p).await?;
    if p["version"] != job["target_version"] || hash != job["target_config_hash"] {
        return Err(Error::conflict("Cấu hình đích diễn tập đã đổi"));
    }
    let mut result = job["result"].clone();
    if !result["attested_at"].is_null() {
        return Err(Error::conflict("Diễn tập đã được xác nhận"));
    }
    if result["backend_staged"] != true {
        return Err(Error::conflict("Backend chưa được stage"));
    }
    result["admin_verified"] = json!(true);
    result["attested_at"] = json!(chrono::Utc::now().to_rfc3339());
    result["verification_note"] = json!(note);
    let started = chrono::DateTime::parse_from_rfc3339(job["started_at"].as_str().unwrap_or(""))
        .map_err(|_| Error::missing())?;
    result["rto_attested_seconds"] = json!(chrono::Utc::now()
        .signed_duration_since(started)
        .num_seconds()
        .max(0));
    sqlx::query("UPDATE recovery_jobs SET result=$1 WHERE id=$2")
        .bind(result)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO audit_logs(user_id,action,context) VALUES($1,'recovery_rehearsal_attested',$2)").bind(u.id).bind(json!({"job_id":id})).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(json!({"ok":true,"admin_verified":true})))
}
