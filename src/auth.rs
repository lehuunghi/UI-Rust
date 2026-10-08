use crate::{
    crypto,
    error::{Error, Result},
    App,
};
use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;
#[derive(Clone, Debug)]
pub struct User {
    pub id: i64,
    pub owner: i64,
    pub role: String,
    pub level: String,
    pub permissions: Value,
    pub csrf: String,
    pub session: String,
}
impl User {
    pub fn can(&self, p: &str) -> bool {
        self.role == "customer"
            || (self.role == "admin" && self.level == "super")
            || self
                .permissions
                .as_array()
                .is_some_and(|a| a.iter().any(|x| x == p))
    }
    pub fn super_admin(&self) -> Result<()> {
        if self.role == "admin" && self.level == "super" {
            Ok(())
        } else {
            Err(Error::forbidden())
        }
    }
}
pub fn origin(s: &App, h: &HeaderMap) -> Result<()> {
    let expected = &s.config.app_url;
    let got = h.get("origin").and_then(|v| v.to_str().ok()).unwrap_or("");
    if got != expected {
        return Err(Error::forbidden());
    }
    Ok(())
}
fn cookie(h: &HeaderMap) -> Option<&str> {
    h.get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|s| s.strip_prefix("ui_session="))
}
pub async fn user(s: &App, h: &HeaderMap, authenticated: bool) -> Result<User> {
    let token = cookie(h)
        .filter(|s| s.len() == 64)
        .ok_or_else(Error::unauthorized)?;
    let r=sqlx::query("SELECT u.*,s.csrf,s.authenticated,s.password_fingerprint FROM panel_sessions s JOIN users u ON u.id=s.user_id WHERE s.id=$1 AND s.expires_at>now() AND s.revoked_at IS NULL AND u.status='active'").bind(crypto::hash(token)).fetch_optional(&s.db).await?.ok_or_else(Error::unauthorized)?;
    if authenticated && !r.get::<bool, _>("authenticated") {
        return Err(Error::unauthorized());
    }
    if r.get::<String, _>("password_fingerprint")
        != crypto::hash(&r.get::<String, _>("password_hash"))
    {
        return Err(Error::unauthorized());
    }
    let id = r.get("id");
    let role: String = r.get("role");
    let owner = r.get::<Option<i64>, _>("parent_customer_id").unwrap_or(id);
    let mut permissions = r
        .get::<Option<Value>, _>("admin_permissions")
        .unwrap_or(json!([]));
    if role == "sub_admin" {
        permissions = sqlx::query_scalar(
            "SELECT permissions FROM sub_admins WHERE user_id=$1 AND status='active'",
        )
        .bind(id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(Error::forbidden)?;
        let active: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM users WHERE id=$1 AND status='active')",
        )
        .bind(owner)
        .fetch_one(&s.db)
        .await?;
        if !active {
            return Err(Error::forbidden());
        }
    }
    Ok(User {
        id,
        owner,
        role,
        level: r
            .get::<Option<String>, _>("admin_level")
            .unwrap_or("super".into()),
        permissions,
        csrf: r.get("csrf"),
        session: crypto::hash(token),
    })
}
pub async fn require(s: &App, h: &HeaderMap, p: &str, admin: bool, write: bool) -> Result<User> {
    let u = user(s, h, true).await?;
    if (admin && u.role != "admin") || !u.can(p) {
        return Err(Error::forbidden());
    }
    if write {
        origin(s, h)?;
        let t = h
            .get("x-csrf-token")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !crypto::equal(&u.csrf, t) {
            return Err(Error::forbidden());
        }
    }
    Ok(u)
}
pub async fn audit(s: &App, uid: Option<i64>, action: &str, context: Value) -> Result<()> {
    sqlx::query("INSERT INTO audit_logs(user_id,action,context) VALUES($1,$2,$3)")
        .bind(uid)
        .bind(action)
        .bind(context)
        .execute(&s.db)
        .await?;
    Ok(())
}
async fn limit(s: &App, key: &str) -> Result<()> {
    let n:i32=sqlx::query_scalar("INSERT INTO auth_rate_limits(bucket,attempts,expires_at) VALUES($1,1,now()+interval '15 minutes') ON CONFLICT(bucket) DO UPDATE SET attempts=CASE WHEN auth_rate_limits.expires_at<now() THEN 1 ELSE auth_rate_limits.attempts+1 END,expires_at=CASE WHEN auth_rate_limits.expires_at<now() THEN now()+interval '15 minutes' ELSE auth_rate_limits.expires_at END RETURNING attempts").bind(crypto::hash(key)).fetch_one(&s.db).await?;
    if n > 10 {
        return Err(Error(
            StatusCode::TOO_MANY_REQUESTS,
            "Thử lại sau 15 phút".into(),
        ));
    }
    Ok(())
}
pub fn valid_email(s: &str) -> bool {
    let Some((l, d)) = s.split_once('@') else {
        return false;
    };
    !l.is_empty()
        && d.contains('.')
        && s.len() <= 190
        && !s.chars().any(|c| c.is_whitespace() || c.is_control())
        && !d.contains('@')
}
#[derive(Deserialize)]
pub struct Credentials {
    email: String,
    password: String,
    #[serde(default)]
    name: String,
}
pub async fn register(
    State(s): State<App>,
    h: HeaderMap,
    Json(d): Json<Credentials>,
) -> Result<Json<Value>> {
    origin(&s, &h)?;
    let email = d.email.trim().to_lowercase();
    if !valid_email(&email) || d.name.trim().is_empty() || d.name.len() > 190 {
        return Err(Error::bad("Tên hoặc email không hợp lệ"));
    }
    limit(&s, &format!("register:{email}")).await?;
    let p = tokio::task::spawn_blocking(move || crypto::password(&d.password))
        .await
        .map_err(|e| anyhow::anyhow!(e))??;
    let id:i64=sqlx::query_scalar("INSERT INTO users(name,email,password_hash,role,two_factor_enabled) VALUES($1,$2,$3,'customer',0) RETURNING id").bind(d.name.trim()).bind(email).bind(p).fetch_one(&s.db).await?;
    audit(&s, Some(id), "register", json!({})).await?;
    Ok(Json(json!({"ok":true})))
}
fn session_cookie(s: &App, token: &str) -> String {
    format!(
        "ui_session={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age=43200{}",
        if s.config.secure_cookie {
            "; Secure"
        } else {
            ""
        }
    )
}
pub async fn login(
    State(s): State<App>,
    h: HeaderMap,
    Json(d): Json<Credentials>,
) -> Result<Response> {
    origin(&s, &h)?;
    let email = d.email.trim().to_lowercase();
    limit(&s, &format!("login:{email}")).await?;
    let r=sqlx::query("SELECT id,password_hash,two_factor_enabled,two_factor_method,two_factor_secret FROM users WHERE lower(email)=$1 AND status='active'").bind(&email).fetch_optional(&s.db).await?;
    let hash = r
        .as_ref()
        .map(|r| r.get::<String, _>("password_hash"))
        .unwrap_or(s.dummy_hash.clone());
    let fp = crypto::hash(&hash);
    let valid = tokio::task::spawn_blocking(move || crypto::verify(&d.password, &hash))
        .await
        .map_err(|e| anyhow::anyhow!(e))?;
    if !valid || r.is_none() {
        return Err(Error::unauthorized());
    }
    let r = r.unwrap();
    let id: i64 = r.get("id");
    let mfa = r.get::<i16, _>("two_factor_enabled") == 1;
    let token = crypto::token();
    let csrf = crypto::token();
    let method: String = r.get("two_factor_method");
    if mfa && method == "email" {
        send_otp(&s, id, &email).await?
    }
    sqlx::query("INSERT INTO panel_sessions(id,user_id,ip,device,last_seen_at,expires_at,csrf,password_fingerprint,authenticated) VALUES($1,$2,'',$3,now(),now()+CASE WHEN $6 THEN interval '12 hours' ELSE interval '10 minutes' END,$4,$5,$6)").bind(crypto::hash(&token)).bind(id).bind(h.get("user-agent").and_then(|v|v.to_str().ok()).unwrap_or("").chars().take(255).collect::<String>()).bind(&csrf).bind(fp).bind(!mfa).execute(&s.db).await?;
    audit(&s, Some(id), "login_password", json!({"mfa":mfa})).await?;
    Ok((
        [(header::SET_COOKIE, session_cookie(&s, &token))],
        Json(json!({"ok":true,"mfa":mfa,"csrf":csrf,"method":method})),
    )
        .into_response())
}
async fn send_otp(s: &App, id: i64, email: &str) -> Result<()> {
    use rand::Rng;
    if s.config.smtp_host.is_empty() {
        return Err(Error(
            StatusCode::SERVICE_UNAVAILABLE,
            "SMTP chưa được cấu hình".into(),
        ));
    }
    let code = format!("{:06}", rand::thread_rng().gen_range(0..1_000_000));
    sqlx::query("UPDATE login_otps SET consumed_at=now() WHERE user_id=$1 AND consumed_at IS NULL")
        .bind(id)
        .execute(&s.db)
        .await?;
    sqlx::query("INSERT INTO login_otps(user_id,code_hash,expires_at) VALUES($1,$2,now()+interval '5 minutes')").bind(id).bind(crypto::hash(&code)).execute(&s.db).await?;
    crate::worker::notify(
        s,
        id,
        email,
        "Mã đăng nhập",
        &format!("Mã xác thực: {code}. Hết hạn sau 5 phút."),
    )
    .await?;
    Ok(())
}
#[derive(Deserialize)]
pub struct Code {
    code: String,
}
pub fn totp(secret: &str, code: &str, time: i64) -> bool {
    totp_step(secret, code, time).is_some()
}
fn totp_step(secret: &str, code: &str, time: i64) -> Option<i64> {
    use hmac::{Hmac, Mac};
    let bytes = data_encoding::BASE32_NOPAD.decode(secret.as_bytes()).ok()?;
    (-1..=1).map(|delta| time / 30 + delta).find(|step| {
        if *step < 0 {
            return false;
        }
        let mut mac = Hmac::<sha1::Sha1>::new_from_slice(&bytes).unwrap();
        mac.update(&(*step as u64).to_be_bytes());
        let h = mac.finalize().into_bytes();
        let o = (h[19] & 15) as usize;
        let n = (u32::from_be_bytes(h[o..o + 4].try_into().unwrap()) & 0x7fff_ffff) % 1_000_000;
        crypto::equal(&format!("{n:06}"), code)
    })
}

pub async fn verify_mfa(
    State(s): State<App>,
    h: HeaderMap,
    Json(d): Json<Code>,
) -> Result<Response> {
    origin(&s, &h)?;
    let u = user(&s, &h, false).await?;
    if !crypto::equal(
        &u.csrf,
        h.get("x-csrf-token")
            .and_then(|v| v.to_str().ok())
            .unwrap_or(""),
    ) {
        return Err(Error::forbidden());
    }
    limit(&s, &format!("mfa:{}", u.id)).await?;
    let r = sqlx::query(
        "SELECT two_factor_method,two_factor_secret,password_hash FROM users WHERE id=$1",
    )
    .bind(u.id)
    .fetch_one(&s.db)
    .await?;
    let method: String = r.get("two_factor_method");
    let mut matched_step = None;
    let valid = if method == "totp" {
        let sec = r
            .get::<Option<String>, _>("two_factor_secret")
            .ok_or_else(Error::unauthorized)?;
        matched_step = totp_step(
            &crypto::open(&s.config.key, &sec)?,
            &d.code,
            Utc::now().timestamp(),
        );
        matched_step.is_some()
    } else {
        sqlx::query("UPDATE login_otps SET consumed_at=now() WHERE user_id=$1 AND code_hash=$2 AND expires_at>now() AND consumed_at IS NULL").bind(u.id).bind(crypto::hash(&d.code)).execute(&s.db).await?.rows_affected()>0
    };
    if !valid {
        return Err(Error::unauthorized());
    }
    if let Some(step) = matched_step {
        let changed =
            sqlx::query("UPDATE users SET totp_last_step=$1 WHERE id=$2 AND totp_last_step<$1")
                .bind(step)
                .bind(u.id)
                .execute(&s.db)
                .await?
                .rows_affected();
        if changed == 0 {
            return Err(Error::bad("Mã đã dùng; chờ mã tiếp theo"));
        }
    }
    let token = crypto::token();
    sqlx::query("UPDATE panel_sessions SET id=$1,authenticated=true,expires_at=now()+interval '12 hours' WHERE id=$2").bind(crypto::hash(&token)).bind(u.session).execute(&s.db).await?;
    Ok((
        [(header::SET_COOKIE, session_cookie(&s, &token))],
        Json(json!({"ok":true})),
    )
        .into_response())
}
pub async fn me(State(s): State<App>, h: HeaderMap) -> Result<Json<Value>> {
    let u = user(&s, &h, true).await?;
    let info:Value=sqlx::query_scalar("SELECT jsonb_build_object('id',id,'name',name,'email',email,'role',role,'two_factor_enabled',two_factor_enabled,'two_factor_method',two_factor_method) FROM users WHERE id=$1").bind(u.id).fetch_one(&s.db).await?;
    Ok(Json(
        json!({"user":info,"csrf":u.csrf,"permissions":u.permissions,"admin_level":u.level}),
    ))
}
pub async fn logout(State(s): State<App>, h: HeaderMap) -> Result<Response> {
    let u = user(&s, &h, true).await?;
    origin(&s, &h)?;
    sqlx::query("DELETE FROM panel_sessions WHERE id=$1")
        .bind(u.session)
        .execute(&s.db)
        .await?;
    Ok((
        [(
            header::SET_COOKIE,
            "ui_session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0",
        )],
        Json(json!({"ok":true})),
    )
        .into_response())
}
#[derive(Deserialize)]
pub struct Security {
    action: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    new_password: String,
    #[serde(default)]
    code: String,
    #[serde(default)]
    session_id: String,
}
pub async fn security(
    State(s): State<App>,
    h: HeaderMap,
    Json(d): Json<Security>,
) -> Result<Json<Value>> {
    let u = user(&s, &h, true).await?;
    origin(&s, &h)?;
    if !crypto::equal(
        &u.csrf,
        h.get("x-csrf-token")
            .and_then(|v| v.to_str().ok())
            .unwrap_or(""),
    ) {
        return Err(Error::forbidden());
    }
    let r = sqlx::query(
        "SELECT password_hash,email,two_factor_secret,two_factor_enabled FROM users WHERE id=$1",
    )
    .bind(u.id)
    .fetch_one(&s.db)
    .await?;
    if !crypto::verify(&d.password, &r.get::<String, _>("password_hash")) {
        return Err(Error::unauthorized());
    }
    match d.action.as_str() {
        "password" => {
            let hash = crypto::password(&d.new_password)?;
            let mut tx = s.db.begin().await?;
            sqlx::query("UPDATE users SET password_hash=$1,updated_at=now() WHERE id=$2")
                .bind(hash)
                .bind(u.id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM panel_sessions WHERE user_id=$1")
                .bind(u.id)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
        }
        "totp_setup" => {
            if r.get::<i16, _>("two_factor_enabled") == 1 {
                return Err(Error::bad("Tắt 2FA hiện tại trước khi cấu hình lại"));
            }
            let raw = hex::decode(crypto::token()).unwrap();
            let secret = data_encoding::BASE32_NOPAD.encode(&raw[..20]);
            sqlx::query("UPDATE users SET two_factor_secret=$1 WHERE id=$2")
                .bind(crypto::seal(&s.config.key, &secret)?)
                .bind(u.id)
                .execute(&s.db)
                .await?;
            return Ok(Json(
                json!({"secret":secret,"email":r.get::<String,_>("email")}),
            ));
        }
        "totp_confirm" => {
            let secret = r
                .get::<Option<String>, _>("two_factor_secret")
                .ok_or_else(|| Error::bad("Chưa cấu hình TOTP"))?;
            if !totp(
                &crypto::open(&s.config.key, &secret)?,
                &d.code,
                Utc::now().timestamp(),
            ) {
                return Err(Error::bad("Mã không hợp lệ"));
            }
            sqlx::query(
                "UPDATE users SET two_factor_enabled=1,two_factor_method='totp' WHERE id=$1",
            )
            .bind(u.id)
            .execute(&s.db)
            .await?;
        }
        "email_enable" => {
            if s.config.smtp_host.is_empty() {
                return Err(Error::bad("SMTP chưa được cấu hình"));
            }
            sqlx::query(
                "UPDATE users SET two_factor_enabled=1,two_factor_method='email' WHERE id=$1",
            )
            .bind(u.id)
            .execute(&s.db)
            .await?;
        }
        "revoke" => {
            sqlx::query("UPDATE panel_sessions SET revoked_at=now() WHERE id=$1 AND user_id=$2")
                .bind(d.session_id)
                .bind(u.id)
                .execute(&s.db)
                .await?;
        }
        "disable_2fa" => {
            if r.get::<i16, _>("two_factor_enabled") == 1 {
                let secret = r.get::<Option<String>, _>("two_factor_secret");
                let valid = if let Some(sec) = secret {
                    totp(
                        &crypto::open(&s.config.key, &sec)?,
                        &d.code,
                        Utc::now().timestamp(),
                    )
                } else {
                    false
                };
                if !valid {
                    return Err(Error::bad(
                        "Cần mã TOTP hợp lệ để tắt; 2FA email cần quản trị hỗ trợ",
                    ));
                }
            }
            sqlx::query("UPDATE users SET two_factor_enabled=0,two_factor_secret=NULL WHERE id=$1")
                .bind(u.id)
                .execute(&s.db)
                .await?;
        }
        _ => return Err(Error::bad("Thao tác không hợp lệ")),
    }
    audit(
        &s,
        Some(u.id),
        "security_change",
        json!({"action":d.action}),
    )
    .await?;
    Ok(Json(json!({"ok":true})))
}
pub async fn sessions(State(s): State<App>, h: HeaderMap) -> Result<Json<Value>> {
    let u = user(&s, &h, true).await?;
    let rows:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',id,'device',device,'created_at',created_at,'expires_at',expires_at,'revoked_at',revoked_at) FROM panel_sessions WHERE user_id=$1 ORDER BY created_at DESC LIMIT 100").bind(u.id).fetch_all(&s.db).await?;
    Ok(Json(json!(rows)))
}
#[derive(Deserialize)]
pub struct Reset {
    #[serde(default)]
    email: String,
    #[serde(default)]
    token: String,
    #[serde(default)]
    password: String,
}
pub async fn forgot(
    State(s): State<App>,
    h: HeaderMap,
    Json(d): Json<Reset>,
) -> Result<Json<Value>> {
    origin(&s, &h)?;
    let email = d.email.trim().to_lowercase();
    limit(&s, &format!("reset:{email}")).await?;
    if let Some(id) = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM users WHERE lower(email)=$1 AND status='active'",
    )
    .bind(&email)
    .fetch_optional(&s.db)
    .await?
    {
        if !s.config.smtp_host.is_empty() {
            let t = crypto::token();
            sqlx::query("INSERT INTO rust_reset_tokens(token_hash,user_id,expires_at) VALUES($1,$2,now()+interval '30 minutes')").bind(crypto::hash(&t)).bind(id).execute(&s.db).await?;
            crate::worker::notify(
                &s,
                id,
                &email,
                "Đặt lại mật khẩu",
                &format!("{}/reset-password#{}", s.config.app_url, t),
            )
            .await?;
        }
    }
    Ok(Json(
        json!({"ok":true,"message":"Nếu tài khoản tồn tại, hướng dẫn sẽ được gửi qua email."}),
    ))
}
pub async fn reset(
    State(s): State<App>,
    h: HeaderMap,
    Json(d): Json<Reset>,
) -> Result<Json<Value>> {
    origin(&s, &h)?;
    let hash = crypto::password(&d.password)?;
    let mut tx = s.db.begin().await?;
    let id=sqlx::query_scalar::<_,i64>("UPDATE rust_reset_tokens SET used_at=now() WHERE token_hash=$1 AND used_at IS NULL AND expires_at>now() RETURNING user_id").bind(crypto::hash(&d.token)).fetch_optional(&mut *tx).await?.ok_or_else(||Error::bad("Liên kết hết hạn hoặc không hợp lệ"))?;
    sqlx::query("UPDATE users SET password_hash=$1,updated_at=now() WHERE id=$2")
        .bind(hash)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM panel_sessions WHERE user_id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM rust_reset_tokens WHERE user_id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({"ok":true})))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn totp_rfc_vector() {
        assert_eq!(
            totp_step("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ", "287082", 59),
            Some(1)
        );
        assert!(!totp("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ", "000000", 59));
    }
}
