pub mod app;
pub mod command;
pub mod ipc;
pub mod render;
pub mod theme;

use std::io::{self, Stdout};
use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
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

const COMPOSER_BODY_KEY_VIEWPORT_LINES: usize = 3;
const DETAIL_KEY_VIEWPORT_LINES: usize = 6;

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
    async fn list_attachments(
        &mut self,
        message_id: Uuid,
    ) -> Result<Vec<app::AttachmentItem>, ipc::MailboxError>;
    async fn preview_attachment(
        &mut self,
        attachment_id: Uuid,
    ) -> Result<app::AttachmentPreviewItem, ipc::MailboxError>;
    async fn export_attachment(
        &mut self,
        attachment_id: Uuid,
        destination_path: &std::path::Path,
    ) -> Result<ipc::AttachmentExportResult, ipc::MailboxError>;
    async fn create_draft(&mut self, draft: &app::ComposerDraft)
        -> Result<Uuid, ipc::MailboxError>;
    async fn update_draft(
        &mut self,
        draft_id: Uuid,
        draft: &app::ComposerDraft,
    ) -> Result<Uuid, ipc::MailboxError>;
    async fn send_draft(
        &mut self,
        account_id: Uuid,
        draft_id: Uuid,
    ) -> Result<String, ipc::MailboxError>;
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

    async fn list_attachments(
        &mut self,
        message_id: Uuid,
    ) -> Result<Vec<app::AttachmentItem>, ipc::MailboxError> {
        MailboxClient::list_attachments(self, message_id).await
    }

    async fn preview_attachment(
        &mut self,
        attachment_id: Uuid,
    ) -> Result<app::AttachmentPreviewItem, ipc::MailboxError> {
        MailboxClient::preview_attachment(self, attachment_id).await
    }

    async fn export_attachment(
        &mut self,
        attachment_id: Uuid,
        destination_path: &std::path::Path,
    ) -> Result<ipc::AttachmentExportResult, ipc::MailboxError> {
        MailboxClient::export_attachment(self, attachment_id, destination_path).await
    }

    async fn create_draft(
        &mut self,
        draft: &app::ComposerDraft,
    ) -> Result<Uuid, ipc::MailboxError> {
        MailboxClient::create_draft(self, draft).await
    }

    async fn update_draft(
        &mut self,
        draft_id: Uuid,
        draft: &app::ComposerDraft,
    ) -> Result<Uuid, ipc::MailboxError> {
        MailboxClient::update_draft(self, draft_id, draft).await
    }

    async fn send_draft(
        &mut self,
        account_id: Uuid,
        draft_id: Uuid,
    ) -> Result<String, ipc::MailboxError> {
        MailboxClient::send_draft(self, account_id, draft_id).await
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
    if app.pending_open_attachment.is_some() {
        return handle_open_confirmation_key(key, app);
    }

    match app.mode {
        InputMode::Command => return handle_command_key(key, app, client).await,
        InputMode::Compose | InputMode::ConfirmDiscard => {
            return handle_composer_key(key, app, client).await;
        }
        InputMode::Normal => {}
    }

    if app.active == ActivePane::Details && handle_detail_key(key, app) {
        return false;
    }

    match key.code {
        KeyCode::Char(':') => {
            app.enter_command_mode();
            false
        }
        KeyCode::Char('c') => {
            match app.selected_account_id() {
                Some(account_id) => app.enter_composer(account_id),
                None => record_command_run_error(app, CommandRunError::AccountNotSelected),
            }
            false
        }
        KeyCode::Char('q') => true,
        KeyCode::Char('d') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            if app.focus_detail_pane() {
                app.set_status("Details");
            } else {
                app.set_status("No message detail open");
            }
            false
        }
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
        KeyCode::Char('a') => {
            if app.toggle_attachment_focus() {
                app.set_status("Attachments");
            } else {
                app.set_status("No attachments for message");
            }
            false
        }
        KeyCode::Char('e') => {
            export_selected_attachment(app, client).await;
            false
        }
        KeyCode::Char('o') => {
            if app.begin_open_attachment_confirmation() {
                let filename = app
                    .pending_open_attachment
                    .as_ref()
                    .map(|attachment| attachment.filename.clone())
                    .unwrap_or_else(|| "attachment".into());
                app.set_status(format!("Open {filename} with xdg-open? y/n"));
            } else {
                app.set_status("No attachment selected");
            }
            false
        }
        KeyCode::Enter => {
            if app.active == ActivePane::Threads {
                app.active = ActivePane::Messages;
                refresh_detail(app, client).await;
            } else if app.active == ActivePane::Attachments {
                refresh_attachment_preview(app, client).await;
            } else {
                refresh_after_selection_change(app, client).await;
            }
            false
        }
        _ => false,
    }
}

fn handle_detail_key(key: KeyEvent, app: &mut AppState) -> bool {
    match key.code {
        KeyCode::Esc => {
            if app.clear_detail_selection() {
                app.set_status("Detail selection cleared");
            } else {
                app.set_status("Details");
            }
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                app.start_detail_line_selection();
            }
            app.move_detail_line(1, DETAIL_KEY_VIEWPORT_LINES);
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                app.start_detail_line_selection();
            }
            app.move_detail_line(-1, DETAIL_KEY_VIEWPORT_LINES);
            true
        }
        KeyCode::PageDown => {
            app.move_detail_line(
                DETAIL_KEY_VIEWPORT_LINES as isize,
                DETAIL_KEY_VIEWPORT_LINES,
            );
            true
        }
        KeyCode::PageUp => {
            app.move_detail_line(
                -(DETAIL_KEY_VIEWPORT_LINES as isize),
                DETAIL_KEY_VIEWPORT_LINES,
            );
            true
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.move_detail_line(
                DETAIL_KEY_VIEWPORT_LINES as isize,
                DETAIL_KEY_VIEWPORT_LINES,
            );
            true
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.move_detail_line(
                -(DETAIL_KEY_VIEWPORT_LINES as isize),
                DETAIL_KEY_VIEWPORT_LINES,
            );
            true
        }
        KeyCode::Left => {
            app.move_detail_cursor_left();
            true
        }
        KeyCode::Right => {
            app.move_detail_cursor_right();
            true
        }
        KeyCode::Home => {
            app.detail_home();
            true
        }
        KeyCode::End => {
            app.detail_end();
            true
        }
        KeyCode::Char('v') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.toggle_detail_line_selection();
            true
        }
        _ => false,
    }
}

fn handle_open_confirmation_key(key: KeyEvent, app: &mut AppState) -> bool {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            if let Some(attachment) = app.take_pending_open_attachment() {
                open_attachment_with_xdg(app, &attachment);
            }
            false
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
            app.cancel_open_attachment_confirmation();
            app.set_status("Open cancelled");
            false
        }
        _ => false,
    }
}

async fn handle_composer_key<C: Mailbox + ?Sized>(
    key: KeyEvent,
    app: &mut AppState,
    client: &mut C,
) -> bool {
    if app.mode == InputMode::ConfirmDiscard {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => app.discard_composer(),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                app.cancel_discard_composer_confirmation();
            }
            _ => {}
        }
        return false;
    }

    let composer_body_focused = app
        .composer
        .as_ref()
        .is_some_and(|composer| composer.focused == app::ComposeField::Body);

    match key.code {
        KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            save_composer(app, client).await;
            false
        }
        KeyCode::Char('x') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            send_composer(app, client).await;
            false
        }
        KeyCode::Esc => {
            if app.clear_composer_body_selection() {
                app.set_status("Body selection cleared");
            } else if app.composer_needs_discard_confirmation() {
                app.begin_discard_composer_confirmation();
            } else {
                app.exit_composer();
                app.set_status("Composer closed");
            }
            false
        }
        KeyCode::Tab => {
            app.next_composer_field();
            false
        }
        KeyCode::BackTab => {
            app.previous_composer_field();
            false
        }
        KeyCode::Down => {
            if composer_body_focused {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    app.start_composer_body_line_selection();
                }
                app.move_composer_body_line(1, COMPOSER_BODY_KEY_VIEWPORT_LINES);
            } else {
                app.next_composer_field();
            }
            false
        }
        KeyCode::Up => {
            if composer_body_focused {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    app.start_composer_body_line_selection();
                }
                app.move_composer_body_line(-1, COMPOSER_BODY_KEY_VIEWPORT_LINES);
            } else {
                app.previous_composer_field();
            }
            false
        }
        KeyCode::PageDown => {
            if composer_body_focused {
                app.move_composer_body_line(
                    COMPOSER_BODY_KEY_VIEWPORT_LINES as isize,
                    COMPOSER_BODY_KEY_VIEWPORT_LINES,
                );
            }
            false
        }
        KeyCode::PageUp => {
            if composer_body_focused {
                app.move_composer_body_line(
                    -(COMPOSER_BODY_KEY_VIEWPORT_LINES as isize),
                    COMPOSER_BODY_KEY_VIEWPORT_LINES,
                );
            }
            false
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if composer_body_focused {
                app.move_composer_body_line(
                    COMPOSER_BODY_KEY_VIEWPORT_LINES as isize,
                    COMPOSER_BODY_KEY_VIEWPORT_LINES,
                );
            }
            false
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if composer_body_focused {
                app.move_composer_body_line(
                    -(COMPOSER_BODY_KEY_VIEWPORT_LINES as isize),
                    COMPOSER_BODY_KEY_VIEWPORT_LINES,
                );
            }
            false
        }
        KeyCode::Left => {
            app.move_composer_cursor_left();
            false
        }
        KeyCode::Right => {
            app.move_composer_cursor_right();
            false
        }
        KeyCode::Home => {
            app.composer_home();
            false
        }
        KeyCode::End => {
            app.composer_end();
            false
        }
        KeyCode::Enter => {
            if !app.composer_enter() {
                app.set_error(format!(
                    "body is limited to {} characters",
                    app::MAX_COMPOSE_BODY_CHARS
                ));
            }
            false
        }
        KeyCode::Backspace => {
            app.backspace_composer();
            false
        }
        KeyCode::Delete => {
            app.delete_composer();
            false
        }
        KeyCode::Char('v')
            if composer_body_focused && !key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            app.toggle_composer_body_line_selection();
            false
        }
        KeyCode::Char(ch) => {
            if !key.modifiers.contains(KeyModifiers::CONTROL) && !app.push_composer_char(ch) {
                app.set_error("compose field is at its limit");
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

async fn save_composer<C: Mailbox + ?Sized>(app: &mut AppState, client: &mut C) -> Option<Uuid> {
    let Some(draft) = app.composer_draft() else {
        app.set_status("No composer open");
        return None;
    };

    app.clear_error();
    app.set_status("Saving draft");
    let result = if let Some(draft_id) = app.composer_draft_id() {
        client.update_draft(draft_id, &draft).await
    } else {
        client.create_draft(&draft).await
    };

    match result {
        Ok(draft_id) => {
            app.mark_composer_saved(draft_id);
            app.set_status(format!("Draft saved {draft_id}"));
            Some(draft_id)
        }
        Err(error) => {
            record_error(app, error);
            None
        }
    }
}

async fn send_composer<C: Mailbox + ?Sized>(app: &mut AppState, client: &mut C) {
    let Some(account_id) = app.composer_account_id() else {
        app.set_status("No composer open");
        return;
    };

    let draft_id = if app.composer_draft_id().is_none() || app.composer_is_dirty() {
        match save_composer(app, client).await {
            Some(draft_id) => draft_id,
            None => return,
        }
    } else {
        app.composer_draft_id().expect("checked above")
    };

    app.clear_error();
    app.set_status("Sending message");
    match client.send_draft(account_id, draft_id).await {
        Ok(message_id) => {
            app.exit_composer();
            app.set_status(format!("Sent message {message_id}"));
        }
        Err(error) => record_error(app, error),
    }
}

async fn export_selected_attachment<C: Mailbox + ?Sized>(app: &mut AppState, client: &mut C) {
    let Some(attachment) = app.selected_attachment().cloned() else {
        app.set_status("No attachment selected");
        return;
    };
    let destination = match default_export_path(&attachment.filename) {
        Ok(path) => path,
        Err(error) => {
            app.set_error(format!("export path: {error}"));
            return;
        }
    };

    app.clear_error();
    app.set_status(format!("Exporting {}", attachment.filename));
    match client.export_attachment(attachment.id, &destination).await {
        Ok(exported) => app.set_status(format!(
            "Exported attachment to {}",
            exported.destination_path
        )),
        Err(error) => record_error(app, error),
    }
}

fn open_attachment_with_xdg(app: &mut AppState, attachment: &app::AttachmentItem) {
    match std::process::Command::new("xdg-open")
        .arg(&attachment.storage_path)
        .status()
    {
        Ok(status) if status.success() => {
            app.set_status(format!("Opened {} with xdg-open", attachment.filename));
        }
        Ok(status) => {
            app.set_error(format!("xdg-open failed with status {status}"));
        }
        Err(error) => {
            app.set_error(format!("xdg-open failed: {error}"));
        }
    }
}

fn default_export_path(filename: &str) -> std::io::Result<PathBuf> {
    let directory = std::env::current_dir()?;
    let filename = safe_export_filename(filename);
    let first = directory.join(&filename);
    if !first.exists() {
        return Ok(first);
    }

    let path = std::path::Path::new(&filename);
    let stem = path
        .file_stem()
        .and_then(|part| part.to_str())
        .unwrap_or("attachment");
    let extension = path.extension().and_then(|part| part.to_str());
    for index in 1..1000 {
        let candidate = match extension {
            Some(extension) => directory.join(format!("{stem} ({index}).{extension}")),
            None => directory.join(format!("{stem} ({index})")),
        };
        if !candidate.exists() {
            return Ok(candidate);
        }
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "no collision-free export path available",
    ))
}

fn safe_export_filename(filename: &str) -> String {
    let leaf = std::path::Path::new(filename)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("attachment.bin");
    let safe = leaf.trim_matches(['.', ' ']);
    if safe.is_empty() {
        "attachment.bin".into()
    } else {
        safe.to_string()
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
        ActivePane::Details => refresh_detail(app, client).await,
        ActivePane::Attachments => refresh_attachments(app, client).await,
    }
}

async fn refresh_after_selection_change<C: Mailbox + ?Sized>(app: &mut AppState, client: &mut C) {
    match app.active {
        ActivePane::Accounts => refresh_folders(app, client).await,
        ActivePane::Folders => refresh_messages(app, client).await,
        ActivePane::Threads => refresh_detail(app, client).await,
        ActivePane::Messages => refresh_detail(app, client).await,
        ActivePane::Details => refresh_detail(app, client).await,
        ActivePane::Attachments => refresh_attachment_preview(app, client).await,
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
            if app.detail.is_some() {
                refresh_attachments(app, client).await;
            }
        }
        Err(error) => record_error(app, error),
    }
}

async fn refresh_attachments<C: Mailbox + ?Sized>(app: &mut AppState, client: &mut C) {
    let Some(message_id) = app.detail.as_ref().map(|detail| detail.id) else {
        app.apply_attachments(Vec::new());
        return;
    };

    match client.list_attachments(message_id).await {
        Ok(attachments) => {
            let count = attachments.len();
            app.apply_attachments(attachments);
            if count == 0 {
                app.set_status("Message loaded");
            } else {
                refresh_attachment_preview(app, client).await;
                if app.error.is_none() {
                    app.set_status(format!("Message loaded • {count} attachment(s)"));
                }
            }
        }
        Err(error) => record_error(app, error),
    }
}

async fn refresh_attachment_preview<C: Mailbox + ?Sized>(app: &mut AppState, client: &mut C) {
    let Some(attachment_id) = app.selected_attachment_id() else {
        app.attachment_preview = None;
        app.set_status("No attachment selected");
        return;
    };

    match client.preview_attachment(attachment_id).await {
        Ok(preview) => {
            app.clear_error();
            app.apply_attachment_preview(preview);
            app.set_status("Attachment preview loaded");
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
        ListAttachments(Uuid),
        PreviewAttachment(Uuid),
        ExportAttachment(Uuid, PathBuf),
        CreateDraft(app::ComposerDraft),
        UpdateDraft(Uuid, app::ComposerDraft),
        SendDraft(Uuid, Uuid),
    }

    #[derive(Default)]
    struct MockMailbox {
        calls: Vec<Call>,
        messages: Vec<MessageItem>,
        detail: Option<MessageDetail>,
        attachments: Vec<app::AttachmentItem>,
        preview: Option<app::AttachmentPreviewItem>,
        draft_id: Option<Uuid>,
        send_message_id: Option<String>,
        fail_sync: bool,
        fail_set_flags: bool,
        fail_draft: bool,
        fail_send: bool,
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

        async fn list_attachments(
            &mut self,
            message_id: Uuid,
        ) -> Result<Vec<app::AttachmentItem>, ipc::MailboxError> {
            self.calls.push(Call::ListAttachments(message_id));
            Ok(self.attachments.clone())
        }

        async fn preview_attachment(
            &mut self,
            attachment_id: Uuid,
        ) -> Result<app::AttachmentPreviewItem, ipc::MailboxError> {
            self.calls.push(Call::PreviewAttachment(attachment_id));
            Ok(self.preview.clone().unwrap_or(app::AttachmentPreviewItem {
                attachment_id,
                text: None,
                message: "No inline preview".into(),
                truncated: false,
                preview_bytes: 0,
            }))
        }

        async fn export_attachment(
            &mut self,
            attachment_id: Uuid,
            destination_path: &std::path::Path,
        ) -> Result<ipc::AttachmentExportResult, ipc::MailboxError> {
            self.calls.push(Call::ExportAttachment(
                attachment_id,
                destination_path.into(),
            ));
            Ok(ipc::AttachmentExportResult {
                attachment_id,
                destination_path: destination_path.display().to_string(),
                bytes_copied: 12,
            })
        }

        async fn create_draft(
            &mut self,
            draft: &app::ComposerDraft,
        ) -> Result<uuid::Uuid, ipc::MailboxError> {
            self.calls.push(Call::CreateDraft(draft.clone()));
            if self.fail_draft {
                Err(server_error("draft.create"))
            } else {
                Ok(self.draft_id.unwrap_or_else(Uuid::new_v4))
            }
        }

        async fn update_draft(
            &mut self,
            draft_id: Uuid,
            draft: &app::ComposerDraft,
        ) -> Result<uuid::Uuid, ipc::MailboxError> {
            self.calls.push(Call::UpdateDraft(draft_id, draft.clone()));
            if self.fail_draft {
                Err(server_error("draft.update"))
            } else {
                Ok(draft_id)
            }
        }

        async fn send_draft(
            &mut self,
            account_id: Uuid,
            draft_id: Uuid,
        ) -> Result<String, ipc::MailboxError> {
            self.calls.push(Call::SendDraft(account_id, draft_id));
            if self.fail_send {
                Err(server_error("message.send"))
            } else {
                Ok(self
                    .send_message_id
                    .clone()
                    .unwrap_or_else(|| "<sent@postblox.local>".into()))
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

    fn detail_with_body(message_id: Uuid, body: &str) -> MessageDetail {
        MessageDetail {
            id: message_id,
            subject: "Hello".into(),
            from: "alice@example.com".into(),
            snippet: "Preview".into(),
            body: body.into(),
            flags: Vec::new(),
        }
    }

    fn app_with_account_folder(account_id: Uuid, folder_id: Uuid) -> AppState {
        let mut app = AppState::default();
        app.apply_accounts(vec![account_item(account_id)]);
        app.apply_folders(vec![folder_item(folder_id)]);
        app
    }

    fn attachment_item(id: Uuid, message_id: Uuid) -> app::AttachmentItem {
        app::AttachmentItem {
            id,
            message_id,
            filename: "notes.txt".into(),
            content_type: "text/plain".into(),
            size_bytes: 12,
            disposition: "attachment".into(),
            storage_path: "/tmp/notes.txt".into(),
        }
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
            vec![
                Call::ListMessages(folder_id),
                Call::GetMessage(older.id),
                Call::ListAttachments(older.id),
            ]
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
            vec![
                Call::ListMessages(folder_id),
                Call::GetMessage(first.id),
                Call::ListAttachments(first.id),
            ]
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
    async fn test_handle_key_details_shortcut_requires_loaded_detail() {
        let mut app = AppState {
            active: ActivePane::Messages,
            ..Default::default()
        };
        let mut client = MockMailbox::default();

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );
        assert_eq!(app.active, ActivePane::Messages);
        assert_eq!(app.status, "No message detail open");

        app.apply_detail(Some(detail_with_body(Uuid::new_v4(), "body")));

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );
        assert_eq!(app.active, ActivePane::Details);
        assert_eq!(app.status, "Details");
    }

    #[tokio::test]
    async fn test_handle_key_details_navigation_selection_and_escape() {
        let body = (1..=12)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = AppState {
            active: ActivePane::Details,
            ..Default::default()
        };
        app.apply_detail(Some(detail_with_body(Uuid::new_v4(), &body)));
        let mut client = MockMailbox::default();

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );
        assert_eq!(app.detail_cursor_line_column(), (1, 0));

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );
        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );
        assert_eq!(app.active, ActivePane::Details);
        assert_eq!(app.detail_cursor_line_column(), (1, 2));

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );
        assert_eq!(app.active, ActivePane::Details);
        assert_eq!(app.detail_cursor_line_column(), (1, 1));

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Home, KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );
        assert_eq!(app.detail_cursor_line_column(), (1, 0));

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );
        assert_eq!(
            app.detail_cursor_line_column().0,
            1 + DETAIL_KEY_VIEWPORT_LINES
        );

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
                &mut app,
                &mut client,
            )
            .await
        );
        assert_eq!(app.detail_cursor_line_column(), (1, 0));

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );
        assert_eq!(app.detail_selected_line_range(), Some(1..=1));

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT),
                &mut app,
                &mut client,
            )
            .await
        );
        assert_eq!(app.detail_selected_line_range(), Some(1..=2));

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );
        assert_eq!(app.detail_selected_line_range(), None);
        assert_eq!(app.active, ActivePane::Details);

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
                &mut app,
                &mut client,
            )
            .await
        );
        assert_eq!(
            app.detail_cursor_line_column().0,
            2 + DETAIL_KEY_VIEWPORT_LINES
        );
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
        assert_eq!(
            client.calls,
            vec![
                Call::GetMessage(selected.id),
                Call::ListAttachments(selected.id),
            ]
        );
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

    #[tokio::test]
    async fn test_refresh_detail_loads_attachments_and_first_preview() {
        let message_id = Uuid::new_v4();
        let attachment_id = Uuid::new_v4();
        let selected = message_item(message_id, vec![]);
        let mut app = AppState::default();
        app.apply_messages(vec![selected.clone()]);
        let mut client = MockMailbox {
            detail: Some(detail_for(&selected)),
            attachments: vec![attachment_item(attachment_id, message_id)],
            preview: Some(app::AttachmentPreviewItem {
                attachment_id,
                text: Some("preview text".into()),
                message: "Inline preview".into(),
                truncated: false,
                preview_bytes: 12,
            }),
            ..Default::default()
        };

        refresh_detail(&mut app, &mut client).await;

        assert_eq!(
            client.calls,
            vec![
                Call::GetMessage(message_id),
                Call::ListAttachments(message_id),
                Call::PreviewAttachment(attachment_id),
            ]
        );
        assert_eq!(app.attachments.len(), 1);
        assert_eq!(
            app.attachment_preview
                .as_ref()
                .and_then(|p| p.text.as_deref()),
            Some("preview text")
        );
    }

    #[tokio::test]
    async fn test_handle_key_attachment_focus_and_selection_refresh_preview() {
        let message_id = Uuid::new_v4();
        let first_id = Uuid::new_v4();
        let second_id = Uuid::new_v4();
        let mut app = AppState::default();
        app.apply_detail(Some(MessageDetail {
            id: message_id,
            subject: "hello".into(),
            from: "alice@example.com".into(),
            snippet: "hello".into(),
            body: "body".into(),
            flags: Vec::new(),
        }));
        app.apply_attachments(vec![
            attachment_item(first_id, message_id),
            attachment_item(second_id, message_id),
        ]);
        let mut client = MockMailbox {
            preview: Some(app::AttachmentPreviewItem {
                attachment_id: second_id,
                text: Some("second preview".into()),
                message: "Inline preview".into(),
                truncated: false,
                preview_bytes: 14,
            }),
            ..Default::default()
        };

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );
        assert_eq!(app.active, ActivePane::Attachments);
        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );

        assert_eq!(app.selected_attachment_id(), Some(second_id));
        assert_eq!(client.calls, vec![Call::PreviewAttachment(second_id)]);
        assert_eq!(
            app.attachment_preview
                .as_ref()
                .and_then(|p| p.text.as_deref()),
            Some("second preview")
        );
    }

    #[tokio::test]
    async fn test_handle_key_composer_ctrl_s_creates_then_updates_draft() {
        let account_id = Uuid::new_v4();
        let draft_id = Uuid::new_v4();
        let mut app = AppState::default();
        app.apply_accounts(vec![account_item(account_id)]);
        let mut client = MockMailbox {
            draft_id: Some(draft_id),
            ..Default::default()
        };

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );
        for ch in "to@example.com".chars() {
            assert!(
                !handle_key(
                    KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
                    &mut app,
                    &mut client,
                )
                .await
            );
        }
        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
                &mut app,
                &mut client,
            )
            .await
        );
        assert_eq!(app.composer.as_ref().unwrap().draft_id, Some(draft_id));
        assert!(!app.composer.as_ref().unwrap().dirty);

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );
        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
                &mut app,
                &mut client,
            )
            .await
        );

        assert_eq!(client.calls.len(), 2);
        assert!(matches!(client.calls[0], Call::CreateDraft(_)));
        assert!(matches!(client.calls[1], Call::UpdateDraft(id, _) if id == draft_id));
    }

    #[tokio::test]
    async fn test_handle_key_composer_arrows_insert_and_delete_at_cursor() {
        let account_id = Uuid::new_v4();
        let mut app = AppState::default();
        app.enter_composer(account_id);
        let mut client = MockMailbox::default();

        for ch in "abc".chars() {
            assert!(
                !handle_key(
                    KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
                    &mut app,
                    &mut client,
                )
                .await
            );
        }
        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );
        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );
        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Char('X'), KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );
        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );
        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::End, KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );
        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );

        let composer = app.composer.as_ref().unwrap();
        assert_eq!(composer.to, "aX");
        assert_eq!(composer.to_cursor, 2);
    }

    #[tokio::test]
    async fn test_handle_key_composer_body_page_selection_and_escape() {
        let account_id = Uuid::new_v4();
        let mut app = AppState::default();
        app.enter_composer(account_id);
        let composer = app.composer.as_mut().unwrap();
        composer.focused = app::ComposeField::Body;
        composer.body = "one\ntwo\nthree\nfour\nfive".into();
        composer.dirty = true;
        let mut client = MockMailbox::default();

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );
        assert_eq!(
            app.composer.as_ref().unwrap().body_cursor_line_column().0,
            3
        );

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );
        assert_eq!(
            app.composer.as_ref().unwrap().body_selected_line_range(),
            Some(3..=3)
        );

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT),
                &mut app,
                &mut client,
            )
            .await
        );
        assert_eq!(
            app.composer.as_ref().unwrap().body_selected_line_range(),
            Some(2..=3)
        );

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );
        assert_eq!(
            app.composer.as_ref().unwrap().body_selected_line_range(),
            None
        );
        assert_eq!(app.mode, InputMode::Compose);

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );
        assert_eq!(app.mode, InputMode::ConfirmDiscard);
    }

    #[tokio::test]
    async fn test_handle_key_composer_ctrl_x_saves_sends_and_exits() {
        let account_id = Uuid::new_v4();
        let draft_id = Uuid::new_v4();
        let mut app = AppState::default();
        app.apply_accounts(vec![account_item(account_id)]);
        let mut client = MockMailbox {
            draft_id: Some(draft_id),
            send_message_id: Some("<sent-1@postblox.local>".into()),
            ..Default::default()
        };

        handle_key(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE),
            &mut app,
            &mut client,
        )
        .await;
        for ch in "to@example.com".chars() {
            handle_key(
                KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await;
        }

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
                &mut app,
                &mut client,
            )
            .await
        );

        assert_eq!(
            client.calls,
            vec![
                Call::CreateDraft(app::ComposerDraft {
                    account_id,
                    in_reply_to_msg: None,
                    to_addrs: vec!["to@example.com".into()],
                    cc_addrs: vec![],
                    bcc_addrs: vec![],
                    subject: None,
                    text_body: None,
                    html_body: None,
                }),
                Call::SendDraft(account_id, draft_id),
            ]
        );
        assert_eq!(app.mode, InputMode::Normal);
        assert!(app.composer.is_none());
        assert!(app.status.contains("<sent-1@postblox.local>"));
    }
}
