use std::sync::Arc;

use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use uuid::Uuid;

use postblox::api;
use postblox::dashboard;
use postblox::events::websocket::WebSocketHub;
use postblox::models::{CreateMessage, InboxType};

pub async fn test_pool() -> PgPool {
    let url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for integration tests");
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("failed to connect to test database");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("failed to run migrations");
    pool
}

pub fn test_state(pool: PgPool) -> api::AppState {
    api::AppState {
        pool,
        stalwart: None,
        webhook_client: reqwest::Client::new(),
        inbound_token: None,
        guard_patterns: vec![],
        embedding_provider: None,
        embedding_semaphore: Arc::new(tokio::sync::Semaphore::new(20)),
        trust_auto_upgrade_threshold: 5,
        hooks: Arc::from(vec![]),
        ws_hub: Arc::new(WebSocketHub::new()),
        rate_limiter: Arc::new(api::rate_limit::RateLimiter::new(1000, 10000)),
        attachment_storage_path: std::env::temp_dir().join("postblox-test-attachments"),
        max_attachment_size_bytes: 25 * 1024 * 1024,
    }
}

pub fn test_app(state: api::AppState) -> axum::Router {
    let templates = dashboard::build_templates();
    let dashboard_routes = dashboard::router(templates, state.clone());
    api::router(state).nest("/dashboard", dashboard_routes)
}

pub async fn setup_org(pool: &PgPool) -> (Uuid, String) {
    let org = postblox::db::organizations::create(pool, "Test Org")
        .await
        .unwrap();
    let raw_key = format!("pb_{}", Uuid::new_v4().to_string().replace('-', ""));
    let hash = postblox::api::auth::hash_key(&raw_key);
    let prefix = &raw_key[..8];
    postblox::db::api_keys::create(pool, org.id, &hash, prefix, Some("test"))
        .await
        .unwrap();
    (org.id, raw_key)
}

pub async fn setup_inbox(pool: &PgPool, org_id: Uuid) -> postblox::models::Inbox {
    let email = format!("test-{}@test.example.com", Uuid::new_v4());
    postblox::db::inboxes::create(pool, org_id, &email, Some("Test Inbox"), InboxType::Native)
        .await
        .unwrap()
}

pub fn create_message_input(inbox_id: Uuid, subject: &str, body: &str) -> CreateMessage {
    CreateMessage {
        inbox_id,
        thread_id: None,
        message_id_header: Some(format!("<{}>", Uuid::new_v4())),
        in_reply_to: None,
        references_header: None,
        from_addr: "sender@example.com".into(),
        to_addrs: json!(["rcpt@example.com"]),
        cc_addrs: None,
        subject: Some(subject.into()),
        text_body: Some(body.into()),
        html_body: None,
        extracted_text: Some(body.into()),
        direction: postblox::models::Direction::Inbound,
        raw_headers: None,
    }
}
