use std::path::PathBuf;

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
    postblox::tui::run(socket_path).await?;
    Ok(())
}
