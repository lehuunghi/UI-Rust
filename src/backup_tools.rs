use crate::error::{Error, Result};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::{io::AsyncReadExt, process::Command};
pub const MAX_ARCHIVE: u64 = 256 * 1024 * 1024;
pub fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
pub async fn private_dir(path: &Path) -> Result<()> {
    tokio::fs::create_dir_all(path)
        .await
        .map_err(|_| Error::bad("Không tạo được thư mục backup"))?;
    let meta = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|_| Error::missing())?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(Error::bad("Thư mục backup không hợp lệ"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .await
            .map_err(|_| Error::bad("Không đặt được quyền thư mục"))?;
    }
    Ok(())
}
pub async fn work(root: &Path) -> Result<PathBuf> {
    private_dir(root).await?;
    let path = root.join(format!(".work-{}", crate::crypto::token()));
    tokio::fs::create_dir(&path)
        .await
        .map_err(|_| Error::bad("Không tạo được thư mục tạm"))?;
    private_dir(&path).await?;
    tokio::fs::canonicalize(path)
        .await
        .map_err(|_| Error::bad("Không xác định được thư mục tạm"))
}
pub async fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut options = tokio::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(path)
        .await
        .map_err(|_| Error::bad("Không lưu được backup"))?;
    file.write_all(bytes)
        .await
        .map_err(|_| Error::bad("Không lưu được backup"))?;
    file.sync_all()
        .await
        .map_err(|_| Error::bad("Không hoàn tất backup"))?;
    Ok(())
}
pub async fn read(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let meta = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|_| Error::missing())?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > limit {
        return Err(Error::bad("Tệp backup không hợp lệ hoặc vượt giới hạn"));
    }
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| Error::missing())?;
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| Error::bad("Không đọc được tệp"))?;
    if bytes.len() as u64 > limit {
        return Err(Error::bad("Tệp vượt giới hạn"));
    }
    Ok(bytes)
}
pub async fn command(
    binary: &str,
    args: &[String],
    env: &[(&str, String)],
    work: &Path,
) -> Result<String> {
    command_with_limit(binary, args, env, work, 1024 * 1024).await
}
pub async fn command_with_limit(
    binary: &str,
    args: &[String],
    env: &[(&str, String)],
    work: &Path,
    output_limit: u64,
) -> Result<String> {
    let mut command = Command::new(binary);
    command
        .args(args)
        .envs(env.iter().map(|(k, v)| (*k, v)))
        .current_dir(work)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|_| {
        Error::bad("Không khởi chạy được công cụ backup; kiểm tra cài đặt trên máy")
    })?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let run = async {
        async fn bounded<R: tokio::io::AsyncRead + Unpin>(
            reader: R,
            limit: u64,
        ) -> Result<Vec<u8>> {
            let mut data = Vec::new();
            reader
                .take(limit + 1)
                .read_to_end(&mut data)
                .await
                .map_err(|_| Error::bad("Không đọc được kết quả công cụ"))?;
            if data.len() as u64 > limit {
                return Err(Error::bad("Công cụ trả kết quả quá lớn"));
            }
            Ok(data)
        }
        let (out, _) = tokio::try_join!(
            bounded(stdout, output_limit.min(64 * 1024 * 1024)),
            bounded(stderr, 1024 * 1024)
        )?;
        let status = child
            .wait()
            .await
            .map_err(|_| Error::bad("Không xác nhận được công cụ"))?;
        if !status.success() {
            return Err(Error::bad(
                "Công cụ backup thất bại; kiểm tra phiên bản, kết nối và credential",
            ));
        }
        Ok(String::from_utf8_lossy(&out).into_owned())
    };
    tokio::time::timeout(std::time::Duration::from_secs(3600), run)
        .await
        .map_err(|_| Error::bad("Công cụ backup quá thời gian"))?
}
pub async fn sqlite(path: &Path) -> Result<()> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        let connection =
            rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|_| Error::bad("Archive SQLite không hợp lệ"))?;
        let check: String = connection
            .query_row("PRAGMA quick_check", [], |r| r.get(0))
            .map_err(|_| Error::bad("Không kiểm tra được archive SQLite"))?;
        if check != "ok" {
            return Err(Error::bad("Archive SQLite hỏng"));
        }
        Ok(())
    })
    .await
    .map_err(|_| Error::bad("Không kiểm tra được archive"))?
}
pub async fn clean_orphans(root: &Path) -> Result<()> {
    if !tokio::fs::try_exists(root).await.unwrap_or(false) {
        return Ok(());
    }
    private_dir(root).await?;
    let mut dir = tokio::fs::read_dir(root)
        .await
        .map_err(|_| Error::bad("Không duyệt được thư mục backup"))?;
    while let Some(entry) = dir
        .next_entry()
        .await
        .map_err(|_| Error::bad("Không duyệt được thư mục backup"))?
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name
            .strip_prefix(".work-")
            .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            let meta = tokio::fs::symlink_metadata(entry.path())
                .await
                .map_err(|_| Error::missing())?;
            if meta.is_dir() && !meta.file_type().is_symlink() {
                tokio::fs::remove_dir_all(entry.path())
                    .await
                    .map_err(|_| Error::bad("Không dọn được backup tạm từ lần chạy trước"))?;
            }
        }
    }
    Ok(())
}
