use crate::{
    auth,
    error::{Error, Result},
    App,
};
use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
#[derive(Deserialize)]
pub struct Language {
    #[serde(default = "vi")]
    pub lang: String,
}
fn vi() -> String {
    "vi".into()
}
pub async fn public(State(s): State<App>, Query(l): Query<Language>) -> Result<Json<Value>> {
    let rows:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('section',section,'content',content) FROM page_content WHERE language=$1 ORDER BY section").bind(&l.lang).fetch_all(&s.db).await?;
    let translations: Value = sqlx::query_scalar(
        "SELECT COALESCE(jsonb_object_agg(key,value),'{}') FROM translations WHERE language=$1",
    )
    .bind(&l.lang)
    .fetch_one(&s.db)
    .await?;
    let settings:Value=sqlx::query_scalar("SELECT COALESCE(jsonb_object_agg(key,value),'{}') FROM settings WHERE key IN ('landing_brand_name','company_name','contact_email','contact_phone','webmail_url','support_url')").fetch_one(&s.db).await?;
    Ok(Json(
        json!({"sections":rows,"settings":settings,"translations":translations}),
    ))
}
pub async fn save(State(s): State<App>, h: HeaderMap, Json(v): Json<Value>) -> Result<Json<Value>> {
    let _u = auth::require(&s, &h, "settings", true, true).await?;
    let lang = v["language"].as_str().unwrap_or("vi");
    if lang.len() > 20 {
        return Err(Error::bad("Mã ngôn ngữ quá dài"));
    }
    let mut tx = s.db.begin().await?;
    match v["kind"].as_str().unwrap_or("section") {
        "translations" => {
            let entries = v["entries"]
                .as_object()
                .ok_or_else(|| Error::bad("entries phải là object"))?;
            for (k, val) in entries {
                let val = val
                    .as_str()
                    .ok_or_else(|| Error::bad("Bản dịch phải là văn bản"))?;
                sqlx::query("INSERT INTO translations(language,key,value) VALUES($1,$2,$3) ON CONFLICT(language,key) DO UPDATE SET value=EXCLUDED.value").bind(lang).bind(k).bind(val).execute(&mut *tx).await?;
            }
        }
        "settings" => {
            let entries = v["entries"]
                .as_object()
                .ok_or_else(|| Error::bad("entries phải là object"))?;
            for (k, val) in entries {
                if ![
                    "landing_brand_name",
                    "company_name",
                    "contact_email",
                    "contact_phone",
                    "webmail_url",
                    "support_url",
                ]
                .contains(&k.as_str())
                {
                    return Err(Error::bad(
                        "Cấu hình không được hỗ trợ; bí mật được cấu hình bằng biến môi trường",
                    ));
                }
                let val = val
                    .as_str()
                    .ok_or_else(|| Error::bad("Giá trị phải là văn bản"))?;
                if k.ends_with("_url") && !val.is_empty() {
                    let url = url::Url::parse(val).map_err(|_| Error::bad("URL không hợp lệ"))?;
                    if !["http", "https"].contains(&url.scheme()) {
                        return Err(Error::bad("URL phải dùng HTTP/HTTPS"));
                    }
                }
                sqlx::query("INSERT INTO settings(key,value) VALUES($1,$2) ON CONFLICT(key) DO UPDATE SET value=EXCLUDED.value,updated_at=now()").bind(k).bind(val).execute(&mut *tx).await?;
            }
        }
        "section" => {
            let section = crate::resources::text(&v, "section", 80)?;
            sqlx::query("INSERT INTO page_content(language,section,content) VALUES($1,$2,$3) ON CONFLICT(language,section) DO UPDATE SET content=EXCLUDED.content,updated_at=now()").bind(lang).bind(section).bind(&v["content"]).execute(&mut *tx).await?;
        }
        _ => return Err(Error::bad("Loại nội dung không hợp lệ")),
    }
    tx.commit().await?;
    Ok(Json(json!({"ok":true})))
}
