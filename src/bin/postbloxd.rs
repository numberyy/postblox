//! `postbloxd` — the local daemon binary.
//!
//! All op logic lives in [`postblox::daemon::DaemonDispatcher`]; this
//! binary just opens the DB, binds the socket, and waits for ctrl-c.

#![deny(clippy::correctness)]
#![warn(clippy::suspicious, clippy::style, clippy::complexity, clippy::perf)]
#![warn(clippy::undocumented_unsafe_blocks)]
#![warn(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use tokio::signal;
#[cfg(unix)]
use tokio::signal::unix::{signal as unix_signal, SignalKind};

use postblox::config;
use postblox::daemon::{worker_manager_with_idle_config, DaemonDispatcher, DaemonServices};
use postblox::db;
use postblox::imap;
use postblox::ipc::{default_socket_path, listen, Hub};
use postblox::secrets::{file::FileSecretStore, keyring::KeyringSecretStore, SecretStore};

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
    let read_pool = db::connect_readonly(&db_path)
        .await
        .with_context(|| format!("connect read-only to db at {}", db_path.display()))?;

    let secrets = build_secret_store(&cfg, &db_path)?;

    let hub = Arc::new(Hub::new());
    let imap_auth = imap::default_auth().context("initialize IMAP auth")?;
    let imap_sync = imap::default_sync().context("initialize IMAP sync")?;
    let imap_idle = imap::default_idle().context("initialize IMAP IDLE")?;
    let services = DaemonServices::default();
    let manager = worker_manager_with_idle_config(
        &pool,
        &hub,
        imap_sync.clone(),
        Some(imap_idle),
        &secrets,
        &services,
        postblox::sync::WorkerConfig::default(),
    );
    let dispatcher = Arc::new(DaemonDispatcher::with_imap_smtp_oauth_and_manager(
        pool,
        read_pool,
        hub.clone(),
        imap_auth,
        imap_sync,
        secrets,
        services,
        manager.clone(),
    ));
    let server = listen(&socket_path, dispatcher, hub).await?;
    tracing::info!(socket = %server.path().display(), "listening");

    wait_for_shutdown_signal().await?;
    tracing::info!("shutdown signal received");
    manager.stop_all().await;
    server.shutdown().await;
    Ok(())
}

/// Wait for either SIGINT (Ctrl-C) or SIGTERM (e.g. `systemctl stop`,
/// container shutdown). Both signals trigger the same graceful exit so
/// the socket file is always cleaned up. Windows has no SIGTERM, so we
/// fall back to ctrl_c there.
async fn wait_for_shutdown_signal() -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        let mut term = unix_signal(SignalKind::terminate()).context("install SIGTERM handler")?;
        tokio::select! {
            res = signal::ctrl_c() => res.context("install SIGINT handler"),
            _ = term.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    {
        signal::ctrl_c().await.context("install ctrl-c handler")
    }
}

fn build_secret_store(
    cfg: &config::Config,
    db_path: &std::path::Path,
) -> anyhow::Result<Arc<dyn SecretStore>> {
    match cfg.secrets.backend {
        config::SecretsBackend::Keyring => {
            tracing::info!("secrets backend: OS keyring");
            Ok(Arc::new(KeyringSecretStore::default()))
        }
        config::SecretsBackend::File => {
            let passphrase = cfg
                .secrets
                .passphrase
                .clone()
                .context("file secrets backend requires [secrets] passphrase")?;
            let path = cfg
                .secrets
                .path
                .clone()
                .unwrap_or_else(|| default_secrets_path(db_path));
            tracing::info!(secrets_path = %path.display(), "secrets backend: file (aes-gcm)");
            Ok(Arc::new(FileSecretStore::new(path, passphrase)))
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
