#![deny(clippy::correctness)]
#![warn(clippy::suspicious, clippy::style, clippy::complexity, clippy::perf)]
#![warn(clippy::undocumented_unsafe_blocks)]
#![deny(unsafe_op_in_unsafe_fn)]

use std::path::PathBuf;

use anyhow::Context;
use postblox::config;
use postblox::ipc::default_socket_path;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("postblox: {error}");
        std::process::exit(1);
    }
}

async fn run() -> anyhow::Result<()> {
    let socket_path = std::env::var_os("POSTBLOX_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(default_socket_path);
    let config_path = std::env::var_os("POSTBLOX_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(default_config_path);

    let cfg = config::load(&config_path)
        .with_context(|| format!("load config from {}", config_path.display()))?;

    postblox::tui::run_with_theme(socket_path, cfg.tui.theme).await?;
    Ok(())
}

fn default_config_path() -> PathBuf {
    if let Some(home) = dirs::config_dir() {
        home.join("postblox").join("postblox.toml")
    } else {
        PathBuf::from("postblox.toml")
    }
}
