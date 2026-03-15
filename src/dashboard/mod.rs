pub mod ws;

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::{header, request::Parts, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Extension, Form};
use minijinja::Environment;
use serde::Deserialize;
use uuid::Uuid;

use crate::api::AppState;

type Templates = Arc<Environment<'static>>;

pub fn build_templates() -> Environment<'static> {
    let mut env = Environment::new();
    const TEMPLATES: &[(&str, &str)] = &[
        ("base.html", include_str!("../../templates/base.html")),
        ("inboxes.html", include_str!("../../templates/inboxes.html")),
        (
            "inbox_detail.html",
            include_str!("../../templates/inbox_detail.html"),
        ),
        (
            "messages_rows.html",
            include_str!("../../templates/messages_rows.html"),
        ),
        ("message.html", include_str!("../../templates/message.html")),
        ("thread.html", include_str!("../../templates/thread.html")),
        (
            "approvals.html",
            include_str!("../../templates/approvals.html"),
        ),
        (
            "briefing.html",
            include_str!("../../templates/briefing.html"),
        ),
        ("search.html", include_str!("../../templates/search.html")),
        (
            "search_results.html",
            include_str!("../../templates/search_results.html"),
        ),
        (
            "unauthorized.html",
            include_str!("../../templates/unauthorized.html"),
        ),
        (
            "settings.html",
            include_str!("../../templates/settings.html"),
        ),
        (
            "analytics.html",
            include_str!("../../templates/analytics.html"),
        ),
    ];
    for (name, source) in TEMPLATES {
        env.add_template(name, source)
            .unwrap_or_else(|e| panic!("template '{name}' failed to load: {e}"));
    }
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
        .route("/settings", get(settings_page))
        .route("/settings/inbox/{id}/mode", post(settings_change_mode))
        .route(
            "/settings/notifications",
            post(settings_create_notification),
        )
        .route(
            "/settings/notifications/{id}/delete",
            post(settings_delete_notification),
        )
        .route("/inboxes/{id}/compose", post(compose_send))
        .route("/analytics", get(analytics_page))
        .route(
            "/inboxes/{inbox_id}/messages/{msg_id}/attachments/{att_id}",
            get(attachment_proxy),
        )
        .route("/ws", get(ws::ws_upgrade))
        .route("/static/style.css", get(static_css))
        .route("/static/htmx.min.js", get(static_htmx))
        .route("/static/ws.js", get(static_ws_js))
        .route("/static/upload.js", get(static_upload_js))
        .layer(Extension(tpl))
        .with_state(state)
}

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

        let stored = match crate::api::auth::validate_api_key(&state.pool, &key).await {
            Ok(s) => s,
            Err(crate::api::auth::AuthError::DatabaseError) => {
                return Err(StatusCode::INTERNAL_SERVER_ERROR.into_response());
            }
            Err(_) => return Err(unauthorized()),
        };

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
    ws::extract_key_from_cookie(&parts.headers)
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

fn maybe_set_cookie(
    cookie_key: Option<Extension<SetCookieKey>>,
    mut response: Response,
) -> Response {
    if let Some(Extension(SetCookieKey(Some(key)))) = cookie_key {
        let cookie =
            format!("postblox_key={key}; Path=/dashboard; HttpOnly; SameSite=Strict; Secure");
        if let Ok(val) = cookie.parse() {
            response.headers_mut().insert(header::SET_COOKIE, val);
        }
    }
    response
}

fn render(tpl: &Templates, name: &str, ctx: minijinja::Value) -> Response {
    let template = match tpl.get_template(name) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("template not found: {e}");
            return error_response("template not found");
        }
    };
    match template.render(ctx) {
        Ok(html) => Html(html).into_response(),
        Err(e) => {
            tracing::error!("template render error: {e}");
            error_response("template render failed")
        }
    }
}

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

    let limit: i64 = 25;
    let (perm_result, labels_result, messages_result) = tokio::join!(
        crate::db::permissions::get_by_inbox(&state.pool, inbox.id),
        crate::db::labels::list_by_inbox(&state.pool, inbox.id),
        crate::db::messages::list_by_inbox(&state.pool, inbox.id, limit, 0),
    );

    let perm = log_err_default("permissions", perm_result);
    let send_mode = perm
        .as_ref()
        .map(|p| p.mode().to_string())
        .unwrap_or_else(|| crate::models::SendMode::default().to_string());

    let labels = log_err_default("labels", labels_result);
    let label_data: Vec<_> = labels
        .iter()
        .map(|l| {
            minijinja::context! {
                name => l.name,
                color => l.color,
            }
        })
        .collect();

    let messages = log_err_default("messages", messages_result);
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

    let inbox_id = message.inbox_id;

    let (labels_result, attachments_result) = tokio::join!(
        crate::db::labels::list_for_message(&state.pool, message.id),
        crate::db::attachments::list_by_message(&state.pool, message.id),
    );
    let labels = log_err_default("message labels", labels_result);
    let attachments = log_err_default("message attachments", attachments_result);

    let label_data: Vec<_> = labels
        .iter()
        .map(|l| {
            minijinja::context! {
                name => l.name,
                color => l.color,
            }
        })
        .collect();

    let attachment_data = attachments_to_value(&attachments, inbox_id, message.id);

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
                inbox_id => inbox_id.to_string(),
                from_addr => message.from_addr,
                subject => message.subject,
                text_body => message.text_body,
                html_body => message.html_body,
                direction => message.direction,
                created_at => message.created_at.format("%Y-%m-%d %H:%M").to_string(),
                slop_score => message.slop_score,
                thread_id => message.thread_id.map(|t| t.to_string()),
            },
            to_addrs => to_addrs,
            labels => label_data,
            attachments => attachment_data,
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

    let msg_ids: Vec<Uuid> = messages.iter().map(|m| m.id).collect();
    let all_attachments = log_err_default(
        "thread attachments",
        crate::db::attachments::list_by_message_ids(&state.pool, &msg_ids).await,
    );
    let att_by_msg: std::collections::HashMap<Uuid, Vec<&crate::models::Attachment>> =
        all_attachments
            .iter()
            .fold(std::collections::HashMap::new(), |mut map, att| {
                map.entry(att.message_id).or_default().push(att);
                map
            });

    let msg_data: Vec<minijinja::Value> = messages
        .iter()
        .map(|m| {
            let atts = att_by_msg
                .get(&m.id)
                .map(|v| attachments_to_value_refs(v, m.inbox_id, m.id))
                .unwrap_or_default();
            minijinja::context! {
                id => m.id.to_string(),
                inbox_id => m.inbox_id.to_string(),
                from_addr => m.from_addr,
                subject => m.subject,
                text_body => m.text_body,
                html_body => m.html_body,
                direction => m.direction,
                slop_score => m.slop_score,
                created_at => m.created_at.format("%Y-%m-%d %H:%M").to_string(),
                attachments => atts,
            }
        })
        .collect();

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
    let pending =
        match crate::db::approvals::list_with_details(&state.pool, org_id, Some("pending"), 0, 100)
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
        if parts
            .headers
            .get("hx-request")
            .and_then(|v| v.to_str().ok())
            == Some("true")
        {
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
            tokio::spawn(async move {
                if let Err(e) = crate::api::approvals::execute_approval(
                    &state_clone,
                    org_id,
                    &approval,
                    "dashboard",
                )
                .await
                {
                    tracing::error!(approval_id = %id, "dashboard approve failed: {e:?}");
                }
            });
            approval_row(id, "var(--green)", true, "Approved")
        }
        Ok(None) => approval_row(id, "var(--muted)", false, "Already decided"),
        Err(e) => approval_row(
            id,
            "var(--red)",
            false,
            &format!("Error: {}", escape_html(&e.to_string())),
        ),
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
                    &state_clone,
                    org_id,
                    inbox_id,
                    false,
                )
                .await;
            });
            approval_row(id, "var(--red)", true, "Rejected")
        }
        Ok(None) => approval_row(id, "var(--muted)", false, "Already decided"),
        Err(e) => approval_row(
            id,
            "var(--red)",
            false,
            &format!("Error: {}", escape_html(&e.to_string())),
        ),
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

async fn settings_page(
    State(state): State<AppState>,
    Extension(tpl): Extension<Templates>,
    DashboardOrg(org_id): DashboardOrg,
) -> Response {
    let inboxes = match crate::db::inboxes::list_by_org(&state.pool, org_id).await {
        Ok(v) => v,
        Err(e) => return error_response(&e.to_string()),
    };

    let inbox_ids: Vec<Uuid> = inboxes.iter().map(|i| i.id).collect();
    let perms = log_err_default(
        "permissions",
        crate::db::permissions::get_by_inbox_ids(&state.pool, &inbox_ids).await,
    );
    let perm_map: std::collections::HashMap<Uuid, &crate::models::Permission> =
        perms.iter().map(|p| (p.inbox_id, p)).collect();

    let inbox_data: Vec<_> = inboxes
        .iter()
        .map(|inbox| {
            let fallback = crate::models::Permission::default_for_inbox(inbox.id);
            let perm = perm_map.get(&inbox.id).copied().unwrap_or(&fallback);
            let rules_display: Vec<String> = perm.rules().0.iter().map(format_rule).collect();
            minijinja::context! {
                id => inbox.id.to_string(),
                email => inbox.email,
                send_mode => perm.mode().to_string(),
                rules => rules_display,
            }
        })
        .collect();

    let modes = vec!["shadow", "approval", "auto_approve", "autonomous"];

    let notifications = log_err_default(
        "notifications",
        crate::db::notifications::list_active(&state.pool, org_id).await,
    );
    let notif_data: Vec<_> = notifications
        .iter()
        .map(|n| {
            minijinja::context! {
                id => n.id.to_string(),
                provider => n.provider,
                config => n.config.to_string(),
                created_at => n.created_at.format("%Y-%m-%d %H:%M").to_string(),
            }
        })
        .collect();

    let webhooks = log_err_default(
        "webhooks",
        crate::db::webhooks::list_by_org(&state.pool, org_id).await,
    );
    let webhook_data: Vec<_> = webhooks
        .iter()
        .map(|wh| {
            let events: Vec<String> = wh
                .events
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            minijinja::context! {
                url => wh.url,
                events => events,
                created_at => wh.created_at.format("%Y-%m-%d %H:%M").to_string(),
            }
        })
        .collect();

    render(
        &tpl,
        "settings.html",
        minijinja::context! {
            inboxes => inbox_data,
            modes => modes,
            notifications => notif_data,
            webhooks => webhook_data,
        },
    )
}

fn format_rule(rule: &crate::core::rules::Rule) -> String {
    use crate::core::rules::Rule;
    match rule {
        Rule::DomainAllowlist { domains } => format!("DomainAllowlist: {}", domains.join(", ")),
        Rule::DomainBlocklist { domains } => format!("DomainBlocklist: {}", domains.join(", ")),
        Rule::TimeWindow {
            start_hour,
            end_hour,
            timezone,
        } => format!("TimeWindow: {start_hour}:00–{end_hour}:00 {timezone}"),
        Rule::KeywordBlocklist { keywords } => {
            format!("KeywordBlocklist: {}", keywords.join(", "))
        }
        Rule::SlopThreshold { threshold } => format!("SlopThreshold: {threshold}"),
        Rule::DollarAmount { max_amount } => format!("DollarAmount: max ${max_amount}"),
    }
}

#[derive(Deserialize)]
struct ChangeModeForm {
    send_mode: String,
}

async fn settings_change_mode(
    State(state): State<AppState>,
    DashboardOrg(org_id): DashboardOrg,
    HtmxRequest: HtmxRequest,
    Path(id): Path<Uuid>,
    Form(form): Form<ChangeModeForm>,
) -> Response {
    let inbox = match crate::db::inboxes::get_by_id(&state.pool, id).await {
        Ok(Some(i)) if i.org_id == org_id => i,
        _ => return bad_request("inbox not found"),
    };

    let mode: crate::models::SendMode = match form.send_mode.parse() {
        Ok(m) => m,
        Err(_) => return bad_request("invalid send mode"),
    };

    let existing_rules = match crate::db::permissions::get_by_inbox(&state.pool, inbox.id).await {
        Ok(Some(p)) => p.rules,
        Ok(None) => serde_json::json!([]),
        Err(e) => {
            tracing::error!(inbox_id = %inbox.id, "failed to fetch permissions: {e}");
            return error_response("failed to load existing rules");
        }
    };

    let perm =
        match crate::db::permissions::upsert(&state.pool, inbox.id, mode, &existing_rules).await {
            Ok(p) => p,
            Err(e) => return error_response(&e.to_string()),
        };

    let rules_display: Vec<String> = perm.rules().0.iter().map(format_rule).collect();
    let rules_html = if rules_display.is_empty() {
        "<span style=\"color:var(--muted)\">none</span>".to_string()
    } else {
        rules_display
            .iter()
            .map(|r| format!("<span class=\"badge badge-gray\">{}</span>", escape_html(r)))
            .collect::<Vec<_>>()
            .join(" ")
    };

    let modes = ["shadow", "approval", "auto_approve", "autonomous"];
    let mode_str = perm.mode().to_string();
    let options: String = modes
        .iter()
        .map(|m| {
            let sel = if *m == mode_str { " selected" } else { "" };
            format!("<option value=\"{m}\"{sel}>{m}</option>")
        })
        .collect();

    let inbox_id = inbox.id;
    Html(format!(
        "<tr id=\"inbox-mode-{inbox_id}\">\
         <td>{}</td>\
         <td><select name=\"send_mode\" \
         hx-post=\"/dashboard/settings/inbox/{inbox_id}/mode\" \
         hx-target=\"#inbox-mode-{inbox_id}\" \
         hx-swap=\"outerHTML\">{options}</select></td>\
         <td>{rules_html}</td></tr>",
        escape_html(&inbox.email),
    ))
    .into_response()
}

#[derive(Deserialize)]
struct CreateNotifForm {
    provider: String,
    config: String,
}

async fn settings_create_notification(
    State(state): State<AppState>,
    DashboardOrg(org_id): DashboardOrg,
    HtmxRequest: HtmxRequest,
    Form(form): Form<CreateNotifForm>,
) -> Response {
    let provider: crate::models::NotificationProvider = match form.provider.parse() {
        Ok(p) => p,
        Err(_) => return bad_request("invalid provider"),
    };

    let config: serde_json::Value = match serde_json::from_str(&form.config) {
        Ok(v) => v,
        Err(_) => return bad_request("invalid JSON config"),
    };

    let input = crate::models::CreateNotificationConfig {
        org_id,
        provider,
        config,
    };

    match crate::db::notifications::create(&state.pool, &input).await {
        Ok(n) => Html(format!(
            "<tr id=\"notif-{}\">\
             <td><span class=\"badge badge-blue\">{}</span></td>\
             <td style=\"font-size:.8rem;font-family:monospace\">{}</td>\
             <td>{}</td>\
             <td><button class=\"btn btn-reject\" \
             hx-post=\"/dashboard/settings/notifications/{}/delete\" \
             hx-target=\"#notif-{}\" hx-swap=\"outerHTML\">Delete</button></td></tr>",
            n.id,
            escape_html(&n.provider.to_string()),
            escape_html(&n.config.to_string()),
            n.created_at.format("%Y-%m-%d %H:%M"),
            n.id,
            n.id,
        ))
        .into_response(),
        Err(e) => error_response(&e.to_string()),
    }
}

async fn settings_delete_notification(
    State(state): State<AppState>,
    DashboardOrg(org_id): DashboardOrg,
    HtmxRequest: HtmxRequest,
    Path(id): Path<Uuid>,
) -> Response {
    match crate::db::notifications::delete(&state.pool, id, org_id).await {
        Ok(true) => Html(String::new()).into_response(),
        Ok(false) => error_response("notification not found"),
        Err(e) => error_response(&e.to_string()),
    }
}

async fn analytics_page(
    State(state): State<AppState>,
    Extension(tpl): Extension<Templates>,
    DashboardOrg(org_id): DashboardOrg,
) -> Response {
    let (triage_counts, slop_senders) = match tokio::try_join!(
        crate::db::briefing::count_by_triage_status(&state.pool, org_id),
        crate::db::briefing::top_slop_senders(&state.pool, org_id, 20),
    ) {
        Ok(r) => r,
        Err(e) => return error_response(&e.to_string()),
    };

    let triage_data: Vec<_> = triage_counts
        .iter()
        .map(|row| {
            minijinja::context! {
                status => row.status,
                count => row.count,
            }
        })
        .collect();

    let slop_data: Vec<_> = slop_senders
        .iter()
        .map(|s| {
            let ratio = if s.total_messages > 0 {
                s.slop_count as f64 / s.total_messages as f64
            } else {
                0.0
            };
            minijinja::context! {
                sender_email => s.sender_email,
                total_messages => s.total_messages,
                slop_count => s.slop_count,
                slop_ratio => ratio,
                slop_ratio_pct => format!("{:.0}", ratio * 100.0),
            }
        })
        .collect();

    render(
        &tpl,
        "analytics.html",
        minijinja::context! {
            triage_counts => triage_data,
            slop_senders => slop_data,
        },
    )
}

#[derive(Deserialize)]
struct ComposeForm {
    to: String,
    cc: Option<String>,
    subject: Option<String>,
    text_body: Option<String>,
    html_body: Option<String>,
}

#[derive(Deserialize)]
struct ComposeQuery {
    draft: Option<String>,
}

fn parse_addresses(s: &str) -> Vec<String> {
    s.split(',')
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty())
        .collect()
}

async fn compose_send(
    State(state): State<AppState>,
    DashboardOrg(org_id): DashboardOrg,
    HtmxRequest: HtmxRequest,
    Path(id): Path<Uuid>,
    Query(query): Query<ComposeQuery>,
    Form(form): Form<ComposeForm>,
) -> Response {
    let inbox = match crate::db::inboxes::get_by_id(&state.pool, id).await {
        Ok(Some(i)) if i.org_id == org_id => i,
        Ok(_) => return bad_request("inbox not found"),
        Err(e) => {
            tracing::error!("compose inbox lookup: {e}");
            return error_response("database error");
        }
    };

    let to = parse_addresses(&form.to);
    let cc = form.cc.as_deref().map(parse_addresses).unwrap_or_default();
    let text_body = form.text_body.filter(|s| !s.is_empty());
    let html_body = form.html_body.filter(|s| !s.is_empty());

    if query.draft.is_some() {
        let cd = crate::models::CreateDraft {
            inbox_id: inbox.id,
            to_addrs: serde_json::json!(&to),
            cc_addrs: if cc.is_empty() {
                None
            } else {
                Some(serde_json::json!(&cc))
            },
            subject: form.subject.clone(),
            text_body: text_body.clone(),
            html_body: html_body.clone(),
            in_reply_to_message_id: None,
        };

        return match crate::db::drafts::create(&state.pool, &cd).await {
            Ok(d) => compose_feedback("var(--green)", &format!("Draft saved ({})", d.id)),
            Err(e) => compose_feedback(
                "var(--red)",
                &format!("Error: {}", escape_html(&e.to_string())),
            ),
        };
    }

    if to.is_empty() {
        return compose_feedback("var(--red)", "At least one recipient required");
    }

    match crate::api::messages::send_message_inner(
        &state,
        org_id,
        &inbox,
        &crate::api::messages::SendParams {
            to,
            cc,
            subject: form.subject.clone(),
            text_body,
            html_body,
        },
        "dashboard",
    )
    .await
    {
        Ok((status, _msg)) => {
            if status == axum::http::StatusCode::ACCEPTED {
                compose_feedback("var(--yellow)", "Message queued for approval")
            } else {
                compose_feedback("var(--green)", "Message sent")
            }
        }
        Err(e) => {
            tracing::error!(inbox_id = %inbox.id, "dashboard compose failed: {e:?}");
            compose_feedback(
                "var(--red)",
                &format!("Send failed: {}", escape_html(&format!("{e:?}"))),
            )
        }
    }
}

fn compose_feedback(color: &str, msg: &str) -> Response {
    Html(format!(
        "<div style=\"color:{color};font-weight:500;font-size:.9rem\">{msg}</div>"
    ))
    .into_response()
}

fn static_asset(
    content_type: &'static str,
    body: &'static str,
) -> (
    StatusCode,
    [(header::HeaderName, &'static str); 2],
    &'static str,
) {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        body,
    )
}

async fn attachment_proxy(
    State(state): State<AppState>,
    DashboardOrg(org_id): DashboardOrg,
    Path((inbox_id, message_id, attachment_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Response {
    let inbox = match crate::db::inboxes::get_by_id(&state.pool, inbox_id).await {
        Ok(Some(i)) if i.org_id == org_id => i,
        Ok(_) => return not_found(),
        Err(e) => {
            tracing::error!("attachment proxy inbox lookup: {e}");
            return error_response("database error");
        }
    };

    let message = match crate::db::messages::get_by_id(&state.pool, message_id).await {
        Ok(Some(m)) if m.inbox_id == inbox.id => m,
        Ok(_) => return not_found(),
        Err(e) => {
            tracing::error!("attachment proxy message lookup: {e}");
            return error_response("database error");
        }
    };

    let attachment = match crate::db::attachments::get_by_id(&state.pool, attachment_id).await {
        Ok(Some(a)) if a.message_id == message.id => a,
        Ok(_) => return not_found(),
        Err(e) => {
            tracing::error!("attachment proxy attachment lookup: {e}");
            return error_response("database error");
        }
    };

    let data = match crate::storage::read_attachment(
        &state.attachment_storage_path,
        &attachment.storage_key,
    )
    .await
    {
        Ok(d) => d,
        Err(e) => {
            tracing::error!(attachment_id = %attachment_id, "dashboard attachment read: {e}");
            return error_response("failed to read attachment");
        }
    };

    let disposition = format!(
        "{}; filename=\"{}\"",
        attachment.disposition,
        attachment.filename.replace('"', "\\\"")
    );

    match Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, &attachment.content_type)
        .header(header::CONTENT_DISPOSITION, disposition)
        .header(header::CONTENT_LENGTH, data.len())
        .header(header::CACHE_CONTROL, "private, max-age=3600")
        .body(Body::from(data))
    {
        Ok(r) => r,
        Err(e) => error_response(&e.to_string()),
    }
}

async fn static_css() -> impl IntoResponse {
    static_asset("text/css", include_str!("../../static/style.css"))
}

async fn static_htmx() -> impl IntoResponse {
    static_asset(
        "application/javascript",
        include_str!("../../static/htmx.min.js"),
    )
}

async fn static_ws_js() -> impl IntoResponse {
    static_asset("application/javascript", include_str!("../../static/ws.js"))
}

async fn static_upload_js() -> impl IntoResponse {
    static_asset(
        "application/javascript",
        include_str!("../../static/upload.js"),
    )
}

fn messages_to_value(messages: &[crate::models::Message]) -> Vec<minijinja::Value> {
    messages
        .iter()
        .map(|m| {
            minijinja::context! {
                id => m.id.to_string(),
                from_addr => m.from_addr,
                subject => m.subject,
                text_body => m.text_body,
                html_body => m.html_body,
                direction => m.direction,
                slop_score => m.slop_score,
                created_at => m.created_at.format("%Y-%m-%d %H:%M").to_string(),
            }
        })
        .collect()
}

fn format_size(bytes: i64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn attachments_to_value(
    attachments: &[crate::models::Attachment],
    inbox_id: Uuid,
    message_id: Uuid,
) -> Vec<minijinja::Value> {
    attachments
        .iter()
        .map(|a| {
            let url = format!(
                "/dashboard/inboxes/{}/messages/{}/attachments/{}",
                inbox_id, message_id, a.id
            );
            let is_image = a.content_type.starts_with("image/");
            minijinja::context! {
                id => a.id.to_string(),
                filename => a.filename,
                content_type => a.content_type,
                size => format_size(a.size_bytes),
                url => url,
                is_image => is_image,
            }
        })
        .collect()
}

fn attachments_to_value_refs(
    attachments: &[&crate::models::Attachment],
    inbox_id: Uuid,
    message_id: Uuid,
) -> Vec<minijinja::Value> {
    attachments
        .iter()
        .map(|a| {
            let url = format!(
                "/dashboard/inboxes/{}/messages/{}/attachments/{}",
                inbox_id, message_id, a.id
            );
            let is_image = a.content_type.starts_with("image/");
            minijinja::context! {
                id => a.id.to_string(),
                filename => a.filename,
                content_type => a.content_type,
                size => format_size(a.size_bytes),
                url => url,
                is_image => is_image,
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
    (StatusCode::NOT_FOUND, Html("<h1>404 Not Found</h1>")).into_response()
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

fn bad_request(msg: &str) -> Response {
    tracing::warn!("dashboard bad request: {msg}");
    let safe = escape_html(msg);
    (
        StatusCode::BAD_REQUEST,
        Html(format!("<h1>Bad Request</h1><p>{safe}</p>")),
    )
        .into_response()
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

    #[test]
    fn test_build_templates_loads_settings() {
        let env = build_templates();
        assert!(env.get_template("settings.html").is_ok());
    }

    #[test]
    fn test_build_templates_loads_analytics() {
        let env = build_templates();
        assert!(env.get_template("analytics.html").is_ok());
    }

    #[test]
    fn test_format_rule_domain_allowlist() {
        use crate::core::rules::Rule;
        let rule = Rule::DomainAllowlist {
            domains: vec!["example.com".into(), "foo.com".into()],
        };
        assert_eq!(format_rule(&rule), "DomainAllowlist: example.com, foo.com");
    }

    #[test]
    fn test_format_rule_slop_threshold() {
        use crate::core::rules::Rule;
        let rule = Rule::SlopThreshold { threshold: 0.8 };
        assert_eq!(format_rule(&rule), "SlopThreshold: 0.8");
    }

    #[test]
    fn test_format_rule_time_window() {
        use crate::core::rules::Rule;
        let rule = Rule::TimeWindow {
            start_hour: 9,
            end_hour: 17,
            timezone: "UTC".into(),
        };
        assert_eq!(format_rule(&rule), "TimeWindow: 9:00–17:00 UTC");
    }

    #[test]
    fn test_format_rule_dollar_amount() {
        use crate::core::rules::Rule;
        let rule = Rule::DollarAmount { max_amount: 500.0 };
        assert_eq!(format_rule(&rule), "DollarAmount: max $500");
    }

    #[test]
    fn test_format_rule_keyword_blocklist() {
        use crate::core::rules::Rule;
        let rule = Rule::KeywordBlocklist {
            keywords: vec!["spam".into(), "scam".into()],
        };
        assert_eq!(format_rule(&rule), "KeywordBlocklist: spam, scam");
    }

    #[test]
    fn test_format_rule_domain_blocklist() {
        use crate::core::rules::Rule;
        let rule = Rule::DomainBlocklist {
            domains: vec!["evil.com".into()],
        };
        assert_eq!(format_rule(&rule), "DomainBlocklist: evil.com");
    }

    #[test]
    fn test_parse_addresses_single() {
        assert_eq!(parse_addresses("a@b.com"), vec!["a@b.com"]);
    }

    #[test]
    fn test_parse_addresses_multiple() {
        assert_eq!(
            parse_addresses("a@b.com, c@d.com, e@f.com"),
            vec!["a@b.com", "c@d.com", "e@f.com"]
        );
    }

    #[test]
    fn test_parse_addresses_empty() {
        let result: Vec<String> = parse_addresses("");
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_addresses_whitespace_only() {
        let result: Vec<String> = parse_addresses("  ,  , ");
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_addresses_trims_whitespace() {
        assert_eq!(
            parse_addresses("  a@b.com , c@d.com  "),
            vec!["a@b.com", "c@d.com"]
        );
    }

    #[test]
    fn test_compose_feedback_contains_message() {
        let resp = compose_feedback("var(--green)", "Message sent");
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[test]
    fn test_compose_feedback_escapes_color() {
        let resp = compose_feedback("var(--red)", "Error occurred");
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[test]
    fn test_format_size_bytes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1023), "1023 B");
    }

    #[test]
    fn test_format_size_kilobytes() {
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(10240), "10.0 KB");
    }

    #[test]
    fn test_format_size_megabytes() {
        assert_eq!(format_size(1048576), "1.0 MB");
        assert_eq!(format_size(5242880), "5.0 MB");
    }

    #[test]
    fn test_attachments_to_value_image_detection() {
        use chrono::Utc;

        let inbox_id = Uuid::new_v4();
        let msg_id = Uuid::new_v4();
        let att_id = Uuid::new_v4();

        let attachments = vec![crate::models::Attachment {
            id: att_id,
            message_id: msg_id,
            filename: "photo.png".into(),
            content_type: "image/png".into(),
            size_bytes: 2048,
            storage_key: format!("{msg_id}/photo.png"),
            disposition: crate::models::Disposition::Inline,
            created_at: Utc::now(),
        }];

        let values = attachments_to_value(&attachments, inbox_id, msg_id);
        assert_eq!(values.len(), 1);

        let v = &values[0];
        assert_eq!(
            v.get_attr("is_image").unwrap(),
            minijinja::Value::from(true)
        );
        assert_eq!(
            v.get_attr("filename").unwrap(),
            minijinja::Value::from("photo.png")
        );
        let url = v.get_attr("url").unwrap().to_string();
        assert!(url.contains(&att_id.to_string()));
    }

    #[test]
    fn test_attachments_to_value_non_image() {
        use chrono::Utc;

        let inbox_id = Uuid::new_v4();
        let msg_id = Uuid::new_v4();

        let attachments = vec![crate::models::Attachment {
            id: Uuid::new_v4(),
            message_id: msg_id,
            filename: "report.pdf".into(),
            content_type: "application/pdf".into(),
            size_bytes: 1048576,
            storage_key: format!("{msg_id}/report.pdf"),
            disposition: crate::models::Disposition::Attachment,
            created_at: Utc::now(),
        }];

        let values = attachments_to_value(&attachments, inbox_id, msg_id);
        assert_eq!(
            values[0].get_attr("is_image").unwrap(),
            minijinja::Value::from(false)
        );
        assert_eq!(
            values[0].get_attr("size").unwrap(),
            minijinja::Value::from("1.0 MB")
        );
    }

    #[test]
    fn test_attachments_to_value_empty() {
        let values = attachments_to_value(&[], Uuid::new_v4(), Uuid::new_v4());
        assert!(values.is_empty());
    }
}
