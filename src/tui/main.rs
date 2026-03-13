mod app;
mod client;
mod components;
mod config;
mod keys;
mod layout;
mod state;
mod theme;
mod ws;

#[tokio::main]
async fn main() {
    let cfg = match config::TuiConfig::load() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };

    let client = match client::PostbloxClient::new(cfg.server_url.clone(), cfg.api_key.clone()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };

    let mut app = app::App::new(&cfg, client);
    if let Err(e) = app.run().await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
