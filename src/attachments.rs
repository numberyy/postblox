//! Attachment persistence, preview, and export helpers.
//!
//! Bridges the parser output ([`crate::mail::parser::ParsedAttachment`])
//! and the on-disk attachment store rooted under the configured data
//! directory. Persists rows through [`crate::db::attachments`], enforces
//! the 25 MiB per-attachment limit from `CLAUDE.md`, and renders bounded
//! UTF-8 previews capped at [`PREVIEW_LIMIT_BYTES`]. Filenames are
//! sanitised before they touch the filesystem so untrusted MIME headers
//! cannot escape the storage root.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::db::{self, DbError};
use crate::mail::parser::{Disposition, ParsedAttachment};
use crate::models::{Attachment, AttachmentDisposition, AttachmentId, MessageId};

/// Maximum bytes returned in an inline attachment preview.
pub const PREVIEW_LIMIT_BYTES: usize = 16 * 1024;
const MAX_ATTACHMENT_BYTES: usize = 25 * 1024 * 1024;

/// Error returned by attachment persistence, preview, and export
/// helpers.
#[derive(Debug, Error)]
pub enum AttachmentError {
    /// Attachment exceeded the per-attachment size limit.
    #[error("attachment '{filename}' exceeds {limit} byte limit")]
    TooLarge {
        /// Name of the offending attachment.
        filename: String,
        /// Configured byte limit.
        limit: usize,
    },
    /// Computed storage path could not be resolved on the filesystem.
    #[error("invalid attachment path: {}", path.display())]
    BadPath {
        /// Path that failed to resolve.
        path: PathBuf,
    },
    /// Underlying database error.
    #[error("db: {0}")]
    Db(#[from] DbError),
    /// Underlying filesystem I/O error.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Bounded, agent-friendly preview of an attachment's contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentPreview {
    /// Database row describing the previewed attachment.
    pub attachment: Attachment,
    /// Inline UTF-8 text payload, when one could be rendered.
    pub inline_text: Option<String>,
    /// Human-readable summary message for the preview.
    pub message: String,
    /// Whether the inline text was truncated to fit the preview cap.
    pub truncated: bool,
    /// Number of bytes returned in `inline_text`.
    pub preview_bytes: usize,
}

/// Result of an attachment export to a caller-supplied destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentExport {
    /// Identifier of the exported attachment.
    pub attachment_id: AttachmentId,
    /// Filesystem path the bytes were written to.
    pub destination_path: String,
    /// Number of bytes copied.
    pub bytes_copied: u64,
}

pub(crate) fn guess_content_type_for_path(path: &Path) -> String {
    mime_guess::from_path(path)
        .first_raw()
        .unwrap_or("application/octet-stream")
        .to_string()
}

/// Persist parsed attachments for `message_id` to the on-disk store and
/// insert the matching rows via [`crate::db::attachments`].
///
/// # Errors
///
/// Returns:
/// - [`AttachmentError::TooLarge`] if any attachment exceeds the 25 MiB
///   per-attachment limit defined by `CLAUDE.md`.
/// - [`AttachmentError::BadPath`] if the computed storage path has no
///   parent directory to create.
/// - [`AttachmentError::Io`] if creating the directory or writing the
///   bytes fails (including the `create_new` collision when a file with
///   the same name already exists).
/// - [`AttachmentError::Db`] wrapping [`crate::db::DbError`] if the row
///   insert or the `PRAGMA database_list` lookup used to derive the
///   storage root fails.
pub async fn persist_parsed_for_message(
    pool: &SqlitePool,
    message_id: MessageId,
    attachments: &[ParsedAttachment],
) -> Result<Vec<Attachment>, AttachmentError> {
    if attachments.is_empty() {
        return Ok(Vec::new());
    }

    let root = attachment_root(pool).await?;
    let mut stored = Vec::with_capacity(attachments.len());
    for (index, attachment) in attachments.iter().enumerate() {
        if attachment.data.len() > MAX_ATTACHMENT_BYTES {
            return Err(AttachmentError::TooLarge {
                filename: attachment.filename.clone(),
                limit: MAX_ATTACHMENT_BYTES,
            });
        }

        let path = stored_attachment_path(&root, message_id, index, &attachment.filename);
        write_new_file(&path, &attachment.data).await?;
        let row = db::attachments::create(
            pool,
            &db::attachments::NewAttachment {
                message_id,
                filename: sanitize_filename(&attachment.filename),
                content_type: attachment.content_type.clone(),
                content_id: attachment.content_id.clone(),
                size_bytes: attachment.data.len() as i64,
                disposition: match attachment.disposition {
                    Disposition::Inline => AttachmentDisposition::Inline,
                    Disposition::Attachment => AttachmentDisposition::Attachment,
                },
                storage_path: path.display().to_string(),
            },
        )
        .await?;
        stored.push(row);
    }
    Ok(stored)
}

/// Render a bounded inline preview for `attachment`, capped at
/// [`PREVIEW_LIMIT_BYTES`].
///
/// # Errors
///
/// Returns [`std::io::Error`] if `attachment.storage_path` cannot be
/// opened or read. Non-text or non-UTF-8 content is reported through
/// the returned [`AttachmentPreview`] (with `inline_text = None`), not
/// as an error.
pub async fn preview_attachment(
    attachment: Attachment,
) -> Result<AttachmentPreview, std::io::Error> {
    if !is_text_like(&attachment.content_type) {
        return Ok(AttachmentPreview {
            message: format!(
                "No inline preview for {} attachment ({} bytes)",
                attachment.content_type, attachment.size_bytes
            ),
            attachment,
            inline_text: None,
            truncated: false,
            preview_bytes: 0,
        });
    }

    let mut file = tokio::fs::File::open(&attachment.storage_path).await?;
    let mut buf = vec![0; PREVIEW_LIMIT_BYTES + 1];
    let read = file.read(&mut buf).await?;
    let truncated = read > PREVIEW_LIMIT_BYTES
        || usize::try_from(attachment.size_bytes).unwrap_or(usize::MAX) > PREVIEW_LIMIT_BYTES;
    buf.truncate(read.min(PREVIEW_LIMIT_BYTES));

    match String::from_utf8(buf) {
        Ok(text) => Ok(AttachmentPreview {
            attachment,
            preview_bytes: text.len(),
            inline_text: Some(text),
            message: if truncated {
                format!("Inline UTF-8 preview (first {PREVIEW_LIMIT_BYTES} bytes)")
            } else {
                "Inline UTF-8 preview".into()
            },
            truncated,
        }),
        Err(_) => Ok(AttachmentPreview {
            message: format!(
                "No inline preview: {} attachment is not valid UTF-8",
                attachment.content_type
            ),
            attachment,
            inline_text: None,
            truncated: false,
            preview_bytes: 0,
        }),
    }
}

/// Copy the stored bytes of `attachment` to `destination_path`,
/// refusing to overwrite an existing file.
///
/// # Errors
///
/// Returns [`std::io::Error`] if:
/// - `destination_path` already exists ([`std::io::ErrorKind::AlreadyExists`]).
/// - The destination's parent directory does not exist
///   ([`std::io::ErrorKind::NotFound`]).
/// - Opening `attachment.storage_path`, creating the destination, or
///   copying/flushing the bytes fails for any other IO reason.
pub async fn export_attachment(
    attachment: &Attachment,
    destination_path: &Path,
) -> Result<AttachmentExport, std::io::Error> {
    if destination_path.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("destination exists: {}", destination_path.display()),
        ));
    }
    if let Some(parent) = destination_path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("destination parent does not exist: {}", parent.display()),
            ));
        }
    }

    let mut source = tokio::fs::File::open(&attachment.storage_path).await?;
    let mut destination = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination_path)
        .await?;
    let bytes_copied = tokio::io::copy(&mut source, &mut destination).await?;
    destination.flush().await?;

    Ok(AttachmentExport {
        attachment_id: attachment.id,
        destination_path: destination_path.display().to_string(),
        bytes_copied,
    })
}

async fn attachment_root(pool: &SqlitePool) -> Result<PathBuf, AttachmentError> {
    if let Some(path) = std::env::var_os("POSTBLOX_ATTACHMENTS").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }

    let rows = sqlx::query("PRAGMA database_list")
        .fetch_all(pool)
        .await
        .map_err(DbError::from)?;
    for row in rows {
        let name: String = row.try_get("name").map_err(DbError::from)?;
        let file: String = row.try_get("file").map_err(DbError::from)?;
        if name == "main" && !file.is_empty() {
            if let Some(parent) = Path::new(&file).parent() {
                return Ok(parent.join("attachments"));
            }
        }
    }

    if let Some(data_dir) = dirs::data_local_dir() {
        Ok(data_dir.join("postblox").join("attachments"))
    } else {
        Ok(std::env::current_dir().map(|dir| dir.join("attachments"))?)
    }
}

fn stored_attachment_path(
    root: &Path,
    message_id: MessageId,
    index: usize,
    filename: &str,
) -> PathBuf {
    root.join(message_id.to_string()).join(format!(
        "{:03}_{}",
        index + 1,
        sanitize_filename(filename)
    ))
}

fn sanitize_filename(filename: &str) -> String {
    let leaf = Path::new(filename)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(filename);
    let mut out = String::new();
    for ch in leaf.chars().take(120) {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | ' ') {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    let out = out.trim_matches(['.', ' ']).trim();
    if out.is_empty() {
        "attachment.bin".into()
    } else {
        out.to_string()
    }
}

async fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), AttachmentError> {
    let parent = path.parent().ok_or_else(|| AttachmentError::BadPath {
        path: path.to_path_buf(),
    })?;
    tokio::fs::create_dir_all(parent).await?;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .await?;
    file.write_all(bytes).await?;
    file.flush().await?;
    Ok(())
}

fn is_text_like(content_type: &str) -> bool {
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    media_type.starts_with("text/")
        || matches!(
            media_type.as_str(),
            "application/json"
                | "application/xml"
                | "application/csv"
                | "application/x-ndjson"
                | "image/svg+xml"
        )
        || media_type.ends_with("+json")
        || media_type.ends_with("+xml")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{accounts, folders, messages, test_pool};
    use crate::models::{AddressList, AuthKind, FolderRole, MessageFlags};
    use chrono::Utc;
    use uuid::Uuid;

    async fn message_id_for_test(pool: &SqlitePool) -> MessageId {
        let account = accounts::create(
            pool,
            &accounts::NewAccount {
                email: format!("u-{}@x.com", Uuid::new_v4()),
                display_name: None,
                auth_kind: AuthKind::Password,
                imap_host: "imap.example.com".into(),
                imap_port: 993,
                imap_use_tls: true,
                smtp_host: "smtp.example.com".into(),
                smtp_port: 465,
                smtp_use_tls: true,
                smtp_starttls: false,
            },
        )
        .await
        .unwrap();
        let folder = folders::create(
            pool,
            &folders::NewFolder {
                account_id: account.id,
                name: "INBOX".into(),
                delimiter: "/".into(),
                role: FolderRole::Inbox,
                selectable: true,
            },
        )
        .await
        .unwrap();
        messages::create(
            pool,
            &messages::NewMessage {
                account_id: account.id,
                folder_id: folder.id,
                thread_id: None,
                uid: 1,
                message_id_header: Some("attachment-test@example.com".into()),
                in_reply_to: None,
                references_header: None,
                from_addr: "alice@example.com".into(),
                to_addrs: AddressList::from(vec!["bob@example.com"]),
                cc_addrs: AddressList::default(),
                bcc_addrs: AddressList::default(),
                reply_to: None,
                subject: Some("attached".into()),
                snippet: None,
                text_body: Some("body".into()),
                html_body: None,
                raw_size: 1,
                flags: MessageFlags::default(),
                internal_date: Utc::now(),
                sent_at: None,
            },
        )
        .await
        .unwrap()
        .id
    }

    #[test]
    fn test_is_text_like_allows_json_svg_and_text() {
        assert!(is_text_like("text/plain; charset=utf-8"));
        assert!(is_text_like("application/json"));
        assert!(is_text_like("application/problem+json"));
        assert!(is_text_like("image/svg+xml"));
        assert!(!is_text_like("image/png"));
        assert!(!is_text_like("application/octet-stream"));
    }

    #[test]
    fn test_guess_content_type_for_path_uses_extension_and_fallback() {
        assert_eq!(
            guess_content_type_for_path(Path::new("message.txt")),
            "text/plain"
        );
        assert_eq!(
            guess_content_type_for_path(Path::new("message.unknown-extension")),
            "application/octet-stream"
        );
    }

    #[tokio::test]
    async fn test_preview_text_caps_utf8_and_marks_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("long.txt");
        let content = "a".repeat(PREVIEW_LIMIT_BYTES + 4);
        tokio::fs::write(&path, content.as_bytes()).await.unwrap();
        let attachment = Attachment {
            id: AttachmentId::new(),
            message_id: MessageId::new(),
            filename: "long.txt".into(),
            content_type: "text/plain".into(),
            content_id: None,
            size_bytes: content.len() as i64,
            disposition: AttachmentDisposition::Attachment,
            storage_path: path.display().to_string(),
            created_at: Utc::now(),
        };

        let preview = preview_attachment(attachment).await.unwrap();

        assert_eq!(preview.inline_text.unwrap().len(), PREVIEW_LIMIT_BYTES);
        assert!(preview.truncated);
    }

    #[tokio::test]
    async fn test_preview_binary_refuses_inline_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("img.png");
        tokio::fs::write(&path, [0, 159, 146, 150]).await.unwrap();
        let attachment = Attachment {
            id: AttachmentId::new(),
            message_id: MessageId::new(),
            filename: "img.png".into(),
            content_type: "image/png".into(),
            content_id: None,
            size_bytes: 4,
            disposition: AttachmentDisposition::Attachment,
            storage_path: path.display().to_string(),
            created_at: Utc::now(),
        };

        let preview = preview_attachment(attachment).await.unwrap();

        assert!(preview.inline_text.is_none());
        assert!(preview.message.contains("No inline preview"));
    }

    #[tokio::test]
    async fn test_persist_parsed_for_message_saves_bytes_and_metadata() {
        let pool = test_pool().await;
        let message_id = message_id_for_test(&pool).await;
        let attachments = vec![ParsedAttachment {
            filename: "../unsafe/name.txt".into(),
            content_type: "text/plain".into(),
            data: b"safe text".to_vec(),
            disposition: Disposition::Attachment,
            content_id: None,
        }];

        let stored = persist_parsed_for_message(&pool, message_id, &attachments)
            .await
            .unwrap();

        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].filename, "name.txt");
        assert_eq!(
            tokio::fs::read_to_string(&stored[0].storage_path)
                .await
                .unwrap(),
            "safe text"
        );
    }

    #[tokio::test]
    async fn test_export_refuses_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.txt");
        let destination = dir.path().join("destination.txt");
        tokio::fs::write(&source, b"source").await.unwrap();
        tokio::fs::write(&destination, b"existing").await.unwrap();
        let attachment = Attachment {
            id: AttachmentId::new(),
            message_id: MessageId::new(),
            filename: "source.txt".into(),
            content_type: "text/plain".into(),
            content_id: None,
            size_bytes: 6,
            disposition: AttachmentDisposition::Attachment,
            storage_path: source.display().to_string(),
            created_at: Utc::now(),
        };

        let err = export_attachment(&attachment, &destination)
            .await
            .unwrap_err();

        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(
            tokio::fs::read_to_string(&destination).await.unwrap(),
            "existing"
        );
    }
}
