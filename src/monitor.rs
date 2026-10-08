use crate::{
    auth, crypto,
    error::{Error, Result},
    management, resources, stalwart, App,
};
use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    Json,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{PgConnection, Row};
async fn signal(
    db: &mut PgConnection,
    server: i64,
    key: &str,
    active: bool,
    message: &str,
    context: Value,
) -> Result<()> {
    let fingerprint = crypto::hash(&format!("{server}:{key}"));
    if !active {
        sqlx::query("UPDATE operational_alerts SET status='resolved',resolved_at=now() WHERE fingerprint=$1 AND status<>'resolved'").bind(fingerprint).execute(db).await?;
        return Ok(());
    }
    sqlx::query("INSERT INTO operational_alerts(server_id,rule_key,fingerprint,severity,message,context,first_seen_at,last_seen_at) VALUES($1,$2,$3,'warning',$4,$5,now(),now()) ON CONFLICT(fingerprint) DO UPDATE SET first_seen_at=CASE WHEN operational_alerts.status='resolved' THEN now() ELSE operational_alerts.first_seen_at END,acknowledged_by=CASE WHEN operational_alerts.status='resolved' THEN NULL ELSE operational_alerts.acknowledged_by END,acknowledged_at=CASE WHEN operational_alerts.status='resolved' THEN NULL ELSE operational_alerts.acknowledged_at END,last_notified_at=CASE WHEN operational_alerts.status='resolved' THEN NULL ELSE operational_alerts.last_notified_at END,status=CASE WHEN operational_alerts.status='resolved' THEN 'open' ELSE operational_alerts.status END,resolved_at=NULL,last_seen_at=now(),message=excluded.message,context=excluded.context").bind(server).bind(key).bind(fingerprint).bind(message).bind(context).execute(db).await?;
    Ok(())
}
pub async fn sample(s: &App, server: i64) -> Result<Value> {
    let mut tx = s.db.begin().await?;
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
        .bind(
            900000000_i64
                .checked_add(server)
                .ok_or_else(Error::missing)?,
        )
        .fetch_one(&mut *tx)
        .await?;
    if !locked {
        return Err(Error::conflict("Server đang được kiểm tra"));
    }
    let r = sqlx::query("SELECT * FROM stalwart_servers WHERE id=$1 AND active=1 FOR SHARE")
        .bind(server)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(Error::missing)?;
    let dry = r.get::<i16, _>("dry_run") == 1;
    let began = std::time::Instant::now();
    let mut metrics = json!({"api_up":null,"queue_total":null,"accounts_total":null,"domains_total":null,"smtp_up":null,"imap_up":null,"tls_days":null});
    if !dry {
        let meta = stalwart::metadata(s, server, "/api/account").await;
        metrics["api_up"] = json!(meta.is_ok());
        if let Ok(meta) = meta {
            let permissions = meta["data"]["permissions"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            for (kind, key, permission) in [
                ("Account", "accounts_total", "sysAccountQuery"),
                ("Domain", "domains_total", "sysDomainQuery"),
                ("QueuedMessage", "queue_total", "sysQueuedMessageQuery"),
            ] {
                if permissions.iter().any(|v| v == permission) {
                    if let Ok(value) = management::method(
                        s,
                        server,
                        &format!("x:{kind}/query"),
                        json!({"limit":1,"calculateTotal":true}),
                    )
                    .await
                    {
                        if let Some(total) = value["total"].as_i64().filter(|n| *n >= 0) {
                            metrics[key] = json!(total);
                        }
                    }
                }
            }
            if permissions.iter().any(|p| p == "sysCertificateQuery")
                && permissions.iter().any(|p| p == "sysCertificateGet")
            {
                certificates(s, &mut tx, server, r.get("tls_warn_days"), &mut metrics).await?;
            }
            sqlx::query("UPDATE stalwart_servers SET permissions=$1 WHERE id=$2")
                .bind(json!(permissions))
                .bind(server)
                .execute(&mut *tx)
                .await?;
        }
        if r.get::<i16, _>("probe_ports") == 1 {
            let url = url::Url::parse(&r.get::<String, _>("base_url"))
                .map_err(|_| Error::bad("URL không hợp lệ"))?;
            let host = url.host_str().ok_or_else(Error::missing)?;
            for (field, key) in [("smtp_port", "smtp_up"), ("imap_port", "imap_up")] {
                let port = u16::try_from(r.get::<i32, _>(field))
                    .map_err(|_| Error::bad("Port không hợp lệ"))?;
                let up = matches!(
                    tokio::time::timeout(
                        std::time::Duration::from_secs(3),
                        tokio::net::TcpStream::connect((host, port))
                    )
                    .await,
                    Ok(Ok(_))
                );
                metrics[key] = json!(up);
            }
        }
    }
    metrics["latency_ms"] = json!(began.elapsed().as_millis() as u64);
    let streak = if metrics["api_up"] == false {
        r.get::<i32, _>("failure_streak").saturating_add(1)
    } else {
        0
    };
    if r.get::<i16, _>("is_primary") == 1 {
        let jobs=sqlx::query("SELECT count(*) FILTER(WHERE status='failed') failed,count(*) FILTER(WHERE status='pending') pending FROM api_sync_jobs").fetch_one(&mut *tx).await?;
        metrics["sync_failed"] = json!(jobs.get::<i64, _>("failed"));
        metrics["sync_pending"] = json!(jobs.get::<i64, _>("pending"));
        let age:Option<f64>=sqlx::query_scalar("SELECT extract(epoch FROM now()-heartbeat_at)::double precision/60 FROM rust_worker_state WHERE id=1").fetch_optional(&mut *tx).await?;
        metrics["sync_worker_age_minutes"] = json!(age);
        let backup:Option<f64>=sqlx::query_scalar("SELECT extract(epoch FROM now()-max(finished_at))::double precision/3600 FROM rust_backups WHERE status='done'").fetch_one(&mut *tx).await?;
        metrics["backup_age_hours"] = json!(backup);
        signal(
            &mut tx,
            server,
            "sync_failed",
            metrics["sync_failed"].as_i64().unwrap_or(0) > 0,
            "Có tác vụ đồng bộ bị lỗi",
            json!({"count":metrics["sync_failed"]}),
        )
        .await?;
        signal(
            &mut tx,
            server,
            "sync_worker_late",
            metrics["sync_pending"].as_i64().unwrap_or(0) > 0
                && age.is_none_or(|n| n > f64::from(r.get::<i32, _>("worker_max_age_minutes"))),
            "Worker đồng bộ bị trễ hoặc chưa chạy",
            json!({"age_minutes":age}),
        )
        .await?;
        let limit = r.get::<i32, _>("backup_max_age_hours");
        signal(
            &mut tx,
            server,
            "backup_late",
            limit > 0 && backup.is_none_or(|n| n > f64::from(limit)),
            "Backup panel quá hạn hoặc chưa có bản hoàn tất",
            json!({"age_hours":backup}),
        )
        .await?;
    }
    if !dry {
        signal(
            &mut tx,
            server,
            "api_down",
            streak >= r.get::<i32, _>("failure_threshold"),
            "Stalwart API mất kết nối",
            json!({"failure_streak":streak}),
        )
        .await?;
        if let Some(queue) = metrics["queue_total"].as_i64() {
            signal(
                &mut tx,
                server,
                "queue_high",
                queue >= i64::from(r.get::<i32, _>("queue_threshold")),
                "Hàng đợi giao thư vượt ngưỡng",
                json!({"count":queue}),
            )
            .await?;
        }
        for key in ["smtp", "imap"] {
            if let Some(up) = metrics[format!("{key}_up")].as_bool() {
                signal(
                    &mut tx,
                    server,
                    &format!("{key}_down"),
                    !up,
                    &format!("Không kết nối được {}", key.to_uppercase()),
                    json!({}),
                )
                .await?;
            }
        }
        if r.get::<i16, _>("usage_monitor") == 1 && metrics["api_up"] == true {
            usage(s, &mut tx, server).await?;
        }
    }
    let status = if dry {
        "simulated"
    } else if metrics["api_up"] == false {
        "down"
    } else {
        "reachable"
    };
    sqlx::query(
        "INSERT INTO server_health_samples(server_id,mode,status,metrics) VALUES($1,$2,$3,$4)",
    )
    .bind(server)
    .bind(if dry { "dry_run" } else { "live" })
    .bind(status)
    .bind(&metrics)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE stalwart_servers SET failure_streak=$1,last_check_at=now(),next_check_at=CASE WHEN next_check_at IS NULL THEN NULL ELSE now()+make_interval(secs=>monitor_interval) END WHERE id=$2").bind(streak).bind(server).execute(&mut *tx).await?;
    if r.get::<i16, _>("notify_enabled") == 1 && !s.config.smtp_host.is_empty() {
        let recipients: Value = r
            .get::<Option<Value>, _>("notify_recipients")
            .unwrap_or(json!([]));
        let alerts=sqlx::query("SELECT id,message,rule_key FROM operational_alerts WHERE server_id=$1 AND status='open' AND (last_notified_at IS NULL OR last_notified_at<now()-interval '1 hour') LIMIT 20 FOR UPDATE").bind(server).fetch_all(&mut *tx).await?;
        for a in alerts {
            for address in recipients
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|a| auth::valid_email(a))
            {
                sqlx::query("INSERT INTO notifications(recipient,subject,body_encrypted,dedupe_key) VALUES($1,$2,$3,$4) ON CONFLICT(dedupe_key) DO NOTHING").bind(address).bind(format!("[Stalwart] {}: {}",r.get::<String,_>("name"),a.get::<String,_>("rule_key"))).bind(crypto::seal(&s.config.key,&a.get::<String,_>("message"))?).bind(format!("alert:{}:{}:{}",a.get::<i64,_>("id"),Utc::now().format("%Y%m%d%H"),crypto::hash(address))).execute(&mut *tx).await?;
            }
            sqlx::query("UPDATE operational_alerts SET last_notified_at=now() WHERE id=$1")
                .bind(a.get::<i64, _>("id"))
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    Ok(json!({"status":status,"mode":if dry{"dry_run"}else{"live"},"metrics":metrics}))
}
async fn usage(s: &App, db: &mut PgConnection, server: i64) -> Result<()> {
    let rows=sqlx::query("SELECT id,stalwart_account_id,storage_limit_mb FROM email_accounts WHERE server_id=$1 AND status='active' AND sync_status='synced' AND stalwart_account_id IS NOT NULL AND (storage_used_synced_at IS NULL OR storage_used_synced_at<now()-interval '6 hours') ORDER BY storage_used_synced_at NULLS FIRST LIMIT 5 FOR UPDATE SKIP LOCKED").bind(server).fetch_all(&mut *db).await?;
    if rows.is_empty() {
        return Ok(());
    }
    let ids: Vec<String> = rows.iter().map(|r| r.get("stalwart_account_id")).collect();
    let Ok(response) = management::method(
        s,
        server,
        "x:Account/get",
        json!({"ids":ids,"properties":["id","usedDiskQuota"]}),
    )
    .await
    else {
        return Ok(());
    };
    let Some(accounts) = response["list"].as_array() else {
        return Ok(());
    };
    for r in rows {
        let remote: String = r.get("stalwart_account_id");
        let found: Vec<_> = accounts.iter().filter(|v| v["id"] == remote).collect();
        if found.len() != 1 {
            continue;
        }
        let Some(used) = found[0]["usedDiskQuota"].as_i64().filter(|n| *n >= 0) else {
            continue;
        };
        let account: i64 = r.get("id");
        sqlx::query("UPDATE email_accounts SET storage_used_bytes=$1,storage_used_synced_at=now() WHERE id=$2").bind(used).bind(account).execute(&mut *db).await?;
        sqlx::query("INSERT INTO account_usage_history(account_id,sample_date,used_bytes) VALUES($1,current_date,$2) ON CONFLICT(account_id,sample_date) DO UPDATE SET used_bytes=excluded.used_bytes").bind(account).bind(used).execute(&mut *db).await?;
        let quota = i64::from(r.get::<i32, _>("storage_limit_mb")) * 1048576;
        signal(
            db,
            server,
            &format!("quota_account_{account}"),
            quota > 0 && i128::from(used) * 100 >= i128::from(quota) * 90,
            "Dung lượng hộp thư vượt 90% quota",
            json!({"account_id":account,"used_bytes":used,"quota_bytes":quota}),
        )
        .await?;
    }
    Ok(())
}
pub async fn tick(s: &App) -> Result<()> {
    let due:Option<i64>=sqlx::query_scalar("SELECT id FROM stalwart_servers WHERE active=1 AND next_check_at<=now() ORDER BY next_check_at LIMIT 1").fetch_optional(&s.db).await?;
    if let Some(id) = due {
        sample(s, id).await?;
    }
    sqlx::query("DELETE FROM server_health_samples WHERE created_at<now()-interval '90 days'")
        .execute(&s.db)
        .await?;
    sqlx::query("DELETE FROM account_usage_history WHERE sample_date<current_date-90")
        .execute(&s.db)
        .await?;
    sqlx::query("DELETE FROM mail_activity_events WHERE occurred_at<now()-interval '30 days'")
        .execute(&s.db)
        .await?;
    Ok(())
}
pub async fn check(State(s): State<App>, h: HeaderMap, Path(id): Path<i64>) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "operations", true, true).await?;
    u.super_admin()?;
    let r = sample(&s, id).await?;
    auth::audit(
        &s,
        Some(u.id),
        "server_health_check",
        json!({"server_id":id}),
    )
    .await?;
    Ok(Json(r))
}
pub async fn configure(
    State(s): State<App>,
    h: HeaderMap,
    Path(id): Path<i64>,
    Json(v): Json<Value>,
) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "settings", true, true).await?;
    u.super_admin()?;
    let interval = v["monitor_interval"]
        .as_i64()
        .filter(|n| (60..=86400).contains(n))
        .ok_or_else(|| Error::bad("Chu kỳ từ 60 đến 86400 giây"))?;
    let failure = v["failure_threshold"]
        .as_i64()
        .filter(|n| (1..=20).contains(n))
        .ok_or_else(|| Error::bad("Ngưỡng lỗi từ 1 đến 20"))?;
    let queue = resources::number(&v, "queue_threshold")?;
    if queue > 1_000_000_000 {
        return Err(Error::bad("Ngưỡng hàng đợi không hợp lệ"));
    }
    let recipients = v.get("notify_recipients").cloned().unwrap_or(json!([]));
    if !recipients.as_array().is_some_and(|a| {
        a.len() <= 20 && a.iter().all(|v| v.as_str().is_some_and(auth::valid_email))
    }) {
        return Err(Error::bad("Tối đa 20 email nhận cảnh báo"));
    }
    let n=sqlx::query("UPDATE stalwart_servers SET monitor_interval=$1,failure_threshold=$2,queue_threshold=$3,next_check_at=CASE WHEN $4 THEN now() ELSE NULL END,usage_monitor=$5,probe_ports=$6,notify_enabled=$7,notify_recipients=$8,updated_at=now() WHERE id=$9").bind(interval as i32).bind(failure as i32).bind(queue as i32).bind(v["enabled"]==true).bind(if v["usage_monitor"]==true{1_i16}else{0}).bind(if v["probe_ports"]==true{1_i16}else{0}).bind(if v["notify_enabled"]==true{1_i16}else{0}).bind(recipients).bind(id).execute(&s.db).await?.rows_affected();
    if n == 0 {
        return Err(Error::missing());
    }
    auth::audit(
        &s,
        Some(u.id),
        "monitor_configured",
        json!({"server_id":id}),
    )
    .await?;
    Ok(Json(json!({"ok":true})))
}
#[derive(Deserialize)]
pub struct Summary {
    server_id: i64,
}
pub async fn history(
    State(s): State<App>,
    h: HeaderMap,
    Query(q): Query<Summary>,
) -> Result<Json<Value>> {
    let u = auth::require(&s, &h, "operations", true, false).await?;
    u.super_admin()?;
    let rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(h) FROM server_health_samples h WHERE server_id=$1 AND created_at>=now()-interval '30 days' ORDER BY id DESC LIMIT 10000").bind(q.server_id).fetch_all(&s.db).await?;
    let mut days: std::collections::BTreeMap<String, Value> = Default::default();
    for row in &rows {
        if row["mode"] != "live" {
            continue;
        }
        let date = row["created_at"]
            .as_str()
            .unwrap_or("")
            .get(..10)
            .unwrap_or("");
        let day=days.entry(date.into()).or_insert(json!({"date":date,"checks":0,"api_pass":0,"queue_max":null,"sync_max":null,"accounts":null,"domains":null}));
        day["checks"] = json!(day["checks"].as_i64().unwrap() + 1);
        day["api_pass"] = json!(
            day["api_pass"].as_i64().unwrap()
                + if row["metrics"]["api_up"] == true {
                    1
                } else {
                    0
                }
        );
        for (key, out) in [("queue_total", "queue_max"), ("sync_failed", "sync_max")] {
            if let Some(n) = row["metrics"][key].as_i64() {
                day[out] = json!(day[out].as_i64().unwrap_or(0).max(n));
            }
        }
        for (key, out) in [("accounts_total", "accounts"), ("domains_total", "domains")] {
            if day[out].is_null() && !row["metrics"][key].is_null() {
                day[out] = row["metrics"][key].clone();
            }
        }
    }
    let usage:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',id,'email',email,'storage_limit_mb',storage_limit_mb,'used_bytes',storage_used_bytes,'synced_at',storage_used_synced_at) FROM email_accounts WHERE server_id=$1 AND status<>'deleted' ORDER BY storage_used_bytes DESC LIMIT 200").bind(q.server_id).fetch_all(&s.db).await?;
    Ok(Json(
        json!({"items":rows,"daily":days.values().collect::<Vec<_>>(),"accounts":usage}),
    ))
}

async fn certificates(
    s: &App,
    db: &mut PgConnection,
    server: i64,
    warn_days: i32,
    metrics: &mut Value,
) -> Result<()> {
    let mut keys = Vec::new();
    let mut earliest: Option<i64> = None;
    let mut complete = false;
    let mut valid = true;
    let mut count = 0_usize;
    for page in 0..4_i64 {
        let Ok(result) = management::page(s, server, "certificates", page * 50, json!({})).await
        else {
            break;
        };
        let rows = result["items"].as_array().ok_or_else(Error::missing)?;
        count += rows.len();
        for row in rows {
            let Some(expiry) = row["notValidAfter"]
                .as_str()
                .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
            else {
                valid = false;
                continue;
            };
            let seconds = expiry.signed_duration_since(Utc::now()).num_seconds();
            let days = seconds.div_euclid(86400);
            earliest = Some(earliest.map_or(days, |old| old.min(days)));
            let key = format!(
                "certificate_{}",
                &crypto::hash(row["id"].as_str().unwrap())[..32]
            );
            keys.push(key.clone());
            signal(db, server, &key, seconds <= i64::from(warn_days.max(0))*86400,
                "Chứng chỉ TLS trên server sắp hết hạn hoặc đã hết hạn",
                json!({"certificate_id":row["id"],"expires":row["notValidAfter"],"subject_alternative_names":row["subjectAlternativeNames"]})).await?;
        }
        if result["next"].is_null() {
            complete = result["total"].as_u64().is_some_and(|n| n == count as u64);
            break;
        }
    }
    metrics["tls_certificate_scan_complete"] = json!(complete && valid);
    metrics["tls_certificates_observed"] = json!(count);
    metrics["tls_days"] = json!(earliest);
    if complete && valid {
        sqlx::query("UPDATE operational_alerts SET status='resolved',resolved_at=now() WHERE server_id=$1 AND left(rule_key,12)='certificate_' AND NOT(rule_key=ANY($2)) AND status<>'resolved'").bind(server).bind(keys).execute(db).await?;
    }
    Ok(())
}
