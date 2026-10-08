use crate::config::Config;
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use anyhow::{anyhow, bail, Result};
use rand::{rngs::OsRng, RngCore};
use std::{path::Path, process::Stdio};
use tokio::{io::AsyncWriteExt, process::Command};
const MAGIC: &[u8] = b"UIRUST-BACKUP-1\n";
fn decode(s: &str) -> Result<String> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                bail!("Invalid connection URL encoding")
            }
            out.push(u8::from_str_radix(&s[i + 1..i + 3], 16)?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Ok(String::from_utf8(out)?)
}
fn command(name: &str, c: &Config) -> Result<(Command, String)> {
    let url = url::Url::parse(&c.database_url)?;
    let database = decode(url.path().trim_start_matches('/'))?;
    if database.is_empty() {
        bail!("Missing database name")
    }
    let mut cmd = Command::new(name);
    cmd.env("PGHOST", url.host_str().unwrap_or("localhost"))
        .env("PGPORT", url.port().unwrap_or(5432).to_string())
        .env("PGUSER", decode(url.username())?)
        .env("PGPASSWORD", decode(url.password().unwrap_or(""))?)
        .env("PGDATABASE", &database);
    for (k, v) in url.query_pairs() {
        if k == "sslmode" {
            cmd.env("PGSSLMODE", v.as_ref());
        }
    }
    Ok((cmd, database))
}
pub fn encrypt(key: &[u8; 32], data: &[u8]) -> Result<Vec<u8>> {
    let mut nonce = [0; 12];
    OsRng.fill_bytes(&mut nonce);
    let cipher = Aes256Gcm::new_from_slice(key)
        .unwrap()
        .encrypt(Nonce::from_slice(&nonce), data)
        .map_err(|_| anyhow!("Backup encryption failed"))?;
    let mut output = MAGIC.to_vec();
    output.extend_from_slice(&nonce);
    output.extend_from_slice(&cipher);
    Ok(output)
}
pub fn decrypt(key: &[u8; 32], data: &[u8]) -> Result<Vec<u8>> {
    if !data.starts_with(MAGIC) || data.len() < MAGIC.len() + 28 {
        bail!("Invalid backup format")
    }
    let n = MAGIC.len();
    Aes256Gcm::new_from_slice(key)
        .unwrap()
        .decrypt(Nonce::from_slice(&data[n..n + 12]), &data[n + 12..])
        .map_err(|_| anyhow!("Wrong APP_KEY or damaged backup"))
}
pub async fn create(c: &Config, directory: &Path) -> Result<std::path::PathBuf> {
    let (mut cmd, _) = command("pg_dump", c)?;
    let output = cmd
        .args(["--format=custom", "--no-owner", "--no-acl"])
        .output()
        .await?;
    if !output.status.success() {
        bail!(
            "pg_dump failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    }
    let encrypted = encrypt(&c.key, &output.stdout)?;
    tokio::fs::create_dir_all(directory).await?;
    let path = directory.join(format!(
        "ui-{}-{}.pgenc",
        chrono::Utc::now().format("%Y%m%dT%H%M%SZ"),
        &crate::crypto::token()[..8]
    ));
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut f = options.open(&path).await?;
    f.write_all(&encrypted).await?;
    f.sync_all().await?;
    Ok(path)
}
pub async fn restore(c: &Config, path: &Path, confirmed_db: &str) -> Result<()> {
    let (mut cmd, db) = command("pg_restore", c)?;
    if db != confirmed_db {
        bail!("Pass --confirm-db followed by the exact destination database name")
    }
    let encrypted = tokio::fs::read(path).await?;
    let data = decrypt(&c.key, &encrypted)?;
    let mut child = cmd
        .args([
            "--dbname",
            &db,
            "--single-transaction",
            "--exit-on-error",
            "--clean",
            "--if-exists",
            "--no-owner",
            "--no-acl",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut input = child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("Missing pg_restore stdin"))?;
    let writer = tokio::spawn(async move { input.write_all(&data).await });
    let output = child.wait_with_output().await?;
    writer.await??;
    if !output.status.success() {
        bail!(
            "pg_restore failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encrypted_archive_authenticates() {
        let key = [9; 32];
        let mut encrypted = encrypt(&key, b"PGDMP fixture").unwrap();
        assert_eq!(decrypt(&key, &encrypted).unwrap(), b"PGDMP fixture");
        let last = encrypted.len() - 1;
        encrypted[last] ^= 1;
        assert!(decrypt(&key, &encrypted).is_err());
    }
}
