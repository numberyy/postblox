use std::path::Path;

use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use thiserror::Error;
use uuid::Uuid;

use crate::ipc::client::{Client, ClientError};
use crate::ipc::{Event, Response, RpcError, Topic};
use crate::models::{Account, Attachment, Draft, Folder, Message};

use super::app::{
    AccountItem, AttachmentItem, AttachmentPreviewItem, ComposerDraft, FolderItem, MessageDetail,
    MessageItem,
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct AttachmentExportResult {
    pub attachment_id: Uuid,
    pub destination_path: String,
    pub bytes_copied: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
struct SendResult {
    message_id: String,
}

#[derive(Debug, Error)]
pub enum MailboxError {
    #[error("connect failed: {0}")]
    Connect(#[source] ClientError),
    #[error("{op} request failed: {source}")]
    Request {
        op: &'static str,
        #[source]
        source: ClientError,
    },
    #[error("{op} failed: {code}: {message}")]
    Server {
        op: &'static str,
        code: String,
        message: String,
    },
    #[error("{op} returned malformed data: {source}")]
    Decode {
        op: &'static str,
        #[source]
        source: serde_json::Error,
    },
}

pub struct MailboxClient {
    client: Client,
}

impl MailboxClient {
    pub async fn connect(path: &Path) -> Result<Self, MailboxError> {
        Client::connect(path)
            .await
            .map(|client| Self { client })
            .map_err(MailboxError::Connect)
    }

    pub async fn list_accounts(&mut self) -> Result<Vec<AccountItem>, MailboxError> {
        let response = self.request("account.list", json!({})).await?;
        let accounts: Vec<Account> = decode_response("account.list", response)?;
        Ok(accounts.into_iter().map(AccountItem::from).collect())
    }

    pub async fn list_folders(
        &mut self,
        account_id: Uuid,
    ) -> Result<Vec<FolderItem>, MailboxError> {
        let response = self
            .request("folder.list", json!({ "account_id": account_id }))
            .await?;
        let folders: Vec<Folder> = decode_response("folder.list", response)?;
        Ok(folders.into_iter().map(FolderItem::from).collect())
    }

    pub async fn list_messages(
        &mut self,
        folder_id: Uuid,
    ) -> Result<Vec<MessageItem>, MailboxError> {
        let response = self
            .request(
                "message.list_by_folder",
                json!({ "folder_id": folder_id, "limit": 100, "offset": 0 }),
            )
            .await?;
        let messages: Vec<Message> = decode_response("message.list_by_folder", response)?;
        Ok(messages.into_iter().map(MessageItem::from).collect())
    }

    pub async fn get_message(
        &mut self,
        message_id: Uuid,
    ) -> Result<Option<MessageDetail>, MailboxError> {
        let response = self
            .request("message.get", json!({ "id": message_id }))
            .await?;
        let message: Option<Message> = decode_response("message.get", response)?;
        Ok(message.map(MessageDetail::from))
    }

    pub async fn sync_folder(
        &mut self,
        account_id: Uuid,
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

    pub async fn start_sync(
        &mut self,
        account_id: Uuid,
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

    pub async fn stop_sync(
        &mut self,
        account_id: Uuid,
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

    pub async fn set_flags(
        &mut self,
        message_id: Uuid,
        flags: &[String],
    ) -> Result<(), MailboxError> {
        let response = self
            .request("message.set_flags", set_flags_args(message_id, flags))
            .await?;
        let _: Value = decode_response("message.set_flags", response)?;
        Ok(())
    }

    pub async fn archive_message(&mut self, message_id: Uuid) -> Result<(), MailboxError> {
        let response = self
            .request("message.archive", json!({ "id": message_id }))
            .await?;
        let _: Value = decode_response("message.archive", response)?;
        Ok(())
    }

    pub async fn delete_message(&mut self, message_id: Uuid) -> Result<(), MailboxError> {
        let response = self
            .request("message.delete", json!({ "id": message_id }))
            .await?;
        let _: Value = decode_response("message.delete", response)?;
        Ok(())
    }

    pub async fn move_message(
        &mut self,
        message_id: Uuid,
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

    pub async fn list_attachments(
        &mut self,
        message_id: Uuid,
    ) -> Result<Vec<AttachmentItem>, MailboxError> {
        let response = self
            .request("attachment.list", attachment_list_args(message_id))
            .await?;
        let attachments: Vec<Attachment> = decode_response("attachment.list", response)?;
        Ok(attachments.into_iter().map(AttachmentItem::from).collect())
    }

    pub async fn preview_attachment(
        &mut self,
        attachment_id: Uuid,
    ) -> Result<AttachmentPreviewItem, MailboxError> {
        let response = self
            .request("attachment.preview", attachment_preview_args(attachment_id))
            .await?;
        let preview: crate::attachments::AttachmentPreview =
            decode_response("attachment.preview", response)?;
        Ok(AttachmentPreviewItem::from(preview))
    }

    pub async fn export_attachment(
        &mut self,
        attachment_id: Uuid,
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

    pub async fn create_draft(&mut self, draft: &ComposerDraft) -> Result<Uuid, MailboxError> {
        let response = self
            .request("draft.create", draft_create_args(draft))
            .await?;
        let draft: Draft = decode_response("draft.create", response)?;
        Ok(draft.id)
    }

    pub async fn update_draft(
        &mut self,
        draft_id: Uuid,
        draft: &ComposerDraft,
    ) -> Result<Uuid, MailboxError> {
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

    pub async fn send_draft(
        &mut self,
        account_id: Uuid,
        draft_id: Uuid,
    ) -> Result<String, MailboxError> {
        let response = self
            .request("message.send", message_send_args(account_id, draft_id))
            .await?;
        let sent: SendResult = decode_response("message.send", response)?;
        Ok(sent.message_id)
    }

    /// Subscribe to a daemon event topic. Returns the daemon-allocated
    /// `sub_id` so callers can later unsubscribe if needed.
    pub async fn subscribe(&mut self, topic: Topic) -> Result<u64, MailboxError> {
        self.client
            .subscribe(topic)
            .await
            .map_err(|source| MailboxError::Request {
                op: "subscribe",
                source,
            })
    }

    /// Pull the next inbound event off the client's event queue.
    pub async fn next_event(&mut self) -> Result<Event, MailboxError> {
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

pub(crate) fn account_folder_args(account_id: Uuid, folder_name: &str) -> Value {
    json!({ "account_id": account_id, "folder_name": folder_name })
}

pub(crate) fn set_flags_args(message_id: Uuid, flags: &[String]) -> Value {
    json!({ "id": message_id, "flags": flags })
}

pub(crate) fn attachment_list_args(message_id: Uuid) -> Value {
    json!({ "message_id": message_id })
}

pub(crate) fn attachment_preview_args(attachment_id: Uuid) -> Value {
    json!({ "id": attachment_id })
}

pub(crate) fn attachment_export_args(attachment_id: Uuid, destination_path: &Path) -> Value {
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
    })
}

pub(crate) fn draft_update_args(draft_id: Uuid, draft: &ComposerDraft) -> Value {
    json!({
        "id": draft_id,
        "to_addrs": &draft.to_addrs,
        "cc_addrs": &draft.cc_addrs,
        "bcc_addrs": &draft.bcc_addrs,
        "subject": &draft.subject,
        "text_body": &draft.text_body,
        "html_body": &draft.html_body,
    })
}

pub(crate) fn message_send_args(account_id: Uuid, draft_id: Uuid) -> Value {
    json!({ "account_id": account_id, "draft_id": draft_id })
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::json;

    use crate::models::Message;

    use super::*;

    fn message() -> Message {
        let id = Uuid::new_v4();
        let account_id = Uuid::new_v4();
        let folder_id = Uuid::new_v4();
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
            to_addrs: json!(["bob@example.com"]),
            cc_addrs: json!([]),
            bcc_addrs: json!([]),
            reply_to: None,
            subject: Some("Hello".into()),
            snippet: Some("short preview".into()),
            text_body: Some("full body".into()),
            html_body: None,
            raw_size: 128,
            flags: json!(["\\Seen"]),
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
        let thread_id = Uuid::new_v4();
        original.thread_id = Some(thread_id);

        let item = MessageItem::from(original);

        assert_eq!(item.thread_id, Some(thread_id));
    }

    #[test]
    fn test_decode_response_preserves_server_error() {
        let response = Response::err(1, RpcError::bad_args("missing folder_id"));

        let err = decode_response::<Vec<Message>>("message.list_by_folder", response).unwrap_err();

        assert!(err.to_string().contains("bad_args"));
        assert!(err.to_string().contains("missing folder_id"));
    }

    #[test]
    fn test_decode_response_reports_malformed_data() {
        let response = Response::ok(1, json!({ "not": "an array" }));

        let err = decode_response::<Vec<Message>>("message.list_by_folder", response).unwrap_err();

        assert!(err.to_string().contains("malformed data"));
    }

    #[test]
    fn test_account_folder_args_match_daemon_write_ops() {
        let account_id = Uuid::new_v4();

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
        let message_id = Uuid::new_v4();
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
        let message_id = Uuid::new_v4();
        let attachment_id = Uuid::new_v4();

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
        let account_id = Uuid::new_v4();
        let draft_id = Uuid::new_v4();
        let draft = super::super::app::ComposerDraft {
            account_id,
            in_reply_to_msg: None,
            to_addrs: vec!["to@example.com".into()],
            cc_addrs: vec!["copy@example.com".into()],
            bcc_addrs: vec!["blind@example.com".into()],
            subject: Some("Hello".into()),
            text_body: Some("Body".into()),
            html_body: None,
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
            })
        );
        assert_eq!(
            message_send_args(account_id, draft_id),
            json!({ "account_id": account_id, "draft_id": draft_id })
        );
    }
}
