pub mod auth;
pub mod billing;
pub mod config;
pub mod content;
pub mod crypto;
pub mod dns;
pub mod error;
pub mod operations;
pub mod resources;
pub mod stalwart;
pub mod worker;
use axum::{
    extract::{DefaultBodyLimit, State},
    http::{header, HeaderValue},
    response::Html,
    routing::{get, post},
    Router,
};
use tower_http::{services::ServeDir, set_header::SetResponseHeaderLayer, trace::TraceLayer};
#[derive(Clone)]
pub struct App {
    pub db: sqlx::PgPool,
    pub config: config::Config,
    pub http: reqwest::Client,
    pub dummy_hash: String,
}
pub async fn state(config: config::Config) -> anyhow::Result<App> {
    let db = sqlx::postgres::PgPoolOptions::new()
        .max_connections(20)
        .connect(&config.database_url)
        .await?;
    sqlx::migrate!().run(&db).await?;
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    Ok(App {
        db,
        config,
        http,
        dummy_hash: crypto::password(&crypto::token())?,
    })
}
pub fn router(s: App) -> Router {
    Router::new()
 .route("/healthz",get(||async{"ok"}))
 .route("/readyz",get(|State(s):State<App>|async move{sqlx::query("SELECT 1").execute(&s.db).await.map(|_|"ok").map_err(error::Error::from)}))
 .route("/api/auth/register",post(auth::register)).route("/api/auth/login",post(auth::login)).route("/api/auth/mfa",post(auth::verify_mfa)).route("/api/auth/me",get(auth::me)).route("/api/auth/logout",post(auth::logout)).route("/api/auth/security",post(auth::security)).route("/api/auth/sessions",get(auth::sessions)).route("/api/auth/forgot",post(auth::forgot)).route("/api/auth/reset",post(auth::reset))
 .route("/api/packages",get(billing::packages)).route("/api/quote",post(billing::pricing)).route("/api/orders",post(billing::order)).route("/api/invoices/{id}",get(billing::invoice)).route("/api/subscriptions/{id}",post(billing::manual)).route("/sepay/webhook",post(billing::webhook).get(||async{axum::Json(serde_json::json!({"service":"SePay webhook","method":"POST"}))}))
 .route("/api/domains/{id}/dns",get(dns::instructions))
 .route("/api/domains/{id}/verify",post(dns::verify))
 .route("/api/dashboard",get(resources::dashboard)).route("/api/resources/{kind}",get(resources::list).post(resources::save))
 .route("/api/content",get(content::public).post(content::save)).route("/api/trials",post(operations::trial)).route("/api/trials/{id}",post(operations::trial_review))
 .route("/api/operations/read",post(operations::read)).route("/api/operations/preview",post(operations::preview)).route("/api/operations/execute/{id}",post(operations::execute))
 .nest_service("/assets",ServeDir::new("static"))
 .fallback(||async{Html(include_str!("../static/index.html"))})
 .layer(DefaultBodyLimit::max(65536)).layer(TraceLayer::new_for_http())
 .layer(SetResponseHeaderLayer::if_not_present(header::CACHE_CONTROL,HeaderValue::from_static("no-store")))
 .layer(SetResponseHeaderLayer::if_not_present(header::X_CONTENT_TYPE_OPTIONS,HeaderValue::from_static("nosniff")))
 .layer(SetResponseHeaderLayer::if_not_present(header::REFERRER_POLICY,HeaderValue::from_static("same-origin")))
 .layer(SetResponseHeaderLayer::if_not_present(header::CONTENT_SECURITY_POLICY,HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' https://qr.sepay.vn; frame-ancestors 'none'; base-uri 'self'; form-action 'self'")))
 .with_state(s)
}
