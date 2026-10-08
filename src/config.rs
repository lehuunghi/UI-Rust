use anyhow::{bail, Context, Result};
#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub bind: String,
    pub app_url: String,
    pub key: [u8; 32],
    pub secure_cookie: bool,
    pub sepay_key: String,
    pub sepay_hmac: String,
    pub sepay_account: String,
    pub sepay_bank: String,
    pub smtp_host: String,
    pub smtp_user: String,
    pub smtp_password: String,
    pub smtp_from: String,
}
impl Config {
    pub fn load() -> Result<Self> {
        let key = hex::decode(
            std::env::var("APP_KEY").context("Set APP_KEY to 64 random hex characters")?,
        )?;
        let key: [u8; 32] = key
            .try_into()
            .map_err(|_| anyhow::anyhow!("APP_KEY must contain 32 bytes"))?;
        let app_url = env("APP_URL", "http://localhost:8080");
        let u = url::Url::parse(&app_url)?;
        if !["http", "https"].contains(&u.scheme()) || u.host_str().is_none() {
            bail!("Invalid APP_URL")
        }
        let secure_cookie = u.scheme() == "https";
        Ok(Self {
            database_url: std::env::var("DATABASE_URL").context("DATABASE_URL required")?,
            bind: env("BIND", "127.0.0.1:8080"),
            app_url: app_url.trim_end_matches('/').into(),
            key,
            secure_cookie,
            sepay_key: env("SEPAY_API_KEY", ""),
            sepay_hmac: env("SEPAY_HMAC_SECRET", ""),
            sepay_account: env("SEPAY_ACCOUNT_NUMBER", ""),
            sepay_bank: env("SEPAY_BANK_CODE", ""),
            smtp_host: env("SMTP_HOST", ""),
            smtp_user: env("SMTP_USER", ""),
            smtp_password: env("SMTP_PASSWORD", ""),
            smtp_from: env("SMTP_FROM", ""),
        })
    }
}
pub fn env(k: &str, d: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| d.into())
}
