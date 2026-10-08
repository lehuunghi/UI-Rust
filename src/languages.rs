use crate::{
    auth,
    error::{Error, Result},
    resources, App,
};
use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

pub fn catalog(code: &str) -> Value {
    serde_json::from_str(match code {
        "en" => include_str!("../locales/en.json"),
        "vi" => include_str!("../locales/vi.json"),
        _ => "{}",
    })
    .expect("validated bundled catalog")
}
pub fn valid_code(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= 20
        && code
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && code.as_bytes()[0].is_ascii_lowercase()
}
#[derive(Deserialize)]
pub struct Page {
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub offset: i64,
    #[serde(default)]
    pub q: String,
}
pub async fn list(
    State(s): State<App>,
    h: HeaderMap,
    Query(p): Query<Page>,
) -> Result<Json<Value>> {
    auth::require(&s, &h, "languages", true, false).await?;
    let langs: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(l) FROM languages l ORDER BY code")
        .fetch_all(&s.db)
        .await?;
    let default: String =
        sqlx::query_scalar("SELECT value FROM settings WHERE key='site_language'")
            .fetch_one(&s.db)
            .await?;
    let code = if p.code.is_empty() {
        default.clone()
    } else {
        p.code
    };
    if !langs.iter().any(|l| l["code"] == code) {
        return Err(Error::missing());
    }
    let base = catalog(&code);
    let keys = catalog("vi");
    let sql="WITH entries AS (SELECT k.key,COALESCE(t.value,b.value,'') value FROM (SELECT key FROM jsonb_each_text($1) UNION SELECT key FROM translations WHERE language=$3) k LEFT JOIN jsonb_each_text($2) b ON b.key=k.key LEFT JOIN translations t ON t.key=k.key AND t.language=$3) SELECT jsonb_build_object('key',key,'value',value) FROM entries WHERE $4='' OR key ILIKE '%'||$4||'%' OR value ILIKE '%'||$4||'%' ORDER BY key LIMIT 80 OFFSET $5";
    let entries: Vec<Value> = sqlx::query_scalar(sql)
        .bind(keys)
        .bind(base)
        .bind(&code)
        .bind(p.q.chars().take(200).collect::<String>())
        .bind(p.offset.clamp(0, 100_000))
        .fetch_all(&s.db)
        .await?;
    Ok(Json(
        json!({"languages":langs,"default":default,"code":code,"entries":entries,"offset":p.offset,"next":if entries.len()==80 {Some(p.offset+80)}else{None}}),
    ))
}
pub async fn save(State(s): State<App>, h: HeaderMap, Json(v): Json<Value>) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "languages", true, true).await?;
    let code = resources::text(&v, "code", 20)?;
    if !valid_code(code) {
        return Err(Error::bad("Mã ngôn ngữ không hợp lệ"));
    }
    let mut tx = s.db.begin().await?;
    let default: String =
        sqlx::query_scalar("SELECT value FROM settings WHERE key='site_language' FOR UPDATE")
            .fetch_one(&mut *tx)
            .await?;
    match v["action"].as_str().unwrap_or("entries") {
        "metadata" => {
            let enabled = if v["enabled"] == false { 0_i16 } else { 1 };
            if enabled == 0 && code == default {
                return Err(Error::bad("Không thể tắt ngôn ngữ mặc định"));
            }
            sqlx::query("INSERT INTO languages(code,name,native_name,enabled) VALUES($1,$2,$3,$4) ON CONFLICT(code) DO UPDATE SET name=EXCLUDED.name,native_name=EXCLUDED.native_name,enabled=EXCLUDED.enabled,updated_at=now()").bind(code).bind(resources::text(&v,"name",100)?).bind(resources::text(&v,"native_name",100)?).bind(enabled).execute(&mut *tx).await?;
        }
        "default" => {
            let enabled: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM languages WHERE code=$1 AND enabled=1)",
            )
            .bind(code)
            .fetch_one(&mut *tx)
            .await?;
            if !enabled {
                return Err(Error::bad("Ngôn ngữ mặc định phải đang bật"));
            }
            sqlx::query("UPDATE settings SET value=$1,updated_at=now() WHERE key='site_language'")
                .bind(code)
                .execute(&mut *tx)
                .await?;
        }
        "delete" => {
            if code == default {
                return Err(Error::bad("Không thể xóa ngôn ngữ mặc định"));
            }
            for table in ["translations", "page_content"] {
                sqlx::query(&format!("DELETE FROM {table} WHERE language=$1"))
                    .bind(code)
                    .execute(&mut *tx)
                    .await?;
            }
            sqlx::query("DELETE FROM languages WHERE code=$1")
                .bind(code)
                .execute(&mut *tx)
                .await?;
        }
        "entries" => {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM languages WHERE code=$1)")
                    .bind(code)
                    .fetch_one(&mut *tx)
                    .await?;
            if !exists {
                return Err(Error::missing());
            }
            let entries = v["entries"]
                .as_object()
                .filter(|e| e.len() <= 80)
                .ok_or_else(|| Error::bad("Mỗi lần lưu tối đa 80 bản dịch"))?;
            for (key, value) in entries {
                let value = value
                    .as_str()
                    .filter(|s| s.chars().count() <= 10_000)
                    .ok_or_else(|| Error::bad("Bản dịch không hợp lệ"))?;
                if key.is_empty() || key.len() > 2000 || key.starts_with('_') {
                    return Err(Error::bad("Khóa bản dịch không hợp lệ"));
                }
                sqlx::query("INSERT INTO translations(language,key,value) VALUES($1,$2,$3) ON CONFLICT(language,key) DO UPDATE SET value=EXCLUDED.value").bind(code).bind(key).bind(value).execute(&mut *tx).await?;
            }
        }
        _ => return Err(Error::bad("Thao tác không hợp lệ")),
    }
    sqlx::query("INSERT INTO audit_logs(user_id,action,context) VALUES($1,'language_update',$2)")
        .bind(u.id)
        .bind(json!({"code":code,"action":v["action"]}))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({"ok":true})))
}
