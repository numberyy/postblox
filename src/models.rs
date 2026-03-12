#![allow(dead_code)] // Create* structs used only in tests until Phase 4

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Organization {
    pub id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateOrganization {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct ApiKey {
    pub id: Uuid,
    pub org_id: Uuid,
    pub key_hash: String,
    pub prefix: String,
    pub name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateApiKey {
    pub org_id: Uuid,
    pub key_hash: String,
    pub prefix: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Inbox {
    pub id: Uuid,
    pub org_id: Uuid,
    pub email: String,
    pub display_name: Option<String>,
    pub inbox_type: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateInbox {
    pub org_id: Uuid,
    pub email: String,
    pub display_name: Option<String>,
    pub inbox_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Thread {
    pub id: Uuid,
    pub inbox_id: Uuid,
    pub subject: Option<String>,
    pub message_count: i32,
    pub last_message_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Message {
    pub id: Uuid,
    pub inbox_id: Uuid,
    pub thread_id: Option<Uuid>,
    pub message_id_header: Option<String>,
    pub in_reply_to: Option<String>,
    pub references_header: Option<String>,
    pub from_addr: String,
    pub to_addrs: serde_json::Value,
    pub cc_addrs: Option<serde_json::Value>,
    pub subject: Option<String>,
    pub text_body: Option<String>,
    pub html_body: Option<String>,
    pub extracted_text: Option<String>,
    pub direction: String,
    pub raw_headers: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    #[sqlx(default)]
    pub slop_score: Option<f32>,
    #[sqlx(default)]
    pub slop_signals: Option<serde_json::Value>,
    #[sqlx(default)]
    pub category: Option<String>,
    #[sqlx(default)]
    pub priority: Option<String>,
    #[sqlx(default)]
    pub triage_status: Option<String>,
    #[sqlx(default)]
    pub requires_action: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateMessage {
    pub inbox_id: Uuid,
    pub thread_id: Option<Uuid>,
    pub message_id_header: Option<String>,
    pub in_reply_to: Option<String>,
    pub references_header: Option<String>,
    pub from_addr: String,
    pub to_addrs: serde_json::Value,
    pub cc_addrs: Option<serde_json::Value>,
    pub subject: Option<String>,
    pub text_body: Option<String>,
    pub html_body: Option<String>,
    pub extracted_text: Option<String>,
    pub direction: String,
    pub raw_headers: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Webhook {
    pub id: Uuid,
    pub org_id: Uuid,
    pub url: String,
    pub events: serde_json::Value,
    pub secret: String,
    pub active: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateWebhook {
    pub org_id: Uuid,
    pub url: String,
    pub events: serde_json::Value,
    pub secret: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Label {
    pub id: Uuid,
    pub inbox_id: Uuid,
    pub name: String,
    pub color: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Draft {
    pub id: Uuid,
    pub inbox_id: Uuid,
    pub to_addrs: serde_json::Value,
    pub cc_addrs: Option<serde_json::Value>,
    pub subject: Option<String>,
    pub text_body: Option<String>,
    pub html_body: Option<String>,
    pub in_reply_to_message_id: Option<Uuid>,
    pub updated_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateDraft {
    pub inbox_id: Uuid,
    pub to_addrs: serde_json::Value,
    pub cc_addrs: Option<serde_json::Value>,
    pub subject: Option<String>,
    pub text_body: Option<String>,
    pub html_body: Option<String>,
    pub in_reply_to_message_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Domain {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub status: String,
    pub stalwart_principal_id: Option<String>,
    pub verified_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct LinkedAccount {
    pub id: Uuid,
    pub inbox_id: Uuid,
    pub org_id: Uuid,
    pub provider: String,
    pub imap_host: String,
    pub imap_port: i32,
    pub username: String,
    pub password: String,
    pub last_sync_at: Option<DateTime<Utc>>,
    pub sync_status: String,
    pub message_count: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateLinkedAccount {
    pub inbox_id: Uuid,
    pub org_id: Uuid,
    pub imap_host: String,
    pub imap_port: Option<i32>,
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct SenderReputation {
    pub id: Uuid,
    pub org_id: Uuid,
    pub sender_email: String,
    pub total_messages: i32,
    pub slop_count: i32,
    pub last_seen_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

impl SenderReputation {
    pub fn slop_ratio(&self) -> f32 {
        if self.total_messages > 0 {
            self.slop_count as f32 / self.total_messages as f32
        } else {
            0.0
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct SlopFeedback {
    pub id: Uuid,
    pub org_id: Uuid,
    pub message_id: Uuid,
    pub is_slop: bool,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_organization_serialization_roundtrip() {
        let org = Organization {
            id: Uuid::new_v4(),
            name: "Acme Corp".into(),
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&org).unwrap();
        let back: Organization = serde_json::from_str(&json).unwrap();
        assert_eq!(org, back);
    }

    #[test]
    fn test_api_key_serialization_roundtrip() {
        let key = ApiKey {
            id: Uuid::new_v4(),
            org_id: Uuid::new_v4(),
            key_hash: "hash123".into(),
            prefix: "pb_abcd".into(),
            name: Some("prod key".into()),
            created_at: Utc::now(),
            last_used_at: None,
        };
        let json = serde_json::to_string(&key).unwrap();
        let back: ApiKey = serde_json::from_str(&json).unwrap();
        assert_eq!(key, back);
    }

    #[test]
    fn test_api_key_nullable_fields() {
        let json = serde_json::json!({
            "id": Uuid::new_v4(),
            "org_id": Uuid::new_v4(),
            "key_hash": "h",
            "prefix": "pb_1234",
            "name": null,
            "created_at": "2026-03-12T00:00:00Z",
            "last_used_at": null
        });
        let key: ApiKey = serde_json::from_value(json).unwrap();
        assert!(key.name.is_none());
        assert!(key.last_used_at.is_none());
    }

    #[test]
    fn test_inbox_serialization_roundtrip() {
        let inbox = Inbox {
            id: Uuid::new_v4(),
            org_id: Uuid::new_v4(),
            email: "bot@example.com".into(),
            display_name: Some("Bot".into()),
            inbox_type: "native".into(),
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&inbox).unwrap();
        let back: Inbox = serde_json::from_str(&json).unwrap();
        assert_eq!(inbox, back);
    }

    #[test]
    fn test_thread_serialization_roundtrip() {
        let thread = Thread {
            id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
            subject: Some("Hello".into()),
            message_count: 5,
            last_message_at: Some(Utc::now()),
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&thread).unwrap();
        let back: Thread = serde_json::from_str(&json).unwrap();
        assert_eq!(thread, back);
    }

    #[test]
    fn test_thread_defaults_in_deserialization() {
        let json = serde_json::json!({
            "id": Uuid::new_v4(),
            "inbox_id": Uuid::new_v4(),
            "subject": null,
            "message_count": 0,
            "last_message_at": null,
            "created_at": "2026-03-12T00:00:00Z"
        });
        let thread: Thread = serde_json::from_value(json).unwrap();
        assert_eq!(thread.message_count, 0);
        assert!(thread.subject.is_none());
        assert!(thread.last_message_at.is_none());
    }

    #[test]
    fn test_message_serialization_roundtrip() {
        let msg = Message {
            id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
            thread_id: None,
            message_id_header: Some("<abc@example.com>".into()),
            in_reply_to: None,
            references_header: None,
            from_addr: "sender@example.com".into(),
            to_addrs: serde_json::json!(["rcpt@example.com"]),
            cc_addrs: Some(serde_json::json!([])),
            subject: Some("Test".into()),
            text_body: Some("Hello".into()),
            html_body: None,
            extracted_text: None,
            direction: "inbound".into(),
            raw_headers: None,
            created_at: Utc::now(),
            slop_score: None,
            slop_signals: None,
            category: None,
            priority: None,
            triage_status: None,
            requires_action: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, back);
    }

    #[test]
    fn test_message_jsonb_fields_various_shapes() {
        let msg = Message {
            id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
            thread_id: None,
            message_id_header: None,
            in_reply_to: None,
            references_header: None,
            from_addr: "a@b.com".into(),
            to_addrs: serde_json::json!(["a@b.com", "c@d.com", "e@f.com"]),
            cc_addrs: None,
            subject: None,
            text_body: None,
            html_body: None,
            extracted_text: None,
            direction: "outbound".into(),
            raw_headers: Some(serde_json::json!({"X-Custom": "val", "X-Other": "val2"})),
            created_at: Utc::now(),
            slop_score: None,
            slop_signals: None,
            category: None,
            priority: None,
            triage_status: None,
            requires_action: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back.to_addrs.as_array().unwrap().len(), 3);
        assert!(back.cc_addrs.is_none());
        assert!(back.raw_headers.is_some());
    }

    #[test]
    fn test_webhook_serialization_roundtrip() {
        let wh = Webhook {
            id: Uuid::new_v4(),
            org_id: Uuid::new_v4(),
            url: "https://example.com/hook".into(),
            events: serde_json::json!(["message.inbound", "message.outbound"]),
            secret: "whsec_abc123".into(),
            active: true,
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&wh).unwrap();
        let back: Webhook = serde_json::from_str(&json).unwrap();
        assert_eq!(wh, back);
    }

    #[test]
    fn test_create_organization_deserialize() {
        let json = r#"{"name": "Test Org"}"#;
        let input: CreateOrganization = serde_json::from_str(json).unwrap();
        assert_eq!(input.name, "Test Org");
    }

    #[test]
    fn test_create_inbox_optional_type() {
        let json = r#"{"org_id": "00000000-0000-0000-0000-000000000001", "email": "bot@x.com"}"#;
        let input: CreateInbox = serde_json::from_str(json).unwrap();
        assert_eq!(input.email, "bot@x.com");
        assert!(input.inbox_type.is_none());
        assert!(input.display_name.is_none());
    }

    #[test]
    fn test_create_message_serialize() {
        let cm = CreateMessage {
            inbox_id: Uuid::new_v4(),
            thread_id: None,
            message_id_header: Some("<msg1@example.com>".into()),
            in_reply_to: None,
            references_header: None,
            from_addr: "bot@example.com".into(),
            to_addrs: serde_json::json!(["user@example.com"]),
            cc_addrs: None,
            subject: Some("Hi".into()),
            text_body: Some("Body".into()),
            html_body: None,
            extracted_text: None,
            direction: "outbound".into(),
            raw_headers: None,
        };
        let json = serde_json::to_value(&cm).unwrap();
        assert_eq!(json["direction"], "outbound");
        assert_eq!(json["to_addrs"][0], "user@example.com");
    }

    #[test]
    fn test_create_webhook_events_empty_array() {
        let wh = CreateWebhook {
            org_id: Uuid::new_v4(),
            url: "https://example.com".into(),
            events: serde_json::json!([]),
            secret: "secret".into(),
        };
        let json = serde_json::to_string(&wh).unwrap();
        let back: CreateWebhook = serde_json::from_str(&json).unwrap();
        assert_eq!(back.events.as_array().unwrap().len(), 0);
    }

    #[test]
    fn test_label_serialization_roundtrip() {
        let label = Label {
            id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
            name: "important".into(),
            color: Some("#ff0000".into()),
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&label).unwrap();
        let back: Label = serde_json::from_str(&json).unwrap();
        assert_eq!(label, back);
    }

    #[test]
    fn test_draft_serialization_roundtrip() {
        let draft = Draft {
            id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
            to_addrs: serde_json::json!(["a@b.com"]),
            cc_addrs: Some(serde_json::json!(["c@d.com"])),
            subject: Some("Draft subject".into()),
            text_body: Some("Hello".into()),
            html_body: None,
            in_reply_to_message_id: None,
            updated_at: Utc::now(),
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&draft).unwrap();
        let back: Draft = serde_json::from_str(&json).unwrap();
        assert_eq!(draft, back);
    }

    #[test]
    fn test_create_draft_optional_fields() {
        let json = serde_json::json!({
            "inbox_id": Uuid::new_v4(),
            "to_addrs": []
        });
        let cd: CreateDraft = serde_json::from_value(json).unwrap();
        assert!(cd.subject.is_none());
        assert!(cd.text_body.is_none());
        assert!(cd.html_body.is_none());
        assert!(cd.cc_addrs.is_none());
        assert!(cd.in_reply_to_message_id.is_none());
    }

    #[test]
    fn test_domain_serialization_roundtrip() {
        let domain = Domain {
            id: Uuid::new_v4(),
            org_id: Uuid::new_v4(),
            name: "example.com".into(),
            status: "pending".into(),
            stalwart_principal_id: None,
            verified_at: None,
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&domain).unwrap();
        let back: Domain = serde_json::from_str(&json).unwrap();
        assert_eq!(domain, back);
    }

    #[test]
    fn test_domain_nullable_fields() {
        let json = serde_json::json!({
            "id": Uuid::new_v4(),
            "org_id": Uuid::new_v4(),
            "name": "test.com",
            "status": "verified",
            "stalwart_principal_id": "principal-123",
            "verified_at": "2026-03-12T00:00:00Z",
            "created_at": "2026-03-12T00:00:00Z"
        });
        let domain: Domain = serde_json::from_value(json).unwrap();
        assert_eq!(
            domain.stalwart_principal_id.as_deref(),
            Some("principal-123")
        );
        assert!(domain.verified_at.is_some());
    }

    #[test]
    fn test_linked_account_serialization_roundtrip() {
        let acct = LinkedAccount {
            id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
            org_id: Uuid::new_v4(),
            provider: "imap".into(),
            imap_host: "imap.gmail.com".into(),
            imap_port: 993,
            username: "user@gmail.com".into(),
            password: "enc_secret".into(),
            last_sync_at: Some(Utc::now()),
            sync_status: "idle".into(),
            message_count: 42,
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&acct).unwrap();
        let back: LinkedAccount = serde_json::from_str(&json).unwrap();
        assert_eq!(acct, back);
    }

    #[test]
    fn test_linked_account_nullable_fields() {
        let json = serde_json::json!({
            "id": Uuid::new_v4(),
            "inbox_id": Uuid::new_v4(),
            "org_id": Uuid::new_v4(),
            "provider": "imap",
            "imap_host": "imap.example.com",
            "imap_port": 993,
            "username": "user",
            "password": "pw",
            "last_sync_at": null,
            "sync_status": "idle",
            "message_count": 0,
            "created_at": "2026-03-12T00:00:00Z"
        });
        let acct: LinkedAccount = serde_json::from_value(json).unwrap();
        assert!(acct.last_sync_at.is_none());
        assert_eq!(acct.message_count, 0);
    }

    #[test]
    fn test_create_linked_account_serialization_roundtrip() {
        let create = CreateLinkedAccount {
            inbox_id: Uuid::new_v4(),
            org_id: Uuid::new_v4(),
            imap_host: "imap.outlook.com".into(),
            imap_port: Some(993),
            username: "user@outlook.com".into(),
            password: "enc_pass".into(),
        };
        let json = serde_json::to_string(&create).unwrap();
        let back: CreateLinkedAccount = serde_json::from_str(&json).unwrap();
        assert_eq!(create, back);
    }

    #[test]
    fn test_create_linked_account_optional_fields() {
        let json = serde_json::json!({
            "inbox_id": Uuid::new_v4(),
            "org_id": Uuid::new_v4(),
            "imap_host": "imap.example.com",
            "username": "user",
            "password": "pw"
        });
        let create: CreateLinkedAccount = serde_json::from_value(json).unwrap();
        assert!(create.imap_port.is_none());
    }

    #[test]
    fn test_message_unicode_fields() {
        let msg = Message {
            id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
            thread_id: None,
            message_id_header: None,
            in_reply_to: None,
            references_header: None,
            from_addr: "用户@example.com".into(),
            to_addrs: serde_json::json!(["пользователь@example.com"]),
            cc_addrs: None,
            subject: Some("日本語のメール".into()),
            text_body: Some("مرحبا بالعالم".into()),
            html_body: None,
            extracted_text: None,
            direction: "inbound".into(),
            raw_headers: None,
            created_at: Utc::now(),
            slop_score: None,
            slop_signals: None,
            category: None,
            priority: None,
            triage_status: None,
            requires_action: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, back);
    }
}
