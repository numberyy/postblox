mod client;
mod error;
mod sse;
mod tools;
mod transport;

use clap::Parser;

#[derive(Parser)]
#[command(name = "postblox-mcp", about = "MCP server for postblox")]
struct Args {
    /// Transport mode
    #[arg(long, default_value = "stdio")]
    transport: String,

    /// Port for SSE transport
    #[arg(long, default_value_t = 3001)]
    port: u16,

    /// Bind address for SSE transport
    #[arg(long, default_value = "127.0.0.1")]
    bind: String,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let base_url = std::env::var("POSTBLOX_URL").unwrap_or_else(|_| "http://localhost:3000".into());

    let api_key = match std::env::var("POSTBLOX_API_KEY") {
        Ok(key) => key,
        Err(_) => {
            eprintln!("error: POSTBLOX_API_KEY environment variable is required");
            std::process::exit(1);
        }
    };

    let client = match client::PostbloxClient::new(base_url, api_key) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("failed to create client: {e}");
            std::process::exit(1);
        }
    };

    match args.transport.as_str() {
        "stdio" => {
            if let Err(e) = transport::run(client).await {
                eprintln!("transport error: {e}");
                std::process::exit(1);
            }
        }
        "sse" => {
            if let Err(e) = sse::run_sse(client, &args.bind, args.port).await {
                eprintln!("SSE server error: {e}");
                std::process::exit(1);
            }
        }
        other => {
            eprintln!("unknown transport: {other} (expected 'stdio' or 'sse')");
            std::process::exit(1);
        }
    }
}
