use crate::{
    auth, backup,
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
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::path::PathBuf;

pub(crate) fn directory(s: &App) -> PathBuf {
    PathBuf::from(std::env::var("BACKUP_DIRECTORY").unwrap_or("backups".into())).join(
        &crate::crypto::hash(
            &url::Url::parse(&s.config.database_url)
                .map(|u| {
                    format!(
                        "{}:{}{}",
                        u.host_str().unwrap_or("localhost"),
                        u.port().unwrap_or(5432),
                        u.path()
                    )
                })
                .unwrap_or_default(),
        )[..16],
    )
}
fn filename(name: &str) -> Result<&str> {
    if !name.starts_with("ui-")
        || !name.ends_with(".pgenc")
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        return Err(Error::bad("Tên backup không hợp lệ"));
    }
    Ok(name)
}
async fn verified(s: &App, id: i64) -> Result<Vec<u8>> {
    let r=sqlx::query("SELECT filename,checksum FROM rust_backups WHERE id=$1 AND status='done' AND local_available").bind(id).fetch_optional(&s.db).await?.ok_or_else(Error::missing)?;
    let name: String = r.get("filename");
    let path = directory(s).join(filename(&name)?);
    let metadata = tokio::fs::symlink_metadata(&path)
        .await
        .map_err(|_| Error::missing())?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > 256 * 1024 * 1024
    {
        return Err(Error::bad(
            "Tệp không hợp lệ hoặc lớn hơn 256 MB; dùng CLI cho backup lớn",
        ));
    }
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|e| anyhow::anyhow!(e))?;
    if hex::encode(Sha256::digest(&bytes)) != r.get::<String, _>("checksum") {
        return Err(Error::bad("Checksum backup không khớp"));
    }
    backup::decrypt(&s.config.key, &bytes)?;
    Ok(bytes)
}
pub async fn list(State(s): State<App>, h: HeaderMap) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "settings", true, false).await?;
    u.super_admin()?;
    let policy: Value = sqlx::query_scalar("SELECT to_jsonb(p) FROM rust_backup_policy p")
        .fetch_one(&s.db)
        .await?;
    let backups: Vec<Value> =
        sqlx::query_scalar("SELECT to_jsonb(b) FROM rust_backups b ORDER BY id DESC LIMIT 100")
            .fetch_all(&s.db)
            .await?;
    Ok(Json(json!({"policy":policy,"backups":backups})))
}
pub async fn save_policy(
    State(s): State<App>,
    h: HeaderMap,
    Json(v): Json<Value>,
) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "settings", true, true).await?;
    u.super_admin()?;
    let destination = v["destination"].as_str().unwrap_or("");
    if !destination.is_empty() {
        let allowed =
            regex::Regex::new(r"^[A-Za-z0-9_][A-Za-z0-9_-]{0,60}:[A-Za-z0-9/_ .-]*$").unwrap();
        if !allowed.is_match(destination)
            || destination.len() > 500
            || destination.split('/').any(|p| p == "..")
        {
            return Err(Error::bad("Đích phải là remote rclone đã cấu hình:folder"));
        }
    }
    sqlx::query("UPDATE rust_backup_policy SET enabled=$1,interval_hours=$2,keep_local=$3,keep_cloud=$4,destination=$5,next_run_at=now() WHERE id=true").bind(v["enabled"]==true).bind(i32::try_from(resources::number(&v,"interval_hours")?).map_err(|_|Error::bad("Chu kỳ không hợp lệ"))?).bind(i32::try_from(resources::number(&v,"keep_local")?).map_err(|_|Error::bad("Retention không hợp lệ"))?).bind(i32::try_from(resources::number(&v,"keep_cloud")?).map_err(|_|Error::bad("Retention không hợp lệ"))?).bind(destination).execute(&s.db).await?;
    auth::audit(
        &s,
        Some(u.id),
        "backup_policy_update",
        json!({"enabled":v["enabled"]}),
    )
    .await?;
    Ok(Json(json!({"ok":true})))
}
pub async fn request(State(s): State<App>, h: HeaderMap) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "settings", true, true).await?;
    u.super_admin()?;
    let mut tx = s.db.begin().await?;
    sqlx::query("SELECT id FROM rust_backup_policy FOR UPDATE")
        .fetch_one(&mut *tx)
        .await?;
    let busy: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM rust_backups WHERE status IN ('pending','running'))",
    )
    .fetch_one(&mut *tx)
    .await?;
    if busy {
        return Err(Error::conflict("Đã có backup đang chờ hoặc đang chạy"));
    }
    let id: i64 = sqlx::query_scalar("INSERT INTO rust_backups(actor_id) VALUES($1) RETURNING id")
        .bind(u.id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    auth::audit(&s, Some(u.id), "backup_requested", json!({"id":id})).await?;
    Ok(Json(json!({"id":id,"status":"pending"})))
}
pub async fn download(State(s): State<App>, h: HeaderMap, Path(id): Path<i64>) -> Result<Response> {
    let u = auth::require(&s, &h, "settings", true, false).await?;
    u.super_admin()?;
    let bytes = verified(&s, id).await?;
    auth::audit(&s, Some(u.id), "backup_download", json!({"id":id})).await?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (
                header::CONTENT_DISPOSITION,
                &format!("attachment; filename=\"backup-{id}.pgenc\""),
            ),
        ],
        bytes,
    )
        .into_response())
}
pub async fn restore(
    State(s): State<App>,
    h: HeaderMap,
    Path(id): Path<i64>,
    Json(v): Json<Value>,
) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "settings", true, true).await?;
    u.super_admin()?;
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id=$1")
        .bind(u.id)
        .fetch_one(&s.db)
        .await?;
    if !crate::crypto::verify(v["password"].as_str().unwrap_or(""), &hash) {
        return Err(Error::unauthorized());
    }
    let target = resources::text(&v, "database", 63)?;
    if !target
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        || v["confirm_database"].as_str() != Some(target)
    {
        return Err(Error::bad("Nhập lại đúng tên database đích"));
    }
    let mut config = s.config.clone();
    let mut url = url::Url::parse(&config.database_url)
        .map_err(|_| Error::bad("DATABASE_URL không hợp lệ"))?;
    if url.path().trim_start_matches('/') == target {
        return Err(Error::bad("Không phục hồi vào database đang phục vụ panel"));
    }
    url.set_path(target);
    config.database_url = url.to_string();
    let target_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&config.database_url)
        .await?;
    let empty:bool=sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname NOT IN ('pg_catalog','information_schema') AND n.nspname NOT LIKE 'pg_%' AND c.relkind IN ('r','p','v','m','S','f'))").fetch_one(&target_pool).await?;
    target_pool.close().await;
    if !empty {
        return Err(Error::bad(
            "Giao diện chỉ phục hồi vào database trống, tách biệt",
        ));
    }
    verified(&s, id).await?;
    let name: String = sqlx::query_scalar("SELECT filename FROM rust_backups WHERE id=$1")
        .bind(id)
        .fetch_one(&s.db)
        .await?;
    backup::restore(&config, &directory(&s).join(filename(&name)?), target).await?;
    auth::audit(
        &s,
        Some(u.id),
        "backup_isolated_restore",
        json!({"id":id,"database":target}),
    )
    .await?;
    Ok(Json(json!({"ok":true,"database":target})))
}
pub async fn tick(s: &App) -> anyhow::Result<()> {
    let mut tx = s.db.begin().await?;
    let p = sqlx::query("SELECT * FROM rust_backup_policy FOR UPDATE")
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query("UPDATE rust_backups SET status='failed',last_error='Interrupted backup; request a new backup',finished_at=now() WHERE status='running' AND started_at<now()-interval '30 minutes'").execute(&mut *tx).await?;
    if p.get::<bool, _>("enabled")
        && p.get::<chrono::DateTime<chrono::Utc>, _>("next_run_at") <= chrono::Utc::now()
    {
        sqlx::query("INSERT INTO rust_backups(status) SELECT 'pending' WHERE NOT EXISTS(SELECT 1 FROM rust_backups WHERE status IN ('pending','running'))").execute(&mut *tx).await?;
        sqlx::query("UPDATE rust_backup_policy SET next_run_at=now()+make_interval(hours=>$1)")
            .bind(p.get::<i32, _>("interval_hours"))
            .execute(&mut *tx)
            .await?;
    }
    let id:Option<i64>=sqlx::query_scalar("UPDATE rust_backups SET status='running',started_at=now() WHERE id=(SELECT id FROM rust_backups WHERE status='pending' ORDER BY id LIMIT 1 FOR UPDATE SKIP LOCKED) RETURNING id").fetch_optional(&mut *tx).await?;
    tx.commit().await?;
    if let Some(id) = id {
        match tokio::time::timeout(
            std::time::Duration::from_secs(900),
            backup::create(&s.config, &directory(s)),
        )
        .await
        {
            Ok(Ok(path)) => {
                let bytes = tokio::fs::read(&path).await?;
                let name = path.file_name().unwrap().to_string_lossy().to_string();
                let destination: String = p.get("destination");
                let cloud = if destination.is_empty() {
                    None
                } else {
                    Some(format!("{}/{name}", destination.trim_end_matches('/')))
                };
                sqlx::query("UPDATE rust_backups SET status='done',filename=$1,checksum=$2,bytes=$3,local_available=true,cloud_path=$4,cloud_status=CASE WHEN $4 IS NULL THEN 'none' ELSE 'pending' END,finished_at=now() WHERE id=$5").bind(name).bind(hex::encode(Sha256::digest(bytes.as_slice()))).bind(bytes.len() as i64).bind(cloud).bind(id).execute(&s.db).await?;
            }
            _ => {
                sqlx::query("UPDATE rust_backups SET status='failed',last_error='Backup failed; check pg_dump, database permissions and disk space',finished_at=now() WHERE id=$1").bind(id).execute(&s.db).await?;
            }
        }
    }
    retrieve_one(s).await?;
    cloud_one(s).await?;
    retain(s, p.get("keep_local"), p.get("keep_cloud")).await?;
    Ok(())
}
async fn cloud_one(s: &App) -> anyhow::Result<()> {
    let mut tx = s.db.begin().await?;
    let r=sqlx::query("SELECT * FROM rust_backups WHERE cloud_status IN ('pending','failed') AND cloud_attempts<5 AND cloud_retry_at<=now() AND local_available AND status='done' ORDER BY id LIMIT 1 FOR UPDATE SKIP LOCKED").fetch_optional(&mut *tx).await?;
    if let Some(r) = r {
        let name: String = r.get("filename");
        let path = directory(s).join(filename(&name).map_err(|e| anyhow::anyhow!(e.1))?);
        let result = tokio::process::Command::new(rclone_bin())
            .args(["copyto", "--checksum", "--retries", "1"])
            .arg(path)
            .arg(r.get::<String, _>("cloud_path"))
            .kill_on_drop(true)
            .output();
        let uploaded = matches!(tokio::time::timeout(std::time::Duration::from_secs(300),result).await,Ok(Ok(ref output))if output.status.success());
        sqlx::query("UPDATE rust_backups SET cloud_status=$1,cloud_attempts=cloud_attempts+1,cloud_retry_at=now()+interval '5 minutes',last_error=CASE WHEN $2 THEN NULL ELSE 'Cloud upload failed; inspect rclone configuration and connectivity' END WHERE id=$3").bind(if uploaded{"uploaded"}else{"failed"}).bind(uploaded).bind(r.get::<i64,_>("id")).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}
async fn retain(s: &App, local: i32, cloud: i32) -> anyhow::Result<()> {
    let mut tx = s.db.begin().await?;
    let rows=sqlx::query("SELECT * FROM rust_backups WHERE status='done' AND local_available ORDER BY id DESC OFFSET $1 FOR UPDATE SKIP LOCKED").bind(i64::from(local)).fetch_all(&mut *tx).await?;
    for r in rows {
        if matches!(
            r.get::<String, _>("retrieval_status").as_str(),
            "pending" | "running"
        ) || r
            .get::<Option<chrono::DateTime<chrono::Utc>>, _>("retrieval_finished_at")
            .is_some_and(|t| t > chrono::Utc::now() - chrono::Duration::hours(24))
        {
            continue;
        }
        if !["none", "uploaded", "deleted"].contains(&r.get::<String, _>("cloud_status").as_str()) {
            continue;
        }
        let name: String = r.get("filename");
        let path = directory(s).join(filename(&name).map_err(|e| anyhow::anyhow!(e.1))?);
        match tokio::fs::remove_file(path).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        };
        sqlx::query("UPDATE rust_backups SET local_available=false WHERE id=$1")
            .bind(r.get::<i64, _>("id"))
            .execute(&mut *tx)
            .await?;
    }
    let rows=sqlx::query("SELECT * FROM rust_backups WHERE cloud_status='uploaded' AND retrieval_status NOT IN ('pending','running') ORDER BY id DESC OFFSET $1 FOR UPDATE SKIP LOCKED").bind(i64::from(cloud)).fetch_all(&mut *tx).await?;
    for r in rows {
        let remote: String = r.get("cloud_path");
        let output = tokio::process::Command::new(rclone_bin())
            .args(["deletefile", "--retries", "1"])
            .arg(remote)
            .kill_on_drop(true)
            .output();
        if matches!(tokio::time::timeout(std::time::Duration::from_secs(30),output).await,Ok(Ok(ref o))if o.status.success())
        {
            sqlx::query("UPDATE rust_backups SET cloud_status='deleted' WHERE id=$1")
                .bind(r.get::<i64, _>("id"))
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

fn rclone_bin() -> String {
    std::env::var("BACKUP_RCLONE_BIN").unwrap_or("rclone".into())
}
pub async fn request_retrieval(
    State(s): State<App>,
    h: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "settings", true, true).await?;
    u.super_admin()?;
    let mut tx = s.db.begin().await?;
    let destination: String =
        sqlx::query_scalar("SELECT destination FROM rust_backup_policy FOR SHARE")
            .fetch_one(&mut *tx)
            .await?;
    let row=sqlx::query("SELECT * FROM rust_backups WHERE id=$1 AND status='done' AND cloud_status='uploaded' AND NOT local_available FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?.ok_or_else(Error::missing)?;
    let name: String = row.get("filename");
    if destination.is_empty()
        || row.get::<String, _>("cloud_path")
            != format!("{}/{}", destination.trim_end_matches('/'), filename(&name)?)
    {
        return Err(Error::conflict("Đích cloud hiện tại không khớp bản backup"));
    }
    if matches!(
        row.get::<String, _>("retrieval_status").as_str(),
        "pending" | "running"
    ) {
        return Err(Error::conflict("Backup đang được tải"));
    }
    if row.get::<i64, _>("bytes") > 256 * 1024 * 1024 {
        return Err(Error::bad("Tải qua panel giới hạn 256 MiB"));
    }
    sqlx::query("UPDATE rust_backups SET retrieval_status='pending',retrieval_actor_id=$1,retrieval_requested_at=now(),retrieval_finished_at=NULL,last_error=NULL WHERE id=$2").bind(u.id).bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    auth::audit(
        &s,
        Some(u.id),
        "backup_retrieval_requested",
        json!({"id":id}),
    )
    .await?;
    Ok(Json(json!({"ok":true,"status":"pending"})))
}
async fn retrieve_one(s: &App) -> anyhow::Result<()> {
    use crate::backup_tools as files;
    let mut tx = s.db.begin().await?;
    sqlx::query("UPDATE rust_backups SET retrieval_status='failed',last_error='Interrupted retrieval; request again',retrieval_finished_at=now() WHERE retrieval_status='running' AND retrieval_requested_at<now()-interval '15 minutes'").execute(&mut *tx).await?;
    let row=sqlx::query("SELECT * FROM rust_backups WHERE retrieval_status='pending' AND status='done' ORDER BY id LIMIT 1 FOR UPDATE SKIP LOCKED").fetch_optional(&mut *tx).await?;
    let Some(row) = row else { return Ok(()) };
    let id: i64 = row.get("id");
    sqlx::query("UPDATE rust_backups SET retrieval_status='running' WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let mut tx = s.db.begin().await?;
    let destination: String =
        sqlx::query_scalar("SELECT destination FROM rust_backup_policy FOR SHARE")
            .fetch_one(&mut *tx)
            .await?;
    sqlx::query("SELECT id FROM rust_backups WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    let root = directory(s);
    files::private_dir(&root)
        .await
        .map_err(|e| anyhow::anyhow!(e.1))?;
    let work = files::work(&root).await.map_err(|e| anyhow::anyhow!(e.1))?;
    let result:Result<()>=async {
        let actor:i64=row.get("retrieval_actor_id");
        let allowed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id=$1 AND role='admin' AND admin_level='super' AND status='active')").bind(actor).fetch_one(&mut *tx).await?;
        if !allowed {return Err(Error::forbidden());}
        let name:String=row.get("filename");let name=filename(&name)?;
        let remote:String=row.get("cloud_path");
        if destination.is_empty() || remote!=format!("{}/{}",destination.trim_end_matches('/'),name) || row.get::<String,_>("cloud_status")!="uploaded" || row.get::<bool,_>("local_available") {return Err(Error::conflict("Trạng thái hoặc đích cloud đã thay đổi"));}
        let target=work.join(name);
        files::command(&rclone_bin(),&["copy".into(),"--retries".into(),"1".into(),"--max-size".into(),"256M".into(),"--max-depth".into(),"1".into(),"--include".into(),name.into(),destination.clone(),work.to_string_lossy().into_owned()],&[],&work).await?;
        let bytes=files::read(&target,files::MAX_ARCHIVE).await?;
        if files::hash(&bytes)!=row.get::<String,_>("checksum") || bytes.len() as i64!=row.get::<i64,_>("bytes") {return Err(Error::bad("Checksum hoặc kích thước cloud không khớp"));}
        backup::decrypt(&s.config.key,&bytes)?;
        files::write(&root.join(name),&bytes).await?;
        Ok(())
    }.await;
    let success = result.is_ok();
    sqlx::query("UPDATE rust_backups SET retrieval_status=$1,retrieval_finished_at=now(),local_available=CASE WHEN $2 THEN true ELSE local_available END,last_error=CASE WHEN $2 THEN NULL ELSE 'Cloud retrieval failed; inspect destination, checksum, APP_KEY and rclone' END WHERE id=$3").bind(if success{"done"}else{"failed"}).bind(success).bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    let _ = tokio::fs::remove_dir_all(work).await;
    auth::audit(
        s,
        row.get::<Option<i64>, _>("retrieval_actor_id"),
        "backup_retrieval_finished",
        json!({"id":id,"success":success}),
    )
    .await
    .map_err(|e| anyhow::anyhow!(e.1))?;
    Ok(())
}
