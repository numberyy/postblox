pub mod app;
pub mod command;
pub mod ipc;
pub mod render;
pub mod theme;

use std::io::{self, Stdout};
use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use thiserror::Error;
use uuid::Uuid;

use app::{ActivePane, AppState, InputMode, FLAGGED_FLAG, SEEN_FLAG};
use command::{parse_command, Command};
use ipc::MailboxClient;

#[derive(Debug, Error)]
pub enum TuiError {
    #[error("unable to connect to daemon socket {path}: {source}")]
    Connect {
        path: PathBuf,
        #[source]
        source: ipc::MailboxError,
    },
    #[error("terminal error: {0}")]
    Terminal(#[from] std::io::Error),
}

type CrosstermTerminal = Terminal<CrosstermBackend<Stdout>>;

#[async_trait::async_trait(?Send)]
trait Mailbox {
    async fn list_accounts(&mut self) -> Result<Vec<app::AccountItem>, ipc::MailboxError>;
    async fn list_folders(
        &mut self,
        account_id: Uuid,
    ) -> Result<Vec<app::FolderItem>, ipc::MailboxError>;
    async fn list_messages(
        &mut self,
        folder_id: Uuid,
    ) -> Result<Vec<app::MessageItem>, ipc::MailboxError>;
    async fn get_message(
        &mut self,
        message_id: Uuid,
    ) -> Result<Option<app::MessageDetail>, ipc::MailboxError>;
    async fn sync_folder(
        &mut self,
        account_id: Uuid,
        folder_name: &str,
    ) -> Result<serde_json::Value, ipc::MailboxError>;
    async fn start_sync(
        &mut self,
        account_id: Uuid,
        folder_name: &str,
    ) -> Result<serde_json::Value, ipc::MailboxError>;
    async fn stop_sync(
        &mut self,
        account_id: Uuid,
        folder_name: &str,
    ) -> Result<serde_json::Value, ipc::MailboxError>;
    async fn set_flags(
        &mut self,
        message_id: Uuid,
        flags: &[String],
    ) -> Result<(), ipc::MailboxError>;
}

#[async_trait::async_trait(?Send)]
impl Mailbox for MailboxClient {
    async fn list_accounts(&mut self) -> Result<Vec<app::AccountItem>, ipc::MailboxError> {
        MailboxClient::list_accounts(self).await
    }

    async fn list_folders(
        &mut self,
        account_id: Uuid,
    ) -> Result<Vec<app::FolderItem>, ipc::MailboxError> {
        MailboxClient::list_folders(self, account_id).await
    }

    async fn list_messages(
        &mut self,
        folder_id: Uuid,
    ) -> Result<Vec<app::MessageItem>, ipc::MailboxError> {
        MailboxClient::list_messages(self, folder_id).await
    }

    async fn get_message(
        &mut self,
        message_id: Uuid,
    ) -> Result<Option<app::MessageDetail>, ipc::MailboxError> {
        MailboxClient::get_message(self, message_id).await
    }

    async fn sync_folder(
        &mut self,
        account_id: Uuid,
        folder_name: &str,
    ) -> Result<serde_json::Value, ipc::MailboxError> {
        MailboxClient::sync_folder(self, account_id, folder_name).await
    }

    async fn start_sync(
        &mut self,
        account_id: Uuid,
        folder_name: &str,
    ) -> Result<serde_json::Value, ipc::MailboxError> {
        MailboxClient::start_sync(self, account_id, folder_name).await
    }

    async fn stop_sync(
        &mut self,
        account_id: Uuid,
        folder_name: &str,
    ) -> Result<serde_json::Value, ipc::MailboxError> {
        MailboxClient::stop_sync(self, account_id, folder_name).await
    }

    async fn set_flags(
        &mut self,
        message_id: Uuid,
        flags: &[String],
    ) -> Result<(), ipc::MailboxError> {
        MailboxClient::set_flags(self, message_id, flags).await
    }
}

pub async fn run(socket_path: PathBuf) -> Result<(), TuiError> {
    let mut client = MailboxClient::connect(&socket_path)
        .await
        .map_err(|source| TuiError::Connect {
            path: socket_path.clone(),
            source,
        })?;
    let mut app = AppState::default();
    app.set_status(format!("Connected to {}", socket_path.display()));
    refresh_accounts(&mut app, &mut client).await;

    let mut terminal = setup_terminal()?;
    let result = run_loop(&mut terminal, app, client).await;
    let restore_result = restore_terminal(&mut terminal);

    match (result, restore_result) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

async fn run_loop(
    terminal: &mut CrosstermTerminal,
    mut app: AppState,
    mut client: MailboxClient,
) -> Result<(), TuiError> {
    loop {
        terminal.draw(|frame| render::render(frame, &app))?;
        if event::poll(Duration::from_millis(200))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if handle_key(key, &mut app, &mut client).await {
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

async fn handle_key<C: Mailbox + ?Sized>(
    key: KeyEvent,
    app: &mut AppState,
    client: &mut C,
) -> bool {
    if app.mode == InputMode::Command {
        return handle_command_key(key, app, client).await;
    }

    match key.code {
        KeyCode::Char(':') => {
            app.enter_command_mode();
            false
        }
        KeyCode::Char('q') => true,
        KeyCode::Down | KeyCode::Char('j') => {
            if app.move_selection(1) {
                refresh_after_selection_change(app, client).await;
            }
            false
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if app.move_selection(-1) {
                refresh_after_selection_change(app, client).await;
            }
            false
        }
        KeyCode::Right | KeyCode::Tab => {
            app.cycle_active_pane();
            false
        }
        KeyCode::Left => {
            app.cycle_active_pane_reverse();
            false
        }
        KeyCode::Char('r') => {
            refresh_current_pane(app, client).await;
            false
        }
        KeyCode::Char('s') => {
            execute_command(Command::Sync, app, client).await;
            false
        }
        KeyCode::Char('u') => {
            let command = if app.selected_message_has_flag(SEEN_FLAG).unwrap_or(false) {
                Command::Unseen
            } else {
                Command::Seen
            };
            execute_command(command, app, client).await;
            false
        }
        KeyCode::Char('f') => {
            let command = if app.selected_message_has_flag(FLAGGED_FLAG).unwrap_or(false) {
                Command::Unflag
            } else {
                Command::Flag
            };
            execute_command(command, app, client).await;
            false
        }
        KeyCode::Char('t') => {
            execute_command(Command::ThemeNext, app, client).await;
            false
        }
        KeyCode::Enter => {
            if app.active == ActivePane::Threads {
                app.active = ActivePane::Messages;
                refresh_detail(app, client).await;
            } else {
                refresh_after_selection_change(app, client).await;
            }
            false
        }
        _ => false,
    }
}

async fn handle_command_key<C: Mailbox + ?Sized>(
    key: KeyEvent,
    app: &mut AppState,
    client: &mut C,
) -> bool {
    match key.code {
        KeyCode::Esc => {
            app.cancel_command_mode();
            false
        }
        KeyCode::Enter => {
            let input = app.finish_command();
            run_command_line(input, app, client).await;
            false
        }
        KeyCode::Backspace => {
            app.backspace_command();
            false
        }
        KeyCode::Char(ch) => {
            if !app.push_command_char(ch) {
                app.set_error(format!(
                    "command is limited to {} characters",
                    app::MAX_COMMAND_CHARS
                ));
            }
            false
        }
        _ => false,
    }
}

async fn run_command_line<C: Mailbox + ?Sized>(input: String, app: &mut AppState, client: &mut C) {
    match parse_command(&input) {
        Ok(command) => execute_command(command, app, client).await,
        Err(error) => record_command_parse_error(app, error.to_string()),
    }
}

async fn execute_command<C: Mailbox + ?Sized>(
    command: Command,
    app: &mut AppState,
    client: &mut C,
) {
    match command {
        Command::Sync => run_folder_write(app, client, FolderWrite::Sync).await,
        Command::StartSync => run_folder_write(app, client, FolderWrite::StartSync).await,
        Command::StopSync => run_folder_write(app, client, FolderWrite::StopSync).await,
        Command::Seen => run_flag_write(app, client, SEEN_FLAG, true, "Marked message seen").await,
        Command::Unseen => {
            run_flag_write(app, client, SEEN_FLAG, false, "Marked message unseen").await;
        }
        Command::Flag => {
            run_flag_write(app, client, FLAGGED_FLAG, true, "Flagged message").await;
        }
        Command::Unflag => {
            run_flag_write(app, client, FLAGGED_FLAG, false, "Unflagged message").await;
        }
        Command::ThemeNext => {
            let theme = app.cycle_theme();
            app.clear_error();
            app.set_status(format!("Theme: {theme}"));
        }
        Command::Theme(theme) => {
            app.set_theme(theme);
            app.clear_error();
            app.set_status(format!("Theme: {theme}"));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FolderWrite {
    Sync,
    StartSync,
    StopSync,
}

impl FolderWrite {
    fn running_status(self, folder_name: &str) -> String {
        match self {
            Self::Sync => format!("Syncing {folder_name}"),
            Self::StartSync => format!("Starting sync for {folder_name}"),
            Self::StopSync => format!("Stopping sync for {folder_name}"),
        }
    }

    fn success_status(self, folder_name: &str) -> String {
        match self {
            Self::Sync => format!("Synced {folder_name}"),
            Self::StartSync => format!("Started sync for {folder_name}"),
            Self::StopSync => format!("Stopped sync for {folder_name}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
enum CommandRunError {
    #[error("No account selected")]
    AccountNotSelected,
    #[error("No folder selected")]
    FolderUnavailable,
    #[error("No message selected")]
    MessageMissing,
}

async fn run_folder_write<C: Mailbox + ?Sized>(
    app: &mut AppState,
    client: &mut C,
    op: FolderWrite,
) {
    let (account_id, folder_name) = match selected_account_folder(app) {
        Ok(selection) => selection,
        Err(error) => {
            record_command_run_error(app, error);
            return;
        }
    };

    app.clear_error();
    app.set_status(op.running_status(&folder_name));
    let result = match op {
        FolderWrite::Sync => client.sync_folder(account_id, &folder_name).await,
        FolderWrite::StartSync => client.start_sync(account_id, &folder_name).await,
        FolderWrite::StopSync => client.stop_sync(account_id, &folder_name).await,
    };

    match result {
        Ok(_) => {
            refresh_messages(app, client).await;
            if app.error.is_none() {
                app.set_status(op.success_status(&folder_name));
            }
        }
        Err(error) => record_error(app, error),
    }
}

async fn run_flag_write<C: Mailbox + ?Sized>(
    app: &mut AppState,
    client: &mut C,
    flag: &str,
    enabled: bool,
    success: &'static str,
) {
    let (message_id, flags) = match app.selected_message_flag_update(flag, enabled) {
        Some(update) => update,
        None => {
            record_command_run_error(app, CommandRunError::MessageMissing);
            return;
        }
    };

    app.clear_error();
    app.set_status(success);
    match client.set_flags(message_id, &flags).await {
        Ok(()) => {
            app.apply_message_flags(message_id, flags);
            refresh_messages(app, client).await;
            if app.error.is_none() {
                app.set_status(success);
            }
        }
        Err(error) => record_error(app, error),
    }
}

fn selected_account_folder(app: &AppState) -> Result<(Uuid, String), CommandRunError> {
    let account_id = app
        .selected_account_id()
        .ok_or(CommandRunError::AccountNotSelected)?;
    let folder_name = app
        .selected_folder_name()
        .ok_or(CommandRunError::FolderUnavailable)?
        .to_string();
    Ok((account_id, folder_name))
}

fn record_command_parse_error(app: &mut AppState, message: String) {
    app.set_status(message.clone());
    app.set_error(message);
}

fn record_command_run_error(app: &mut AppState, error: CommandRunError) {
    let message = error.to_string();
    app.set_status(message.clone());
    app.set_error(message);
}

async fn refresh_current_pane<C: Mailbox + ?Sized>(app: &mut AppState, client: &mut C) {
    match app.active {
        ActivePane::Accounts => refresh_accounts(app, client).await,
        ActivePane::Folders => refresh_folders(app, client).await,
        ActivePane::Threads => refresh_messages(app, client).await,
        ActivePane::Messages => refresh_messages(app, client).await,
    }
}

async fn refresh_after_selection_change<C: Mailbox + ?Sized>(app: &mut AppState, client: &mut C) {
    match app.active {
        ActivePane::Accounts => refresh_folders(app, client).await,
        ActivePane::Folders => refresh_messages(app, client).await,
        ActivePane::Threads => refresh_detail(app, client).await,
        ActivePane::Messages => refresh_detail(app, client).await,
    }
}

async fn refresh_accounts<C: Mailbox + ?Sized>(app: &mut AppState, client: &mut C) {
    app.set_status("Loading accounts");
    match client.list_accounts().await {
        Ok(accounts) => {
            let count = accounts.len();
            app.clear_error();
            app.apply_accounts(accounts);
            if count == 0 {
                app.set_status("Connected. No accounts found");
            } else {
                app.set_status(format!("Loaded {count} account(s)"));
                refresh_folders(app, client).await;
            }
        }
        Err(error) => record_error(app, error),
    }
}

async fn refresh_folders<C: Mailbox + ?Sized>(app: &mut AppState, client: &mut C) {
    let Some(account_id) = app.selected_account_id() else {
        app.apply_folders(Vec::new());
        app.set_status("No account selected");
        return;
    };

    app.set_status("Loading folders");
    match client.list_folders(account_id).await {
        Ok(folders) => {
            let count = folders.len();
            app.clear_error();
            app.apply_folders(folders);
            if count == 0 {
                app.set_status("No folders for selected account");
            } else {
                app.set_status(format!("Loaded {count} folder(s)"));
                refresh_messages(app, client).await;
            }
        }
        Err(error) => record_error(app, error),
    }
}

async fn refresh_messages<C: Mailbox + ?Sized>(app: &mut AppState, client: &mut C) {
    let Some(folder_id) = app.selected_folder_id() else {
        app.apply_folder_messages(Vec::new());
        app.set_status("No folder selected");
        return;
    };

    app.set_status("Loading messages");
    match client.list_messages(folder_id).await {
        Ok(messages) => {
            let message_count = messages.len();
            app.clear_error();
            app.apply_folder_messages(messages);
            let thread_count = app.threads.len();
            if message_count == 0 {
                app.set_status("No messages in selected folder");
            } else {
                if app.threads_pane_visible() {
                    app.set_status(format!(
                        "Loaded {thread_count} thread(s), {message_count} message(s)"
                    ));
                } else {
                    app.set_status(format!("Loaded {message_count} message(s)"));
                }
                refresh_detail(app, client).await;
            }
        }
        Err(error) => record_error(app, error),
    }
}

async fn refresh_detail<C: Mailbox + ?Sized>(app: &mut AppState, client: &mut C) {
    let Some(message_id) = app.selected_message_id() else {
        app.apply_detail(None);
        app.set_status("No message selected");
        return;
    };

    app.set_status("Loading message");
    match client.get_message(message_id).await {
        Ok(detail) => {
            app.clear_error();
            if detail.is_some() {
                app.set_status("Message loaded");
            } else {
                app.set_status("Message no longer exists");
            }
            app.apply_detail(detail);
        }
        Err(error) => record_error(app, error),
    }
}

fn record_error(app: &mut AppState, error: ipc::MailboxError) {
    let message = error.to_string();
    app.set_status(message.clone());
    app.set_error(message);
}

fn setup_terminal() -> Result<CrosstermTerminal, TuiError> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    Ok(Terminal::new(backend)?)
}

fn restore_terminal(terminal: &mut CrosstermTerminal) -> Result<(), TuiError> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyEvent, KeyModifiers};
    use serde_json::json;

    use super::*;
    use crate::tui::app::{AccountItem, FolderItem, MessageDetail, MessageItem};
    use crate::tui::theme::ThemeName;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        Sync(Uuid, String),
        StartSync(Uuid, String),
        StopSync(Uuid, String),
        SetFlags(Uuid, Vec<String>),
        ListMessages(Uuid),
        GetMessage(Uuid),
    }

    #[derive(Default)]
    struct MockMailbox {
        calls: Vec<Call>,
        messages: Vec<MessageItem>,
        detail: Option<MessageDetail>,
        fail_sync: bool,
        fail_set_flags: bool,
    }

    #[async_trait::async_trait(?Send)]
    impl Mailbox for MockMailbox {
        async fn list_accounts(&mut self) -> Result<Vec<AccountItem>, ipc::MailboxError> {
            Ok(Vec::new())
        }

        async fn list_folders(&mut self, _: Uuid) -> Result<Vec<FolderItem>, ipc::MailboxError> {
            Ok(Vec::new())
        }

        async fn list_messages(
            &mut self,
            folder_id: Uuid,
        ) -> Result<Vec<MessageItem>, ipc::MailboxError> {
            self.calls.push(Call::ListMessages(folder_id));
            Ok(self.messages.clone())
        }

        async fn get_message(
            &mut self,
            message_id: Uuid,
        ) -> Result<Option<MessageDetail>, ipc::MailboxError> {
            self.calls.push(Call::GetMessage(message_id));
            Ok(self.detail.clone())
        }

        async fn sync_folder(
            &mut self,
            account_id: Uuid,
            folder_name: &str,
        ) -> Result<serde_json::Value, ipc::MailboxError> {
            self.calls
                .push(Call::Sync(account_id, folder_name.to_string()));
            if self.fail_sync {
                Err(server_error("account.sync_folder"))
            } else {
                Ok(json!({"inserted": 0, "wiped": 0}))
            }
        }

        async fn start_sync(
            &mut self,
            account_id: Uuid,
            folder_name: &str,
        ) -> Result<serde_json::Value, ipc::MailboxError> {
            self.calls
                .push(Call::StartSync(account_id, folder_name.to_string()));
            Ok(json!({"ok": true, "started": true}))
        }

        async fn stop_sync(
            &mut self,
            account_id: Uuid,
            folder_name: &str,
        ) -> Result<serde_json::Value, ipc::MailboxError> {
            self.calls
                .push(Call::StopSync(account_id, folder_name.to_string()));
            Ok(json!({"ok": true, "stopped": true}))
        }

        async fn set_flags(
            &mut self,
            message_id: Uuid,
            flags: &[String],
        ) -> Result<(), ipc::MailboxError> {
            self.calls.push(Call::SetFlags(message_id, flags.to_vec()));
            if self.fail_set_flags {
                Err(server_error("message.set_flags"))
            } else {
                Ok(())
            }
        }
    }

    fn server_error(op: &'static str) -> ipc::MailboxError {
        ipc::MailboxError::Server {
            op,
            code: "boom".into(),
            message: "daemon rejected request".into(),
        }
    }

    fn account_item(id: Uuid) -> AccountItem {
        AccountItem {
            id,
            label: "Work".into(),
            email: "work@example.com".into(),
            status: "idle".into(),
        }
    }

    fn folder_item(id: Uuid) -> FolderItem {
        FolderItem {
            id,
            name: "INBOX".into(),
            role: "inbox".into(),
        }
    }

    fn message_item(id: Uuid, flags: Vec<&str>) -> MessageItem {
        MessageItem {
            id,
            thread_id: None,
            subject: "Hello".into(),
            from: "alice@example.com".into(),
            date: "2026-05-07 10:00".into(),
            snippet: "Preview".into(),
            flags: flags.into_iter().map(str::to_string).collect(),
        }
    }

    fn thread_message_item(
        thread_id: Uuid,
        subject: &str,
        date: &str,
        flags: Vec<&str>,
    ) -> MessageItem {
        MessageItem {
            id: Uuid::new_v4(),
            thread_id: Some(thread_id),
            subject: subject.into(),
            from: "alice@example.com".into(),
            date: date.into(),
            snippet: "Preview".into(),
            flags: flags.into_iter().map(str::to_string).collect(),
        }
    }

    fn detail_for(message: &MessageItem) -> MessageDetail {
        MessageDetail {
            id: message.id,
            subject: message.subject.clone(),
            from: message.from.clone(),
            snippet: message.snippet.clone(),
            body: "Body".into(),
            flags: message.flags.clone(),
        }
    }

    fn app_with_account_folder(account_id: Uuid, folder_id: Uuid) -> AppState {
        let mut app = AppState::default();
        app.apply_accounts(vec![account_item(account_id)]);
        app.apply_folders(vec![folder_item(folder_id)]);
        app
    }

    fn app_with_threaded_messages() -> AppState {
        let thread_id = Uuid::new_v4();
        let mut app = AppState::default();
        app.apply_folder_messages(vec![
            thread_message_item(thread_id, "Reply", "2026-05-07 11:00", vec!["\\Seen"]),
            thread_message_item(thread_id, "Start", "2026-05-07 10:00", vec!["\\Seen"]),
        ]);
        app
    }

    #[tokio::test]
    async fn test_execute_command_sync_calls_daemon_and_refreshes_messages() {
        let account_id = Uuid::new_v4();
        let folder_id = Uuid::new_v4();
        let mut app = app_with_account_folder(account_id, folder_id);
        let mut client = MockMailbox::default();

        execute_command(Command::Sync, &mut app, &mut client).await;

        assert_eq!(
            client.calls,
            vec![
                Call::Sync(account_id, "INBOX".into()),
                Call::ListMessages(folder_id),
            ]
        );
        assert_eq!(app.status, "Synced INBOX");
        assert!(app.error.is_none());
    }

    #[tokio::test]
    async fn test_execute_command_start_and_stop_sync_use_selected_folder() {
        let account_id = Uuid::new_v4();
        let folder_id = Uuid::new_v4();
        let mut app = app_with_account_folder(account_id, folder_id);
        let mut client = MockMailbox::default();

        execute_command(Command::StartSync, &mut app, &mut client).await;
        execute_command(Command::StopSync, &mut app, &mut client).await;

        assert_eq!(
            client.calls,
            vec![
                Call::StartSync(account_id, "INBOX".into()),
                Call::ListMessages(folder_id),
                Call::StopSync(account_id, "INBOX".into()),
                Call::ListMessages(folder_id),
            ]
        );
        assert_eq!(app.status, "Stopped sync for INBOX");
        assert!(app.error.is_none());
    }

    #[tokio::test]
    async fn test_execute_command_seen_preserves_other_flags() {
        let account_id = Uuid::new_v4();
        let folder_id = Uuid::new_v4();
        let message_id = Uuid::new_v4();
        let mut app = app_with_account_folder(account_id, folder_id);
        app.apply_messages(vec![message_item(message_id, vec!["\\Answered"])]);
        let mut client = MockMailbox::default();

        execute_command(Command::Seen, &mut app, &mut client).await;

        assert_eq!(
            client.calls,
            vec![
                Call::SetFlags(message_id, vec!["\\Answered".into(), "\\Seen".into()]),
                Call::ListMessages(folder_id),
            ]
        );
        assert_eq!(app.status, "Marked message seen");
        assert!(app.error.is_none());
    }

    #[tokio::test]
    async fn test_refresh_messages_builds_thread_rows_and_loads_selected_message() {
        let account_id = Uuid::new_v4();
        let folder_id = Uuid::new_v4();
        let thread_id = Uuid::new_v4();
        let mut app = app_with_account_folder(account_id, folder_id);
        let older = thread_message_item(thread_id, "Start", "2026-05-07 09:00", vec!["\\Seen"]);
        let newer = thread_message_item(thread_id, "Reply", "2026-05-07 10:00", vec![]);
        let mut client = MockMailbox {
            messages: vec![newer, older.clone()],
            detail: Some(detail_for(&older)),
            ..Default::default()
        };

        refresh_messages(&mut app, &mut client).await;

        assert_eq!(
            client.calls,
            vec![Call::ListMessages(folder_id), Call::GetMessage(older.id)]
        );
        assert_eq!(app.threads.len(), 1);
        assert_eq!(app.threads[0].message_count, 2);
        assert!(app.threads[0].unread);
        assert_eq!(app.messages[0].id, older.id);
        assert_eq!(app.detail.as_ref().unwrap().id, older.id);
        assert_eq!(app.status, "Message loaded");
    }

    #[tokio::test]
    async fn test_refresh_messages_moves_active_threads_to_messages_when_threads_hide() {
        let account_id = Uuid::new_v4();
        let folder_id = Uuid::new_v4();
        let thread_id = Uuid::new_v4();
        let mut app = app_with_account_folder(account_id, folder_id);
        app.apply_folder_messages(vec![
            thread_message_item(thread_id, "Reply", "2026-05-07 10:00", vec!["\\Seen"]),
            thread_message_item(thread_id, "Start", "2026-05-07 09:00", vec!["\\Seen"]),
        ]);
        app.active = ActivePane::Threads;
        let first = message_item(Uuid::new_v4(), vec!["\\Seen"]);
        let second = message_item(Uuid::new_v4(), vec![]);
        let mut client = MockMailbox {
            messages: vec![first.clone(), second.clone()],
            detail: Some(detail_for(&first)),
            ..Default::default()
        };

        refresh_messages(&mut app, &mut client).await;

        assert_eq!(
            client.calls,
            vec![Call::ListMessages(folder_id), Call::GetMessage(first.id)]
        );
        assert!(!app.threads_pane_visible());
        assert_eq!(app.active, ActivePane::Messages);
        assert_eq!(
            app.messages
                .iter()
                .map(|message| message.id)
                .collect::<Vec<_>>(),
            vec![first.id, second.id]
        );
        assert_eq!(app.detail.as_ref().unwrap().id, first.id);
    }

    #[tokio::test]
    async fn test_execute_command_flag_error_keeps_local_flags_and_reports_daemon_error() {
        let account_id = Uuid::new_v4();
        let folder_id = Uuid::new_v4();
        let message_id = Uuid::new_v4();
        let mut app = app_with_account_folder(account_id, folder_id);
        app.apply_messages(vec![message_item(message_id, vec!["\\Seen"])]);
        let mut client = MockMailbox {
            fail_set_flags: true,
            ..Default::default()
        };

        execute_command(Command::Flag, &mut app, &mut client).await;

        assert_eq!(
            client.calls,
            vec![Call::SetFlags(
                message_id,
                vec!["\\Seen".into(), "\\Flagged".into()]
            )]
        );
        assert_eq!(app.messages[0].flags, vec!["\\Seen"]);
        assert!(app.error.as_deref().unwrap().contains("boom"));
    }

    #[tokio::test]
    async fn test_execute_command_reports_missing_selections_without_daemon_call() {
        let mut app = AppState::default();
        let mut client = MockMailbox::default();

        execute_command(Command::Sync, &mut app, &mut client).await;
        assert_eq!(app.error.as_deref(), Some("No account selected"));

        app.clear_error();
        execute_command(Command::Seen, &mut app, &mut client).await;
        assert_eq!(app.error.as_deref(), Some("No message selected"));
        assert!(client.calls.is_empty());
    }

    #[tokio::test]
    async fn test_run_command_line_reports_parse_errors() {
        let mut app = AppState::default();
        let mut client = MockMailbox::default();

        run_command_line("theme solarized".into(), &mut app, &mut client).await;

        assert_eq!(
            app.error.as_deref(),
            Some("usage: theme next|default|dark|high-contrast")
        );
        assert!(client.calls.is_empty());
    }

    #[tokio::test]
    async fn test_handle_key_theme_shortcut_cycles_theme() {
        let mut app = AppState::default();
        let mut client = MockMailbox::default();

        let quit = handle_key(
            KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE),
            &mut app,
            &mut client,
        )
        .await;

        assert!(!quit);
        assert_eq!(app.theme, ThemeName::Dark);
        assert_eq!(app.status, "Theme: dark");
    }

    #[tokio::test]
    async fn test_handle_key_tab_skips_threads_pane_when_hidden() {
        let mut app = AppState::default();
        let mut client = MockMailbox::default();

        for expected in [
            ActivePane::Folders,
            ActivePane::Messages,
            ActivePane::Accounts,
            ActivePane::Folders,
        ] {
            assert!(
                !handle_key(
                    KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
                    &mut app,
                    &mut client,
                )
                .await
            );
            assert_eq!(app.active, expected);
        }
    }

    #[tokio::test]
    async fn test_handle_key_tab_includes_threads_pane_when_visible() {
        let mut app = app_with_threaded_messages();
        let mut client = MockMailbox::default();

        for expected in [
            ActivePane::Folders,
            ActivePane::Threads,
            ActivePane::Messages,
            ActivePane::Accounts,
        ] {
            assert!(
                !handle_key(
                    KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
                    &mut app,
                    &mut client,
                )
                .await
            );
            assert_eq!(app.active, expected);
        }
    }

    #[tokio::test]
    async fn test_handle_key_right_skips_threads_pane_when_hidden() {
        let mut app = AppState::default();
        let mut client = MockMailbox::default();

        for expected in [
            ActivePane::Folders,
            ActivePane::Messages,
            ActivePane::Accounts,
            ActivePane::Folders,
        ] {
            assert!(
                !handle_key(
                    KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
                    &mut app,
                    &mut client,
                )
                .await
            );
            assert_eq!(app.active, expected);
        }
    }

    #[tokio::test]
    async fn test_handle_key_right_includes_threads_pane_when_visible() {
        let mut app = app_with_threaded_messages();
        let mut client = MockMailbox::default();

        for expected in [
            ActivePane::Folders,
            ActivePane::Threads,
            ActivePane::Messages,
            ActivePane::Accounts,
        ] {
            assert!(
                !handle_key(
                    KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
                    &mut app,
                    &mut client,
                )
                .await
            );
            assert_eq!(app.active, expected);
        }
    }

    #[tokio::test]
    async fn test_handle_key_left_skips_threads_pane_when_hidden() {
        let mut app = AppState::default();
        let mut client = MockMailbox::default();

        for expected in [
            ActivePane::Messages,
            ActivePane::Folders,
            ActivePane::Accounts,
            ActivePane::Messages,
        ] {
            assert!(
                !handle_key(
                    KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
                    &mut app,
                    &mut client,
                )
                .await
            );
            assert_eq!(app.active, expected);
        }
    }

    #[tokio::test]
    async fn test_handle_key_left_includes_threads_pane_when_visible() {
        let mut app = app_with_threaded_messages();
        let mut client = MockMailbox::default();

        for expected in [
            ActivePane::Messages,
            ActivePane::Threads,
            ActivePane::Folders,
            ActivePane::Accounts,
        ] {
            assert!(
                !handle_key(
                    KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
                    &mut app,
                    &mut client,
                )
                .await
            );
            assert_eq!(app.active, expected);
        }
    }

    #[tokio::test]
    async fn test_handle_key_up_down_move_selection_without_switching_panes() {
        let first_thread = Uuid::new_v4();
        let second_thread = Uuid::new_v4();
        let mut app = AppState {
            active: ActivePane::Threads,
            ..Default::default()
        };
        app.apply_folder_messages(vec![
            thread_message_item(first_thread, "First", "2026-05-07 12:00", vec!["\\Seen"]),
            thread_message_item(
                first_thread,
                "First start",
                "2026-05-07 10:00",
                vec!["\\Seen"],
            ),
            thread_message_item(second_thread, "Second", "2026-05-07 11:00", vec!["\\Seen"]),
        ]);
        let mut client = MockMailbox::default();

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );
        assert_eq!(app.active, ActivePane::Threads);
        assert_eq!(app.selected_thread, 1);

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );
        assert_eq!(app.active, ActivePane::Threads);
        assert_eq!(app.selected_thread, 0);
    }

    #[tokio::test]
    async fn test_handle_key_j_k_move_selection_without_switching_panes() {
        let first_thread = Uuid::new_v4();
        let second_thread = Uuid::new_v4();
        let mut app = AppState {
            active: ActivePane::Threads,
            ..Default::default()
        };
        app.apply_folder_messages(vec![
            thread_message_item(first_thread, "First", "2026-05-07 12:00", vec!["\\Seen"]),
            thread_message_item(
                first_thread,
                "First start",
                "2026-05-07 10:00",
                vec!["\\Seen"],
            ),
            thread_message_item(second_thread, "Second", "2026-05-07 11:00", vec!["\\Seen"]),
        ]);
        let mut client = MockMailbox::default();

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );
        assert_eq!(app.active, ActivePane::Threads);
        assert_eq!(app.selected_thread, 1);

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );
        assert_eq!(app.active, ActivePane::Threads);
        assert_eq!(app.selected_thread, 0);
    }

    #[tokio::test]
    async fn test_handle_key_command_mode_left_right_do_not_switch_panes() {
        let mut app = AppState {
            active: ActivePane::Folders,
            ..Default::default()
        };
        app.enter_command_mode();
        assert!(app.push_command_char('s'));
        let mut client = MockMailbox::default();

        for key in [KeyCode::Left, KeyCode::Right] {
            assert!(
                !handle_key(
                    KeyEvent::new(key, KeyModifiers::NONE),
                    &mut app,
                    &mut client
                )
                .await
            );
            assert_eq!(app.active, ActivePane::Folders);
            assert_eq!(app.command_input, "s");
        }
    }

    #[tokio::test]
    async fn test_handle_key_enter_on_thread_focuses_messages_and_loads_detail() {
        let thread_id = Uuid::new_v4();
        let selected = thread_message_item(thread_id, "Start", "2026-05-07 09:00", vec!["\\Seen"]);
        let reply = thread_message_item(thread_id, "Reply", "2026-05-07 10:00", vec!["\\Seen"]);
        let mut app = AppState {
            active: ActivePane::Threads,
            ..Default::default()
        };
        app.apply_folder_messages(vec![reply, selected.clone()]);
        let mut client = MockMailbox {
            detail: Some(detail_for(&selected)),
            ..Default::default()
        };

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );

        assert_eq!(app.active, ActivePane::Messages);
        assert_eq!(client.calls, vec![Call::GetMessage(selected.id)]);
        assert_eq!(app.detail.as_ref().unwrap().id, selected.id);
    }

    #[tokio::test]
    async fn test_handle_key_command_mode_cancel_does_not_quit_on_q() {
        let mut app = AppState::default();
        let mut client = MockMailbox::default();

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );
        assert_eq!(app.mode, InputMode::Command);
        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );
        assert_eq!(app.command_input, "q");
        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );

        assert_eq!(app.mode, InputMode::Normal);
        assert!(app.command_input.is_empty());
        assert_eq!(app.status, "Command cancelled");
    }
}
