use crate::{
    backup_tools as files,
    error::{Error, Result},
};
use chrono::Utc;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
pub async fn all() -> Result<Value> {
    let Ok(path) = std::env::var("RECOVERY_SOURCES_FILE") else {
        return Ok(json!({}));
    };
    let bytes = files::read(Path::new(&path), 262144).await?;
    let data: Value = serde_json::from_slice(&bytes)
        .map_err(|_| Error::bad("Cấu hình nguồn recovery không hợp lệ"))?;
    data["profiles"]
        .as_object()
        .filter(|o| o.len() <= 100)
        .ok_or_else(|| Error::bad("Cấu hình nguồn recovery thiếu profiles"))?;
    Ok(data["profiles"].clone())
}
pub async fn get(key: &str, base: &str) -> Result<Value> {
    if !regex::Regex::new(r"^[A-Za-z0-9_-]{1,100}$")
        .unwrap()
        .is_match(key)
    {
        return Err(Error::bad("Nguồn recovery không hợp lệ"));
    }
    let all = all().await?;
    let config = all[key].clone();
    let origin = url::Url::parse(base)
        .map_err(|_| Error::bad("URL server không hợp lệ"))?
        .origin()
        .ascii_serialization();
    if !config.is_object() || config["server_origin"] != origin {
        return Err(Error::bad(
            "Chưa khai báo nguồn recovery khớp server trên máy vận hành",
        ));
    }
    Ok(config)
}
pub async fn path(path: &str) -> Result<PathBuf> {
    let path = PathBuf::from(path);
    if !path.is_absolute() || path.to_string_lossy().chars().any(char::is_control) {
        return Err(Error::bad("Snapshot cần đường dẫn tuyệt đối"));
    }
    for ancestor in path.ancestors() {
        let meta = tokio::fs::symlink_metadata(ancestor)
            .await
            .map_err(|_| Error::bad("Đường dẫn snapshot không tồn tại"))?;
        if meta.file_type().is_symlink() {
            return Err(Error::bad("Snapshot không được chứa symlink"));
        }
    }
    let real = tokio::fs::canonicalize(&path)
        .await
        .map_err(|_| Error::bad("Snapshot không tồn tại"))?;
    let root = std::env::current_dir().map_err(|_| Error::missing())?;
    if real.starts_with(root.join("static")) || real.starts_with(root.join("backups")) {
        return Err(Error::bad(
            "Snapshot phải nằm ngoài thư mục public và backup",
        ));
    }
    Ok(real)
}
pub async fn marker(config: &Value) -> Result<Value> {
    let path = path(config["snapshot_marker"].as_str().unwrap_or("")).await?;
    let bytes = files::read(&path, 32768).await?;
    let marker: Value =
        serde_json::from_slice(&bytes).map_err(|_| Error::bad("Marker snapshot không hợp lệ"))?;
    let created = chrono::DateTime::parse_from_rfc3339(marker["created_at"].as_str().unwrap_or(""))
        .map_err(|_| Error::bad("Marker thiếu thời gian"))?;
    let age = Utc::now().signed_duration_since(created).num_seconds();
    let max = config["max_snapshot_age_seconds"]
        .as_i64()
        .unwrap_or(86400)
        .clamp(300, 604800);
    if marker["consistent"] != true
        || marker["server_origin"] != config["server_origin"]
        || !marker["snapshot_id"].as_str().is_some_and(|s| {
            regex::Regex::new(r"^[A-Za-z0-9_.-]{1,100}$")
                .unwrap()
                .is_match(s)
        })
        || age < -300
        || age > max
    {
        return Err(Error::bad("Snapshot chưa xác nhận nhất quán hoặc đã cũ"));
    }
    Ok(marker)
}
pub fn entry(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1024
        && !value.starts_with('/')
        && !value
            .chars()
            .any(|c| c.is_control() || ['\\', ':'].contains(&c))
        && value
            .split('/')
            .all(|s| !s.is_empty() && s != "." && s != "..")
}
pub async fn inventory(config: &Value) -> Result<Vec<(String, PathBuf)>> {
    let components = config["components"]
        .as_object()
        .filter(|o| !o.is_empty() && o.len() <= 12)
        .ok_or_else(|| Error::bad("Inventory backend chưa đầy đủ"))?;
    let required =
        config
            .get("required_components")
            .cloned()
            .unwrap_or(json!(["data", "blob", "bootstrap"]));
    for role in required
        .as_array()
        .ok_or_else(|| Error::bad("Thành phần bắt buộc không hợp lệ"))?
    {
        if !role.as_str().is_some_and(|r| components.contains_key(r)) {
            return Err(Error::bad("Inventory thiếu thành phần bắt buộc"));
        }
    }
    let max = config["max_bytes"]
        .as_u64()
        .unwrap_or(files::MAX_ARCHIVE)
        .min(files::MAX_ARCHIVE);
    let max_files = config["max_files"]
        .as_u64()
        .unwrap_or(10000)
        .clamp(1, 20000);
    let mut total = 0_u64;
    let mut output = Vec::new();
    let mut roots = Vec::new();
    for (role, value) in components {
        if ![
            "data",
            "blob",
            "search",
            "bootstrap",
            "queue",
            "certificates",
            "external_directory",
            "lookup",
            "metrics",
            "traces",
            "encryption_keys",
            "logs",
        ]
        .contains(&role.as_str())
        {
            return Err(Error::bad("Loại backend không hỗ trợ"));
        }
        let root = path(value.as_str().unwrap_or("")).await?;
        if roots
            .iter()
            .any(|r: &PathBuf| root.starts_with(r) || r.starts_with(&root))
        {
            return Err(Error::bad("Thành phần snapshot chồng lấp"));
        }
        roots.push(root.clone());
        let mut pending = vec![root.clone()];
        while let Some(file) = pending.pop() {
            let meta = tokio::fs::symlink_metadata(&file)
                .await
                .map_err(|_| Error::bad("Snapshot không đọc được"))?;
            if meta.file_type().is_symlink() {
                return Err(Error::bad("Snapshot chứa symlink"));
            }
            if meta.is_dir() {
                let mut dir = tokio::fs::read_dir(&file)
                    .await
                    .map_err(|_| Error::bad("Không duyệt được snapshot"))?;
                while let Some(child) = dir
                    .next_entry()
                    .await
                    .map_err(|_| Error::bad("Không duyệt được snapshot"))?
                {
                    pending.push(child.path());
                    if pending.len() > max_files as usize {
                        return Err(Error::bad("Snapshot vượt giới hạn số tệp"));
                    }
                }
            } else if meta.is_file() {
                total = total
                    .checked_add(meta.len())
                    .filter(|n| *n <= max)
                    .ok_or_else(|| Error::bad("Snapshot vượt giới hạn byte"))?;
                if output.len() >= max_files as usize {
                    return Err(Error::bad("Snapshot vượt giới hạn số tệp"));
                }
                let relative = if root == file {
                    file.file_name().unwrap().to_string_lossy().into_owned()
                } else {
                    file.strip_prefix(&root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned()
                };
                let name = format!("components/{role}/{relative}");
                if !entry(&name) {
                    return Err(Error::bad("Tên tệp snapshot không hợp lệ"));
                }
                output.push((name, file));
            } else {
                return Err(Error::bad("Snapshot chứa tệp đặc biệt"));
            }
        }
    }
    if output.is_empty() {
        return Err(Error::bad("Snapshot rỗng"));
    }
    output.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(output)
}
pub async fn target(config: &Value) -> Result<PathBuf> {
    if config["isolated_rehearsal"] != true || config["outbound_isolated"] != true {
        return Err(Error::bad(
            "Đích phải là môi trường diễn tập cô lập và chặn gửi ra ngoài",
        ));
    }
    let root = path(config["restore_root"].as_str().unwrap_or("")).await?;
    if !root.is_dir() {
        return Err(Error::bad("Thư mục phục hồi không hợp lệ"));
    }
    let app = std::env::current_dir().map_err(|_| Error::missing())?;
    if root.starts_with(&app) || app.starts_with(&root) {
        return Err(Error::bad("Đích phục hồi không được bao phủ mã panel"));
    }
    let all = all().await?;
    for source in all.as_object().into_iter().flat_map(|o| o.values()) {
        for value in source["components"]
            .as_object()
            .into_iter()
            .flat_map(|o| o.values())
        {
            let p = path(value.as_str().unwrap_or("")).await?;
            if root.starts_with(&p) || p.starts_with(&root) {
                return Err(Error::bad("Đích phục hồi chồng lấp nguồn snapshot"));
            }
        }
    }
    Ok(root)
}
pub async fn adapter(
    config: &Value,
    phase: &str,
    work: &Path,
    stage: Option<&Path>,
    artifact: Option<&Value>,
) -> Result<Option<Value>> {
    let Some(command) = config.get(format!("{phase}_command")) else {
        return Ok(None);
    };
    let args = command
        .as_array()
        .filter(|a| {
            !a.is_empty()
                && a.len() <= 20
                && a.iter().all(|v| {
                    v.as_str().is_some_and(|s| {
                        s.len() <= 1024 && !s.chars().any(|c| ['\0', '\r', '\n'].contains(&c))
                    })
                })
        })
        .ok_or_else(|| Error::bad("Adapter recovery không hợp lệ"))?;
    let binary = path(args[0].as_str().unwrap()).await?;
    let app = std::env::current_dir().map_err(|_| Error::missing())?;
    if binary.starts_with(app) || !binary.is_file() {
        return Err(Error::bad("Adapter phải nằm ngoài mã panel"));
    }
    let receipt = work.join("adapter-receipt.json");
    let sha = artifact.and_then(|a| a["sha256"].as_str()).unwrap_or("");
    if phase == "restore" && config["outbound_isolated"] != true {
        return Err(Error::bad("Adapter yêu cầu cô lập gửi thư"));
    }
    let argv: Vec<_> = args[1..]
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    files::command(
        binary.to_str().ok_or_else(Error::missing)?,
        &argv,
        &[
            ("UI_RECOVERY_PHASE", phase.into()),
            (
                "UI_RECOVERY_ORIGIN",
                config["server_origin"].as_str().unwrap().into(),
            ),
            (
                "UI_RECOVERY_STAGE",
                stage
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            ),
            (
                "UI_RECOVERY_RECEIPT",
                receipt.to_string_lossy().into_owned(),
            ),
            ("UI_RECOVERY_ARTIFACT_SHA256", sha.into()),
        ],
        work,
    )
    .await?;
    if phase == "snapshot" {
        return Ok(Some(json!({"checkpoint_prepared":true})));
    }
    let bytes = files::read(&receipt, 32768).await?;
    let r: Value =
        serde_json::from_slice(&bytes).map_err(|_| Error::bad("Biên bản adapter không hợp lệ"))?;
    if r["artifact_sha256"] != sha
        || r["server_origin"] != config["server_origin"]
        || ![
            "backend_loaded",
            "credentials_checked",
            "mail_folders_checked",
            "attachments_checked",
            "queue_checked",
            "outbound_isolated",
        ]
        .iter()
        .all(|k| r[*k] == true)
    {
        return Err(Error::bad(
            "Adapter chưa hoàn tất kiểm chứng backend hoặc sai biên bản",
        ));
    }
    Ok(Some(
        json!({"backend_activated":true,"adapter_checks_passed":true,"checked_at":Utc::now().to_rfc3339()}),
    ))
}
