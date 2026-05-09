//! Typed vocabulary of IPC ops the daemon dispatches.
//!
//! The wire format keeps `Request.op` as a `String`; this enum is the
//! internal compile-time vocabulary the daemon matches on. Parsing
//! happens once at the wire boundary in `crate::ipc::server`'s
//! per-connection reader, so a typo'd op fails fast before any handler
//! runs.
//!
//! Pattern mirrors the typed-enum convention used by
//! [`crate::ipc::Topic`] and the `text_enum!` macro in
//! [`crate::models`].

use std::fmt;
use std::str::FromStr;

/// Closed vocabulary of dispatcher ops. The wire string is preserved
/// verbatim by [`Op::as_str`] and the [`FromStr`] impl so external
/// clients keep using the same op names they always have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Op {
    // -- read ops --
    AccountList,
    FolderList,
    ThreadList,
    MessageListByFolder,
    MessageListByThread,
    MessageGet,
    AttachmentList,
    AttachmentPreview,
    Search,
    AuditListRecent,

    // -- MCP gate/approval ops --
    McpGateList,
    McpGateCreate,
    McpGateDelete,
    McpApprovalCreate,
    McpApprovalList,
    McpApprovalGet,
    McpApprovalDecide,

    // -- write ops --
    AccountCreate,
    AccountDelete,
    FolderUpsert,
    MessageSetFlags,
    MessageArchive,
    MessageDelete,
    MessageMove,
    DraftCreate,
    DraftUpdate,
    DraftDelete,
    DraftList,
    DraftGet,
    AttachmentExport,
    MessagePrepareReply,
    MessagePrepareForward,
    AttachmentFetchForForward,
    MessageSend,

    // -- network ops --
    AccountTestLogin,
    AccountSyncFolder,
    AccountStartSync,
    AccountStopSync,

    // -- secret ops --
    AccountSetSecret,
    AccountDeleteSecret,

    // -- OAuth ops --
    OauthGoogleAuthUrl,
    OauthGoogleComplete,
}

impl Op {
    /// Wire string for this op. Stable: external clients rely on these
    /// exact bytes.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::AccountList => "account.list",
            Self::FolderList => "folder.list",
            Self::ThreadList => "thread.list",
            Self::MessageListByFolder => "message.list_by_folder",
            Self::MessageListByThread => "message.list_by_thread",
            Self::MessageGet => "message.get",
            Self::AttachmentList => "attachment.list",
            Self::AttachmentPreview => "attachment.preview",
            Self::Search => "search",
            Self::AuditListRecent => "audit.list_recent",
            Self::McpGateList => "mcp.gate.list",
            Self::McpGateCreate => "mcp.gate.create",
            Self::McpGateDelete => "mcp.gate.delete",
            Self::McpApprovalCreate => "mcp.approval.create",
            Self::McpApprovalList => "mcp.approval.list",
            Self::McpApprovalGet => "mcp.approval.get",
            Self::McpApprovalDecide => "mcp.approval.decide",
            Self::AccountCreate => "account.create",
            Self::AccountDelete => "account.delete",
            Self::FolderUpsert => "folder.upsert",
            Self::MessageSetFlags => "message.set_flags",
            Self::MessageArchive => "message.archive",
            Self::MessageDelete => "message.delete",
            Self::MessageMove => "message.move",
            Self::DraftCreate => "draft.create",
            Self::DraftUpdate => "draft.update",
            Self::DraftDelete => "draft.delete",
            Self::DraftList => "draft.list",
            Self::DraftGet => "draft.get",
            Self::AttachmentExport => "attachment.export",
            Self::MessagePrepareReply => "message.prepare_reply",
            Self::MessagePrepareForward => "message.prepare_forward",
            Self::AttachmentFetchForForward => "attachment.fetch_for_forward",
            Self::MessageSend => "message.send",
            Self::AccountTestLogin => "account.test_login",
            Self::AccountSyncFolder => "account.sync_folder",
            Self::AccountStartSync => "account.start_sync",
            Self::AccountStopSync => "account.stop_sync",
            Self::AccountSetSecret => "account.set_secret",
            Self::AccountDeleteSecret => "account.delete_secret",
            Self::OauthGoogleAuthUrl => "oauth.google.auth_url",
            Self::OauthGoogleComplete => "oauth.google.complete",
        }
    }
}

/// Returned by [`Op::from_str`] when the wire string isn't a known op.
/// Carries the offending input so the wire boundary can format an
/// `unknown_op` error without re-threading the original string.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown op: {0}")]
pub struct ParseOpError(pub String);

impl fmt::Display for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Op {
    type Err = ParseOpError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "account.list" => Ok(Self::AccountList),
            "folder.list" => Ok(Self::FolderList),
            "thread.list" => Ok(Self::ThreadList),
            "message.list_by_folder" => Ok(Self::MessageListByFolder),
            "message.list_by_thread" => Ok(Self::MessageListByThread),
            "message.get" => Ok(Self::MessageGet),
            "attachment.list" => Ok(Self::AttachmentList),
            "attachment.preview" => Ok(Self::AttachmentPreview),
            "search" => Ok(Self::Search),
            "audit.list_recent" => Ok(Self::AuditListRecent),
            "mcp.gate.list" => Ok(Self::McpGateList),
            "mcp.gate.create" => Ok(Self::McpGateCreate),
            "mcp.gate.delete" => Ok(Self::McpGateDelete),
            "mcp.approval.create" => Ok(Self::McpApprovalCreate),
            "mcp.approval.list" => Ok(Self::McpApprovalList),
            "mcp.approval.get" => Ok(Self::McpApprovalGet),
            "mcp.approval.decide" => Ok(Self::McpApprovalDecide),
            "account.create" => Ok(Self::AccountCreate),
            "account.delete" => Ok(Self::AccountDelete),
            "folder.upsert" => Ok(Self::FolderUpsert),
            "message.set_flags" => Ok(Self::MessageSetFlags),
            "message.archive" => Ok(Self::MessageArchive),
            "message.delete" => Ok(Self::MessageDelete),
            "message.move" => Ok(Self::MessageMove),
            "draft.create" => Ok(Self::DraftCreate),
            "draft.update" => Ok(Self::DraftUpdate),
            "draft.delete" => Ok(Self::DraftDelete),
            "draft.list" => Ok(Self::DraftList),
            "draft.get" => Ok(Self::DraftGet),
            "attachment.export" => Ok(Self::AttachmentExport),
            "message.prepare_reply" => Ok(Self::MessagePrepareReply),
            "message.prepare_forward" => Ok(Self::MessagePrepareForward),
            "attachment.fetch_for_forward" => Ok(Self::AttachmentFetchForForward),
            "message.send" => Ok(Self::MessageSend),
            "account.test_login" => Ok(Self::AccountTestLogin),
            "account.sync_folder" => Ok(Self::AccountSyncFolder),
            "account.start_sync" => Ok(Self::AccountStartSync),
            "account.stop_sync" => Ok(Self::AccountStopSync),
            "account.set_secret" => Ok(Self::AccountSetSecret),
            "account.delete_secret" => Ok(Self::AccountDeleteSecret),
            "oauth.google.auth_url" => Ok(Self::OauthGoogleAuthUrl),
            "oauth.google.complete" => Ok(Self::OauthGoogleComplete),
            _ => Err(ParseOpError(s.to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every variant the dispatcher handles. Kept in sync with the
    /// match arms above; round-trip tests iterate this list.
    const ALL_OPS: &[Op] = &[
        Op::AccountList,
        Op::FolderList,
        Op::ThreadList,
        Op::MessageListByFolder,
        Op::MessageListByThread,
        Op::MessageGet,
        Op::AttachmentList,
        Op::AttachmentPreview,
        Op::Search,
        Op::AuditListRecent,
        Op::McpGateList,
        Op::McpGateCreate,
        Op::McpGateDelete,
        Op::McpApprovalCreate,
        Op::McpApprovalList,
        Op::McpApprovalGet,
        Op::McpApprovalDecide,
        Op::AccountCreate,
        Op::AccountDelete,
        Op::FolderUpsert,
        Op::MessageSetFlags,
        Op::MessageArchive,
        Op::MessageDelete,
        Op::MessageMove,
        Op::DraftCreate,
        Op::DraftUpdate,
        Op::DraftDelete,
        Op::DraftList,
        Op::DraftGet,
        Op::AttachmentExport,
        Op::MessagePrepareReply,
        Op::MessagePrepareForward,
        Op::AttachmentFetchForForward,
        Op::MessageSend,
        Op::AccountTestLogin,
        Op::AccountSyncFolder,
        Op::AccountStartSync,
        Op::AccountStopSync,
        Op::AccountSetSecret,
        Op::AccountDeleteSecret,
        Op::OauthGoogleAuthUrl,
        Op::OauthGoogleComplete,
    ];

    #[test]
    fn test_op_parse_unknown_returns_typed_error() {
        let err = "not_a_real_op".parse::<Op>().unwrap_err();
        assert_eq!(err.0, "not_a_real_op");
    }

    #[test]
    fn test_op_round_trip_display_via_from_str_for_each_variant() {
        for op in ALL_OPS {
            let rendered = op.to_string();
            let parsed = rendered.parse::<Op>().unwrap_or_else(|e| {
                panic!("variant {op:?} rendered as {rendered:?} but failed to parse: {e:?}")
            });
            assert_eq!(parsed, *op, "round-trip mismatch for {op:?}");
        }
    }

    #[test]
    fn test_op_as_str_matches_display() {
        for op in ALL_OPS {
            assert_eq!(op.as_str(), op.to_string());
        }
    }

    #[test]
    fn test_parse_op_error_carries_input() {
        let err = "garbage".parse::<Op>().unwrap_err();
        assert_eq!(err, ParseOpError("garbage".to_string()));
    }
}
