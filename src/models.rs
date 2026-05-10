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
            /// Construct a fresh, randomly-generated identifier.
            #[inline]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            /// Consume the newtype and return the inner [`Uuid`].
            #[inline]
            pub fn into_inner(self) -> Uuid {
                self.0
            }

            /// Borrow the inner [`Uuid`] without taking ownership.
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
    /// Static name of the enum that rejected the input (e.g. `"AuthKind"`).
    pub kind: &'static str,
    /// Offending input string that did not match any variant.
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
            $($(#[$vmeta:meta])* $variant:ident => $repr:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        $vis enum $name {
            $($(#[$vmeta])* $variant),+
        }

        impl $name {
            /// Stable wire string for this variant.
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
    /// Authentication mechanism used by an account.
    pub enum AuthKind {
        /// Username + password (IMAP `LOGIN` / `AUTH PLAIN`, SMTP `AUTH LOGIN`).
        Password     => "password",
        /// Google OAuth2 with the `XOAUTH2` SASL mechanism.
        OAuth2Google => "*************",
    }
}

text_enum! {
    /// Lifecycle state of an account's background sync worker.
    pub enum SyncStatus {
        /// No sync activity in progress; ready to be started.
        Idle    => "idle",
        /// An IDLE/reconcile worker is currently active.
        Syncing => "syncing",
        /// The last sync attempt failed; see the account's `sync_error`.
        Error   => "error",
    }
}

text_enum! {
    /// Semantic role assigned to a mail folder.
    pub enum FolderRole {
        /// Primary incoming-mail folder.
        Inbox   => "inbox",
        /// Sent-items folder.
        Sent    => "sent",
        /// Local or server-side drafts folder.
        Drafts  => "drafts",
        /// Archive folder.
        Archive => "archive",
        /// Trash / deleted-items folder.
        Trash   => "trash",
        /// Spam / junk folder.
        Spam    => "spam",
        /// Gmail-style "All Mail" aggregate folder.
        All     => "all",
        /// Starred / flagged messages virtual folder.
        Starred => "starred",
        /// User-defined folder with no special semantics.
        Custom  => "custom",
    }
}

text_enum! {
    /// MIME `Content-Disposition` for an attachment.
    pub enum AttachmentDisposition {
        /// Rendered inline within the message body.
        Inline     => "inline",
        /// Presented as a downloadable attachment.
        Attachment => "attachment",
    }
}

text_enum! {
    /// Policy action a configured MCP gate applies to a tool call.
    pub enum GateAction {
        /// Allow the call without prompting the user.
        AutoAllow => "auto_allow",
        /// Require an explicit user approval before proceeding.
        Require   => "require",
        /// Block the call outright.
        Deny      => "deny",
    }
}

text_enum! {
    /// Lifecycle state of an MCP approval request.
    pub enum ApprovalState {
        /// Awaiting a user decision.
        Pending => "pending",
        /// User approved the call.
        Allowed => "allowed",
        /// User denied the call.
        Denied  => "denied",
        /// Decision window elapsed without a response.
        Expired => "expired",
    }
}

// -----------------------------------------------------------------------------
// Typed wrappers for SQLite JSON array columns
// -----------------------------------------------------------------------------

macro_rules! json_string_array {
    (
        $(#[$meta:meta])*
        $name:ident
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
        #[serde(transparent)]
        #[repr(transparent)]
        pub struct $name(Vec<String>);

        impl $name {
            /// Borrow the underlying strings as a slice.
            #[inline]
            pub fn as_slice(&self) -> &[String] {
                &self.0
            }

            /// Clone the underlying strings into a new `Vec`.
            #[inline]
            pub fn to_vec(&self) -> Vec<String> {
                self.0.clone()
            }

            /// Consume the wrapper and return the inner `Vec`.
            #[inline]
            pub fn into_vec(self) -> Vec<String> {
                self.0
            }
        }

        impl From<Vec<String>> for $name {
            #[inline]
            fn from(value: Vec<String>) -> Self {
                Self(value)
            }
        }

        impl<'a> From<Vec<&'a str>> for $name {
            fn from(value: Vec<&'a str>) -> Self {
                Self(value.into_iter().map(str::to_string).collect())
            }
        }

        impl<const N: usize> From<[&str; N]> for $name {
            fn from(value: [&str; N]) -> Self {
                Self(value.into_iter().map(str::to_string).collect())
            }
        }

        impl From<$name> for Vec<String> {
            #[inline]
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl From<$name> for serde_json::Value {
            fn from(value: $name) -> Self {
                serde_json::Value::Array(
                    value.0.into_iter().map(serde_json::Value::String).collect(),
                )
            }
        }

        impl TryFrom<serde_json::Value> for $name {
            type Error = serde_json::Error;

            fn try_from(value: serde_json::Value) -> Result<Self, Self::Error> {
                serde_json::from_value::<Vec<String>>(value).map(Self)
            }
        }

        impl FromStr for $name {
            type Err = serde_json::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                serde_json::from_str::<Vec<String>>(s).map(Self)
            }
        }

        impl sqlx::Type<sqlx::Sqlite> for $name {
            fn type_info() -> sqlx::sqlite::SqliteTypeInfo {
                <String as sqlx::Type<sqlx::Sqlite>>::type_info()
            }

            fn compatible(ty: &sqlx::sqlite::SqliteTypeInfo) -> bool {
                <String as sqlx::Type<sqlx::Sqlite>>::compatible(ty)
            }
        }

        impl<'r> sqlx::Decode<'r, sqlx::Sqlite> for $name {
            fn decode(
                value: sqlx::sqlite::SqliteValueRef<'r>,
            ) -> Result<Self, sqlx::error::BoxDynError> {
                let raw = <String as sqlx::Decode<sqlx::Sqlite>>::decode(value)?;
                raw.parse::<Self>()
                    .map_err(|e| -> sqlx::error::BoxDynError { e.into() })
            }
        }

        impl<'q> sqlx::Encode<'q, sqlx::Sqlite> for $name {
            fn encode_by_ref(
                &self,
                buf: &mut <sqlx::Sqlite as sqlx::Database>::ArgumentBuffer<'q>,
            ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
                let raw = serde_json::to_string(&self.0)?;
                <String as sqlx::Encode<sqlx::Sqlite>>::encode(raw, buf)
            }
        }
    };
}

json_string_array!(
    /// JSON array of address strings stored in message and draft address columns.
    AddressList
);

json_string_array!(
    /// JSON array of IMAP flag strings stored in the message flags column.
    MessageFlags
);

// -----------------------------------------------------------------------------
// Row types
// -----------------------------------------------------------------------------

/// SQL row representation of a configured mail account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Account {
    /// Stable identifier for this account row.
    pub id: AccountId,
    /// Primary email address used as the account's login identity.
    pub email: String,
    /// Optional display name (e.g. `"Alice Example"`).
    pub display_name: Option<String>,
    /// Authentication mechanism used for IMAP and SMTP.
    pub auth_kind: AuthKind,
    /// IMAP server hostname.
    pub imap_host: String,
    /// IMAP server port.
    pub imap_port: i64,
    /// Whether IMAP uses implicit TLS on connect.
    pub imap_use_tls: bool,
    /// SMTP submission server hostname.
    pub smtp_host: String,
    /// SMTP submission server port.
    pub smtp_port: i64,
    /// Whether SMTP uses implicit TLS on connect.
    pub smtp_use_tls: bool,
    /// Whether SMTP issues `STARTTLS` after connect.
    pub smtp_starttls: bool,
    /// Backend-specific reference to the [`crate::secrets::SecretStore`] entry.
    pub secret_ref: Option<String>,
    /// Timestamp of the most recent successful sync, if any.
    pub last_synced_at: Option<DateTime<Utc>>,
    /// Current lifecycle state of the sync worker.
    pub sync_status: SyncStatus,
    /// Human-readable error message from the last failed sync, if any.
    pub sync_error: Option<String>,
    /// Timestamp this row was created.
    pub created_at: DateTime<Utc>,
    /// Timestamp this row was last modified.
    pub updated_at: DateTime<Utc>,
}

/// SQL row representation of an IMAP mailbox folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Folder {
    /// Stable identifier for this folder row.
    pub id: FolderId,
    /// Account this folder belongs to.
    pub account_id: AccountId,
    /// Server-reported folder name (e.g. `"INBOX"` or `"[Gmail]/All Mail"`).
    pub name: String,
    /// IMAP hierarchy delimiter reported by the server (often `/` or `.`).
    pub delimiter: String,
    /// Inferred semantic role for the folder.
    pub role: FolderRole,
    /// IMAP `UIDVALIDITY` value seen for this folder, if known.
    pub uid_validity: Option<i64>,
    /// Next UID expected from the server, if known.
    pub uid_next: Option<i64>,
    /// Highest UID already pulled into the local store, if any.
    pub last_seen_uid: Option<i64>,
    /// Whether the folder can be `SELECT`ed (versus a container-only node).
    pub selectable: bool,
    /// Timestamp this row was created.
    pub created_at: DateTime<Utc>,
}

/// SQL row representation of a conversation thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Thread {
    /// Stable identifier for this thread row.
    pub id: ThreadId,
    /// Account this thread belongs to.
    pub account_id: AccountId,
    /// Provider-specific thread id (e.g. Gmail `X-GM-THRID`) when available.
    pub external_id: Option<String>,
    /// Normalised subject line representative of the thread.
    pub subject: Option<String>,
    /// Timestamp of the most recent message in the thread.
    pub last_message_at: Option<DateTime<Utc>>,
    /// Number of messages currently in the thread.
    pub message_count: i64,
    /// Timestamp this row was created.
    pub created_at: DateTime<Utc>,
}

/// SQL row representation of a single email message with bodies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Message {
    /// Stable identifier for this message row.
    pub id: MessageId,
    /// Account this message belongs to.
    pub account_id: AccountId,
    /// Folder containing this message.
    pub folder_id: FolderId,
    /// Optional thread association.
    pub thread_id: Option<ThreadId>,
    /// IMAP UID of the message within its folder.
    pub uid: i64,
    /// `Message-Id` header value, if present.
    pub message_id_header: Option<String>,
    /// `In-Reply-To` header value, if present.
    pub in_reply_to: Option<String>,
    /// `References` header value verbatim, if present.
    pub references_header: Option<String>,
    /// `From` address as a single rendered string.
    pub from_addr: String,
    /// `To` recipients.
    pub to_addrs: AddressList,
    /// `Cc` recipients.
    pub cc_addrs: AddressList,
    /// `Bcc` recipients, if locally retained.
    pub bcc_addrs: AddressList,
    /// `Reply-To` address, if present.
    pub reply_to: Option<String>,
    /// `Subject` header value.
    pub subject: Option<String>,
    /// Short preview snippet derived from the body.
    pub snippet: Option<String>,
    /// Plain-text body, if available.
    pub text_body: Option<String>,
    /// HTML body, if available.
    pub html_body: Option<String>,
    /// Raw RFC 5322 message size in bytes.
    pub raw_size: i64,
    /// IMAP flags currently set on the message.
    pub flags: MessageFlags,
    /// Server-assigned `INTERNALDATE`.
    pub internal_date: DateTime<Utc>,
    /// `Date` header timestamp, if parseable.
    pub sent_at: Option<DateTime<Utc>>,
    /// Timestamp this row was created.
    pub created_at: DateTime<Utc>,
}

/// Lightweight projection of a [`Message`] without the heavy body columns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct MessageSummary {
    /// Stable identifier for this message row.
    pub id: MessageId,
    /// Account this message belongs to.
    pub account_id: AccountId,
    /// Folder containing this message.
    pub folder_id: FolderId,
    /// Optional thread association.
    pub thread_id: Option<ThreadId>,
    /// IMAP UID of the message within its folder.
    pub uid: i64,
    /// `Message-Id` header value, if present.
    pub message_id_header: Option<String>,
    /// `In-Reply-To` header value, if present.
    pub in_reply_to: Option<String>,
    /// `References` header value verbatim, if present.
    pub references_header: Option<String>,
    /// `From` address as a single rendered string.
    pub from_addr: String,
    /// `To` recipients.
    pub to_addrs: AddressList,
    /// `Cc` recipients.
    pub cc_addrs: AddressList,
    /// `Bcc` recipients, if locally retained.
    pub bcc_addrs: AddressList,
    /// `Reply-To` address, if present.
    pub reply_to: Option<String>,
    /// `Subject` header value.
    pub subject: Option<String>,
    /// Short preview snippet derived from the body.
    pub snippet: Option<String>,
    /// Raw RFC 5322 message size in bytes.
    pub raw_size: i64,
    /// IMAP flags currently set on the message.
    pub flags: MessageFlags,
    /// Server-assigned `INTERNALDATE`.
    pub internal_date: DateTime<Utc>,
    /// `Date` header timestamp, if parseable.
    pub sent_at: Option<DateTime<Utc>>,
    /// Timestamp this row was created.
    pub created_at: DateTime<Utc>,
}

const _: () = assert!(std::mem::size_of::<Message>() <= 640);
const _: () = assert!(std::mem::size_of::<MessageSummary>() <= 576);

/// SQL row representation of a message attachment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Attachment {
    /// Stable identifier for this attachment row.
    pub id: AttachmentId,
    /// Message this attachment is associated with.
    pub message_id: MessageId,
    /// Original filename reported by the sender.
    pub filename: String,
    /// MIME `Content-Type` of the attachment.
    pub content_type: String,
    /// `Content-Id` for inline attachments referenced by HTML bodies.
    pub content_id: Option<String>,
    /// Size of the attachment payload in bytes.
    pub size_bytes: i64,
    /// MIME `Content-Disposition` (inline vs attachment).
    pub disposition: AttachmentDisposition,
    /// Filesystem path where the payload is stored.
    pub storage_path: String,
    /// Timestamp this row was created.
    pub created_at: DateTime<Utc>,
}

/// SQL row representation of an outbound message draft.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Draft {
    /// Stable identifier for this draft row.
    pub id: DraftId,
    /// Account the draft will be sent from.
    pub account_id: AccountId,
    /// Local message this draft is a reply to, if any.
    pub in_reply_to_msg: Option<MessageId>,
    /// `To` recipients.
    pub to_addrs: AddressList,
    /// `Cc` recipients.
    pub cc_addrs: AddressList,
    /// `Bcc` recipients.
    pub bcc_addrs: AddressList,
    /// `Subject` header value.
    pub subject: Option<String>,
    /// Plain-text body.
    pub text_body: Option<String>,
    /// HTML body.
    pub html_body: Option<String>,
    /// `In-Reply-To` header to thread the outgoing message.
    pub in_reply_to: Option<String>,
    /// `References` header to thread the outgoing message.
    pub references_header: Option<String>,
    /// Remote folder where the draft was appended, if uploaded to IMAP.
    pub remote_folder_id: Option<FolderId>,
    /// Remote UID of the appended draft, if known.
    pub remote_uid: Option<i64>,
    /// Timestamp this row was created.
    pub created_at: DateTime<Utc>,
    /// Timestamp this row was last modified.
    pub updated_at: DateTime<Utc>,
}

/// SQL row representation of a file attached to an outbound draft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct DraftAttachment {
    /// Stable identifier for this draft attachment row.
    pub id: Uuid,
    /// Draft this attachment belongs to.
    pub draft_id: DraftId,
    /// Filename presented to the recipient.
    pub filename: String,
    /// MIME `Content-Type` of the attachment.
    pub content_type: String,
    /// Size of the attachment payload in bytes.
    pub size_bytes: i64,
    /// Timestamp this row was created.
    pub created_at: DateTime<Utc>,
}

/// SQL row representation of an MCP policy gate matching tool calls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct McpGate {
    /// Stable identifier for this gate row.
    pub id: Uuid,
    /// Name of the MCP tool this gate matches.
    pub tool: String,
    /// Optional argument pattern that scopes the gate further.
    pub arg_pattern: Option<String>,
    /// Action to apply when the gate matches.
    pub action: GateAction,
    /// Free-form note describing why the gate exists.
    pub note: Option<String>,
    /// Timestamp this row was created.
    pub created_at: DateTime<Utc>,
}

/// SQL row representation of a pending or decided MCP approval request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct McpApproval {
    /// Stable identifier for this approval row.
    pub id: Uuid,
    /// Name of the MCP tool the call targets.
    pub tool: String,
    /// JSON-encoded arguments captured at request time.
    pub args: serde_json::Value,
    /// Short human-readable summary shown in approval UIs.
    pub summary: String,
    /// Current lifecycle state of the request.
    pub state: ApprovalState,
    /// Timestamp the request was decided, if decided.
    pub decided_at: Option<DateTime<Utc>>,
    /// Identifier of the actor that decided the request.
    pub decided_by: Option<String>,
    /// Timestamp this row was created.
    pub created_at: DateTime<Utc>,
}

/// SQL row representation of a single audit-log entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct AuditEntry {
    /// Stable identifier for this audit entry.
    pub id: Uuid,
    /// Identifier of the actor that performed the action.
    pub actor: String,
    /// Logical action name (e.g. `"message.delete"`).
    pub action: String,
    /// Optional identifier of the entity acted upon.
    pub target: Option<String>,
    /// JSON-encoded detail payload describing the action.
    pub details: serde_json::Value,
    /// Timestamp this row was created.
    pub created_at: DateTime<Utc>,
}

// -----------------------------------------------------------------------------
// Tests for enum round-trips. The sqlx encode/decode paths are exercised in
// db tests against a real SQLite file.
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
    fn test_address_list_serde_preserves_json_array_wire_shape() {
        let addresses = AddressList::from(vec!["alice@example.com", "bob@example.com"]);
        let value = serde_json::to_value(&addresses).unwrap();
        assert_eq!(value, json!(["alice@example.com", "bob@example.com"]));

        let decoded: AddressList = serde_json::from_value(value).unwrap();
        assert_eq!(decoded, addresses);
    }

    #[test]
    fn test_message_flags_serde_preserves_json_array_wire_shape() {
        let flags = MessageFlags::from(vec!["\\Seen", "\\Flagged"]);
        let value = serde_json::to_value(&flags).unwrap();
        assert_eq!(value, json!(["\\Seen", "\\Flagged"]));

        let decoded: MessageFlags = serde_json::from_value(value).unwrap();
        assert_eq!(decoded, flags);
    }

    #[tokio::test]
    async fn test_json_string_wrappers_sqlx_round_trip() {
        let pool = crate::db::test_pool().await;
        let addresses = AddressList::from(vec!["alice@example.com", "bob@example.com"]);
        let decoded_addresses: AddressList = sqlx::query_scalar("SELECT ?")
            .bind(&addresses)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(decoded_addresses, addresses);

        let flags = MessageFlags::from(vec!["\\Seen", "\\Flagged"]);
        let decoded_flags: MessageFlags = sqlx::query_scalar("SELECT ?")
            .bind(&flags)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(decoded_flags, flags);
    }

    #[tokio::test]
    async fn test_json_string_wrapper_decode_rejects_malformed_json_column() {
        let pool = crate::db::test_pool().await;
        let err = sqlx::query_scalar::<_, AddressList>(r#"SELECT '["ok", 7]'"#)
            .fetch_one(&pool)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("error occurred while decoding"));

        let err = sqlx::query_scalar::<_, MessageFlags>(r#"SELECT '{"flag":"\\Seen"}'"#)
            .fetch_one(&pool)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("error occurred while decoding"));
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
