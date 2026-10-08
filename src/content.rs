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
    #[serde(default)]
    pub lang: String,
}
pub async fn public(State(s): State<App>, Query(mut l): Query<Language>) -> Result<Json<Value>> {
    let default: String =
        sqlx::query_scalar("SELECT value FROM settings WHERE key='site_language'")
            .fetch_one(&s.db)
            .await?;
    let enabled: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM languages WHERE code=$1 AND enabled=1)")
            .bind(&l.lang)
            .fetch_one(&s.db)
            .await?;
    if !enabled {
        l.lang = default;
    }
    let languages:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('code',code,'name',name,'native_name',native_name) FROM languages WHERE enabled=1 ORDER BY code").fetch_all(&s.db).await?;
    let rows:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('section',section,'content',content) FROM page_content WHERE language=$1 ORDER BY section").bind(&l.lang).fetch_all(&s.db).await?;
    let overrides: Value = sqlx::query_scalar(
        "SELECT COALESCE(jsonb_object_agg(key,value),'{}') FROM translations WHERE language=$1",
    )
    .bind(&l.lang)
    .fetch_one(&s.db)
    .await?;
    let mut translations = crate::languages::catalog(&l.lang);
    translations
        .as_object_mut()
        .unwrap()
        .extend(overrides.as_object().unwrap().clone());
    let settings:Value=sqlx::query_scalar("SELECT COALESCE(jsonb_object_agg(key,value),'{}') FROM settings WHERE key IN ('landing_brand_name','company_name','contact_email','contact_phone','webmail_url','support_url','trial_require_identity','contact_address','site_meta_description')").fetch_one(&s.db).await?;
    Ok(Json(
        json!({"sections":rows,"settings":settings,"translations":translations,"language":l.lang,"languages":languages}),
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
                    "contact_address",
                    "site_meta_description",
                    "trial_require_identity",
                    "trial_retention_days",
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
                if val.chars().count() > 2000 || val.chars().any(|c| c.is_control()) {
                    return Err(Error::bad("Giá trị cấu hình không hợp lệ"));
                }
                if k == "trial_require_identity" && !["0", "1"].contains(&val) {
                    return Err(Error::bad("Chọn 0 hoặc 1"));
                }
                if k == "trial_retention_days"
                    && !val.parse::<i32>().is_ok_and(|n| (1..=3650).contains(&n))
                {
                    return Err(Error::bad("Thời gian lưu từ 1 đến 3650 ngày"));
                }
                if k.ends_with("_url")
                    && !val.is_empty()
                    && !(val.starts_with('/') && !val.starts_with("//") && !val.contains('\\'))
                {
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
            validate_section(section, &v["content"])?;
            sqlx::query("INSERT INTO page_content(language,section,content) VALUES($1,$2,$3) ON CONFLICT(language,section) DO UPDATE SET content=EXCLUDED.content,updated_at=now()").bind(lang).bind(section).bind(&v["content"]).execute(&mut *tx).await?;
        }
        _ => return Err(Error::bad("Loại nội dung không hợp lệ")),
    }
    tx.commit().await?;
    Ok(Json(json!({"ok":true})))
}

pub fn validate_section(section: &str, v: &Value) -> Result<()> {
    if ![
        "seo", "hero", "about", "benefits", "pricing", "features", "process", "apps", "faq", "cta",
        "contact", "footer",
    ]
    .contains(&section)
        || !v.is_object()
    {
        return Err(Error::bad("Mục trang chủ không hợp lệ"));
    }
    for (key, value) in v.as_object().unwrap() {
        if ["visible", "enabled"].contains(&key.as_str()) {
            if !value.is_boolean() {
                return Err(Error::bad("Trạng thái hiển thị phải là boolean"));
            }
            continue;
        }
        if key == "plan_labels" {
            let labels = value
                .as_object()
                .filter(|o| o.len() <= 150)
                .ok_or_else(|| Error::bad("Tên gói không hợp lệ"))?;
            for (id, label) in labels {
                if id.parse::<i64>().is_err()
                    || label.as_str().is_none_or(|s| s.chars().count() > 120)
                {
                    return Err(Error::bad("Tên gói không hợp lệ"));
                }
            }
            continue;
        }
        if key == "items" {
            let max = match section {
                "benefits" | "apps" => 8,
                "features" | "faq" => 12,
                "process" => 6,
                _ => 0,
            };
            let items = value
                .as_array()
                .filter(|a| a.len() <= max)
                .ok_or_else(|| Error::bad("Số mục vượt giới hạn"))?;
            for item in items {
                let item = item
                    .as_object()
                    .ok_or_else(|| Error::bad("Nội dung không hợp lệ"))?;
                for (key, val) in item {
                    check_text(val, 1400)?;
                    if key == "url" {
                        let url = url::Url::parse(val.as_str().unwrap())
                            .map_err(|_| Error::bad("Link tải không hợp lệ"))?;
                        if url.scheme() != "https"
                            || !url.username().is_empty()
                            || url.password().is_some()
                        {
                            return Err(Error::bad("Link tải phải dùng HTTPS"));
                        }
                    }
                }
            }
            continue;
        }
        if ![
            "badge",
            "title",
            "description",
            "primary_label",
            "secondary_label",
            "button_label",
            "note",
        ]
        .contains(&key.as_str())
        {
            return Err(Error::bad("Trường nội dung không hợp lệ"));
        }
        check_text(value, 1400)?;
    }
    Ok(())
}
fn check_text(v: &Value, max: usize) -> Result<()> {
    if v.as_str().is_none_or(|s| {
        s.chars().count() > max
            || s.chars()
                .any(|c| c.is_control() && !['\n', '\r', '\t'].contains(&c))
    }) {
        return Err(Error::bad("Nội dung không hợp lệ hoặc quá dài"));
    }
    Ok(())
}
