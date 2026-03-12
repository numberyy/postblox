use std::net::SocketAddr;

use axum::{extract::State, routing::get, Json, Router};
use sqlx::PgPool;
use tracing_subscriber::EnvFilter;

mod config;
mod db;
mod mail;
mod models;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let config = config::Config::load()?;
    let pool = db::connect(&config.database_url).await?;

    let app = Router::new().route("/health", get(health)).with_state(pool);

    let addr: SocketAddr = format!("{}:{}", config.host, config.port).parse()?;
    tracing::info!("postblox listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn health(State(pool): State<PgPool>) -> Json<serde_json::Value> {
    let db_ok = sqlx::query("SELECT 1").execute(&pool).await.is_ok();

    Json(serde_json::json!({
        "status": if db_ok { "ok" } else { "degraded" },
        "database": db_ok,
    }))
}
