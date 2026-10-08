pub mod auth;
pub mod backup;
pub mod backup_admin;
pub mod backup_tools;
pub mod billing;
pub mod bulk;
pub mod config;
pub mod content;
pub mod crypto;
pub mod dns;
pub mod emergency_dkim;
pub mod error;
pub mod languages;
pub mod mail_dns;
pub mod mailbox_backup;
pub mod management;
pub mod migration;
pub mod monitor;
pub mod native_plan;
pub mod notifications;
pub mod operations;
pub mod placement;
pub mod reconciliation;
pub mod recovery;
pub mod recovery_sources;
pub mod reports;
pub mod resources;
pub mod sepay;
pub mod stalwart;
pub mod tenants;
pub mod trials;
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
 .route("/api/auth/impersonate",post(auth::impersonate)).route("/api/auth/impersonation/stop",post(auth::stop_impersonation))
 .route("/api/backups",get(backup_admin::list).post(backup_admin::request)).route("/api/backups/policy",post(backup_admin::save_policy)).route("/api/backups/{id}/download",get(backup_admin::download)).route("/api/backups/{id}/restore",post(backup_admin::restore)).route("/api/backups/{id}/retrieve",post(backup_admin::request_retrieval))
 .route("/api/email-templates",get(notifications::list).post(notifications::save)).route("/api/email-broadcast",post(notifications::broadcast))
 .route("/api/payments/import",post(sepay::import))
 .route("/api/languages",get(languages::list).post(languages::save))
 .route("/api/accounts/import",post(bulk::import)).route("/api/resources/{kind}/export",get(bulk::export))
 .route("/api/trials/{id}/documents/{kind}",get(trials::download))
 .route("/api/packages",get(billing::packages)).route("/api/quote",post(billing::pricing)).route("/api/orders",post(billing::order)).route("/api/invoices/{id}",get(billing::invoice)).route("/api/payments/{id}",post(billing::reconcile)).route("/api/subscriptions/{id}",post(billing::manual)).route("/sepay/webhook",post(billing::webhook).get(||async{axum::Json(serde_json::json!({"service":"SePay webhook","method":"POST"}))}))
 .route("/api/domains/{id}/mail-dns",post(mail_dns::inspect))
 .route("/api/domains/{id}/dns",get(dns::instructions))
 .route("/api/domains/{id}/verify",post(dns::verify))
 .route("/api/dashboard",get(resources::dashboard)).route("/api/resources/{kind}",get(resources::list).post(resources::save))
 .route("/api/content",get(content::public).post(content::save)).route("/api/trials",post(trials::submit).layer(DefaultBodyLimit::max(15*1024*1024))).route("/api/trials/public",post(trials::public_submit).layer(DefaultBodyLimit::max(15*1024*1024))).route("/api/trials/{id}",post(operations::trial_review))
 .route("/api/reconciliation",post(reconciliation::preview)).route("/api/reconciliation/{id}",get(reconciliation::list)).route("/api/reconciliation/{id}/apply",post(reconciliation::apply))
 .route("/api/placements",get(placement::list).post(placement::save))
 .route("/api/emergency-dkim",get(emergency_dkim::list).post(emergency_dkim::create)).route("/api/emergency-dkim/{id}/apply",post(emergency_dkim::apply))
 .route("/api/recovery",get(recovery::list).post(recovery::save).layer(DefaultBodyLimit::max(2*1024*1024))).route("/api/recovery/jobs",post(recovery::queue)).route("/api/recovery/jobs/{id}/attest",post(recovery::attest)).route("/api/recovery/artifacts/{id}/download",get(recovery::download))
 .route("/api/mailbox-backups",get(mailbox_backup::list).post(mailbox_backup::save)).route("/api/mailbox-backups/jobs",post(mailbox_backup::queue)).route("/api/mailbox-backups/artifacts/{id}/download",get(mailbox_backup::download))
 .route("/api/monitor",get(monitor::history)).route("/api/servers/{id}/check",post(monitor::check)).route("/api/servers/{id}/monitor",post(monitor::configure))
 .route("/api/reports",get(reports::list)).route("/api/reports/import",post(reports::import).layer(DefaultBodyLimit::max(12*1024*1024)))
 .route("/api/tenant-bindings",get(tenants::list).post(tenants::bind))
 .route("/api/migrations/preview",post(migration::preview)).route("/api/migrations/{id}/apply",post(migration::apply))
 .route("/api/management/list",post(management::list)).route("/api/management/preview",post(management::preview)).route("/api/management/secrets/{id}",post(management::reveal))
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
