use uuid::Uuid;

use crate::models::{Account, Folder, Message};

use super::theme::ThemeName;

pub const SEEN_FLAG: &str = "\\Seen";
pub const FLAGGED_FLAG: &str = "\\Flagged";
pub const MAX_COMMAND_CHARS: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivePane {
    Accounts,
    Folders,
    Messages,
}

impl ActivePane {
    pub fn next(self) -> Self {
        match self {
            Self::Accounts => Self::Folders,
            Self::Folders => Self::Messages,
            Self::Messages => Self::Accounts,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Command,
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

#[derive(Debug, Clone)]
pub struct AppState {
    pub active: ActivePane,
    pub mode: InputMode,
    pub accounts: Vec<AccountItem>,
    pub folders: Vec<FolderItem>,
    pub messages: Vec<MessageItem>,
    pub detail: Option<MessageDetail>,
    pub selected_account: usize,
    pub selected_folder: usize,
    pub selected_message: usize,
    pub command_input: String,
    pub status: String,
    pub error: Option<String>,
    pub theme: ThemeName,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            active: ActivePane::Accounts,
            mode: InputMode::Normal,
            accounts: Vec::new(),
            folders: Vec::new(),
            messages: Vec::new(),
            detail: None,
            selected_account: 0,
            selected_folder: 0,
            selected_message: 0,
            command_input: String::new(),
            status: "Connecting".into(),
            error: None,
            theme: ThemeName::default(),
        }
    }
}

impl AppState {
    pub fn cycle_active_pane(&mut self) {
        self.active = self.active.next();
    }

    pub fn move_selection(&mut self, delta: isize) -> bool {
        match self.active {
            ActivePane::Accounts => {
                let changed = move_index(&mut self.selected_account, self.accounts.len(), delta);
                if changed {
                    self.folders.clear();
                    self.messages.clear();
                    self.detail = None;
                    self.selected_folder = 0;
                    self.selected_message = 0;
                }
                changed
            }
            ActivePane::Folders => {
                let changed = move_index(&mut self.selected_folder, self.folders.len(), delta);
                if changed {
                    self.messages.clear();
                    self.detail = None;
                    self.selected_message = 0;
                }
                changed
            }
            ActivePane::Messages => {
                let changed = move_index(&mut self.selected_message, self.messages.len(), delta);
                if changed {
                    self.detail = None;
                }
                changed
            }
        }
    }

    pub fn apply_accounts(&mut self, accounts: Vec<AccountItem>) {
        self.accounts = accounts;
        clamp_index(&mut self.selected_account, self.accounts.len());
        self.folders.clear();
        self.messages.clear();
        self.detail = None;
        self.selected_folder = 0;
        self.selected_message = 0;
    }

    pub fn apply_folders(&mut self, folders: Vec<FolderItem>) {
        self.folders = folders;
        clamp_index(&mut self.selected_folder, self.folders.len());
        self.messages.clear();
        self.detail = None;
        self.selected_message = 0;
    }

    pub fn apply_messages(&mut self, messages: Vec<MessageItem>) {
        self.messages = messages;
        clamp_index(&mut self.selected_message, self.messages.len());
        self.detail = None;
    }

    pub fn apply_detail(&mut self, detail: Option<MessageDetail>) {
        if let Some(detail) = &detail {
            if let Some(message) = self
                .messages
                .iter_mut()
                .find(|message| message.id == detail.id)
            {
                message.flags = detail.flags.clone();
            }
        }
        self.detail = detail;
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

    pub fn selected_message_id(&self) -> Option<Uuid> {
        self.messages.get(self.selected_message).map(|m| m.id)
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

    pub fn apply_message_flags(&mut self, message_id: Uuid, flags: Vec<String>) {
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

    pub fn cycle_theme(&mut self) -> ThemeName {
        self.theme = self.theme.next();
        self.theme
    }

    pub fn set_theme(&mut self, theme: ThemeName) {
        self.theme = theme;
    }
}

fn text_or_default(value: Option<&str>, default: &str) -> String {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(default)
        .to_string()
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
            subject: subject.into(),
            from: "alice@example.com".into(),
            date: "2026-05-07 10:00".into(),
            snippet: "hello".into(),
            flags: Vec::new(),
        }
    }

    #[test]
    fn test_cycle_active_pane_wraps_to_accounts() {
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
        assert!(app.messages.is_empty());
        assert!(app.detail.is_none());
        assert_eq!(app.selected_folder, 0);
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
    fn test_apply_accounts_empty_resets_selection_and_children() {
        let mut app = AppState {
            selected_account: 3,
            ..Default::default()
        };
        app.folders.push(folder("INBOX"));
        app.messages.push(message("hello"));

        app.apply_accounts(Vec::new());

        assert_eq!(app.selected_account, 0);
        assert!(app.folders.is_empty());
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
    fn test_theme_cycle_wraps_to_default() {
        let mut app = AppState::default();

        assert_eq!(app.theme, ThemeName::Default);
        assert_eq!(app.cycle_theme(), ThemeName::Dark);
        assert_eq!(app.cycle_theme(), ThemeName::HighContrast);
        assert_eq!(app.cycle_theme(), ThemeName::Default);
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
}
