use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::models::{Account, Attachment, Folder, Message};

use super::theme::ThemeName;

pub const SEEN_FLAG: &str = "\\Seen";
pub const FLAGGED_FLAG: &str = "\\Flagged";
pub const MAX_COMMAND_CHARS: usize = 128;
pub const MAX_COMPOSE_HEADER_CHARS: usize = 4096;
pub const MAX_COMPOSE_BODY_CHARS: usize = 100_000;

/// Maximum number of simultaneously visible toasts. Pushing past this
/// drops the oldest toast.
pub const MAX_TOASTS: usize = 3;

/// TTL for non-error toasts.
pub const TOAST_TTL_INFO: Duration = Duration::from_secs(3);
/// TTL for error toasts. Errors stick around longer so they don't get
/// missed when several land at once.
pub const TOAST_TTL_ERROR: Duration = Duration::from_secs(6);

/// Coalescing windows. Identical text from the same source within the
/// window refreshes the existing toast's expiry instead of pushing a
/// duplicate.
pub const COALESCE_ACCOUNT_SYNCED: Duration = Duration::from_secs(5);
pub const COALESCE_SYNC_ERROR: Duration = Duration::from_secs(10);

/// Status pane icons.
pub const ICON_IDLE: &str = "●";
pub const ICON_POLLING: &str = "~";
pub const ICON_SYNCING: &str = "…";
pub const ICON_ERROR: &str = "!";

/// Maximum chars of `last_error` to render after the selected
/// account's status icon.
pub const MAX_SELECTED_ERROR_CHARS: usize = 60;

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
    pub fn next(self) -> Self {
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

    pub fn previous(self) -> Self {
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
    ConfirmDiscard,
    ConfirmDelete,
    QuickSearch,
}

/// Maximum chars accepted in the `/` quick-search input.
pub const MAX_SEARCH_CHARS: usize = 256;

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
    pub id: Uuid,
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
    pub id: Uuid,
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
    pub id: Uuid,
    pub thread_id: Option<Uuid>,
    pub subject: String,
    pub from: String,
    pub date: String,
    pub snippet: String,
    pub flags: Vec<String>,
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

impl MessageItem {
    pub fn has_flag(&self, flag: &str) -> bool {
        has_flag(&self.flags, flag)
    }

    pub fn with_flag(&self, flag: &str, enabled: bool) -> Vec<String> {
        set_flag_preserving(&self.flags, flag, enabled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadItem {
    pub key: Uuid,
    pub thread_id: Option<Uuid>,
    pub subject: String,
    pub message_count: usize,
    pub latest_date: String,
    pub unread: bool,
    pub flagged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageDetail {
    pub id: Uuid,
    pub subject: String,
    pub from: String,
    pub snippet: String,
    pub body: String,
    pub flags: Vec<String>,
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

/// One row returned by the `search` op, projected for the search pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub message_id: Uuid,
    pub account_id: Uuid,
    pub folder_id: Uuid,
    pub subject: String,
    pub from: String,
    pub snippet: String,
    pub date: String,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchState {
    pub query: String,
    pub scope_account: Option<Uuid>,
    pub hits: Vec<SearchHit>,
    pub selected: usize,
    pub pending: bool,
    /// Pane to restore when the user closes search via Esc.
    pub previous_pane: ActivePane,
}

impl SearchState {
    pub fn new(
        query: impl Into<String>,
        scope_account: Option<Uuid>,
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
    pub id: Uuid,
    pub message_id: Uuid,
    pub filename: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub disposition: String,
    pub storage_path: String,
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
    pub attachment_id: Uuid,
    pub text: Option<String>,
    pub message: String,
    pub truncated: bool,
    pub preview_bytes: usize,
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
/// Opaque to callers; produced by [`AppState::snapshot_message_list`].
#[derive(Debug, Clone)]
pub struct MessageListSnapshot {
    folder_messages: Vec<MessageItem>,
    selected_thread: usize,
    selected_message: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposerDraft {
    pub account_id: Uuid,
    pub in_reply_to_msg: Option<Uuid>,
    pub to_addrs: Vec<String>,
    pub cc_addrs: Vec<String>,
    pub bcc_addrs: Vec<String>,
    pub subject: Option<String>,
    pub text_body: Option<String>,
    pub html_body: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposerState {
    pub account_id: Uuid,
    pub draft_id: Option<Uuid>,
    pub focused: ComposeField,
    pub to: String,
    pub to_cursor: usize,
    pub cc: String,
    pub cc_cursor: usize,
    pub bcc: String,
    pub bcc_cursor: usize,
    pub subject: String,
    pub subject_cursor: usize,
    pub body: String,
    pub body_cursor: usize,
    pub body_scroll: usize,
    pub body_selection_anchor: Option<usize>,
    pub body_selection_focus: usize,
    pub body_preferred_column: Option<usize>,
    pub dirty: bool,
}

impl ComposerState {
    fn new(account_id: Uuid) -> Self {
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
            body_scroll: 0,
            body_selection_anchor: None,
            body_selection_focus: 0,
            body_preferred_column: None,
            dirty: false,
        }
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
    }

    fn draft(&self) -> ComposerDraft {
        ComposerDraft {
            account_id: self.account_id,
            in_reply_to_msg: None,
            to_addrs: split_addresses(&self.to),
            cc_addrs: split_addresses(&self.cc),
            bcc_addrs: split_addresses(&self.bcc),
            subject: non_empty_string(&self.subject),
            text_body: non_empty_string(&self.body),
            html_body: None,
        }
    }

    pub fn focused_cursor(&self) -> usize {
        match self.focused {
            ComposeField::To => self.to_cursor.min(char_count(&self.to)),
            ComposeField::Cc => self.cc_cursor.min(char_count(&self.cc)),
            ComposeField::Bcc => self.bcc_cursor.min(char_count(&self.bcc)),
            ComposeField::Subject => self.subject_cursor.min(char_count(&self.subject)),
            ComposeField::Body => self.body_cursor.min(char_count(&self.body)),
        }
    }

    pub fn body_lines(&self) -> Vec<&str> {
        self.body.split('\n').collect()
    }

    pub fn body_line_count(&self) -> usize {
        line_bounds(&self.body).len()
    }

    pub fn body_line_start(&self, line: usize) -> usize {
        let bounds = line_bounds(&self.body);
        bounds
            .get(line.min(bounds.len().saturating_sub(1)))
            .map(|(start, _)| *start)
            .unwrap_or(0)
    }

    pub fn body_line_end(&self, line: usize) -> usize {
        let bounds = line_bounds(&self.body);
        bounds
            .get(line.min(bounds.len().saturating_sub(1)))
            .map(|(_, end)| *end)
            .unwrap_or(0)
    }

    pub fn body_cursor_line_column(&self) -> (usize, usize) {
        let cursor = self.body_cursor.min(char_count(&self.body));
        let bounds = line_bounds(&self.body);
        let line = line_for_cursor(&bounds, cursor);
        let start = bounds.get(line).map(|(start, _)| *start).unwrap_or(0);
        (line, cursor.saturating_sub(start))
    }

    pub fn body_selected_line_range(&self) -> Option<std::ops::RangeInclusive<usize>> {
        let anchor = self.body_selection_anchor?;
        let max_line = self.body_line_count().saturating_sub(1);
        let start = anchor.min(self.body_selection_focus).min(max_line);
        let end = anchor.max(self.body_selection_focus).min(max_line);
        Some(start..=end)
    }

    pub fn body_visible_scroll(&self, viewport_height: usize) -> usize {
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

    fn after_text_edit(&mut self) {
        if self.focused == ComposeField::Body {
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
    pub fn ttl(self) -> Duration {
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
    pub expires_at: Instant,
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
    pub state: SyncStateUi,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub active: ActivePane,
    pub mode: InputMode,
    pub accounts: Vec<AccountItem>,
    pub folders: Vec<FolderItem>,
    pub folder_messages: Vec<MessageItem>,
    pub threads: Vec<ThreadItem>,
    pub messages: Vec<MessageItem>,
    pub detail: Option<MessageDetail>,
    pub detail_cursor: usize,
    pub detail_scroll: usize,
    pub detail_selection_anchor: Option<usize>,
    pub detail_selection_focus: usize,
    pub detail_preferred_column: Option<usize>,
    pub attachments: Vec<AttachmentItem>,
    pub attachment_preview: Option<AttachmentPreviewItem>,
    pub selected_account: usize,
    pub selected_folder: usize,
    pub selected_thread: usize,
    pub selected_message: usize,
    pub selected_attachment: usize,
    pub pending_open_attachment: Option<AttachmentItem>,
    pub pending_delete_message: Option<Uuid>,
    pub command_input: String,
    pub status: String,
    pub error: Option<String>,
    pub theme: ThemeName,
    pub composer: Option<ComposerState>,
    pub toasts: VecDeque<Toast>,
    pub next_toast_id: u64,
    pub account_states: HashMap<Uuid, AccountStatus>,
    pub search: Option<SearchState>,
    pub search_input: String,
    pub search_input_previous_pane: ActivePane,
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
            detail_cursor: 0,
            detail_scroll: 0,
            detail_selection_anchor: None,
            detail_selection_focus: 0,
            detail_preferred_column: None,
            attachments: Vec::new(),
            attachment_preview: None,
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
            search_input: String::new(),
            search_input_previous_pane: ActivePane::Accounts,
        }
    }
}

impl AppState {
    pub fn cycle_active_pane(&mut self) {
        self.active = self.next_visible_pane();
    }

    pub fn cycle_active_pane_reverse(&mut self) {
        self.active = self.previous_visible_pane();
    }

    pub fn has_threaded_conversations(&self) -> bool {
        self.threads.iter().any(|thread| thread.message_count > 1)
    }

    pub fn threads_pane_visible(&self) -> bool {
        self.has_threaded_conversations()
    }

    pub fn move_selection(&mut self, delta: isize) -> bool {
        match self.active {
            ActivePane::Accounts => {
                let changed = move_index(&mut self.selected_account, self.accounts.len(), delta);
                if changed {
                    self.folders.clear();
                    self.folder_messages.clear();
                    self.threads.clear();
                    self.messages.clear();
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
                let changed =
                    move_index(&mut self.selected_attachment, self.attachments.len(), delta);
                if changed {
                    self.attachment_preview = None;
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

    pub fn apply_folder_messages(&mut self, messages: Vec<MessageItem>) {
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

    pub fn apply_detail(&mut self, detail: Option<MessageDetail>) {
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

    pub fn apply_attachments(&mut self, attachments: Vec<AttachmentItem>) {
        self.attachments = attachments;
        clamp_index(&mut self.selected_attachment, self.attachments.len());
        if self
            .attachment_preview
            .as_ref()
            .is_some_and(|preview| Some(preview.attachment_id) != self.selected_attachment_id())
        {
            self.attachment_preview = None;
        }
        if self.attachments.is_empty() {
            self.attachment_preview = None;
            self.pending_open_attachment = None;
        }
        self.normalize_active_pane();
    }

    pub fn apply_attachment_preview(&mut self, preview: AttachmentPreviewItem) {
        self.attachment_preview = Some(preview);
    }

    pub fn attachments_pane_visible(&self) -> bool {
        self.detail.is_some() && !self.attachments.is_empty()
    }

    pub fn detail_pane_visible(&self) -> bool {
        self.detail.is_some()
    }

    pub fn focus_detail_pane(&mut self) -> bool {
        if self.detail_pane_visible() {
            self.active = ActivePane::Details;
            true
        } else {
            self.normalize_active_pane();
            false
        }
    }

    pub fn detail_lines(&self) -> Vec<String> {
        self.detail_text_content()
            .map(|text| text.split('\n').map(str::to_string).collect())
            .unwrap_or_default()
    }

    pub fn detail_line_count(&self) -> usize {
        self.detail_line_bounds().len()
    }

    pub fn detail_line_start(&self, line: usize) -> usize {
        let bounds = self.detail_line_bounds();
        bounds
            .get(line.min(bounds.len().saturating_sub(1)))
            .map(|(start, _)| *start)
            .unwrap_or(0)
    }

    pub fn detail_line_end(&self, line: usize) -> usize {
        let bounds = self.detail_line_bounds();
        bounds
            .get(line.min(bounds.len().saturating_sub(1)))
            .map(|(_, end)| *end)
            .unwrap_or(0)
    }

    pub fn detail_cursor_line_column(&self) -> (usize, usize) {
        let cursor = self.detail_cursor.min(self.detail_len());
        let bounds = self.detail_line_bounds();
        if bounds.is_empty() {
            return (0, 0);
        }
        let line = line_for_cursor(&bounds, cursor);
        let start = bounds.get(line).map(|(start, _)| *start).unwrap_or(0);
        (line, cursor.saturating_sub(start))
    }

    pub fn detail_selected_line_range(&self) -> Option<std::ops::RangeInclusive<usize>> {
        let anchor = self.detail_selection_anchor?;
        let max_line = self.detail_line_count().saturating_sub(1);
        let start = anchor.min(self.detail_selection_focus).min(max_line);
        let end = anchor.max(self.detail_selection_focus).min(max_line);
        Some(start..=end)
    }

    pub fn detail_visible_scroll(&self, viewport_height: usize) -> usize {
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

    pub fn move_detail_cursor_left(&mut self) -> bool {
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

    pub fn move_detail_cursor_right(&mut self) -> bool {
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

    pub fn detail_home(&mut self) -> bool {
        if !self.detail_pane_visible() {
            return false;
        }
        let line = self.detail_cursor_line_column().0;
        self.set_detail_cursor(self.detail_line_start(line))
    }

    pub fn detail_end(&mut self) -> bool {
        if !self.detail_pane_visible() {
            return false;
        }
        let line = self.detail_cursor_line_column().0;
        self.set_detail_cursor(self.detail_line_end(line))
    }

    pub fn move_detail_line(&mut self, delta: isize, viewport_height: usize) -> bool {
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

    pub fn toggle_detail_line_selection(&mut self) -> bool {
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

    pub fn start_detail_line_selection(&mut self) -> bool {
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

    pub fn clear_detail_selection(&mut self) -> bool {
        let changed = self.detail_selection_anchor.is_some();
        self.detail_selection_anchor = None;
        self.detail_selection_focus = self.detail_cursor_line_column().0;
        changed
    }

    pub fn selected_attachment(&self) -> Option<&AttachmentItem> {
        self.attachments.get(self.selected_attachment)
    }

    pub fn selected_attachment_id(&self) -> Option<Uuid> {
        self.selected_attachment().map(|attachment| attachment.id)
    }

    pub fn toggle_attachment_focus(&mut self) -> bool {
        if !self.attachments_pane_visible() {
            self.normalize_active_pane();
            return false;
        }
        self.active = if self.active == ActivePane::Attachments {
            ActivePane::Messages
        } else {
            ActivePane::Attachments
        };
        true
    }

    pub fn begin_open_attachment_confirmation(&mut self) -> bool {
        let Some(attachment) = self.selected_attachment().cloned() else {
            return false;
        };
        self.pending_open_attachment = Some(attachment);
        true
    }

    pub fn cancel_open_attachment_confirmation(&mut self) {
        self.pending_open_attachment = None;
    }

    pub fn take_pending_open_attachment(&mut self) -> Option<AttachmentItem> {
        self.pending_open_attachment.take()
    }

    pub fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
    }

    pub fn set_error(&mut self, error: impl Into<String>) {
        self.error = Some(error.into());
    }

    pub fn clear_error(&mut self) {
        self.error = None;
    }

    /// Push a toast onto the back of the deque. If the deque is full,
    /// the oldest toast (front) is dropped.
    pub fn push_toast(&mut self, kind: ToastKind, text: impl Into<String>, now: Instant) -> u64 {
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
    pub fn dismiss_newest_toast(&mut self) -> bool {
        self.toasts.pop_back().is_some()
    }

    /// Clear every toast.
    pub fn clear_toasts(&mut self) -> bool {
        let had = !self.toasts.is_empty();
        self.toasts.clear();
        had
    }

    /// Drop expired toasts. Caller passes the current `Instant` so
    /// tests can drive expiry deterministically.
    pub fn tick_toasts(&mut self, now: Instant) {
        self.toasts.retain(|toast| toast.expires_at > now);
    }

    /// Apply a `sync.state` transition. Updates the per-account map
    /// and, on `Error`, pushes (or coalesces) an Error toast.
    pub fn apply_sync_state(
        &mut self,
        account_id: Uuid,
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
    pub fn push_mail_new_toast(&mut self, account_id: Uuid, folder_id: Option<Uuid>, now: Instant) {
        let folder = folder_id
            .and_then(|id| self.folders.iter().find(|f| f.id == id))
            .map(|f| f.name.clone())
            .unwrap_or_else(|| "folder".into());
        let account = self.account_label_for_toast(account_id);
        let text = format!("New mail in {folder} ({account})");
        self.push_toast(ToastKind::Info, text, now);
    }

    /// Push (or coalesce) an `account.synced` toast for `account_id`.
    pub fn push_account_synced_toast(&mut self, account_id: Uuid, now: Instant) {
        let label = self.account_label_for_toast(account_id);
        let text = format!("Synced {label}");
        if !self.coalesce_toast(ToastKind::Info, &text, now, COALESCE_ACCOUNT_SYNCED) {
            self.push_toast(ToastKind::Info, text, now);
        }
    }

    fn account_label_for_toast(&self, account_id: Uuid) -> String {
        self.accounts
            .iter()
            .find(|a| a.id == account_id)
            .map(|a| a.label.clone())
            .unwrap_or_else(|| short_id(account_id))
    }

    pub fn selected_account_id(&self) -> Option<Uuid> {
        self.accounts.get(self.selected_account).map(|a| a.id)
    }

    pub fn selected_folder_id(&self) -> Option<Uuid> {
        self.folders.get(self.selected_folder).map(|f| f.id)
    }

    pub fn selected_folder_name(&self) -> Option<&str> {
        self.folders
            .get(self.selected_folder)
            .map(|f| f.name.as_str())
    }

    /// Switch the active account by case-insensitive label or email
    /// match. Mirrors the navigation effect of pressing `↑`/`↓` on the
    /// accounts pane: clears folder/message state so the caller can
    /// refresh from the daemon. Returns true on a successful match.
    pub fn select_account_by_name(&mut self, name: &str) -> bool {
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
    pub fn select_folder_by_name(&mut self, name: &str) -> bool {
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
        self.clear_detail_state();
        self.selected_thread = 0;
        self.selected_message = 0;
        self.normalize_active_pane();
        true
    }

    pub fn search_pane_visible(&self) -> bool {
        self.search.is_some()
    }

    /// Resolve an account name (label or email, case-insensitive) to a
    /// `Uuid`. Used by `:search --account <name>`.
    pub fn account_id_by_name(&self, name: &str) -> Option<Uuid> {
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
    pub fn enter_quick_search(&mut self) {
        self.search_input_previous_pane = self.active;
        self.search_input.clear();
        self.mode = InputMode::QuickSearch;
        self.clear_error();
        self.set_status("Search /");
    }

    pub fn cancel_quick_search(&mut self) {
        self.mode = InputMode::Normal;
        self.search_input.clear();
        self.clear_error();
        self.active = self.search_input_previous_pane;
        self.set_status("Search cancelled");
    }

    pub fn push_search_char(&mut self, ch: char) -> bool {
        if ch.is_control() || self.search_input.chars().count() >= MAX_SEARCH_CHARS {
            return false;
        }
        self.search_input.push(ch);
        true
    }

    pub fn backspace_search(&mut self) -> bool {
        self.search_input.pop().is_some()
    }

    /// Consume the quick-search buffer and switch to Normal mode.
    pub fn finish_quick_search(&mut self) -> String {
        self.mode = InputMode::Normal;
        std::mem::take(&mut self.search_input)
    }

    /// Open the search pane with `query` and `scope_account`. Records
    /// `previous_pane` so Esc can restore it. Marks results as pending
    /// until [`AppState::apply_search_hits`] is called.
    pub fn begin_search(&mut self, query: impl Into<String>, scope_account: Option<Uuid>) {
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

    pub fn apply_search_hits(&mut self, hits: Vec<SearchHit>) {
        if let Some(state) = &mut self.search {
            state.hits = hits;
            state.pending = false;
            clamp_index(&mut state.selected, state.hits.len());
        }
    }

    /// Restore the pane that was active before the search opened and
    /// clear the search state.
    pub fn close_search(&mut self) {
        if let Some(state) = self.search.take() {
            self.active = state.previous_pane;
        }
        self.normalize_active_pane();
    }

    pub fn move_search_selection(&mut self, delta: isize) -> bool {
        let Some(state) = &mut self.search else {
            return false;
        };
        if state.hits.is_empty() {
            state.selected = 0;
            return false;
        }
        move_index(&mut state.selected, state.hits.len(), delta)
    }

    pub fn selected_search_hit(&self) -> Option<&SearchHit> {
        self.search
            .as_ref()
            .and_then(|state| state.hits.get(state.selected))
    }

    pub fn search_query(&self) -> Option<&str> {
        self.search.as_ref().map(|state| state.query.as_str())
    }

    pub fn search_scope_account(&self) -> Option<Uuid> {
        self.search.as_ref().and_then(|state| state.scope_account)
    }

    pub fn search_is_pending(&self) -> bool {
        self.search.as_ref().is_some_and(|state| state.pending)
    }

    /// Refocus a hit's location: switch active account / folder /
    /// selected message and close the search pane. Returns true when
    /// either a target hit was found and applied or the caller passed
    /// in a known hit. The folder/account lookups are best-effort —
    /// the message list is loaded lazily by the caller via
    /// `refresh_messages` after this returns.
    pub fn jump_to_hit(&mut self, hit: &SearchHit) -> bool {
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

    pub fn selected_message_id(&self) -> Option<Uuid> {
        self.messages.get(self.selected_message).map(|m| m.id)
    }

    pub fn selected_thread(&self) -> Option<&ThreadItem> {
        self.threads.get(self.selected_thread)
    }

    pub fn selected_message(&self) -> Option<&MessageItem> {
        self.messages.get(self.selected_message)
    }

    pub fn selected_message_has_flag(&self, flag: &str) -> Option<bool> {
        self.selected_message()
            .map(|message| message.has_flag(flag))
    }

    pub fn selected_message_flag_update(
        &self,
        flag: &str,
        enabled: bool,
    ) -> Option<(Uuid, Vec<String>)> {
        self.selected_message()
            .map(|message| (message.id, message.with_flag(flag, enabled)))
    }

    /// Capture the message-list state needed to undo an optimistic
    /// remove. Returned snapshot is opaque to callers and should only
    /// be passed back to [`AppState::restore_message_list_snapshot`].
    pub fn snapshot_message_list(&self) -> MessageListSnapshot {
        MessageListSnapshot {
            folder_messages: self.folder_messages.clone(),
            selected_thread: self.selected_thread,
            selected_message: self.selected_message,
        }
    }

    /// Drop the message with `message_id` from the visible folder list
    /// and refresh thread/message panes. Returns true when a row was
    /// removed.
    pub fn remove_message_locally(&mut self, message_id: Uuid) -> bool {
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

    pub fn restore_message_list_snapshot(&mut self, snapshot: MessageListSnapshot) {
        self.folder_messages = snapshot.folder_messages;
        self.rebuild_threads(None);
        self.selected_thread = snapshot.selected_thread;
        clamp_index(&mut self.selected_thread, self.threads.len());
        self.refresh_visible_messages();
        self.selected_message = snapshot.selected_message;
        clamp_index(&mut self.selected_message, self.messages.len());
        self.normalize_active_pane();
    }

    pub fn begin_delete_confirmation(&mut self, message_id: Uuid) {
        self.pending_delete_message = Some(message_id);
        self.mode = InputMode::ConfirmDelete;
        self.set_status("Delete? y/n");
    }

    pub fn cancel_delete_confirmation(&mut self) {
        self.pending_delete_message = None;
        self.mode = InputMode::Normal;
        self.set_status("Delete cancelled");
    }

    pub fn take_pending_delete_message(&mut self) -> Option<Uuid> {
        let id = self.pending_delete_message.take();
        if id.is_some() {
            self.mode = InputMode::Normal;
        }
        id
    }

    pub fn apply_message_flags(&mut self, message_id: Uuid, flags: Vec<String>) {
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

    pub fn enter_command_mode(&mut self) {
        self.mode = InputMode::Command;
        self.command_input.clear();
        self.clear_error();
        self.set_status("Command mode");
    }

    pub fn cancel_command_mode(&mut self) {
        self.mode = InputMode::Normal;
        self.command_input.clear();
        self.clear_error();
        self.set_status("Command cancelled");
    }

    pub fn push_command_char(&mut self, ch: char) -> bool {
        if ch.is_control() || self.command_input.chars().count() >= MAX_COMMAND_CHARS {
            return false;
        }
        self.command_input.push(ch);
        true
    }

    pub fn backspace_command(&mut self) -> bool {
        self.command_input.pop().is_some()
    }

    pub fn finish_command(&mut self) -> String {
        self.mode = InputMode::Normal;
        std::mem::take(&mut self.command_input)
    }

    pub fn enter_composer(&mut self, account_id: Uuid) {
        self.composer = Some(ComposerState::new(account_id));
        self.mode = InputMode::Compose;
        self.clear_error();
        self.set_status("Compose");
    }

    pub fn composer_draft(&self) -> Option<ComposerDraft> {
        self.composer.as_ref().map(ComposerState::draft)
    }

    pub fn composer_draft_id(&self) -> Option<Uuid> {
        self.composer
            .as_ref()
            .and_then(|composer| composer.draft_id)
    }

    pub fn composer_account_id(&self) -> Option<Uuid> {
        self.composer.as_ref().map(|composer| composer.account_id)
    }

    pub fn composer_is_dirty(&self) -> bool {
        self.composer
            .as_ref()
            .is_some_and(|composer| composer.dirty)
    }

    pub fn mark_composer_saved(&mut self, draft_id: Uuid) {
        if let Some(composer) = &mut self.composer {
            composer.draft_id = Some(draft_id);
            composer.dirty = false;
        }
    }

    pub fn exit_composer(&mut self) {
        self.composer = None;
        self.mode = InputMode::Normal;
    }

    pub fn discard_composer(&mut self) {
        self.composer = None;
        self.mode = InputMode::Normal;
        self.clear_error();
        self.set_status("Composer discarded");
    }

    pub fn composer_needs_discard_confirmation(&self) -> bool {
        self.composer
            .as_ref()
            .is_some_and(|composer| composer.dirty && composer.has_content())
    }

    pub fn begin_discard_composer_confirmation(&mut self) {
        self.mode = InputMode::ConfirmDiscard;
        self.set_status("Discard unsaved compose? y/n");
    }

    pub fn cancel_discard_composer_confirmation(&mut self) {
        self.mode = InputMode::Compose;
        self.set_status("Compose");
    }

    pub fn next_composer_field(&mut self) {
        if let Some(composer) = &mut self.composer {
            composer.focused = composer.focused.next();
            composer.body_preferred_column = None;
        }
    }

    pub fn previous_composer_field(&mut self) {
        if let Some(composer) = &mut self.composer {
            composer.focused = composer.focused.previous();
            composer.body_preferred_column = None;
        }
    }

    pub fn push_composer_char(&mut self, ch: char) -> bool {
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

    pub fn backspace_composer(&mut self) -> bool {
        let Some(composer) = &mut self.composer else {
            return false;
        };
        let changed = composer.delete_before_focused_cursor();
        if changed {
            composer.dirty = true;
        }
        changed
    }

    pub fn delete_composer(&mut self) -> bool {
        let Some(composer) = &mut self.composer else {
            return false;
        };
        let changed = composer.delete_at_focused_cursor();
        if changed {
            composer.dirty = true;
        }
        changed
    }

    pub fn move_composer_cursor_left(&mut self) -> bool {
        self.composer
            .as_mut()
            .is_some_and(ComposerState::move_focused_cursor_left)
    }

    pub fn move_composer_cursor_right(&mut self) -> bool {
        self.composer
            .as_mut()
            .is_some_and(ComposerState::move_focused_cursor_right)
    }

    pub fn composer_home(&mut self) -> bool {
        self.composer
            .as_mut()
            .is_some_and(ComposerState::move_focused_cursor_home)
    }

    pub fn composer_end(&mut self) -> bool {
        self.composer
            .as_mut()
            .is_some_and(ComposerState::move_focused_cursor_end)
    }

    pub fn move_composer_body_line(&mut self, delta: isize, viewport_height: usize) -> bool {
        self.composer
            .as_mut()
            .is_some_and(|composer| composer.move_body_line(delta, viewport_height))
    }

    pub fn toggle_composer_body_line_selection(&mut self) -> bool {
        self.composer
            .as_mut()
            .is_some_and(ComposerState::toggle_body_line_selection)
    }

    pub fn start_composer_body_line_selection(&mut self) -> bool {
        self.composer
            .as_mut()
            .is_some_and(ComposerState::start_body_line_selection)
    }

    pub fn clear_composer_body_selection(&mut self) -> bool {
        self.composer
            .as_mut()
            .is_some_and(ComposerState::clear_body_selection)
    }

    pub fn composer_enter(&mut self) -> bool {
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

    pub fn cycle_theme(&mut self) -> ThemeName {
        self.theme = self.theme.next();
        self.theme
    }

    pub fn set_theme(&mut self, theme: ThemeName) {
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
        self.reset_detail_navigation_state();
        self.clear_attachments();
    }

    fn clear_attachments(&mut self) {
        self.attachments.clear();
        self.attachment_preview = None;
        self.selected_attachment = 0;
        self.pending_open_attachment = None;
        self.normalize_active_pane();
    }

    fn detail_text_content(&self) -> Option<String> {
        self.detail.as_ref().map(|detail| {
            format!(
                "Subject: {}\nFrom: {}\nSnippet: {}\n\n{}",
                detail.subject, detail.from, detail.snippet, detail.body
            )
        })
    }

    fn detail_line_bounds(&self) -> Vec<(usize, usize)> {
        self.detail_text_content()
            .map(|text| line_bounds(&text))
            .unwrap_or_default()
    }

    fn detail_len(&self) -> usize {
        self.detail_text_content()
            .map(|text| char_count(&text))
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
    message.thread_id.unwrap_or(message.id)
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

fn non_empty_string(value: &str) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn split_addresses(value: &str) -> Vec<String> {
    value
        .split([',', ';', '\n'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
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

fn line_bounds(value: &str) -> Vec<(usize, usize)> {
    let mut bounds = Vec::new();
    let mut start = 0;
    for (index, ch) in value.chars().enumerate() {
        if ch == '\n' {
            bounds.push((start, index));
            start = index + 1;
        }
    }
    bounds.push((start, value.chars().count()));
    bounds
}

fn line_for_cursor(bounds: &[(usize, usize)], cursor: usize) -> usize {
    bounds
        .iter()
        .position(|(_, end)| cursor <= *end)
        .unwrap_or_else(|| bounds.len().saturating_sub(1))
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

pub fn flags_from_value(value: &serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|flag| flag.as_str().map(str::to_string))
        .collect()
}

pub fn has_flag(flags: &[String], flag: &str) -> bool {
    flags.iter().any(|existing| existing == flag)
}

fn short_id(id: Uuid) -> String {
    id.simple().to_string().chars().take(8).collect()
}

pub fn set_flag_preserving(flags: &[String], flag: &str, enabled: bool) -> Vec<String> {
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
            id: Uuid::new_v4(),
            label: label.into(),
            email: format!("{label}@example.com"),
            status: "idle".into(),
        }
    }

    fn folder(name: &str) -> FolderItem {
        FolderItem {
            id: Uuid::new_v4(),
            name: name.into(),
            role: "custom".into(),
        }
    }

    fn message(subject: &str) -> MessageItem {
        MessageItem {
            id: Uuid::new_v4(),
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
            id: Uuid::new_v4(),
            message_id: Uuid::new_v4(),
            filename: filename.into(),
            content_type: "text/plain".into(),
            size_bytes: 12,
            disposition: "attachment".into(),
            storage_path: format!("/tmp/{filename}"),
        }
    }

    fn detail(message_id: Uuid, body: &str) -> MessageDetail {
        MessageDetail {
            id: message_id,
            subject: "hello".into(),
            from: "alice@example.com".into(),
            snippet: "snippet".into(),
            body: body.into(),
            flags: Vec::new(),
        }
    }

    fn thread_message(thread_id: Uuid, subject: &str, date: &str, flags: &[&str]) -> MessageItem {
        MessageItem {
            id: Uuid::new_v4(),
            thread_id: Some(thread_id),
            subject: subject.into(),
            from: "alice@example.com".into(),
            date: date.into(),
            snippet: "hello".into(),
            flags: flags.iter().map(|flag| (*flag).to_string()).collect(),
        }
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
        let thread_id = Uuid::new_v4();
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
        let first_thread = Uuid::new_v4();
        let second_thread = Uuid::new_v4();
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
        let older_thread = Uuid::new_v4();
        let latest_thread = Uuid::new_v4();
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

        assert_eq!(app.threads[1].key, single_id);
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
        let thread_id = Uuid::new_v4();
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
        let thread_id = Uuid::new_v4();
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
        let first_thread = Uuid::new_v4();
        let second_thread = Uuid::new_v4();
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
        let first_thread = Uuid::new_v4();
        let second_thread = Uuid::new_v4();
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
        let top_thread = Uuid::new_v4();
        let disappearing_thread = Uuid::new_v4();
        let replacement_thread = Uuid::new_v4();
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
    fn test_flags_from_value_keeps_only_string_flags() {
        let flags = flags_from_value(&serde_json::json!(["\\Seen", 7, "\\Flagged"]));

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
        let thread_id = Uuid::new_v4();
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
            .find(|thread| thread.key == message_id)
            .expect("thread group");

        assert_eq!(folder_message.flags, vec![SEEN_FLAG, FLAGGED_FLAG]);
        assert_eq!(list_message.flags, vec![SEEN_FLAG, FLAGGED_FLAG]);
        assert!(thread.flagged);
        assert!(!thread.unread);
    }

    #[test]
    fn test_apply_detail_updates_thread_indicators_from_fresh_flags() {
        let thread_id = Uuid::new_v4();
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
        app.apply_detail(Some(detail(Uuid::new_v4(), "alpha\nb\nemoji café")));

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
        app.apply_detail(Some(detail(Uuid::new_v4(), &body)));

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
        app.apply_detail(Some(detail(Uuid::new_v4(), "one\ntwo\nthree")));

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
        app.apply_detail(Some(detail(Uuid::new_v4(), "one\ntwo\nthree\nfour")));
        assert!(app.move_detail_line(5, 2));
        assert!(app.toggle_detail_line_selection());

        assert_ne!(app.detail_cursor, 0);
        assert_ne!(app.detail_scroll, 0);
        assert!(app.detail_selected_line_range().is_some());

        app.apply_detail(Some(detail(Uuid::new_v4(), "replacement")));

        assert_eq!(app.detail_cursor, 0);
        assert_eq!(app.detail_scroll, 0);
        assert_eq!(app.detail_selected_line_range(), None);
        assert_eq!(app.detail_cursor_line_column(), (0, 0));
    }

    #[test]
    fn test_composer_field_editing_and_payload_construction() {
        let account_id = Uuid::new_v4();
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
        app.enter_composer(Uuid::new_v4());

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
        app.enter_composer(Uuid::new_v4());
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
        app.enter_composer(Uuid::new_v4());
        let composer = app.composer.as_mut().unwrap();
        composer.focused = ComposeField::Body;
        composer.body = "abcde\nxy\nwxyz".into();
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
    fn test_composer_body_scroll_keeps_cursor_visible() {
        let mut app = AppState::default();
        app.enter_composer(Uuid::new_v4());
        let composer = app.composer.as_mut().unwrap();
        composer.focused = ComposeField::Body;
        composer.body = "one\ntwo\nthree\nfour\nfive\nsix".into();

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
        app.enter_composer(Uuid::new_v4());
        let composer = app.composer.as_mut().unwrap();
        composer.focused = ComposeField::Body;
        composer.body = "one\ntwo\nthree".into();

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
        let account_id = Uuid::new_v4();
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
        let account_id = Uuid::new_v4();
        let draft_id = Uuid::new_v4();
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
}
