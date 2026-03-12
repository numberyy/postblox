mod client;
mod error;
mod tools;
mod transport;

#[tokio::main]
async fn main() {
    let base_url = std::env::var("POSTBLOX_URL").unwrap_or_else(|_| "http://localhost:3000".into());

    let api_key = match std::env::var("POSTBLOX_API_KEY") {
        Ok(key) => key,
        Err(_) => {
            eprintln!("error: POSTBLOX_API_KEY environment variable is required");
            std::process::exit(1);
        }
    };

    let client = client::PostbloxClient::new(base_url, api_key);

    if let Err(e) = transport::run(client).await {
        eprintln!("transport error: {e}");
        std::process::exit(1);
    }
}
