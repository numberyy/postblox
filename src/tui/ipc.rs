//! Thin RPC layer between [`super::app::AppState`] and `postbloxd`.
//!
//! Wraps [`crate::ipc::client::Client`] to give the TUI strongly
//! typed list/CRUD/send/search calls plus the response decoders for
//! reply, forward, and draft fetches. Attachment payloads cross the
//! socket as base64 (the wire stays JSON-friendly); the typed result
//! structs decode them on the TUI side. All daemon traffic in the
//! TUI flows through this module — [`super::app::AppState`] never
//! touches the socket directly.

use std::path::Path;

use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use thiserror::Error;
use uuid::Uuid;

use crate::ipc::client::{Client, ClientError};
use crate::ipc::{Event, Response, RpcError, Topic};
use crate::models::{
    Account, AccountId, ApprovalState, Attachment, AttachmentId, Draft, DraftId, Folder, FolderId,
    McpApproval, Message, MessageId, MessageSummary,
};

use super::app::{
    AccountItem, ApprovalItem, ApprovalTargetContext, AttachmentItem, AttachmentPreviewItem,
    ComposerDraft, DraftItem, DraftSummary, FolderItem, MessageDetail, MessageItem, SearchHit,
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub(crate) struct AttachmentExportResult {
    pub(crate) attachment_id: AttachmentId,
    pub(crate) destination_path: String,
    pub(crate) bytes_copied: u64,
}

/// Decoded `message.prepare_reply` response.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub(crate) struct ReplyPrepared {
    pub(crate) message_id: MessageId,
    pub(crate) account_id: AccountId,
    pub(crate) to: Vec<String>,
    pub(crate) cc: Vec<String>,
    pub(crate) subject: String,
    pub(crate) in_reply_to: String,
    pub(crate) references: String,
    pub(crate) quoted_body: String,
}

/// Decoded `message.prepare_forward` response.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub(crate) struct ForwardPrepared {
    pub(crate) message_id: MessageId,
    pub(crate) account_id: AccountId,
    pub(crate) subject: String,
    pub(crate) forwarded_body: String,
    pub(crate) forwarded_attachments: Vec<ForwardAttachmentMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub(crate) struct ForwardAttachmentMeta {
    pub(crate) message_id: MessageId,
    pub(crate) attachment_id: AttachmentId,
    pub(crate) filename: String,
    pub(crate) content_type: String,
    pub(crate) size_bytes: i64,
}

/// Decoded `attachment.fetch_for_forward` response. The bytes are
/// base64-encoded over the wire; the helper returns raw bytes via
/// [`ForwardAttachmentBytes::decoded_bytes`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub(crate) struct ForwardAttachmentBytes {
    pub(crate) attachment_id: AttachmentId,
    pub(crate) filename: String,
    pub(crate) content_type: String,
    pub(crate) size_bytes: i64,
    pub(crate) content_base64: String,
}

impl ForwardAttachmentBytes {
    /// Decode `content_base64` into raw attachment bytes.
    ///
    /// # Errors
    ///
    /// Returns [`base64::DecodeError`] if the daemon-supplied base64
    /// payload is malformed.
    pub(crate) fn decoded_bytes(&self) -> Result<Vec<u8>, base64::DecodeError> {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.decode(&self.content_base64)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub(crate) struct ForwardAttachmentBatch {
    pub(crate) attachments: Vec<ForwardAttachmentBytes>,
    pub(crate) failed: Vec<ForwardAttachmentFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub(crate) struct ForwardAttachmentFailure {
    pub(crate) attachment_id: AttachmentId,
    pub(crate) filename: String,
    pub(crate) code: String,
    pub(crate) message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
struct SendResult {
    message_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
struct ApprovalDecisionResult {
    decided: bool,
}

/// Decoded `draft.get` response. The daemon sends attachment bytes as
/// base64 so the wire stays JSON-friendly; the TUI re-materialises
/// them as temp files when re-opening a draft.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub(crate) struct DraftGetResult {
    pub(crate) draft: Draft,
    pub(crate) attachments: Vec<DraftAttachmentPayload>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub(crate) struct DraftAttachmentPayload {
    pub(crate) id: Uuid,
    pub(crate) draft_id: DraftId,
    pub(crate) filename: String,
    pub(crate) content_type: String,
    pub(crate) size_bytes: i64,
    pub(crate) content_base64: String,
}

impl DraftAttachmentPayload {
    /// Decode `content_base64` into raw attachment bytes.
    ///
    /// # Errors
    ///
    /// Returns [`base64::DecodeError`] if the daemon-supplied base64
    /// payload is malformed.
    pub(crate) fn decoded_bytes(&self) -> Result<Vec<u8>, base64::DecodeError> {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.decode(&self.content_base64)
    }
}

/// Errors surfaced by the TUI's [`MailboxClient`] when talking to the daemon.
#[derive(Debug, Error)]
pub enum MailboxError {
    /// Failed to connect to the daemon's Unix socket.
    #[error("connect failed: {0}")]
    Connect(#[source] ClientError),
    /// Transport- or framing-level failure while issuing an op.
    #[error("{op} request failed: {source}")]
    Request {
        /// Name of the IPC op that failed.
        op: &'static str,
        /// Underlying client error.
        #[source]
        source: ClientError,
    },
    /// Daemon returned an `RpcError` for the op.
    #[error("{op} failed: {code}: {message}")]
    Server {
        /// Name of the IPC op that failed.
        op: &'static str,
        /// `RpcError` code returned by the daemon.
        code: String,
        /// Human-readable message returned by the daemon.
        message: String,
    },
    /// Daemon returned a successful response that did not match the expected schema.
    #[error("{op} returned malformed data: {source}")]
    Decode {
        /// Name of the IPC op whose response failed to decode.
        op: &'static str,
        /// Underlying serde error.
        #[source]
        source: serde_json::Error,
    },
}

/// Daemon-facing IPC client used by the TUI for read and write ops.
pub struct MailboxClient {
    client: Client,
}

impl MailboxClient {
    /// Connect to the daemon Unix socket at `path`.
    ///
    /// # Errors
    ///
    /// Returns [`MailboxError::Connect`] if the underlying socket
    /// connection or handshake fails (missing socket, permission
    /// denied, daemon not running, codec mismatch).
    pub async fn connect(path: &Path) -> Result<Self, MailboxError> {
        Client::connect(path)
            .await
            .map(|client| Self { client })
            .map_err(MailboxError::Connect)
    }

    /// Fetch the configured accounts in display order.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`MailboxError::Request`] if the IPC request itself fails.
    /// - [`MailboxError::Server`] if the daemon returned `ok = false`.
    /// - [`MailboxError::Decode`] if the response payload cannot be
    ///   decoded as `Vec<Account>`.
    pub(crate) async fn list_accounts(&mut self) -> Result<Vec<AccountItem>, MailboxError> {
        let response = self.request("account.list", json!({})).await?;
        let accounts: Vec<Account> = decode_response("account.list", response)?;
        Ok(accounts.into_iter().map(AccountItem::from).collect())
    }

    /// List folders for `account_id`.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`MailboxError::Request`] if the IPC request itself fails.
    /// - [`MailboxError::Server`] if the daemon returned `ok = false`.
    /// - [`MailboxError::Decode`] if the response payload cannot be
    ///   decoded as `Vec<Folder>`.
    pub(crate) async fn list_folders(
        &mut self,
        account_id: AccountId,
    ) -> Result<Vec<FolderItem>, MailboxError> {
        let response = self
            .request("folder.list", json!({ "account_id": account_id }))
            .await?;
        let folders: Vec<Folder> = decode_response("folder.list", response)?;
        Ok(folders.into_iter().map(FolderItem::from).collect())
    }

    /// List the first 100 messages in `folder_id` ordered by the
    /// daemon's default sort.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`MailboxError::Request`] if the IPC request itself fails.
    /// - [`MailboxError::Server`] if the daemon returned `ok = false`.
    /// - [`MailboxError::Decode`] if the response payload cannot be
    ///   decoded as `Vec<MessageSummary>`.
    pub(crate) async fn list_messages(
        &mut self,
        folder_id: FolderId,
    ) -> Result<Vec<MessageItem>, MailboxError> {
        let response = self
            .request(
                "message.list_by_folder",
                json!({ "folder_id": folder_id, "limit": 100, "offset": 0 }),
            )
            .await?;
        let messages: Vec<MessageSummary> = decode_response("message.list_by_folder", response)?;
        Ok(messages.into_iter().map(MessageItem::from).collect())
    }

    /// Fetch a single message by id; `Ok(None)` if not present.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`MailboxError::Request`] if the IPC request itself fails.
    /// - [`MailboxError::Server`] if the daemon returned `ok = false`.
    /// - [`MailboxError::Decode`] if the response payload cannot be
    ///   decoded as `Option<Message>`.
    pub(crate) async fn get_message(
        &mut self,
        message_id: MessageId,
    ) -> Result<Option<MessageDetail>, MailboxError> {
        let response = self
            .request("message.get", json!({ "id": message_id }))
            .await?;
        let message: Option<Message> = decode_response("message.get", response)?;
        Ok(message.map(MessageDetail::from))
    }

    /// Fetch message target metadata for approval rows.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`MailboxError::Request`] if the IPC request itself fails.
    /// - [`MailboxError::Server`] if the daemon returned `ok = false`.
    /// - [`MailboxError::Decode`] if the response payload cannot be
    ///   decoded as `Option<Message>`.
    pub(crate) async fn get_message_approval_context(
        &mut self,
        message_id: MessageId,
    ) -> Result<Option<ApprovalTargetContext>, MailboxError> {
        let response = self
            .request("message.get", json!({ "id": message_id }))
            .await?;
        let message: Option<Message> = decode_response("message.get", response)?;
        Ok(message.as_ref().map(ApprovalTargetContext::from_message))
    }

    /// One-shot sync of `folder_name` against the upstream IMAP server.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`MailboxError::Request`] if the IPC request itself fails.
    /// - [`MailboxError::Server`] if the daemon returned `ok = false`
    ///   (e.g. unknown account, IMAP failure).
    /// - [`MailboxError::Decode`] if the response payload cannot be
    ///   decoded.
    pub(crate) async fn sync_folder(
        &mut self,
        account_id: AccountId,
        folder_name: &str,
    ) -> Result<Value, MailboxError> {
        let response = self
            .request(
                "account.sync_folder",
                account_folder_args(account_id, folder_name),
            )
            .await?;
        decode_response("account.sync_folder", response)
    }

    /// Start the IMAP IDLE worker for `folder_name` on `account_id`.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`MailboxError::Request`] if the IPC request itself fails.
    /// - [`MailboxError::Server`] if the daemon returned `ok = false`
    ///   (e.g. unknown account, worker spawn failure).
    /// - [`MailboxError::Decode`] if the response payload cannot be
    ///   decoded.
    pub(crate) async fn start_sync(
        &mut self,
        account_id: AccountId,
        folder_name: &str,
    ) -> Result<Value, MailboxError> {
        let response = self
            .request(
                "account.start_sync",
                account_folder_args(account_id, folder_name),
            )
            .await?;
        decode_response("account.start_sync", response)
    }

    /// Stop the IMAP IDLE worker for `folder_name` on `account_id`.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`MailboxError::Request`] if the IPC request itself fails.
    /// - [`MailboxError::Server`] if the daemon returned `ok = false`.
    /// - [`MailboxError::Decode`] if the response payload cannot be
    ///   decoded.
    pub(crate) async fn stop_sync(
        &mut self,
        account_id: AccountId,
        folder_name: &str,
    ) -> Result<Value, MailboxError> {
        let response = self
            .request(
                "account.stop_sync",
                account_folder_args(account_id, folder_name),
            )
            .await?;
        decode_response("account.stop_sync", response)
    }

    /// Replace the IMAP flag set on `message_id` with `flags`.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`MailboxError::Request`] if the IPC request itself fails.
    /// - [`MailboxError::Server`] if the daemon returned `ok = false`.
    /// - [`MailboxError::Decode`] if the response payload cannot be
    ///   decoded.
    pub(crate) async fn set_flags(
        &mut self,
        message_id: MessageId,
        flags: &[String],
    ) -> Result<(), MailboxError> {
        let response = self
            .request("message.set_flags", set_flags_args(message_id, flags))
            .await?;
        let _: Value = decode_response("message.set_flags", response)?;
        Ok(())
    }

    /// Move `message_id` into the account's Archive folder.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`MailboxError::Request`] if the IPC request itself fails.
    /// - [`MailboxError::Server`] if the daemon returned `ok = false`.
    /// - [`MailboxError::Decode`] if the response payload cannot be
    ///   decoded.
    pub async fn archive_message(&mut self, message_id: MessageId) -> Result<(), MailboxError> {
        let response = self
            .request("message.archive", json!({ "id": message_id }))
            .await?;
        let _: Value = decode_response("message.archive", response)?;
        Ok(())
    }

    /// Move `message_id` into the account's Trash folder.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`MailboxError::Request`] if the IPC request itself fails.
    /// - [`MailboxError::Server`] if the daemon returned `ok = false`.
    /// - [`MailboxError::Decode`] if the response payload cannot be
    ///   decoded.
    pub(crate) async fn delete_message(
        &mut self,
        message_id: MessageId,
    ) -> Result<(), MailboxError> {
        let response = self
            .request("message.delete", json!({ "id": message_id }))
            .await?;
        let _: Value = decode_response("message.delete", response)?;
        Ok(())
    }

    /// Move `message_id` into `folder_name` (resolved by name on the
    /// daemon side).
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`MailboxError::Request`] if the IPC request itself fails.
    /// - [`MailboxError::Server`] if the daemon returned `ok = false`
    ///   (e.g. unknown folder name).
    /// - [`MailboxError::Decode`] if the response payload cannot be
    ///   decoded.
    pub(crate) async fn move_message(
        &mut self,
        message_id: MessageId,
        folder_name: &str,
    ) -> Result<(), MailboxError> {
        let response = self
            .request(
                "message.move",
                json!({ "id": message_id, "folder_name": folder_name }),
            )
            .await?;
        let _: Value = decode_response("message.move", response)?;
        Ok(())
    }

    /// List attachments for `message_id`.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`MailboxError::Request`] if the IPC request itself fails.
    /// - [`MailboxError::Server`] if the daemon returned `ok = false`.
    /// - [`MailboxError::Decode`] if the response payload cannot be
    ///   decoded as `Vec<Attachment>`.
    pub(crate) async fn list_attachments(
        &mut self,
        message_id: MessageId,
    ) -> Result<Vec<AttachmentItem>, MailboxError> {
        let response = self
            .request("attachment.list", attachment_list_args(message_id))
            .await?;
        let attachments: Vec<Attachment> = decode_response("attachment.list", response)?;
        Ok(attachments.into_iter().map(AttachmentItem::from).collect())
    }

    /// Fetch a preview blob for `attachment_id`.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`MailboxError::Request`] if the IPC request itself fails.
    /// - [`MailboxError::Server`] if the daemon returned `ok = false`
    ///   (e.g. attachment too large to preview).
    /// - [`MailboxError::Decode`] if the response payload cannot be
    ///   decoded as [`crate::attachments::AttachmentPreview`].
    pub(crate) async fn preview_attachment(
        &mut self,
        attachment_id: AttachmentId,
    ) -> Result<AttachmentPreviewItem, MailboxError> {
        let response = self
            .request("attachment.preview", attachment_preview_args(attachment_id))
            .await?;
        let preview: crate::attachments::AttachmentPreview =
            decode_response("attachment.preview", response)?;
        Ok(AttachmentPreviewItem::from(preview))
    }

    /// Persist `attachment_id` to `destination_path` on disk.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`MailboxError::Request`] if the IPC request itself fails.
    /// - [`MailboxError::Server`] if the daemon returned `ok = false`
    ///   (e.g. unknown attachment, IO failure on the daemon side).
    /// - [`MailboxError::Decode`] if the response payload cannot be
    ///   decoded as [`AttachmentExportResult`].
    pub(crate) async fn export_attachment(
        &mut self,
        attachment_id: AttachmentId,
        destination_path: &Path,
    ) -> Result<AttachmentExportResult, MailboxError> {
        let response = self
            .request(
                "attachment.export",
                attachment_export_args(attachment_id, destination_path),
            )
            .await?;
        decode_response("attachment.export", response)
    }

    /// Persist a new draft and return its assigned id.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`MailboxError::Request`] if the IPC request itself fails.
    /// - [`MailboxError::Server`] if the daemon returned `ok = false`.
    /// - [`MailboxError::Decode`] if the response payload cannot be
    ///   decoded as [`Draft`].
    pub(crate) async fn create_draft(
        &mut self,
        draft: &ComposerDraft,
    ) -> Result<DraftId, MailboxError> {
        let response = self
            .request("draft.create", draft_create_args(draft))
            .await?;
        let draft: Draft = decode_response("draft.create", response)?;
        Ok(draft.id)
    }

    /// Replace draft `draft_id` with the contents of `draft`.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`MailboxError::Request`] if the IPC request itself fails.
    /// - [`MailboxError::Server`] if the daemon returned `ok = false`,
    ///   or the draft was deleted between read and update (synthesised
    ///   `not_found`).
    /// - [`MailboxError::Decode`] if the response payload cannot be
    ///   decoded as `Option<Draft>`.
    pub(crate) async fn update_draft(
        &mut self,
        draft_id: DraftId,
        draft: &ComposerDraft,
    ) -> Result<DraftId, MailboxError> {
        let response = self
            .request("draft.update", draft_update_args(draft_id, draft))
            .await?;
        let draft: Option<Draft> = decode_response("draft.update", response)?;
        draft
            .map(|draft| draft.id)
            .ok_or_else(|| MailboxError::Server {
                op: "draft.update",
                code: "not_found".into(),
                message: "draft no longer exists".into(),
            })
    }

    /// Submit `draft_id` via SMTP for `account_id` and return the
    /// resulting RFC 5322 `Message-Id`.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`MailboxError::Request`] if the IPC request itself fails.
    /// - [`MailboxError::Server`] if the daemon returned `ok = false`
    ///   (SMTP submission failed, draft missing, etc.).
    /// - [`MailboxError::Decode`] if the response payload cannot be
    ///   decoded.
    pub(crate) async fn send_draft(
        &mut self,
        account_id: AccountId,
        draft_id: DraftId,
    ) -> Result<String, MailboxError> {
        let response = self
            .request("message.send", message_send_args(account_id, draft_id))
            .await?;
        let sent: SendResult = decode_response("message.send", response)?;
        Ok(sent.message_id)
    }

    /// List all drafts for `account_id`.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`MailboxError::Request`] if the IPC request itself fails.
    /// - [`MailboxError::Server`] if the daemon returned `ok = false`.
    /// - [`MailboxError::Decode`] if the response payload cannot be
    ///   decoded as `Vec<Draft>`.
    pub(crate) async fn list_drafts(
        &mut self,
        account_id: AccountId,
    ) -> Result<Vec<DraftItem>, MailboxError> {
        let response = self
            .request("draft.list", json!({ "account_id": account_id }))
            .await?;
        let drafts: Vec<Draft> = decode_response("draft.list", response)?;
        Ok(drafts.into_iter().map(DraftItem::from).collect())
    }

    /// Fetch a single draft (with attachment payloads) by id;
    /// `Ok(None)` if not present.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`MailboxError::Request`] if the IPC request itself fails.
    /// - [`MailboxError::Server`] if the daemon returned `ok = false`.
    /// - [`MailboxError::Decode`] if the response payload cannot be
    ///   decoded as `Option<DraftGetResult>`.
    pub(crate) async fn get_draft(
        &mut self,
        draft_id: DraftId,
    ) -> Result<Option<DraftSummary>, MailboxError> {
        let response = self.request("draft.get", json!({ "id": draft_id })).await?;
        let payload: Option<DraftGetResult> = decode_response("draft.get", response)?;
        Ok(payload.map(DraftSummary::from))
    }

    /// Fetch draft target metadata for approval rows.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`MailboxError::Request`] if the IPC request itself fails.
    /// - [`MailboxError::Server`] if the daemon returned `ok = false`.
    /// - [`MailboxError::Decode`] if the response payload cannot be
    ///   decoded as `Option<DraftGetResult>`.
    pub(crate) async fn get_draft_approval_context(
        &mut self,
        draft_id: DraftId,
    ) -> Result<Option<ApprovalTargetContext>, MailboxError> {
        let response = self.request("draft.get", json!({ "id": draft_id })).await?;
        let payload: Option<DraftGetResult> = decode_response("draft.get", response)?;
        Ok(payload
            .as_ref()
            .map(|payload| ApprovalTargetContext::from_draft(&payload.draft)))
    }

    /// Permanently delete `draft_id`.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`MailboxError::Request`] if the IPC request itself fails.
    /// - [`MailboxError::Server`] if the daemon returned `ok = false`.
    /// - [`MailboxError::Decode`] if the response payload cannot be
    ///   decoded.
    pub(crate) async fn delete_draft(&mut self, draft_id: DraftId) -> Result<(), MailboxError> {
        let response = self
            .request("draft.delete", json!({ "id": draft_id }))
            .await?;
        let _: Value = decode_response("draft.delete", response)?;
        Ok(())
    }

    /// Run a FTS5 search across messages, optionally scoped to a
    /// single account.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`MailboxError::Request`] if the IPC request itself fails.
    /// - [`MailboxError::Server`] if the daemon returned `ok = false`.
    /// - [`MailboxError::Decode`] if the response payload cannot be
    ///   decoded as `Vec<MessageSummary>`.
    pub(crate) async fn search(
        &mut self,
        query: &str,
        account_id: Option<AccountId>,
    ) -> Result<Vec<SearchHit>, MailboxError> {
        let response = self
            .request("search", search_args(query, account_id))
            .await?;
        let hits: Vec<MessageSummary> = decode_response("search", response)?;
        Ok(hits.into_iter().map(SearchHit::from).collect())
    }

    /// List pending MCP approvals newest-first.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`MailboxError::Request`] if the IPC request itself fails.
    /// - [`MailboxError::Server`] if the daemon returned `ok = false`.
    /// - [`MailboxError::Decode`] if the response payload cannot be
    ///   decoded as `Vec<McpApproval>`.
    pub(crate) async fn list_pending_approvals(
        &mut self,
    ) -> Result<Vec<ApprovalItem>, MailboxError> {
        let response = self
            .request(
                "mcp.approval.list",
                json!({ "state": "pending", "limit": 100, "offset": 0 }),
            )
            .await?;
        let approvals: Vec<McpApproval> = decode_response("mcp.approval.list", response)?;
        Ok(approvals
            .into_iter()
            .filter(|approval| approval.state == ApprovalState::Pending)
            .map(ApprovalItem::from)
            .collect())
    }

    /// Decide a pending MCP approval.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`MailboxError::Request`] if the IPC request itself fails.
    /// - [`MailboxError::Server`] if the daemon returned `ok = false`.
    /// - [`MailboxError::Decode`] if the response payload cannot be
    ///   decoded as the expected decision result.
    pub(crate) async fn decide_approval(
        &mut self,
        approval_id: Uuid,
        state: ApprovalState,
    ) -> Result<bool, MailboxError> {
        let response = self
            .request(
                "mcp.approval.decide",
                approval_decide_args(approval_id, state),
            )
            .await?;
        let result: ApprovalDecisionResult = decode_response("mcp.approval.decide", response)?;
        Ok(result.decided)
    }

    /// Build a reply skeleton (subject, recipients, headers, quoted
    /// body) for `message_id`. `reply_all` controls whether the
    /// original `Cc` recipients are included.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`MailboxError::Request`] if the IPC request itself fails.
    /// - [`MailboxError::Server`] if the daemon returned `ok = false`
    ///   (e.g. unknown message).
    /// - [`MailboxError::Decode`] if the response payload cannot be
    ///   decoded as [`ReplyPrepared`].
    pub(crate) async fn prepare_reply(
        &mut self,
        message_id: MessageId,
        reply_all: bool,
    ) -> Result<ReplyPrepared, MailboxError> {
        let response = self
            .request(
                "message.prepare_reply",
                json!({ "message_id": message_id, "reply_all": reply_all }),
            )
            .await?;
        decode_response("message.prepare_reply", response)
    }

    /// Build a forward skeleton (subject, body, attachment metadata)
    /// for `message_id`.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`MailboxError::Request`] if the IPC request itself fails.
    /// - [`MailboxError::Server`] if the daemon returned `ok = false`
    ///   (e.g. unknown message).
    /// - [`MailboxError::Decode`] if the response payload cannot be
    ///   decoded as [`ForwardPrepared`].
    pub(crate) async fn prepare_forward(
        &mut self,
        message_id: MessageId,
    ) -> Result<ForwardPrepared, MailboxError> {
        let response = self
            .request(
                "message.prepare_forward",
                json!({ "message_id": message_id }),
            )
            .await?;
        decode_response("message.prepare_forward", response)
    }

    /// Fetch the raw bytes (base64) for a single attachment when
    /// preparing a forward.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`MailboxError::Request`] if the IPC request itself fails.
    /// - [`MailboxError::Server`] if the daemon returned `ok = false`
    ///   (e.g. unknown attachment, disk IO failure).
    /// - [`MailboxError::Decode`] if the response payload cannot be
    ///   decoded as [`ForwardAttachmentBytes`].
    pub(crate) async fn fetch_attachment_for_forward(
        &mut self,
        attachment_id: AttachmentId,
    ) -> Result<ForwardAttachmentBytes, MailboxError> {
        let response = self
            .request(
                "attachment.fetch_for_forward",
                json!({ "attachment_id": attachment_id }),
            )
            .await?;
        decode_response("attachment.fetch_for_forward", response)
    }

    /// Fetch raw bytes for a batch of attachments in one request.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`MailboxError::Request`] if the IPC request itself fails.
    /// - [`MailboxError::Server`] if the daemon returned `ok = false`.
    /// - [`MailboxError::Decode`] if the response payload cannot be
    ///   decoded as [`ForwardAttachmentBatch`]. Per-attachment failures
    ///   surface inside the decoded payload, not as errors.
    pub(crate) async fn fetch_attachments_for_forward(
        &mut self,
        message_id: MessageId,
        attachment_ids: &[AttachmentId],
    ) -> Result<ForwardAttachmentBatch, MailboxError> {
        let response = self
            .request(
                "attachment.fetch_for_forward_batch",
                json!({ "message_id": message_id, "attachment_ids": attachment_ids }),
            )
            .await?;
        decode_response("attachment.fetch_for_forward_batch", response)
    }

    /// Subscribe to a daemon event topic. Returns the daemon-allocated
    /// `sub_id` so callers can later unsubscribe if needed.
    ///
    /// # Errors
    ///
    /// Returns [`MailboxError::Request`] if the IPC subscribe request
    /// fails (e.g. the socket dropped or the per-connection
    /// subscription cap was hit).
    pub(crate) async fn subscribe(&mut self, topic: Topic) -> Result<u64, MailboxError> {
        self.client
            .subscribe(topic)
            .await
            .map_err(|source| MailboxError::Request {
                op: "subscribe",
                source,
            })
    }

    /// Pull the next inbound event off the client's event queue.
    ///
    /// # Errors
    ///
    /// Returns [`MailboxError::Request`] if the underlying IPC stream
    /// closed or returned a transport error.
    pub(crate) async fn next_event(&mut self) -> Result<Event, MailboxError> {
        self.client
            .next_event()
            .await
            .map_err(|source| MailboxError::Request {
                op: "next_event",
                source,
            })
    }

    async fn request(
        &mut self,
        op: &'static str,
        args: serde_json::Value,
    ) -> Result<Response, MailboxError> {
        self.client
            .request(op, args)
            .await
            .map_err(|source| MailboxError::Request { op, source })
    }
}

pub(crate) fn decode_response<T>(op: &'static str, response: Response) -> Result<T, MailboxError>
where
    T: DeserializeOwned,
{
    if !response.ok {
        let error = response.error.unwrap_or_else(|| RpcError {
            code: "unknown".into(),
            message: "daemon returned ok=false without an error".into(),
        });
        return Err(MailboxError::Server {
            op,
            code: error.code,
            message: error.message,
        });
    }

    serde_json::from_value(response.data).map_err(|source| MailboxError::Decode { op, source })
}

pub(crate) fn account_folder_args(account_id: AccountId, folder_name: &str) -> Value {
    json!({ "account_id": account_id, "folder_name": folder_name })
}

pub(crate) fn set_flags_args(message_id: MessageId, flags: &[String]) -> Value {
    json!({ "id": message_id, "flags": flags })
}

pub(crate) fn attachment_list_args(message_id: MessageId) -> Value {
    json!({ "message_id": message_id })
}

pub(crate) fn attachment_preview_args(attachment_id: AttachmentId) -> Value {
    json!({ "id": attachment_id })
}

pub(crate) fn attachment_export_args(
    attachment_id: AttachmentId,
    destination_path: &Path,
) -> Value {
    json!({
        "id": attachment_id,
        "destination_path": destination_path.display().to_string(),
    })
}

pub(crate) fn draft_create_args(draft: &ComposerDraft) -> Value {
    json!({
        "account_id": draft.account_id,
        "in_reply_to_msg": draft.in_reply_to_msg,
        "to_addrs": &draft.to_addrs,
        "cc_addrs": &draft.cc_addrs,
        "bcc_addrs": &draft.bcc_addrs,
        "subject": &draft.subject,
        "text_body": &draft.text_body,
        "html_body": &draft.html_body,
        "in_reply_to": &draft.in_reply_to,
        "references_header": &draft.references_header,
        "attachments": draft_attachment_specs(draft),
    })
}

pub(crate) fn draft_update_args(draft_id: DraftId, draft: &ComposerDraft) -> Value {
    json!({
        "id": draft_id,
        "to_addrs": &draft.to_addrs,
        "cc_addrs": &draft.cc_addrs,
        "bcc_addrs": &draft.bcc_addrs,
        "subject": &draft.subject,
        "text_body": &draft.text_body,
        "html_body": &draft.html_body,
        "attachments": draft_attachment_specs(draft),
    })
}

fn draft_attachment_specs(draft: &ComposerDraft) -> Value {
    Value::Array(
        draft
            .attachments
            .iter()
            .map(|a| {
                json!({
                    "path": a.path.display().to_string(),
                    "filename": &a.filename,
                    "content_type": &a.content_type,
                })
            })
            .collect(),
    )
}

pub(crate) fn message_send_args(account_id: AccountId, draft_id: DraftId) -> Value {
    json!({ "account_id": account_id, "draft_id": draft_id })
}

pub(crate) fn search_args(query: &str, account_id: Option<AccountId>) -> Value {
    match account_id {
        Some(account_id) => json!({ "q": query, "account_id": account_id, "limit": 50 }),
        None => json!({ "q": query, "limit": 50 }),
    }
}

/// Build `mcp.approval.decide` args for the TUI actor.
pub(crate) fn approval_decide_args(approval_id: Uuid, state: ApprovalState) -> Value {
    json!({
        "id": approval_id,
        "state": state.as_str(),
        "decided_by": "tui",
    })
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::json;

    use crate::models::{AddressList, Message, MessageFlags, ThreadId};

    use super::*;

    fn message() -> Message {
        let id = MessageId::new();
        let account_id = AccountId::new();
        let folder_id = FolderId::new();
        Message {
            id,
            account_id,
            folder_id,
            thread_id: None,
            uid: 7,
            message_id_header: Some("<7@example.com>".into()),
            in_reply_to: None,
            references_header: None,
            from_addr: "alice@example.com".into(),
            to_addrs: AddressList::from(vec!["bob@example.com"]),
            cc_addrs: AddressList::default(),
            bcc_addrs: AddressList::default(),
            reply_to: None,
            subject: Some("Hello".into()),
            snippet: Some("short preview".into()),
            text_body: Some("full body".into()),
            html_body: None,
            raw_size: 128,
            flags: MessageFlags::from(vec!["\\Seen"]),
            internal_date: Utc::now(),
            sent_at: None,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn test_decode_response_maps_message_to_detail() {
        let original = message();
        let response = Response::ok(1, serde_json::to_value(Some(original)).unwrap());

        let decoded: Option<Message> = decode_response("message.get", response).unwrap();
        let detail = decoded.map(MessageDetail::from).unwrap();

        assert_eq!(detail.subject, "Hello");
        assert_eq!(detail.from, "alice@example.com");
        assert_eq!(detail.snippet, "short preview");
        assert_eq!(detail.body, "full body");
    }

    #[test]
    fn test_message_item_from_preserves_thread_id() {
        let mut original = message();
        let thread_id = ThreadId::new();
        original.thread_id = Some(thread_id);

        let item = MessageItem::from(original);

        assert_eq!(item.thread_id, Some(thread_id));
    }

    #[test]
    fn test_decode_response_preserves_server_error() {
        let response = Response::err(1, RpcError::bad_args("missing folder_id"));

        let err =
            decode_response::<Vec<MessageSummary>>("message.list_by_folder", response).unwrap_err();

        assert!(err.to_string().contains("bad_args"));
        assert!(err.to_string().contains("missing folder_id"));
    }

    #[test]
    fn test_decode_response_reports_malformed_data() {
        let response = Response::ok(1, json!({ "not": "an array" }));

        let err =
            decode_response::<Vec<MessageSummary>>("message.list_by_folder", response).unwrap_err();

        assert!(err.to_string().contains("malformed data"));
    }

    #[test]
    fn test_account_folder_args_match_daemon_write_ops() {
        let account_id = AccountId::new();

        let args = account_folder_args(account_id, "INBOX");

        assert_eq!(
            args,
            json!({
                "account_id": account_id,
                "folder_name": "INBOX",
            })
        );
    }

    #[test]
    fn test_set_flags_args_serializes_complete_flag_list() {
        let message_id = MessageId::new();
        let flags = vec!["\\Answered".to_string(), "\\Seen".to_string()];

        let args = set_flags_args(message_id, &flags);

        assert_eq!(
            args,
            json!({
                "id": message_id,
                "flags": ["\\Answered", "\\Seen"],
            })
        );
    }

    #[test]
    fn test_attachment_args_match_daemon_ops() {
        let message_id = MessageId::new();
        let attachment_id = AttachmentId::new();

        assert_eq!(
            attachment_list_args(message_id),
            json!({ "message_id": message_id })
        );
        assert_eq!(
            attachment_preview_args(attachment_id),
            json!({ "id": attachment_id })
        );
        assert_eq!(
            attachment_export_args(attachment_id, Path::new("/tmp/report.txt")),
            json!({ "id": attachment_id, "destination_path": "/tmp/report.txt" })
        );
    }

    #[test]
    fn test_draft_and_send_args_match_daemon_payloads() {
        let account_id = AccountId::new();
        let draft_id = DraftId::new();
        let draft = super::super::app::ComposerDraft {
            account_id,
            in_reply_to_msg: None,
            to_addrs: vec!["to@example.com".into()],
            cc_addrs: vec!["copy@example.com".into()],
            bcc_addrs: vec!["blind@example.com".into()],
            subject: Some("Hello".into()),
            text_body: Some("Body".into()),
            html_body: None,
            attachments: Vec::new(),
            in_reply_to: None,
            references_header: None,
        };

        assert_eq!(
            draft_create_args(&draft),
            json!({
                "account_id": account_id,
                "in_reply_to_msg": null,
                "to_addrs": ["to@example.com"],
                "cc_addrs": ["copy@example.com"],
                "bcc_addrs": ["blind@example.com"],
                "subject": "Hello",
                "text_body": "Body",
                "html_body": null,
                "in_reply_to": null,
                "references_header": null,
                "attachments": [],
            })
        );
        assert_eq!(
            draft_update_args(draft_id, &draft),
            json!({
                "id": draft_id,
                "to_addrs": ["to@example.com"],
                "cc_addrs": ["copy@example.com"],
                "bcc_addrs": ["blind@example.com"],
                "subject": "Hello",
                "text_body": "Body",
                "html_body": null,
                "attachments": [],
            })
        );
        assert_eq!(
            message_send_args(account_id, draft_id),
            json!({ "account_id": account_id, "draft_id": draft_id })
        );
    }

    #[test]
    fn test_approval_decide_args_match_daemon_payload() {
        let approval_id = Uuid::new_v4();

        assert_eq!(
            approval_decide_args(approval_id, ApprovalState::Allowed),
            json!({
                "id": approval_id,
                "state": "allowed",
                "decided_by": "tui",
            })
        );
        assert_eq!(
            approval_decide_args(approval_id, ApprovalState::Denied),
            json!({
                "id": approval_id,
                "state": "denied",
                "decided_by": "tui",
            })
        );
    }
}
