#![deny(clippy::correctness)]
#![warn(clippy::suspicious, clippy::style, clippy::complexity, clippy::perf)]
#![warn(clippy::undocumented_unsafe_blocks)]
#![deny(unsafe_op_in_unsafe_fn)]

use std::path::PathBuf;

use anyhow::Context;

use postblox::ipc::default_socket_path;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let socket_path = std::env::var_os("POSTBLOX_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(default_socket_path);

    postblox::mcp::run_stdio(socket_path)
        .await
        .context("run MCP stdio bridge")
}
