//! ratatui-based TUI app state machine that talks to the daemon over
//! the IPC socket.
//!
//! [`AppState`] is the single source of truth for what the four-pane
//! mail client is showing: accounts, folders, threads, messages, plus
//! the search/attachments/composer overlays and the toast queue.
//! Keystrokes drive state transitions, and the command bar (`:`-mode)
//! routes through [`super::command`]. All daemon I/O — list ops,
//! write-throughs, event subscriptions — is funnelled through
//! [`super::ipc`]; this module never touches `tokio::net` directly.
//! Bounds (`MAX_TOASTS`, `MAX_COMMAND_CHARS`, `MAX_COMPOSE_*`) match
//! the daemon-side limits so the UI rejects oversize inputs before
//! round-tripping.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::models::{
    Account, AccountId, AddressList, Attachment, AttachmentId, Draft, DraftId, Folder, FolderId,
    Message, MessageFlags, MessageId, MessageSummary, ThreadId,
};

use super::theme::ThemeName;

pub(crate) const SEEN_FLAG: &str = "\\Seen";
pub(crate) const FLAGGED_FLAG: &str = "\\Flagged";
pub(crate) const MAX_COMMAND_CHARS: usize = 128;
pub(crate) const MAX_COMPOSE_HEADER_CHARS: usize = 4096;
pub(crate) const MAX_COMPOSE_BODY_CHARS: usize = 100_000;

/// Maximum bytes for any single compose attachment AND the per-draft
/// aggregate. Mirrors the daemon-side limit (`CLAUDE.md`: "Attachment
/// size: max 25 MB") so the TUI can reject before round-tripping.
pub(crate) const MAX_COMPOSE_ATTACHMENT_BYTES: u64 = 25 * 1024 * 1024;

/// Maximum chars accepted in the inline path-input prompt opened with
/// `Ctrl-A` while composing.
pub(crate) const MAX_COMPOSE_PATH_CHARS: usize = 4096;

/// Maximum number of simultaneously visible toasts. Pushing past this
/// drops the oldest toast.
pub(crate) const MAX_TOASTS: usize = 3;

/// TTL for non-error toasts.
pub(crate) const TOAST_TTL_INFO: Duration = Duration::from_secs(3);
/// TTL for error toasts. Errors stick around longer so they don't get
/// missed when several land at once.
pub(crate) const TOAST_TTL_ERROR: Duration = Duration::from_secs(6);

/// Coalescing windows. Identical text from the same source within the
/// window refreshes the existing toast's expiry instead of pushing a
/// duplicate.
pub(crate) const COALESCE_ACCOUNT_SYNCED: Duration = Duration::from_secs(5);
pub(crate) const COALESCE_SYNC_ERROR: Duration = Duration::from_secs(10);

/// Status pane icons.
pub(crate) const ICON_IDLE: &str = "●";
pub(crate) const ICON_POLLING: &str = "~";
pub(crate) const ICON_SYNCING: &str = "…";
pub(crate) const ICON_ERROR: &str = "!";

/// Maximum chars of `last_error` to render after the selected
/// account's status icon.
pub(crate) const MAX_SELECTED_ERROR_CHARS: usize = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivePane {
    Accounts,
    Folders,
    Threads,
    Messages,
    Details,
    Attachments,
    Search,
}

impl ActivePane {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Accounts => Self::Folders,
            Self::Folders => Self::Threads,
            Self::Threads => Self::Messages,
            Self::Messages => Self::Details,
            Self::Details => Self::Attachments,
            Self::Attachments => Self::Search,
            Self::Search => Self::Accounts,
        }
    }

    pub(crate) fn previous(self) -> Self {
        match self {
            Self::Accounts => Self::Search,
            Self::Folders => Self::Accounts,
            Self::Threads => Self::Folders,
            Self::Messages => Self::Threads,
            Self::Details => Self::Messages,
            Self::Attachments => Self::Details,
            Self::Search => Self::Attachments,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Command,
    Compose,
    ComposeAttachPath,
    ConfirmDiscard,
    ConfirmDelete,
    QuickSearch,
}

/// Maximum chars accepted in the `/` quick-search input.
pub(crate) const MAX_SEARCH_CHARS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposeField {
    To,
    Cc,
    Bcc,
    Subject,
    Body,
}

impl ComposeField {
    fn next(self) -> Self {
        match self {
            Self::To => Self::Cc,
            Self::Cc => Self::Bcc,
            Self::Bcc => Self::Subject,
            Self::Subject => Self::Body,
            Self::Body => Self::To,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::To => Self::Body,
            Self::Cc => Self::To,
            Self::Bcc => Self::Cc,
            Self::Subject => Self::Bcc,
            Self::Body => Self::Subject,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountItem {
    pub id: AccountId,
    pub label: String,
    pub email: String,
    pub status: String,
}

impl From<Account> for AccountItem {
    fn from(account: Account) -> Self {
        let label = account
            .display_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&account.email)
            .to_string();
        Self {
            id: account.id,
            label,
            email: account.email,
            status: account.sync_status.as_str().to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderItem {
    pub id: FolderId,
    pub name: String,
    pub role: String,
}

impl From<Folder> for FolderItem {
    fn from(folder: Folder) -> Self {
        Self {
            id: folder.id,
            name: folder.name,
            role: folder.role.as_str().to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageItem {
    pub(crate) id: MessageId,
    pub(crate) thread_id: Option<ThreadId>,
    pub(crate) subject: String,
    pub(crate) from: String,
    pub(crate) date: String,
    pub(crate) snippet: String,
    pub(crate) flags: Vec<String>,
}

impl From<Message> for MessageItem {
    fn from(message: Message) -> Self {
        let subject = text_or_default(message.subject.as_deref(), "(no subject)");
        let snippet = text_or_default(message.snippet.as_deref(), "");
        let flags = flags_from_value(&message.flags);
        Self {
            id: message.id,
            thread_id: message.thread_id,
            subject,
            from: message.from_addr,
            date: message.internal_date.format("%Y-%m-%d %H:%M").to_string(),
            snippet,
            flags,
        }
    }
}

impl From<MessageSummary> for MessageItem {
    fn from(message: MessageSummary) -> Self {
        let subject = text_or_default(message.subject.as_deref(), "(no subject)");
        let snippet = text_or_default(message.snippet.as_deref(), "");
        let flags = flags_from_value(&message.flags);
        Self {
            id: message.id,
            thread_id: message.thread_id,
            subject,
            from: message.from_addr,
            date: message.internal_date.format("%Y-%m-%d %H:%M").to_string(),
            snippet,
            flags,
        }
    }
}

impl MessageItem {
    pub(crate) fn has_flag(&self, flag: &str) -> bool {
        has_flag(&self.flags, flag)
    }

    pub(crate) fn with_flag(&self, flag: &str, enabled: bool) -> Vec<String> {
        set_flag_preserving(&self.flags, flag, enabled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadItem {
    pub(crate) key: Uuid,
    pub(crate) thread_id: Option<ThreadId>,
    pub(crate) subject: String,
    pub(crate) message_count: usize,
    pub(crate) latest_date: String,
    pub(crate) unread: bool,
    pub(crate) flagged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageDetail {
    pub(crate) id: MessageId,
    pub(crate) subject: String,
    pub(crate) from: String,
    pub(crate) snippet: String,
    pub(crate) body: String,
    pub(crate) flags: Vec<String>,
}

impl From<Message> for MessageDetail {
    fn from(message: Message) -> Self {
        let subject = text_or_default(message.subject.as_deref(), "(no subject)");
        let snippet = text_or_default(message.snippet.as_deref(), "");
        let body = message
            .text_body
            .as_deref()
            .or(message.html_body.as_deref())
            .or(message.snippet.as_deref())
            .unwrap_or("")
            .to_string();
        Self {
            id: message.id,
            subject,
            from: message.from_addr,
            snippet,
            body,
            flags: flags_from_value(&message.flags),
        }
    }
}

/// One row in the Drafts pane (shown when a Drafts folder is active).
/// Mirrors the `MessageItem` shape so the messages list renderer can
/// reuse the same widget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftItem {
    pub(crate) id: DraftId,
    pub(crate) account_id: AccountId,
    pub(crate) subject: String,
    pub(crate) to: String,
    pub(crate) date: String,
    pub(crate) snippet: String,
}

impl From<Draft> for DraftItem {
    fn from(draft: Draft) -> Self {
        let subject = text_or_default(draft.subject.as_deref(), "(no subject)");
        let to = addrs_label(&draft.to_addrs);
        let snippet = first_line_or_default(draft.text_body.as_deref());
        Self {
            id: draft.id,
            account_id: draft.account_id,
            subject,
            to,
            date: draft.updated_at.format("%Y-%m-%d %H:%M").to_string(),
            snippet,
        }
    }
}

/// Decoded `draft.get` payload re-shaped for the composer reopen
/// path. The byte payloads stay base64-encoded until `enter_composer_for_draft`
/// materialises them into temp files.
#[derive(Debug, Clone, PartialEq)]
pub struct DraftSummary {
    pub(crate) draft: Draft,
    pub(crate) attachments: Vec<DraftAttachmentBytes>,
}

impl From<crate::tui::ipc::DraftGetResult> for DraftSummary {
    fn from(payload: crate::tui::ipc::DraftGetResult) -> Self {
        Self {
            draft: payload.draft,
            attachments: payload
                .attachments
                .into_iter()
                .map(DraftAttachmentBytes::from)
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftAttachmentBytes {
    pub(crate) filename: String,
    pub(crate) content_type: String,
    pub(crate) size_bytes: i64,
    pub(crate) bytes: Option<Vec<u8>>,
    pub(crate) decode_error: Option<String>,
}

impl From<crate::tui::ipc::DraftAttachmentPayload> for DraftAttachmentBytes {
    fn from(payload: crate::tui::ipc::DraftAttachmentPayload) -> Self {
        let decoded = payload.decoded_bytes();
        let (bytes, decode_error) = match decoded {
            Ok(bytes) => (Some(bytes), None),
            Err(error) => (None, Some(error.to_string())),
        };
        Self {
            filename: payload.filename,
            content_type: payload.content_type,
            size_bytes: payload.size_bytes,
            bytes,
            decode_error,
        }
    }
}

/// One row returned by the `search` op, projected for the search pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub(crate) message_id: MessageId,
    pub(crate) account_id: AccountId,
    pub(crate) folder_id: FolderId,
    pub(crate) subject: String,
    pub(crate) from: String,
    pub(crate) snippet: String,
    pub(crate) date: String,
}

impl From<Message> for SearchHit {
    fn from(message: Message) -> Self {
        let subject = text_or_default(message.subject.as_deref(), "(no subject)");
        let snippet = text_or_default(message.snippet.as_deref(), "");
        Self {
            message_id: message.id,
            account_id: message.account_id,
            folder_id: message.folder_id,
            subject,
            from: message.from_addr,
            snippet,
            date: message.internal_date.format("%Y-%m-%d %H:%M").to_string(),
        }
    }
}

impl From<MessageSummary> for SearchHit {
    fn from(message: MessageSummary) -> Self {
        let subject = text_or_default(message.subject.as_deref(), "(no subject)");
        let snippet = text_or_default(message.snippet.as_deref(), "");
        Self {
            message_id: message.id,
            account_id: message.account_id,
            folder_id: message.folder_id,
            subject,
            from: message.from_addr,
            snippet,
            date: message.internal_date.format("%Y-%m-%d %H:%M").to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchState {
    pub(crate) query: String,
    pub(crate) scope_account: Option<AccountId>,
    pub(crate) hits: Vec<SearchHit>,
    pub(crate) selected: usize,
    pub(crate) pending: bool,
    /// Pane to restore when the user closes search via Esc.
    pub(crate) previous_pane: ActivePane,
}

impl SearchState {
    pub(crate) fn new(
        query: impl Into<String>,
        scope_account: Option<AccountId>,
        previous_pane: ActivePane,
    ) -> Self {
        Self {
            query: query.into(),
            scope_account,
            hits: Vec::new(),
            selected: 0,
            pending: true,
            previous_pane,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentItem {
    pub(crate) id: AttachmentId,
    pub(crate) message_id: MessageId,
    pub(crate) filename: String,
    pub(crate) content_type: String,
    pub(crate) size_bytes: i64,
    pub(crate) disposition: String,
    pub(crate) storage_path: String,
}

impl From<Attachment> for AttachmentItem {
    fn from(attachment: Attachment) -> Self {
        Self {
            id: attachment.id,
            message_id: attachment.message_id,
            filename: attachment.filename,
            content_type: attachment.content_type,
            size_bytes: attachment.size_bytes,
            disposition: attachment.disposition.as_str().to_string(),
            storage_path: attachment.storage_path,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentPreviewItem {
    pub(crate) attachment_id: AttachmentId,
    pub(crate) text: Option<String>,
    pub(crate) message: String,
    pub(crate) truncated: bool,
    pub(crate) preview_bytes: usize,
}

impl From<crate::attachments::AttachmentPreview> for AttachmentPreviewItem {
    fn from(preview: crate::attachments::AttachmentPreview) -> Self {
        Self {
            attachment_id: preview.attachment.id,
            text: preview.inline_text,
            message: preview.message,
            truncated: preview.truncated,
            preview_bytes: preview.preview_bytes,
        }
    }
}

/// Captured state needed to undo an optimistic message-list mutation.
/// Opaque to callers; produced by `AppState::snapshot_message_list`.
#[derive(Debug, Clone)]
pub struct MessageListSnapshot {
    folder_messages: Vec<MessageItem>,
    selected_thread: usize,
    selected_message: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposerAttachment {
    pub(crate) path: PathBuf,
    pub(crate) filename: String,
    pub(crate) size_bytes: u64,
    pub(crate) content_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposerDraft {
    pub(crate) account_id: AccountId,
    pub(crate) in_reply_to_msg: Option<MessageId>,
    pub(crate) to_addrs: Vec<String>,
    pub(crate) cc_addrs: Vec<String>,
    pub(crate) bcc_addrs: Vec<String>,
    pub(crate) subject: Option<String>,
    pub(crate) text_body: Option<String>,
    pub(crate) html_body: Option<String>,
    pub(crate) attachments: Vec<ComposerAttachment>,
    pub(crate) in_reply_to: Option<String>,
    pub(crate) references_header: Option<String>,
}

/// Pre-fill payload handed to `AppState::enter_composer_with_prefill`.
/// Used by the reply / reply-all / forward key bindings to seed the
/// composer with the response headers + quoted body before the user
/// starts editing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ComposerPrefill {
    pub(crate) in_reply_to_msg: Option<MessageId>,
    pub(crate) to_addrs: Vec<String>,
    pub(crate) cc_addrs: Vec<String>,
    pub(crate) bcc_addrs: Vec<String>,
    pub(crate) subject: Option<String>,
    pub(crate) body: Option<String>,
    pub(crate) in_reply_to: Option<String>,
    pub(crate) references_header: Option<String>,
    pub(crate) attachments: Vec<ComposerAttachment>,
}

/// Reasons a path the user typed into the compose attach prompt was
/// rejected. Surfaces concise toast text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachError {
    NotFound(PathBuf),
    NotAFile(PathBuf),
    TooLarge { size: u64 },
    AggregateTooLarge { total: u64 },
    Io { path: PathBuf, message: String },
}

impl AttachError {
    pub(crate) fn toast_text(&self) -> String {
        match self {
            Self::NotFound(path) => format!("File not found: {}", path.display()),
            Self::NotAFile(path) => format!("Not a regular file: {}", path.display()),
            Self::TooLarge { size } => format!(
                "Attachment too large: {} > {}",
                human_size(*size),
                human_size(MAX_COMPOSE_ATTACHMENT_BYTES)
            ),
            Self::AggregateTooLarge { total } => format!(
                "Aggregate over limit: {} > {}",
                human_size(*total),
                human_size(MAX_COMPOSE_ATTACHMENT_BYTES)
            ),
            Self::Io { path, message } => {
                format!("Cannot read {}: {}", path.display(), message)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LineCache {
    bounds: Vec<LineBounds>,
    char_len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LineBounds {
    char_start: usize,
    char_end: usize,
    byte_start: usize,
    byte_end: usize,
}

impl Default for LineCache {
    fn default() -> Self {
        Self::from_text("")
    }
}

impl LineCache {
    fn from_text(value: &str) -> Self {
        let mut bounds = Vec::new();
        let mut char_start = 0;
        let mut byte_start = 0;
        let mut char_index = 0;

        for (byte_index, ch) in value.char_indices() {
            if ch == '\n' {
                bounds.push(LineBounds {
                    char_start,
                    char_end: char_index,
                    byte_start,
                    byte_end: byte_index,
                });
                char_index += 1;
                char_start = char_index;
                byte_start = byte_index + ch.len_utf8();
            } else {
                char_index += 1;
            }
        }

        bounds.push(LineBounds {
            char_start,
            char_end: char_index,
            byte_start,
            byte_end: value.len(),
        });

        Self {
            bounds,
            char_len: char_index,
        }
    }

    fn line_count(&self) -> usize {
        self.bounds.len()
    }

    fn char_len(&self) -> usize {
        self.char_len
    }

    fn clamped_line(&self, line: usize) -> Option<LineBounds> {
        self.bounds
            .get(line.min(self.bounds.len().saturating_sub(1)))
            .copied()
    }

    fn line_start(&self, line: usize) -> usize {
        self.clamped_line(line)
            .map(|bounds| bounds.char_start)
            .unwrap_or_default()
    }

    fn line_end(&self, line: usize) -> usize {
        self.clamped_line(line)
            .map(|bounds| bounds.char_end)
            .unwrap_or_default()
    }

    fn line_for_cursor(&self, cursor: usize) -> usize {
        line_for_cursor(&self.bounds, cursor)
    }

    fn line<'a>(&self, text: &'a str, line: usize) -> Option<&'a str> {
        let bounds = self.bounds.get(line).copied()?;
        text.get(bounds.byte_start..bounds.byte_end)
    }

    fn lines<'a>(&self, text: &'a str) -> Vec<&'a str> {
        (0..self.line_count())
            .filter_map(|line| self.line(text, line))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextLineCache {
    text: String,
    lines: LineCache,
}

impl TextLineCache {
    fn new(text: String) -> Self {
        let lines = LineCache::from_text(&text);
        Self { text, lines }
    }

    fn line(&self, line: usize) -> Option<&str> {
        self.lines.line(&self.text, line)
    }

    fn line_count(&self) -> usize {
        self.lines.line_count()
    }

    fn line_start(&self, line: usize) -> usize {
        self.lines.line_start(line)
    }

    fn line_end(&self, line: usize) -> usize {
        self.lines.line_end(line)
    }

    fn line_for_cursor(&self, cursor: usize) -> usize {
        self.lines.line_for_cursor(cursor)
    }

    fn char_len(&self) -> usize {
        self.lines.char_len()
    }

    fn text(&self) -> &str {
        &self.text
    }

    fn lines(&self) -> Vec<&str> {
        self.lines.lines(&self.text)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposerState {
    pub(crate) account_id: AccountId,
    pub(crate) draft_id: Option<DraftId>,
    pub(crate) focused: ComposeField,
    pub(crate) to: String,
    pub(crate) to_cursor: usize,
    pub(crate) cc: String,
    pub(crate) cc_cursor: usize,
    pub(crate) bcc: String,
    pub(crate) bcc_cursor: usize,
    pub(crate) subject: String,
    pub(crate) subject_cursor: usize,
    pub(crate) body: String,
    pub(crate) body_cursor: usize,
    pub(crate) body_line_cache: LineCache,
    pub(crate) body_scroll: usize,
    pub(crate) body_selection_anchor: Option<usize>,
    pub(crate) body_selection_focus: usize,
    pub(crate) body_preferred_column: Option<usize>,
    pub(crate) dirty: bool,
    pub(crate) attachments: Vec<ComposerAttachment>,
    pub(crate) selected_attachment: usize,
    pub(crate) attach_input: String,
    pub(crate) in_reply_to_msg: Option<MessageId>,
    pub(crate) in_reply_to: Option<String>,
    pub(crate) references_header: Option<String>,
}

impl ComposerState {
    fn new(account_id: AccountId) -> Self {
        Self {
            account_id,
            draft_id: None,
            focused: ComposeField::To,
            to: String::new(),
            to_cursor: 0,
            cc: String::new(),
            cc_cursor: 0,
            bcc: String::new(),
            bcc_cursor: 0,
            subject: String::new(),
            subject_cursor: 0,
            body: String::new(),
            body_cursor: 0,
            body_line_cache: LineCache::default(),
            body_scroll: 0,
            body_selection_anchor: None,
            body_selection_focus: 0,
            body_preferred_column: None,
            dirty: false,
            attachments: Vec::new(),
            selected_attachment: 0,
            attach_input: String::new(),
            in_reply_to_msg: None,
            in_reply_to: None,
            references_header: None,
        }
    }

    /// Build a composer state already populated with reply / forward
    /// data. The composer is marked dirty so the autosaver persists it
    /// on the next idle tick.
    fn from_prefill(account_id: AccountId, prefill: ComposerPrefill) -> Self {
        let mut state = Self::new(account_id);
        state.to = join_addresses(&prefill.to_addrs);
        state.to_cursor = char_count(&state.to);
        state.cc = join_addresses(&prefill.cc_addrs);
        state.cc_cursor = char_count(&state.cc);
        state.bcc = join_addresses(&prefill.bcc_addrs);
        state.bcc_cursor = char_count(&state.bcc);
        if let Some(subject) = prefill.subject {
            state.subject = subject;
            state.subject_cursor = char_count(&state.subject);
        }
        if let Some(body) = prefill.body {
            state.body = body;
            state.body_cursor = char_count(&state.body);
            state.refresh_body_line_cache();
        }
        state.attachments = prefill.attachments;
        state.in_reply_to_msg = prefill.in_reply_to_msg;
        state.in_reply_to = prefill.in_reply_to;
        state.references_header = prefill.references_header;
        state.focused = ComposeField::Body;
        state.dirty = state.has_content();
        state
    }

    pub fn attachments(&self) -> &[ComposerAttachment] {
        &self.attachments
    }

    pub(crate) fn aggregate_attachment_size(&self) -> u64 {
        self.attachments
            .iter()
            .map(|a| a.size_bytes)
            .fold(0u64, u64::saturating_add)
    }

    fn focused_text(&self) -> &str {
        match self.focused {
            ComposeField::To => &self.to,
            ComposeField::Cc => &self.cc,
            ComposeField::Bcc => &self.bcc,
            ComposeField::Subject => &self.subject,
            ComposeField::Body => &self.body,
        }
    }

    fn focused_text_and_cursor_mut(&mut self) -> (&mut String, &mut usize) {
        match self.focused {
            ComposeField::To => (&mut self.to, &mut self.to_cursor),
            ComposeField::Cc => (&mut self.cc, &mut self.cc_cursor),
            ComposeField::Bcc => (&mut self.bcc, &mut self.bcc_cursor),
            ComposeField::Subject => (&mut self.subject, &mut self.subject_cursor),
            ComposeField::Body => (&mut self.body, &mut self.body_cursor),
        }
    }

    fn field_len(&self) -> usize {
        self.focused_text().chars().count()
    }

    fn field_limit(&self) -> usize {
        match self.focused {
            ComposeField::Body => MAX_COMPOSE_BODY_CHARS,
            _ => MAX_COMPOSE_HEADER_CHARS,
        }
    }

    fn has_content(&self) -> bool {
        [&self.to, &self.cc, &self.bcc, &self.subject, &self.body]
            .iter()
            .any(|value| !value.trim().is_empty())
            || !self.attachments.is_empty()
    }

    fn draft(&self) -> ComposerDraft {
        ComposerDraft {
            account_id: self.account_id,
            in_reply_to_msg: self.in_reply_to_msg,
            to_addrs: split_addresses(&self.to),
            cc_addrs: split_addresses(&self.cc),
            bcc_addrs: split_addresses(&self.bcc),
            subject: non_empty_string(&self.subject),
            text_body: non_empty_string(&self.body),
            html_body: None,
            attachments: self.attachments.clone(),
            in_reply_to: self.in_reply_to.clone(),
            references_header: self.references_header.clone(),
        }
    }

    pub fn focused_cursor(&self) -> usize {
        match self.focused {
            ComposeField::To => self.to_cursor.min(char_count(&self.to)),
            ComposeField::Cc => self.cc_cursor.min(char_count(&self.cc)),
            ComposeField::Bcc => self.bcc_cursor.min(char_count(&self.bcc)),
            ComposeField::Subject => self.subject_cursor.min(char_count(&self.subject)),
            ComposeField::Body => self.body_cursor.min(self.body_line_cache.char_len()),
        }
    }

    pub fn body_lines(&self) -> Vec<&str> {
        self.body_line_cache.lines(&self.body)
    }

    pub(crate) fn body_line_count(&self) -> usize {
        self.body_line_cache.line_count()
    }

    pub(crate) fn body_line_start(&self, line: usize) -> usize {
        self.body_line_cache.line_start(line)
    }

    pub(crate) fn body_line_end(&self, line: usize) -> usize {
        self.body_line_cache.line_end(line)
    }

    pub(crate) fn body_cursor_line_column(&self) -> (usize, usize) {
        let cursor = self.body_cursor.min(self.body_line_cache.char_len());
        let line = self.body_line_cache.line_for_cursor(cursor);
        let start = self.body_line_cache.line_start(line);
        (line, cursor.saturating_sub(start))
    }

    pub(crate) fn body_line_text(&self, line: usize) -> Option<&str> {
        self.body_line_cache.line(&self.body, line)
    }

    pub(crate) fn body_selected_line_range(&self) -> Option<std::ops::RangeInclusive<usize>> {
        let anchor = self.body_selection_anchor?;
        let max_line = self.body_line_count().saturating_sub(1);
        let start = anchor.min(self.body_selection_focus).min(max_line);
        let end = anchor.max(self.body_selection_focus).min(max_line);
        Some(start..=end)
    }

    pub(crate) fn body_visible_scroll(&self, viewport_height: usize) -> usize {
        let viewport_height = viewport_height.max(1);
        let line_count = self.body_line_count();
        let max_scroll = line_count.saturating_sub(viewport_height);
        let mut scroll = self.body_scroll.min(max_scroll);
        let cursor_line = self.body_cursor_line_column().0;

        if cursor_line < scroll {
            scroll = cursor_line;
        } else if cursor_line >= scroll.saturating_add(viewport_height) {
            scroll = cursor_line
                .saturating_add(1)
                .saturating_sub(viewport_height);
        }

        scroll.min(max_scroll)
    }

    fn ensure_body_cursor_visible(&mut self, viewport_height: usize) {
        self.body_scroll = self.body_visible_scroll(viewport_height);
    }

    fn move_focused_cursor_left(&mut self) -> bool {
        let changed = {
            let (_, cursor) = self.focused_text_and_cursor_mut();
            if *cursor == 0 {
                false
            } else {
                *cursor -= 1;
                true
            }
        };
        if changed {
            self.reset_body_navigation_state();
        }
        changed
    }

    fn move_focused_cursor_right(&mut self) -> bool {
        let len = self.field_len();
        let changed = {
            let (_, cursor) = self.focused_text_and_cursor_mut();
            let old = (*cursor).min(len);
            if old >= len {
                *cursor = len;
                false
            } else {
                *cursor = old + 1;
                true
            }
        };
        if changed {
            self.reset_body_navigation_state();
        }
        changed
    }

    fn move_focused_cursor_home(&mut self) -> bool {
        let next = if self.focused == ComposeField::Body {
            let line = self.body_cursor_line_column().0;
            self.body_line_start(line)
        } else {
            0
        };
        self.set_focused_cursor(next)
    }

    fn move_focused_cursor_end(&mut self) -> bool {
        let next = if self.focused == ComposeField::Body {
            let line = self.body_cursor_line_column().0;
            self.body_line_end(line)
        } else {
            self.field_len()
        };
        self.set_focused_cursor(next)
    }

    fn set_focused_cursor(&mut self, next: usize) -> bool {
        let len = self.field_len();
        let next = next.min(len);
        let changed = {
            let (_, cursor) = self.focused_text_and_cursor_mut();
            let old = (*cursor).min(len);
            *cursor = next;
            old != next
        };
        if changed {
            self.reset_body_navigation_state();
        }
        changed
    }

    fn move_body_line(&mut self, delta: isize, viewport_height: usize) -> bool {
        if self.focused != ComposeField::Body {
            return false;
        }

        let old_cursor = self.body_cursor;
        let old_scroll = self.body_scroll;
        let old_selection_focus = self.body_selection_focus;
        let line_count = self.body_line_count();
        let max_line = line_count.saturating_sub(1);
        let (line, column) = self.body_cursor_line_column();
        let preferred_column = self.body_preferred_column.unwrap_or(column);
        self.body_preferred_column = Some(preferred_column);

        let next_line = if delta < 0 {
            line.saturating_sub(delta.unsigned_abs())
        } else {
            line.saturating_add(delta as usize).min(max_line)
        };
        let next_column = preferred_column.min(self.body_line_len(next_line));
        self.body_cursor = self.body_line_start(next_line) + next_column;
        if self.body_selection_anchor.is_some() {
            self.body_selection_focus = next_line;
        }
        self.ensure_body_cursor_visible(viewport_height);

        self.body_cursor != old_cursor
            || self.body_scroll != old_scroll
            || self.body_selection_focus != old_selection_focus
    }

    fn body_line_len(&self, line: usize) -> usize {
        self.body_line_end(line)
            .saturating_sub(self.body_line_start(line))
    }

    fn insert_focused_char(&mut self, ch: char) {
        {
            let (text, cursor) = self.focused_text_and_cursor_mut();
            let current = (*cursor).min(char_count(text));
            let byte_index = char_to_byte_index(text, current);
            text.insert(byte_index, ch);
            *cursor = current + 1;
        }
        self.after_text_edit();
    }

    fn insert_body_newline(&mut self) {
        {
            let current = self.body_cursor.min(char_count(&self.body));
            let byte_index = char_to_byte_index(&self.body, current);
            self.body.insert(byte_index, '\n');
            self.body_cursor = current + 1;
        }
        self.after_text_edit();
    }

    fn delete_before_focused_cursor(&mut self) -> bool {
        let changed = {
            let (text, cursor) = self.focused_text_and_cursor_mut();
            let current = (*cursor).min(char_count(text));
            if current == 0 {
                *cursor = 0;
                false
            } else {
                let start = char_to_byte_index(text, current - 1);
                let end = char_to_byte_index(text, current);
                text.replace_range(start..end, "");
                *cursor = current - 1;
                true
            }
        };
        if changed {
            self.after_text_edit();
        }
        changed
    }

    fn delete_at_focused_cursor(&mut self) -> bool {
        let changed = {
            let (text, cursor) = self.focused_text_and_cursor_mut();
            let current = (*cursor).min(char_count(text));
            let len = char_count(text);
            if current >= len {
                *cursor = len;
                false
            } else {
                let start = char_to_byte_index(text, current);
                let end = char_to_byte_index(text, current + 1);
                text.replace_range(start..end, "");
                *cursor = current;
                true
            }
        };
        if changed {
            self.after_text_edit();
        }
        changed
    }

    fn toggle_body_line_selection(&mut self) -> bool {
        if self.focused != ComposeField::Body {
            return false;
        }
        if self.body_selection_anchor.is_some() {
            self.clear_body_selection()
        } else {
            let line = self.body_cursor_line_column().0;
            self.body_selection_anchor = Some(line);
            self.body_selection_focus = line;
            true
        }
    }

    fn start_body_line_selection(&mut self) -> bool {
        if self.focused != ComposeField::Body || self.body_selection_anchor.is_some() {
            return false;
        }
        let line = self.body_cursor_line_column().0;
        self.body_selection_anchor = Some(line);
        self.body_selection_focus = line;
        true
    }

    fn clear_body_selection(&mut self) -> bool {
        let changed = self.body_selection_anchor.is_some();
        self.body_selection_anchor = None;
        self.body_selection_focus = self.body_cursor_line_column().0;
        changed
    }

    fn reset_body_navigation_state(&mut self) {
        if self.focused == ComposeField::Body {
            self.body_preferred_column = None;
            self.clear_body_selection();
        }
    }

    pub(crate) fn refresh_body_line_cache(&mut self) {
        self.body_line_cache = LineCache::from_text(&self.body);
    }

    fn after_text_edit(&mut self) {
        if self.focused == ComposeField::Body {
            self.refresh_body_line_cache();
            self.body_preferred_column = None;
            self.clear_body_selection();
            self.ensure_body_cursor_visible(1);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Warn,
    Error,
}

impl ToastKind {
    pub(crate) fn ttl(self) -> Duration {
        match self {
            Self::Error => TOAST_TTL_ERROR,
            _ => TOAST_TTL_INFO,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub id: u64,
    pub kind: ToastKind,
    pub text: String,
    pub(crate) expires_at: Instant,
}

/// TUI-side mirror of the wire `sync.state` enum. Kept independent so
/// the tui module doesn't pull crate-internal types into its surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncStateUi {
    Idle,
    Polling,
    Syncing,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountStatus {
    pub(crate) state: SyncStateUi,
    pub(crate) last_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub(crate) active: ActivePane,
    pub(crate) mode: InputMode,
    pub(crate) accounts: Vec<AccountItem>,
    pub(crate) folders: Vec<FolderItem>,
    pub(crate) folder_messages: Vec<MessageItem>,
    pub(crate) threads: Vec<ThreadItem>,
    pub(crate) messages: Vec<MessageItem>,
    pub(crate) detail: Option<MessageDetail>,
    pub(crate) detail_text_cache: Option<TextLineCache>,
    pub(crate) detail_cursor: usize,
    pub(crate) detail_scroll: usize,
    pub(crate) detail_selection_anchor: Option<usize>,
    pub(crate) detail_selection_focus: usize,
    pub(crate) detail_preferred_column: Option<usize>,
    pub(crate) attachments: Vec<AttachmentItem>,
    pub(crate) attachment_preview: Option<AttachmentPreviewItem>,
    /// True when keyboard input within `ActivePane::Attachments` should
    /// drive the preview viewport (scroll, visual select, yank) instead
    /// of the attachment list. Toggled with Enter / Esc; reset whenever
    /// the underlying preview goes away.
    pub(crate) preview_focused: bool,
    /// Top-line offset into the preview text. Bound by viewport height
    /// so we never scroll past the last line.
    pub(crate) preview_scroll: usize,
    /// Visual line-mode anchor (the line the user pressed `v` on) and
    /// the current focus line. `None` means no active selection.
    pub(crate) preview_selection: Option<(usize, usize)>,
    pub(crate) selected_account: usize,
    pub(crate) selected_folder: usize,
    pub(crate) selected_thread: usize,
    pub(crate) selected_message: usize,
    pub(crate) selected_attachment: usize,
    pub(crate) pending_open_attachment: Option<AttachmentItem>,
    pub(crate) pending_delete_message: Option<MessageId>,
    pub(crate) command_input: String,
    pub(crate) status: String,
    pub(crate) error: Option<String>,
    pub(crate) theme: ThemeName,
    pub(crate) composer: Option<ComposerState>,
    pub toasts: VecDeque<Toast>,
    pub(crate) next_toast_id: u64,
    pub(crate) account_states: HashMap<AccountId, AccountStatus>,
    pub(crate) search: Option<SearchState>,
    pub(crate) search_input: String,
    pub(crate) search_input_previous_pane: ActivePane,
    /// Drafts list when the Drafts folder is selected. Disjoint from
    /// `folder_messages`/`messages` so the renderer can pick a code
    /// path without inspecting both stores.
    pub(crate) drafts: Vec<DraftItem>,
    pub(crate) selected_draft: usize,
    /// Pending draft to delete; mirrors `pending_delete_message` so
    /// the same y/n confirmation flow can be reused.
    pub(crate) pending_delete_draft: Option<DraftId>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            active: ActivePane::Accounts,
            mode: InputMode::Normal,
            accounts: Vec::new(),
            folders: Vec::new(),
            folder_messages: Vec::new(),
            threads: Vec::new(),
            messages: Vec::new(),
            detail: None,
            detail_text_cache: None,
            detail_cursor: 0,
            detail_scroll: 0,
            detail_selection_anchor: None,
            detail_selection_focus: 0,
            detail_preferred_column: None,
            attachments: Vec::new(),
            attachment_preview: None,
            preview_focused: false,
            preview_scroll: 0,
            preview_selection: None,
            selected_account: 0,
            selected_folder: 0,
            selected_thread: 0,
            selected_message: 0,
            selected_attachment: 0,
            pending_open_attachment: None,
            pending_delete_message: None,
            command_input: String::new(),
            status: "Connecting".into(),
            error: None,
            theme: ThemeName::default(),
            composer: None,
            toasts: VecDeque::new(),
            next_toast_id: 0,
            account_states: HashMap::new(),
            search: None,
            drafts: Vec::new(),
            selected_draft: 0,
            pending_delete_draft: None,
            search_input: String::new(),
            search_input_previous_pane: ActivePane::Accounts,
        }
    }
}

impl AppState {
    pub(crate) fn cycle_active_pane(&mut self) {
        self.active = self.next_visible_pane();
    }

    pub(crate) fn cycle_active_pane_reverse(&mut self) {
        self.active = self.previous_visible_pane();
    }

    pub(crate) fn has_threaded_conversations(&self) -> bool {
        self.threads.iter().any(|thread| thread.message_count > 1)
    }

    pub(crate) fn threads_pane_visible(&self) -> bool {
        self.has_threaded_conversations()
    }

    pub(crate) fn move_selection(&mut self, delta: isize) -> bool {
        match self.active {
            ActivePane::Accounts => {
                let changed = move_index(&mut self.selected_account, self.accounts.len(), delta);
                if changed {
                    self.folders.clear();
                    self.folder_messages.clear();
                    self.threads.clear();
                    self.messages.clear();
                    self.clear_drafts();
                    self.clear_detail_state();
                    self.selected_folder = 0;
                    self.selected_thread = 0;
                    self.selected_message = 0;
                }
                changed
            }
            ActivePane::Folders => {
                let changed = move_index(&mut self.selected_folder, self.folders.len(), delta);
                if changed {
                    self.folder_messages.clear();
                    self.threads.clear();
                    self.messages.clear();
                    self.clear_drafts();
                    self.clear_detail_state();
                    self.selected_thread = 0;
                    self.selected_message = 0;
                }
                changed
            }
            ActivePane::Threads => {
                if !self.threads_pane_visible() {
                    self.normalize_active_pane();
                    return false;
                }
                let changed = move_index(&mut self.selected_thread, self.threads.len(), delta);
                if changed {
                    self.selected_message = 0;
                    self.refresh_visible_messages();
                    self.clear_detail_state();
                }
                changed
            }
            ActivePane::Messages => {
                if self.drafts_pane_active() {
                    let changed = move_index(&mut self.selected_draft, self.drafts.len(), delta);
                    if changed {
                        self.clear_detail_state();
                    }
                    return changed;
                }
                let changed = move_index(&mut self.selected_message, self.messages.len(), delta);
                if changed {
                    self.clear_detail_state();
                }
                changed
            }
            ActivePane::Details => false,
            ActivePane::Attachments => {
                if !self.attachments_pane_visible() {
                    self.normalize_active_pane();
                    return false;
                }
                if self.preview_focused {
                    return self.move_preview_line(delta);
                }
                let changed =
                    move_index(&mut self.selected_attachment, self.attachments.len(), delta);
                if changed {
                    self.attachment_preview = None;
                    self.reset_preview_navigation_state();
                }
                changed
            }
            ActivePane::Search => self.move_search_selection(delta),
        }
    }

    pub fn apply_accounts(&mut self, accounts: Vec<AccountItem>) {
        self.accounts = accounts;
        clamp_index(&mut self.selected_account, self.accounts.len());
        self.folders.clear();
        self.folder_messages.clear();
        self.threads.clear();
        self.messages.clear();
        self.clear_drafts();
        self.clear_detail_state();
        self.selected_folder = 0;
        self.selected_thread = 0;
        self.selected_message = 0;
        self.search = None;
        self.normalize_active_pane();
    }

    pub fn apply_folders(&mut self, folders: Vec<FolderItem>) {
        self.folders = folders;
        clamp_index(&mut self.selected_folder, self.folders.len());
        self.folder_messages.clear();
        self.threads.clear();
        self.messages.clear();
        self.clear_drafts();
        self.clear_detail_state();
        self.selected_thread = 0;
        self.selected_message = 0;
        self.normalize_active_pane();
    }

    pub fn apply_messages(&mut self, messages: Vec<MessageItem>) {
        self.messages = messages;
        clamp_index(&mut self.selected_message, self.messages.len());
        self.clear_detail_state();
    }

    pub(crate) fn apply_folder_messages(&mut self, messages: Vec<MessageItem>) {
        let previous_key = self.selected_thread().map(|thread| thread.key);
        self.folder_messages = messages;
        self.rebuild_threads(previous_key);
        if self.selected_thread().map(|thread| thread.key) != previous_key {
            self.selected_message = 0;
        }
        self.refresh_visible_messages();
        self.normalize_active_pane();
        self.clear_detail_state();
    }

    pub(crate) fn apply_drafts(&mut self, drafts: Vec<DraftItem>) {
        self.drafts = drafts;
        clamp_index(&mut self.selected_draft, self.drafts.len());
    }

    pub(crate) fn clear_drafts(&mut self) {
        self.drafts.clear();
        self.selected_draft = 0;
    }

    pub(crate) fn selected_draft_id(&self) -> Option<DraftId> {
        self.drafts.get(self.selected_draft).map(|d| d.id)
    }

    pub fn selected_draft(&self) -> Option<&DraftItem> {
        self.drafts.get(self.selected_draft)
    }

    /// Move the drafts cursor by `delta` rows. Returns true on a real
    /// position change so callers can trigger refresh logic.
    pub fn move_draft_selection(&mut self, delta: isize) -> bool {
        move_index(&mut self.selected_draft, self.drafts.len(), delta)
    }

    pub(crate) fn begin_draft_delete(&mut self, draft_id: DraftId) {
        self.pending_delete_draft = Some(draft_id);
        self.mode = InputMode::ConfirmDelete;
    }

    pub(crate) fn cancel_pending_delete_draft(&mut self) {
        self.pending_delete_draft = None;
        if self.mode == InputMode::ConfirmDelete {
            self.mode = InputMode::Normal;
        }
    }

    pub(crate) fn take_pending_delete_draft(&mut self) -> Option<DraftId> {
        let id = self.pending_delete_draft.take();
        if id.is_some() && self.mode == InputMode::ConfirmDelete {
            self.mode = InputMode::Normal;
        }
        id
    }

    /// Drop the row matching `draft_id` from the drafts list and
    /// clamp the selection. Used by the optimistic delete path.
    pub(crate) fn remove_draft_locally(&mut self, draft_id: DraftId) -> bool {
        let before = self.drafts.len();
        self.drafts.retain(|d| d.id != draft_id);
        let removed = self.drafts.len() != before;
        if removed {
            clamp_index(&mut self.selected_draft, self.drafts.len());
        }
        removed
    }

    pub(crate) fn apply_detail(&mut self, detail: Option<MessageDetail>) {
        let was_detail_focused = self.active == ActivePane::Details;
        let old_detail_id = self.detail.as_ref().map(|detail| detail.id);
        let new_detail_id = detail.as_ref().map(|detail| detail.id);
        if old_detail_id != new_detail_id {
            self.clear_attachments();
        }
        if let Some(detail) = &detail {
            let selected_thread = self.selected_thread().map(|thread| thread.key);
            if let Some(message) = self
                .folder_messages
                .iter_mut()
                .find(|message| message.id == detail.id)
            {
                message.flags = detail.flags.clone();
            }
            if let Some(message) = self
                .messages
                .iter_mut()
                .find(|message| message.id == detail.id)
            {
                message.flags = detail.flags.clone();
            }
            if !self.folder_messages.is_empty() {
                self.rebuild_threads(selected_thread);
                self.refresh_visible_messages();
            }
        }
        self.detail_text_cache = detail
            .as_ref()
            .map(|detail| TextLineCache::new(detail_text(detail)));
        self.detail = detail;
        self.reset_detail_navigation_state();
        if was_detail_focused && self.detail.is_some() {
            self.active = ActivePane::Details;
        }
        if self.detail.is_none() {
            self.clear_attachments();
        }
        self.normalize_active_pane();
    }

    pub(crate) fn apply_attachments(&mut self, attachments: Vec<AttachmentItem>) {
        self.attachments = attachments;
        clamp_index(&mut self.selected_attachment, self.attachments.len());
        if self
            .attachment_preview
            .as_ref()
            .is_some_and(|preview| Some(preview.attachment_id) != self.selected_attachment_id())
        {
            self.attachment_preview = None;
            self.reset_preview_navigation_state();
        }
        if self.attachments.is_empty() {
            self.attachment_preview = None;
            self.pending_open_attachment = None;
            self.reset_preview_navigation_state();
        }
        self.normalize_active_pane();
    }

    pub(crate) fn apply_attachment_preview(&mut self, preview: AttachmentPreviewItem) {
        let same_attachment = self
            .attachment_preview
            .as_ref()
            .map(|existing| existing.attachment_id)
            == Some(preview.attachment_id);
        self.attachment_preview = Some(preview);
        if !same_attachment {
            self.reset_preview_navigation_state();
        }
    }

    /// Renderable preview text. Mirrors what the user sees in the
    /// preview pane so scroll, selection, and yank operate on a single
    /// source of truth.
    pub(crate) fn preview_text(&self) -> Option<String> {
        let preview = self.attachment_preview.as_ref()?;
        let mut text = preview.message.clone();
        if let Some(body) = &preview.text {
            text.push_str("\n\n");
            text.push_str(body);
        }
        if preview.truncated {
            text.push_str("\n\n[truncated]");
        }
        Some(text)
    }

    pub(crate) fn preview_lines(&self) -> Vec<String> {
        self.preview_text()
            .map(|text| text.split('\n').map(str::to_string).collect())
            .unwrap_or_default()
    }

    pub(crate) fn preview_line_count(&self) -> usize {
        self.preview_lines().len()
    }

    /// Maximum legal `preview_scroll` value for a viewport of
    /// `viewport_height` lines. Anything larger leaves blank rows at
    /// the bottom, so we clamp.
    pub(crate) fn preview_max_scroll(&self, viewport_height: usize) -> usize {
        let viewport_height = viewport_height.max(1);
        self.preview_line_count().saturating_sub(viewport_height)
    }

    pub(crate) fn preview_visible_scroll(&self, viewport_height: usize) -> usize {
        self.preview_scroll
            .min(self.preview_max_scroll(viewport_height))
    }

    /// Scroll the preview by `delta` lines. Positive values scroll
    /// down. Returns true if the offset moved.
    pub(crate) fn scroll_preview(&mut self, delta: isize, viewport_height: usize) -> bool {
        if !self.is_preview_focus_active() {
            return false;
        }
        let max = self.preview_max_scroll(viewport_height);
        let old = self.preview_scroll.min(max);
        let next = if delta < 0 {
            old.saturating_sub(delta.unsigned_abs())
        } else {
            old.saturating_add(delta as usize).min(max)
        };
        self.preview_scroll = next;
        next != old
    }

    pub(crate) fn scroll_preview_to_top(&mut self) -> bool {
        if !self.is_preview_focus_active() {
            return false;
        }
        let changed = self.preview_scroll != 0;
        self.preview_scroll = 0;
        changed
    }

    pub(crate) fn scroll_preview_to_bottom(&mut self, viewport_height: usize) -> bool {
        if !self.is_preview_focus_active() {
            return false;
        }
        let max = self.preview_max_scroll(viewport_height);
        let changed = self.preview_scroll != max;
        self.preview_scroll = max;
        changed
    }

    /// Move the preview "cursor" line by `delta`. If a visual selection
    /// is active, extend it; otherwise just scroll the viewport so the
    /// new line stays visible.
    pub(crate) fn move_preview_line(&mut self, delta: isize) -> bool {
        if !self.is_preview_focus_active() {
            return false;
        }
        if let Some((anchor, focus)) = self.preview_selection {
            let max_line = self.preview_line_count().saturating_sub(1);
            let next = clamp_isize(focus as isize + delta, 0, max_line as isize) as usize;
            if next == focus {
                return false;
            }
            self.preview_selection = Some((anchor, next));
            true
        } else {
            self.scroll_preview(delta, 1)
        }
    }

    pub(crate) fn preview_selected_line_range(&self) -> Option<std::ops::RangeInclusive<usize>> {
        let (anchor, focus) = self.preview_selection?;
        let max_line = self.preview_line_count().saturating_sub(1);
        if max_line == 0 && self.preview_line_count() == 0 {
            return None;
        }
        let start = anchor.min(focus).min(max_line);
        let end = anchor.max(focus).min(max_line);
        Some(start..=end)
    }

    /// Toggle visual line-mode selection, anchoring on the current
    /// preview cursor (taken to be the top of the viewport).
    pub(crate) fn toggle_preview_selection(&mut self) -> bool {
        if !self.is_preview_focus_active() {
            return false;
        }
        if self.preview_selection.is_some() {
            self.preview_selection = None;
            return true;
        }
        let line = self
            .preview_scroll
            .min(self.preview_line_count().saturating_sub(1).max(0));
        self.preview_selection = Some((line, line));
        true
    }

    pub(crate) fn clear_preview_selection(&mut self) -> bool {
        let had = self.preview_selection.is_some();
        self.preview_selection = None;
        had
    }

    /// Build the clipboard payload for `y`. With an active selection,
    /// joins the selected line range with `\n`. With no selection,
    /// returns `None` so the caller can decide what to do.
    pub(crate) fn preview_yank_text(&self) -> Option<String> {
        let range = self.preview_selected_line_range()?;
        let lines = self.preview_lines();
        let start = *range.start();
        let end = *range.end();
        if start >= lines.len() {
            return None;
        }
        let end = end.min(lines.len().saturating_sub(1));
        Some(lines[start..=end].join("\n"))
    }

    pub(crate) fn focus_preview(&mut self) -> bool {
        if self.attachment_preview.is_none() {
            return false;
        }
        if self.preview_focused {
            return false;
        }
        self.preview_focused = true;
        true
    }

    pub(crate) fn defocus_preview(&mut self) -> bool {
        if !self.preview_focused {
            return false;
        }
        self.preview_focused = false;
        self.preview_selection = None;
        true
    }

    pub(crate) fn is_preview_focus_active(&self) -> bool {
        self.preview_focused
            && self.attachment_preview.is_some()
            && self.active == ActivePane::Attachments
    }

    pub(crate) fn attachments_pane_visible(&self) -> bool {
        self.detail.is_some() && !self.attachments.is_empty()
    }

    pub(crate) fn detail_pane_visible(&self) -> bool {
        self.detail.is_some()
    }

    pub(crate) fn focus_detail_pane(&mut self) -> bool {
        if self.detail_pane_visible() {
            self.active = ActivePane::Details;
            true
        } else {
            self.normalize_active_pane();
            false
        }
    }

    pub fn detail_lines(&self) -> Vec<String> {
        self.detail_text_cache
            .as_ref()
            .map(|cache| cache.lines().into_iter().map(str::to_string).collect())
            .unwrap_or_default()
    }

    pub(crate) fn detail_line_count(&self) -> usize {
        self.detail_text_cache
            .as_ref()
            .map(TextLineCache::line_count)
            .unwrap_or_default()
    }

    pub(crate) fn detail_line_start(&self, line: usize) -> usize {
        self.detail_text_cache
            .as_ref()
            .map(|cache| cache.line_start(line))
            .unwrap_or_default()
    }

    pub(crate) fn detail_line_end(&self, line: usize) -> usize {
        self.detail_text_cache
            .as_ref()
            .map(|cache| cache.line_end(line))
            .unwrap_or_default()
    }

    pub(crate) fn detail_cursor_line_column(&self) -> (usize, usize) {
        let cursor = self.detail_cursor.min(self.detail_len());
        self.detail_text_cache
            .as_ref()
            .map(|cache| {
                let line = cache.line_for_cursor(cursor);
                let start = cache.line_start(line);
                (line, cursor.saturating_sub(start))
            })
            .unwrap_or_default()
    }

    pub(crate) fn detail_line_text(&self, line: usize) -> Option<&str> {
        self.detail_text_cache
            .as_ref()
            .and_then(|cache| cache.line(line))
    }

    pub(crate) fn detail_selected_line_range(&self) -> Option<std::ops::RangeInclusive<usize>> {
        let anchor = self.detail_selection_anchor?;
        let max_line = self.detail_line_count().saturating_sub(1);
        let start = anchor.min(self.detail_selection_focus).min(max_line);
        let end = anchor.max(self.detail_selection_focus).min(max_line);
        Some(start..=end)
    }

    pub(crate) fn detail_visible_scroll(&self, viewport_height: usize) -> usize {
        let viewport_height = viewport_height.max(1);
        let line_count = self.detail_line_count();
        let max_scroll = line_count.saturating_sub(viewport_height);
        let mut scroll = self.detail_scroll.min(max_scroll);
        let cursor_line = self.detail_cursor_line_column().0;

        if cursor_line < scroll {
            scroll = cursor_line;
        } else if cursor_line >= scroll.saturating_add(viewport_height) {
            scroll = cursor_line
                .saturating_add(1)
                .saturating_sub(viewport_height);
        }

        scroll.min(max_scroll)
    }

    pub(crate) fn move_detail_cursor_left(&mut self) -> bool {
        if !self.detail_pane_visible() {
            return false;
        }
        let column = self.detail_cursor_line_column().1;
        if column == 0 {
            return false;
        }
        self.detail_cursor = self.detail_cursor.min(self.detail_len()).saturating_sub(1);
        self.detail_preferred_column = None;
        true
    }

    pub(crate) fn move_detail_cursor_right(&mut self) -> bool {
        if !self.detail_pane_visible() {
            return false;
        }
        let (line, column) = self.detail_cursor_line_column();
        let line_len = self.detail_line_len(line);
        if column >= line_len {
            return false;
        }
        self.detail_cursor = self.detail_line_start(line) + column + 1;
        self.detail_preferred_column = None;
        true
    }

    pub(crate) fn detail_home(&mut self) -> bool {
        if !self.detail_pane_visible() {
            return false;
        }
        let line = self.detail_cursor_line_column().0;
        self.set_detail_cursor(self.detail_line_start(line))
    }

    pub(crate) fn detail_end(&mut self) -> bool {
        if !self.detail_pane_visible() {
            return false;
        }
        let line = self.detail_cursor_line_column().0;
        self.set_detail_cursor(self.detail_line_end(line))
    }

    pub(crate) fn move_detail_line(&mut self, delta: isize, viewport_height: usize) -> bool {
        if self.active != ActivePane::Details || !self.detail_pane_visible() {
            return false;
        }

        let old_cursor = self.detail_cursor;
        let old_scroll = self.detail_scroll;
        let old_selection_focus = self.detail_selection_focus;
        let line_count = self.detail_line_count();
        if line_count == 0 {
            return false;
        }
        let max_line = line_count.saturating_sub(1);
        let (line, column) = self.detail_cursor_line_column();
        let preferred_column = self.detail_preferred_column.unwrap_or(column);
        self.detail_preferred_column = Some(preferred_column);

        let next_line = if delta < 0 {
            line.saturating_sub(delta.unsigned_abs())
        } else {
            line.saturating_add(delta as usize).min(max_line)
        };
        let next_column = preferred_column.min(self.detail_line_len(next_line));
        self.detail_cursor = self.detail_line_start(next_line) + next_column;
        if self.detail_selection_anchor.is_some() {
            self.detail_selection_focus = next_line;
        }
        self.ensure_detail_cursor_visible(viewport_height);

        self.detail_cursor != old_cursor
            || self.detail_scroll != old_scroll
            || self.detail_selection_focus != old_selection_focus
    }

    pub(crate) fn toggle_detail_line_selection(&mut self) -> bool {
        if self.active != ActivePane::Details || !self.detail_pane_visible() {
            return false;
        }
        if self.detail_selection_anchor.is_some() {
            self.clear_detail_selection()
        } else {
            let line = self.detail_cursor_line_column().0;
            self.detail_selection_anchor = Some(line);
            self.detail_selection_focus = line;
            true
        }
    }

    pub(crate) fn start_detail_line_selection(&mut self) -> bool {
        if self.active != ActivePane::Details
            || !self.detail_pane_visible()
            || self.detail_selection_anchor.is_some()
        {
            return false;
        }
        let line = self.detail_cursor_line_column().0;
        self.detail_selection_anchor = Some(line);
        self.detail_selection_focus = line;
        true
    }

    pub(crate) fn clear_detail_selection(&mut self) -> bool {
        let changed = self.detail_selection_anchor.is_some();
        self.detail_selection_anchor = None;
        self.detail_selection_focus = self.detail_cursor_line_column().0;
        changed
    }

    pub(crate) fn selected_attachment(&self) -> Option<&AttachmentItem> {
        self.attachments.get(self.selected_attachment)
    }

    pub(crate) fn selected_attachment_id(&self) -> Option<AttachmentId> {
        self.selected_attachment().map(|attachment| attachment.id)
    }

    pub(crate) fn toggle_attachment_focus(&mut self) -> bool {
        if !self.attachments_pane_visible() {
            self.normalize_active_pane();
            return false;
        }
        self.active = if self.active == ActivePane::Attachments {
            self.preview_focused = false;
            self.preview_selection = None;
            ActivePane::Messages
        } else {
            ActivePane::Attachments
        };
        true
    }

    pub(crate) fn begin_open_attachment_confirmation(&mut self) -> bool {
        let Some(attachment) = self.selected_attachment().cloned() else {
            return false;
        };
        self.pending_open_attachment = Some(attachment);
        true
    }

    pub(crate) fn cancel_open_attachment_confirmation(&mut self) {
        self.pending_open_attachment = None;
    }

    pub(crate) fn take_pending_open_attachment(&mut self) -> Option<AttachmentItem> {
        self.pending_open_attachment.take()
    }

    pub(crate) fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
    }

    pub(crate) fn set_error(&mut self, error: impl Into<String>) {
        self.error = Some(error.into());
    }

    pub(crate) fn clear_error(&mut self) {
        self.error = None;
    }

    /// Push a toast onto the back of the deque. If the deque is full,
    /// the oldest toast (front) is dropped.
    pub(crate) fn push_toast(
        &mut self,
        kind: ToastKind,
        text: impl Into<String>,
        now: Instant,
    ) -> u64 {
        let id = self.next_toast_id;
        self.next_toast_id = self.next_toast_id.wrapping_add(1);
        let toast = Toast {
            id,
            kind,
            text: text.into(),
            expires_at: now + kind.ttl(),
        };
        if self.toasts.len() >= MAX_TOASTS {
            self.toasts.pop_front();
        }
        self.toasts.push_back(toast);
        id
    }

    /// Refresh the expiry of an existing toast that matches `kind` and
    /// `text`, provided it was pushed within `window` of `now`.
    /// Returns true if a coalesce happened.
    fn coalesce_toast(
        &mut self,
        kind: ToastKind,
        text: &str,
        now: Instant,
        window: Duration,
    ) -> bool {
        let ttl = kind.ttl();
        // A toast was originally pushed at `expires_at - ttl`. We
        // coalesce iff `now - push_time <= window`, equivalently
        // `expires_at + window >= now + ttl`.
        if let Some(existing) = self.toasts.iter_mut().rev().find(|toast| {
            toast.kind == kind && toast.text == text && toast.expires_at + window >= now + ttl
        }) {
            existing.expires_at = now + ttl;
            return true;
        }
        false
    }

    /// Drop the most recently pushed toast (back of deque).
    pub(crate) fn dismiss_newest_toast(&mut self) -> bool {
        self.toasts.pop_back().is_some()
    }

    /// Clear every toast.
    pub(crate) fn clear_toasts(&mut self) -> bool {
        let had = !self.toasts.is_empty();
        self.toasts.clear();
        had
    }

    /// Drop expired toasts. Caller passes the current `Instant` so
    /// tests can drive expiry deterministically.
    pub(crate) fn tick_toasts(&mut self, now: Instant) {
        self.toasts.retain(|toast| toast.expires_at > now);
    }

    /// Apply a `sync.state` transition. Updates the per-account map
    /// and, on `Error`, pushes (or coalesces) an Error toast.
    pub(crate) fn apply_sync_state(
        &mut self,
        account_id: AccountId,
        state: SyncStateUi,
        last_error: Option<String>,
        now: Instant,
    ) {
        if state == SyncStateUi::Error {
            let message = last_error.clone().unwrap_or_else(|| "sync error".into());
            let label = self.account_label_for_toast(account_id);
            let text = format!("{label}: {message}");
            if !self.coalesce_toast(ToastKind::Error, &text, now, COALESCE_SYNC_ERROR) {
                self.push_toast(ToastKind::Error, text, now);
            }
        }
        self.account_states.insert(
            account_id,
            AccountStatus {
                state,
                last_error: if state == SyncStateUi::Error {
                    last_error.or_else(|| Some("sync error".into()))
                } else {
                    None
                },
            },
        );
    }

    /// Push a `mail.new` toast resolved against current accounts/folders.
    pub(crate) fn push_mail_new_toast(
        &mut self,
        account_id: AccountId,
        folder_id: Option<FolderId>,
        now: Instant,
    ) {
        let folder = folder_id
            .and_then(|id| self.folders.iter().find(|f| f.id == id))
            .map(|f| f.name.clone())
            .unwrap_or_else(|| "folder".into());
        let account = self.account_label_for_toast(account_id);
        let text = format!("New mail in {folder} ({account})");
        self.push_toast(ToastKind::Info, text, now);
    }

    /// Push (or coalesce) an `account.synced` toast for `account_id`.
    pub(crate) fn push_account_synced_toast(&mut self, account_id: AccountId, now: Instant) {
        let label = self.account_label_for_toast(account_id);
        let text = format!("Synced {label}");
        if !self.coalesce_toast(ToastKind::Info, &text, now, COALESCE_ACCOUNT_SYNCED) {
            self.push_toast(ToastKind::Info, text, now);
        }
    }

    fn account_label_for_toast(&self, account_id: AccountId) -> String {
        self.accounts
            .iter()
            .find(|a| a.id == account_id)
            .map(|a| a.label.clone())
            .unwrap_or_else(|| short_id(account_id))
    }

    pub(crate) fn selected_account_id(&self) -> Option<AccountId> {
        self.accounts.get(self.selected_account).map(|a| a.id)
    }

    pub(crate) fn selected_folder_id(&self) -> Option<FolderId> {
        self.folders.get(self.selected_folder).map(|f| f.id)
    }

    pub(crate) fn selected_folder_name(&self) -> Option<&str> {
        self.folders
            .get(self.selected_folder)
            .map(|f| f.name.as_str())
    }

    pub(crate) fn selected_folder_role(&self) -> Option<&str> {
        self.folders
            .get(self.selected_folder)
            .map(|f| f.role.as_str())
    }

    /// True when the user is currently viewing a folder whose role is
    /// `drafts`. Drives the "Enter opens composer" / "D deletes draft"
    /// keybindings on the messages list.
    pub(crate) fn drafts_pane_active(&self) -> bool {
        self.selected_folder_role() == Some("drafts")
    }

    /// Switch the active account by case-insensitive label or email
    /// match. Mirrors the navigation effect of pressing `↑`/`↓` on the
    /// accounts pane: clears folder/message state so the caller can
    /// refresh from the daemon. Returns true on a successful match.
    pub(crate) fn select_account_by_name(&mut self, name: &str) -> bool {
        let needle = name.trim();
        if needle.is_empty() {
            return false;
        }
        let lowered = needle.to_lowercase();
        let Some(index) = self.accounts.iter().position(|account| {
            account.label.to_lowercase() == lowered || account.email.to_lowercase() == lowered
        }) else {
            return false;
        };
        if self.selected_account == index {
            return true;
        }
        self.selected_account = index;
        self.active = ActivePane::Accounts;
        self.folders.clear();
        self.folder_messages.clear();
        self.threads.clear();
        self.messages.clear();
        self.clear_drafts();
        self.clear_detail_state();
        self.selected_folder = 0;
        self.selected_thread = 0;
        self.selected_message = 0;
        self.normalize_active_pane();
        true
    }

    /// Switch the active folder by exact name match within the current
    /// account. Returns true on a successful match. Same downstream
    /// reset as moving via `↑`/`↓` on the folders pane.
    pub(crate) fn select_folder_by_name(&mut self, name: &str) -> bool {
        let needle = name.trim();
        if needle.is_empty() {
            return false;
        }
        let Some(index) = self.folders.iter().position(|folder| folder.name == needle) else {
            return false;
        };
        if self.selected_folder == index {
            return true;
        }
        self.selected_folder = index;
        self.active = ActivePane::Folders;
        self.folder_messages.clear();
        self.threads.clear();
        self.messages.clear();
        self.clear_drafts();
        self.clear_detail_state();
        self.selected_thread = 0;
        self.selected_message = 0;
        self.normalize_active_pane();
        true
    }

    pub(crate) fn search_pane_visible(&self) -> bool {
        self.search.is_some()
    }

    /// Resolve an account name (label or email, case-insensitive) to a
    /// `AccountId`. Used by `:search --account <name>`.
    pub(crate) fn account_id_by_name(&self, name: &str) -> Option<AccountId> {
        let lowered = name.trim().to_lowercase();
        if lowered.is_empty() {
            return None;
        }
        self.accounts
            .iter()
            .find(|account| {
                account.label.to_lowercase() == lowered || account.email.to_lowercase() == lowered
            })
            .map(|account| account.id)
    }

    /// Begin quick-search input over the message list. Restores
    /// `previous_pane` on cancel.
    pub(crate) fn enter_quick_search(&mut self) {
        self.search_input_previous_pane = self.active;
        self.search_input.clear();
        self.mode = InputMode::QuickSearch;
        self.clear_error();
        self.set_status("Search /");
    }

    pub(crate) fn cancel_quick_search(&mut self) {
        self.mode = InputMode::Normal;
        self.search_input.clear();
        self.clear_error();
        self.active = self.search_input_previous_pane;
        self.set_status("Search cancelled");
    }

    pub(crate) fn push_search_char(&mut self, ch: char) -> bool {
        if ch.is_control() || self.search_input.chars().count() >= MAX_SEARCH_CHARS {
            return false;
        }
        self.search_input.push(ch);
        true
    }

    pub(crate) fn backspace_search(&mut self) -> bool {
        self.search_input.pop().is_some()
    }

    /// Consume the quick-search buffer and switch to Normal mode.
    pub(crate) fn finish_quick_search(&mut self) -> String {
        self.mode = InputMode::Normal;
        std::mem::take(&mut self.search_input)
    }

    /// Open the search pane with `query` and `scope_account`. Records
    /// `previous_pane` so Esc can restore it. Marks results as pending
    /// until [`AppState::apply_search_hits`] is called.
    pub(crate) fn begin_search(
        &mut self,
        query: impl Into<String>,
        scope_account: Option<AccountId>,
    ) {
        let previous = if self.search_pane_visible() {
            self.search
                .as_ref()
                .map(|state| state.previous_pane)
                .unwrap_or(self.active)
        } else {
            self.active
        };
        self.search = Some(SearchState::new(query, scope_account, previous));
        self.active = ActivePane::Search;
        self.clear_error();
    }

    pub(crate) fn apply_search_hits(&mut self, hits: Vec<SearchHit>) {
        if let Some(state) = &mut self.search {
            state.hits = hits;
            state.pending = false;
            clamp_index(&mut state.selected, state.hits.len());
        }
    }

    /// Restore the pane that was active before the search opened and
    /// clear the search state.
    pub(crate) fn close_search(&mut self) {
        if let Some(state) = self.search.take() {
            self.active = state.previous_pane;
        }
        self.normalize_active_pane();
    }

    pub(crate) fn move_search_selection(&mut self, delta: isize) -> bool {
        let Some(state) = &mut self.search else {
            return false;
        };
        if state.hits.is_empty() {
            state.selected = 0;
            return false;
        }
        move_index(&mut state.selected, state.hits.len(), delta)
    }

    pub(crate) fn selected_search_hit(&self) -> Option<&SearchHit> {
        self.search
            .as_ref()
            .and_then(|state| state.hits.get(state.selected))
    }

    pub(crate) fn search_query(&self) -> Option<&str> {
        self.search.as_ref().map(|state| state.query.as_str())
    }

    pub(crate) fn search_scope_account(&self) -> Option<AccountId> {
        self.search.as_ref().and_then(|state| state.scope_account)
    }

    pub(crate) fn search_is_pending(&self) -> bool {
        self.search.as_ref().is_some_and(|state| state.pending)
    }

    /// Refocus a hit's location: switch active account / folder /
    /// selected message and close the search pane. Returns true when
    /// either a target hit was found and applied or the caller passed
    /// in a known hit. The folder/account lookups are best-effort —
    /// the message list is loaded lazily by the caller via
    /// `refresh_messages` after this returns.
    pub(crate) fn jump_to_hit(&mut self, hit: &SearchHit) -> bool {
        let Some(account_index) = self
            .accounts
            .iter()
            .position(|account| account.id == hit.account_id)
        else {
            return false;
        };
        if self.selected_account != account_index {
            self.selected_account = account_index;
            self.folders.clear();
            self.folder_messages.clear();
            self.threads.clear();
            self.messages.clear();
            self.clear_detail_state();
            self.selected_folder = 0;
            self.selected_thread = 0;
        }
        if let Some(folder_index) = self
            .folders
            .iter()
            .position(|folder| folder.id == hit.folder_id)
        {
            self.selected_folder = folder_index;
        }
        if let Some(message_index) = self
            .messages
            .iter()
            .position(|message| message.id == hit.message_id)
        {
            self.selected_message = message_index;
        }
        self.search = None;
        self.active = ActivePane::Messages;
        self.normalize_active_pane();
        true
    }

    pub(crate) fn selected_message_id(&self) -> Option<MessageId> {
        self.messages.get(self.selected_message).map(|m| m.id)
    }

    pub(crate) fn selected_thread(&self) -> Option<&ThreadItem> {
        self.threads.get(self.selected_thread)
    }

    pub(crate) fn selected_message(&self) -> Option<&MessageItem> {
        self.messages.get(self.selected_message)
    }

    pub(crate) fn selected_message_has_flag(&self, flag: &str) -> Option<bool> {
        self.selected_message()
            .map(|message| message.has_flag(flag))
    }

    pub(crate) fn selected_message_flag_update(
        &self,
        flag: &str,
        enabled: bool,
    ) -> Option<(MessageId, Vec<String>)> {
        self.selected_message()
            .map(|message| (message.id, message.with_flag(flag, enabled)))
    }

    /// Capture the message-list state needed to undo an optimistic
    /// remove. Returned snapshot is opaque to callers and should only
    /// be passed back to [`AppState::restore_message_list_snapshot`].
    pub(crate) fn snapshot_message_list(&self) -> MessageListSnapshot {
        MessageListSnapshot {
            folder_messages: self.folder_messages.clone(),
            selected_thread: self.selected_thread,
            selected_message: self.selected_message,
        }
    }

    /// Drop the message with `message_id` from the visible folder list
    /// and refresh thread/message panes. Returns true when a row was
    /// removed.
    pub(crate) fn remove_message_locally(&mut self, message_id: MessageId) -> bool {
        let before = self.folder_messages.len();
        let selected_thread_key = self.selected_thread().map(|thread| thread.key);
        self.folder_messages
            .retain(|message| message.id != message_id);
        let removed = self.folder_messages.len() != before;
        if !removed {
            return false;
        }
        self.rebuild_threads(selected_thread_key);
        self.refresh_visible_messages();
        if self
            .detail
            .as_ref()
            .is_some_and(|detail| detail.id == message_id)
        {
            self.clear_detail_state();
        }
        self.normalize_active_pane();
        true
    }

    pub(crate) fn restore_message_list_snapshot(&mut self, snapshot: MessageListSnapshot) {
        self.folder_messages = snapshot.folder_messages;
        self.rebuild_threads(None);
        self.selected_thread = snapshot.selected_thread;
        clamp_index(&mut self.selected_thread, self.threads.len());
        self.refresh_visible_messages();
        self.selected_message = snapshot.selected_message;
        clamp_index(&mut self.selected_message, self.messages.len());
        self.normalize_active_pane();
    }

    pub(crate) fn begin_delete_confirmation(&mut self, message_id: MessageId) {
        self.pending_delete_message = Some(message_id);
        self.mode = InputMode::ConfirmDelete;
        self.set_status("Delete? y/n");
    }

    pub(crate) fn cancel_delete_confirmation(&mut self) {
        self.pending_delete_message = None;
        self.mode = InputMode::Normal;
        self.set_status("Delete cancelled");
    }

    pub(crate) fn take_pending_delete_message(&mut self) -> Option<MessageId> {
        let id = self.pending_delete_message.take();
        if id.is_some() {
            self.mode = InputMode::Normal;
        }
        id
    }

    pub(crate) fn apply_message_flags(&mut self, message_id: MessageId, flags: Vec<String>) {
        let selected_thread = self.selected_thread().map(|thread| thread.key);
        if let Some(message) = self
            .folder_messages
            .iter_mut()
            .find(|message| message.id == message_id)
        {
            message.flags = flags.clone();
        }
        if let Some(message) = self
            .messages
            .iter_mut()
            .find(|message| message.id == message_id)
        {
            message.flags = flags.clone();
        }
        if let Some(detail) = &mut self.detail {
            if detail.id == message_id {
                detail.flags = flags;
            }
        }
        if !self.folder_messages.is_empty() {
            self.rebuild_threads(selected_thread);
            self.refresh_visible_messages();
        }
    }

    pub(crate) fn enter_command_mode(&mut self) {
        self.mode = InputMode::Command;
        self.command_input.clear();
        self.clear_error();
        self.set_status("Command mode");
    }

    pub(crate) fn cancel_command_mode(&mut self) {
        self.mode = if self.composer.is_some() {
            InputMode::Compose
        } else {
            InputMode::Normal
        };
        self.command_input.clear();
        self.clear_error();
        self.set_status("Command cancelled");
    }

    pub(crate) fn push_command_char(&mut self, ch: char) -> bool {
        if ch.is_control() || self.command_input.chars().count() >= MAX_COMMAND_CHARS {
            return false;
        }
        self.command_input.push(ch);
        true
    }

    pub(crate) fn backspace_command(&mut self) -> bool {
        self.command_input.pop().is_some()
    }

    pub(crate) fn finish_command(&mut self) -> String {
        // Restore the composer mode if we entered command mode from
        // inside a composer (e.g. `:w`). Otherwise drop back to normal.
        self.mode = if self.composer.is_some() {
            InputMode::Compose
        } else {
            InputMode::Normal
        };
        std::mem::take(&mut self.command_input)
    }

    pub(crate) fn enter_composer(&mut self, account_id: AccountId) {
        self.composer = Some(ComposerState::new(account_id));
        self.mode = InputMode::Compose;
        self.clear_error();
        self.set_status("Compose");
    }

    /// Enter the composer pre-populated with reply / forward state.
    /// The composer is marked dirty so the autosaver flushes it on the
    /// next idle.
    pub(crate) fn enter_composer_with_prefill(
        &mut self,
        account_id: AccountId,
        prefill: ComposerPrefill,
    ) {
        self.composer = Some(ComposerState::from_prefill(account_id, prefill));
        self.mode = InputMode::Compose;
        self.clear_error();
        self.set_status("Compose");
    }

    /// Enter the composer pre-populated with an existing draft so the
    /// user can keep editing. `draft_id` is recorded so subsequent
    /// saves go through `draft.update` rather than creating a new
    /// draft. Restored composers are clean — the dirty flag only
    /// flips once the user starts editing.
    pub(crate) fn enter_composer_for_existing_draft(
        &mut self,
        draft_id: DraftId,
        draft: ComposerDraft,
        focus: ComposeField,
    ) {
        let mut state = ComposerState::new(draft.account_id);
        state.draft_id = Some(draft_id);
        state.to = join_addresses(&draft.to_addrs);
        state.to_cursor = char_count(&state.to);
        state.cc = join_addresses(&draft.cc_addrs);
        state.cc_cursor = char_count(&state.cc);
        state.bcc = join_addresses(&draft.bcc_addrs);
        state.bcc_cursor = char_count(&state.bcc);
        if let Some(subject) = draft.subject {
            state.subject = subject;
            state.subject_cursor = char_count(&state.subject);
        }
        if let Some(body) = draft.text_body {
            state.body = body;
            state.body_cursor = char_count(&state.body);
            state.refresh_body_line_cache();
        }
        state.in_reply_to_msg = draft.in_reply_to_msg;
        state.in_reply_to = draft.in_reply_to;
        state.references_header = draft.references_header;
        state.attachments = draft.attachments;
        state.focused = focus;
        state.dirty = false;
        self.composer = Some(state);
        self.mode = InputMode::Compose;
        self.clear_error();
        self.set_status("Compose");
    }

    pub(crate) fn composer_draft(&self) -> Option<ComposerDraft> {
        self.composer.as_ref().map(ComposerState::draft)
    }

    pub(crate) fn composer_draft_id(&self) -> Option<DraftId> {
        self.composer
            .as_ref()
            .and_then(|composer| composer.draft_id)
    }

    pub(crate) fn composer_account_id(&self) -> Option<AccountId> {
        self.composer.as_ref().map(|composer| composer.account_id)
    }

    pub(crate) fn composer_is_dirty(&self) -> bool {
        self.composer
            .as_ref()
            .is_some_and(|composer| composer.dirty)
    }

    pub(crate) fn mark_composer_saved(&mut self, draft_id: DraftId) {
        if let Some(composer) = &mut self.composer {
            composer.draft_id = Some(draft_id);
            composer.dirty = false;
        }
    }

    pub(crate) fn exit_composer(&mut self) {
        self.composer = None;
        self.mode = InputMode::Normal;
    }

    pub(crate) fn discard_composer(&mut self) {
        self.composer = None;
        self.mode = InputMode::Normal;
        self.clear_error();
        self.set_status("Composer discarded");
    }

    pub(crate) fn composer_needs_discard_confirmation(&self) -> bool {
        self.composer
            .as_ref()
            .is_some_and(|composer| composer.dirty && composer.has_content())
    }

    pub(crate) fn begin_discard_composer_confirmation(&mut self) {
        self.mode = InputMode::ConfirmDiscard;
        self.set_status("Discard unsaved compose? y/n");
    }

    pub(crate) fn cancel_discard_composer_confirmation(&mut self) {
        self.mode = InputMode::Compose;
        self.set_status("Compose");
    }

    pub(crate) fn next_composer_field(&mut self) {
        if let Some(composer) = &mut self.composer {
            composer.focused = composer.focused.next();
            composer.body_preferred_column = None;
        }
    }

    pub(crate) fn previous_composer_field(&mut self) {
        if let Some(composer) = &mut self.composer {
            composer.focused = composer.focused.previous();
            composer.body_preferred_column = None;
        }
    }

    pub(crate) fn push_composer_char(&mut self, ch: char) -> bool {
        let Some(composer) = &mut self.composer else {
            return false;
        };
        if ch.is_control() || composer.field_len() >= composer.field_limit() {
            return false;
        }
        composer.insert_focused_char(ch);
        composer.dirty = true;
        true
    }

    pub(crate) fn backspace_composer(&mut self) -> bool {
        let Some(composer) = &mut self.composer else {
            return false;
        };
        let changed = composer.delete_before_focused_cursor();
        if changed {
            composer.dirty = true;
        }
        changed
    }

    pub(crate) fn delete_composer(&mut self) -> bool {
        let Some(composer) = &mut self.composer else {
            return false;
        };
        let changed = composer.delete_at_focused_cursor();
        if changed {
            composer.dirty = true;
        }
        changed
    }

    pub(crate) fn move_composer_cursor_left(&mut self) -> bool {
        self.composer
            .as_mut()
            .is_some_and(ComposerState::move_focused_cursor_left)
    }

    pub(crate) fn move_composer_cursor_right(&mut self) -> bool {
        self.composer
            .as_mut()
            .is_some_and(ComposerState::move_focused_cursor_right)
    }

    pub(crate) fn composer_home(&mut self) -> bool {
        self.composer
            .as_mut()
            .is_some_and(ComposerState::move_focused_cursor_home)
    }

    pub(crate) fn composer_end(&mut self) -> bool {
        self.composer
            .as_mut()
            .is_some_and(ComposerState::move_focused_cursor_end)
    }

    pub(crate) fn move_composer_body_line(&mut self, delta: isize, viewport_height: usize) -> bool {
        self.composer
            .as_mut()
            .is_some_and(|composer| composer.move_body_line(delta, viewport_height))
    }

    pub(crate) fn toggle_composer_body_line_selection(&mut self) -> bool {
        self.composer
            .as_mut()
            .is_some_and(ComposerState::toggle_body_line_selection)
    }

    pub(crate) fn start_composer_body_line_selection(&mut self) -> bool {
        self.composer
            .as_mut()
            .is_some_and(ComposerState::start_body_line_selection)
    }

    pub(crate) fn clear_composer_body_selection(&mut self) -> bool {
        self.composer
            .as_mut()
            .is_some_and(ComposerState::clear_body_selection)
    }

    pub(crate) fn composer_enter(&mut self) -> bool {
        let Some(composer) = &mut self.composer else {
            return false;
        };
        if composer.focused == ComposeField::Body {
            if composer.body.chars().count() >= MAX_COMPOSE_BODY_CHARS {
                return false;
            }
            composer.insert_body_newline();
            composer.dirty = true;
        } else {
            composer.focused = composer.focused.next();
        }
        true
    }

    /// Open the inline path-input prompt for adding a compose
    /// attachment. Returns `false` if no composer is active.
    pub(crate) fn begin_compose_attach(&mut self) -> bool {
        let Some(composer) = &mut self.composer else {
            return false;
        };
        composer.attach_input.clear();
        self.mode = InputMode::ComposeAttachPath;
        true
    }

    pub(crate) fn cancel_compose_attach(&mut self) {
        if let Some(composer) = &mut self.composer {
            composer.attach_input.clear();
        }
        self.mode = InputMode::Compose;
    }

    pub(crate) fn push_compose_attach_char(&mut self, ch: char) -> bool {
        let Some(composer) = &mut self.composer else {
            return false;
        };
        if ch.is_control() || composer.attach_input.chars().count() >= MAX_COMPOSE_PATH_CHARS {
            return false;
        }
        composer.attach_input.push(ch);
        true
    }

    pub(crate) fn backspace_compose_attach(&mut self) -> bool {
        let Some(composer) = &mut self.composer else {
            return false;
        };
        composer.attach_input.pop().is_some()
    }

    pub(crate) fn compose_attach_input(&self) -> Option<&str> {
        self.composer
            .as_ref()
            .map(|composer| composer.attach_input.as_str())
    }

    /// Try to add the path the user typed in the inline prompt as an
    /// attachment. Returns `Ok(filename)` on success; `Err(AttachError)`
    /// otherwise. On success the input is cleared and we return to
    /// `Compose` mode.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`AttachError::Io`] with an empty path if there is no active
    ///   composer.
    /// - [`AttachError::NotFound`] if the input is empty or the path
    ///   does not exist.
    /// - [`AttachError::NotAFile`] if the path is not a regular file.
    /// - [`AttachError::TooLarge`] if the single attachment exceeds the
    ///   per-file limit.
    /// - [`AttachError::AggregateTooLarge`] if the cumulative size
    ///   would exceed the composer's aggregate cap.
    /// - [`AttachError::Io`] for any other IO failure on stat.
    pub(crate) async fn confirm_compose_attach(&mut self) -> Result<String, AttachError> {
        let Some(composer) = &mut self.composer else {
            return Err(AttachError::Io {
                path: PathBuf::new(),
                message: "no composer".into(),
            });
        };
        let raw = composer.attach_input.trim().to_string();
        if raw.is_empty() {
            return Err(AttachError::NotFound(PathBuf::from("(empty)")));
        }
        let path = PathBuf::from(&raw);
        let attachment = probe_attachment(&path).await?;
        let aggregate = composer
            .aggregate_attachment_size()
            .saturating_add(attachment.size_bytes);
        if aggregate > MAX_COMPOSE_ATTACHMENT_BYTES {
            return Err(AttachError::AggregateTooLarge { total: aggregate });
        }
        let filename = attachment.filename.clone();
        composer.attachments.push(attachment);
        composer.attach_input.clear();
        composer.dirty = true;
        // Land selection on the just-added attachment so `d` works
        // intuitively right after Enter.
        composer.selected_attachment = composer.attachments.len() - 1;
        self.mode = InputMode::Compose;
        Ok(filename)
    }

    /// Remove the currently selected attachment. Index clamps to the
    /// end of the new list. Returns the removed filename if any.
    pub(crate) fn remove_selected_compose_attachment(&mut self) -> Option<String> {
        let composer = self.composer.as_mut()?;
        if composer.attachments.is_empty() {
            return None;
        }
        let index = composer
            .selected_attachment
            .min(composer.attachments.len() - 1);
        let removed = composer.attachments.remove(index);
        if composer.attachments.is_empty() {
            composer.selected_attachment = 0;
        } else if composer.selected_attachment >= composer.attachments.len() {
            composer.selected_attachment = composer.attachments.len() - 1;
        }
        composer.dirty = true;
        Some(removed.filename)
    }

    pub fn move_compose_attachment_selection(&mut self, delta: isize) -> bool {
        let Some(composer) = &mut self.composer else {
            return false;
        };
        if composer.attachments.is_empty() {
            return false;
        }
        move_index(
            &mut composer.selected_attachment,
            composer.attachments.len(),
            delta,
        )
    }

    pub fn selected_compose_attachment(&self) -> Option<&ComposerAttachment> {
        let composer = self.composer.as_ref()?;
        composer.attachments.get(composer.selected_attachment)
    }

    pub(crate) fn cycle_theme(&mut self) -> ThemeName {
        self.theme = self.theme.next();
        self.theme
    }

    pub(crate) fn set_theme(&mut self, theme: ThemeName) {
        self.theme = theme;
    }

    fn rebuild_threads(&mut self, selected_key: Option<Uuid>) {
        self.threads = build_threads(&self.folder_messages);
        if let Some(selected_key) = selected_key {
            if let Some(index) = self
                .threads
                .iter()
                .position(|thread| thread.key == selected_key)
            {
                self.selected_thread = index;
                return;
            }
        }
        clamp_index(&mut self.selected_thread, self.threads.len());
    }

    fn refresh_visible_messages(&mut self) {
        if !self.threads_pane_visible() {
            self.messages = self.folder_messages.clone();
        } else if let Some(thread_key) = self.selected_thread().map(|thread| thread.key) {
            self.messages = self
                .folder_messages
                .iter()
                .filter(|message| message_thread_key(message) == thread_key)
                .cloned()
                .collect();
            sort_messages_oldest_first(&mut self.messages);
        } else {
            self.messages.clear();
        }
        clamp_index(&mut self.selected_message, self.messages.len());
    }

    fn normalize_active_pane(&mut self) {
        if self.active == ActivePane::Threads && !self.threads_pane_visible() {
            self.active = if self.messages.is_empty() {
                ActivePane::Folders
            } else {
                ActivePane::Messages
            };
        }
        if self.active == ActivePane::Details && !self.detail_pane_visible() {
            self.active = if self.messages.is_empty() {
                ActivePane::Folders
            } else {
                ActivePane::Messages
            };
        }
        if self.active == ActivePane::Attachments && !self.attachments_pane_visible() {
            self.active = if self.detail_pane_visible() {
                ActivePane::Details
            } else if self.messages.is_empty() {
                ActivePane::Folders
            } else {
                ActivePane::Messages
            };
        }
        if self.active == ActivePane::Search && !self.search_pane_visible() {
            self.active = if self.messages.is_empty() {
                ActivePane::Folders
            } else {
                ActivePane::Messages
            };
        }
    }

    fn next_visible_pane(&self) -> ActivePane {
        let mut pane = self.active;
        for _ in 0..7 {
            pane = pane.next();
            if self.pane_visible(pane) {
                return pane;
            }
        }
        self.active
    }

    fn previous_visible_pane(&self) -> ActivePane {
        let mut pane = self.active;
        for _ in 0..7 {
            pane = pane.previous();
            if self.pane_visible(pane) {
                return pane;
            }
        }
        self.active
    }

    fn pane_visible(&self, pane: ActivePane) -> bool {
        match pane {
            ActivePane::Threads => self.threads_pane_visible(),
            ActivePane::Details => self.detail_pane_visible(),
            ActivePane::Attachments => self.attachments_pane_visible(),
            ActivePane::Search => self.search_pane_visible(),
            ActivePane::Accounts | ActivePane::Folders | ActivePane::Messages => true,
        }
    }

    fn clear_detail_state(&mut self) {
        self.detail = None;
        self.detail_text_cache = None;
        self.reset_detail_navigation_state();
        self.clear_attachments();
    }

    fn clear_attachments(&mut self) {
        self.attachments.clear();
        self.attachment_preview = None;
        self.selected_attachment = 0;
        self.pending_open_attachment = None;
        self.reset_preview_navigation_state();
        self.normalize_active_pane();
    }

    fn reset_preview_navigation_state(&mut self) {
        self.preview_focused = false;
        self.preview_scroll = 0;
        self.preview_selection = None;
    }

    pub(crate) fn detail_text_content(&self) -> Option<&str> {
        self.detail_text_cache.as_ref().map(TextLineCache::text)
    }

    fn detail_len(&self) -> usize {
        self.detail_text_cache
            .as_ref()
            .map(TextLineCache::char_len)
            .unwrap_or(0)
    }

    fn detail_line_len(&self, line: usize) -> usize {
        self.detail_line_end(line)
            .saturating_sub(self.detail_line_start(line))
    }

    fn set_detail_cursor(&mut self, next: usize) -> bool {
        let len = self.detail_len();
        let next = next.min(len);
        let old = self.detail_cursor.min(len);
        self.detail_cursor = next;
        self.detail_preferred_column = None;
        old != next
    }

    fn ensure_detail_cursor_visible(&mut self, viewport_height: usize) {
        self.detail_scroll = self.detail_visible_scroll(viewport_height);
    }

    fn reset_detail_navigation_state(&mut self) {
        self.detail_cursor = 0;
        self.detail_scroll = 0;
        self.detail_selection_anchor = None;
        self.detail_selection_focus = 0;
        self.detail_preferred_column = None;
    }
}

fn build_threads(messages: &[MessageItem]) -> Vec<ThreadItem> {
    let mut threads = Vec::<ThreadItem>::new();

    for message in messages {
        let key = message_thread_key(message);
        if let Some(thread) = threads.iter_mut().find(|thread| thread.key == key) {
            thread.message_count += 1;
            if message.date > thread.latest_date {
                thread.latest_date = message.date.clone();
                thread.subject = text_or_default(Some(&message.subject), "(no subject)");
            }
            thread.unread |= !message.has_flag(SEEN_FLAG);
            thread.flagged |= message.has_flag(FLAGGED_FLAG);
        } else {
            threads.push(ThreadItem {
                key,
                thread_id: message.thread_id,
                subject: text_or_default(Some(&message.subject), "(no subject)"),
                message_count: 1,
                latest_date: message.date.clone(),
                unread: !message.has_flag(SEEN_FLAG),
                flagged: message.has_flag(FLAGGED_FLAG),
            });
        }
    }

    threads.sort_by(|left, right| {
        right
            .latest_date
            .cmp(&left.latest_date)
            .then_with(|| left.subject.cmp(&right.subject))
            .then_with(|| left.key.cmp(&right.key))
    });
    threads
}

fn message_thread_key(message: &MessageItem) -> Uuid {
    message
        .thread_id
        .map(ThreadId::into_inner)
        .unwrap_or_else(|| message.id.into_inner())
}

fn sort_messages_oldest_first(messages: &mut [MessageItem]) {
    messages.sort_by(|left, right| {
        left.date
            .cmp(&right.date)
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn text_or_default(value: Option<&str>, default: &str) -> String {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(default)
        .to_string()
}

fn detail_text(detail: &MessageDetail) -> String {
    format!(
        "Subject: {}\nFrom: {}\nSnippet: {}\n\n{}",
        detail.subject, detail.from, detail.snippet, detail.body
    )
}

/// Render a JSON address array as a comma-joined label for a Drafts
/// row. Empty / non-array inputs map to the literal "(no recipient)"
/// so Drafts without a To: still surface in the list.
fn addrs_label(value: &AddressList) -> String {
    let collected: Vec<&str> = value
        .as_slice()
        .iter()
        .map(String::as_str)
        .filter(|s| !s.trim().is_empty())
        .collect();
    if collected.is_empty() {
        "(no recipient)".into()
    } else {
        collected.join(", ")
    }
}

/// First non-empty line of a draft's text body, used as the snippet
/// in the Drafts row. Empty bodies render as "(empty)".
fn first_line_or_default(value: Option<&str>) -> String {
    let Some(text) = value else {
        return "(empty)".into();
    };
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.to_string())
        .unwrap_or_else(|| "(empty)".into())
}

fn non_empty_string(value: &str) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// Format a byte size in IEC units up to GiB. Single decimal place.
pub(crate) fn human_size(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * 1024 * 1024;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// Probe a candidate attachment path: returns metadata + content-type
/// or a structured error suitable for toast surfacing.
///
/// # Errors
///
/// Returns:
/// - [`AttachError::NotFound`] if the path does not exist.
/// - [`AttachError::NotAFile`] if the path is not a regular file
///   (e.g. directory, symlink to a directory).
/// - [`AttachError::TooLarge`] if the file exceeds the per-attachment
///   size cap.
/// - [`AttachError::Io`] for any other `tokio::fs::metadata` failure
///   (permissions, IO error).
pub(crate) async fn probe_attachment(path: &Path) -> Result<ComposerAttachment, AttachError> {
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => AttachError::NotFound(path.to_path_buf()),
            _ => AttachError::Io {
                path: path.to_path_buf(),
                message: e.to_string(),
            },
        })?;
    if !metadata.is_file() {
        return Err(AttachError::NotAFile(path.to_path_buf()));
    }
    let size = metadata.len();
    if size > MAX_COMPOSE_ATTACHMENT_BYTES {
        return Err(AttachError::TooLarge { size });
    }
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("attachment.bin")
        .to_string();
    Ok(ComposerAttachment {
        path: path.to_path_buf(),
        filename,
        size_bytes: size,
        content_type: crate::attachments::guess_content_type_for_path(path),
    })
}

fn split_addresses(value: &str) -> Vec<String> {
    value
        .split([',', ';', '\n'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

fn join_addresses(values: &[String]) -> String {
    values
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

fn char_count(value: &str) -> usize {
    value.chars().count()
}

fn char_to_byte_index(value: &str, char_index: usize) -> usize {
    if char_index == 0 {
        return 0;
    }
    value
        .char_indices()
        .nth(char_index)
        .map(|(index, _)| index)
        .unwrap_or(value.len())
}

fn line_for_cursor(bounds: &[LineBounds], cursor: usize) -> usize {
    bounds
        .partition_point(|bounds| bounds.char_end < cursor)
        .min(bounds.len().saturating_sub(1))
}

fn move_index(index: &mut usize, len: usize, delta: isize) -> bool {
    if len == 0 {
        *index = 0;
        return false;
    }

    let old = (*index).min(len - 1);
    let next = if delta < 0 {
        old.saturating_sub((-delta) as usize)
    } else {
        old.saturating_add(delta as usize).min(len - 1)
    };
    *index = next;
    next != old
}

fn clamp_index(index: &mut usize, len: usize) {
    if len == 0 {
        *index = 0;
    } else {
        *index = (*index).min(len - 1);
    }
}

fn clamp_isize(value: isize, min: isize, max: isize) -> isize {
    value.max(min).min(max.max(min))
}

pub(crate) fn flags_from_value(value: &MessageFlags) -> Vec<String> {
    value.to_vec()
}

pub(crate) fn has_flag(flags: &[String], flag: &str) -> bool {
    flags.iter().any(|existing| existing == flag)
}

fn short_id(id: AccountId) -> String {
    id.as_uuid().simple().to_string().chars().take(8).collect()
}

pub(crate) fn set_flag_preserving(flags: &[String], flag: &str, enabled: bool) -> Vec<String> {
    let mut out = Vec::with_capacity(flags.len() + usize::from(enabled));
    let mut saw_target = false;

    for existing in flags {
        if existing == flag {
            saw_target = true;
            if enabled && !has_flag(&out, flag) {
                out.push(existing.clone());
            }
        } else {
            out.push(existing.clone());
        }
    }

    if enabled && !saw_target {
        out.push(flag.to_string());
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(label: &str) -> AccountItem {
        AccountItem {
            id: AccountId::new(),
            label: label.into(),
            email: format!("{label}@example.com"),
            status: "idle".into(),
        }
    }

    fn folder(name: &str) -> FolderItem {
        FolderItem {
            id: FolderId::new(),
            name: name.into(),
            role: "custom".into(),
        }
    }

    fn message(subject: &str) -> MessageItem {
        MessageItem {
            id: MessageId::new(),
            thread_id: None,
            subject: subject.into(),
            from: "alice@example.com".into(),
            date: "2026-05-07 10:00".into(),
            snippet: "hello".into(),
            flags: Vec::new(),
        }
    }

    fn attachment(filename: &str) -> AttachmentItem {
        AttachmentItem {
            id: AttachmentId::new(),
            message_id: MessageId::new(),
            filename: filename.into(),
            content_type: "text/plain".into(),
            size_bytes: 12,
            disposition: "attachment".into(),
            storage_path: format!("/tmp/{filename}"),
        }
    }

    fn draft_attachment_payload(content_base64: &str) -> crate::tui::ipc::DraftAttachmentPayload {
        crate::tui::ipc::DraftAttachmentPayload {
            id: Uuid::new_v4(),
            draft_id: DraftId::new(),
            filename: "notes.txt".into(),
            content_type: "text/plain".into(),
            size_bytes: 0,
            content_base64: content_base64.into(),
        }
    }

    fn detail(message_id: MessageId, body: &str) -> MessageDetail {
        MessageDetail {
            id: message_id,
            subject: "hello".into(),
            from: "alice@example.com".into(),
            snippet: "snippet".into(),
            body: body.into(),
            flags: Vec::new(),
        }
    }

    fn thread_message(
        thread_id: ThreadId,
        subject: &str,
        date: &str,
        flags: &[&str],
    ) -> MessageItem {
        MessageItem {
            id: MessageId::new(),
            thread_id: Some(thread_id),
            subject: subject.into(),
            from: "alice@example.com".into(),
            date: date.into(),
            snippet: "hello".into(),
            flags: flags.iter().map(|flag| (*flag).to_string()).collect(),
        }
    }

    #[test]
    fn test_draft_attachment_decode_failure_keeps_bytes_absent() {
        let attachment = DraftAttachmentBytes::from(draft_attachment_payload("not valid base64"));

        assert!(attachment.bytes.is_none());
        assert!(attachment.decode_error.is_some());
    }

    #[test]
    fn test_draft_attachment_decode_allows_legitimate_empty_file() {
        let attachment = DraftAttachmentBytes::from(draft_attachment_payload(""));

        assert_eq!(attachment.bytes, Some(Vec::new()));
        assert!(attachment.decode_error.is_none());
    }

    #[test]
    fn test_cycle_active_pane_skips_threads_when_hidden() {
        let mut app = AppState::default();
        assert_eq!(app.active, ActivePane::Accounts);
        app.cycle_active_pane();
        assert_eq!(app.active, ActivePane::Folders);
        app.cycle_active_pane();
        assert_eq!(app.active, ActivePane::Messages);
        app.cycle_active_pane();
        assert_eq!(app.active, ActivePane::Accounts);
    }

    #[test]
    fn test_cycle_active_pane_includes_threads_when_visible() {
        let thread_id = ThreadId::new();
        let mut app = AppState::default();
        app.apply_folder_messages(vec![
            thread_message(thread_id, "reply", "2026-05-07 11:00", &[SEEN_FLAG]),
            thread_message(thread_id, "start", "2026-05-07 10:00", &[SEEN_FLAG]),
        ]);

        assert!(app.threads_pane_visible());
        app.cycle_active_pane();
        assert_eq!(app.active, ActivePane::Folders);
        app.cycle_active_pane();
        assert_eq!(app.active, ActivePane::Threads);
        app.cycle_active_pane();
        assert_eq!(app.active, ActivePane::Messages);
        app.cycle_active_pane();
        assert_eq!(app.active, ActivePane::Accounts);
    }

    #[test]
    fn test_move_selection_clamps_at_list_boundaries() {
        let mut app = AppState::default();
        app.apply_accounts(vec![account("one"), account("two")]);

        assert!(!app.move_selection(-1));
        assert_eq!(app.selected_account, 0);
        assert!(app.move_selection(1));
        assert_eq!(app.selected_account, 1);
        assert!(!app.move_selection(1));
        assert_eq!(app.selected_account, 1);
    }

    #[test]
    fn test_move_account_clears_dependent_folder_and_message_state() {
        let mut app = AppState::default();
        app.apply_accounts(vec![account("one"), account("two")]);
        app.apply_folders(vec![folder("INBOX")]);
        app.apply_messages(vec![message("hello")]);
        app.apply_detail(Some(MessageDetail {
            id: app.messages[0].id,
            subject: "hello".into(),
            from: "alice@example.com".into(),
            snippet: "hello".into(),
            body: "body".into(),
            flags: Vec::new(),
        }));

        assert!(app.move_selection(1));

        assert!(app.folders.is_empty());
        assert!(app.folder_messages.is_empty());
        assert!(app.threads.is_empty());
        assert!(app.messages.is_empty());
        assert!(app.detail.is_none());
        assert_eq!(app.selected_folder, 0);
        assert_eq!(app.selected_thread, 0);
        assert_eq!(app.selected_message, 0);
    }

    #[test]
    fn test_move_message_clears_stale_detail() {
        let mut app = AppState {
            active: ActivePane::Messages,
            ..Default::default()
        };
        app.apply_messages(vec![message("one"), message("two")]);
        app.apply_detail(Some(MessageDetail {
            id: app.messages[0].id,
            subject: "one".into(),
            from: "alice@example.com".into(),
            snippet: "hello".into(),
            body: "body".into(),
            flags: Vec::new(),
        }));

        assert!(app.move_selection(1));

        assert_eq!(app.selected_message, 1);
        assert!(app.detail.is_none());
    }

    #[test]
    fn test_move_thread_filters_messages_and_clears_stale_detail() {
        let first_thread = ThreadId::new();
        let second_thread = ThreadId::new();
        let mut app = AppState {
            active: ActivePane::Threads,
            ..Default::default()
        };
        app.apply_folder_messages(vec![
            thread_message(second_thread, "new", "2026-05-07 12:00", &[SEEN_FLAG]),
            thread_message(first_thread, "old latest", "2026-05-07 11:00", &[SEEN_FLAG]),
            thread_message(first_thread, "old first", "2026-05-07 09:00", &[SEEN_FLAG]),
        ]);
        app.apply_detail(Some(MessageDetail {
            id: app.messages[0].id,
            subject: "new".into(),
            from: "alice@example.com".into(),
            snippet: "hello".into(),
            body: "body".into(),
            flags: Vec::new(),
        }));

        assert!(app.move_selection(1));

        assert_eq!(app.selected_thread().unwrap().thread_id, Some(first_thread));
        assert_eq!(app.messages.len(), 2);
        assert_eq!(app.messages[0].subject, "old first");
        assert_eq!(app.messages[1].subject, "old latest");
        assert!(app.detail.is_none());
    }

    #[test]
    fn test_apply_accounts_empty_resets_selection_and_children() {
        let mut app = AppState {
            selected_account: 3,
            ..Default::default()
        };
        app.folders.push(folder("INBOX"));
        app.folder_messages.push(message("hello"));
        app.threads.push(ThreadItem {
            key: Uuid::new_v4(),
            thread_id: None,
            subject: "hello".into(),
            message_count: 1,
            latest_date: "2026-05-07 10:00".into(),
            unread: true,
            flagged: false,
        });
        app.messages.push(message("hello"));

        app.apply_accounts(Vec::new());

        assert_eq!(app.selected_account, 0);
        assert!(app.folders.is_empty());
        assert!(app.folder_messages.is_empty());
        assert!(app.threads.is_empty());
        assert!(app.messages.is_empty());
        assert!(app.detail.is_none());
    }

    #[test]
    fn test_apply_messages_clamps_selection_after_refresh() {
        let mut app = AppState {
            selected_message: 5,
            ..Default::default()
        };

        app.apply_messages(vec![message("only")]);

        assert_eq!(app.selected_message, 0);
        assert_eq!(app.selected_message_id(), Some(app.messages[0].id));
    }

    #[test]
    fn test_apply_folder_messages_groups_threads_with_counts_latest_and_indicators() {
        let older_thread = ThreadId::new();
        let latest_thread = ThreadId::new();
        let single = message("single");
        let single_id = single.id;
        let mut app = AppState::default();

        app.apply_folder_messages(vec![
            thread_message(
                older_thread,
                "older reply",
                "2026-05-07 09:00",
                &[SEEN_FLAG],
            ),
            thread_message(latest_thread, "latest", "2026-05-07 12:00", &[FLAGGED_FLAG]),
            single,
            thread_message(
                older_thread,
                "older start",
                "2026-05-07 08:00",
                &[SEEN_FLAG],
            ),
        ]);

        assert!(app.threads_pane_visible());
        assert_eq!(app.threads.len(), 3);
        assert_eq!(app.threads[0].thread_id, Some(latest_thread));
        assert_eq!(app.threads[0].subject, "latest");
        assert_eq!(app.threads[0].message_count, 1);
        assert_eq!(app.threads[0].latest_date, "2026-05-07 12:00");
        assert!(app.threads[0].unread);
        assert!(app.threads[0].flagged);

        assert_eq!(app.threads[1].key, single_id.into_inner());
        assert_eq!(app.threads[1].thread_id, None);
        assert_eq!(app.threads[1].message_count, 1);

        assert_eq!(app.threads[2].thread_id, Some(older_thread));
        assert_eq!(app.threads[2].message_count, 2);
        assert!(!app.threads[2].unread);
        assert!(!app.threads[2].flagged);
    }

    #[test]
    fn test_apply_folder_messages_singletons_hide_threads_and_show_all_messages() {
        let mut newer = message("newer");
        newer.date = "2026-05-07 12:00".into();
        let newer_id = newer.id;
        let mut older = message("older");
        older.date = "2026-05-07 09:00".into();
        let older_id = older.id;
        let mut app = AppState::default();

        app.apply_folder_messages(vec![newer, older]);

        assert!(!app.threads_pane_visible());
        assert_eq!(app.threads.len(), 2);
        assert_eq!(
            app.messages
                .iter()
                .map(|message| message.id)
                .collect::<Vec<_>>(),
            vec![newer_id, older_id]
        );
    }

    #[test]
    fn test_apply_folder_messages_moves_active_threads_when_pane_becomes_hidden() {
        let thread_id = ThreadId::new();
        let mut app = AppState {
            active: ActivePane::Threads,
            ..Default::default()
        };
        app.apply_folder_messages(vec![
            thread_message(thread_id, "reply", "2026-05-07 11:00", &[SEEN_FLAG]),
            thread_message(thread_id, "start", "2026-05-07 10:00", &[SEEN_FLAG]),
        ]);
        app.active = ActivePane::Threads;

        app.apply_folder_messages(vec![message("single")]);

        assert!(!app.threads_pane_visible());
        assert_eq!(app.active, ActivePane::Messages);
        assert_eq!(app.messages.len(), 1);
    }

    #[test]
    fn test_apply_folder_messages_moves_active_threads_to_folders_when_empty() {
        let thread_id = ThreadId::new();
        let mut app = AppState {
            active: ActivePane::Threads,
            ..Default::default()
        };
        app.apply_folder_messages(vec![
            thread_message(thread_id, "reply", "2026-05-07 11:00", &[SEEN_FLAG]),
            thread_message(thread_id, "start", "2026-05-07 10:00", &[SEEN_FLAG]),
        ]);
        app.active = ActivePane::Threads;

        app.apply_folder_messages(Vec::new());

        assert!(!app.threads_pane_visible());
        assert_eq!(app.active, ActivePane::Folders);
        assert!(app.messages.is_empty());
    }

    #[test]
    fn test_apply_folder_messages_filters_selected_thread_oldest_first() {
        let first_thread = ThreadId::new();
        let second_thread = ThreadId::new();
        let mut app = AppState::default();
        app.apply_folder_messages(vec![
            thread_message(second_thread, "other", "2026-05-07 12:00", &[SEEN_FLAG]),
            thread_message(first_thread, "reply", "2026-05-07 11:00", &[SEEN_FLAG]),
            thread_message(first_thread, "start", "2026-05-07 09:00", &[SEEN_FLAG]),
        ]);

        app.active = ActivePane::Threads;
        assert!(app.move_selection(1));

        assert_eq!(app.selected_thread().unwrap().thread_id, Some(first_thread));
        assert_eq!(app.messages.len(), 2);
        assert_eq!(app.messages[0].subject, "start");
        assert_eq!(app.messages[1].subject, "reply");
    }

    #[test]
    fn test_apply_folder_messages_clamps_selection_when_thread_disappears() {
        let first_thread = ThreadId::new();
        let second_thread = ThreadId::new();
        let mut app = AppState::default();
        app.apply_folder_messages(vec![
            thread_message(first_thread, "first", "2026-05-07 12:00", &[SEEN_FLAG]),
            thread_message(second_thread, "second", "2026-05-07 11:00", &[SEEN_FLAG]),
        ]);
        app.selected_thread = 1;

        app.apply_folder_messages(vec![thread_message(
            first_thread,
            "first",
            "2026-05-07 13:00",
            &[SEEN_FLAG],
        )]);

        assert_eq!(app.selected_thread, 0);
        assert_eq!(app.selected_thread().unwrap().thread_id, Some(first_thread));
        assert_eq!(app.messages.len(), 1);
    }

    #[test]
    fn test_apply_folder_messages_resets_selected_message_for_multi_message_replacement_thread() {
        let top_thread = ThreadId::new();
        let disappearing_thread = ThreadId::new();
        let replacement_thread = ThreadId::new();
        let mut app = AppState {
            active: ActivePane::Threads,
            ..Default::default()
        };
        app.apply_folder_messages(vec![
            thread_message(top_thread, "top", "2026-05-07 13:00", &[SEEN_FLAG]),
            thread_message(
                disappearing_thread,
                "gone latest",
                "2026-05-07 12:00",
                &[SEEN_FLAG],
            ),
            thread_message(
                disappearing_thread,
                "gone first",
                "2026-05-07 10:00",
                &[SEEN_FLAG],
            ),
        ]);
        assert!(app.move_selection(1));
        app.selected_message = 1;

        app.apply_folder_messages(vec![
            thread_message(
                replacement_thread,
                "replacement reply",
                "2026-05-07 15:00",
                &[SEEN_FLAG],
            ),
            thread_message(
                replacement_thread,
                "replacement first",
                "2026-05-07 14:00",
                &[SEEN_FLAG],
            ),
        ]);

        assert_eq!(app.selected_thread, 0);
        assert_eq!(
            app.selected_thread().unwrap().thread_id,
            Some(replacement_thread)
        );
        assert_eq!(app.selected_message, 0);
        assert_eq!(app.messages.len(), 2);
        assert_eq!(app.messages[0].subject, "replacement first");
        assert_eq!(app.messages[1].subject, "replacement reply");
    }

    #[test]
    fn test_command_mode_supports_editing_cancel_and_submit() {
        let mut app = AppState::default();

        app.enter_command_mode();
        assert_eq!(app.mode, InputMode::Command);
        assert!(app.push_command_char('s'));
        assert!(app.push_command_char('y'));
        assert!(app.backspace_command());
        assert!(app.push_command_char('n'));
        assert_eq!(app.command_input, "sn");

        app.cancel_command_mode();
        assert_eq!(app.mode, InputMode::Normal);
        assert!(app.command_input.is_empty());
        assert_eq!(app.status, "Command cancelled");

        app.enter_command_mode();
        for ch in "theme next".chars() {
            assert!(app.push_command_char(ch));
        }
        assert_eq!(app.finish_command(), "theme next");
        assert_eq!(app.mode, InputMode::Normal);
        assert!(app.command_input.is_empty());
    }

    #[test]
    fn test_command_input_is_bounded() {
        let mut app = AppState::default();
        app.enter_command_mode();

        for _ in 0..MAX_COMMAND_CHARS {
            assert!(app.push_command_char('x'));
        }

        assert!(!app.push_command_char('y'));
        assert_eq!(app.command_input.chars().count(), MAX_COMMAND_CHARS);
    }

    #[test]
    fn test_theme_cycle_wraps_to_light() {
        let mut app = AppState::default();

        assert_eq!(app.theme, ThemeName::Light);
        assert_eq!(app.cycle_theme(), ThemeName::Dark);
        assert_eq!(app.cycle_theme(), ThemeName::HighContrast);
        assert_eq!(app.cycle_theme(), ThemeName::Light);
    }

    #[test]
    fn test_set_theme_unknown_string_via_from_str_leaves_state_unchanged() {
        let mut app = AppState::default();
        app.set_theme(ThemeName::Dark);

        // FromStr path: an unknown name produces an error and the
        // caller is responsible for not applying it. Confirm
        // set_theme does not mutate when given the existing value
        // either, so :theme bogus → toast → theme unchanged remains
        // a routing-layer concern (verified separately).
        assert!("bogus".parse::<ThemeName>().is_err());
        assert_eq!(app.theme, ThemeName::Dark);
    }

    #[test]
    fn test_flags_from_value_clones_typed_flags() {
        let flags = flags_from_value(&MessageFlags::from(vec!["\\Seen", "\\Flagged"]));

        assert_eq!(flags, vec!["\\Seen", "\\Flagged"]);
    }

    #[test]
    fn test_set_flag_preserving_adds_and_removes_target_without_losing_other_flags() {
        let flags = vec!["\\Answered".to_string(), "\\Flagged".to_string()];

        let seen = set_flag_preserving(&flags, SEEN_FLAG, true);
        assert_eq!(seen, vec!["\\Answered", "\\Flagged", "\\Seen"]);

        let unflagged = set_flag_preserving(&seen, FLAGGED_FLAG, false);
        assert_eq!(unflagged, vec!["\\Answered", "\\Seen"]);
    }

    #[test]
    fn test_set_flag_preserving_collapses_duplicate_target_flags() {
        let flags = vec![
            "\\Seen".to_string(),
            "\\Answered".to_string(),
            "\\Seen".to_string(),
        ];

        let seen = set_flag_preserving(&flags, SEEN_FLAG, true);

        assert_eq!(seen, vec!["\\Seen", "\\Answered"]);
    }

    #[test]
    fn test_selected_message_flag_update_preserves_existing_flags() {
        let mut app = AppState {
            active: ActivePane::Messages,
            ..Default::default()
        };
        let mut selected = message("hello");
        selected.flags = vec!["\\Answered".into()];
        let message_id = selected.id;
        app.apply_messages(vec![selected]);

        let update = app
            .selected_message_flag_update(SEEN_FLAG, true)
            .expect("selected message");

        assert_eq!(update.0, message_id);
        assert_eq!(update.1, vec!["\\Answered", "\\Seen"]);
    }

    #[test]
    fn test_apply_message_flags_updates_list_and_detail() {
        let mut app = AppState::default();
        let selected = message("hello");
        let message_id = selected.id;
        app.apply_messages(vec![selected]);
        app.apply_detail(Some(MessageDetail {
            id: message_id,
            subject: "hello".into(),
            from: "alice@example.com".into(),
            snippet: "hello".into(),
            body: "body".into(),
            flags: vec!["\\Seen".into()],
        }));

        app.apply_message_flags(message_id, vec!["\\Flagged".into()]);

        assert_eq!(app.messages[0].flags, vec!["\\Flagged"]);
        assert_eq!(app.detail.as_ref().unwrap().flags, vec!["\\Flagged"]);
    }

    #[test]
    fn test_apply_message_flags_updates_thread_indicators() {
        let thread_id = ThreadId::new();
        let mut app = AppState::default();
        let selected = thread_message(thread_id, "hello", "2026-05-07 10:00", &[SEEN_FLAG]);
        let message_id = selected.id;
        app.apply_folder_messages(vec![selected]);

        assert!(!app.threads[0].unread);
        assert!(!app.threads[0].flagged);

        app.apply_message_flags(message_id, vec![FLAGGED_FLAG.into()]);

        assert!(app.threads[0].unread);
        assert!(app.threads[0].flagged);
        assert_eq!(app.messages[0].flags, vec![FLAGGED_FLAG]);
    }

    #[test]
    fn test_apply_message_flags_in_direct_message_mode_updates_messages_and_thread_state() {
        let mut selected = message("selected");
        selected.flags = vec![SEEN_FLAG.into()];
        let message_id = selected.id;
        let mut app = AppState::default();
        app.apply_folder_messages(vec![selected, message("other")]);

        assert!(!app.threads_pane_visible());

        app.apply_message_flags(message_id, vec![SEEN_FLAG.into(), FLAGGED_FLAG.into()]);

        let folder_message = app
            .folder_messages
            .iter()
            .find(|message| message.id == message_id)
            .expect("folder message");
        let list_message = app
            .messages
            .iter()
            .find(|message| message.id == message_id)
            .expect("list message");
        let thread = app
            .threads
            .iter()
            .find(|thread| thread.key == message_id.into_inner())
            .expect("thread group");

        assert_eq!(folder_message.flags, vec![SEEN_FLAG, FLAGGED_FLAG]);
        assert_eq!(list_message.flags, vec![SEEN_FLAG, FLAGGED_FLAG]);
        assert!(thread.flagged);
        assert!(!thread.unread);
    }

    #[test]
    fn test_apply_detail_updates_thread_indicators_from_fresh_flags() {
        let thread_id = ThreadId::new();
        let mut app = AppState::default();
        let selected = thread_message(thread_id, "hello", "2026-05-07 10:00", &[SEEN_FLAG]);
        let message_id = selected.id;
        app.apply_folder_messages(vec![selected]);

        app.apply_detail(Some(MessageDetail {
            id: message_id,
            subject: "hello".into(),
            from: "alice@example.com".into(),
            snippet: "hello".into(),
            body: "body".into(),
            flags: vec![FLAGGED_FLAG.into()],
        }));

        assert!(app.threads[0].unread);
        assert!(app.threads[0].flagged);
        assert_eq!(app.messages[0].flags, vec![FLAGGED_FLAG]);
    }

    #[test]
    fn test_attachment_pane_visibility_and_cycle_skips_hidden() {
        let mut app = AppState::default();
        app.apply_messages(vec![message("hello")]);
        app.active = ActivePane::Messages;

        assert!(!app.attachments_pane_visible());
        app.cycle_active_pane();
        assert_eq!(app.active, ActivePane::Accounts);

        let message_id = app.messages[0].id;
        app.apply_detail(Some(detail(message_id, "body")));
        app.apply_attachments(vec![attachment("notes.txt")]);

        assert!(app.attachments_pane_visible());
        app.active = ActivePane::Messages;
        app.cycle_active_pane();
        assert_eq!(app.active, ActivePane::Details);
        app.cycle_active_pane();
        assert_eq!(app.active, ActivePane::Attachments);
        app.cycle_active_pane();
        assert_eq!(app.active, ActivePane::Accounts);
        app.cycle_active_pane_reverse();
        assert_eq!(app.active, ActivePane::Attachments);
        app.cycle_active_pane_reverse();
        assert_eq!(app.active, ActivePane::Details);
    }

    #[test]
    fn test_attachment_selection_and_preview_follow_selected_attachment() {
        let mut app = AppState {
            active: ActivePane::Attachments,
            ..Default::default()
        };
        let first = attachment("first.txt");
        let first_id = first.id;
        let second = attachment("second.txt");
        let second_id = second.id;
        app.apply_detail(Some(MessageDetail {
            id: first.message_id,
            subject: "hello".into(),
            from: "alice@example.com".into(),
            snippet: "hello".into(),
            body: "body".into(),
            flags: Vec::new(),
        }));
        app.apply_attachments(vec![first, second]);
        app.active = ActivePane::Attachments;

        assert_eq!(app.selected_attachment_id(), Some(first_id));
        assert!(app.move_selection(1));
        assert_eq!(app.selected_attachment_id(), Some(second_id));
        assert!(!app.move_selection(1));

        app.apply_attachment_preview(AttachmentPreviewItem {
            attachment_id: second_id,
            text: Some("hello attachment".into()),
            message: "Inline preview".into(),
            truncated: false,
            preview_bytes: 16,
        });

        let preview = app.attachment_preview.as_ref().unwrap();
        assert_eq!(preview.attachment_id, second_id);
        assert_eq!(preview.text.as_deref(), Some("hello attachment"));
    }

    #[test]
    fn test_detail_pane_visibility_cycle_and_direct_focus_require_detail() {
        let mut app = AppState {
            active: ActivePane::Messages,
            ..Default::default()
        };
        app.apply_messages(vec![message("hello")]);

        assert!(!app.detail_pane_visible());
        assert!(!app.focus_detail_pane());
        assert_eq!(app.active, ActivePane::Messages);
        app.cycle_active_pane();
        assert_eq!(app.active, ActivePane::Accounts);

        let message_id = app.messages[0].id;
        app.apply_detail(Some(detail(message_id, "body")));
        app.active = ActivePane::Messages;

        assert!(app.detail_pane_visible());
        app.cycle_active_pane();
        assert_eq!(app.active, ActivePane::Details);
        app.cycle_active_pane();
        assert_eq!(app.active, ActivePane::Accounts);
        app.cycle_active_pane_reverse();
        assert_eq!(app.active, ActivePane::Details);

        app.active = ActivePane::Accounts;
        assert!(app.focus_detail_pane());
        assert_eq!(app.active, ActivePane::Details);
    }

    #[test]
    fn test_detail_cursor_line_navigation_and_horizontal_bounds() {
        let mut app = AppState {
            active: ActivePane::Details,
            ..Default::default()
        };
        app.apply_detail(Some(detail(MessageId::new(), "alpha\nb\nemoji café")));

        assert_eq!(app.detail_cursor_line_column(), (0, 0));
        assert!(!app.move_detail_cursor_left());

        assert!(app.detail_end());
        let subject_len = "Subject: hello".chars().count();
        assert_eq!(app.detail_cursor_line_column(), (0, subject_len));
        assert!(!app.move_detail_cursor_right());

        assert!(app.move_detail_cursor_left());
        assert_eq!(app.detail_cursor_line_column(), (0, subject_len - 1));
        assert!(app.detail_home());
        assert_eq!(app.detail_cursor_line_column(), (0, 0));

        assert!(app.move_detail_line(4, 10));
        assert_eq!(app.detail_cursor_line_column(), (4, 0));
        for _ in 0.."alpha".chars().count() {
            assert!(app.move_detail_cursor_right());
        }
        assert_eq!(app.detail_cursor_line_column(), (4, 5));
        assert!(!app.move_detail_cursor_right());

        assert!(app.move_detail_line(1, 10));
        assert_eq!(app.detail_cursor_line_column(), (5, 1));
        assert!(app.move_detail_line(-1, 10));
        assert_eq!(app.detail_cursor_line_column(), (4, 5));
    }

    #[test]
    fn test_detail_page_movement_updates_scroll_and_keeps_cursor_visible() {
        let body = (1..=10)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = AppState {
            active: ActivePane::Details,
            ..Default::default()
        };
        app.apply_detail(Some(detail(MessageId::new(), &body)));

        assert!(app.move_detail_line(5, 3));
        assert_eq!(app.detail_cursor_line_column().0, 5);
        assert_eq!(app.detail_scroll, 3);
        assert_eq!(app.detail_visible_scroll(3), 3);

        assert!(app.move_detail_line(3, 3));
        assert_eq!(app.detail_cursor_line_column().0, 8);
        assert_eq!(app.detail_scroll, 6);
        assert_eq!(app.detail_visible_scroll(3), 6);

        assert!(app.move_detail_line(-6, 3));
        assert_eq!(app.detail_cursor_line_column().0, 2);
        assert_eq!(app.detail_scroll, 2);
        assert_eq!(app.detail_visible_scroll(3), 2);
    }

    #[test]
    fn test_detail_visual_line_selection_toggles_extends_and_clears() {
        let mut app = AppState {
            active: ActivePane::Details,
            ..Default::default()
        };
        app.apply_detail(Some(detail(MessageId::new(), "one\ntwo\nthree")));

        assert!(app.toggle_detail_line_selection());
        assert_eq!(app.detail_selected_line_range(), Some(0..=0));

        assert!(app.move_detail_line(5, 10));
        assert_eq!(app.detail_selected_line_range(), Some(0..=5));

        assert!(app.toggle_detail_line_selection());
        assert_eq!(app.detail_selected_line_range(), None);

        assert!(app.start_detail_line_selection());
        assert_eq!(app.detail_selected_line_range(), Some(5..=5));
        assert!(app.move_detail_line(-1, 10));
        assert_eq!(app.detail_selected_line_range(), Some(4..=5));

        assert!(app.clear_detail_selection());
        assert_eq!(app.detail_selected_line_range(), None);
    }

    #[test]
    fn test_apply_detail_resets_detail_navigation_state() {
        let mut app = AppState {
            active: ActivePane::Details,
            ..Default::default()
        };
        app.apply_detail(Some(detail(MessageId::new(), "one\ntwo\nthree\nfour")));
        assert!(app.move_detail_line(5, 2));
        assert!(app.toggle_detail_line_selection());

        assert_ne!(app.detail_cursor, 0);
        assert_ne!(app.detail_scroll, 0);
        assert!(app.detail_selected_line_range().is_some());

        app.apply_detail(Some(detail(MessageId::new(), "replacement")));

        assert_eq!(app.detail_cursor, 0);
        assert_eq!(app.detail_scroll, 0);
        assert_eq!(app.detail_selected_line_range(), None);
        assert_eq!(app.detail_cursor_line_column(), (0, 0));
    }

    #[test]
    fn test_apply_detail_rebuilds_cached_text_lines_and_bounds() {
        let mut app = AppState {
            active: ActivePane::Details,
            ..Default::default()
        };

        app.apply_detail(Some(detail(MessageId::new(), "alpha\nemoji café")));

        assert_eq!(
            app.detail_lines(),
            vec![
                "Subject: hello".to_string(),
                "From: alice@example.com".to_string(),
                "Snippet: snippet".to_string(),
                String::new(),
                "alpha".to_string(),
                "emoji café".to_string(),
            ]
        );
        assert_eq!(app.detail_line_text(5), Some("emoji café"));
        assert_eq!(app.detail_line_count(), 6);
        app.detail_cursor = app.detail_line_end(5);
        assert_eq!(
            app.detail_cursor_line_column(),
            (5, "emoji café".chars().count())
        );

        app.apply_detail(Some(detail(MessageId::new(), "replacement")));

        assert_eq!(
            app.detail_lines(),
            vec![
                "Subject: hello".to_string(),
                "From: alice@example.com".to_string(),
                "Snippet: snippet".to_string(),
                String::new(),
                "replacement".to_string(),
            ]
        );
        assert_eq!(app.detail_line_count(), 5);
        assert_eq!(app.detail_cursor_line_column(), (0, 0));
    }

    #[test]
    fn test_line_cache_preserves_empty_and_trailing_unicode_lines() {
        let text = "é\n\nx\n";
        let cache = LineCache::from_text(text);

        assert_eq!(cache.lines(text), vec!["é", "", "x", ""]);
        assert_eq!(cache.line_count(), 4);
        assert_eq!(cache.char_len(), 5);
        assert_eq!(cache.line_start(1), 2);
        assert_eq!(cache.line_end(1), 2);
        assert_eq!(cache.line_start(3), 5);
        assert_eq!(cache.line_end(3), 5);
        assert_eq!(cache.line_for_cursor(1), 0);
        assert_eq!(cache.line_for_cursor(2), 1);
        assert_eq!(cache.line_for_cursor(3), 2);
        assert_eq!(cache.line_for_cursor(5), 3);
    }

    fn preview_focused_app(body: &str) -> AppState {
        // Body becomes preview.text. The header line is the preview
        // `message` field, then a blank separator, then the body lines.
        let attachment = attachment("notes.txt");
        let attachment_id = attachment.id;
        let message_id = attachment.message_id;
        let mut app = AppState::default();
        app.apply_detail(Some(detail(message_id, "body")));
        app.apply_attachments(vec![attachment]);
        app.active = ActivePane::Attachments;
        app.apply_attachment_preview(AttachmentPreviewItem {
            attachment_id,
            text: Some(body.into()),
            message: "Inline preview".into(),
            truncated: false,
            preview_bytes: body.len(),
        });
        assert!(app.focus_preview());
        app
    }

    #[derive(Default)]
    struct CapturingClipboard {
        last: Option<String>,
        fail: Option<String>,
    }

    impl CapturingClipboard {
        fn ok() -> Self {
            Self::default()
        }

        fn failing(reason: &str) -> Self {
            Self {
                fail: Some(reason.into()),
                ..Self::default()
            }
        }
    }

    impl crate::tui::ClipboardSink for CapturingClipboard {
        fn copy(&mut self, text: &str) -> Result<(), String> {
            if let Some(reason) = self.fail.clone() {
                return Err(reason);
            }
            self.last = Some(text.to_string());
            Ok(())
        }
    }

    #[test]
    fn test_preview_text_renders_message_then_body_then_truncated_marker() {
        let mut app = preview_focused_app("alpha\nbeta");
        // Default preview was non-truncated; flip it now.
        if let Some(p) = app.attachment_preview.as_mut() {
            p.truncated = true;
        }
        let text = app.preview_text().unwrap();
        assert!(text.starts_with("Inline preview\n\n"));
        assert!(text.ends_with("\n\n[truncated]"));
        assert!(text.contains("alpha"));
        assert!(text.contains("beta"));
    }

    #[test]
    fn test_scroll_preview_clamps_to_max_offset() {
        // 10 body lines + 2 header rows ("Inline preview", "")
        // gives 12 total preview lines.
        let body = (1..=10)
            .map(|n| format!("body {n:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = preview_focused_app(&body);
        assert_eq!(app.preview_line_count(), 12);

        // Scroll past the end: max for height 6 is 12 - 6 = 6.
        assert!(app.scroll_preview(20, 6));
        assert_eq!(app.preview_scroll, 6);
        // Already at max — no change.
        assert!(!app.scroll_preview(1, 6));
        assert_eq!(app.preview_scroll, 6);
        // Scroll up by 4 -> 2.
        assert!(app.scroll_preview(-4, 6));
        assert_eq!(app.preview_scroll, 2);
        // Scroll up past 0 clamps.
        assert!(app.scroll_preview(-99, 6));
        assert_eq!(app.preview_scroll, 0);
    }

    #[test]
    fn test_scroll_preview_top_and_bottom_helpers() {
        let body = (1..=10)
            .map(|n| format!("body {n:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = preview_focused_app(&body);
        assert!(app.scroll_preview_to_bottom(6));
        assert_eq!(app.preview_scroll, 6);
        assert!(app.scroll_preview_to_top());
        assert_eq!(app.preview_scroll, 0);
    }

    #[test]
    fn test_scroll_preview_requires_focus() {
        let body = "alpha\nbeta\ngamma\ndelta\nepsilon";
        let mut app = preview_focused_app(body);
        // Defocus: scroll calls become no-ops.
        assert!(app.defocus_preview());
        assert!(!app.scroll_preview(2, 6));
        assert_eq!(app.preview_scroll, 0);
        assert!(!app.scroll_preview_to_bottom(6));
    }

    #[test]
    fn test_toggle_preview_selection_anchors_at_viewport_top_and_extends_with_movement() {
        let body = (1..=10)
            .map(|n| format!("body {n:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = preview_focused_app(&body);
        // Scroll so anchor lands on a non-zero line. The fully-rendered
        // preview is 12 lines; viewport 6 caps max scroll at 6.
        assert!(app.scroll_preview(3, 6));
        assert_eq!(app.preview_scroll, 3);
        assert!(app.toggle_preview_selection());
        assert_eq!(app.preview_selected_line_range(), Some(3..=3));

        // j extends the focus end.
        assert!(app.move_preview_line(2));
        assert_eq!(app.preview_selected_line_range(), Some(3..=5));

        // k moves the focus end back below the anchor; the range
        // sorts so start <= end.
        assert!(app.move_preview_line(-3));
        assert_eq!(app.preview_selected_line_range(), Some(2..=3));

        // Out-of-bounds movement clamps.
        assert!(app.move_preview_line(99));
        assert_eq!(
            app.preview_selected_line_range(),
            Some(3..=app.preview_line_count() - 1)
        );

        // Toggle off.
        assert!(app.toggle_preview_selection());
        assert_eq!(app.preview_selected_line_range(), None);
    }

    #[test]
    fn test_clear_preview_selection_via_escape_does_not_yank() {
        let mut app = preview_focused_app("alpha\nbeta\ngamma");
        assert!(app.toggle_preview_selection());
        assert!(app.move_preview_line(1));
        assert!(app.preview_selection.is_some());

        assert!(app.clear_preview_selection());
        assert_eq!(app.preview_selection, None);
        assert_eq!(app.preview_yank_text(), None);
    }

    #[test]
    fn test_preview_yank_builds_clipboard_string_from_selected_lines() {
        let body = "alpha\nbeta\ngamma\ndelta";
        let mut app = preview_focused_app(body);
        // Lines: 0 "Inline preview", 1 "", 2 "alpha", 3 "beta",
        // 4 "gamma", 5 "delta".
        app.preview_scroll = 2;
        assert!(app.toggle_preview_selection());
        assert!(app.move_preview_line(2));
        let yanked = app.preview_yank_text().expect("yank");
        assert_eq!(yanked, "alpha\nbeta\ngamma");
    }

    #[test]
    fn test_handle_preview_key_yank_writes_to_clipboard_sink() {
        use crate::tui::handle_preview_focus_key;
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let body = "first\nsecond\nthird";
        let mut app = preview_focused_app(body);
        app.preview_scroll = 2;
        assert!(handle_preview_focus_key(
            KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE),
            &mut app,
            &mut CapturingClipboard::ok(),
        ));
        assert!(handle_preview_focus_key(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
            &mut app,
            &mut CapturingClipboard::ok(),
        ));
        let mut clipboard = CapturingClipboard::ok();
        assert!(handle_preview_focus_key(
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
            &mut app,
            &mut clipboard,
        ));
        assert_eq!(clipboard.last.as_deref(), Some("first\nsecond"));
        assert!(app.status.contains("2 line"));
    }

    #[test]
    fn test_handle_preview_key_yank_failure_sets_error() {
        use crate::tui::handle_preview_focus_key;
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut app = preview_focused_app("only-line");
        assert!(handle_preview_focus_key(
            KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE),
            &mut app,
            &mut CapturingClipboard::ok(),
        ));
        let mut clipboard = CapturingClipboard::failing("no display");
        assert!(handle_preview_focus_key(
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
            &mut app,
            &mut clipboard,
        ));
        assert_eq!(app.error.as_deref(), Some("Clipboard error: no display"));
    }

    #[test]
    fn test_handle_preview_key_yank_with_no_selection_sets_status_and_skips_clipboard() {
        use crate::tui::handle_preview_focus_key;
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut app = preview_focused_app("alpha\nbeta\ngamma");
        assert_eq!(app.preview_selection, None);

        let mut clipboard = CapturingClipboard::ok();
        assert!(handle_preview_focus_key(
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
            &mut app,
            &mut clipboard,
        ));
        assert_eq!(app.status, "No preview selection");
        assert_eq!(clipboard.last, None);
    }

    #[test]
    fn test_scroll_preview_half_page_advances_by_half_viewport_from_top() {
        // 12 preview lines (2 header + 10 body); viewport 6, half = 3.
        let body = (1..=10)
            .map(|n| format!("body {n:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = preview_focused_app(&body);
        assert_eq!(app.preview_scroll, 0);
        assert!(app.scroll_preview(3, 6));
        assert_eq!(app.preview_scroll, 3);
    }

    #[test]
    fn test_scroll_preview_half_page_up_from_zero_clamps_at_zero() {
        let body = (1..=10)
            .map(|n| format!("body {n:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = preview_focused_app(&body);
        assert!(!app.scroll_preview(-3, 6));
        assert_eq!(app.preview_scroll, 0);
    }

    #[test]
    fn test_scroll_preview_half_page_down_clamps_at_preview_max() {
        let body = (1..=10)
            .map(|n| format!("body {n:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = preview_focused_app(&body);
        // Move close to the bottom: max for 12 lines @ viewport 6 is 6.
        app.preview_scroll = 5;
        assert!(app.scroll_preview(3, 6));
        assert_eq!(app.preview_scroll, app.preview_max_scroll(6));
        // Already at max — no-op.
        assert!(!app.scroll_preview(3, 6));
        assert_eq!(app.preview_scroll, app.preview_max_scroll(6));
    }

    #[test]
    fn test_handle_preview_key_ctrl_d_and_ctrl_u_route_to_half_page_scroll() {
        use crate::tui::handle_preview_focus_key;
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        // 12 preview lines (2 header + 10 body); viewport in handler is 6,
        // so half-page = 3.
        let body = (1..=10)
            .map(|n| format!("body {n:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = preview_focused_app(&body);
        assert_eq!(app.preview_scroll, 0);

        // Ctrl-D scrolls down by half page.
        assert!(handle_preview_focus_key(
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
            &mut app,
            &mut CapturingClipboard::ok(),
        ));
        assert_eq!(app.preview_scroll, 3);

        // Ctrl-U scrolls up by half page.
        assert!(handle_preview_focus_key(
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
            &mut app,
            &mut CapturingClipboard::ok(),
        ));
        assert_eq!(app.preview_scroll, 0);
    }

    #[test]
    fn test_handle_preview_key_g_and_capital_g_jump_to_top_and_bottom() {
        use crate::tui::handle_preview_focus_key;
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let body = (1..=12)
            .map(|n| format!("body {n:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = preview_focused_app(&body);

        assert!(handle_preview_focus_key(
            KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT),
            &mut app,
            &mut CapturingClipboard::ok(),
        ));
        assert_eq!(app.preview_scroll, app.preview_max_scroll(6));

        assert!(handle_preview_focus_key(
            KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
            &mut app,
            &mut CapturingClipboard::ok(),
        ));
        assert_eq!(app.preview_scroll, 0);
    }

    #[test]
    fn test_handle_preview_key_escape_clears_selection_then_focus() {
        use crate::tui::handle_preview_focus_key;
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut app = preview_focused_app("alpha\nbeta\ngamma");
        assert!(app.toggle_preview_selection());

        assert!(handle_preview_focus_key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &mut app,
            &mut CapturingClipboard::ok(),
        ));
        // First Esc clears selection, focus remains.
        assert!(app.preview_focused);
        assert_eq!(app.preview_selection, None);

        assert!(handle_preview_focus_key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &mut app,
            &mut CapturingClipboard::ok(),
        ));
        // Second Esc defocuses.
        assert!(!app.preview_focused);
    }

    #[test]
    fn test_apply_attachment_preview_resets_navigation_when_attachment_changes() {
        let mut app = preview_focused_app("alpha\nbeta\ngamma");
        app.preview_scroll = 2;
        assert!(app.toggle_preview_selection());
        assert!(app.preview_focused);

        let other_id = AttachmentId::new();
        app.apply_attachment_preview(AttachmentPreviewItem {
            attachment_id: other_id,
            text: Some("brand new".into()),
            message: "Inline preview".into(),
            truncated: false,
            preview_bytes: 9,
        });

        assert_eq!(app.preview_scroll, 0);
        assert_eq!(app.preview_selection, None);
        assert!(!app.preview_focused);
    }

    #[test]
    fn test_composer_field_editing_and_payload_construction() {
        let account_id = AccountId::new();
        let mut app = AppState::default();
        app.enter_composer(account_id);

        assert_eq!(app.mode, InputMode::Compose);
        assert_eq!(app.composer.as_ref().unwrap().focused, ComposeField::To);
        for ch in "bob@example.com, alice@example.com".chars() {
            assert!(app.push_composer_char(ch));
        }
        app.next_composer_field();
        assert_eq!(app.composer.as_ref().unwrap().focused, ComposeField::Cc);
        app.next_composer_field();
        app.next_composer_field();
        for ch in "Status".chars() {
            assert!(app.push_composer_char(ch));
        }
        app.next_composer_field();
        for ch in "Line one".chars() {
            assert!(app.push_composer_char(ch));
        }
        assert!(app.composer_enter());
        for ch in "Line two".chars() {
            assert!(app.push_composer_char(ch));
        }

        let draft = app.composer_draft().unwrap();
        assert_eq!(draft.account_id, account_id);
        assert_eq!(
            draft.to_addrs,
            vec![
                "bob@example.com".to_string(),
                "alice@example.com".to_string()
            ]
        );
        assert_eq!(draft.subject.as_deref(), Some("Status"));
        assert_eq!(draft.text_body.as_deref(), Some("Line one\nLine two"));
        assert!(app.composer.as_ref().unwrap().dirty);
    }

    #[test]
    fn test_composer_inserts_at_cursor_in_header_and_body() {
        let mut app = AppState::default();
        app.enter_composer(AccountId::new());

        for ch in "ac".chars() {
            assert!(app.push_composer_char(ch));
        }
        assert!(app.move_composer_cursor_left());
        assert!(app.push_composer_char('b'));
        let composer = app.composer.as_ref().unwrap();
        assert_eq!(composer.to, "abc");
        assert_eq!(composer.to_cursor, 2);

        app.composer.as_mut().unwrap().focused = ComposeField::Body;
        for ch in "wy".chars() {
            assert!(app.push_composer_char(ch));
        }
        assert!(app.move_composer_cursor_left());
        assert!(app.push_composer_char('x'));
        let composer = app.composer.as_ref().unwrap();
        assert_eq!(composer.body, "wxy");
        assert_eq!(composer.body_cursor, 2);
    }

    #[test]
    fn test_composer_cursor_editing_keys_handle_boundaries() {
        let mut app = AppState::default();
        app.enter_composer(AccountId::new());
        for ch in "abcd".chars() {
            assert!(app.push_composer_char(ch));
        }

        assert!(app.move_composer_cursor_left());
        assert!(app.move_composer_cursor_left());
        assert!(app.backspace_composer());
        assert!(app.delete_composer());
        assert_eq!(app.composer.as_ref().unwrap().to, "ad");
        assert_eq!(app.composer.as_ref().unwrap().to_cursor, 1);

        assert!(app.composer_home());
        assert!(!app.backspace_composer());
        assert_eq!(app.composer.as_ref().unwrap().to_cursor, 0);

        assert!(app.composer_end());
        assert!(!app.delete_composer());
        assert_eq!(app.composer.as_ref().unwrap().to_cursor, 2);
    }

    #[test]
    fn test_composer_body_line_navigation_preserves_column() {
        let mut app = AppState::default();
        app.enter_composer(AccountId::new());
        let composer = app.composer.as_mut().unwrap();
        composer.focused = ComposeField::Body;
        composer.body = "abcde\nxy\nwxyz".into();
        composer.refresh_body_line_cache();
        composer.body_cursor = 5;

        assert!(app.move_composer_body_line(1, 10));
        let composer = app.composer.as_ref().unwrap();
        assert_eq!(composer.body_cursor, 8);
        assert_eq!(composer.body_cursor_line_column(), (1, 2));

        assert!(app.move_composer_body_line(1, 10));
        let composer = app.composer.as_ref().unwrap();
        assert_eq!(composer.body_cursor, 13);
        assert_eq!(composer.body_cursor_line_column(), (2, 4));

        assert!(app.move_composer_body_line(-1, 10));
        let composer = app.composer.as_ref().unwrap();
        assert_eq!(composer.body_cursor, 8);
        assert_eq!(composer.body_cursor_line_column(), (1, 2));
    }

    #[test]
    fn test_composer_body_line_cache_updates_after_body_edits() {
        let mut app = AppState::default();
        app.enter_composer(AccountId::new());
        app.composer.as_mut().unwrap().focused = ComposeField::Body;

        for ch in "ab".chars() {
            assert!(app.push_composer_char(ch));
        }
        assert!(app.composer_enter());
        for ch in "café".chars() {
            assert!(app.push_composer_char(ch));
        }

        let composer = app.composer.as_ref().unwrap();
        assert_eq!(composer.body_lines(), vec!["ab", "café"]);
        assert_eq!(composer.body_line_start(1), 3);
        assert_eq!(composer.body_line_end(1), 7);
        assert_eq!(composer.body_cursor_line_column(), (1, 4));

        assert!(app.backspace_composer());
        let composer = app.composer.as_ref().unwrap();
        assert_eq!(composer.body_lines(), vec!["ab", "caf"]);
        assert_eq!(composer.body_line_end(1), 6);
        assert_eq!(composer.body_cursor_line_column(), (1, 3));

        assert!(app.composer_home());
        assert!(app.delete_composer());
        let composer = app.composer.as_ref().unwrap();
        assert_eq!(composer.body_lines(), vec!["ab", "af"]);
        assert_eq!(composer.body_line_start(1), 3);
        assert_eq!(composer.body_cursor_line_column(), (1, 0));
    }

    #[test]
    fn test_composer_body_scroll_keeps_cursor_visible() {
        let mut app = AppState::default();
        app.enter_composer(AccountId::new());
        let composer = app.composer.as_mut().unwrap();
        composer.focused = ComposeField::Body;
        composer.body = "one\ntwo\nthree\nfour\nfive\nsix".into();
        composer.refresh_body_line_cache();

        for _ in 0..4 {
            assert!(app.move_composer_body_line(1, 2));
        }

        let composer = app.composer.as_ref().unwrap();
        assert_eq!(composer.body_cursor_line_column().0, 4);
        assert_eq!(composer.body_scroll, 3);

        assert!(app.move_composer_body_line(-1, 2));
        assert_eq!(app.composer.as_ref().unwrap().body_scroll, 3);
        assert!(app.move_composer_body_line(-1, 2));
        assert_eq!(app.composer.as_ref().unwrap().body_scroll, 2);
    }

    #[test]
    fn test_composer_visual_line_selection_toggles_updates_and_clears() {
        let mut app = AppState::default();
        app.enter_composer(AccountId::new());
        let composer = app.composer.as_mut().unwrap();
        composer.focused = ComposeField::Body;
        composer.body = "one\ntwo\nthree".into();
        composer.refresh_body_line_cache();

        assert!(app.toggle_composer_body_line_selection());
        assert_eq!(
            app.composer.as_ref().unwrap().body_selected_line_range(),
            Some(0..=0)
        );

        assert!(app.move_composer_body_line(1, 5));
        assert!(app.move_composer_body_line(1, 5));
        assert_eq!(
            app.composer.as_ref().unwrap().body_selected_line_range(),
            Some(0..=2)
        );

        assert!(app.clear_composer_body_selection());
        assert_eq!(
            app.composer.as_ref().unwrap().body_selected_line_range(),
            None
        );
    }

    #[test]
    fn test_composer_draft_payload_preserves_edited_multiline_body() {
        let account_id = AccountId::new();
        let mut app = AppState::default();
        app.enter_composer(account_id);
        app.composer.as_mut().unwrap().focused = ComposeField::Body;

        for ch in "Line 1".chars() {
            assert!(app.push_composer_char(ch));
        }
        assert!(app.composer_enter());
        for ch in "Line 3".chars() {
            assert!(app.push_composer_char(ch));
        }
        assert!(app.move_composer_body_line(-1, 10));
        app.composer_end();
        assert!(app.composer_enter());
        for ch in "Line 2".chars() {
            assert!(app.push_composer_char(ch));
        }

        let draft = app.composer_draft().unwrap();
        assert_eq!(draft.account_id, account_id);
        assert_eq!(draft.text_body.as_deref(), Some("Line 1\nLine 2\nLine 3"));
    }

    #[test]
    fn test_composer_save_state_and_discard_confirmation() {
        let account_id = AccountId::new();
        let draft_id = DraftId::new();
        let mut app = AppState::default();
        app.enter_composer(account_id);
        assert!(app.push_composer_char('a'));

        assert!(app.composer_needs_discard_confirmation());
        app.mark_composer_saved(draft_id);
        assert_eq!(app.composer.as_ref().unwrap().draft_id, Some(draft_id));
        assert!(!app.composer_needs_discard_confirmation());

        app.previous_composer_field();
        assert!(app.push_composer_char('B'));
        assert!(app.composer_needs_discard_confirmation());
        app.begin_discard_composer_confirmation();
        assert_eq!(app.mode, InputMode::ConfirmDiscard);
        app.cancel_discard_composer_confirmation();
        assert_eq!(app.mode, InputMode::Compose);
        assert!(app.composer.is_some());
        app.begin_discard_composer_confirmation();
        app.discard_composer();
        assert_eq!(app.mode, InputMode::Normal);
        assert!(app.composer.is_none());
    }

    #[test]
    fn test_toast_deque_caps_at_three_dropping_oldest() {
        let mut app = AppState::default();
        let now = Instant::now();
        let first = app.push_toast(ToastKind::Info, "one", now);
        let _second = app.push_toast(ToastKind::Info, "two", now);
        let _third = app.push_toast(ToastKind::Info, "three", now);
        let fourth = app.push_toast(ToastKind::Info, "four", now);

        assert_eq!(app.toasts.len(), MAX_TOASTS);
        assert!(app.toasts.iter().all(|t| t.id != first));
        assert!(app.toasts.iter().any(|t| t.id == fourth));
        let texts: Vec<_> = app.toasts.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(texts, vec!["two", "three", "four"]);
    }

    #[test]
    fn test_toast_tick_drops_only_expired() {
        let mut app = AppState::default();
        let start = Instant::now();
        app.push_toast(ToastKind::Info, "info", start);
        app.push_toast(ToastKind::Error, "error", start);

        // Just before info expiry: both still visible.
        app.tick_toasts(start + TOAST_TTL_INFO - Duration::from_millis(1));
        assert_eq!(app.toasts.len(), 2);

        // Just after info expiry: info gone, error still around.
        app.tick_toasts(start + TOAST_TTL_INFO + Duration::from_millis(1));
        assert_eq!(app.toasts.len(), 1);
        assert_eq!(app.toasts[0].kind, ToastKind::Error);

        // Just before error expiry: still there.
        app.tick_toasts(start + TOAST_TTL_ERROR - Duration::from_millis(1));
        assert_eq!(app.toasts.len(), 1);

        // Just after: gone.
        app.tick_toasts(start + TOAST_TTL_ERROR + Duration::from_millis(1));
        assert!(app.toasts.is_empty());
    }

    #[test]
    fn test_account_synced_toast_coalesces_within_5_seconds() {
        let mut app = AppState::default();
        let acct = account("Work");
        let acct_id = acct.id;
        app.apply_accounts(vec![acct]);
        let start = Instant::now();

        app.push_account_synced_toast(acct_id, start);
        assert_eq!(app.toasts.len(), 1);
        let original_expiry = app.toasts.back().unwrap().expires_at;

        let later = start + Duration::from_secs(2);
        app.push_account_synced_toast(acct_id, later);

        assert_eq!(app.toasts.len(), 1, "second toast should have coalesced");
        let new_expiry = app.toasts.back().unwrap().expires_at;
        assert!(new_expiry > original_expiry);
        assert_eq!(new_expiry, later + TOAST_TTL_INFO);
    }

    #[test]
    fn test_account_synced_toast_does_not_coalesce_after_window() {
        let mut app = AppState::default();
        let acct = account("Work");
        let acct_id = acct.id;
        app.apply_accounts(vec![acct]);
        let start = Instant::now();

        app.push_account_synced_toast(acct_id, start);
        // Advance time past the prior toast's expiry so it has aged out
        // of the coalesce window.
        let later = start + COALESCE_ACCOUNT_SYNCED + Duration::from_millis(1);
        app.push_account_synced_toast(acct_id, later);

        assert_eq!(app.toasts.len(), 2);
    }

    #[test]
    fn test_sync_state_error_coalesces_identical_text_within_10s() {
        let mut app = AppState::default();
        let acct = account("Work");
        let acct_id = acct.id;
        app.apply_accounts(vec![acct]);
        let start = Instant::now();

        app.apply_sync_state(
            acct_id,
            SyncStateUi::Error,
            Some("login refused".into()),
            start,
        );
        assert_eq!(app.toasts.len(), 1);
        let first_expiry = app.toasts.back().unwrap().expires_at;

        // Same text within 10s → coalesce.
        app.apply_sync_state(
            acct_id,
            SyncStateUi::Error,
            Some("login refused".into()),
            start + Duration::from_secs(4),
        );
        assert_eq!(app.toasts.len(), 1);
        assert!(app.toasts.back().unwrap().expires_at > first_expiry);

        // Beyond the 10s window → second toast pushed.
        app.apply_sync_state(
            acct_id,
            SyncStateUi::Error,
            Some("login refused".into()),
            start + Duration::from_secs(20),
        );
        assert_eq!(app.toasts.len(), 2);
    }

    #[test]
    fn test_dismiss_newest_toast_pops_back_only() {
        let mut app = AppState::default();
        let now = Instant::now();
        app.push_toast(ToastKind::Info, "one", now);
        app.push_toast(ToastKind::Info, "two", now);

        assert!(app.dismiss_newest_toast());
        assert_eq!(app.toasts.len(), 1);
        assert_eq!(app.toasts.front().unwrap().text, "one");

        assert!(app.dismiss_newest_toast());
        assert!(!app.dismiss_newest_toast());
    }

    #[test]
    fn test_clear_toasts_drops_everything() {
        let mut app = AppState::default();
        let now = Instant::now();
        app.push_toast(ToastKind::Info, "one", now);
        app.push_toast(ToastKind::Error, "two", now);

        assert!(app.clear_toasts());
        assert!(app.toasts.is_empty());
        assert!(!app.clear_toasts());
    }

    #[test]
    fn test_apply_sync_state_updates_account_states_map() {
        let mut app = AppState::default();
        let acct = account("Work");
        let acct_id = acct.id;
        app.apply_accounts(vec![acct]);
        let now = Instant::now();

        app.apply_sync_state(acct_id, SyncStateUi::Polling, None, now);
        assert_eq!(
            app.account_states.get(&acct_id).map(|s| s.state),
            Some(SyncStateUi::Polling)
        );
        assert!(app.account_states[&acct_id].last_error.is_none());

        app.apply_sync_state(acct_id, SyncStateUi::Error, Some("boom".into()), now);
        assert_eq!(app.account_states[&acct_id].state, SyncStateUi::Error);
        assert_eq!(
            app.account_states[&acct_id].last_error.as_deref(),
            Some("boom")
        );

        // Recovering clears the error text but keeps the entry.
        app.apply_sync_state(acct_id, SyncStateUi::Idle, None, now);
        assert_eq!(app.account_states[&acct_id].state, SyncStateUi::Idle);
        assert!(app.account_states[&acct_id].last_error.is_none());
    }

    #[test]
    fn test_apply_sync_state_error_without_last_error_falls_back_to_default() {
        let mut app = AppState::default();
        let acct = account("Work");
        let acct_id = acct.id;
        app.apply_accounts(vec![acct]);
        let now = Instant::now();

        app.apply_sync_state(acct_id, SyncStateUi::Error, None, now);

        let status = &app.account_states[&acct_id];
        assert_eq!(status.last_error.as_deref(), Some("sync error"));
        assert_eq!(app.toasts.len(), 1);
        assert!(app.toasts[0].text.contains("sync error"));
    }

    #[test]
    fn test_account_states_stored_for_selected_error_60_char_truncation() {
        // The 60-char truncation is applied at render time. This test
        // confirms the raw error text is preserved on the model so
        // render_status can do its own truncation deterministically.
        let mut app = AppState::default();
        let acct = account("Work");
        let acct_id = acct.id;
        app.apply_accounts(vec![acct]);
        let long_error = "a".repeat(120);

        app.apply_sync_state(
            acct_id,
            SyncStateUi::Error,
            Some(long_error.clone()),
            Instant::now(),
        );

        let stored = app.account_states[&acct_id].last_error.as_deref().unwrap();
        assert_eq!(stored.len(), 120);
        assert!(MAX_SELECTED_ERROR_CHARS < stored.len());
        // The toast also carries the full error for the user.
        assert!(app.toasts.back().unwrap().text.contains(&long_error));
    }

    // ---------------- Compose attachments (Slice 6) ----------------

    fn temp_attach_file(name: &str, bytes: &[u8]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        (dir, path)
    }

    #[test]
    fn test_compose_attach_path_mode_collects_chars_and_cancels_on_esc() {
        let mut app = AppState::default();
        app.enter_composer(AccountId::new());

        assert!(app.begin_compose_attach());
        assert_eq!(app.mode, InputMode::ComposeAttachPath);
        for ch in "/tmp/x.txt".chars() {
            assert!(app.push_compose_attach_char(ch));
        }
        assert_eq!(app.compose_attach_input(), Some("/tmp/x.txt"));
        assert!(app.backspace_compose_attach());
        assert_eq!(app.compose_attach_input(), Some("/tmp/x.tx"));

        app.cancel_compose_attach();
        assert_eq!(app.mode, InputMode::Compose);
        assert_eq!(app.compose_attach_input(), Some(""));
    }

    #[test]
    fn test_compose_attach_path_mode_rejects_control_chars_and_caps_length() {
        let mut app = AppState::default();
        app.enter_composer(AccountId::new());
        app.begin_compose_attach();

        assert!(!app.push_compose_attach_char('\n'));
        assert!(!app.push_compose_attach_char('\0'));
        assert_eq!(app.compose_attach_input(), Some(""));

        // Fill the input to its cap.
        for _ in 0..MAX_COMPOSE_PATH_CHARS {
            assert!(app.push_compose_attach_char('a'));
        }
        assert!(!app.push_compose_attach_char('a'));
    }

    #[tokio::test]
    async fn test_confirm_compose_attach_adds_attachment_with_filename_size_and_type() {
        let mut app = AppState::default();
        app.enter_composer(AccountId::new());
        let (_dir, path) = temp_attach_file("notes.txt", b"hello world");
        app.begin_compose_attach();
        for ch in path.display().to_string().chars() {
            app.push_compose_attach_char(ch);
        }

        let name = app.confirm_compose_attach().await.expect("confirm ok");
        assert_eq!(name, "notes.txt");
        assert_eq!(app.mode, InputMode::Compose);
        let composer = app.composer.as_ref().unwrap();
        assert_eq!(composer.attachments.len(), 1);
        let attached = &composer.attachments[0];
        assert_eq!(attached.filename, "notes.txt");
        assert_eq!(attached.size_bytes, b"hello world".len() as u64);
        assert_eq!(attached.content_type, "text/plain");
        assert_eq!(composer.selected_attachment, 0);
        assert!(composer.dirty);
        assert!(composer.attach_input.is_empty());
    }

    #[tokio::test]
    async fn test_confirm_compose_attach_unknown_extension_falls_back_to_octet_stream() {
        let mut app = AppState::default();
        app.enter_composer(AccountId::new());
        let (_dir, path) = temp_attach_file("blob.weird", &[1, 2, 3]);
        app.begin_compose_attach();
        for ch in path.display().to_string().chars() {
            app.push_compose_attach_char(ch);
        }

        app.confirm_compose_attach().await.unwrap();
        assert_eq!(
            app.composer.as_ref().unwrap().attachments[0].content_type,
            "application/octet-stream"
        );
    }

    #[tokio::test]
    async fn test_confirm_compose_attach_missing_file_yields_not_found_toast_text() {
        let mut app = AppState::default();
        app.enter_composer(AccountId::new());
        app.begin_compose_attach();
        for ch in "/nonexistent/path/does-not-exist.txt".chars() {
            app.push_compose_attach_char(ch);
        }

        let err = app.confirm_compose_attach().await.unwrap_err();
        assert!(matches!(err, AttachError::NotFound(_)));
        assert!(err.toast_text().starts_with("File not found:"));
    }

    #[tokio::test]
    async fn test_confirm_compose_attach_directory_rejected_as_not_a_file() {
        let mut app = AppState::default();
        app.enter_composer(AccountId::new());
        let dir = tempfile::tempdir().unwrap();
        app.begin_compose_attach();
        for ch in dir.path().display().to_string().chars() {
            app.push_compose_attach_char(ch);
        }

        let err = app.confirm_compose_attach().await.unwrap_err();
        assert!(matches!(err, AttachError::NotAFile(_)));
    }

    #[tokio::test]
    async fn test_confirm_compose_attach_over_25mib_rejected_with_toast_text() {
        let mut app = AppState::default();
        app.enter_composer(AccountId::new());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.bin");
        // One byte over the cap.
        let size = (MAX_COMPOSE_ATTACHMENT_BYTES + 1) as usize;
        std::fs::write(&path, vec![0u8; size]).unwrap();
        app.begin_compose_attach();
        for ch in path.display().to_string().chars() {
            app.push_compose_attach_char(ch);
        }

        let err = app.confirm_compose_attach().await.unwrap_err();
        assert!(matches!(err, AttachError::TooLarge { .. }));
        assert!(err.toast_text().contains("Attachment too large"));
        // Composer attachments untouched after rejection.
        assert!(app.composer.as_ref().unwrap().attachments.is_empty());
    }

    #[tokio::test]
    async fn test_confirm_compose_attach_aggregate_over_limit_is_rejected() {
        let mut app = AppState::default();
        app.enter_composer(AccountId::new());
        // Pre-seed a fake attachment that already eats most of the cap.
        let composer = app.composer.as_mut().unwrap();
        composer.attachments.push(ComposerAttachment {
            path: PathBuf::from("/tmp/seed.bin"),
            filename: "seed.bin".into(),
            size_bytes: MAX_COMPOSE_ATTACHMENT_BYTES - 10,
            content_type: "application/octet-stream".into(),
        });

        // Add a small real file whose size pushes the aggregate past the cap.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("more.bin");
        std::fs::write(&path, vec![0u8; 100]).unwrap();
        app.begin_compose_attach();
        for ch in path.display().to_string().chars() {
            app.push_compose_attach_char(ch);
        }

        let err = app.confirm_compose_attach().await.unwrap_err();
        assert!(matches!(err, AttachError::AggregateTooLarge { .. }));
        assert!(err.toast_text().contains("Aggregate over limit"));
        assert_eq!(app.composer.as_ref().unwrap().attachments.len(), 1);
    }

    #[test]
    fn test_remove_selected_compose_attachment_clamps_index_at_end() {
        let mut app = AppState::default();
        app.enter_composer(AccountId::new());
        let composer = app.composer.as_mut().unwrap();
        for name in ["a.txt", "b.txt", "c.txt"] {
            composer.attachments.push(ComposerAttachment {
                path: PathBuf::from(format!("/tmp/{name}")),
                filename: name.to_string(),
                size_bytes: 1,
                content_type: "text/plain".into(),
            });
        }
        composer.selected_attachment = 2;

        let removed = app.remove_selected_compose_attachment().unwrap();
        assert_eq!(removed, "c.txt");
        let composer = app.composer.as_ref().unwrap();
        assert_eq!(composer.attachments.len(), 2);
        // Index clamped to last surviving entry.
        assert_eq!(composer.selected_attachment, 1);
        assert!(composer.dirty);

        // Remove from middle keeps the new last in range.
        app.remove_selected_compose_attachment();
        let composer = app.composer.as_ref().unwrap();
        assert_eq!(composer.attachments.len(), 1);
        assert_eq!(composer.selected_attachment, 0);

        app.remove_selected_compose_attachment();
        assert!(app.composer.as_ref().unwrap().attachments.is_empty());
        assert_eq!(app.composer.as_ref().unwrap().selected_attachment, 0);

        // Empty list: removal is a no-op.
        assert!(app.remove_selected_compose_attachment().is_none());
    }

    #[test]
    fn test_move_compose_attachment_selection_navigates_within_bounds() {
        let mut app = AppState::default();
        app.enter_composer(AccountId::new());
        let composer = app.composer.as_mut().unwrap();
        for name in ["a", "b", "c"] {
            composer.attachments.push(ComposerAttachment {
                path: PathBuf::from(format!("/tmp/{name}")),
                filename: name.into(),
                size_bytes: 1,
                content_type: "text/plain".into(),
            });
        }

        assert!(app.move_compose_attachment_selection(1));
        assert_eq!(app.composer.as_ref().unwrap().selected_attachment, 1);
        assert!(app.move_compose_attachment_selection(1));
        assert_eq!(app.composer.as_ref().unwrap().selected_attachment, 2);
        // At end: no further movement.
        assert!(!app.move_compose_attachment_selection(1));
        assert!(app.move_compose_attachment_selection(-2));
        assert_eq!(app.composer.as_ref().unwrap().selected_attachment, 0);
        assert!(!app.move_compose_attachment_selection(-1));
    }

    #[test]
    fn test_composer_state_survives_attach_path_mode_toggle() {
        let mut app = AppState::default();
        app.enter_composer(AccountId::new());
        for ch in "to@x.com".chars() {
            assert!(app.push_composer_char(ch));
        }
        app.next_composer_field();
        app.next_composer_field();
        app.next_composer_field();
        for ch in "Subject text".chars() {
            assert!(app.push_composer_char(ch));
        }

        app.begin_compose_attach();
        for ch in "/tmp/whatever".chars() {
            app.push_compose_attach_char(ch);
        }
        app.cancel_compose_attach();

        let composer = app.composer.as_ref().unwrap();
        assert_eq!(composer.to, "to@x.com");
        assert_eq!(composer.subject, "Subject text");
        assert!(composer.attach_input.is_empty());
    }

    #[tokio::test]
    async fn test_composer_draft_payload_includes_attachments() {
        let mut app = AppState::default();
        app.enter_composer(AccountId::new());
        let (_dir, path) = temp_attach_file("doc.txt", b"abc");
        app.begin_compose_attach();
        for ch in path.display().to_string().chars() {
            app.push_compose_attach_char(ch);
        }
        app.confirm_compose_attach().await.unwrap();

        let draft = app.composer_draft().unwrap();
        assert_eq!(draft.attachments.len(), 1);
        assert_eq!(draft.attachments[0].filename, "doc.txt");
        assert_eq!(draft.attachments[0].size_bytes, 3);
    }

    #[test]
    fn test_attach_error_toast_text_includes_human_readable_size() {
        let err = AttachError::TooLarge {
            size: 26 * 1024 * 1024,
        };
        let text = err.toast_text();
        assert!(text.contains("MiB"));
        assert!(text.contains("Attachment too large"));
    }

    #[test]
    fn test_human_size_formats_bytes_kib_mib_gib() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1024), "1.0 KiB");
        assert_eq!(human_size(1024 * 1024), "1.0 MiB");
        assert_eq!(human_size(2 * 1024 * 1024 * 1024), "2.0 GiB");
    }

    #[test]
    fn test_enter_composer_with_prefill_seeds_fields_focus_and_dirty_flag() {
        let account_id = AccountId::new();
        let original_id = MessageId::new();
        let mut app = AppState::default();
        app.enter_composer_with_prefill(
            account_id,
            ComposerPrefill {
                in_reply_to_msg: Some(original_id),
                to_addrs: vec!["alice@x.com".into()],
                cc_addrs: vec!["b@x.com".into(), "c@x.com".into()],
                bcc_addrs: Vec::new(),
                subject: Some("Re: Hi".into()),
                body: Some("On Sat, alice wrote:\n> Hi".into()),
                in_reply_to: Some("<orig@x>".into()),
                references_header: Some("<root@x> <orig@x>".into()),
                attachments: Vec::new(),
            },
        );
        let composer = app.composer.as_ref().unwrap();
        assert_eq!(composer.account_id, account_id);
        assert_eq!(composer.focused, ComposeField::Body);
        assert_eq!(composer.to, "alice@x.com");
        assert_eq!(composer.cc, "b@x.com, c@x.com");
        assert_eq!(composer.subject, "Re: Hi");
        assert!(composer.body.contains("> Hi"));
        assert!(composer.dirty);
        assert_eq!(composer.in_reply_to_msg, Some(original_id));
        assert_eq!(composer.in_reply_to.as_deref(), Some("<orig@x>"));
        assert_eq!(
            composer.references_header.as_deref(),
            Some("<root@x> <orig@x>")
        );

        // The serialised draft carries the threading context too so
        // the daemon side stitches the headers onto the outgoing
        // MIME.
        let draft = app.composer_draft().unwrap();
        assert_eq!(draft.in_reply_to_msg, Some(original_id));
        assert_eq!(draft.in_reply_to.as_deref(), Some("<orig@x>"));
        assert_eq!(
            draft.references_header.as_deref(),
            Some("<root@x> <orig@x>")
        );
    }

    #[test]
    fn test_enter_composer_for_existing_draft_seeds_state_clean_and_records_id() {
        let account_id = AccountId::new();
        let draft_id = DraftId::new();
        let mut app = AppState::default();
        app.enter_composer_for_existing_draft(
            draft_id,
            ComposerDraft {
                account_id,
                in_reply_to_msg: None,
                to_addrs: vec!["bob@x.com".into()],
                cc_addrs: Vec::new(),
                bcc_addrs: Vec::new(),
                subject: Some("Resume".into()),
                text_body: Some("partial work".into()),
                html_body: None,
                attachments: Vec::new(),
                in_reply_to: None,
                references_header: None,
            },
            ComposeField::Body,
        );
        let composer = app.composer.as_ref().unwrap();
        assert_eq!(composer.account_id, account_id);
        assert_eq!(composer.draft_id, Some(draft_id));
        assert_eq!(composer.to, "bob@x.com");
        assert_eq!(composer.subject, "Resume");
        assert_eq!(composer.body, "partial work");
        // Reopened drafts are clean — only edits flip the dirty flag.
        assert!(!composer.dirty);
        assert_eq!(composer.focused, ComposeField::Body);
        assert_eq!(app.mode, InputMode::Compose);
    }

    #[test]
    fn test_enter_composer_for_existing_draft_typing_marks_dirty_but_keeps_id() {
        let mut app = AppState::default();
        let draft_id = DraftId::new();
        app.enter_composer_for_existing_draft(
            draft_id,
            ComposerDraft {
                account_id: AccountId::new(),
                in_reply_to_msg: None,
                to_addrs: Vec::new(),
                cc_addrs: Vec::new(),
                bcc_addrs: Vec::new(),
                subject: None,
                text_body: Some("body".into()),
                html_body: None,
                attachments: Vec::new(),
                in_reply_to: None,
                references_header: None,
            },
            ComposeField::Body,
        );
        assert!(!app.composer_is_dirty());
        assert!(app.push_composer_char('!'));
        assert!(app.composer_is_dirty());
        // Save target stays the same draft so we hit `draft.update`.
        assert_eq!(app.composer_draft_id(), Some(draft_id));
    }

    #[test]
    fn test_drafts_pane_active_follows_selected_folder_role() {
        let mut app = AppState::default();
        app.apply_folders(vec![
            FolderItem {
                id: FolderId::new(),
                name: "INBOX".into(),
                role: "inbox".into(),
            },
            FolderItem {
                id: FolderId::new(),
                name: "[Gmail]/Drafts".into(),
                role: "drafts".into(),
            },
        ]);
        assert!(!app.drafts_pane_active());
        // Move the cursor to the drafts folder.
        app.selected_folder = 1;
        assert!(app.drafts_pane_active());
    }

    #[test]
    fn test_apply_drafts_clamps_selection_and_remove_local_works() {
        let mut app = AppState::default();
        let id_a = DraftId::new();
        let id_b = DraftId::new();
        let id_c = DraftId::new();
        let make = |id| DraftItem {
            id,
            account_id: AccountId::new(),
            subject: "s".into(),
            to: "t".into(),
            date: "d".into(),
            snippet: "x".into(),
        };
        app.apply_drafts(vec![make(id_a), make(id_b), make(id_c)]);
        app.selected_draft = 2;
        assert_eq!(app.selected_draft_id(), Some(id_c));
        assert!(app.remove_draft_locally(id_c));
        // After removal selection clamps to the new last row.
        assert_eq!(app.selected_draft_id(), Some(id_b));
        // No-op when already gone.
        assert!(!app.remove_draft_locally(id_c));
    }

    #[test]
    fn test_begin_draft_delete_uses_confirm_delete_mode() {
        let mut app = AppState::default();
        let id = DraftId::new();
        app.begin_draft_delete(id);
        assert_eq!(app.mode, InputMode::ConfirmDelete);
        assert_eq!(app.pending_delete_draft, Some(id));
        // Cancelling restores Normal and clears the slot.
        app.cancel_pending_delete_draft();
        assert_eq!(app.mode, InputMode::Normal);
        assert!(app.pending_delete_draft.is_none());
    }

    #[test]
    fn test_take_pending_delete_draft_returns_id_and_resets_mode() {
        let mut app = AppState::default();
        let id = DraftId::new();
        app.begin_draft_delete(id);
        assert_eq!(app.take_pending_delete_draft(), Some(id));
        assert_eq!(app.mode, InputMode::Normal);
        assert!(app.pending_delete_draft.is_none());
    }

    #[test]
    fn test_finish_command_returns_to_compose_when_composer_open() {
        let mut app = AppState::default();
        app.enter_composer(AccountId::new());
        app.enter_command_mode();
        app.push_command_char('w');
        let _ = app.finish_command();
        // `:w` from inside the composer should leave the composer
        // open, not drop back to Normal.
        assert_eq!(app.mode, InputMode::Compose);
        assert!(app.composer.is_some());
    }

    #[test]
    fn test_finish_command_drops_to_normal_when_no_composer() {
        let mut app = AppState::default();
        app.enter_command_mode();
        app.push_command_char('s');
        let _ = app.finish_command();
        assert_eq!(app.mode, InputMode::Normal);
    }
}
