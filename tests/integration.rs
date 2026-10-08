use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use tower::ServiceExt;
use ui_rust::{config::Config, crypto, App};
struct Client {
    cookie: String,
    csrf: String,
}
async fn request(
    app: &Router,
    path: &str,
    data: Option<Value>,
    c: Option<&Client>,
) -> (StatusCode, Value, String) {
    let mut r = Request::builder()
        .uri(path)
        .header("origin", "http://localhost:8080");
    if let Some(c) = c {
        r = r
            .header("cookie", &c.cookie)
            .header("x-csrf-token", &c.csrf)
    }
    let body = if let Some(d) = data {
        r = r.method("POST").header("content-type", "application/json");
        Body::from(d.to_string())
    } else {
        Body::empty()
    };
    let r = app.clone().oneshot(r.body(body).unwrap()).await.unwrap();
    let status = r.status();
    let cookie = r
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .to_owned();
    let bytes = to_bytes(r.into_body(), 1_000_000).await.unwrap();
    let value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({"body":String::from_utf8_lossy(&bytes)}));
    (status, value, cookie)
}
async fn login(app: &Router, email: &str) -> Client {
    let (status, r, cookie) = request(
        app,
        "/api/auth/login",
        Some(json!({"email":email,"password":"Strong-test-password-123"})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{r}");
    Client {
        cookie,
        csrf: r["csrf"].as_str().unwrap().into(),
    }
}
fn config() -> Config {
    Config {
        database_url: String::new(),
        bind: "127.0.0.1:8080".into(),
        app_url: "http://localhost:8080".into(),
        key: [23; 32],
        secure_cookie: false,
        sepay_key: "fixture-webhook-key".into(),
        sepay_hmac: String::new(),
        sepay_account: "123456789".into(),
        sepay_bank: "VCB".into(),
        smtp_host: String::new(),
        smtp_user: String::new(),
        smtp_password: String::new(),
        smtp_from: String::new(),
    }
}
#[sqlx::test]
async fn tenant_quotas_sessions_and_payments(pool: sqlx::PgPool) {
    let hash = crypto::password("Strong-test-password-123").unwrap();
    let admin:i64=sqlx::query_scalar("INSERT INTO users(name,email,password_hash,role,admin_level,two_factor_enabled) VALUES('Admin','admin@test.example',$1,'admin','super',0) RETURNING id").bind(&hash).fetch_one(&pool).await.unwrap();
    let a:i64=sqlx::query_scalar("INSERT INTO users(name,email,password_hash,two_factor_enabled) VALUES('A','a@test.example',$1,0) RETURNING id").bind(&hash).fetch_one(&pool).await.unwrap();
    sqlx::query("INSERT INTO users(name,email,password_hash,two_factor_enabled) VALUES('B','b@test.example',$1,0)").bind(&hash).execute(&pool).await.unwrap();
    let state = App {
        db: pool.clone(),
        config: config(),
        http: reqwest::Client::new(),
        dummy_hash: hash,
    };
    let app = ui_rust::router(state.clone());
    let admin_client = login(&app, "admin@test.example").await;
    let ca = login(&app, "a@test.example").await;
    let cb = login(&app, "b@test.example").await;
    let (status, _, _) = request(&app, "/api/resources/customers", None, Some(&ca)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let bad_csrf = Client {
        cookie: ca.cookie.clone(),
        csrf: "incorrect".into(),
    };
    let (status, _, _) = request(
        &app,
        "/api/orders",
        Some(json!({"package_id":1})),
        Some(&bad_csrf),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let(status,p,_)=request(&app,"/api/resources/packages",Some(json!({"name":"Test","price":10,"billing_months":2,"duration_days":30,"min_email_accounts":1,"max_email_accounts":2,"min_domains":1,"max_domains":1,"storage_per_account_mb":1024})),Some(&admin_client)).await;
    assert_eq!(status, StatusCode::OK, "{p}");
    let pid = p["id"].as_i64().unwrap();
    let (status, o, _) = request(
        &app,
        "/api/orders",
        Some(json!({"package_id":pid,"emails":1,"domains":1})),
        Some(&ca),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{o}");
    let sid = o["id"].as_i64().unwrap();
    assert_eq!(
        o["quote"]["total"]
            .as_str()
            .unwrap()
            .parse::<rust_decimal::Decimal>()
            .unwrap(),
        rust_decimal::Decimal::from(20)
    );
    let (status, _, _) = request(&app, &format!("/api/invoices/{sid}"), None, Some(&cb)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let(status,_,_)=request(&app,"/api/resources/servers",Some(json!({"name":"Fixture","base_url":"http://unused.example","dry_run":1,"active":1,"is_primary":1})),Some(&admin_client)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = request(
        &app,
        "/api/resources/domains",
        Some(json!({"domain_name":"tenant.example"})),
        Some(&ca),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let code = o["invoice_code"].as_str().unwrap();
    async fn pay(app: &Router, id: i64, amount: i64, code: &str) -> (StatusCode, Value) {
        let p = json!({"id":id,"transferAmount":amount,"transferType":"in","accountNumber":"123456789","referenceCode":format!("bank-{id}"),"content":code});
        let req = Request::builder()
            .uri("/sepay/webhook")
            .method("POST")
            .header("authorization", "Apikey fixture-webhook-key")
            .header("content-type", "application/json")
            .body(Body::from(p.to_string()))
            .unwrap();
        let r = app.clone().oneshot(req).await.unwrap();
        let status = r.status();
        let v = serde_json::from_slice(&to_bytes(r.into_body(), 100000).await.unwrap()).unwrap();
        (status, v)
    }
    let (status, p) = pay(&app, 101, 10, code).await;
    assert_eq!(status, StatusCode::OK, "{p}");
    assert_eq!(p["state"], "partial");
    let (p1, p2) = tokio::join!(pay(&app, 102, 10, code), pay(&app, 102, 10, code));
    assert_eq!(p1.0, StatusCode::OK, "{:?}", p1);
    assert_eq!(p2.0, StatusCode::OK, "{:?}", p2);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM payment_transactions WHERE subscription_id=$1")
            .bind(sid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 2);
    let status: String = sqlx::query_scalar("SELECT status FROM subscriptions WHERE id=$1")
        .bind(sid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "active");
    assert_eq!(pay(&app, 102, 11, code).await.0, StatusCode::CONFLICT);
    let (status, d, _) = request(
        &app,
        "/api/resources/domains",
        Some(json!({"domain_name":"tenant.example"})),
        Some(&ca),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{d}");
    let did = d["id"].as_i64().unwrap();
    let (status, _, _) = request(
        &app,
        "/api/resources/domains",
        Some(json!({"domain_name":"second.example"})),
        Some(&ca),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    ui_rust::worker::tick(&state).await.unwrap();
    let remote: Option<String> =
        sqlx::query_scalar("SELECT stalwart_domain_id FROM domains WHERE id=$1")
            .bind(did)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(remote.unwrap().starts_with("dry-"));
    let gid: i64 = sqlx::query_scalar("SELECT id FROM email_groups WHERE customer_id=$1 LIMIT 1")
        .bind(a)
        .fetch_one(&pool)
        .await
        .unwrap();
    let(status,acc,_)=request(&app,"/api/resources/accounts",Some(json!({"domain_id":did,"group_id":gid,"local_part":"hello","password":"Mail-test-password-123"})),Some(&ca)).await;
    assert_eq!(status, StatusCode::OK, "{acc}");
    ui_rust::worker::tick(&state).await.unwrap();
    let (status, _, _) = request(
        &app,
        "/api/resources/accounts",
        Some(json!({"id":acc["id"],"action":"delete"})),
        Some(&cb),
    )
    .await;
    assert_ne!(status, StatusCode::OK);
    let (status, rows, _) = request(&app, "/api/resources/accounts", None, Some(&cb)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(rows["items"].as_array().unwrap().is_empty());
    let (status, _, _) = request(
        &app,
        "/api/content",
        Some(json!({"kind":"settings","entries":{"sepay_api_key":"bad"}})),
        Some(&admin_client),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let(status,_,_)=request(&app,"/api/auth/security",Some(json!({"action":"password","password":"Strong-test-password-123","new_password":"Changed-test-password-123"})),Some(&ca)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        request(&app, "/api/auth/me", None, Some(&ca)).await.0,
        StatusCode::UNAUTHORIZED
    );
    let billing_hash = crypto::password("Strong-test-password-123").unwrap();
    sqlx::query("INSERT INTO users(name,email,password_hash,role,admin_level,admin_permissions,two_factor_enabled) VALUES('Billing','billing@test.example',$1,'admin','billing','[\"subscriptions\"]'::jsonb,0)").bind(billing_hash).execute(&pool).await.unwrap();
    let billing = login(&app, "billing@test.example").await;
    assert_eq!(
        request(&app, "/api/resources/subscriptions", None, Some(&billing))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        request(&app, "/api/resources/customers", None, Some(&billing))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert!(admin > 0);
}
