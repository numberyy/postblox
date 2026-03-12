use std::net::SocketAddr;

use tracing_subscriber::EnvFilter;

mod api;
mod config;
mod core;
mod db;
mod embeddings;
mod events;
mod mail;
mod models;
mod notifications;
mod stalwart;
mod sync;

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

    let embedding_provider: Option<std::sync::Arc<dyn embeddings::EmbeddingProvider>> =
        match &config.embedding_url {
            Some(url) => {
                let model = config
                    .embedding_model
                    .as_deref()
                    .unwrap_or("text-embedding-3-small");
                tracing::info!("embedding provider: {url}, model: {model}");
                Some(std::sync::Arc::new(
                    embeddings::openai::OpenAiProvider::new(
                        url,
                        model,
                        config.embedding_api_key.clone(),
                        768,
                    ),
                ))
            }
            None => {
                tracing::info!("no embedding provider configured, semantic search disabled");
                None
            }
        };

    if config.relay.is_some() {
        tracing::info!("SMTP relay configured");
    }

    let state = api::AppState {
        pool,
        stalwart: stalwart_client,
        webhook_client,
        inbound_token: config.stalwart_inbound_token,
        guard_patterns,
        relay: config.relay,
        embedding_provider,
        embedding_semaphore: std::sync::Arc::new(tokio::sync::Semaphore::new(20)),
        trust_auto_upgrade_threshold: config.trust_auto_upgrade_threshold,
    };
    let app = api::router(state);

    let addr: SocketAddr = format!("{}:{}", config.host, config.port).parse()?;
    tracing::info!("postblox listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
