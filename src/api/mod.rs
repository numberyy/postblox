use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Json;
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

pub mod api_keys;
pub mod approvals;
pub mod audit;
pub mod auth;
pub mod bounces;
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
pub mod members;
pub mod messages;
pub mod notifications;
pub mod organizations;
pub mod permissions;
pub mod rate_limit;
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
    pub embedding_provider: Option<std::sync::Arc<dyn crate::embeddings::EmbeddingProvider>>,
    pub embedding_semaphore: std::sync::Arc<tokio::sync::Semaphore>,
    pub trust_auto_upgrade_threshold: i32,
    pub hooks: std::sync::Arc<[crate::hooks::HookConfig]>,
    pub ws_hub: Arc<crate::events::websocket::WebSocketHub>,
    pub rate_limiter: Arc<rate_limit::RateLimiter>,
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

pub struct SendCheck<'a> {
    pub to: &'a [String],
    pub subject: Option<&'a str>,
    pub text_body: Option<&'a str>,
    pub html_body: Option<&'a str>,
    pub from_addr: &'a str,
}

pub async fn check_send_allowed(
    state: &AppState,
    inbox_id: Uuid,
    check: &SendCheck<'_>,
) -> Result<crate::models::SendMode, error::ApiError> {
    if let Err(violations) = crate::mail::guard::scan(
        check.subject,
        check.text_body,
        check.html_body,
        &state.guard_patterns,
    ) {
        let details: Vec<String> = violations
            .iter()
            .map(|v| format!("{} in {}", v.pattern_name, v.field))
            .collect();
        return Err(error::ApiError::BadRequest(format!(
            "message blocked: detected {}",
            details.join(", ")
        )));
    }

    let permission = crate::db::permissions::get_by_inbox(&state.pool, inbox_id)
        .await
        .map_err(|e| error::ApiError::Internal(e.to_string()))?;
    let send_mode = permission.as_ref().map(|p| p.mode()).unwrap_or_default();

    match send_mode {
        crate::models::SendMode::Shadow => {
            return Err(error::ApiError::Forbidden(
                "inbox is in shadow mode, sending disabled".into(),
            ));
        }
        crate::models::SendMode::Approval => {}
        crate::models::SendMode::AutoApprove => {
            if let Some(ref perm) = permission {
                let slop_score = {
                    let input = crate::core::slop::ClassifierInput {
                        from_addr: check.from_addr,
                        subject: check.subject,
                        text_body: check.text_body,
                        raw_headers: None,
                        sender_slop_ratio: None,
                    };
                    crate::core::slop::classify(&input).score as f64
                };
                if let crate::core::rules::RuleVerdict::Block { reason, .. } =
                    perm.rules().evaluate(
                        check.to,
                        check.subject.unwrap_or(""),
                        check.text_body.unwrap_or(""),
                        Some(slop_score),
                    )
                {
                    return Err(error::ApiError::Forbidden(format!(
                        "rule check failed: {reason}"
                    )));
                }
            }
        }
        crate::models::SendMode::Autonomous => {}
    }

    if let Err(e) = crate::hooks::run_before_send_hooks(
        &state.hooks,
        &serde_json::json!({
            "to": check.to,
            "subject": check.subject,
            "body": check.text_body,
            "inbox_id": inbox_id,
        }),
    )
    .await
    {
        return Err(error::ApiError::Forbidden(e.to_string()));
    }

    Ok(send_mode)
}

pub fn spawn_approval_event(
    state: &AppState,
    org_id: Uuid,
    inbox_id: Uuid,
    msg_id: Uuid,
    approval_id: Uuid,
) {
    let pool = state.pool.clone();
    let webhook_client = state.webhook_client.clone();
    let hooks = state.hooks.clone();
    let ws_hub = state.ws_hub.clone();
    tokio::spawn(async move {
        crate::events::dispatch(
            &pool,
            org_id,
            crate::events::PostbloxEvent::ApprovalRequested {
                message_id: msg_id,
                inbox_id,
                approval_id,
            },
            &webhook_client,
            &hooks,
            &ws_hub,
        )
        .await;
    });
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
            "/inboxes/{inbox_id}/messages/{id}/delivery-status",
            get(bounces::get_delivery_status),
        )
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
        .route("/members", get(members::list).post(members::add))
        .route(
            "/members/{api_key_id}",
            axum::routing::delete(members::remove),
        )
        .route("/inboxes/{inbox_id}/trust", get(trust::get))
        .route(
            "/notifications",
            get(notifications::list).post(notifications::create),
        )
        .route(
            "/notifications/{id}",
            axum::routing::delete(notifications::delete),
        )
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            rate_limit::middleware,
        ));

    axum::Router::new()
        .route("/health", get(health))
        .route("/internal/stalwart/inbound", post(inbound::receive_inbound))
        .route("/internal/stalwart/bounce", post(bounces::receive_bounce))
        .route("/api/v1/ws", get(ws_upgrade))
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

#[derive(Deserialize)]
struct WsParams {
    key: String,
}

async fn ws_upgrade(
    State(state): State<AppState>,
    Query(params): Query<WsParams>,
    ws: axum::extract::ws::WebSocketUpgrade,
) -> impl IntoResponse {
    let stored = match auth::validate_api_key(&state.pool, &params.key).await {
        Ok(k) => k,
        Err(()) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let hub = state.ws_hub.clone();
    let org_id = stored.org_id;
    ws.on_upgrade(move |socket| async move { hub.handle_ws(socket, org_id).await })
        .into_response()
}
