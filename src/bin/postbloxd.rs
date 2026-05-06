//! `postbloxd` — the local daemon binary.
//!
//! All op logic lives in [`postblox::daemon::DaemonDispatcher`]; this
//! binary just opens the DB, binds the socket, and waits for ctrl-c.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use tokio::signal;

use postblox::daemon::DaemonDispatcher;
use postblox::db;
use postblox::ipc::{default_socket_path, listen, Hub};

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let socket_path = std::env::var_os("POSTBLOX_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(default_socket_path);
    let db_path = std::env::var_os("POSTBLOX_DB")
        .map(PathBuf::from)
        .unwrap_or_else(default_db_path);

    tracing::info!(?db_path, ?socket_path, "starting postbloxd");

    let pool = db::connect(&db_path)
        .await
        .with_context(|| format!("connect to db at {}", db_path.display()))?;

    let hub = Arc::new(Hub::new());
    let dispatcher = Arc::new(DaemonDispatcher::new(pool, hub.clone()));
    let server = listen(&socket_path, dispatcher, hub).await?;
    tracing::info!(socket = %server.path().display(), "listening");

    signal::ctrl_c().await.context("install ctrl-c handler")?;
    tracing::info!("shutdown signal received");
    server.shutdown().await;
    Ok(())
}

fn default_db_path() -> PathBuf {
    if let Some(home) = dirs::data_local_dir() {
        home.join("postblox").join("postblox.db")
    } else {
        PathBuf::from("postblox.db")
    }
}
