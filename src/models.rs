//! Domain types — one type per table row.
//!
//! These are the contract between `db::*` and the rest of the crate.
//! Nothing here knows about IMAP, SMTP, the daemon, or the TUI.

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// -----------------------------------------------------------------------------
// Enums + helpers for SQLite TEXT columns
// -----------------------------------------------------------------------------

macro_rules! text_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $($variant:ident => $repr:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        $vis enum $name {
            $($variant),+
        }

        impl $name {
            pub fn as_str(&self) -> &'static str {
                match self {
                    $(Self::$variant => $repr),+
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = String;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($repr => Ok(Self::$variant),)+
                    other => Err(format!(concat!("invalid ", stringify!($name), ": {}"), other)),
                }
            }
        }

        impl sqlx::Type<sqlx::Sqlite> for $name {
            fn type_info() -> sqlx::sqlite::SqliteTypeInfo {
                <String as sqlx::Type<sqlx::Sqlite>>::type_info()
            }
        }

        impl<'r> sqlx::Decode<'r, sqlx::Sqlite> for $name {
            fn decode(value: sqlx::sqlite::SqliteValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
                let s = <String as sqlx::Decode<sqlx::Sqlite>>::decode(value)?;
                s.parse::<Self>().map_err(|e| -> sqlx::error::BoxDynError { e.into() })
            }
        }

        impl<'q> sqlx::Encode<'q, sqlx::Sqlite> for $name {
            fn encode_by_ref(
                &self,
                buf: &mut <sqlx::Sqlite as sqlx::Database>::ArgumentBuffer<'q>,
            ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
                <String as sqlx::Encode<sqlx::Sqlite>>::encode(self.as_str().to_string(), buf)
            }
        }
    };
}

text_enum! {
    pub enum AuthKind {
        Password     => "password",
        OAuth2Google => "oauth2_google",
    }
}

text_enum! {
    pub enum SyncStatus {
        Idle    => "idle",
        Syncing => "syncing",
        Error   => "error",
    }
}

text_enum! {
    pub enum FolderRole {
        Inbox   => "inbox",
        Sent    => "sent",
        Drafts  => "drafts",
        Archive => "archive",
        Trash   => "trash",
        Spam    => "spam",
        All     => "all",
        Starred => "starred",
        Custom  => "custom",
    }
}

text_enum! {
    pub enum AttachmentDisposition {
        Inline     => "inline",
        Attachment => "attachment",
    }
}

text_enum! {
    pub enum GateAction {
        AutoAllow => "auto_allow",
        Require   => "require",
        Deny      => "deny",
    }
}

text_enum! {
    pub enum ApprovalState {
        Pending => "pending",
        Allowed => "allowed",
        Denied  => "denied",
        Expired => "expired",
    }
}

// -----------------------------------------------------------------------------
// Row types
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Account {
    pub id: Uuid,
    pub email: String,
    pub display_name: Option<String>,
    pub auth_kind: AuthKind,
    pub imap_host: String,
    pub imap_port: i64,
    pub imap_use_tls: bool,
    pub smtp_host: String,
    pub smtp_port: i64,
    pub smtp_use_tls: bool,
    pub smtp_starttls: bool,
    pub secret_ref: Option<String>,
    pub last_synced_at: Option<DateTime<Utc>>,
    pub sync_status: SyncStatus,
    pub sync_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Folder {
    pub id: Uuid,
    pub account_id: Uuid,
    pub name: String,
    pub delimiter: String,
    pub role: FolderRole,
    pub uid_validity: Option<i64>,
    pub uid_next: Option<i64>,
    pub last_seen_uid: Option<i64>,
    pub selectable: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Thread {
    pub id: Uuid,
    pub account_id: Uuid,
    pub external_id: Option<String>,
    pub subject: Option<String>,
    pub last_message_at: Option<DateTime<Utc>>,
    pub message_count: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Message {
    pub id: Uuid,
    pub account_id: Uuid,
    pub folder_id: Uuid,
    pub thread_id: Option<Uuid>,
    pub uid: i64,
    pub message_id_header: Option<String>,
    pub in_reply_to: Option<String>,
    pub references_header: Option<String>,
    pub from_addr: String,
    pub to_addrs: serde_json::Value,
    pub cc_addrs: serde_json::Value,
    pub bcc_addrs: serde_json::Value,
    pub reply_to: Option<String>,
    pub subject: Option<String>,
    pub snippet: Option<String>,
    pub text_body: Option<String>,
    pub html_body: Option<String>,
    pub raw_size: i64,
    pub flags: serde_json::Value,
    pub internal_date: DateTime<Utc>,
    pub sent_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Attachment {
    pub id: Uuid,
    pub message_id: Uuid,
    pub filename: String,
    pub content_type: String,
    pub content_id: Option<String>,
    pub size_bytes: i64,
    pub disposition: AttachmentDisposition,
    pub storage_path: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Draft {
    pub id: Uuid,
    pub account_id: Uuid,
    pub in_reply_to_msg: Option<Uuid>,
    pub to_addrs: serde_json::Value,
    pub cc_addrs: serde_json::Value,
    pub bcc_addrs: serde_json::Value,
    pub subject: Option<String>,
    pub text_body: Option<String>,
    pub html_body: Option<String>,
    pub in_reply_to: Option<String>,
    pub references_header: Option<String>,
    pub remote_folder_id: Option<Uuid>,
    pub remote_uid: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct DraftAttachment {
    pub id: Uuid,
    pub draft_id: Uuid,
    pub filename: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct McpGate {
    pub id: Uuid,
    pub tool: String,
    pub arg_pattern: Option<String>,
    pub action: GateAction,
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct McpApproval {
    pub id: Uuid,
    pub tool: String,
    pub args: serde_json::Value,
    pub summary: String,
    pub state: ApprovalState,
    pub decided_at: Option<DateTime<Utc>>,
    pub decided_by: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct AuditEntry {
    pub id: Uuid,
    pub actor: String,
    pub action: String,
    pub target: Option<String>,
    pub details: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

// -----------------------------------------------------------------------------
// Tests for enum round-trips. The sqlx encode/decode paths are exercised in
// db tests against a real SQLite file.
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_kind_round_trip() {
        for v in [AuthKind::Password, AuthKind::OAuth2Google] {
            assert_eq!(v.to_string().parse::<AuthKind>().unwrap(), v);
        }
    }

    #[test]
    fn test_folder_role_round_trip() {
        for v in [
            FolderRole::Inbox,
            FolderRole::Sent,
            FolderRole::Drafts,
            FolderRole::Archive,
            FolderRole::Trash,
            FolderRole::Spam,
            FolderRole::All,
            FolderRole::Starred,
            FolderRole::Custom,
        ] {
            assert_eq!(v.to_string().parse::<FolderRole>().unwrap(), v);
        }
    }

    #[test]
    fn test_sync_status_round_trip() {
        for v in [SyncStatus::Idle, SyncStatus::Syncing, SyncStatus::Error] {
            assert_eq!(v.to_string().parse::<SyncStatus>().unwrap(), v);
        }
    }

    #[test]
    fn test_attachment_disposition_round_trip() {
        for v in [
            AttachmentDisposition::Inline,
            AttachmentDisposition::Attachment,
        ] {
            assert_eq!(v.to_string().parse::<AttachmentDisposition>().unwrap(), v);
        }
    }

    #[test]
    fn test_gate_action_round_trip() {
        for v in [GateAction::AutoAllow, GateAction::Require, GateAction::Deny] {
            assert_eq!(v.to_string().parse::<GateAction>().unwrap(), v);
        }
    }

    #[test]
    fn test_approval_state_round_trip() {
        for v in [
            ApprovalState::Pending,
            ApprovalState::Allowed,
            ApprovalState::Denied,
            ApprovalState::Expired,
        ] {
            assert_eq!(v.to_string().parse::<ApprovalState>().unwrap(), v);
        }
    }

    #[test]
    fn test_invalid_enum_parse_returns_error() {
        assert!("garbage".parse::<AuthKind>().is_err());
        assert!("inboxxx".parse::<FolderRole>().is_err());
        assert!("".parse::<SyncStatus>().is_err());
    }
}
