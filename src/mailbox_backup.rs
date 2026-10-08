use crate::{
    auth, backup, backup_admin, backup_tools as files, crypto,
    error::{Error, Result},
    resources, App,
};
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use sqlx::{PgConnection, Row};
use std::path::PathBuf;
fn directory(s: &App) -> PathBuf {
    backup_admin::directory(s).join("mailboxes")
}
fn archive_path(s: &App, name: &str) -> Result<PathBuf> {
    if !regex::Regex::new(r"^mailbox-[0-9]+-[0-9]+-[a-f0-9]{16}\.mbenc$")
        .unwrap()
        .is_match(name)
    {
        return Err(Error::bad("Tên archive không hợp lệ"));
    }
    Ok(directory(s).join(name))
}
async fn profile(db: &mut PgConnection, id: i64) -> Result<Value> {
    sqlx::query_scalar("SELECT to_jsonb(p) FROM mailbox_backup_profiles p WHERE id=$1 FOR SHARE")
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or_else(Error::missing)
}
async fn guard(db: &mut PgConnection, p: &Value) -> Result<()> {
    let r=sqlx::query("SELECT e.status,e.email,e.stalwart_account_id,e.server_id,s.base_url,s.config_version,s.active,s.dry_run FROM email_accounts e JOIN stalwart_servers s ON s.id=e.server_id WHERE e.id=$1 FOR SHARE OF e,s").bind(p["account_id"].as_i64().ok_or_else(Error::missing)?).fetch_optional(db).await?.ok_or_else(Error::missing)?;
    let base = url::Url::parse(&r.get::<String, _>("base_url"))
        .map_err(|_| Error::bad("URL server không hợp lệ"))?;
    let jmap = url::Url::parse(p["jmap_url"].as_str().unwrap_or(""))
        .map_err(|_| Error::bad("URL JMAP không hợp lệ"))?;
    if r.get::<String, _>("status") != "active"
        || r.get::<String, _>("email") != p["username"]
        || r.get::<Option<i64>, _>("server_id") != p["server_id"].as_i64()
        || r.get::<Option<String>, _>("stalwart_account_id").as_deref()
            != p["account_remote_id"].as_str()
        || r.get::<i64, _>("config_version") != p["server_version"].as_i64().unwrap_or(-1)
        || r.get::<i16, _>("active") != 1
        || r.get::<i16, _>("dry_run") != 0
        || base.origin() != jmap.origin()
    {
        return Err(Error::conflict(
            "Liên kết hoặc cấu hình hộp thư/server đã thay đổi",
        ));
    }
    Ok(())
}
pub async fn list(State(s): State<App>, h: HeaderMap) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "operations", true, false).await?;
    u.super_admin()?;
    let profiles:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(p)-'password_encrypted' FROM mailbox_backup_profiles p ORDER BY id DESC LIMIT 200").fetch_all(&s.db).await?;
    let artifacts:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(a)-'password_encrypted' FROM mailbox_backup_artifacts a ORDER BY id DESC LIMIT 200").fetch_all(&s.db).await?;
    let jobs: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(j) FROM mailbox_backup_jobs j ORDER BY id DESC LIMIT 100",
    )
    .fetch_all(&s.db)
    .await?;
    Ok(Json(
        json!({"profiles":profiles,"artifacts":artifacts,"jobs":jobs}),
    ))
}
pub async fn save(State(s): State<App>, h: HeaderMap, Json(v): Json<Value>) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "operations", true, true).await?;
    u.super_admin()?;
    let account = resources::number(&v, "account_id")?;
    let url = resources::text(&v, "jmap_url", 500)?;
    let url = url::Url::parse(url).map_err(|_| Error::bad("URL JMAP không hợp lệ"))?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::bad("JMAP cần HTTPS và cùng origin với server"));
    }
    let hours = v["interval_hours"]
        .as_i64()
        .filter(|n| (1..=168).contains(n))
        .ok_or_else(|| Error::bad("Chu kỳ từ 1 đến 168 giờ"))?;
    let keep = v["keep_local"]
        .as_i64()
        .filter(|n| (1..=90).contains(n))
        .ok_or_else(|| Error::bad("Giữ 1 đến 90 bản"))?;
    let mut tx = s.db.begin().await?;
    let a=sqlx::query("SELECT e.email,e.server_id,e.stalwart_account_id,s.config_version FROM email_accounts e JOIN stalwart_servers s ON s.id=e.server_id WHERE e.id=$1 AND e.status='active' AND e.sync_status='synced' AND s.active=1 AND s.dry_run=0 FOR SHARE OF e,s").bind(account).fetch_optional(&mut *tx).await?.ok_or_else(Error::missing)?;
    let server: i64 = a.get("server_id");
    let existing: Option<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(p) FROM mailbox_backup_profiles p WHERE account_id=$1 FOR UPDATE",
    )
    .bind(account)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(p) = &existing {
        if p["config_version"] != v["config_version"] {
            return Err(Error::conflict("Cấu hình đã thay đổi; tải lại trang"));
        }
        let busy:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM mailbox_backup_jobs WHERE (profile_id=$1 OR target_profile_id=$1) AND status IN ('pending','running'))").bind(p["id"].as_i64().unwrap()).fetch_one(&mut *tx).await?;
        if busy {
            return Err(Error::conflict("Hồ sơ đang có tác vụ"));
        }
    }
    let password = v["app_password"].as_str().unwrap_or("");
    if password.len() > 4096 {
        return Err(Error::bad("Credential quá dài"));
    }
    let encrypted = if password.is_empty() {
        existing
            .as_ref()
            .and_then(|p| p["password_encrypted"].as_str())
            .map(str::to_owned)
            .ok_or_else(|| Error::bad("Cần mật khẩu ứng dụng"))?
    } else {
        crypto::seal(&s.config.key, password)?
    };
    let id:i64=sqlx::query_scalar("INSERT INTO mailbox_backup_profiles(account_id,server_id,jmap_url,username,password_encrypted,enabled,restore_target,interval_hours,keep_local,keep_cloud,next_run_at,server_version,account_remote_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$9,now(),$10,$11) ON CONFLICT(account_id) DO UPDATE SET server_id=excluded.server_id,jmap_url=excluded.jmap_url,username=excluded.username,password_encrypted=excluded.password_encrypted,enabled=excluded.enabled,restore_target=excluded.restore_target,interval_hours=excluded.interval_hours,keep_local=excluded.keep_local,config_version=mailbox_backup_profiles.config_version+1,server_version=excluded.server_version,account_remote_id=excluded.account_remote_id,next_run_at=now(),updated_at=now() RETURNING id").bind(account).bind(server).bind(url.to_string()).bind(a.get::<String,_>("email")).bind(encrypted).bind(if v["enabled"]==true{1_i16}else{0}).bind(if v["restore_target"]==true{1_i16}else{0}).bind(hours as i32).bind(keep as i32).bind(a.get::<i64,_>("config_version")).bind(a.get::<Option<String>,_>("stalwart_account_id")).fetch_one(&mut *tx).await?;
    let p = profile(&mut tx, id).await?;
    guard(&mut tx, &p).await?;
    sqlx::query(
        "INSERT INTO audit_logs(user_id,action,context) VALUES($1,'mailbox_profile_saved',$2)",
    )
    .bind(u.id)
    .bind(json!({"profile_id":id,"account_id":account}))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"id":id})))
}
async fn artifact(db: &mut PgConnection, id: i64) -> Result<Value> {
    sqlx::query_scalar("SELECT to_jsonb(a) FROM mailbox_backup_artifacts a WHERE id=$1 FOR SHARE")
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or_else(Error::missing)
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
        return Err(Error::bad("Checksum archive không khớp"));
    }
    backup::decrypt(&s.config.key, &bytes)?;
    Ok(bytes)
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
        return Err(Error::bad("Tác vụ không hỗ trợ"));
    }
    let target = v["target_profile_id"].as_i64();
    let mut tx = s.db.begin().await?;
    let mut locks = vec![id];
    if let Some(target) = target {
        locks.push(target);
    }
    locks.sort();
    locks.dedup();
    for lock in locks {
        sqlx::query("SELECT id FROM mailbox_backup_profiles WHERE id=$1 FOR UPDATE")
            .bind(lock)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(Error::missing)?;
        let busy:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM mailbox_backup_jobs WHERE (profile_id=$1 OR target_profile_id=$1) AND status IN ('pending','running'))").bind(lock).fetch_one(&mut *tx).await?;
        if busy {
            return Err(Error::conflict("Hồ sơ đang có tác vụ chờ hoặc đang chạy"));
        }
    }
    let p = profile(&mut tx, id).await?;
    guard(&mut tx, &p).await?;
    let mut version = None;
    let artifact_id = v["artifact_id"].as_i64();
    let preview = v["preview_id"].as_i64();
    if kind != "backup" {
        let a = artifact(&mut tx, artifact_id.ok_or_else(Error::missing)?).await?;
        if a["profile_id"] != id {
            return Err(Error::missing());
        }
        if kind != "retrieve" {
            verified(&s, &a).await?;
        }
        if kind.starts_with("restore") {
            let t = profile(&mut tx, target.ok_or_else(Error::missing)?).await?;
            guard(&mut tx, &t).await?;
            if t["restore_target"] != 1 || t["account_id"] == p["account_id"] {
                return Err(Error::bad("Chọn hộp thư thử nghiệm khác"));
            }
            version = t["config_version"].as_i64();
            if kind == "restore" {
                if v["confirmation"] != "RESTORE" {
                    return Err(Error::bad("Nhập RESTORE để xác nhận"));
                }
                valid_preview(
                    &mut tx,
                    preview.ok_or_else(Error::missing)?,
                    u.id,
                    artifact_id.unwrap(),
                    target.unwrap(),
                    version.unwrap(),
                )
                .await?;
            }
        }
    }
    let job:i64=sqlx::query_scalar("INSERT INTO mailbox_backup_jobs(profile_id,profile_version,actor_id,kind,source_artifact_id,target_profile_id,target_version,preview_id,run_after) VALUES($1,$2,$3,$4,$5,$6,$7,$8,now()) RETURNING id").bind(id).bind(p["config_version"].as_i64().unwrap()as i32).bind(u.id).bind(kind).bind(artifact_id).bind(target).bind(version.map(|v|v as i32)).bind(preview).fetch_one(&mut *tx).await?;
    sqlx::query(
        "INSERT INTO audit_logs(user_id,action,context) VALUES($1,'mailbox_job_queued',$2)",
    )
    .bind(u.id)
    .bind(json!({"job_id":job,"kind":kind}))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"job_id":job})))
}
async fn valid_preview(
    db: &mut PgConnection,
    id: i64,
    actor: i64,
    artifact: i64,
    target: i64,
    version: i64,
) -> Result<()> {
    let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM mailbox_backup_jobs WHERE id=$1 AND actor_id=$2 AND source_artifact_id=$3 AND target_profile_id=$4 AND target_version=$5 AND kind='restore_preview' AND status='done' AND finished_at>now()-interval '15 minutes')").bind(id).bind(actor).bind(artifact).bind(target).bind(version as i32).fetch_one(db).await?;
    if !valid {
        return Err(Error::conflict(
            "Bản xem trước đã cũ hoặc không thuộc thao tác này",
        ));
    }
    Ok(())
}
async fn tool(
    s: &App,
    p: &Value,
    kind: &str,
    path: &std::path::Path,
    work: &std::path::Path,
    dry: bool,
) -> Result<Value> {
    let mut args = vec![kind.into()];
    if kind == "import" {
        args.push("jmap".into());
    }
    args.extend([
        "--url".into(),
        p["jmap_url"].as_str().unwrap().into(),
        "--auth-basic".into(),
        p["username"].as_str().unwrap().into(),
        "--account-name".into(),
        p["username"].as_str().unwrap().into(),
    ]);
    if dry {
        args.push("--dry-run".into());
    }
    args.push(path.to_string_lossy().into());
    let output = files::command(
        &std::env::var("MAILBOX_VANDELAY_BIN").unwrap_or("vandelay".into()),
        &args,
        &[(
            "VANDELAY_PASSWORD",
            crypto::open(&s.config.key, p["password_encrypted"].as_str().unwrap())?,
        )],
        work,
    )
    .await?;
    let mut counts = serde_json::Map::new();
    for c in regex::Regex::new(
        r"(?i)\b(mailboxes|messages|contacts|calendars|events|files)\s*[:=]\s*(\d+)",
    )
    .unwrap()
    .captures_iter(&output)
    {
        if let Ok(n) = c[2].parse::<u64>() {
            counts.insert(c[1].to_ascii_lowercase(), json!(n));
        }
    }
    Ok(json!(counts))
}
async fn run(s: &App, job: &Value, work: &std::path::Path) -> Result<Value> {
    let mut tx = s.db.begin().await?;
    let p = profile(&mut tx, job["profile_id"].as_i64().unwrap()).await?;
    guard(&mut tx, &p).await?;
    if p["config_version"] != job["profile_version"] {
        return Err(Error::conflict("Hồ sơ đã thay đổi"));
    }
    if let Some(actor) = job["actor_id"].as_i64() {
        let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id=$1 AND role='admin' AND admin_level='super' AND status='active')").bind(actor).fetch_one(&mut *tx).await?;
        if !valid {
            return Err(Error::forbidden());
        }
    }
    if job["kind"] == "backup" {
        let path = work.join("archive.sqlite");
        let counts = tool(s, &p, "import", &path, work, false).await?;
        files::sqlite(&path).await?;
        let bytes = files::read(&path, files::MAX_ARCHIVE - 65536).await?;
        let manifest = json!({"format":"ui-rust-mailbox-v1","account_id":p["account_id"],"email":p["username"],"server_id":p["server_id"],"sha256":files::hash(&bytes),"bytes":bytes.len(),"created_at":chrono::Utc::now().to_rfc3339(),"scope":"account contents"});
        let header = manifest.to_string().into_bytes();
        let mut payload = (header.len() as u32).to_be_bytes().to_vec();
        payload.extend(header);
        payload.extend(bytes);
        let encrypted = backup::encrypt(&s.config.key, &payload)?;
        let name = format!(
            "mailbox-{}-{}-{}.mbenc",
            p["id"],
            job["id"],
            &crypto::token()[..16]
        );
        files::write(&archive_path(s, &name)?, &encrypted).await?;
        let id:i64=sqlx::query_scalar("INSERT INTO mailbox_backup_artifacts(profile_id,job_id,filename,sha256,bytes,manifest,password_encrypted,destinations) VALUES($1,$2,$3,$4,$5,$6,'APP_KEY','[]') RETURNING id").bind(p["id"].as_i64().unwrap()).bind(job["id"].as_i64().unwrap()).bind(name).bind(files::hash(&encrypted)).bind(encrypted.len()as i64).bind(manifest).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        let mut db = s.db.acquire().await?;
        let a = artifact(&mut db, id).await?;
        let cloud = upload(s, &a, work).await?;
        return Ok(json!({"artifact_id":id,"counts":counts,"cloud_failed":if cloud{0}else{1}}));
    }
    let a = artifact(&mut tx, job["source_artifact_id"].as_i64().unwrap()).await?;
    if a["profile_id"] != p["id"] {
        return Err(Error::missing());
    }
    if job["kind"] == "upload" {
        tx.commit().await?;
        let ok = upload(s, &a, work).await?;
        return Ok(json!({"artifact_id":a["id"],"cloud_failed":if ok{0}else{1}}));
    }
    if job["kind"] == "retrieve" {
        tx.commit().await?;
        retrieve(s, &a, work).await?;
        return Ok(json!({"artifact_id":a["id"],"retrieved":true}));
    }
    let t = profile(&mut tx, job["target_profile_id"].as_i64().unwrap()).await?;
    guard(&mut tx, &t).await?;
    if t["restore_target"] != 1
        || t["account_id"] == p["account_id"]
        || t["config_version"] != job["target_version"]
    {
        return Err(Error::conflict("Hồ sơ phục hồi đã thay đổi"));
    }
    let dry = job["kind"] == "restore_preview";
    if !dry {
        valid_preview(
            &mut tx,
            job["preview_id"].as_i64().unwrap(),
            job["actor_id"].as_i64().unwrap(),
            a["id"].as_i64().unwrap(),
            t["id"].as_i64().unwrap(),
            t["config_version"].as_i64().unwrap(),
        )
        .await?;
    }
    let payload = backup::decrypt(&s.config.key, &verified(s, &a).await?)?;
    if payload.len() < 4 {
        return Err(Error::bad("Archive không hợp lệ"));
    }
    let len = u32::from_be_bytes(payload[..4].try_into().unwrap()) as usize;
    if len > 65536 || payload.len() < len + 4 {
        return Err(Error::bad("Manifest không hợp lệ"));
    }
    let manifest: Value = serde_json::from_slice(&payload[4..4 + len])
        .map_err(|_| Error::bad("Manifest không hợp lệ"))?;
    let bytes = &payload[4 + len..];
    if manifest != a["manifest"]
        || manifest["format"] != "ui-rust-mailbox-v1"
        || manifest["sha256"] != files::hash(bytes)
        || manifest["bytes"] != bytes.len()
    {
        return Err(Error::bad("Checksum/manifest archive không khớp"));
    }
    let path = work.join("restore.sqlite");
    files::write(&path, bytes).await?;
    files::sqlite(&path).await?;
    let counts = tool(s, &t, "export", &path, work, dry).await?;
    tx.commit().await?;
    Ok(
        json!({"artifact_id":a["id"],"target_email":t["username"],"dry_run":dry,"mode":"additive","counts":counts}),
    )
}
pub async fn tick(s: &App) -> Result<()> {
    let mut lease = s.db.begin().await?;
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(88261733)")
        .fetch_one(&mut *lease)
        .await?;
    if !locked {
        return Ok(());
    }
    sqlx::query("UPDATE mailbox_backup_jobs SET status=CASE WHEN kind='restore' THEN 'uncertain' ELSE 'failed' END,error='Worker interrupted; inspect target before retry',finished_at=now() WHERE status='running'").execute(&s.db).await?;
    files::clean_orphans(&directory(s)).await?;
    let mut tx = s.db.begin().await?;
    let due=sqlx::query("SELECT id,config_version,interval_hours FROM mailbox_backup_profiles WHERE enabled=1 AND restore_target=0 AND (next_run_at IS NULL OR next_run_at<=now()) ORDER BY next_run_at LIMIT 2 FOR UPDATE SKIP LOCKED").fetch_all(&mut *tx).await?;
    for p in due {
        let id: i64 = p.get("id");
        sqlx::query("INSERT INTO mailbox_backup_jobs(profile_id,profile_version,kind,run_after) SELECT $1,$2,'backup',now() WHERE NOT EXISTS(SELECT 1 FROM mailbox_backup_jobs WHERE (profile_id=$1 OR target_profile_id=$1) AND status IN ('pending','running'))").bind(id).bind(p.get::<i32,_>("config_version")).execute(&mut *tx).await?;
        sqlx::query("UPDATE mailbox_backup_profiles SET next_run_at=now()+make_interval(hours=>$1) WHERE id=$2").bind(p.get::<i32,_>("interval_hours")).bind(id).execute(&mut *tx).await?;
    }
    let job:Option<Value>=sqlx::query_scalar("UPDATE mailbox_backup_jobs SET status='running',started_at=now() WHERE id=(SELECT id FROM mailbox_backup_jobs WHERE status='pending' AND run_after<=now() ORDER BY id LIMIT 1 FOR UPDATE SKIP LOCKED) RETURNING to_jsonb(mailbox_backup_jobs)").fetch_optional(&mut *tx).await?;
    tx.commit().await?;
    if let Some(job) = job {
        let work = files::work(&directory(s)).await;
        let result = match &work {
            Ok(path) => run(s, &job, path).await,
            Err(e) => Err(Error::bad(&e.1)),
        };
        let (status, value, error) = match result {
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
        sqlx::query("UPDATE mailbox_backup_jobs SET status=$1,result=$2,error=$3,finished_at=now() WHERE id=$4").bind(status).bind(value).bind(error).bind(job["id"].as_i64().unwrap()).execute(&s.db).await?;
        if let Ok(path) = work {
            tokio::fs::remove_dir_all(path)
                .await
                .map_err(|_| Error::bad("Không dọn được archive tạm"))?;
        }
        auth::audit(
            s,
            job["actor_id"].as_i64(),
            "mailbox_job_finished",
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
        "mailbox_backup_downloaded",
        json!({"artifact_id":id}),
    )
    .await?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=mailbox.mbenc",
            ),
        ],
        bytes,
    )
        .into_response())
}
async fn upload(s: &App, a: &Value, work: &std::path::Path) -> Result<bool> {
    let destination: String = sqlx::query_scalar("SELECT destination FROM rust_backup_policy")
        .fetch_one(&s.db)
        .await?;
    if destination.is_empty() {
        return Ok(true);
    }
    let name = a["filename"].as_str().ok_or_else(Error::missing)?;
    let path = archive_path(s, name)?;
    verified(s, a).await?;
    let remote = format!(
        "{}/mailboxes/{}/{}",
        destination.trim_end_matches('/'),
        a["profile_id"],
        name
    );
    let result = files::command(
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
    .await;
    let ok = result.is_ok();
    let records = json!([{"destination":destination,"remote":remote,"status":if ok{"uploaded"}else{"failed"}}]);
    sqlx::query("UPDATE mailbox_backup_artifacts SET destinations=$1 WHERE id=$2")
        .bind(records)
        .bind(a["id"].as_i64().unwrap())
        .execute(&s.db)
        .await?;
    Ok(ok)
}
async fn retrieve(s: &App, a: &Value, work: &std::path::Path) -> Result<()> {
    let destination: String = sqlx::query_scalar("SELECT destination FROM rust_backup_policy")
        .fetch_one(&s.db)
        .await?;
    let name = a["filename"].as_str().ok_or_else(Error::missing)?;
    let expected = format!(
        "{}/mailboxes/{}/{}",
        destination.trim_end_matches('/'),
        a["profile_id"],
        name
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
        return Err(Error::bad("Không có bản cloud khớp cấu hình hiện tại"));
    }
    let path = work.join("retrieved.mbenc");
    files::command(
        "rclone",
        &[
            "copyto".into(),
            expected,
            path.to_string_lossy().into(),
            "--retries".into(),
            "1".into(),
            "--max-transfer".into(),
            files::MAX_ARCHIVE.to_string(),
            "--timeout".into(),
            "30s".into(),
        ],
        &[],
        work,
    )
    .await?;
    let bytes = files::read(&path, files::MAX_ARCHIVE).await?;
    if files::hash(&bytes) != a["sha256"] || bytes.len() as i64 != a["bytes"] {
        return Err(Error::bad("Checksum cloud không khớp"));
    }
    backup::decrypt(&s.config.key, &bytes)?;
    let output = archive_path(s, name)?;
    if tokio::fs::try_exists(&output).await.unwrap_or(true) {
        return Err(Error::conflict("Archive cục bộ đã tồn tại"));
    }
    files::write(&output, &bytes).await?;
    sqlx::query("UPDATE mailbox_backup_artifacts SET local_status='ready' WHERE id=$1")
        .bind(a["id"].as_i64().unwrap())
        .execute(&s.db)
        .await?;
    Ok(())
}
async fn retain(s: &App) -> Result<()> {
    let mut tx = s.db.begin().await?;
    let rows=sqlx::query("SELECT a.id,a.filename,a.destinations FROM mailbox_backup_artifacts a JOIN mailbox_backup_profiles p ON p.id=a.profile_id WHERE a.local_status='ready' AND (SELECT count(*) FROM mailbox_backup_artifacts newer WHERE newer.profile_id=a.profile_id AND newer.id>a.id)>=p.keep_local AND NOT EXISTS(SELECT 1 FROM mailbox_backup_jobs j WHERE j.source_artifact_id=a.id AND j.status IN ('pending','running')) FOR UPDATE OF a SKIP LOCKED").fetch_all(&mut *tx).await?;
    for a in rows {
        let destinations: Option<Value> = a.get("destinations");
        if destinations.as_ref().is_some_and(|v| {
            v.as_array()
                .is_some_and(|r| r.iter().any(|r| r["status"] == "failed"))
        }) {
            continue;
        }
        let path = archive_path(s, &a.get::<String, _>("filename"))?;
        match tokio::fs::remove_file(path).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(Error::bad("Không xóa được archive hết hạn")),
        };
        sqlx::query("UPDATE mailbox_backup_artifacts SET local_status='pruned' WHERE id=$1")
            .bind(a.get::<i64, _>("id"))
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}
