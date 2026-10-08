use ui_rust::{config::Config, crypto};
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ui_rust=info,tower_http=info".into()),
        )
        .init();
    let cmd = std::env::args().nth(1).unwrap_or("serve".into());
    if cmd == "keygen" {
        println!("{}", crypto::token());
        return Ok(());
    }
    let config = Config::load()?;
    if cmd == "backup" {
        let path = ui_rust::backup::create(&config, std::path::Path::new("backups")).await?;
        println!("{}", path.display());
        return Ok(());
    }
    if cmd == "restore" {
        let args: Vec<String> = std::env::args().collect();
        if args.len() != 5 || args[3] != "--confirm-db" {
            anyhow::bail!("Usage: ui-rust restore FILE --confirm-db DATABASE")
        }
        ui_rust::backup::restore(&config, std::path::Path::new(&args[2]), &args[4]).await?;
        return Ok(());
    }
    let s = ui_rust::state(config).await?;
    match cmd.as_str() {
        "migrate" => {}
        "create-admin" => {
            let email = std::env::var("ADMIN_EMAIL")?;
            let password = std::env::var("ADMIN_PASSWORD")?;
            if !ui_rust::auth::valid_email(&email) {
                anyhow::bail!("Invalid ADMIN_EMAIL")
            }
            let hash = crypto::password(&password)?;
            sqlx::query("INSERT INTO users(name,email,password_hash,role,admin_level,two_factor_enabled) VALUES('Administrator',$1,$2,'admin','super',0)").bind(email.to_lowercase()).bind(hash).execute(&s.db).await?;
            println!("Administrator created. Sign in and enable TOTP in Account security.");
        }
        "recovery-once" => ui_rust::recovery::tick(&s).await.map_err(|e|anyhow::anyhow!(e.1))?,
        "recovery-worker" => loop {
            tokio::select! {_=tokio::signal::ctrl_c()=>break,result=ui_rust::recovery::tick(&s)=>{if let Err(e)=result{tracing::error!(error=%e.1,"Recovery worker failed");}}}
            tokio::select! {_=tokio::signal::ctrl_c()=>break,_=tokio::time::sleep(std::time::Duration::from_secs(5))=>{}}
        },
        "mailbox-once" => ui_rust::mailbox_backup::tick(&s).await.map_err(|e|anyhow::anyhow!(e.1))?,
        "mailbox-worker" => loop {
            tokio::select! { _=tokio::signal::ctrl_c()=>break, result=ui_rust::mailbox_backup::tick(&s)=>{if let Err(e)=result {tracing::error!(error=%e.1,"Mailbox worker failed");}} }
            tokio::select! {_=tokio::signal::ctrl_c()=>break,_=tokio::time::sleep(std::time::Duration::from_secs(5))=>{}}
        },
        "worker-once" => ui_rust::worker::tick(&s).await?,
        "worker" => loop {
            if let Err(e) = ui_rust::worker::tick(&s).await {
                tracing::error!(error=%e,"Worker tick failed")
            }
            tokio::select! {_=tokio::signal::ctrl_c()=>break,_=tokio::time::sleep(std::time::Duration::from_secs(3))=>{}}
        },
        "serve" => {
            let addr = s.config.bind.clone();
            let listener = tokio::net::TcpListener::bind(&addr).await?;
            tracing::info!(%addr,"HTTP server started");
            axum::serve(listener, ui_rust::router(s))
                .with_graceful_shutdown(async {
                    tokio::signal::ctrl_c().await.ok();
                })
                .await?;
        }
        _ => anyhow::bail!(
            "Usage: ui-rust [serve|worker|worker-once|mailbox-worker|mailbox-once|recovery-worker|recovery-once|migrate|create-admin|keygen|backup|restore]"
        ),
    };
    Ok(())
}
