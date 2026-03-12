use axum::extract::State;
use axum::routing::{get, post};
use axum::Json;
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

pub mod api_keys;
pub mod approvals;
pub mod audit;
pub mod auth;
pub mod briefing;
pub mod deliver;
pub mod domains;
pub mod drafts;
pub mod error;
pub mod feedback;
pub mod inbound;
pub mod inboxes;
pub mod labels;
pub mod linked_accounts;
pub mod messages;
pub mod notifications;
pub mod organizations;
pub mod permissions;
pub mod search;
pub mod threads;
pub mod trust;
pub mod webhooks;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub stalwart: Option<crate::stalwart::StalwartClient>,
    pub webhook_client: reqwest::Client,
    pub inbound_token: Option<String>,
    pub guard_patterns: Vec<crate::mail::guard::GuardPattern>,
    #[allow(dead_code)] // used once relay sending is wired up
    pub relay: Option<crate::config::RelayConfig>,
    pub embedding_provider: Option<std::sync::Arc<dyn crate::embeddings::EmbeddingProvider>>,
    pub embedding_semaphore: std::sync::Arc<tokio::sync::Semaphore>,
    pub trust_auto_upgrade_threshold: i32,
}

#[derive(Deserialize)]
pub struct PaginationParams {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

pub fn clamp_pagination(params: &PaginationParams) -> (i64, i64) {
    let limit = params.limit.unwrap_or(50).clamp(1, 100);
    let offset = params.offset.unwrap_or(0).max(0);
    (limit, offset)
}

pub async fn get_inbox_for_org(
    pool: &PgPool,
    inbox_id: Uuid,
    org_id: Uuid,
) -> Result<crate::models::Inbox, error::ApiError> {
    let inbox = crate::db::inboxes::get_by_id(pool, inbox_id)
        .await
        .map_err(|e| error::ApiError::Internal(e.to_string()))?
        .ok_or(error::ApiError::NotFound)?;
    if inbox.org_id != org_id {
        return Err(error::ApiError::NotFound);
    }
    Ok(inbox)
}

pub fn router(state: AppState) -> axum::Router {
    let api_routes = axum::Router::new()
        .route("/inboxes", get(inboxes::list).post(inboxes::create))
        .route("/inboxes/{id}", get(inboxes::get).delete(inboxes::delete))
        .route("/inboxes/{inbox_id}/threads", get(threads::list))
        .route("/inboxes/{inbox_id}/threads/{id}", get(threads::get))
        .route("/webhooks", get(webhooks::list).post(webhooks::create))
        .route(
            "/webhooks/{id}",
            get(webhooks::get).delete(webhooks::delete),
        )
        .route(
            "/inboxes/{inbox_id}/messages",
            get(messages::list).post(messages::send),
        )
        .route("/inboxes/{inbox_id}/messages/{id}", get(messages::get))
        .route(
            "/inboxes/{inbox_id}/labels",
            get(labels::list).post(labels::create),
        )
        .route(
            "/inboxes/{inbox_id}/labels/{id}",
            axum::routing::delete(labels::delete),
        )
        .route(
            "/inboxes/{inbox_id}/messages/{message_id}/labels",
            get(labels::list_for_message).post(labels::add_to_message),
        )
        .route(
            "/inboxes/{inbox_id}/messages/{message_id}/labels/{label_id}",
            axum::routing::delete(labels::remove_from_message),
        )
        .route(
            "/inboxes/{inbox_id}/drafts",
            get(drafts::list).post(drafts::create),
        )
        .route(
            "/inboxes/{inbox_id}/drafts/{id}",
            get(drafts::get).put(drafts::update).delete(drafts::delete),
        )
        .route(
            "/inboxes/{inbox_id}/drafts/{id}/send",
            post(drafts::send_draft),
        )
        .route("/organizations", post(organizations::bootstrap))
        .route("/api-keys", get(api_keys::list).post(api_keys::create))
        .route("/api-keys/{id}", axum::routing::delete(api_keys::delete))
        .route("/briefing", get(briefing::get))
        .route("/search", get(search::search))
        .route("/feedback", post(feedback::submit))
        .route("/domains", get(domains::list).post(domains::create))
        .route("/domains/{id}", get(domains::get).delete(domains::delete))
        .route("/domains/{id}/verify", post(domains::verify))
        .route(
            "/linked-accounts",
            get(linked_accounts::list).post(linked_accounts::create),
        )
        .route(
            "/linked-accounts/{id}",
            get(linked_accounts::get).delete(linked_accounts::delete),
        )
        .route("/linked-accounts/{id}/sync", post(linked_accounts::sync))
        .route(
            "/inboxes/{inbox_id}/permissions",
            get(permissions::get).put(permissions::upsert),
        )
        .route("/audit", get(audit::list))
        .route("/approvals", get(approvals::list))
        .route("/approvals/{id}", get(approvals::get))
        .route("/approvals/{id}/approve", post(approvals::approve))
        .route("/approvals/{id}/reject", post(approvals::reject))
        .route("/approvals/batch", post(approvals::batch))
        .route("/inboxes/{inbox_id}/trust", get(trust::get))
        .route(
            "/notifications",
            get(notifications::list).post(notifications::create),
        )
        .route(
            "/notifications/{id}",
            axum::routing::delete(notifications::delete),
        );

    axum::Router::new()
        .route("/health", get(health))
        .route("/internal/stalwart/inbound", post(inbound::receive_inbound))
        .nest("/api/v1", api_routes)
        .with_state(state)
}

async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    let db_ok = sqlx::query("SELECT 1").execute(&state.pool).await.is_ok();

    Json(serde_json::json!({
        "status": if db_ok { "ok" } else { "degraded" },
        "database": db_ok,
    }))
}
