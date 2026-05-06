use std::path::Path;

use serde::de::DeserializeOwned;
use serde_json::json;
use thiserror::Error;
use uuid::Uuid;

use crate::ipc::client::{Client, ClientError};
use crate::ipc::{Response, RpcError};
use crate::models::{Account, Folder, Message};

use super::app::{AccountItem, FolderItem, MessageDetail, MessageItem};

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
}
