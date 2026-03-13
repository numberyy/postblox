use std::net::SocketAddr;

use postblox::{api, config, dashboard, db, embeddings, events, hooks, mail, stalwart};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let config = config::Config::load()?;
    let pool = db::connect(&config.database_url).await?;

    sqlx::migrate!("./migrations").run(&pool).await?;
    tracing::info!("migrations applied");

    let stalwart_client = match (&config.stalwart_url, &config.stalwart_admin_token) {
        (Some(url), Some(token)) => {
            let user = config.stalwart_admin_user.as_deref().unwrap_or("admin");
            tracing::info!("stalwart client configured at {url}");
            Some(stalwart::StalwartClient::new(
                url,
                user,
                token,
                config.stalwart_smtp_host.as_deref(),
                config.stalwart_smtp_port,
            ))
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

    let raw_hooks = config.hooks.unwrap_or_default();
    for h in &raw_hooks {
        if h.event != "before_send" && !events::KNOWN_EVENTS.contains(&h.event.as_str()) {
            tracing::warn!("unknown hook event '{}' — will never fire", h.event);
        }
    }
    let hooks: std::sync::Arc<[hooks::HookConfig]> = raw_hooks.into();

    let state = api::AppState {
        pool,
        stalwart: stalwart_client,
        webhook_client,
        inbound_token: config.stalwart_inbound_token,
        guard_patterns,
        embedding_provider,
        embedding_semaphore: std::sync::Arc::new(tokio::sync::Semaphore::new(20)),
        trust_auto_upgrade_threshold: config.trust_auto_upgrade_threshold,
        hooks,
        ws_hub: std::sync::Arc::new(events::websocket::WebSocketHub::new()),
        rate_limiter: std::sync::Arc::new(api::rate_limit::RateLimiter::new(
            config.rate_limit.requests_per_minute,
            config.rate_limit.requests_per_hour,
        )),
    };
    let templates = dashboard::build_templates();
    let dashboard_routes = dashboard::router(templates, state.clone());
    let app = api::router(state).nest("/dashboard", dashboard_routes);

    let addr: SocketAddr = format!("{}:{}", config.host, config.port).parse()?;
    tracing::info!("postblox listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
