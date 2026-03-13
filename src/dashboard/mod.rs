use std::sync::Arc;

use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::{header, request::Parts, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Extension;
use minijinja::Environment;
use serde::Deserialize;
use uuid::Uuid;

use crate::api::AppState;

type Templates = Arc<Environment<'static>>;

pub fn build_templates() -> Environment<'static> {
    let mut env = Environment::new();
    env.add_template("base.html", include_str!("../../templates/base.html")).unwrap();
    env.add_template("inboxes.html", include_str!("../../templates/inboxes.html")).unwrap();
    env.add_template("inbox_detail.html", include_str!("../../templates/inbox_detail.html")).unwrap();
    env.add_template("messages_rows.html", include_str!("../../templates/messages_rows.html")).unwrap();
    env.add_template("message.html", include_str!("../../templates/message.html")).unwrap();
    env.add_template("thread.html", include_str!("../../templates/thread.html")).unwrap();
    env.add_template("approvals.html", include_str!("../../templates/approvals.html")).unwrap();
    env.add_template("briefing.html", include_str!("../../templates/briefing.html")).unwrap();
    env.add_template("search.html", include_str!("../../templates/search.html")).unwrap();
    env.add_template("search_results.html", include_str!("../../templates/search_results.html")).unwrap();
    env.add_template("unauthorized.html", include_str!("../../templates/unauthorized.html")).unwrap();
    env
}

pub fn router(templates: Environment<'static>, state: AppState) -> axum::Router {
    let tpl: Templates = Arc::new(templates);

    axum::Router::new()
        .route("/", get(index))
        .route("/inboxes", get(inboxes_list))
        .route("/inboxes/{id}", get(inbox_detail))
        .route("/inboxes/{id}/messages", get(inbox_messages_partial))
        .route("/messages/{id}", get(message_detail))
        .route("/threads/{id}", get(thread_view))
        .route("/approvals", get(approvals_list))
        .route("/approvals/{id}/approve", post(approval_approve))
        .route("/approvals/{id}/reject", post(approval_reject))
        .route("/briefing", get(briefing))
        .route("/search", get(search_page))
        .route("/search/results", get(search_results))
        .route("/static/style.css", get(static_css))
        .route("/static/htmx.min.js", get(static_htmx))
        .layer(Extension(tpl))
        .with_state(state)
}

/// Set if auth came from ?key= query param (not cookie). Handler should set cookie.
#[derive(Clone)]
struct SetCookieKey(Option<String>);

struct DashboardOrg(Uuid);

impl FromRequestParts<AppState> for DashboardOrg {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let from_cookie = extract_key_from_cookie(parts);
        let key = from_cookie
            .clone()
            .or_else(|| extract_key_from_query(parts))
            .ok_or_else(unauthorized)?;

        let stored = crate::api::auth::validate_api_key(&state.pool, &key)
            .await
            .map_err(|()| unauthorized())?;

        // If key came from query param, mark it so we set a cookie
        let needs_cookie = if from_cookie.is_none() {
            Some(key)
        } else {
            None
        };
        parts.extensions.insert(SetCookieKey(needs_cookie));

        Ok(DashboardOrg(stored.org_id))
    }
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Html(include_str!("../../templates/unauthorized.html")),
    )
        .into_response()
}

fn extract_key_from_cookie(parts: &Parts) -> Option<String> {
    let cookie_header = parts.headers.get(header::COOKIE)?.to_str().ok()?;
    for part in cookie_header.split(';') {
        let part = part.trim();
        if let Some(val) = part.strip_prefix("postblox_key=") {
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

fn extract_key_from_query(parts: &Parts) -> Option<String> {
    let query = parts.uri.query()?;
    for pair in query.split('&') {
        if let Some(val) = pair.strip_prefix("key=") {
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

fn maybe_set_cookie(cookie_key: Option<Extension<SetCookieKey>>, mut response: Response) -> Response {
    if let Some(Extension(SetCookieKey(Some(key)))) = cookie_key {
        let cookie = format!("postblox_key={key}; Path=/dashboard; HttpOnly; SameSite=Strict");
        if let Ok(val) = cookie.parse() {
            response.headers_mut().insert(header::SET_COOKIE, val);
        }
    }
    response
}

fn render(tpl: &Templates, name: &str, ctx: minijinja::Value) -> Response {
    match tpl.get_template(name) {
        Ok(template) => match template.render(ctx) {
            Ok(html) => Html(html).into_response(),
            Err(e) => {
                tracing::error!("template render error: {e}");
                error_response("template render failed")
            }
        },
        Err(e) => {
            tracing::error!("template not found: {e}");
            error_response("template not found")
        }
    }
}

// --- Handlers ---

async fn index() -> Redirect {
    Redirect::to("/dashboard/inboxes")
}

async fn inboxes_list(
    State(state): State<AppState>,
    Extension(tpl): Extension<Templates>,
    DashboardOrg(org_id): DashboardOrg,
    cookie_ext: Option<Extension<SetCookieKey>>,
) -> Response {
    let inboxes = match crate::db::inboxes::list_by_org(&state.pool, org_id).await {
        Ok(v) => v,
        Err(e) => return error_response(&e.to_string()),
    };

    let inbox_ids: Vec<uuid::Uuid> = inboxes.iter().map(|i| i.id).collect();
    let perms = log_err_default(
        "permissions",
        crate::db::permissions::get_by_inbox_ids(&state.pool, &inbox_ids).await,
    );
    let perm_map: std::collections::HashMap<uuid::Uuid, &crate::models::Permission> =
        perms.iter().map(|p| (p.inbox_id, p)).collect();

    let default_mode = crate::models::SendMode::default().to_string();
    let inbox_data: Vec<_> = inboxes
        .iter()
        .map(|inbox| {
            let send_mode = perm_map
                .get(&inbox.id)
                .map(|p| p.mode().to_string())
                .unwrap_or_else(|| default_mode.clone());
            minijinja::context! {
                id => inbox.id.to_string(),
                email => inbox.email,
                display_name => inbox.display_name,
                inbox_type => inbox.inbox_type,
                send_mode => send_mode,
                created_at => inbox.created_at.format("%Y-%m-%d %H:%M").to_string(),
            }
        })
        .collect();

    let resp = render(
        &tpl,
        "inboxes.html",
        minijinja::context! { inboxes => inbox_data },
    );
    maybe_set_cookie(cookie_ext, resp)
}


async fn inbox_detail(
    State(state): State<AppState>,
    Extension(tpl): Extension<Templates>,
    DashboardOrg(org_id): DashboardOrg,
    Path(id): Path<Uuid>,
) -> Response {
    let inbox = match crate::db::inboxes::get_by_id(&state.pool, id).await {
        Ok(Some(i)) if i.org_id == org_id => i,
        Ok(_) => return not_found(),
        Err(e) => return error_response(&e.to_string()),
    };

    let perm = crate::db::permissions::get_by_inbox(&state.pool, inbox.id)
        .await
        .ok()
        .flatten();
    let send_mode = perm
        .as_ref()
        .map(|p| p.mode().to_string())
        .unwrap_or_else(|| crate::models::SendMode::default().to_string());

    let labels = log_err_default(
        "labels",
        crate::db::labels::list_by_inbox(&state.pool, inbox.id).await,
    );
    let label_data: Vec<_> = labels
        .iter()
        .map(|l| {
            minijinja::context! {
                name => l.name,
                color => l.color,
            }
        })
        .collect();

    let limit: i64 = 25;
    let messages = log_err_default(
        "messages",
        crate::db::messages::list_by_inbox(&state.pool, inbox.id, limit, 0).await,
    );
    let has_more = messages.len() as i64 >= limit;
    let msg_data = messages_to_value(&messages);

    render(
        &tpl,
        "inbox_detail.html",
        minijinja::context! {
            inbox => minijinja::context! {
                id => inbox.id.to_string(),
                email => inbox.email,
                display_name => inbox.display_name,
                inbox_type => inbox.inbox_type,
            },
            send_mode => send_mode,
            labels => label_data,
            messages => msg_data,
            inbox_id => inbox.id.to_string(),
            offset => 0i64,
            limit => limit,
            has_more => has_more,
        },
    )
}

async fn inbox_messages_partial(
    State(state): State<AppState>,
    Extension(tpl): Extension<Templates>,
    DashboardOrg(org_id): DashboardOrg,
    Path(id): Path<Uuid>,
    Query(params): Query<crate::api::PaginationParams>,
) -> Response {
    let inbox = match crate::db::inboxes::get_by_id(&state.pool, id).await {
        Ok(Some(i)) if i.org_id == org_id => i,
        _ => return (StatusCode::NOT_FOUND, Html("not found".to_string())).into_response(),
    };

    let (limit, offset) = crate::api::clamp_pagination(&params);
    let messages = log_err_default(
        "messages partial",
        crate::db::messages::list_by_inbox(&state.pool, inbox.id, limit, offset).await,
    );
    let has_more = messages.len() as i64 >= limit;
    let msg_data = messages_to_value(&messages);

    render(
        &tpl,
        "messages_rows.html",
        minijinja::context! {
            messages => msg_data,
            inbox_id => inbox.id.to_string(),
            offset => offset,
            limit => limit,
            has_more => has_more,
        },
    )
}

async fn message_detail(
    State(state): State<AppState>,
    Extension(tpl): Extension<Templates>,
    DashboardOrg(org_id): DashboardOrg,
    Path(id): Path<Uuid>,
) -> Response {
    let message = match crate::db::messages::get_by_id(&state.pool, id).await {
        Ok(Some(m)) => m,
        Ok(None) => return not_found(),
        Err(e) => return error_response(&e.to_string()),
    };

    match crate::db::inboxes::get_by_id(&state.pool, message.inbox_id).await {
        Ok(Some(i)) if i.org_id == org_id => {}
        _ => return not_found(),
    }

    let labels = log_err_default(
        "message labels",
        crate::db::labels::list_for_message(&state.pool, message.id).await,
    );
    let label_data: Vec<_> = labels
        .iter()
        .map(|l| {
            minijinja::context! {
                name => l.name,
                color => l.color,
            }
        })
        .collect();

    let to_addrs = message
        .to_addrs
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();

    render(
        &tpl,
        "message.html",
        minijinja::context! {
            message => minijinja::context! {
                id => message.id.to_string(),
                from_addr => message.from_addr,
                subject => message.subject,
                text_body => message.text_body,
                direction => message.direction,
                created_at => message.created_at.format("%Y-%m-%d %H:%M").to_string(),
                slop_score => message.slop_score,
                thread_id => message.thread_id.map(|t| t.to_string()),
            },
            to_addrs => to_addrs,
            labels => label_data,
        },
    )
}

async fn thread_view(
    State(state): State<AppState>,
    Extension(tpl): Extension<Templates>,
    DashboardOrg(org_id): DashboardOrg,
    Path(id): Path<Uuid>,
) -> Response {
    let thread = match crate::db::threads::get_by_id(&state.pool, id).await {
        Ok(Some(t)) => t,
        Ok(None) => return not_found(),
        Err(e) => return error_response(&e.to_string()),
    };

    match crate::db::inboxes::get_by_id(&state.pool, thread.inbox_id).await {
        Ok(Some(i)) if i.org_id == org_id => {}
        _ => return not_found(),
    }

    let messages = log_err_default(
        "thread messages",
        crate::db::messages::list_by_thread(&state.pool, thread.id).await,
    );
    let msg_data = messages_to_value(&messages);

    render(
        &tpl,
        "thread.html",
        minijinja::context! {
            subject => thread.subject,
            messages => msg_data,
        },
    )
}

async fn approvals_list(
    State(state): State<AppState>,
    Extension(tpl): Extension<Templates>,
    DashboardOrg(org_id): DashboardOrg,
) -> Response {
    let pending = match crate::db::approvals::list_pending_with_details(&state.pool, org_id, 100)
        .await
    {
        Ok(v) => v,
        Err(e) => return error_response(&e.to_string()),
    };

    let items: Vec<_> = pending
        .iter()
        .map(|a| {
            minijinja::context! {
                approval => minijinja::context! {
                    id => a.id.to_string(),
                    created_at => a.created_at.format("%Y-%m-%d %H:%M").to_string(),
                },
                subject => a.subject,
                from_addr => a.from_addr,
                inbox_email => a.inbox_email,
            }
        })
        .collect();

    render(
        &tpl,
        "approvals.html",
        minijinja::context! { approvals => items },
    )
}

struct HtmxRequest;

impl FromRequestParts<AppState> for HtmxRequest {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if parts.headers.get("hx-request").and_then(|v| v.to_str().ok()) == Some("true") {
            Ok(HtmxRequest)
        } else {
            Err((StatusCode::FORBIDDEN, Html("CSRF check failed".to_string())).into_response())
        }
    }
}

async fn approval_approve(
    State(state): State<AppState>,
    DashboardOrg(org_id): DashboardOrg,
    HtmxRequest: HtmxRequest,
    Path(id): Path<Uuid>,
) -> Response {
    match crate::db::approvals::approve(&state.pool, org_id, id, "dashboard").await {
        Ok(Some(approval)) => {
            let state_clone = state.clone();
            let msg_id = approval.message_id;
            let inbox_id = approval.inbox_id;
            tokio::spawn(async move {
                let (msg_result, inbox_result) = tokio::join!(
                    crate::db::messages::get_by_id(&state_clone.pool, msg_id),
                    crate::db::inboxes::get_by_id(&state_clone.pool, inbox_id),
                );
                if let (Ok(Some(msg)), Ok(Some(inbox))) = (msg_result, inbox_result) {
                    let (to, cc) = crate::api::deliver::extract_addrs(&msg);
                    let _ = crate::api::deliver::deliver_message(
                        &state_clone, org_id, inbox_id, msg_id,
                        &crate::api::deliver::DeliveryParams {
                            from: &inbox.email,
                            to: &to,
                            cc: &cc,
                            subject: msg.subject.as_deref().unwrap_or(""),
                            text_body: msg.text_body.as_deref(),
                            html_body: msg.html_body.as_deref(),
                            message_id_header: msg.message_id_header.as_deref().unwrap_or("unknown@postblox"),
                        },
                    ).await;
                }
                crate::events::audit(
                    &state_clone.pool, org_id, Some(inbox_id),
                    crate::models::AuditAction::MessageApproved,
                    "dashboard",
                    serde_json::json!({"approval_id": id.to_string(), "message_id": msg_id.to_string()}),
                ).await;
                crate::api::approvals::record_trust_and_maybe_upgrade(
                    &state_clone, org_id, inbox_id, true,
                ).await;
            });
            approval_row(id, "var(--green)", true, "Approved")
        }
        Ok(None) => approval_row(id, "var(--muted)", false, "Already decided"),
        Err(e) => approval_row(id, "var(--red)", false, &format!("Error: {}", escape_html(&e.to_string()))),
    }
}

async fn approval_reject(
    State(state): State<AppState>,
    DashboardOrg(org_id): DashboardOrg,
    HtmxRequest: HtmxRequest,
    Path(id): Path<Uuid>,
) -> Response {
    match crate::db::approvals::reject(&state.pool, org_id, id, "dashboard").await {
        Ok(Some(approval)) => {
            let state_clone = state.clone();
            let inbox_id = approval.inbox_id;
            let msg_id = approval.message_id;
            tokio::spawn(async move {
                crate::events::audit(
                    &state_clone.pool, org_id, Some(inbox_id),
                    crate::models::AuditAction::MessageRejected,
                    "dashboard",
                    serde_json::json!({"approval_id": id.to_string(), "message_id": msg_id.to_string()}),
                ).await;
                crate::api::approvals::record_trust_and_maybe_upgrade(
                    &state_clone, org_id, inbox_id, false,
                ).await;
            });
            approval_row(id, "var(--red)", true, "Rejected")
        }
        Ok(None) => approval_row(id, "var(--muted)", false, "Already decided"),
        Err(e) => approval_row(id, "var(--red)", false, &format!("Error: {}", escape_html(&e.to_string()))),
    }
}

async fn briefing(
    State(state): State<AppState>,
    Extension(tpl): Extension<Templates>,
    DashboardOrg(org_id): DashboardOrg,
) -> Response {
    let since = chrono::Utc::now() - chrono::Duration::hours(24);

    let (by_inbox, top_senders, top_subjects, pending_count) = match tokio::try_join!(
        crate::db::briefing::stats_by_inbox(&state.pool, org_id, since),
        crate::db::briefing::top_senders(&state.pool, org_id, since),
        crate::db::briefing::top_subjects(&state.pool, org_id, since),
        crate::db::approvals::count_by_status(
            &state.pool,
            org_id,
            crate::models::ApprovalStatus::Pending,
        ),
    ) {
        Ok(r) => r,
        Err(e) => return error_response(&e.to_string()),
    };

    let (total_received, total_sent) = by_inbox
        .iter()
        .fold((0i64, 0i64), |(r, s), row| (r + row.received, s + row.sent));

    let inbox_data: Vec<_> = by_inbox
        .iter()
        .map(|row| {
            minijinja::context! {
                inbox_email => row.inbox_email,
                received => row.received,
                sent => row.sent,
            }
        })
        .collect();

    let sender_data: Vec<_> = top_senders
        .iter()
        .map(|s| {
            minijinja::context! {
                address => s.address,
                count => s.count,
            }
        })
        .collect();

    let subject_data: Vec<_> = top_subjects
        .iter()
        .map(|s| {
            minijinja::context! {
                subject => s.subject,
                count => s.count,
            }
        })
        .collect();

    render(
        &tpl,
        "briefing.html",
        minijinja::context! {
            period => "24h",
            total_received => total_received,
            total_sent => total_sent,
            pending_approvals => pending_count,
            by_inbox => inbox_data,
            top_senders => sender_data,
            top_subjects => subject_data,
        },
    )
}

async fn search_page(
    Extension(tpl): Extension<Templates>,
    DashboardOrg(_org_id): DashboardOrg,
) -> Response {
    render(&tpl, "search.html", minijinja::context! {})
}

#[derive(Deserialize)]
struct SearchQuery {
    q: Option<String>,
}

async fn search_results(
    State(state): State<AppState>,
    Extension(tpl): Extension<Templates>,
    DashboardOrg(org_id): DashboardOrg,
    Query(params): Query<SearchQuery>,
) -> Response {
    let q = params.q.unwrap_or_default();
    if q.trim().is_empty() {
        return render(&tpl, "search_results.html", minijinja::context! {});
    }

    let results: Vec<crate::models::SearchResultWithInbox> =
        match crate::db::messages::search_with_inbox(&state.pool, org_id, &q, 50).await {
            Ok(v) => v,
            Err(e) => return error_response(&e.to_string()),
        };

    let items: Vec<_> = results
        .iter()
        .map(|r| {
            minijinja::context! {
                id => r.id.to_string(),
                subject => r.subject,
                from_addr => r.from_addr,
                inbox_email => r.inbox_email,
                created_at => r.created_at.format("%Y-%m-%d %H:%M").to_string(),
            }
        })
        .collect();

    render(
        &tpl,
        "search_results.html",
        minijinja::context! {
            results => items,
            q => q,
        },
    )
}

// --- Static assets (embedded for Docker scratch) ---

async fn static_css() -> (
    StatusCode,
    [(header::HeaderName, &'static str); 2],
    &'static str,
) {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/css"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        include_str!("../../static/style.css"),
    )
}

async fn static_htmx() -> (
    StatusCode,
    [(header::HeaderName, &'static str); 2],
    &'static str,
) {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/javascript"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        include_str!("../../static/htmx.min.js"),
    )
}

// --- Helpers ---

fn messages_to_value(messages: &[crate::models::Message]) -> Vec<minijinja::Value> {
    messages
        .iter()
        .map(|m| {
            minijinja::context! {
                id => m.id.to_string(),
                from_addr => m.from_addr,
                subject => m.subject,
                text_body => m.text_body,
                direction => m.direction,
                slop_score => m.slop_score,
                created_at => m.created_at.format("%Y-%m-%d %H:%M").to_string(),
            }
        })
        .collect()
}

fn approval_row(id: Uuid, color: &str, bold: bool, msg: &str) -> Response {
    let weight = if bold { "font-weight:600;" } else { "" };
    Html(format!(
        "<tr id=\"approval-{id}\"><td colspan=\"5\" style=\"color:{color};{weight}\">{msg}</td></tr>"
    ))
    .into_response()
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Html("<h1>404 Not Found</h1>".to_string()),
    )
        .into_response()
}

fn log_err_default<T: Default>(context: &str, result: Result<T, sqlx::Error>) -> T {
    match result {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("dashboard {context}: {e}");
            T::default()
        }
    }
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn error_response(msg: &str) -> Response {
    tracing::error!("dashboard error: {msg}");
    let safe = escape_html(msg);
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Html(format!("<h1>Error</h1><p>{safe}</p>")),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;

    fn parts_with_cookie(val: &str) -> Parts {
        let (parts, _) = Request::builder()
            .header("cookie", val)
            .body(())
            .unwrap()
            .into_parts();
        parts
    }

    fn parts_with_uri(uri: &str) -> Parts {
        let (parts, _) = Request::builder().uri(uri).body(()).unwrap().into_parts();
        parts
    }

    fn empty_parts() -> Parts {
        let (parts, _) = Request::builder().body(()).unwrap().into_parts();
        parts
    }

    #[test]
    fn test_extract_key_from_cookie_found() {
        let parts = parts_with_cookie("other=val; postblox_key=pb_test1234; foo=bar");
        assert_eq!(extract_key_from_cookie(&parts), Some("pb_test1234".into()));
    }

    #[test]
    fn test_extract_key_from_cookie_not_found() {
        let parts = parts_with_cookie("other=val");
        assert_eq!(extract_key_from_cookie(&parts), None);
    }

    #[test]
    fn test_extract_key_from_cookie_empty_value() {
        let parts = parts_with_cookie("postblox_key=");
        assert_eq!(extract_key_from_cookie(&parts), None);
    }

    #[test]
    fn test_extract_key_from_cookie_no_header() {
        let parts = empty_parts();
        assert_eq!(extract_key_from_cookie(&parts), None);
    }

    #[test]
    fn test_extract_key_from_query_found() {
        let parts = parts_with_uri("/dashboard?key=pb_test1234&other=foo");
        assert_eq!(extract_key_from_query(&parts), Some("pb_test1234".into()));
    }

    #[test]
    fn test_extract_key_from_query_not_found() {
        let parts = parts_with_uri("/dashboard?other=foo");
        assert_eq!(extract_key_from_query(&parts), None);
    }

    #[test]
    fn test_extract_key_from_query_no_query() {
        let parts = parts_with_uri("/dashboard");
        assert_eq!(extract_key_from_query(&parts), None);
    }

    #[test]
    fn test_extract_key_from_query_empty() {
        let parts = parts_with_uri("/dashboard?key=");
        assert_eq!(extract_key_from_query(&parts), None);
    }

    #[test]
    fn test_build_templates_loads() {
        let env = build_templates();
        assert!(env.get_template("base.html").is_ok());
    }
}
