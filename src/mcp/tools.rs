use serde_json::{json, Value};

use crate::client::PostbloxClient;
use crate::error::McpError;

fn require_str(args: &Value, key: &str) -> Result<String, McpError> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| McpError::MissingArgument(key.into()))
}

fn optional_str(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(|v| v.as_str()).map(String::from)
}

fn optional_i64(args: &Value, key: &str) -> Option<i64> {
    args.get(key).and_then(|v| v.as_i64())
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push('%');
                out.push(char::from(b"0123456789ABCDEF"[(b >> 4) as usize]));
                out.push(char::from(b"0123456789ABCDEF"[(b & 0x0f) as usize]));
            }
        }
    }
    out
}

fn build_query_string(params: &[(&str, Option<String>)]) -> String {
    let pairs: Vec<String> = params
        .iter()
        .filter_map(|(k, v)| v.as_ref().map(|val| format!("{k}={}", url_encode(val))))
        .collect();
    if pairs.is_empty() {
        String::new()
    } else {
        format!("?{}", pairs.join("&"))
    }
}

pub fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "postblox_list_inboxes",
            "description": "List all inboxes for the organization.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
        json!({
            "name": "postblox_create_inbox",
            "description": "Create a new inbox with the given email address.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "email": { "type": "string", "description": "Email address for the inbox" },
                    "display_name": { "type": "string", "description": "Optional display name" },
                    "inbox_type": { "type": "string", "description": "Inbox type (default: native)" }
                },
                "required": ["email"]
            }
        }),
        json!({
            "name": "postblox_get_inbox",
            "description": "Get a specific inbox by ID.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "inbox_id": { "type": "string", "description": "UUID of the inbox" }
                },
                "required": ["inbox_id"]
            }
        }),
        json!({
            "name": "postblox_delete_inbox",
            "description": "Delete an inbox by ID.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "inbox_id": { "type": "string", "description": "UUID of the inbox" }
                },
                "required": ["inbox_id"]
            }
        }),
        json!({
            "name": "postblox_send_email",
            "description": "Send an email from an inbox.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "inbox_id": { "type": "string", "description": "UUID of the sending inbox" },
                    "to": { "type": "array", "items": { "type": "string" }, "description": "Recipient email addresses" },
                    "subject": { "type": "string", "description": "Email subject" },
                    "text_body": { "type": "string", "description": "Plain text body" },
                    "html_body": { "type": "string", "description": "HTML body" },
                    "cc": { "type": "array", "items": { "type": "string" }, "description": "CC recipients" },
                    "in_reply_to": { "type": "string", "description": "Message-ID header of the message being replied to" }
                },
                "required": ["inbox_id", "to", "subject"]
            }
        }),
        json!({
            "name": "postblox_list_messages",
            "description": "List messages in an inbox, optionally filtered by thread.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "inbox_id": { "type": "string", "description": "UUID of the inbox" },
                    "limit": { "type": "integer", "description": "Max results (1-100, default 50)" },
                    "offset": { "type": "integer", "description": "Offset for pagination" },
                    "thread_id": { "type": "string", "description": "Filter by thread UUID" }
                },
                "required": ["inbox_id"]
            }
        }),
        json!({
            "name": "postblox_get_message",
            "description": "Get a specific message by ID.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "inbox_id": { "type": "string", "description": "UUID of the inbox" },
                    "message_id": { "type": "string", "description": "UUID of the message" }
                },
                "required": ["inbox_id", "message_id"]
            }
        }),
        json!({
            "name": "postblox_list_threads",
            "description": "List threads in an inbox.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "inbox_id": { "type": "string", "description": "UUID of the inbox" },
                    "limit": { "type": "integer", "description": "Max results (1-100, default 50)" },
                    "offset": { "type": "integer", "description": "Offset for pagination" }
                },
                "required": ["inbox_id"]
            }
        }),
        json!({
            "name": "postblox_get_thread",
            "description": "Get a specific thread with its messages.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "inbox_id": { "type": "string", "description": "UUID of the inbox" },
                    "thread_id": { "type": "string", "description": "UUID of the thread" }
                },
                "required": ["inbox_id", "thread_id"]
            }
        }),
        json!({
            "name": "postblox_search",
            "description": "Search across messages. Supports full-text and semantic (vector similarity) search.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "q": { "type": "string", "description": "Search query" },
                    "inbox_id": { "type": "string", "description": "Limit search to a specific inbox" },
                    "limit": { "type": "integer", "description": "Max results" },
                    "offset": { "type": "integer", "description": "Offset for pagination" },
                    "semantic": { "type": "boolean", "description": "Use semantic (vector similarity) search instead of full-text" },
                    "threshold": { "type": "number", "description": "Minimum similarity threshold for semantic search (0.0-1.0, default 0.7)" }
                },
                "required": ["q"]
            }
        }),
        json!({
            "name": "postblox_briefing",
            "description": "Get a summary briefing of recent email activity.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "period": { "type": "string", "description": "Time period: 1h, 6h, 12h, 24h, or 7d (default: 24h)" }
                },
                "required": []
            }
        }),
        json!({
            "name": "postblox_register_webhook",
            "description": "Register a webhook to receive email event notifications.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "Webhook callback URL" },
                    "events": { "type": "array", "items": { "type": "string" }, "description": "Events to subscribe to (message.received, message.sent)" }
                },
                "required": ["url", "events"]
            }
        }),
        json!({
            "name": "postblox_list_webhooks",
            "description": "List all registered webhooks.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
        json!({
            "name": "postblox_delete_webhook",
            "description": "Delete a webhook by ID.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "webhook_id": { "type": "string", "description": "UUID of the webhook" }
                },
                "required": ["webhook_id"]
            }
        }),
        json!({
            "name": "postblox_list_labels",
            "description": "List labels for an inbox.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "inbox_id": { "type": "string", "description": "UUID of the inbox" }
                },
                "required": ["inbox_id"]
            }
        }),
        json!({
            "name": "postblox_create_label",
            "description": "Create a label for an inbox.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "inbox_id": { "type": "string", "description": "UUID of the inbox" },
                    "name": { "type": "string", "description": "Label name" },
                    "color": { "type": "string", "description": "Optional hex color (e.g. #ff0000)" }
                },
                "required": ["inbox_id", "name"]
            }
        }),
        json!({
            "name": "postblox_add_label_to_message",
            "description": "Add a label to a message.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "inbox_id": { "type": "string", "description": "UUID of the inbox" },
                    "message_id": { "type": "string", "description": "UUID of the message" },
                    "label_id": { "type": "string", "description": "UUID of the label" }
                },
                "required": ["inbox_id", "message_id", "label_id"]
            }
        }),
        json!({
            "name": "postblox_remove_label_from_message",
            "description": "Remove a label from a message.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "inbox_id": { "type": "string", "description": "UUID of the inbox" },
                    "message_id": { "type": "string", "description": "UUID of the message" },
                    "label_id": { "type": "string", "description": "UUID of the label" }
                },
                "required": ["inbox_id", "message_id", "label_id"]
            }
        }),
        json!({
            "name": "postblox_list_drafts",
            "description": "List drafts in an inbox.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "inbox_id": { "type": "string", "description": "UUID of the inbox" },
                    "limit": { "type": "integer", "description": "Max results" },
                    "offset": { "type": "integer", "description": "Offset for pagination" }
                },
                "required": ["inbox_id"]
            }
        }),
        json!({
            "name": "postblox_create_draft",
            "description": "Create a draft email.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "inbox_id": { "type": "string", "description": "UUID of the inbox" },
                    "to": { "type": "array", "items": { "type": "string" }, "description": "Recipients" },
                    "cc": { "type": "array", "items": { "type": "string" }, "description": "CC recipients" },
                    "subject": { "type": "string", "description": "Email subject" },
                    "text_body": { "type": "string", "description": "Plain text body" },
                    "html_body": { "type": "string", "description": "HTML body" },
                    "in_reply_to_message_id": { "type": "string", "description": "UUID of the message being replied to" }
                },
                "required": ["inbox_id"]
            }
        }),
        json!({
            "name": "postblox_update_draft",
            "description": "Update a draft email.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "inbox_id": { "type": "string", "description": "UUID of the inbox" },
                    "draft_id": { "type": "string", "description": "UUID of the draft" },
                    "to": { "type": "array", "items": { "type": "string" }, "description": "Recipients" },
                    "cc": { "type": "array", "items": { "type": "string" }, "description": "CC recipients" },
                    "subject": { "type": "string", "description": "Email subject" },
                    "text_body": { "type": "string", "description": "Plain text body" },
                    "html_body": { "type": "string", "description": "HTML body" }
                },
                "required": ["inbox_id", "draft_id"]
            }
        }),
        json!({
            "name": "postblox_send_draft",
            "description": "Send a draft, converting it to a sent message and deleting the draft.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "inbox_id": { "type": "string", "description": "UUID of the inbox" },
                    "draft_id": { "type": "string", "description": "UUID of the draft" }
                },
                "required": ["inbox_id", "draft_id"]
            }
        }),
        json!({
            "name": "postblox_delete_draft",
            "description": "Delete a draft.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "inbox_id": { "type": "string", "description": "UUID of the inbox" },
                    "draft_id": { "type": "string", "description": "UUID of the draft" }
                },
                "required": ["inbox_id", "draft_id"]
            }
        }),
        json!({
            "name": "postblox_list_domains",
            "description": "List all domains for the organization.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
        json!({
            "name": "postblox_create_domain",
            "description": "Add a domain to the organization for email sending.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Domain name (e.g. example.com)" }
                },
                "required": ["name"]
            }
        }),
        json!({
            "name": "postblox_get_domain",
            "description": "Get domain details including DNS records.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "domain_id": { "type": "string", "description": "UUID of the domain" }
                },
                "required": ["domain_id"]
            }
        }),
        json!({
            "name": "postblox_verify_domain",
            "description": "Trigger DNS verification for a domain.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "domain_id": { "type": "string", "description": "UUID of the domain" }
                },
                "required": ["domain_id"]
            }
        }),
        json!({
            "name": "postblox_delete_domain",
            "description": "Delete a domain.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "domain_id": { "type": "string", "description": "UUID of the domain" }
                },
                "required": ["domain_id"]
            }
        }),
        json!({
            "name": "postblox_list_approvals",
            "description": "List pending approval requests, paginated.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "description": "Max results" },
                    "offset": { "type": "integer", "description": "Offset for pagination" }
                },
                "required": []
            }
        }),
        json!({
            "name": "postblox_get_approval",
            "description": "Get a specific approval request by ID.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "approval_id": { "type": "string", "description": "UUID of the approval" }
                },
                "required": ["approval_id"]
            }
        }),
        json!({
            "name": "postblox_approve_message",
            "description": "Approve a pending message.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "approval_id": { "type": "string", "description": "UUID of the approval" },
                    "decided_by": { "type": "string", "description": "Who approved this message" }
                },
                "required": ["approval_id", "decided_by"]
            }
        }),
        json!({
            "name": "postblox_reject_message",
            "description": "Reject a pending message.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "approval_id": { "type": "string", "description": "UUID of the approval" },
                    "decided_by": { "type": "string", "description": "Who rejected this message" }
                },
                "required": ["approval_id", "decided_by"]
            }
        }),
        json!({
            "name": "postblox_batch_approvals",
            "description": "Approve or reject multiple messages at once.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "ids": { "type": "array", "items": { "type": "string" }, "description": "UUIDs of the approvals" },
                    "status": { "type": "string", "enum": ["approved", "rejected"], "description": "Decision: approved or rejected" },
                    "decided_by": { "type": "string", "description": "Who made this decision" }
                },
                "required": ["ids", "status", "decided_by"]
            }
        }),
        json!({
            "name": "postblox_get_permissions",
            "description": "Get the permission settings for an inbox.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "inbox_id": { "type": "string", "description": "UUID of the inbox" }
                },
                "required": ["inbox_id"]
            }
        }),
        json!({
            "name": "postblox_set_permissions",
            "description": "Update permission settings for an inbox.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "inbox_id": { "type": "string", "description": "UUID of the inbox" },
                    "send_mode": { "type": "string", "enum": ["shadow", "approval", "auto_approve", "autonomous"], "description": "Outbound sending mode" },
                    "rules": { "type": "array", "description": "Permission rules as JSON array" }
                },
                "required": ["inbox_id"]
            }
        }),
        json!({
            "name": "postblox_get_trust",
            "description": "Get trust level for an inbox.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "inbox_id": { "type": "string", "description": "UUID of the inbox" }
                },
                "required": ["inbox_id"]
            }
        }),
        json!({
            "name": "postblox_list_audit",
            "description": "List audit log entries with optional filters.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "description": "Max results" },
                    "offset": { "type": "integer", "description": "Offset for pagination" },
                    "inbox_id": { "type": "string", "description": "Filter by inbox UUID" },
                    "action": { "type": "string", "description": "Filter by action type" },
                    "after": { "type": "string", "description": "Filter entries after this ISO datetime" },
                    "before": { "type": "string", "description": "Filter entries before this ISO datetime" }
                },
                "required": []
            }
        }),
        json!({
            "name": "postblox_list_notifications",
            "description": "List notification channels.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
        json!({
            "name": "postblox_create_notification",
            "description": "Create a notification channel.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "provider": { "type": "string", "enum": ["ntfy", "email", "webhook"], "description": "Notification provider" },
                    "config": { "type": "object", "description": "Provider-specific configuration" }
                },
                "required": ["provider", "config"]
            }
        }),
        json!({
            "name": "postblox_delete_notification",
            "description": "Delete a notification channel.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "notification_id": { "type": "string", "description": "UUID of the notification channel" }
                },
                "required": ["notification_id"]
            }
        }),
        json!({
            "name": "postblox_reply",
            "description": "Reply to a message. Automatically threads the reply and sets the recipient to the original sender.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "inbox_id": { "type": "string", "description": "UUID of the inbox" },
                    "message_id": { "type": "string", "description": "UUID of the message to reply to" },
                    "text_body": { "type": "string", "description": "Plain text reply body" },
                    "html_body": { "type": "string", "description": "HTML reply body" },
                    "cc": { "type": "array", "items": { "type": "string" }, "description": "CC recipients" }
                },
                "required": ["inbox_id", "message_id", "text_body"]
            }
        }),
        json!({
            "name": "postblox_link_inbox",
            "description": "Link an external IMAP inbox for syncing emails into postblox.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "inbox_id": { "type": "string", "description": "UUID of the postblox inbox to sync into" },
                    "imap_host": { "type": "string", "description": "IMAP server hostname" },
                    "imap_port": { "type": "integer", "description": "IMAP port (default: 993)" },
                    "username": { "type": "string", "description": "IMAP username" },
                    "password": { "type": "string", "description": "IMAP password" }
                },
                "required": ["inbox_id", "imap_host", "username", "password"]
            }
        }),
        json!({
            "name": "postblox_list_linked_inboxes",
            "description": "List all linked external inboxes.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
        json!({
            "name": "postblox_sync_linked_inbox",
            "description": "Trigger an IMAP sync for a linked inbox.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "linked_account_id": { "type": "string", "description": "UUID of the linked account" }
                },
                "required": ["linked_account_id"]
            }
        }),
    ]
}

pub async fn dispatch(
    client: &PostbloxClient,
    name: &str,
    args: Value,
) -> Result<String, McpError> {
    match name {
        "postblox_list_inboxes" => client.get("/inboxes").await,

        "postblox_create_inbox" => {
            let email = require_str(&args, "email")?;
            let mut body = json!({ "email": email });
            if let Some(dn) = optional_str(&args, "display_name") {
                body["display_name"] = json!(dn);
            }
            if let Some(it) = optional_str(&args, "inbox_type") {
                body["inbox_type"] = json!(it);
            }
            client.post("/inboxes", body).await
        }

        "postblox_get_inbox" => {
            let id = require_str(&args, "inbox_id")?;
            client.get(&format!("/inboxes/{id}")).await
        }

        "postblox_delete_inbox" => {
            let id = require_str(&args, "inbox_id")?;
            client.delete(&format!("/inboxes/{id}")).await
        }

        "postblox_send_email" => {
            let inbox_id = require_str(&args, "inbox_id")?;
            let mut body = json!({
                "to": args.get("to").cloned().unwrap_or(json!([])),
                "subject": require_str(&args, "subject")?,
            });
            if let Some(tb) = optional_str(&args, "text_body") {
                body["text_body"] = json!(tb);
            }
            if let Some(hb) = optional_str(&args, "html_body") {
                body["html_body"] = json!(hb);
            }
            if let Some(cc) = args.get("cc").cloned() {
                body["cc"] = cc;
            }
            if let Some(irt) = optional_str(&args, "in_reply_to") {
                body["in_reply_to"] = json!(irt);
            }
            client
                .post(&format!("/inboxes/{inbox_id}/messages"), body)
                .await
        }

        "postblox_list_messages" => {
            let inbox_id = require_str(&args, "inbox_id")?;
            let qs = build_query_string(&[
                ("limit", optional_i64(&args, "limit").map(|v| v.to_string())),
                (
                    "offset",
                    optional_i64(&args, "offset").map(|v| v.to_string()),
                ),
                ("thread_id", optional_str(&args, "thread_id")),
            ]);
            client
                .get(&format!("/inboxes/{inbox_id}/messages{qs}"))
                .await
        }

        "postblox_get_message" => {
            let inbox_id = require_str(&args, "inbox_id")?;
            let msg_id = require_str(&args, "message_id")?;
            client
                .get(&format!("/inboxes/{inbox_id}/messages/{msg_id}"))
                .await
        }

        "postblox_list_threads" => {
            let inbox_id = require_str(&args, "inbox_id")?;
            let qs = build_query_string(&[
                ("limit", optional_i64(&args, "limit").map(|v| v.to_string())),
                (
                    "offset",
                    optional_i64(&args, "offset").map(|v| v.to_string()),
                ),
            ]);
            client
                .get(&format!("/inboxes/{inbox_id}/threads{qs}"))
                .await
        }

        "postblox_get_thread" => {
            let inbox_id = require_str(&args, "inbox_id")?;
            let thread_id = require_str(&args, "thread_id")?;
            client
                .get(&format!("/inboxes/{inbox_id}/threads/{thread_id}"))
                .await
        }

        "postblox_search" => {
            let q = require_str(&args, "q")?;
            let semantic = args
                .get("semantic")
                .and_then(|v| v.as_bool())
                .map(|v| v.to_string());
            let threshold = args
                .get("threshold")
                .and_then(|v| v.as_f64())
                .map(|v| v.to_string());
            let params: Vec<(&str, Option<String>)> = vec![
                ("q", Some(q)),
                ("inbox_id", optional_str(&args, "inbox_id")),
                ("limit", optional_i64(&args, "limit").map(|v| v.to_string())),
                (
                    "offset",
                    optional_i64(&args, "offset").map(|v| v.to_string()),
                ),
                ("semantic", semantic),
                ("threshold", threshold),
            ];
            let qs = build_query_string(&params);
            client.get(&format!("/search{qs}")).await
        }

        "postblox_briefing" => {
            let qs = build_query_string(&[("period", optional_str(&args, "period"))]);
            client.get(&format!("/briefing{qs}")).await
        }

        "postblox_register_webhook" => {
            let body = json!({
                "url": require_str(&args, "url")?,
                "events": args.get("events").cloned().unwrap_or(json!([])),
            });
            client.post("/webhooks", body).await
        }

        "postblox_list_webhooks" => client.get("/webhooks").await,

        "postblox_delete_webhook" => {
            let id = require_str(&args, "webhook_id")?;
            client.delete(&format!("/webhooks/{id}")).await
        }

        "postblox_list_labels" => {
            let inbox_id = require_str(&args, "inbox_id")?;
            client.get(&format!("/inboxes/{inbox_id}/labels")).await
        }

        "postblox_create_label" => {
            let inbox_id = require_str(&args, "inbox_id")?;
            let mut body = json!({ "name": require_str(&args, "name")? });
            if let Some(c) = optional_str(&args, "color") {
                body["color"] = json!(c);
            }
            client
                .post(&format!("/inboxes/{inbox_id}/labels"), body)
                .await
        }

        "postblox_add_label_to_message" => {
            let inbox_id = require_str(&args, "inbox_id")?;
            let msg_id = require_str(&args, "message_id")?;
            let label_id = require_str(&args, "label_id")?;
            client
                .post(
                    &format!("/inboxes/{inbox_id}/messages/{msg_id}/labels"),
                    json!({ "label_id": label_id }),
                )
                .await
        }

        "postblox_remove_label_from_message" => {
            let inbox_id = require_str(&args, "inbox_id")?;
            let msg_id = require_str(&args, "message_id")?;
            let label_id = require_str(&args, "label_id")?;
            client
                .delete(&format!(
                    "/inboxes/{inbox_id}/messages/{msg_id}/labels/{label_id}"
                ))
                .await
        }

        "postblox_list_drafts" => {
            let inbox_id = require_str(&args, "inbox_id")?;
            let qs = build_query_string(&[
                ("limit", optional_i64(&args, "limit").map(|v| v.to_string())),
                (
                    "offset",
                    optional_i64(&args, "offset").map(|v| v.to_string()),
                ),
            ]);
            client.get(&format!("/inboxes/{inbox_id}/drafts{qs}")).await
        }

        "postblox_create_draft" => {
            let inbox_id = require_str(&args, "inbox_id")?;
            let mut body = json!({});
            if let Some(to) = args.get("to").cloned() {
                body["to"] = to;
            }
            if let Some(cc) = args.get("cc").cloned() {
                body["cc"] = cc;
            }
            if let Some(s) = optional_str(&args, "subject") {
                body["subject"] = json!(s);
            }
            if let Some(tb) = optional_str(&args, "text_body") {
                body["text_body"] = json!(tb);
            }
            if let Some(hb) = optional_str(&args, "html_body") {
                body["html_body"] = json!(hb);
            }
            if let Some(irt) = optional_str(&args, "in_reply_to_message_id") {
                body["in_reply_to_message_id"] = json!(irt);
            }
            client
                .post(&format!("/inboxes/{inbox_id}/drafts"), body)
                .await
        }

        "postblox_update_draft" => {
            let inbox_id = require_str(&args, "inbox_id")?;
            let draft_id = require_str(&args, "draft_id")?;
            let mut body = json!({});
            if let Some(to) = args.get("to").cloned() {
                body["to"] = to;
            }
            if let Some(cc) = args.get("cc").cloned() {
                body["cc"] = cc;
            }
            if let Some(s) = optional_str(&args, "subject") {
                body["subject"] = json!(s);
            }
            if let Some(tb) = optional_str(&args, "text_body") {
                body["text_body"] = json!(tb);
            }
            if let Some(hb) = optional_str(&args, "html_body") {
                body["html_body"] = json!(hb);
            }
            client
                .put(&format!("/inboxes/{inbox_id}/drafts/{draft_id}"), body)
                .await
        }

        "postblox_send_draft" => {
            let inbox_id = require_str(&args, "inbox_id")?;
            let draft_id = require_str(&args, "draft_id")?;
            client
                .post(
                    &format!("/inboxes/{inbox_id}/drafts/{draft_id}/send"),
                    json!({}),
                )
                .await
        }

        "postblox_delete_draft" => {
            let inbox_id = require_str(&args, "inbox_id")?;
            let draft_id = require_str(&args, "draft_id")?;
            client
                .delete(&format!("/inboxes/{inbox_id}/drafts/{draft_id}"))
                .await
        }

        "postblox_list_domains" => client.get("/domains").await,

        "postblox_create_domain" => {
            let body = json!({ "name": require_str(&args, "name")? });
            client.post("/domains", body).await
        }

        "postblox_get_domain" => {
            let id = require_str(&args, "domain_id")?;
            client.get(&format!("/domains/{id}")).await
        }

        "postblox_verify_domain" => {
            let id = require_str(&args, "domain_id")?;
            client
                .post(&format!("/domains/{id}/verify"), json!({}))
                .await
        }

        "postblox_delete_domain" => {
            let id = require_str(&args, "domain_id")?;
            client.delete(&format!("/domains/{id}")).await
        }

        "postblox_list_approvals" => {
            let qs = build_query_string(&[
                ("limit", optional_i64(&args, "limit").map(|v| v.to_string())),
                (
                    "offset",
                    optional_i64(&args, "offset").map(|v| v.to_string()),
                ),
            ]);
            client.get(&format!("/approvals{qs}")).await
        }

        "postblox_get_approval" => {
            let id = require_str(&args, "approval_id")?;
            client.get(&format!("/approvals/{id}")).await
        }

        "postblox_approve_message" => {
            let id = require_str(&args, "approval_id")?;
            let body = json!({ "decided_by": require_str(&args, "decided_by")? });
            client.post(&format!("/approvals/{id}/approve"), body).await
        }

        "postblox_reject_message" => {
            let id = require_str(&args, "approval_id")?;
            let body = json!({ "decided_by": require_str(&args, "decided_by")? });
            client.post(&format!("/approvals/{id}/reject"), body).await
        }

        "postblox_batch_approvals" => {
            let body = json!({
                "ids": args.get("ids").cloned().unwrap_or(json!([])),
                "status": require_str(&args, "status")?,
                "decided_by": require_str(&args, "decided_by")?,
            });
            client.post("/approvals/batch", body).await
        }

        "postblox_get_permissions" => {
            let inbox_id = require_str(&args, "inbox_id")?;
            client
                .get(&format!("/inboxes/{inbox_id}/permissions"))
                .await
        }

        "postblox_set_permissions" => {
            let inbox_id = require_str(&args, "inbox_id")?;
            let mut body = json!({});
            if let Some(sm) = optional_str(&args, "send_mode") {
                body["send_mode"] = json!(sm);
            }
            if let Some(rules) = args.get("rules").cloned() {
                body["rules"] = rules;
            }
            client
                .put(&format!("/inboxes/{inbox_id}/permissions"), body)
                .await
        }

        "postblox_get_trust" => {
            let inbox_id = require_str(&args, "inbox_id")?;
            client.get(&format!("/inboxes/{inbox_id}/trust")).await
        }

        "postblox_list_audit" => {
            let qs = build_query_string(&[
                ("limit", optional_i64(&args, "limit").map(|v| v.to_string())),
                (
                    "offset",
                    optional_i64(&args, "offset").map(|v| v.to_string()),
                ),
                ("inbox_id", optional_str(&args, "inbox_id")),
                ("action", optional_str(&args, "action")),
                ("after", optional_str(&args, "after")),
                ("before", optional_str(&args, "before")),
            ]);
            client.get(&format!("/audit{qs}")).await
        }

        "postblox_list_notifications" => client.get("/notifications").await,

        "postblox_create_notification" => {
            let body = json!({
                "provider": require_str(&args, "provider")?,
                "config": args.get("config").cloned().unwrap_or(json!({})),
            });
            client.post("/notifications", body).await
        }

        "postblox_delete_notification" => {
            let id = require_str(&args, "notification_id")?;
            client.delete(&format!("/notifications/{id}")).await
        }

        "postblox_reply" => {
            let inbox_id = require_str(&args, "inbox_id")?;
            let message_id = require_str(&args, "message_id")?;
            let text_body = require_str(&args, "text_body")?;

            // Fetch original message to get sender and subject
            let orig_json = client
                .get(&format!("/inboxes/{inbox_id}/messages/{message_id}"))
                .await?;
            let orig: Value = serde_json::from_str(&orig_json)
                .map_err(|e| McpError::Api(format!("failed to parse message: {e}")))?;

            let reply_to = orig["from_addr"].as_str().unwrap_or("").to_string();
            let subject = orig["subject"]
                .as_str()
                .map(|s| {
                    if s.starts_with("Re: ") || s.starts_with("re: ") {
                        s.to_string()
                    } else {
                        format!("Re: {s}")
                    }
                })
                .unwrap_or_else(|| "Re: ".to_string());

            // Create draft with in_reply_to
            let mut draft_body = json!({
                "to": [reply_to],
                "subject": subject,
                "text_body": text_body,
                "in_reply_to_message_id": message_id,
            });
            if let Some(hb) = optional_str(&args, "html_body") {
                draft_body["html_body"] = json!(hb);
            }
            if let Some(cc) = args.get("cc").cloned() {
                draft_body["cc"] = cc;
            }
            let draft_json = client
                .post(&format!("/inboxes/{inbox_id}/drafts"), draft_body)
                .await?;
            let draft: Value = serde_json::from_str(&draft_json)
                .map_err(|e| McpError::Api(format!("failed to parse draft: {e}")))?;
            let draft_id = draft["id"]
                .as_str()
                .ok_or_else(|| McpError::Api("draft missing id".into()))?;

            // Send the draft
            client
                .post(
                    &format!("/inboxes/{inbox_id}/drafts/{draft_id}/send"),
                    json!({}),
                )
                .await
        }

        "postblox_link_inbox" => {
            let mut body = json!({
                "inbox_id": require_str(&args, "inbox_id")?,
                "imap_host": require_str(&args, "imap_host")?,
                "username": require_str(&args, "username")?,
                "password": require_str(&args, "password")?,
            });
            if let Some(port) = optional_i64(&args, "imap_port") {
                body["imap_port"] = json!(port);
            }
            client.post("/linked-accounts", body).await
        }

        "postblox_list_linked_inboxes" => client.get("/linked-accounts").await,

        "postblox_sync_linked_inbox" => {
            let id = require_str(&args, "linked_account_id")?;
            client
                .post(&format!("/linked-accounts/{id}/sync"), json!({}))
                .await
        }

        _ => Err(McpError::UnknownTool(name.into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_definitions_all_have_required_fields() {
        let defs = tool_definitions();
        assert!(!defs.is_empty());
        for def in &defs {
            assert!(def.get("name").and_then(|v| v.as_str()).is_some());
            assert!(def.get("description").and_then(|v| v.as_str()).is_some());
            let schema = def.get("inputSchema").unwrap();
            assert_eq!(schema["type"], "object");
            assert!(schema.get("properties").is_some());
            assert!(schema.get("required").is_some());
        }
    }

    #[test]
    fn test_tool_definitions_unique_names() {
        let defs = tool_definitions();
        let names: Vec<&str> = defs.iter().map(|d| d["name"].as_str().unwrap()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(names.len(), sorted.len(), "duplicate tool names found");
    }

    #[test]
    fn test_tool_definitions_count() {
        let defs = tool_definitions();
        assert_eq!(defs.len(), 44);
    }

    #[test]
    fn test_require_str_present() {
        let args = json!({"email": "bot@x.com"});
        assert_eq!(require_str(&args, "email").unwrap(), "bot@x.com");
    }

    #[test]
    fn test_require_str_missing_returns_error() {
        let args = json!({});
        let err = require_str(&args, "email").unwrap_err();
        assert!(err.to_string().contains("email"));
    }

    #[test]
    fn test_require_str_null_returns_error() {
        let args = json!({"email": null});
        assert!(require_str(&args, "email").is_err());
    }

    #[test]
    fn test_optional_str_present() {
        let args = json!({"name": "test"});
        assert_eq!(optional_str(&args, "name"), Some("test".into()));
    }

    #[test]
    fn test_optional_str_missing() {
        let args = json!({});
        assert_eq!(optional_str(&args, "name"), None);
    }

    #[test]
    fn test_optional_i64_present() {
        let args = json!({"limit": 10});
        assert_eq!(optional_i64(&args, "limit"), Some(10));
    }

    #[test]
    fn test_optional_i64_missing() {
        let args = json!({});
        assert_eq!(optional_i64(&args, "limit"), None);
    }

    #[test]
    fn test_build_query_string_empty() {
        let qs = build_query_string(&[("a", None), ("b", None)]);
        assert_eq!(qs, "");
    }

    #[test]
    fn test_build_query_string_single_param() {
        let qs = build_query_string(&[("q", Some("hello".into()))]);
        assert_eq!(qs, "?q=hello");
    }

    #[test]
    fn test_build_query_string_multiple_params() {
        let qs = build_query_string(&[
            ("q", Some("hello".into())),
            ("limit", Some("10".into())),
            ("missing", None),
        ]);
        assert_eq!(qs, "?q=hello&limit=10");
    }

    #[test]
    fn test_build_query_string_encodes_special_chars() {
        let qs = build_query_string(&[("q", Some("hello world&foo=bar".into()))]);
        assert_eq!(qs, "?q=hello%20world%26foo%3Dbar");
    }

    #[test]
    fn test_url_encode_unreserved_chars_unchanged() {
        assert_eq!(url_encode("abc-_.~123"), "abc-_.~123");
    }

    #[tokio::test]
    async fn test_dispatch_unknown_tool_returns_error() {
        let client = PostbloxClient::new("http://localhost:1".into(), "key".into());
        let err = dispatch(&client, "nonexistent_tool", json!({}))
            .await
            .unwrap_err();
        assert!(matches!(err, McpError::UnknownTool(_)));
    }

    #[tokio::test]
    async fn test_dispatch_missing_required_arg() {
        let client = PostbloxClient::new("http://localhost:1".into(), "key".into());
        let err = dispatch(&client, "postblox_send_email", json!({}))
            .await
            .unwrap_err();
        assert!(matches!(err, McpError::MissingArgument(_)));
    }

    #[test]
    fn test_tool_definitions_required_fields_exist_in_properties() {
        let defs = tool_definitions();
        for def in &defs {
            let props = def["inputSchema"]["properties"].as_object().unwrap();
            let required = def["inputSchema"]["required"].as_array().unwrap();
            for req in required {
                let name = req.as_str().unwrap();
                assert!(
                    props.contains_key(name),
                    "tool {} requires '{}' but it's not in properties",
                    def["name"],
                    name
                );
            }
        }
    }
}
