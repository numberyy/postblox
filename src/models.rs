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
// Strongly-typed entity IDs
//
// Each of these is a `#[repr(transparent)]` newtype around `uuid::Uuid` so the
// type system can prevent accidentally passing e.g. a `FolderId` where an
// `AccountId` is expected. Serde is `transparent` so the wire format and JSON
// representation stay identical to a bare UUID string.
// -----------------------------------------------------------------------------

macro_rules! entity_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        #[repr(transparent)]
        pub struct $name(pub Uuid);

        #[allow(clippy::new_without_default)]
        impl $name {
            #[inline]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            #[inline]
            pub fn into_inner(self) -> Uuid {
                self.0
            }

            #[inline]
            pub fn as_uuid(&self) -> &Uuid {
                &self.0
            }
        }

        impl From<Uuid> for $name {
            #[inline]
            fn from(value: Uuid) -> Self {
                Self(value)
            }
        }

        impl From<$name> for Uuid {
            #[inline]
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::from_str(s).map(Self)
            }
        }

        impl sqlx::Type<sqlx::Sqlite> for $name {
            fn type_info() -> sqlx::sqlite::SqliteTypeInfo {
                <Uuid as sqlx::Type<sqlx::Sqlite>>::type_info()
            }

            fn compatible(ty: &sqlx::sqlite::SqliteTypeInfo) -> bool {
                <Uuid as sqlx::Type<sqlx::Sqlite>>::compatible(ty)
            }
        }

        impl<'r> sqlx::Decode<'r, sqlx::Sqlite> for $name {
            fn decode(
                value: sqlx::sqlite::SqliteValueRef<'r>,
            ) -> Result<Self, sqlx::error::BoxDynError> {
                <Uuid as sqlx::Decode<sqlx::Sqlite>>::decode(value).map(Self)
            }
        }

        impl<'q> sqlx::Encode<'q, sqlx::Sqlite> for $name {
            fn encode_by_ref(
                &self,
                buf: &mut <sqlx::Sqlite as sqlx::Database>::ArgumentBuffer<'q>,
            ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
                <Uuid as sqlx::Encode<sqlx::Sqlite>>::encode_by_ref(&self.0, buf)
            }
        }
    };
}

entity_id!(
    /// Identifier for an Account row.
    AccountId
);
entity_id!(
    /// Identifier for a Folder row.
    FolderId
);
entity_id!(
    /// Identifier for a Thread row.
    ThreadId
);
entity_id!(
    /// Identifier for a Message row.
    MessageId
);
entity_id!(
    /// Identifier for a Draft row.
    DraftId
);
entity_id!(
    /// Identifier for an Attachment row.
    AttachmentId
);

// -----------------------------------------------------------------------------
// Enums + helpers for SQLite TEXT columns
// -----------------------------------------------------------------------------

/// Error returned when parsing a `text_enum!`-generated enum from a string
/// fails. `kind` is the enum's type name (e.g. `"AuthKind"`) and `value` is
/// the unrecognised input.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid {kind}: {value}")]
pub struct ParseEnumError {
    pub kind: &'static str,
    pub value: String,
}

// Allow lossy conversion into `String` so existing call sites that propagate
// the parse error into `impl Into<String>` (e.g. `RpcError::bad_args`) keep
// compiling without forcing every caller through `.to_string()`.
impl From<ParseEnumError> for String {
    fn from(err: ParseEnumError) -> Self {
        err.to_string()
    }
}

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
            type Err = ParseEnumError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($repr => Ok(Self::$variant),)+
                    other => Err(ParseEnumError {
                        kind: stringify!($name),
                        value: other.to_string(),
                    }),
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
    pub id: AccountId,
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
    pub id: FolderId,
    pub account_id: AccountId,
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
    pub id: ThreadId,
    pub account_id: AccountId,
    pub external_id: Option<String>,
    pub subject: Option<String>,
    pub last_message_at: Option<DateTime<Utc>>,
    pub message_count: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Message {
    pub id: MessageId,
    pub account_id: AccountId,
    pub folder_id: FolderId,
    pub thread_id: Option<ThreadId>,
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
    pub id: AttachmentId,
    pub message_id: MessageId,
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
    pub id: DraftId,
    pub account_id: AccountId,
    pub in_reply_to_msg: Option<MessageId>,
    pub to_addrs: serde_json::Value,
    pub cc_addrs: serde_json::Value,
    pub bcc_addrs: serde_json::Value,
    pub subject: Option<String>,
    pub text_body: Option<String>,
    pub html_body: Option<String>,
    pub in_reply_to: Option<String>,
    pub references_header: Option<String>,
    pub remote_folder_id: Option<FolderId>,
    pub remote_uid: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct DraftAttachment {
    pub id: Uuid,
    pub draft_id: DraftId,
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

    #[test]
    fn test_parse_enum_error_carries_kind_and_value() {
        let err = AuthKind::from_str("garbage").unwrap_err();
        assert_eq!(err.kind, "AuthKind");
        assert_eq!(err.value, "garbage");

        let err = FolderRole::from_str("inboxxx").unwrap_err();
        assert_eq!(err.kind, "FolderRole");
        assert_eq!(err.value, "inboxxx");
    }

    #[test]
    fn test_account_id_round_trips_via_serde_as_uuid_string() {
        let id = AccountId::new();
        let value = serde_json::to_value(id).unwrap();
        assert!(
            matches!(value, serde_json::Value::String(_)),
            "expected JSON string, got {value:?}"
        );
        let decoded: AccountId = serde_json::from_value(value).unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn test_account_id_from_str_rejects_non_uuid() {
        assert!(AccountId::from_str("not-a-uuid").is_err());
    }

    #[tokio::test]
    async fn test_account_id_sqlx_round_trip() {
        let pool = crate::db::test_pool().await;
        let id = AccountId::new();
        let decoded: AccountId = sqlx::query_scalar("SELECT ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn test_account_id_and_folder_id_have_distinct_types() {
        use std::any::TypeId;
        assert_ne!(TypeId::of::<AccountId>(), TypeId::of::<FolderId>());
    }

    #[test]
    fn test_message_id_display_round_trips_through_from_str() {
        let id = MessageId::new();
        let s = id.to_string();
        let parsed = MessageId::from_str(&s).unwrap();
        assert_eq!(parsed, id);
    }
}
