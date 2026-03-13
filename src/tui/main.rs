mod app;
#[allow(dead_code)] // Wired in Round 3 when data layer integrates
mod client;
mod components;
#[allow(dead_code)] // Wired in Round 3 when data layer integrates
mod config;
#[allow(dead_code)] // Wired in Round 3 when data layer integrates
mod event;
mod keys;
mod layout;
#[allow(dead_code)] // Wired in Round 3 when data layer integrates
mod state;
mod theme;
#[allow(dead_code)] // Wired in Round 3 when WS connects
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

    let mut app = app::App::new(&cfg);
    if let Err(e) = app.run().await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
