pub mod webhooks;
pub mod websocket;

use sqlx::PgPool;
use uuid::Uuid;

pub const KNOWN_EVENTS: &[&str] = &[
    "message.received",
    "message.sent",
    "message.classified",
    "message.bounced",
    "message.delivered",
    "approval.requested",
    "trust.changed",
];

#[derive(Debug, Clone)]
pub enum PostbloxEvent {
    MessageReceived {
        message_id: Uuid,
        inbox_id: Uuid,
    },
    MessageSent {
        message_id: Uuid,
        inbox_id: Uuid,
    },
    MessageClassified {
        message_id: Uuid,
        inbox_id: Uuid,
    },
    ApprovalRequested {
        message_id: Uuid,
        inbox_id: Uuid,
        approval_id: Uuid,
    },
    MessageBounced {
        message_id: Uuid,
        inbox_id: Uuid,
    },
    MessageDelivered {
        message_id: Uuid,
        inbox_id: Uuid,
    },
    TrustChanged {
        inbox_id: Uuid,
        new_mode: crate::models::SendMode,
        approved_count: i32,
    },
}

impl PostbloxEvent {
    pub fn event_name(&self) -> &'static str {
        match self {
            Self::MessageReceived { .. } => "message.received",
            Self::MessageSent { .. } => "message.sent",
            Self::MessageClassified { .. } => "message.classified",
            Self::MessageBounced { .. } => "message.bounced",
            Self::MessageDelivered { .. } => "message.delivered",
            Self::ApprovalRequested { .. } => "approval.requested",
            Self::TrustChanged { .. } => "trust.changed",
        }
    }

    pub fn data(&self) -> serde_json::Value {
        match self {
            Self::MessageReceived {
                message_id,
                inbox_id,
            }
            | Self::MessageSent {
                message_id,
                inbox_id,
            }
            | Self::MessageClassified {
                message_id,
                inbox_id,
            }
            | Self::MessageBounced {
                message_id,
                inbox_id,
            }
            | Self::MessageDelivered {
                message_id,
                inbox_id,
            } => serde_json::json!({
                "message_id": message_id,
                "inbox_id": inbox_id,
            }),
            Self::ApprovalRequested {
                message_id,
                inbox_id,
                approval_id,
            } => serde_json::json!({
                "message_id": message_id,
                "inbox_id": inbox_id,
                "approval_id": approval_id,
            }),
            Self::TrustChanged {
                inbox_id,
                new_mode,
                approved_count,
            } => serde_json::json!({
                "inbox_id": inbox_id,
                "new_mode": new_mode.to_string(),
                "approved_count": approved_count,
            }),
        }
    }
}

pub async fn audit(
    pool: &PgPool,
    org_id: Uuid,
    inbox_id: Option<Uuid>,
    action: crate::models::AuditAction,
    actor: &str,
    details: serde_json::Value,
) {
    if let Err(e) =
        crate::db::audit::create_entry(pool, org_id, inbox_id, action, actor, details).await
    {
        tracing::error!("failed to create audit entry: {e}");
    }
}

pub async fn dispatch(
    pool: &PgPool,
    org_id: Uuid,
    event: PostbloxEvent,
    client: &reqwest::Client,
    hooks: &[crate::hooks::HookConfig],
    ws_hub: &websocket::WebSocketHub,
) {
    match &event {
        PostbloxEvent::ApprovalRequested {
            message_id,
            inbox_id,
            approval_id,
        } => {
            crate::notifications::notify_org(
                pool,
                org_id,
                "Approval Required",
                &format!(
                    "Message {message_id} from inbox {inbox_id} needs approval (approval {approval_id})"
                ),
                client,
            )
            .await;
        }
        PostbloxEvent::TrustChanged {
            inbox_id,
            new_mode,
            approved_count,
        } => {
            crate::notifications::notify_org(
                pool,
                org_id,
                "Trust Level Changed",
                &format!(
                    "Inbox {inbox_id} auto-upgraded to {new_mode} after {approved_count} approved sends"
                ),
                client,
            )
            .await;
        }
        _ => {}
    }

    let event_name = event.event_name();
    let data = event.data();

    let wh_list = match crate::db::webhooks::list_active_for_event(pool, org_id, event_name).await {
        Ok(h) => h,
        Err(e) => {
            tracing::error!("failed to query webhooks for {event_name}: {e}");
            ws_hub.broadcast(org_id, event_name, &data);
            crate::hooks::run_event_hooks(hooks, event_name, data);
            return;
        }
    };

    if wh_list.is_empty() && hooks.iter().all(|h| h.event != event_name) {
        // broadcast() no-ops internally when no receivers — skip webhook/hook
        // work but still call broadcast to avoid a second lock acquisition
        ws_hub.broadcast(org_id, event_name, &data);
        return;
    }

    if wh_list.len() > 20 {
        tracing::warn!(org_id = %org_id, count = wh_list.len(), "exceeds webhook concurrency limit, delivering first 20");
    }

    for wh in wh_list.into_iter().take(20) {
        let client = client.clone();
        let name = event_name.to_string();
        let payload = data.clone();
        tokio::spawn(async move {
            if let Err(reason) = validate_webhook_url(&wh.url).await {
                tracing::warn!(webhook_id = %wh.id, url = %wh.url, "webhook blocked (SSRF): {reason}");
                return;
            }
            if let Err(e) = webhooks::deliver(&client, &wh.url, &wh.secret, &name, &payload).await {
                tracing::warn!(webhook_id = %wh.id, url = %wh.url, "webhook delivery failed: {e}");
            }
        });
    }

    ws_hub.broadcast(org_id, event_name, &data);

    crate::hooks::run_event_hooks(hooks, event_name, data);
}

fn is_private_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64) // 100.64.0.0/10 CGN
        }
        std::net::IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // fc00::/7 unique local
                || v6.to_ipv4_mapped().is_some_and(|v4| {
                    v4.is_loopback()
                        || v4.is_private()
                        || v4.is_link_local()
                        || v4.is_broadcast()
                        || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64)
                })
        }
    }
}

fn extract_host_port(url: &str) -> Option<(String, u16)> {
    let without_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let host_port = without_scheme.split('/').next()?;
    if host_port.is_empty() {
        return None;
    }
    if let Some((host, port_str)) = host_port.rsplit_once(':') {
        let port: u16 = port_str.parse().ok()?;
        Some((host.to_string(), port))
    } else {
        let default_port = if url.starts_with("https://") { 443 } else { 80 };
        Some((host_port.to_string(), default_port))
    }
}

async fn validate_webhook_url(url: &str) -> Result<(), String> {
    let (host, port) =
        extract_host_port(url).ok_or_else(|| format!("invalid webhook URL: {url}"))?;
    let addrs: Vec<_> = tokio::net::lookup_host(format!("{host}:{port}"))
        .await
        .map_err(|e| format!("DNS resolution failed for {host}: {e}"))?
        .collect();
    if addrs.is_empty() {
        return Err(format!("no DNS records for {host}"));
    }
    for addr in &addrs {
        if is_private_ip(addr.ip()) {
            return Err(format!(
                "webhook URL {host} resolves to private/reserved IP {}",
                addr.ip()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_name_message_received() {
        let event = PostbloxEvent::MessageReceived {
            message_id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
        };
        assert_eq!(event.event_name(), "message.received");
    }

    #[test]
    fn test_event_name_message_sent() {
        let event = PostbloxEvent::MessageSent {
            message_id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
        };
        assert_eq!(event.event_name(), "message.sent");
    }

    #[test]
    fn test_event_data_contains_ids() {
        let msg_id = Uuid::new_v4();
        let inbox_id = Uuid::new_v4();
        let event = PostbloxEvent::MessageReceived {
            message_id: msg_id,
            inbox_id,
        };
        let data = event.data();
        assert_eq!(data["message_id"], msg_id.to_string());
        assert_eq!(data["inbox_id"], inbox_id.to_string());
    }

    #[test]
    fn test_event_data_sent_contains_ids() {
        let msg_id = Uuid::new_v4();
        let inbox_id = Uuid::new_v4();
        let event = PostbloxEvent::MessageSent {
            message_id: msg_id,
            inbox_id,
        };
        let data = event.data();
        assert_eq!(data["message_id"], msg_id.to_string());
        assert_eq!(data["inbox_id"], inbox_id.to_string());
    }

    #[test]
    fn test_event_name_approval_requested() {
        let event = PostbloxEvent::ApprovalRequested {
            message_id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
            approval_id: Uuid::new_v4(),
        };
        assert_eq!(event.event_name(), "approval.requested");
    }

    #[test]
    fn test_event_data_approval_requested_contains_ids() {
        let msg_id = Uuid::new_v4();
        let inbox_id = Uuid::new_v4();
        let approval_id = Uuid::new_v4();
        let event = PostbloxEvent::ApprovalRequested {
            message_id: msg_id,
            inbox_id,
            approval_id,
        };
        let data = event.data();
        assert_eq!(data["message_id"], msg_id.to_string());
        assert_eq!(data["inbox_id"], inbox_id.to_string());
        assert_eq!(data["approval_id"], approval_id.to_string());
    }

    #[test]
    fn test_known_events_includes_approval_requested() {
        assert!(KNOWN_EVENTS.contains(&"approval.requested"));
    }

    #[test]
    fn test_event_name_trust_changed() {
        let event = PostbloxEvent::TrustChanged {
            inbox_id: Uuid::new_v4(),
            new_mode: crate::models::SendMode::AutoApprove,
            approved_count: 10,
        };
        assert_eq!(event.event_name(), "trust.changed");
    }

    #[test]
    fn test_event_data_trust_changed_contains_fields() {
        let inbox_id = Uuid::new_v4();
        let event = PostbloxEvent::TrustChanged {
            inbox_id,
            new_mode: crate::models::SendMode::AutoApprove,
            approved_count: 15,
        };
        let data = event.data();
        assert_eq!(data["inbox_id"], inbox_id.to_string());
        assert_eq!(data["new_mode"], "auto_approve");
        assert_eq!(data["approved_count"], 15);
    }

    #[test]
    fn test_known_events_includes_trust_changed() {
        assert!(KNOWN_EVENTS.contains(&"trust.changed"));
    }

    #[test]
    fn test_event_name_message_bounced() {
        let event = PostbloxEvent::MessageBounced {
            message_id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
        };
        assert_eq!(event.event_name(), "message.bounced");
    }

    #[test]
    fn test_event_name_message_delivered() {
        let event = PostbloxEvent::MessageDelivered {
            message_id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
        };
        assert_eq!(event.event_name(), "message.delivered");
    }

    #[test]
    fn test_event_data_bounced_contains_ids() {
        let msg_id = Uuid::new_v4();
        let inbox_id = Uuid::new_v4();
        let event = PostbloxEvent::MessageBounced {
            message_id: msg_id,
            inbox_id,
        };
        let data = event.data();
        assert_eq!(data["message_id"], msg_id.to_string());
        assert_eq!(data["inbox_id"], inbox_id.to_string());
    }

    #[test]
    fn test_known_events_includes_message_bounced() {
        assert!(KNOWN_EVENTS.contains(&"message.bounced"));
    }

    #[test]
    fn test_known_events_includes_message_delivered() {
        assert!(KNOWN_EVENTS.contains(&"message.delivered"));
    }

    #[test]
    fn test_is_private_ip_loopback_v4() {
        assert!(is_private_ip("127.0.0.1".parse().unwrap()));
        assert!(is_private_ip("127.0.0.2".parse().unwrap()));
    }

    #[test]
    fn test_is_private_ip_rfc1918_10() {
        assert!(is_private_ip("10.0.0.1".parse().unwrap()));
        assert!(is_private_ip("10.255.255.255".parse().unwrap()));
    }

    #[test]
    fn test_is_private_ip_rfc1918_172() {
        assert!(is_private_ip("172.16.0.1".parse().unwrap()));
        assert!(is_private_ip("172.31.255.255".parse().unwrap()));
        assert!(!is_private_ip("172.32.0.1".parse().unwrap()));
    }

    #[test]
    fn test_is_private_ip_rfc1918_192() {
        assert!(is_private_ip("192.168.0.1".parse().unwrap()));
        assert!(is_private_ip("192.168.255.255".parse().unwrap()));
    }

    #[test]
    fn test_is_private_ip_link_local() {
        assert!(is_private_ip("169.254.1.1".parse().unwrap()));
        assert!(is_private_ip("169.254.169.254".parse().unwrap()));
    }

    #[test]
    fn test_is_private_ip_unspecified() {
        assert!(is_private_ip("0.0.0.0".parse().unwrap()));
        assert!(is_private_ip("::".parse().unwrap()));
    }

    #[test]
    fn test_is_private_ip_broadcast() {
        assert!(is_private_ip("255.255.255.255".parse().unwrap()));
    }

    #[test]
    fn test_is_private_ip_cgn() {
        assert!(is_private_ip("100.64.0.1".parse().unwrap()));
        assert!(is_private_ip("100.127.255.255".parse().unwrap()));
        assert!(!is_private_ip("100.128.0.1".parse().unwrap()));
    }

    #[test]
    fn test_is_private_ip_v6_unique_local() {
        assert!(is_private_ip("fc00::1".parse().unwrap()));
        assert!(is_private_ip("fd12:3456::1".parse().unwrap()));
    }

    #[test]
    fn test_is_private_ip_public() {
        assert!(!is_private_ip("8.8.8.8".parse().unwrap()));
        assert!(!is_private_ip("1.1.1.1".parse().unwrap()));
        assert!(!is_private_ip("203.0.113.1".parse().unwrap()));
    }

    #[test]
    fn test_is_private_ip_loopback_v6() {
        assert!(is_private_ip("::1".parse().unwrap()));
    }

    #[test]
    fn test_is_private_ip_v4_mapped_v6() {
        assert!(is_private_ip("::ffff:127.0.0.1".parse().unwrap()));
        assert!(is_private_ip("::ffff:10.0.0.1".parse().unwrap()));
        assert!(!is_private_ip("::ffff:8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn test_extract_host_port_https() {
        assert_eq!(
            extract_host_port("https://example.com/webhook"),
            Some(("example.com".into(), 443))
        );
    }

    #[test]
    fn test_extract_host_port_http() {
        assert_eq!(
            extract_host_port("http://example.com/path"),
            Some(("example.com".into(), 80))
        );
    }

    #[test]
    fn test_extract_host_port_custom_port() {
        assert_eq!(
            extract_host_port("https://example.com:8443/webhook"),
            Some(("example.com".into(), 8443))
        );
    }

    #[test]
    fn test_extract_host_port_no_path() {
        assert_eq!(
            extract_host_port("https://example.com"),
            Some(("example.com".into(), 443))
        );
    }

    #[test]
    fn test_extract_host_port_invalid() {
        assert!(extract_host_port("not-a-url").is_none());
        assert!(extract_host_port("ftp://example.com").is_none());
    }

    #[tokio::test]
    async fn test_validate_webhook_url_public() {
        // google.com should resolve to public IPs
        let result = validate_webhook_url("https://google.com/webhook").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_webhook_url_localhost() {
        let result = validate_webhook_url("http://localhost:8080/hook").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("private/reserved"));
    }

    #[tokio::test]
    async fn test_validate_webhook_url_invalid() {
        let result = validate_webhook_url("not-a-url").await;
        assert!(result.is_err());
    }
}
