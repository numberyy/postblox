use std::net::SocketAddr;

use clap::Parser;
use postblox::cli::{self, Cli, Command};
use postblox::{api, config, dashboard, db, embeddings, events, hooks, mail, stalwart};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if let Some(cmd) = cli.command {
        return match cmd {
            Command::Init(args) => cli::init::run(*args)
                .await
                .map_err(|e| anyhow::anyhow!("{e}")),
            Command::Doctor(args) => cli::doctor::run(args)
                .await
                .map_err(|e| anyhow::anyhow!("{e}")),
        };
    }

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
            )?)
        }
        _ => {
            tracing::info!("stalwart not configured, email delivery disabled");
            None
        }
    };

    let webhook_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

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
                let dimension = config.embedding_dimension.unwrap_or(768);
                tracing::info!("embedding dimension: {dimension}");
                Some(std::sync::Arc::new(
                    embeddings::openai::OpenAiProvider::new(
                        url,
                        model,
                        config.embedding_api_key.clone(),
                        dimension,
                    )?,
                ))
            }
            None => {
                tracing::info!("no embedding provider configured, semantic search disabled");
                None
            }
        };

    let raw_hooks = config.hooks.unwrap_or_default();
    for h in &raw_hooks {
        if h.event != "before_send"
            && h.event != "before_receive"
            && !events::KNOWN_EVENTS.contains(&h.event.as_str())
        {
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
        attachment_storage_path: config.attachment_storage_path,
        max_attachment_size_bytes: config.max_attachment_size_bytes,
        content_filter: postblox::core::content_filter::ContentFilter::new(
            config.content_filter.allowed_types,
            config.content_filter.blocked_types,
        ),
        syntect: std::sync::Arc::new(api::SyntectResources {
            syntax_set: syntect::parsing::SyntaxSet::load_defaults_newlines(),
            theme_set: syntect::highlighting::ThemeSet::load_defaults(),
        }),
    };
    if config.dns_check_interval_secs > 0 {
        if let Some(ref stalwart) = state.stalwart {
            let pool = state.pool.clone();
            let stalwart = stalwart.clone();
            let interval_secs = config.dns_check_interval_secs;
            tokio::spawn(dns_check_loop(pool, stalwart, interval_secs));
            tracing::info!("DNS verification poller started (interval: {interval_secs}s)");
        }
    }

    let templates = dashboard::build_templates();
    let dashboard_routes = dashboard::router(templates, state.clone());
    let app = api::router(state)
        .nest("/dashboard", dashboard_routes)
        .layer(axum::extract::DefaultBodyLimit::max(10 * 1024 * 1024));

    let addr: SocketAddr = format!("{}:{}", config.host, config.port).parse()?;
    tracing::info!("postblox listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn dns_check_loop(
    pool: sqlx::PgPool,
    stalwart: postblox::stalwart::StalwartClient,
    interval_secs: u64,
) {
    use postblox::models::DomainStatus;
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(interval_secs)).await;
        let domains = match postblox::db::domains::list_pending(&pool).await {
            Ok(d) => d,
            Err(e) => {
                tracing::error!("DNS poller: failed to query pending domains: {e}");
                continue;
            }
        };
        if domains.is_empty() {
            continue;
        }
        tracing::info!("DNS poller: checking {} pending domain(s)", domains.len());
        for domain in domains {
            let dns = match stalwart.get_dns_records(&domain.name).await {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!("DNS poller: failed to check {}: {e}", domain.name);
                    continue;
                }
            };
            let records = dns["data"].as_array();
            let verified = records.is_some_and(|r| !r.is_empty());
            let result = if verified {
                postblox::db::domains::set_verified(&pool, domain.id).await
            } else {
                postblox::db::domains::update_status(&pool, domain.id, DomainStatus::Failed, None)
                    .await
            };
            match result {
                Ok(Some(d)) => {
                    tracing::info!("DNS poller: {} → {}", domain.name, d.status);
                }
                Ok(None) => {
                    tracing::warn!("DNS poller: {} disappeared during update", domain.name);
                }
                Err(e) => {
                    tracing::error!("DNS poller: failed to update {}: {e}", domain.name);
                }
            }
        }
    }
}

async fn shutdown_signal() {
    let ctrl_c = async { tokio::signal::ctrl_c().await.unwrap() };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => tracing::info!("received SIGINT, shutting down"),
        _ = terminate => tracing::info!("received SIGTERM, shutting down"),
    }
}
