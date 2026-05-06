//! `postbloxd` — the local daemon binary.
//!
//! All op logic lives in [`postblox::daemon::DaemonDispatcher`]; this
//! binary just opens the DB, binds the socket, and waits for ctrl-c.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use tokio::signal;

use postblox::config;
use postblox::daemon::DaemonDispatcher;
use postblox::db;
use postblox::imap;
use postblox::ipc::{default_socket_path, listen, Hub};
use postblox::secrets::{file::FileSecretStore, SecretStore, UnconfiguredSecretStore};
use postblox::sync::WorkerManager;

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
    let config_path = std::env::var_os("POSTBLOX_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(default_config_path);

    tracing::info!(?db_path, ?socket_path, ?config_path, "starting postbloxd");

    let cfg = config::load(&config_path)
        .with_context(|| format!("load config from {}", config_path.display()))?;

    let pool = db::connect(&db_path)
        .await
        .with_context(|| format!("connect to db at {}", db_path.display()))?;

    let secrets = build_secret_store(&cfg, &db_path);

    let hub = Arc::new(Hub::new());
    let imap_auth = imap::default_auth().context("initialize IMAP auth")?;
    let imap_sync = imap::default_sync().context("initialize IMAP sync")?;
    let imap_idle = imap::default_idle().context("initialize IMAP IDLE")?;
    let manager = Arc::new(WorkerManager::with_idle_config(
        pool.clone(),
        hub.clone(),
        imap_sync.clone(),
        Some(imap_idle),
        postblox::sync::WorkerConfig::default(),
    ));
    let dispatcher = Arc::new(DaemonDispatcher::with_imap_and_manager(
        pool,
        hub.clone(),
        imap_auth,
        imap_sync,
        secrets,
        manager.clone(),
    ));
    let server = listen(&socket_path, dispatcher, hub).await?;
    tracing::info!(socket = %server.path().display(), "listening");

    signal::ctrl_c().await.context("install ctrl-c handler")?;
    tracing::info!("shutdown signal received");
    manager.stop_all().await;
    server.shutdown().await;
    Ok(())
}

fn build_secret_store(cfg: &config::Config, db_path: &std::path::Path) -> Arc<dyn SecretStore> {
    match cfg.secrets.as_ref() {
        Some(s) => {
            let path = s
                .path
                .clone()
                .unwrap_or_else(|| default_secrets_path(db_path));
            tracing::info!(secrets_path = %path.display(), "secrets backend: file (aes-gcm)");
            Arc::new(FileSecretStore::new(path, s.passphrase.clone()))
        }
        None => {
            tracing::warn!(
                "no [secrets] section in config — account.set_secret will refuse to run"
            );
            Arc::new(UnconfiguredSecretStore)
        }
    }
}

fn default_db_path() -> PathBuf {
    if let Some(home) = dirs::data_local_dir() {
        home.join("postblox").join("postblox.db")
    } else {
        PathBuf::from("postblox.db")
    }
}

fn default_config_path() -> PathBuf {
    if let Some(home) = dirs::config_dir() {
        home.join("postblox").join("postblox.toml")
    } else {
        PathBuf::from("postblox.toml")
    }
}

fn default_secrets_path(db_path: &std::path::Path) -> PathBuf {
    db_path
        .parent()
        .map(|p| p.join("secrets.bin"))
        .unwrap_or_else(|| PathBuf::from("secrets.bin"))
}
