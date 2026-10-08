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
        sepay_api_token: String::new(),
        sepay_api_mode: "live".into(),
        sepay_poll_seconds: 0,
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

async fn parity_fixture(pool: &sqlx::PgPool) -> (App, i64, i64, i64) {
    let hash = crypto::password("Strong-test-password-123").unwrap();
    sqlx::query("INSERT INTO users(name,email,password_hash,role,admin_level,two_factor_enabled) VALUES('Admin','parity-admin@test.example',$1,'admin','super',0)").bind(&hash).execute(pool).await.unwrap();
    let owner:i64=sqlx::query_scalar("INSERT INTO users(name,email,password_hash,two_factor_enabled) VALUES('Customer','parity@test.example',$1,0) RETURNING id").bind(&hash).fetch_one(pool).await.unwrap();
    let other:i64=sqlx::query_scalar("INSERT INTO users(name,email,password_hash,two_factor_enabled) VALUES('Other','other@test.example',$1,0) RETURNING id").bind(&hash).fetch_one(pool).await.unwrap();
    let package:i64=sqlx::query_scalar("INSERT INTO packages(name,max_email_accounts,max_domains,price) VALUES('Parity',2,2,10) RETURNING id").fetch_one(pool).await.unwrap();
    (
        App {
            db: pool.clone(),
            config: config(),
            http: reqwest::Client::new(),
            dummy_hash: hash,
        },
        owner,
        other,
        package,
    )
}

#[sqlx::test]
async fn impersonation_rotates_sessions_and_follows_admin_revocation(pool: sqlx::PgPool) {
    let (state, owner, _, _) = parity_fixture(&pool).await;
    let app = ui_rust::router(state);
    let admin = login(&app, "parity-admin@test.example").await;
    let customer = login(&app, "parity@test.example").await;
    assert_eq!(
        request(
            &app,
            "/api/auth/impersonate",
            Some(json!({"customer_id":owner})),
            Some(&customer)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let bad = Client {
        cookie: admin.cookie.clone(),
        csrf: "wrong".into(),
    };
    assert_eq!(
        request(
            &app,
            "/api/auth/impersonate",
            Some(json!({"customer_id":owner})),
            Some(&bad)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, result, cookie) = request(
        &app,
        "/api/auth/impersonate",
        Some(json!({"customer_id":owner})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_ne!(cookie, admin.cookie);
    let support = Client {
        cookie,
        csrf: result["csrf"].as_str().unwrap().into(),
    };
    let me = request(&app, "/api/auth/me", None, Some(&support)).await.1;
    assert_eq!(me["user"]["id"], owner);
    assert!(me["impersonator"].is_number());
    assert_eq!(
        request(&app, "/api/resources/customers", None, Some(&support))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(request(&app,"/api/auth/security",Some(json!({"action":"password","password":"Strong-test-password-123","new_password":"Another-strong-password"})),Some(&support)).await.0,StatusCode::FORBIDDEN);
    let (status, result, cookie) = request(
        &app,
        "/api/auth/impersonation/stop",
        Some(json!({})),
        Some(&support),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(
        request(&app, "/api/auth/me", None, Some(&support)).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, "/api/auth/me", None, Some(&admin)).await.0,
        StatusCode::UNAUTHORIZED
    );
    let admin = Client {
        cookie,
        csrf: result["csrf"].as_str().unwrap().into(),
    };
    assert_eq!(
        request(&app, "/api/auth/me", None, Some(&admin)).await.1["user"]["role"],
        "admin"
    );
    let (_, result, cookie) = request(
        &app,
        "/api/auth/impersonate",
        Some(json!({"customer_id":owner})),
        Some(&admin),
    )
    .await;
    let support = Client {
        cookie,
        csrf: result["csrf"].as_str().unwrap().into(),
    };
    request(&app, "/api/auth/logout", Some(json!({})), Some(&admin)).await;
    assert_eq!(
        request(&app, "/api/auth/me", None, Some(&support)).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test]
async fn bulk_import_preserves_quota_scope_and_password_secrecy(pool: sqlx::PgPool) {
    let (state, owner, other, package) = parity_fixture(&pool).await;
    sqlx::query("INSERT INTO subscriptions(customer_id,package_id,price,selected_email_accounts,selected_domains,status,start_date,end_date) VALUES($1,$2,10,2,2,'active',current_date,current_date+30)").bind(owner).bind(package).execute(&pool).await.unwrap();
    let server:i64=sqlx::query_scalar("INSERT INTO stalwart_servers(name,base_url,token_encrypted,dry_run,active,is_primary) VALUES('Test','https://test.example',$1,1,1,1) RETURNING id").bind(crypto::seal(&state.config.key,"test").unwrap()).fetch_one(&pool).await.unwrap();
    sqlx::query("INSERT INTO domains(customer_id,domain_name,server_id,stalwart_domain_id,sync_status,verification_status) VALUES($1,'parity.example',$2,'dry-domain','synced','verified')").bind(owner).bind(server).execute(&pool).await.unwrap();
    let group: i64 = sqlx::query_scalar(
        "INSERT INTO email_groups(customer_id,name) VALUES($1,'Team') RETURNING id",
    )
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let foreign_group: i64 = sqlx::query_scalar(
        "INSERT INTO email_groups(customer_id,name) VALUES($1,'Other') RETURNING id",
    )
    .bind(other)
    .fetch_one(&pool)
    .await
    .unwrap();
    let app = ui_rust::router(state);
    let c = login(&app, "parity@test.example").await;
    let text=format!("\u{feff}\"An, Nguyễn\",A,one@parity.example,Import-password-123,{group}\nB,C,foreign@parity.example,Import-password-123,{foreign_group}\nD,E,one@parity.example,Import-password-123,{group}\nF,G,two@parity.example,Import-password-123,{group}\nH,I,three@parity.example,Import-password-123,{group}");
    let (status, result, _) = request(
        &app,
        "/api/accounts/import",
        Some(json!({"text":text,"customer_id":other})),
        Some(&c),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["created"], 2);
    assert_eq!(result["failed"], 3);
    assert!(!result.to_string().contains("Import-password"));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM email_accounts WHERE customer_id=$1")
        .bind(owner)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 2);
    let encrypted: String =
        sqlx::query_scalar("SELECT payload_encrypted FROM api_sync_jobs LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!encrypted.contains("Import-password"));
    let (status, csv, _) = request(&app, "/api/resources/accounts/export", None, Some(&c)).await;
    assert_eq!(status, StatusCode::OK);
    let csv = csv["body"].as_str().unwrap();
    assert!(csv.contains("one@parity.example"));
    assert!(csv.contains("\"An, Nguyễn A\""));
    assert!(!csv.contains("Import-password"));
    assert!(!csv.contains("foreign@"));
}

#[sqlx::test]
async fn trial_documents_are_encrypted_private_and_retained(pool: sqlx::PgPool) {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let (state, _, _, package) = parity_fixture(&pool).await;
    let app = ui_rust::router(state.clone());
    let c = login(&app, "parity@test.example").await;
    let a = login(&app, "parity-admin@test.example").await;
    let mut data = json!({"package_id":package,"phone":"0123456","address":"Test address"});
    assert_eq!(
        request(&app, "/api/trials", Some(data.clone()), Some(&c))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    data["documents"] =
        json!({"citizen":{"mime":"application/pdf","data":STANDARD.encode(b"not a PDF")}});
    assert_eq!(
        request(&app, "/api/trials", Some(data.clone()), Some(&c))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let bytes = b"%PDF-1.7\nPrivate identity fixture\n%%EOF";
    data["documents"]["citizen"]["data"] = json!(STANDARD.encode(bytes));
    data["citizen_id"] = json!("IDENTITY-123");
    let (status, result, _) = request(&app, "/api/trials", Some(data), Some(&c)).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let id = result["id"].as_i64().unwrap();
    let enc: String = sqlx::query_scalar("SELECT encrypted FROM trial_documents WHERE trial_id=$1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!enc.contains("Private identity"));
    let (status, list, _) = request(&app, "/api/resources/trials", None, Some(&a)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["items"][0]["documents"], json!(["citizen"]));
    assert!(!list.to_string().contains("IDENTITY-123"));
    let path = format!("/api/trials/{id}/documents/citizen");
    assert_eq!(
        request(&app, &path, None, Some(&c)).await.0,
        StatusCode::FORBIDDEN
    );
    let (status, body, _) = request(&app, &path, None, Some(&a)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["body"], String::from_utf8_lossy(bytes).as_ref());
    sqlx::query("UPDATE trial_requests SET status='rejected',updated_at=now()-interval '100 days' WHERE id=$1").bind(id).execute(&pool).await.unwrap();
    ui_rust::trials::retain(&state).await.unwrap();
    assert_eq!(
        request(&app, &path, None, Some(&a)).await.0,
        StatusCode::NOT_FOUND
    );
}

#[sqlx::test]
async fn languages_and_landing_preserve_other_pages_and_sections(pool: sqlx::PgPool) {
    let (state, _, _, _) = parity_fixture(&pool).await;
    let app = ui_rust::router(state);
    let a = login(&app, "parity-admin@test.example").await;
    let c = login(&app, "parity@test.example").await;
    let public = request(&app, "/api/content?lang=en", None, None).await.1;
    assert!(public["translations"].as_object().unwrap().len() >= 2227);
    assert_eq!(
        request(&app, "/api/languages", None, Some(&c)).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(request(&app,"/api/languages",Some(json!({"action":"metadata","code":"fr","name":"French","native_name":"Français","enabled":true})),Some(&a)).await.0,StatusCode::OK);
    for (key, value) in [("first", "Bonjour"), ("second", "Au revoir")] {
        assert_eq!(
            request(
                &app,
                "/api/languages",
                Some(json!({"code":"fr","entries":{key:value}})),
                Some(&a)
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    let fr = request(&app, "/api/content?lang=fr", None, None).await.1;
    assert_eq!(fr["translations"]["first"], "Bonjour");
    assert_eq!(fr["translations"]["second"], "Au revoir");
    assert_eq!(
        request(
            &app,
            "/api/languages",
            Some(json!({"action":"default","code":"fr"})),
            Some(&a)
        )
        .await
        .0,
        StatusCode::OK
    );
    for action in ["delete", "metadata"] {
        assert_eq!(request(&app,"/api/languages",Some(json!({"action":action,"code":"fr","name":"French","native_name":"Français","enabled":false})),Some(&a)).await.0,StatusCode::BAD_REQUEST);
    }
    assert_eq!(
        request(&app, "/api/content", None, None).await.1["language"],
        "fr"
    );
    let first = request(&app, "/api/languages?code=en", None, Some(&a))
        .await
        .1;
    let second = request(&app, "/api/languages?code=en&offset=80", None, Some(&a))
        .await
        .1;
    assert_eq!(first["entries"].as_array().unwrap().len(), 80);
    assert_ne!(first["entries"][0]["key"], second["entries"][0]["key"]);
    for (section, content) in [
        (
            "hero",
            json!({"title":"Bonjour","description":"Bienvenue","visible":true}),
        ),
        (
            "faq",
            json!({"title":"FAQ","items":[{"question":"<script>x</script>","answer":"Escaped text"}],"visible":false}),
        ),
    ] {
        let (status, r, _) = request(
            &app,
            "/api/content",
            Some(json!({"kind":"section","language":"fr","section":section,"content":content})),
            Some(&a),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{r}");
    }
    let sections = request(&app, "/api/content?lang=fr", None, None).await.1["sections"].clone();
    assert_eq!(sections.as_array().unwrap().len(), 2);
    assert_eq!(request(&app,"/api/content",Some(json!({"kind":"section","language":"fr","section":"apps","content":{"items":[{"url":"javascript:alert(1)"}]}})),Some(&a)).await.0,StatusCode::BAD_REQUEST);
    assert_eq!(
        request(&app, "/api/content?lang=fr", None, None).await.1["sections"],
        sections
    );
}

#[sqlx::test]
async fn sepay_api_deduplicates_webhooks_and_never_applies_sandbox(pool: sqlx::PgPool) {
    let (state, _, _, package) = parity_fixture(&pool).await;
    let app = ui_rust::router(state.clone());
    let c = login(&app, "parity@test.example").await;
    let (_, order, _) = request(
        &app,
        "/api/orders",
        Some(json!({"package_id":package})),
        Some(&c),
    )
    .await;
    let sid = order["id"].as_i64().unwrap();
    let code = order["invoice_code"].as_str().unwrap();
    let row = json!({"id":"12345678-1234-1234-1234-123456789012","amount_in":"120","transfer_type":"in","account_number":"123456789","reference_number":"REF-API","transaction_content":code});
    let page = json!({"status":"success","data":[row],"meta":{"pagination":{"current_page":1,"has_more":false}}});
    let mut totals = json!({"read":0,"recorded":0,"duplicates":0,"skipped":0,"complete":false});
    ui_rust::sepay::apply_page(&state, &page, "sandbox", 1, &mut totals)
        .await
        .unwrap();
    assert_eq!(
        request(&app, &format!("/api/invoices/{sid}"), None, Some(&c))
            .await
            .1["invoice"]["status"],
        "pending"
    );
    ui_rust::sepay::apply_page(&state, &page, "live", 1, &mut totals)
        .await
        .unwrap();
    assert_eq!(
        request(&app, &format!("/api/invoices/{sid}"), None, Some(&c))
            .await
            .1["invoice"]["status"],
        "active"
    );
    ui_rust::sepay::apply_page(&state, &page, "live", 1, &mut totals)
        .await
        .unwrap();
    assert_eq!(totals["duplicates"], 1);
    let mut webhook = ui_rust::sepay::payload(&page["data"][0]);
    webhook["id"] = json!(123456);
    assert_eq!(
        ui_rust::billing::record(&state, webhook.clone(), "live", "webhook")
            .await
            .unwrap()
            .0["duplicate"],
        true
    );
    webhook["transferAmount"] = json!(121);
    assert!(ui_rust::billing::record(&state, webhook, "live", "webhook")
        .await
        .is_err());
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM payment_transactions WHERE subscription_id=$1")
            .bind(sid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
    let mut unreferenced = page["data"][0].clone();
    unreferenced["id"] = json!("12345678-1234-1234-1234-123456789013");
    unreferenced["reference_number"] = json!("");
    assert_eq!(
        ui_rust::billing::record(
            &state,
            ui_rust::sepay::payload(&unreferenced),
            "live",
            "api"
        )
        .await
        .unwrap()
        .0["state"],
        "needs_review"
    );
    let malformed = json!({"status":"success","data":[],"meta":{"pagination":{"current_page":2,"has_more":"false"}}});
    assert!(
        ui_rust::sepay::apply_page(&state, &malformed, "live", 1, &mut totals)
            .await
            .is_err()
    );
}

#[sqlx::test]
async fn templates_broadcast_and_reminders_are_transactional_and_idempotent(pool: sqlx::PgPool) {
    let (mut state, owner, _, package) = parity_fixture(&pool).await;
    state.config.smtp_host = "smtp.test.example".into();
    let app = ui_rust::router(state.clone());
    let a = login(&app, "parity-admin@test.example").await;
    let c = login(&app, "parity@test.example").await;
    assert_eq!(
        request(&app, "/api/email-templates", None, Some(&c))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let data = request(&app, "/api/email-templates", None, Some(&a))
        .await
        .1;
    assert_eq!(data["templates"].as_array().unwrap().len(), 11);
    let notice = json!({"template_key":"notice","name":"Notice","subject":"{{subject}}","body":"Hello {{name}}: {{message}}","enabled":true});
    assert_eq!(
        request(&app, "/api/email-templates", Some(notice), Some(&a))
            .await
            .0,
        StatusCode::OK
    );
    let broadcast = json!({"subject":"Test only","message":"Private message","password":"Strong-test-password-123","request_id":"repeatable-request"});
    assert_eq!(
        request(
            &app,
            "/api/email-broadcast",
            Some(broadcast.clone()),
            Some(&c)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            "/api/email-broadcast",
            Some(broadcast.clone()),
            Some(&a)
        )
        .await
        .1["queued"],
        2
    );
    assert_eq!(
        request(&app, "/api/email-broadcast", Some(broadcast), Some(&a))
            .await
            .1["queued"],
        0
    );
    let body:String=sqlx::query_scalar("SELECT body_encrypted FROM notifications WHERE dedupe_key LIKE 'broadcast:%' AND user_id=$1").bind(owner).fetch_one(&pool).await.unwrap();
    assert!(!body.contains("Private message"));
    assert_eq!(
        crypto::open(&state.config.key, &body).unwrap(),
        "Hello Customer: Private message"
    );
    sqlx::query("INSERT INTO subscriptions(customer_id,package_id,price,selected_email_accounts,selected_domains,status,start_date,end_date) VALUES($1,$2,120,1,1,'active',current_date,current_date+10)").bind(owner).bind(package).execute(&pool).await.unwrap();
    ui_rust::notifications::reminders(&state).await.unwrap();
    ui_rust::notifications::reminders(&state).await.unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications WHERE dedupe_key LIKE 'renewal:%'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM subscriptions WHERE renewal_for_subscription_id IS NOT NULL",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[sqlx::test]
async fn totp_enrollment_login_and_replay_are_enforced(pool: sqlx::PgPool) {
    use hmac::{Hmac, Mac};
    let (state, _, _, _) = parity_fixture(&pool).await;
    let app = ui_rust::router(state);
    let c = login(&app, "parity@test.example").await;
    let (status, result, _) = request(
        &app,
        "/api/auth/security",
        Some(json!({"action":"totp_setup","password":"Strong-test-password-123"})),
        Some(&c),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let secret = result["secret"].as_str().unwrap();
    let key = data_encoding::BASE32_NOPAD
        .decode(secret.as_bytes())
        .unwrap();
    let step = chrono::Utc::now().timestamp() / 30;
    let mut mac = Hmac::<sha1::Sha1>::new_from_slice(&key).unwrap();
    mac.update(&(step as u64).to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let off = (digest[19] & 15) as usize;
    let code = format!(
        "{:06}",
        (u32::from_be_bytes(digest[off..off + 4].try_into().unwrap()) & 0x7fffffff) % 1_000_000
    );
    assert_eq!(
        request(
            &app,
            "/api/auth/security",
            Some(
                json!({"action":"totp_confirm","password":"Strong-test-password-123","code":code})
            ),
            Some(&c)
        )
        .await
        .0,
        StatusCode::OK
    );
    request(&app, "/api/auth/logout", Some(json!({})), Some(&c)).await;
    let (status, result, cookie) = request(
        &app,
        "/api/auth/login",
        Some(json!({"email":"parity@test.example","password":"Strong-test-password-123"})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["mfa"], true);
    let pending = Client {
        cookie,
        csrf: result["csrf"].as_str().unwrap().into(),
    };
    assert_eq!(
        request(&app, "/api/auth/me", None, Some(&pending)).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (status, _, cookie) = request(
        &app,
        "/api/auth/mfa",
        Some(json!({"code":code})),
        Some(&pending),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let c = Client {
        cookie,
        csrf: pending.csrf.clone(),
    };
    assert_eq!(
        request(&app, "/api/auth/me", None, Some(&c)).await.0,
        StatusCode::OK
    );
    assert_eq!(
        request(&app, "/api/auth/mfa", Some(json!({"code":code})), Some(&c))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
}

#[sqlx::test]
async fn managed_backup_checks_integrity_and_restores_only_to_empty_database(pool: sqlx::PgPool) {
    let (mut state, _, _, _) = parity_fixture(&pool).await;
    let mut url = url::Url::parse(&std::env::var("DATABASE_URL").unwrap()).unwrap();
    url.set_path(pool.connect_options().get_database().unwrap());
    state.config.database_url = url.to_string();
    let app = ui_rust::router(state.clone());
    let a = login(&app, "parity-admin@test.example").await;
    let c = login(&app, "parity@test.example").await;
    assert_eq!(
        request(&app, "/api/backups", Some(json!({})), Some(&c))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (status, r, _) = request(&app, "/api/backups", Some(json!({})), Some(&a)).await;
    assert_eq!(status, StatusCode::OK, "{r}");
    let id = r["id"].as_i64().unwrap();
    assert_eq!(
        request(&app, "/api/backups", Some(json!({})), Some(&a))
            .await
            .0,
        StatusCode::CONFLICT
    );
    ui_rust::backup_admin::tick(&state).await.unwrap();
    let list = request(&app, "/api/backups", None, Some(&a)).await.1;
    assert_eq!(list["backups"][0]["status"], "done", "{list}");
    assert!(list["backups"][0]["bytes"].as_i64().unwrap() > 1000);
    assert_eq!(
        request(&app, &format!("/api/backups/{id}/download"), None, Some(&a))
            .await
            .0,
        StatusCode::OK
    );
    let current = pool.connect_options().get_database().unwrap().to_string();
    let restore = format!("/api/backups/{id}/restore");
    assert_eq!(request(&app,&restore,Some(json!({"database":current,"confirm_database":current,"password":"Strong-test-password-123"})),Some(&a)).await.0,StatusCode::BAD_REQUEST);
    let target = format!("ui_restore_{}", &crypto::token()[..16]);
    sqlx::query(&format!("CREATE DATABASE {target}"))
        .execute(&pool)
        .await
        .unwrap();
    let data =
        json!({"database":target,"confirm_database":target,"password":"Strong-test-password-123"});
    let (status, r, _) = request(&app, &restore, Some(data.clone()), Some(&a)).await;
    assert_eq!(status, StatusCode::OK, "{r}");
    assert_eq!(
        request(&app, &restore, Some(data), Some(&a)).await.0,
        StatusCode::BAD_REQUEST
    );
    url.set_path(&target);
    let restored = sqlx::PgPool::connect(url.as_str()).await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&restored)
        .await
        .unwrap();
    assert_eq!(count, 3);
    restored.close().await;
    sqlx::query(&format!("DROP DATABASE {target}"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE rust_backups SET checksum=repeat('0',64) WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        request(&app, &format!("/api/backups/{id}/download"), None, Some(&a))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
}

#[derive(Default)]
struct FakeStalwart {
    rows: std::sync::Mutex<Value>,
    writes: std::sync::atomic::AtomicUsize,
    apply_updates: std::sync::atomic::AtomicBool,
}
async fn fake_stalwart(
    axum::extract::State(fake): axum::extract::State<std::sync::Arc<FakeStalwart>>,
    axum::Json(v): axum::Json<Value>,
) -> axum::Json<Value> {
    let mut responses = Vec::new();
    for c in v["methodCalls"].as_array().unwrap() {
        let name = c[0].as_str().unwrap();
        let (kind, action) = name.strip_prefix("x:").unwrap().split_once('/').unwrap();
        let rows = fake.rows.lock().unwrap()[kind]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let body = match action {
            "query" => {
                json!({"ids":rows.iter().map(|r|r["id"].clone()).collect::<Vec<_>>(),"total":rows.len(),"state":"state-1"})
            }
            "get" => {
                json!({"list":rows.into_iter().filter(|r|c[1]["ids"].as_array().unwrap().contains(&r["id"])).collect::<Vec<_>>(),"notFound":[],"state":"state-1"})
            }
            "set" => {
                fake.writes
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let created: serde_json::Map<String, Value> = c[1]["create"]
                    .as_object()
                    .into_iter()
                    .flat_map(|o| o.keys())
                    .map(|k| {
                        (
                            k.clone(),
                            json!({"id":"new-credential","secret":"fixture-created-secret"}),
                        )
                    })
                    .collect();
                let updated: serde_json::Map<String, Value> = c[1]["update"]
                    .as_object()
                    .into_iter()
                    .flat_map(|o| o.keys())
                    .map(|k| (k.clone(), Value::Null))
                    .collect();
                if fake.apply_updates.load(std::sync::atomic::Ordering::SeqCst) {
                    let mut all = fake.rows.lock().unwrap();
                    if let Some(rows) = all[kind].as_array_mut() {
                        for row in rows {
                            if let Some(patch) =
                                c[1]["update"][row["id"].as_str().unwrap()].as_object()
                            {
                                for (key, value) in patch {
                                    if key == "quotas/maxDiskQuota" {
                                        row["quotas"]["maxDiskQuota"] = value.clone()
                                    } else {
                                        row[key] = value.clone();
                                    }
                                }
                            }
                        }
                    }
                }
                json!({"created":created,"updated":updated,"destroyed":c[1].get("destroy").cloned().unwrap_or(json!([]))})
            }
            _ => panic!("Unsupported mock method"),
        };
        responses.push(json!([name, body, c[2]]));
    }
    axum::Json(json!({"methodResponses":responses}))
}
async fn mock_server(
    state: &App,
) -> (
    i64,
    std::sync::Arc<FakeStalwart>,
    tokio::task::JoinHandle<()>,
) {
    let fake = std::sync::Arc::new(FakeStalwart::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router=Router::new().route("/jmap/",axum::routing::post(fake_stalwart)).route("/api/account",axum::routing::get(||async{axum::Json(json!({"data":{"permissions":["sysDomainGet","sysAccountGet","sysTenantGet","sysApiKeySet","sysApiKeyCreate","sysTaskCreate","sysDomainQuery","sysAccountQuery","sysDomainCreate","sysDomainUpdate","sysAccountCreate","sysAccountUpdate","sysDkimSignatureGet","sysDkimSignatureQuery","sysDkimSignatureCreate","sysDkimSignatureUpdate","sysCertificateGet","sysCertificateQuery"]}}))})).with_state(fake.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let id:i64=sqlx::query_scalar("INSERT INTO stalwart_servers(name,base_url,token_encrypted,dry_run,active) VALUES('Mock',$1,$2,0,1) RETURNING id").bind(format!("http://{addr}")).bind(crypto::seal(&state.config.key,"fixture-token").unwrap()).fetch_one(&state.db).await.unwrap();
    (id, fake, task)
}

#[sqlx::test]
async fn management_plans_are_scoped_single_use_and_secrets_are_private(pool: sqlx::PgPool) {
    let (state, owner, _, _) = parity_fixture(&pool).await;
    let (server, fake, task) = mock_server(&state).await;
    *fake.rows.lock().unwrap() =
        json!({"ApiKey":[{"id":"key-1","description":"Existing","secret":"must-not-leak"}]});
    let app = ui_rust::router(state.clone());
    let admin = login(&app, "parity-admin@test.example").await;
    let customer = login(&app, "parity@test.example").await;
    let (status, r, _) = request(
        &app,
        "/api/management/list",
        Some(json!({"server_id":server,"kind":"api_keys"})),
        Some(&customer),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{r}");
    let (status, r, _) = request(
        &app,
        "/api/management/list",
        Some(json!({"server_id":server,"kind":"api_keys"})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{r}");
    assert!(!r.to_string().contains("must-not-leak"));
    assert_eq!(r["mode"], "live");
    let (status,p,_)=request(&app,"/api/management/preview",Some(json!({"server_id":server,"kind":"api_keys","action":"create","fields":{"description":"Fixture","permissions":"sysApiKeySet","expires_at":(chrono::Utc::now()+chrono::Duration::days(1)).to_rfc3339(),"allowed_ips":"127.0.0.1/32"}})),Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{p}");
    let id = p["plan_id"].as_i64().unwrap();
    let (status, r, _) = request(
        &app,
        &format!("/api/operations/execute/{id}"),
        Some(json!({})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{r}");
    assert_eq!(r["secret_available"], true);
    assert!(!r.to_string().contains("fixture-created-secret"));
    assert_eq!(
        request(
            &app,
            &format!("/api/operations/execute/{id}"),
            Some(json!({"password":"Strong-test-password-123"})),
            Some(&admin)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(fake.writes.load(std::sync::atomic::Ordering::SeqCst), 1);
    let persisted: String =
        sqlx::query_scalar("SELECT result_secret FROM rust_change_plans WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!persisted.contains("fixture-created-secret"));
    assert_eq!(
        request(
            &app,
            &format!("/api/management/secrets/{id}"),
            Some(json!({})),
            Some(&customer)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, r, _) = request(
        &app,
        &format!("/api/management/secrets/{id}"),
        Some(json!({"password":"Strong-test-password-123"})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{r}");
    assert!(r["secret"]
        .as_str()
        .unwrap()
        .contains("fixture-created-secret"));
    assert_eq!(
        request(
            &app,
            &format!("/api/management/secrets/{id}"),
            Some(json!({"password":"Strong-test-password-123"})),
            Some(&admin)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let domain:i64=sqlx::query_scalar("INSERT INTO domains(customer_id,domain_name,server_id,stalwart_domain_id,sync_status) VALUES($1,'guard.example',$2,'domain-1','synced') RETURNING id").bind(owner).bind(server).fetch_one(&pool).await.unwrap();
    let account:i64=sqlx::query_scalar("INSERT INTO email_accounts(customer_id,domain_id,email,local_part,storage_limit_mb,server_id,stalwart_account_id,sync_status) VALUES($1,$2,'a@guard.example','a',100,$3,'account-1','synced') RETURNING id").bind(owner).bind(domain).bind(server).fetch_one(&pool).await.unwrap();
    let (status,p,_)=request(&app,"/api/management/preview",Some(json!({"server_id":server,"kind":"tasks","action":"reindex_account","fields":{"account_id":account}})),Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{p}");
    sqlx::query("UPDATE email_accounts SET stalwart_account_id='changed' WHERE id=$1")
        .bind(account)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        request(
            &app,
            &format!("/api/operations/execute/{}", p["plan_id"]),
            Some(json!({"password":"Strong-test-password-123"})),
            Some(&admin)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(fake.writes.load(std::sync::atomic::Ordering::SeqCst), 1);
    task.abort();
}

#[sqlx::test]
async fn migration_rechecks_inventory_aliases_and_server_versions(pool: sqlx::PgPool) {
    let (state, owner, _, _) = parity_fixture(&pool).await;
    let (source, _, source_task) = mock_server(&state).await;
    let (target, fake, target_task) = mock_server(&state).await;
    let domain:i64=sqlx::query_scalar("INSERT INTO domains(customer_id,domain_name,server_id,stalwart_domain_id,sync_status) VALUES($1,'move.example',$2,'old-domain','synced') RETURNING id").bind(owner).bind(source).fetch_one(&pool).await.unwrap();
    let account:i64=sqlx::query_scalar("INSERT INTO email_accounts(customer_id,domain_id,email,local_part,storage_limit_mb,server_id,stalwart_account_id,sync_status) VALUES($1,$2,'a@move.example','a',100,$3,'old-account','synced') RETURNING id").bind(owner).bind(domain).bind(source).fetch_one(&pool).await.unwrap();
    *fake.rows.lock().unwrap() = json!({"Domain":[{"id":"new-domain","name":"move.example","isEnabled":true}],"Account":[{"id":"new-account","name":"a","domainId":"new-domain","quotas":{"maxDiskQuota":104857600},"aliases":[]}]});
    let app = ui_rust::router(state.clone());
    let admin = login(&app, "parity-admin@test.example").await;
    let payload = json!({"domain_id":domain,"target_server_id":target,"mappings":{"domain_id":"new-domain","accounts":{account.to_string():"new-account"}},"checklist":{"backup":true,"restore":true,"delivery":true,"dns":true}});
    let (status, p, _) = request(
        &app,
        "/api/migrations/preview",
        Some(payload.clone()),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{p}");
    fake.rows.lock().unwrap()["Account"][0]["aliases"] =
        json!([{"name":"unexpected","domainId":"new-domain"}]);
    let (status, r, _) = request(
        &app,
        &format!("/api/migrations/{}/apply", p["plan_id"]),
        Some(json!({"confirmation":"MIGRATE"})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{r}");
    let current: i64 = sqlx::query_scalar("SELECT server_id FROM domains WHERE id=$1")
        .bind(domain)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(current, source);
    fake.rows.lock().unwrap()["Account"][0]["aliases"] = json!([]);
    sqlx::query("UPDATE stalwart_servers SET config_version=config_version+1 WHERE id=$1")
        .bind(source)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        request(
            &app,
            &format!("/api/migrations/{}/apply", p["plan_id"]),
            Some(json!({"confirmation":"MIGRATE"})),
            Some(&admin)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (status, p, _) =
        request(&app, "/api/migrations/preview", Some(payload), Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{p}");
    let (status, r, _) = request(
        &app,
        &format!("/api/migrations/{}/apply", p["plan_id"]),
        Some(json!({"confirmation":"MIGRATE"})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{r}");
    let row: (i64, String) =
        sqlx::query_as("SELECT server_id,stalwart_account_id FROM email_accounts WHERE id=$1")
            .bind(account)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row, (target, "new-account".into()));
    assert_eq!(
        request(
            &app,
            &format!("/api/migrations/{}/apply", p["plan_id"]),
            Some(json!({"confirmation":"MIGRATE"})),
            Some(&admin)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    source_task.abort();
    target_task.abort();
}

#[sqlx::test]
async fn mail_reports_validate_compression_deduplicate_and_scope_monthly_exports(
    pool: sqlx::PgPool,
) {
    use base64::Engine;
    use std::io::Write;
    let (state, owner, _, _) = parity_fixture(&pool).await;
    let (server, _, task) = mock_server(&state).await;
    let app = ui_rust::router(state.clone());
    let admin = login(&app, "parity-admin@test.example").await;
    let customer = login(&app, "parity@test.example").await;
    let other = login(&app, "other@test.example").await;
    let now = chrono::Utc::now();
    let xml=format!("<feedback><report_metadata><org_name>Test</org_name><report_id>report-1</report_id><date_range><begin>{}</begin><end>{}</end></date_range></report_metadata><policy_published><domain>report.example</domain></policy_published><record><row><source_ip>192.0.2.1</source_ip><count>20</count><policy_evaluated><dkim>pass</dkim><spf>fail</spf></policy_evaluated></row></record><record><row><source_ip>192.0.2.2</source_ip><count>3</count><policy_evaluated><dkim>fail</dkim><spf>fail</spf></policy_evaluated></row></record></feedback>",now.timestamp()-3600,now.timestamp());
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gzip.write_all(xml.as_bytes()).unwrap();
    let bytes = gzip.finish().unwrap();
    let payload = json!({"server_id":server,"kind":"dmarc","file":base64::engine::general_purpose::STANDARD.encode(bytes)});
    let (status, r, _) = request(
        &app,
        "/api/reports/import",
        Some(payload.clone()),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{r}");
    assert_eq!(r["inserted"], 1);
    let (status, r, _) = request(&app, "/api/reports/import", Some(payload), Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{r}");
    assert_eq!(r["duplicates"], 1);
    let query = format!(
        "/api/reports?server_id={server}&month={}&domain=report.example",
        now.format("%Y-%m")
    );
    assert_eq!(
        request(&app, &query, None, Some(&customer)).await.0,
        StatusCode::FORBIDDEN
    );
    sqlx::query(
        "INSERT INTO domains(customer_id,domain_name,server_id) VALUES($1,'report.example',$2)",
    )
    .bind(owner)
    .bind(server)
    .execute(&pool)
    .await
    .unwrap();
    let (status, r, _) = request(&app, &query, None, Some(&customer)).await;
    assert_eq!(status, StatusCode::OK, "{r}");
    assert_eq!(r["items"][0]["total"], 23);
    assert_eq!(r["items"][0]["passed"], 20);
    assert_eq!(r["items"][0]["failed"], 3);
    assert_eq!(
        request(&app, &query, None, Some(&other)).await.0,
        StatusCode::FORBIDDEN
    );
    let (status, r, _) = request(&app, &(query.clone() + "&format=csv"), None, Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{r}");
    assert!(r["body"].as_str().unwrap().contains("report.example"));
    assert!(ui_rust::reports::parse(
        "dmarc",
        b"<!DOCTYPE feedback [<!ENTITY x SYSTEM 'file:///etc/passwd'>]><feedback>&x;</feedback>"
    )
    .is_err());
    let mut bomb = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    bomb.write_all(&vec![b'x'; 8 * 1024 * 1024 + 1]).unwrap();
    assert!(ui_rust::reports::parse("dmarc", &bomb.finish().unwrap()).is_err());
    let dsn=b"Arrival-Date: Wed, 07 Oct 2026 12:00:00 +0000\r\n\r\nFinal-Recipient: rfc822; user@report.example\r\nAction: failed\r\nStatus: 5.1.1\r\nDiagnostic-Code: smtp; unknown recipient";
    let r = ui_rust::reports::parse("dsn", dsn).unwrap();
    assert_eq!(r[0]["failed"], 1);
    let tls = json!({"report-id":"tls-1","organization-name":"Test","date-range":{"start-datetime":now.to_rfc3339(),"end-datetime":now.to_rfc3339()},"policies":[{"policy":{"policy-domain":"report.example","policy-type":"sts"},"summary":{"total-successful-session-count":5,"total-failure-session-count":2},"failure-details":[]}]});
    assert_eq!(
        ui_rust::reports::parse("tls", tls.to_string().as_bytes()).unwrap()[0]["total"],
        7
    );
    task.abort();
}

#[sqlx::test]
async fn monitoring_distinguishes_unknown_and_dry_run_and_alerts_reopen(pool: sqlx::PgPool) {
    let (state, _, _, _) = parity_fixture(&pool).await;
    let (server, fake, task) = mock_server(&state).await;
    fake.rows.lock().unwrap()["Certificate"] = json!([{"id":"cert-expiring","notValidAfter":(chrono::Utc::now()+chrono::Duration::days(3)).to_rfc3339(),"subjectAlternativeNames":["mail.example"]}]);
    let app = ui_rust::router(state.clone());
    let admin = login(&app, "parity-admin@test.example").await;
    let payload = json!({"enabled":true,"monitor_interval":60,"failure_threshold":2,"queue_threshold":10,"notify_recipients":[]});
    assert_eq!(
        request(
            &app,
            &format!("/api/servers/{server}/monitor"),
            Some(payload),
            Some(&admin)
        )
        .await
        .0,
        StatusCode::OK
    );
    let (status, r, _) = request(
        &app,
        &format!("/api/servers/{server}/check"),
        Some(json!({})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{r}");
    assert_eq!(r["metrics"]["api_up"], true);
    assert!(r["metrics"]["queue_total"].is_null());
    assert!(r["metrics"]["tls_days"].as_i64().unwrap() <= 3);
    assert_eq!(r["metrics"]["tls_certificate_scan_complete"], true);
    let key = format!("certificate_{}", &crypto::hash("cert-expiring")[..32]);
    let alert: i64 = sqlx::query_scalar(
        "SELECT id FROM operational_alerts WHERE server_id=$1 AND rule_key=$2 AND status='open'",
    )
    .bind(server)
    .bind(&key)
    .fetch_one(&pool)
    .await
    .unwrap();
    fake.rows.lock().unwrap()["Certificate"][0]["notValidAfter"] =
        json!((chrono::Utc::now() + chrono::Duration::days(60)).to_rfc3339());
    ui_rust::monitor::sample(&state, server).await.unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM operational_alerts WHERE id=$1")
        .bind(alert)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "resolved");
    fake.rows.lock().unwrap()["Certificate"][0]["notValidAfter"] =
        json!((chrono::Utc::now() - chrono::Duration::days(1)).to_rfc3339());
    ui_rust::monitor::sample(&state, server).await.unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM operational_alerts WHERE id=$1")
        .bind(alert)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "open");
    task.abort();
    tokio::task::yield_now().await;
    ui_rust::monitor::sample(&state, server).await.unwrap();
    ui_rust::monitor::sample(&state, server).await.unwrap();
    let alert:i64=sqlx::query_scalar("SELECT id FROM operational_alerts WHERE server_id=$1 AND rule_key='api_down' AND status='open'").bind(server).fetch_one(&pool).await.unwrap();
    assert_eq!(
        request(
            &app,
            "/api/resources/alerts",
            Some(json!({"id":alert,"action":"ack"})),
            Some(&admin)
        )
        .await
        .0,
        StatusCode::OK
    );
    sqlx::query("UPDATE stalwart_servers SET dry_run=1,next_check_at=NULL WHERE id=$1")
        .bind(server)
        .execute(&pool)
        .await
        .unwrap();
    let r = ui_rust::monitor::sample(&state, server).await.unwrap();
    assert_eq!(r["status"], "simulated");
    assert!(r["metrics"]["api_up"].is_null());
    let (status, r, _) = request(
        &app,
        &format!("/api/monitor?server_id={server}"),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{r}");
    assert_eq!(r["daily"][0]["checks"], 5);
    assert_eq!(r["daily"][0]["api_pass"], 3);
    let next: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT next_check_at FROM stalwart_servers WHERE id=$1")
            .bind(server)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(next.is_none());
}

#[sqlx::test]
async fn mailbox_archives_are_encrypted_restore_preview_is_required_and_one_use(
    pool: sqlx::PgPool,
) {
    use std::os::unix::fs::PermissionsExt;
    let (mut state, owner, _, _) = parity_fixture(&pool).await;
    state.config.database_url = format!(
        "postgres://localhost/{}",
        pool.connect_options().get_database().unwrap()
    );
    let root = std::path::PathBuf::from(format!("/tmp/ui-mailbox-test-{}", crypto::token()));
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let sqlite = root.join("fixture.sqlite");
    let conn = rusqlite::Connection::open(&sqlite).unwrap();
    conn.execute_batch("CREATE TABLE messages(id INTEGER PRIMARY KEY,body TEXT); INSERT INTO messages VALUES(1,'private-fixture-message');").unwrap();
    drop(conn);
    let executable = root.join("vandelay-fixture");
    let script=format!("#!/bin/bash\nset -e\nif [[ $1 == import ]]; then cp '{}' \"${{@: -1}}\"; elif [[ \" $* \" != *' --dry-run '* ]]; then touch '{}/restored'; fi\nprintf 'messages: 1\\nmailboxes: 1\\n'\n",sqlite.display(),root.display());
    std::fs::write(&executable, script).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::env::set_var("MAILBOX_VANDELAY_BIN", &executable);
    let server:i64=sqlx::query_scalar("INSERT INTO stalwart_servers(name,base_url,dry_run,active) VALUES('Mailbox fixture','https://mailbox.fixture.example',0,1) RETURNING id").fetch_one(&pool).await.unwrap();
    let domain:i64=sqlx::query_scalar("INSERT INTO domains(customer_id,domain_name,server_id,stalwart_domain_id,sync_status) VALUES($1,'mailbox.example',$2,'domain-1','synced') RETURNING id").bind(owner).bind(server).fetch_one(&pool).await.unwrap();
    let mut accounts = Vec::new();
    for local in ["source", "lab"] {
        let id:i64=sqlx::query_scalar("INSERT INTO email_accounts(customer_id,domain_id,email,local_part,storage_limit_mb,server_id,stalwart_account_id,sync_status) VALUES($1,$2,$3,$4,100,$5,$6,'synced') RETURNING id").bind(owner).bind(domain).bind(format!("{local}@mailbox.example")).bind(local).bind(server).bind(format!("remote-{local}")).fetch_one(&pool).await.unwrap();
        accounts.push(id);
    }
    let app = ui_rust::router(state.clone());
    let admin = login(&app, "parity-admin@test.example").await;
    let mut profiles = Vec::new();
    for (i, account) in accounts.iter().enumerate() {
        let (status,p,_)=request(&app,"/api/mailbox-backups",Some(json!({"account_id":account,"jmap_url":"https://mailbox.fixture.example/jmap/session","app_password":"fixture-app-password","config_version":0,"interval_hours":24,"keep_local":7,"restore_target":i==1})),Some(&admin)).await;
        assert_eq!(status, StatusCode::OK, "{p}");
        profiles.push(p["id"].as_i64().unwrap());
    }
    let (status, j, _) = request(
        &app,
        "/api/mailbox-backups/jobs",
        Some(json!({"profile_id":profiles[0],"kind":"backup"})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{j}");
    ui_rust::mailbox_backup::tick(&state).await.unwrap();
    let (status, list, _) = request(&app, "/api/mailbox-backups", None, Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["jobs"][0]["status"], "done", "{list}");
    assert!(!list.to_string().contains("fixture-app-password"));
    let artifact = list["artifacts"][0]["id"].as_i64().unwrap();
    let source = json!({"profile_id":profiles[0],"kind":"restore","artifact_id":artifact,"target_profile_id":profiles[1],"confirmation":"RESTORE"});
    assert_ne!(
        request(
            &app,
            "/api/mailbox-backups/jobs",
            Some(source.clone()),
            Some(&admin)
        )
        .await
        .0,
        StatusCode::OK
    );
    let (status,preview,_)=request(&app,"/api/mailbox-backups/jobs",Some(json!({"profile_id":profiles[0],"kind":"restore_preview","artifact_id":artifact,"target_profile_id":profiles[1]})),Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    ui_rust::mailbox_backup::tick(&state).await.unwrap();
    assert!(!root.join("restored").exists());
    let mut source = source;
    source["preview_id"] = preview["job_id"].clone();
    let (status, result, _) = request(
        &app,
        "/api/mailbox-backups/jobs",
        Some(source.clone()),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    ui_rust::mailbox_backup::tick(&state).await.unwrap();
    assert!(root.join("restored").exists());
    assert_eq!(
        request(
            &app,
            "/api/mailbox-backups/jobs",
            Some(source),
            Some(&admin)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let interrupted:i64=sqlx::query_scalar("INSERT INTO mailbox_backup_jobs(profile_id,profile_version,actor_id,kind,status,run_after,started_at) VALUES($1,1,$2,'restore','running',now(),now()) RETURNING id").bind(profiles[0]).bind(1_i64).fetch_one(&pool).await.unwrap();
    ui_rust::mailbox_backup::tick(&state).await.unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM mailbox_backup_jobs WHERE id=$1")
        .bind(interrupted)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "uncertain");
    let (status, body, _) = request(
        &app,
        &format!("/api/mailbox-backups/artifacts/{artifact}/download"),
        None,
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!body.to_string().contains("private-fixture-message"));
    std::fs::remove_dir_all(&root).unwrap();
}

#[sqlx::test]
async fn recovery_archives_validate_snapshot_and_restore_only_into_isolated_lab(
    pool: sqlx::PgPool,
) {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;
    let (mut state, _, _, _) = parity_fixture(&pool).await;
    state.config.database_url = format!(
        "postgres://localhost/{}",
        pool.connect_options().get_database().unwrap()
    );
    let (source, _, source_task) = mock_server(&state).await;
    let (target, _, target_task) = mock_server(&state).await;
    let source_base: String =
        sqlx::query_scalar("SELECT base_url FROM stalwart_servers WHERE id=$1")
            .bind(source)
            .fetch_one(&pool)
            .await
            .unwrap();
    let target_base: String =
        sqlx::query_scalar("SELECT base_url FROM stalwart_servers WHERE id=$1")
            .bind(target)
            .fetch_one(&pool)
            .await
            .unwrap();
    let root = std::path::PathBuf::from(format!("/tmp/ui-recovery-test-{}", crypto::token()));
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let lab = root.join("lab");
    std::fs::create_dir(&lab).unwrap();
    let plan=json!({"@type":"upsert","object":"Domain","matchOn":["name"],"value":{"domain-1":{"name":"recovery.example","isEnabled":true,"dnsManagement":{"@type":"Automatic"}}}}).to_string()+"\n";
    let plan_path = root.join("configuration.ndjson");
    std::fs::write(&plan_path, &plan).unwrap();
    let mut components = serde_json::Map::new();
    for role in ["data", "blob", "bootstrap"] {
        let path = root.join(format!("snapshot-{role}"));
        std::fs::write(&path, format!("private-{role}-snapshot")).unwrap();
        components.insert(role.into(), json!(path));
    }
    let marker_path = root.join("snapshot.json");
    let marker = json!({"snapshot_id":"fixture-snapshot","created_at":chrono::Utc::now().to_rfc3339(),"consistent":true,"server_origin":source_base,"config_sha256":hex::encode(Sha256::digest(plan.as_bytes()))});
    std::fs::write(&marker_path, marker.to_string()).unwrap();
    let config_path = root.join("sources.json");
    let config = json!({"profiles":{"source":{"server_origin":source_base,"snapshot_marker":marker_path,"config_plan_path":plan_path,"components":components},"lab":{"server_origin":target_base,"restore_root":lab,"isolated_rehearsal":true,"outbound_isolated":true,"seed_account_ids":[]}}});
    std::fs::write(&config_path, config.to_string()).unwrap();
    std::env::set_var("RECOVERY_SOURCES_FILE", &config_path);
    let executable = root.join("stalwart-cli-fixture");
    std::fs::write(&executable,format!("#!/bin/bash\nset -e\nif [[ \" $* \" != *' --dry-run '* ]]; then touch '{}/applied'; fi\nprintf 'validated\\n'\n",root.display())).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::env::set_var("RECOVERY_STALWART_CLI_BIN", &executable);
    let app = ui_rust::router(state.clone());
    let admin = login(&app, "parity-admin@test.example").await;
    let mut profiles = Vec::new();
    for (server, key, restore_target) in [(source, "source", false), (target, "lab", true)] {
        let(status,p,_)=request(&app,"/api/recovery",Some(json!({"server_id":server,"name":key,"source_key":key,"object_types":"Domain","interval_hours":24,"keep_local":7,"version":0,"restore_target":restore_target})),Some(&admin)).await;
        assert_eq!(status, StatusCode::OK, "{p}");
        profiles.push(p["id"].as_i64().unwrap());
    }
    let (status, j, _) = request(
        &app,
        "/api/recovery/jobs",
        Some(json!({"profile_id":profiles[0],"kind":"backup"})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{j}");
    ui_rust::recovery::tick(&state).await.unwrap();
    let (status, r, _) = request(&app, "/api/recovery", None, Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{r}");
    assert_eq!(r["jobs"][0]["status"], "done", "{r}");
    let artifact = r["artifacts"][0]["id"].as_i64().unwrap();
    let(status,preview,_)=request(&app,"/api/recovery/jobs",Some(json!({"profile_id":profiles[0],"kind":"restore_preview","artifact_id":artifact,"target_profile_id":profiles[1]})),Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    ui_rust::recovery::tick(&state).await.unwrap();
    assert!(!root.join("applied").exists());
    let (status, r, _) = request(&app, "/api/recovery", None, Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{r}");
    assert_eq!(r["jobs"][0]["status"], "done", "{r}");
    assert_eq!(r["jobs"][0]["result"]["dry_run"], true);
    assert_eq!(r["jobs"][0]["result"]["backend_staged"], false);
    let payload = json!({"profile_id":profiles[0],"kind":"restore","artifact_id":artifact,"target_profile_id":profiles[1],"preview_id":preview["job_id"],"confirmation":"RESTORE_SERVER"});
    let (status, restore, _) = request(
        &app,
        "/api/recovery/jobs",
        Some(payload.clone()),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{restore}");
    ui_rust::recovery::tick(&state).await.unwrap();
    assert!(root.join("applied").exists());
    let (status, r, _) = request(&app, "/api/recovery", None, Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{r}");
    assert_eq!(r["jobs"][0]["status"], "done", "{r}");
    assert_eq!(r["jobs"][0]["result"]["backend_staged"], true);
    assert_eq!(r["jobs"][0]["result"]["full_restore_verified"], false);
    let stage = lab.join(format!("rehearsal-{}", restore["job_id"]));
    assert_eq!(
        std::fs::read_to_string(stage.join("components/blob/snapshot-blob")).unwrap(),
        "private-blob-snapshot"
    );
    assert_eq!(
        request(&app, "/api/recovery/jobs", Some(payload), Some(&admin))
            .await
            .0,
        StatusCode::CONFLICT
    );
    let proof = json!({"backend_loaded":true,"credentials_checked":true,"mail_folders_checked":true,"attachments_checked":true,"queue_checked":true,"outbound_isolated":true,"note":"Fixture rehearsal checked with local files and fake CLI; no live backend attestation."});
    let (status, r, _) = request(
        &app,
        &format!("/api/recovery/jobs/{}/attest", restore["job_id"]),
        Some(proof),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{r}");
    let link = root.join("symlink");
    std::os::unix::fs::symlink(&plan_path, &link).unwrap();
    assert!(ui_rust::recovery_sources::path(link.to_str().unwrap())
        .await
        .is_err());
    assert!(!ui_rust::recovery_sources::entry("../escape"));
    source_task.abort();
    target_task.abort();
    std::fs::remove_dir_all(root).unwrap();
    std::env::remove_var("RECOVERY_SOURCES_FILE");
}

#[sqlx::test]
async fn emergency_dkim_stages_real_rsa_and_keeps_private_key_only_in_encrypted_vault(
    pool: sqlx::PgPool,
) {
    let (mut state, owner, _, _) = parity_fixture(&pool).await;
    state.config.database_url = format!(
        "postgres://localhost/{}",
        pool.connect_options().get_database().unwrap()
    );
    let (server, fake, task) = mock_server(&state).await;
    let domain:i64=sqlx::query_scalar("INSERT INTO domains(customer_id,domain_name,server_id,stalwart_domain_id,sync_status) VALUES($1,'keys.example',$2,'domain-keys','synced') RETURNING id").bind(owner).bind(server).fetch_one(&pool).await.unwrap();
    sqlx::query("INSERT INTO recovery_profiles(server_id,name,source_key,object_types,created_by) VALUES($1,'Vault fixture','fixture','[\"DkimSignature\"]',1)").bind(server).execute(&pool).await.unwrap();
    *fake.rows.lock().unwrap() = json!({"Domain":[{"id":"domain-keys","dkimManagement":{"@type":"Manual"}}],"DkimSignature":[{"id":"key-old","@type":"Dkim1RsaSha256","domainId":"domain-keys","selector":"old","stage":"active","publicKey":"YWJj","createdAt":"2026-10-08T00:00:00Z","nextTransitionAt":null}]});
    let app = ui_rust::router(state.clone());
    let admin = login(&app, "parity-admin@test.example").await;
    let(status,p,_)=request(&app,"/api/emergency-dkim",Some(json!({"server_id":server,"domain_id":domain,"old_key_id":"key-old","selector":"new-2026"})),Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{p}");
    assert!(p["public_value"]
        .as_str()
        .unwrap()
        .starts_with("v=DKIM1; k=rsa; p="));
    assert!(!p.to_string().contains("PRIVATE KEY"));
    let id = p["plan_id"].as_i64().unwrap();
    let (status, r, _) = request(
        &app,
        &format!("/api/emergency-dkim/{id}/apply"),
        Some(json!({"phase":"stage","confirmation":"STAGE_DKIM"})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{r}");
    assert_eq!(r["status"], "staged");
    let private: Option<String> =
        sqlx::query_scalar("SELECT private_encrypted FROM emergency_dkim_plans WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(private.is_none());
    let encrypted: String =
        sqlx::query_scalar("SELECT vault_encrypted FROM recovery_profiles WHERE server_id=$1")
            .bind(server)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!encrypted.contains("PRIVATE KEY"));
    let vault: Value =
        serde_json::from_str(&crypto::open(&state.config.key, &encrypted).unwrap()).unwrap();
    assert!(
        vault["DkimSignature"]["dkimsignature-new-credential"]["privateKey"]["secret"]
            .as_str()
            .unwrap()
            .contains("PRIVATE KEY")
    );
    assert_eq!(
        request(
            &app,
            &format!("/api/emergency-dkim/{id}/apply"),
            Some(json!({"phase":"stage","confirmation":"STAGE_DKIM"})),
            Some(&admin)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(fake.writes.load(std::sync::atomic::Ordering::SeqCst), 1);
    task.abort();
}

#[sqlx::test]
async fn server_capacity_serializes_customers_and_new_placements_keep_old_domains_pinned(
    pool: sqlx::PgPool,
) {
    let (state, owner, other, package) = parity_fixture(&pool).await;
    let (server, _, task) = mock_server(&state).await;
    let (target, _, target_task) = mock_server(&state).await;
    sqlx::query("UPDATE stalwart_servers SET is_primary=1 WHERE id=$1")
        .bind(server)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE server_capacity SET max_accounts=1 WHERE server_id=$1")
        .bind(server)
        .execute(&pool)
        .await
        .unwrap();
    let mut domains = Vec::new();
    let mut groups = Vec::new();
    for (customer, name) in [(owner, "capacity-a.example"), (other, "capacity-b.example")] {
        sqlx::query("INSERT INTO subscriptions(customer_id,package_id,price,start_date,end_date,status,selected_email_accounts,selected_domains) VALUES($1,$2,10,current_date,current_date+30,'active',2,2)").bind(customer).bind(package).execute(&pool).await.unwrap();
        let domain:i64=sqlx::query_scalar("INSERT INTO domains(customer_id,domain_name,server_id,stalwart_domain_id,sync_status,verification_status) VALUES($1,$2,$3,$2,'synced','verified') RETURNING id").bind(customer).bind(name).bind(server).fetch_one(&pool).await.unwrap();
        let group: i64 = sqlx::query_scalar(
            "INSERT INTO email_groups(customer_id,name) VALUES($1,'Group') RETURNING id",
        )
        .bind(customer)
        .fetch_one(&pool)
        .await
        .unwrap();
        domains.push(domain);
        groups.push(group);
    }
    let app = ui_rust::router(state);
    let a = login(&app, "parity@test.example").await;
    let b = login(&app, "other@test.example").await;
    let admin = login(&app, "parity-admin@test.example").await;
    let create = |domain, group| json!({"domain_id":domain,"group_id":group,"local_part":"mail","password":"Strong-test-password-123"});
    let (ra, rb) = tokio::join!(
        request(
            &app,
            "/api/resources/accounts",
            Some(create(domains[0], groups[0])),
            Some(&a)
        ),
        request(
            &app,
            "/api/resources/accounts",
            Some(create(domains[1], groups[1])),
            Some(&b)
        )
    );
    assert_eq!(
        usize::from(ra.0 == StatusCode::OK) + usize::from(rb.0 == StatusCode::OK),
        1,
        "{ra:?} {rb:?}"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM email_accounts WHERE server_id=$1")
        .bind(server)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let (status, r, _) = request(
        &app,
        "/api/placements",
        Some(json!({"server_id":target,"customer_id":owner,"version":0})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{r}");
    let (status, r, _) = request(
        &app,
        "/api/resources/domains",
        Some(json!({"domain_name":"new-capacity.example"})),
        Some(&a),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{r}");
    let placed: i64 = sqlx::query_scalar("SELECT server_id FROM domains WHERE id=$1")
        .bind(r["id"].as_i64().unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(placed, target);
    let pinned: i64 = sqlx::query_scalar("SELECT server_id FROM domains WHERE id=$1")
        .bind(domains[0])
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(pinned, server);
    assert_eq!(
        request(
            &app,
            "/api/placements",
            Some(json!({"server_id":target,"customer_id":owner,"version":0})),
            Some(&admin)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    task.abort();
    target_task.abort();
}

#[sqlx::test]
async fn guest_trial_creates_customer_only_after_approval_and_queues_private_reset(
    pool: sqlx::PgPool,
) {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let (mut state, _, _, package) = parity_fixture(&pool).await;
    let app = ui_rust::router(state.clone());
    let admin = login(&app, "parity-admin@test.example").await;
    let data = json!({"name":"Guest trial","email":"Guest@test.example","package_id":package,"phone":"01234","address":"Guest address","documents":{"citizen":{"mime":"application/pdf","data":STANDARD.encode(b"%PDF-1.7\nGuest private document")}}});
    let (a, b) = tokio::join!(
        request(&app, "/api/trials/public", Some(data.clone()), None),
        request(&app, "/api/trials/public", Some(data.clone()), None)
    );
    assert_eq!(a.0, StatusCode::OK, "{a:?}");
    assert_eq!(b.0, StatusCode::OK, "{b:?}");
    assert!(a.1.get("id").is_none());
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM trial_requests WHERE email='guest@test.example'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
    let id: i64 =
        sqlx::query_scalar("SELECT id FROM trial_requests WHERE email='guest@test.example'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM users WHERE email='guest@test.example'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
    assert_eq!(
        request(
            &app,
            &format!("/api/trials/{id}"),
            Some(json!({"approve":true})),
            Some(&admin)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    state.config.smtp_host = "smtp.fixture.invalid".into();
    let app = ui_rust::router(state.clone());
    sqlx::query("UPDATE email_templates SET enabled=0 WHERE template_key='password_reset'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        request(
            &app,
            &format!("/api/trials/{id}"),
            Some(json!({"approve":true})),
            Some(&admin)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM users WHERE email='guest@test.example'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0, "Failed welcome must roll back account creation");
    sqlx::query("UPDATE email_templates SET enabled=1 WHERE template_key='password_reset'")
        .execute(&pool)
        .await
        .unwrap();
    let (status, r, _) = request(
        &app,
        &format!("/api/trials/{id}"),
        Some(json!({"approve":true})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{r}");
    assert_eq!(
        request(
            &app,
            &format!("/api/trials/{id}"),
            Some(json!({"approve":true})),
            Some(&admin)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let owner: i64 = sqlx::query_scalar("SELECT customer_id FROM trial_requests WHERE id=$1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM subscriptions WHERE customer_id=$1 AND is_trial=1 AND status='active'").bind(owner).fetch_one(&pool).await.unwrap();
    assert_eq!(count, 1);
    let encrypted: String =
        sqlx::query_scalar("SELECT body_encrypted FROM notifications WHERE dedupe_key=$1")
            .bind(format!("trial-welcome:{id}"))
            .fetch_one(&pool)
            .await
            .unwrap();
    let body = crypto::open(&state.config.key, &encrypted).unwrap();
    let token = body
        .split("/reset-password#")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap();
    assert!(!encrypted.contains(token));
    let (status, r, _) = request(
        &app,
        "/api/auth/reset",
        Some(json!({"token":token,"password":"Guest-new-password-123"})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{r}");
    let (status, r, _) = request(
        &app,
        "/api/auth/login",
        Some(json!({"email":"guest@test.example","password":"Guest-new-password-123"})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{r}");
    sqlx::query("UPDATE users SET email='renamed-guest@test.example' WHERE id=$1")
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE subscriptions SET status='expired' WHERE customer_id=$1")
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
    let mut repeated = data.clone();
    repeated["email"] = json!("renamed-guest@test.example");
    assert_eq!(
        request(&app, "/api/trials/public", Some(repeated), None)
            .await
            .0,
        StatusCode::OK
    );
    let repeat: i64 = sqlx::query_scalar(
        "SELECT id FROM trial_requests WHERE email='renamed-guest@test.example'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        request(
            &app,
            &format!("/api/trials/{repeat}"),
            Some(json!({"approve":true})),
            Some(&admin)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let r = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/trials/public")
                .method("POST")
                .header("origin", "https://other.invalid")
                .header("content-type", "application/json")
                .body(Body::from(data.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
}

#[sqlx::test]
async fn panel_cloud_retrieval_verifies_encryption_checksum_and_retention_grace(
    pool: sqlx::PgPool,
) {
    use std::os::unix::fs::PermissionsExt;
    let (mut state, _, _, _) = parity_fixture(&pool).await;
    let mut url = url::Url::parse(&std::env::var("DATABASE_URL").unwrap()).unwrap();
    url.set_path(pool.connect_options().get_database().unwrap());
    state.config.database_url = url.to_string();
    let app = ui_rust::router(state.clone());
    let admin = login(&app, "parity-admin@test.example").await;
    let customer = login(&app, "parity@test.example").await;
    let (status, result, _) = request(&app, "/api/backups", Some(json!({})), Some(&admin)).await;
    assert_eq!(status, StatusCode::OK);
    let id = result["id"].as_i64().unwrap();
    ui_rust::backup_admin::tick(&state).await.unwrap();
    let name: String = sqlx::query_scalar("SELECT filename FROM rust_backups WHERE id=$1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    // Same stable directory namespace as production, distinct SQLx database per test.
    let host = url.host_str().unwrap();
    let namespace = &crypto::hash(&format!(
        "{}:{}{}",
        host,
        url.port().unwrap_or(5432),
        url.path()
    ))[..16];
    let root =
        std::path::PathBuf::from(std::env::var("BACKUP_DIRECTORY").unwrap_or("backups".into()))
            .join(namespace);
    let local = root.join(&name);
    let bytes = std::fs::read(&local).unwrap();
    let fixture = std::path::PathBuf::from(format!("/tmp/ui-rclone-test-{}", crypto::token()));
    std::fs::create_dir(&fixture).unwrap();
    std::fs::set_permissions(&fixture, std::fs::Permissions::from_mode(0o700)).unwrap();
    let config = fixture.join("rclone.conf");
    std::fs::write(&config, "[fixturelocal]\ntype = local\n").unwrap();
    let tool = fixture.join("rclone");
    std::fs::write(
        &tool,
        format!(
            "#!/bin/sh\nexec /usr/bin/rclone --config '{}' \"$@\"\n",
            config.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::env::set_var("BACKUP_RCLONE_BIN", &tool);
    let cloud = fixture.join("cloud");
    std::fs::create_dir(&cloud).unwrap();
    std::fs::write(cloud.join(&name), b"invalid encrypted backup").unwrap();
    std::fs::remove_file(&local).unwrap();
    let destination = format!("fixturelocal:{}", cloud.display());
    sqlx::query("UPDATE rust_backup_policy SET destination=$1")
        .bind(&destination)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE rust_backups SET local_available=false,cloud_status='uploaded',cloud_path=$1 WHERE id=$2").bind(format!("{destination}/{name}")).bind(id).execute(&pool).await.unwrap();
    assert_eq!(
        request(
            &app,
            &format!("/api/backups/{id}/retrieve"),
            Some(json!({})),
            Some(&customer)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            &format!("/api/backups/{id}/retrieve"),
            Some(json!({})),
            Some(&admin)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            &format!("/api/backups/{id}/retrieve"),
            Some(json!({})),
            Some(&admin)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    ui_rust::backup_admin::tick(&state).await.unwrap();
    let status: String =
        sqlx::query_scalar("SELECT retrieval_status FROM rust_backups WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "failed");
    assert!(!local.exists());
    std::fs::write(cloud.join(&name), &bytes).unwrap();
    assert_eq!(
        request(
            &app,
            &format!("/api/backups/{id}/retrieve"),
            Some(json!({})),
            Some(&admin)
        )
        .await
        .0,
        StatusCode::OK
    );
    ui_rust::backup_admin::tick(&state).await.unwrap();
    assert_eq!(std::fs::read(&local).unwrap(), bytes);
    let status: String =
        sqlx::query_scalar("SELECT retrieval_status FROM rust_backups WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "done");
    assert_eq!(
        request(&app, "/api/backups", Some(json!({})), Some(&admin))
            .await
            .0,
        StatusCode::OK
    );
    sqlx::query("UPDATE rust_backup_policy SET keep_local=1")
        .execute(&pool)
        .await
        .unwrap();
    ui_rust::backup_admin::tick(&state).await.unwrap();
    assert!(
        local.exists(),
        "Retrieved archive has a 24-hour download grace period"
    );
    sqlx::query(
        "UPDATE rust_backups SET retrieval_finished_at=now()-interval '25 hours' WHERE id=$1",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();
    ui_rust::backup_admin::tick(&state).await.unwrap();
    assert!(!local.exists());
    sqlx::query("UPDATE rust_backup_policy SET destination='other:folder'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        request(
            &app,
            &format!("/api/backups/{id}/retrieve"),
            Some(json!({})),
            Some(&admin)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    std::env::remove_var("BACKUP_RCLONE_BIN");
    std::fs::remove_dir_all(fixture).unwrap();
}

#[sqlx::test]
async fn reconciliation_guards_local_remote_state_and_verifies_each_write(pool: sqlx::PgPool) {
    let (state, owner, _, _) = parity_fixture(&pool).await;
    let (server, fake, task) = mock_server(&state).await;
    fake.apply_updates
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let domain:i64=sqlx::query_scalar("INSERT INTO domains(customer_id,domain_name,server_id,stalwart_domain_id,sync_status) VALUES($1,'reconcile.example',$2,'domain-r','synced') RETURNING id").bind(owner).bind(server).fetch_one(&pool).await.unwrap();
    let account:i64=sqlx::query_scalar("INSERT INTO email_accounts(customer_id,domain_id,server_id,email,local_part,stalwart_account_id,storage_limit_mb,sync_status) VALUES($1,$2,$3,'mail@reconcile.example','mail','account-r',4,'synced') RETURNING id").bind(owner).bind(domain).bind(server).fetch_one(&pool).await.unwrap();
    *fake.rows.lock().unwrap() = json!({"Domain":[{"id":"domain-r","name":"reconcile.example","isEnabled":false},{"id":"domain-orphan","name":"orphan.example","isEnabled":true}],"Account":[{"id":"account-r","name":"mail","domainId":"domain-r","quotas":{"maxDiskQuota":2097152},"usedDiskQuota":0,"aliases":{}}]});
    let app = ui_rust::router(state);
    let admin = login(&app, "parity-admin@test.example").await;
    let customer = login(&app, "parity@test.example").await;
    assert_eq!(
        request(
            &app,
            "/api/reconciliation",
            Some(json!({"server_id":server})),
            Some(&customer)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, r, _) = request(
        &app,
        "/api/reconciliation",
        Some(json!({"server_id":server})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{r}");
    assert_eq!(r["summary"], json!({"equal":0,"diff":2,"manual":1}));
    let run = r["run_id"].as_i64().unwrap();
    let items = request(
        &app,
        &format!("/api/reconciliation/{run}"),
        None,
        Some(&admin),
    )
    .await
    .1["items"]
        .as_array()
        .unwrap()
        .clone();
    let d = items
        .iter()
        .find(|i| i["kind"] == "domain" && i["status"] == "diff")
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    let a = items.iter().find(|i| i["kind"] == "account").unwrap()["id"]
        .as_i64()
        .unwrap();
    sqlx::query("UPDATE email_accounts SET storage_limit_mb=7 WHERE id=$1")
        .bind(account)
        .execute(&pool)
        .await
        .unwrap();
    let (status, r, _) = request(
        &app,
        &format!("/api/reconciliation/{run}/apply"),
        Some(json!({"confirmation":"RECONCILE","item_ids":[d,a]})),
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{r}");
    assert_eq!(r["applied"], 1);
    assert_eq!(r["stale"], 1);
    assert_eq!(fake.writes.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(fake.rows.lock().unwrap()["Domain"][0]["isEnabled"], true);
    let r = request(
        &app,
        &format!("/api/reconciliation/{run}/apply"),
        Some(json!({"confirmation":"RECONCILE","item_ids":[d,a]})),
        Some(&admin),
    )
    .await
    .1;
    assert_eq!(r["skipped"], 2);
    assert_eq!(fake.writes.load(std::sync::atomic::Ordering::SeqCst), 1);
    let r = request(
        &app,
        "/api/reconciliation",
        Some(json!({"server_id":server})),
        Some(&admin),
    )
    .await
    .1;
    let run = r["run_id"].as_i64().unwrap();
    let items = request(
        &app,
        &format!("/api/reconciliation/{run}"),
        None,
        Some(&admin),
    )
    .await
    .1["items"]
        .as_array()
        .unwrap()
        .clone();
    let a = items.iter().find(|i| i["kind"] == "account").unwrap()["id"]
        .as_i64()
        .unwrap();
    fake.rows.lock().unwrap()["Account"][0]["name"] = json!("remote-changed");
    let r = request(
        &app,
        &format!("/api/reconciliation/{run}/apply"),
        Some(json!({"confirmation":"RECONCILE","item_ids":[a]})),
        Some(&admin),
    )
    .await
    .1;
    assert_eq!(r["stale"], 1);
    assert_eq!(fake.writes.load(std::sync::atomic::Ordering::SeqCst), 1);
    fake.apply_updates
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let r = request(
        &app,
        "/api/reconciliation",
        Some(json!({"server_id":server})),
        Some(&admin),
    )
    .await
    .1;
    let run = r["run_id"].as_i64().unwrap();
    let items = request(
        &app,
        &format!("/api/reconciliation/{run}"),
        None,
        Some(&admin),
    )
    .await
    .1["items"]
        .as_array()
        .unwrap()
        .clone();
    let a = items.iter().find(|i| i["kind"] == "account").unwrap()["id"]
        .as_i64()
        .unwrap();
    let r = request(
        &app,
        &format!("/api/reconciliation/{run}/apply"),
        Some(json!({"confirmation":"RECONCILE","item_ids":[a]})),
        Some(&admin),
    )
    .await
    .1;
    assert_eq!(r["uncertain"], 1);
    assert_eq!(fake.writes.load(std::sync::atomic::Ordering::SeqCst), 2);
    let r = request(
        &app,
        &format!("/api/reconciliation/{run}/apply"),
        Some(json!({"confirmation":"RECONCILE","item_ids":[a]})),
        Some(&admin),
    )
    .await
    .1;
    assert_eq!(r["skipped"], 1);
    assert_eq!(fake.writes.load(std::sync::atomic::Ordering::SeqCst), 2);
    task.abort();
}
