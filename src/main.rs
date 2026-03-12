use std::net::SocketAddr;

use tracing_subscriber::EnvFilter;

mod api;
mod config;
mod db;
mod events;
mod mail;
mod models;
mod stalwart;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let config = config::Config::load()?;
    let pool = db::connect(&config.database_url).await?;

    let stalwart_client = match (&config.stalwart_url, &config.stalwart_admin_token) {
        (Some(url), Some(token)) => {
            tracing::info!("stalwart client configured at {url}");
            Some(stalwart::StalwartClient::new(url, token))
        }
        _ => {
            tracing::info!("stalwart not configured, email delivery disabled");
            None
        }
    };

    let webhook_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("failed to build webhook client");

    let mut guard_patterns = mail::guard::default_patterns();
    if let Some(custom) = &config.guard_patterns {
        for cp in custom {
            match regex::Regex::new(&cp.pattern) {
                Ok(re) => guard_patterns.push(mail::guard::GuardPattern {
                    name: cp.name.clone(),
                    regex: re,
                }),
                Err(e) => tracing::warn!("invalid guard pattern '{}': {e}", cp.name),
            }
        }
    }

    let state = api::AppState {
        pool,
        stalwart: stalwart_client,
        webhook_client,
        inbound_token: config.stalwart_inbound_token,
        guard_patterns,
    };
    let app = api::router(state);

    let addr: SocketAddr = format!("{}:{}", config.host, config.port).parse()?;
    tracing::info!("postblox listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
