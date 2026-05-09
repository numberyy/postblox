pub mod app;
pub mod command;
pub mod ipc;
pub mod render;
pub mod theme;

use std::io::{self, Stdout};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use thiserror::Error;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::ipc::Topic;
use app::{ActivePane, AppState, InputMode, SyncStateUi, FLAGGED_FLAG, SEEN_FLAG};
use command::{parse_command, Command};
use ipc::MailboxClient;
use theme::ThemeName;

/// Bounded mailbox depth for the key-event reader thread. Large enough
/// to ride out a redraw or a brief async stall without dropping
/// keystrokes; small enough that we can't grow unboundedly.
const KEY_EVENT_CHANNEL_CAPACITY: usize = 64;
/// Tick cadence used to expire toasts.
const TICK_INTERVAL: Duration = Duration::from_millis(250);
/// Bound for the blocking event poll inside the reader thread.
const KEY_POLL_TIMEOUT: Duration = Duration::from_millis(100);

const COMPOSER_BODY_KEY_VIEWPORT_LINES: usize = 3;
const DETAIL_KEY_VIEWPORT_LINES: usize = 6;
/// Lines of preview shown in the right column. Matches the single-pane
/// height we render today; used by `j/k`/Page keys when there is no
/// frame around to measure.
const PREVIEW_KEY_VIEWPORT_LINES: usize = 6;

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
    async fn archive_message(&mut self, message_id: Uuid) -> Result<(), ipc::MailboxError>;
    async fn delete_message(&mut self, message_id: Uuid) -> Result<(), ipc::MailboxError>;
    async fn move_message(
        &mut self,
        message_id: Uuid,
        folder_name: &str,
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
    async fn list_drafts(
        &mut self,
        account_id: Uuid,
    ) -> Result<Vec<app::DraftItem>, ipc::MailboxError>;
    async fn get_draft(
        &mut self,
        draft_id: Uuid,
    ) -> Result<Option<app::DraftSummary>, ipc::MailboxError>;
    async fn delete_draft(&mut self, draft_id: Uuid) -> Result<(), ipc::MailboxError>;
    async fn search(
        &mut self,
        query: &str,
        account_id: Option<Uuid>,
    ) -> Result<Vec<app::SearchHit>, ipc::MailboxError>;
    async fn prepare_reply(
        &mut self,
        message_id: Uuid,
        reply_all: bool,
    ) -> Result<ipc::ReplyPrepared, ipc::MailboxError>;
    async fn prepare_forward(
        &mut self,
        message_id: Uuid,
    ) -> Result<ipc::ForwardPrepared, ipc::MailboxError>;
    async fn fetch_attachment_for_forward(
        &mut self,
        attachment_id: Uuid,
    ) -> Result<ipc::ForwardAttachmentBytes, ipc::MailboxError>;
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

    async fn archive_message(&mut self, message_id: Uuid) -> Result<(), ipc::MailboxError> {
        MailboxClient::archive_message(self, message_id).await
    }

    async fn delete_message(&mut self, message_id: Uuid) -> Result<(), ipc::MailboxError> {
        MailboxClient::delete_message(self, message_id).await
    }

    async fn move_message(
        &mut self,
        message_id: Uuid,
        folder_name: &str,
    ) -> Result<(), ipc::MailboxError> {
        MailboxClient::move_message(self, message_id, folder_name).await
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

    async fn list_drafts(
        &mut self,
        account_id: Uuid,
    ) -> Result<Vec<app::DraftItem>, ipc::MailboxError> {
        MailboxClient::list_drafts(self, account_id).await
    }

    async fn get_draft(
        &mut self,
        draft_id: Uuid,
    ) -> Result<Option<app::DraftSummary>, ipc::MailboxError> {
        MailboxClient::get_draft(self, draft_id).await
    }

    async fn delete_draft(&mut self, draft_id: Uuid) -> Result<(), ipc::MailboxError> {
        MailboxClient::delete_draft(self, draft_id).await
    }

    async fn search(
        &mut self,
        query: &str,
        account_id: Option<Uuid>,
    ) -> Result<Vec<app::SearchHit>, ipc::MailboxError> {
        MailboxClient::search(self, query, account_id).await
    }

    async fn prepare_reply(
        &mut self,
        message_id: Uuid,
        reply_all: bool,
    ) -> Result<ipc::ReplyPrepared, ipc::MailboxError> {
        MailboxClient::prepare_reply(self, message_id, reply_all).await
    }

    async fn prepare_forward(
        &mut self,
        message_id: Uuid,
    ) -> Result<ipc::ForwardPrepared, ipc::MailboxError> {
        MailboxClient::prepare_forward(self, message_id).await
    }

    async fn fetch_attachment_for_forward(
        &mut self,
        attachment_id: Uuid,
    ) -> Result<ipc::ForwardAttachmentBytes, ipc::MailboxError> {
        MailboxClient::fetch_attachment_for_forward(self, attachment_id).await
    }
}

pub async fn run(socket_path: PathBuf) -> Result<(), TuiError> {
    run_with_theme(socket_path, None).await
}

/// Same as [`run`], but lets the caller pre-select the initial theme
/// (e.g. from `postblox.toml [tui] theme = "..."`). `None` keeps the
/// type-default.
pub async fn run_with_theme(
    socket_path: PathBuf,
    initial_theme: Option<ThemeName>,
) -> Result<(), TuiError> {
    let mut client = MailboxClient::connect(&socket_path)
        .await
        .map_err(|source| TuiError::Connect {
            path: socket_path.clone(),
            source,
        })?;
    for topic in [Topic::MailNew, Topic::AccountSynced, Topic::SyncState] {
        if let Err(error) = client.subscribe(topic).await {
            tracing::warn!(error = %error, topic = ?topic, "tui subscribe failed");
        }
    }
    let mut app = AppState::default();
    if let Some(theme) = initial_theme {
        app.set_theme(theme);
    }
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

/// Spawn a blocking thread that polls crossterm key events and forwards
/// them on an mpsc. Returns a join handle plus the receiver.
fn spawn_key_reader() -> (
    tokio::task::JoinHandle<()>,
    mpsc::Receiver<KeyEvent>,
    std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    let (tx, rx) = mpsc::channel::<KeyEvent>(KEY_EVENT_CHANNEL_CAPACITY);
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_for_task = stop.clone();
    let handle = tokio::task::spawn_blocking(move || loop {
        if stop_for_task.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        match event::poll(KEY_POLL_TIMEOUT) {
            Ok(true) => match event::read() {
                Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                    if tx.blocking_send(key).is_err() {
                        return;
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(error = %error, "crossterm event::read failed");
                    return;
                }
            },
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(error = %error, "crossterm event::poll failed");
                return;
            }
        }
    });
    (handle, rx, stop)
}

async fn run_loop(
    terminal: &mut CrosstermTerminal,
    mut app: AppState,
    mut client: MailboxClient,
) -> Result<(), TuiError> {
    let (key_handle, mut keys, key_stop) = spawn_key_reader();
    let mut tick = tokio::time::interval(TICK_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        terminal.draw(|frame| render::render(frame, &app))?;
        let quit = tokio::select! {
            biased;
            maybe_key = keys.recv() => match maybe_key {
                Some(key) => handle_key(key, &mut app, &mut client).await,
                None => true,
            },
            event = client.next_event() => {
                match event {
                    Ok(event) => {
                        on_daemon_event(&mut app, &event);
                        false
                    }
                    Err(error) => {
                        tracing::warn!(error = %error, "tui event stream closed");
                        true
                    }
                }
            }
            _ = tick.tick() => {
                app.tick_toasts(Instant::now());
                false
            }
        };
        if quit {
            break;
        }
    }

    key_stop.store(true, std::sync::atomic::Ordering::Relaxed);
    key_handle.abort();
    Ok(())
}

pub fn on_daemon_event(app: &mut AppState, event: &crate::ipc::Event) {
    let now = Instant::now();
    match event.topic.as_str() {
        "mail.new" => {
            if let Some(account_id) = event.data.get("account_id").and_then(parse_uuid_value) {
                let folder_id = event.data.get("folder_id").and_then(parse_uuid_value);
                app.push_mail_new_toast(account_id, folder_id, now);
            }
        }
        "account.synced" => {
            if let Some(account_id) = event.data.get("account_id").and_then(parse_uuid_value) {
                app.push_account_synced_toast(account_id, now);
            }
        }
        "sync.state" => {
            let Some(account_id) = event.data.get("account_id").and_then(parse_uuid_value) else {
                return;
            };
            let state = event.data.get("state").and_then(|v| v.as_str());
            let Some(state) = state.and_then(parse_sync_state_str) else {
                return;
            };
            let last_error = event
                .data
                .get("last_error")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            app.apply_sync_state(account_id, state, last_error, now);
        }
        _ => {}
    }
}

fn parse_uuid_value(value: &serde_json::Value) -> Option<Uuid> {
    value.as_str().and_then(|s| Uuid::parse_str(s).ok())
}

fn parse_sync_state_str(state: &str) -> Option<SyncStateUi> {
    match state {
        "idle" => Some(SyncStateUi::Idle),
        "polling" => Some(SyncStateUi::Polling),
        "syncing" => Some(SyncStateUi::Syncing),
        "error" => Some(SyncStateUi::Error),
        _ => None,
    }
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
        InputMode::ComposeAttachPath => {
            handle_compose_attach_key(key, app).await;
            return false;
        }
        InputMode::ConfirmDelete => {
            return handle_delete_confirmation_key(key, app, client).await;
        }
        InputMode::QuickSearch => return handle_quick_search_key(key, app, client).await,
        InputMode::Normal => {}
    }

    if app.active == ActivePane::Search && handle_search_pane_key(key, app, client).await {
        return false;
    }

    if app.active == ActivePane::Details && handle_detail_key(key, app) {
        return false;
    }

    if app.is_preview_focus_active() {
        let mut clipboard = SystemClipboard;
        if handle_preview_focus_key(key, app, &mut clipboard) {
            return false;
        }
    }

    match key.code {
        KeyCode::Char(':') => {
            app.enter_command_mode();
            false
        }
        KeyCode::Char('/') => {
            app.enter_quick_search();
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
        KeyCode::Char('x') => {
            app.dismiss_newest_toast();
            false
        }
        KeyCode::Char('X') => {
            app.clear_toasts();
            false
        }
        KeyCode::Char('d') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            if drafts_list_focused(app) {
                if let Some(draft_id) = app.selected_draft_id() {
                    app.begin_draft_delete(draft_id);
                    app.set_status("Delete draft? y/n");
                } else {
                    app.set_status("No draft selected");
                }
            } else if message_list_focused(app) && app.selected_message_id().is_some() {
                begin_message_delete(app);
            } else if app.focus_detail_pane() {
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
            if message_list_focused(app) {
                execute_command(Command::Archive, app, client).await;
            } else {
                export_selected_attachment(app, client).await;
            }
            false
        }
        KeyCode::Char('m') => {
            if message_list_focused(app) {
                app.enter_command_mode();
                for ch in "move ".chars() {
                    app.push_command_char(ch);
                }
                app.set_status("Command mode");
            }
            false
        }
        KeyCode::Char('*') => {
            if message_list_focused(app) {
                let command = if app.selected_message_has_flag(FLAGGED_FLAG).unwrap_or(false) {
                    Command::Unflag
                } else {
                    Command::Flag
                };
                execute_command(command, app, client).await;
            }
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
        KeyCode::Char('R') => {
            run_reply(app, client, false).await;
            false
        }
        KeyCode::Char('A') => {
            run_reply(app, client, true).await;
            false
        }
        KeyCode::Char('F') => {
            run_forward(app, client).await;
            false
        }
        KeyCode::Enter => {
            if drafts_list_focused(app) && app.selected_draft_id().is_some() {
                open_selected_draft(app, client).await;
            } else if app.active == ActivePane::Threads {
                app.active = ActivePane::Messages;
                refresh_detail(app, client).await;
            } else if app.active == ActivePane::Attachments {
                if app.attachment_preview.is_some() && !app.preview_focused {
                    app.focus_preview();
                    app.set_status("Preview: j/k scroll  v select  y copy  Esc cancel");
                } else {
                    refresh_attachment_preview(app, client).await;
                    if app.attachment_preview.is_some() {
                        app.focus_preview();
                        app.set_status("Preview: j/k scroll  v select  y copy  Esc cancel");
                    }
                }
            } else {
                refresh_after_selection_change(app, client).await;
            }
            false
        }
        _ => false,
    }
}

async fn run_reply<C: Mailbox + ?Sized>(app: &mut AppState, client: &mut C, reply_all: bool) {
    let Some(message_id) = app.selected_message_id() else {
        let label = if reply_all { "reply-all" } else { "reply" };
        app.set_status(format!("{label}: no message selected"));
        return;
    };
    let prepared = match client.prepare_reply(message_id, reply_all).await {
        Ok(prepared) => prepared,
        Err(error) => {
            let message = error.to_string();
            app.push_toast(app::ToastKind::Error, message.clone(), Instant::now());
            app.set_error(message.clone());
            app.set_status(message);
            return;
        }
    };
    let prefill = app::ComposerPrefill {
        in_reply_to_msg: Some(prepared.message_id),
        to_addrs: prepared.to,
        cc_addrs: prepared.cc,
        bcc_addrs: Vec::new(),
        subject: Some(prepared.subject),
        body: non_empty_string(&prepared.quoted_body),
        in_reply_to: non_empty_string(&prepared.in_reply_to),
        references_header: non_empty_string(&prepared.references),
        attachments: Vec::new(),
    };
    app.enter_composer_with_prefill(prepared.account_id, prefill);
    let label = if reply_all { "Reply-all" } else { "Reply" };
    app.set_status(label);
}

async fn run_forward<C: Mailbox + ?Sized>(app: &mut AppState, client: &mut C) {
    let Some(message_id) = app.selected_message_id() else {
        app.set_status("forward: no message selected");
        return;
    };
    let prepared = match client.prepare_forward(message_id).await {
        Ok(prepared) => prepared,
        Err(error) => {
            let message = error.to_string();
            app.push_toast(app::ToastKind::Error, message.clone(), Instant::now());
            app.set_error(message.clone());
            app.set_status(message);
            return;
        }
    };
    let mut attachments: Vec<app::ComposerAttachment> =
        Vec::with_capacity(prepared.forwarded_attachments.len());
    let mut failed_attachments: Vec<String> = Vec::new();
    for meta in &prepared.forwarded_attachments {
        match client
            .fetch_attachment_for_forward(meta.attachment_id)
            .await
        {
            Ok(bytes) => match materialise_forward_attachment(&bytes).await {
                Ok(attachment) => attachments.push(attachment),
                Err(_) => failed_attachments.push(meta.filename.clone()),
            },
            Err(_) => failed_attachments.push(meta.filename.clone()),
        }
    }
    if !failed_attachments.is_empty() {
        let message = format!("Could not carry forward: {}", failed_attachments.join(", "));
        app.push_toast(app::ToastKind::Error, message.clone(), Instant::now());
    }
    let prefill = app::ComposerPrefill {
        in_reply_to_msg: None,
        to_addrs: Vec::new(),
        cc_addrs: Vec::new(),
        bcc_addrs: Vec::new(),
        subject: Some(prepared.subject),
        body: non_empty_string(&prepared.forwarded_body),
        in_reply_to: None,
        references_header: None,
        attachments,
    };
    app.enter_composer_with_prefill(prepared.account_id, prefill);
    app.set_status("Forward");
}

/// Decode the bytes returned by `attachment.fetch_for_forward` and
/// stash them in a temp file the composer can attach. The composer
/// API only takes file paths, so we materialise the bytes once.
async fn materialise_forward_attachment(
    bytes: &ipc::ForwardAttachmentBytes,
) -> Result<app::ComposerAttachment, std::io::Error> {
    let decoded = bytes
        .decoded_bytes()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    let dir = std::env::temp_dir().join("postblox-forward");
    tokio::fs::create_dir_all(&dir).await?;
    let unique = format!("{}-{}", Uuid::new_v4().simple(), bytes.filename);
    let path = dir.join(unique);
    tokio::fs::write(&path, &decoded).await?;
    Ok(app::ComposerAttachment {
        path,
        filename: bytes.filename.clone(),
        size_bytes: decoded.len() as u64,
        content_type: bytes.content_type.clone(),
    })
}

/// Same temp-file dance as `materialise_forward_attachment` but for
/// a draft attachment fetched via `draft.get`. Used when re-opening a
/// saved draft into the composer.
async fn materialise_draft_attachment(
    bytes: &app::DraftAttachmentBytes,
) -> Result<app::ComposerAttachment, std::io::Error> {
    let dir = std::env::temp_dir().join("postblox-drafts");
    tokio::fs::create_dir_all(&dir).await?;
    let unique = format!("{}-{}", Uuid::new_v4().simple(), bytes.filename);
    let path = dir.join(unique);
    tokio::fs::write(&path, &bytes.bytes).await?;
    Ok(app::ComposerAttachment {
        path,
        filename: bytes.filename.clone(),
        size_bytes: bytes.bytes.len() as u64,
        content_type: bytes.content_type.clone(),
    })
}

/// Build a `ComposerDraft` from a `DraftSummary`. The address arrays
/// are decoded from the JSON columns; attachments are written to temp
/// files so the existing `draft.update` flow can re-upload them.
async fn composer_draft_from_summary(
    summary: &app::DraftSummary,
) -> Result<app::ComposerDraft, std::io::Error> {
    let mut attachments = Vec::with_capacity(summary.attachments.len());
    for att in &summary.attachments {
        attachments.push(materialise_draft_attachment(att).await?);
    }
    Ok(app::ComposerDraft {
        account_id: summary.draft.account_id,
        in_reply_to_msg: summary.draft.in_reply_to_msg,
        to_addrs: addr_array_to_strings(&summary.draft.to_addrs),
        cc_addrs: addr_array_to_strings(&summary.draft.cc_addrs),
        bcc_addrs: addr_array_to_strings(&summary.draft.bcc_addrs),
        subject: summary.draft.subject.clone(),
        text_body: summary.draft.text_body.clone(),
        html_body: summary.draft.html_body.clone(),
        attachments,
        in_reply_to: summary.draft.in_reply_to.clone(),
        references_header: summary.draft.references_header.clone(),
    })
}

fn addr_array_to_strings(value: &serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn non_empty_string(value: &str) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value.to_string())
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

/// Clipboard sink used by the preview yank handler. Pulled behind a
/// trait so tests can assert what gets copied without spawning a real
/// system clipboard (Wayland-less CI etc.).
pub(crate) trait ClipboardSink {
    fn copy(&mut self, text: &str) -> Result<(), String>;
}

/// Production sink: hands off to `arboard`. Failures (no display
/// server, Wayland sandbox, etc.) bubble up as `Err(message)` so the
/// caller can surface a toast.
struct SystemClipboard;

impl ClipboardSink for SystemClipboard {
    fn copy(&mut self, text: &str) -> Result<(), String> {
        let mut clipboard = arboard::Clipboard::new().map_err(|err| err.to_string())?;
        clipboard.set_text(text).map_err(|err| err.to_string())
    }
}

pub(crate) fn handle_preview_focus_key<S: ClipboardSink>(
    key: KeyEvent,
    app: &mut AppState,
    clipboard: &mut S,
) -> bool {
    let viewport = PREVIEW_KEY_VIEWPORT_LINES;
    let half_page = (viewport / 2).max(1) as isize;
    match key.code {
        KeyCode::Esc => {
            if app.clear_preview_selection() {
                app.set_status("Preview selection cleared");
            } else if app.defocus_preview() {
                app.set_status("Attachments");
            }
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.move_preview_line(1);
            app.scroll_preview(1, viewport);
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.move_preview_line(-1);
            app.scroll_preview(-1, viewport);
            true
        }
        KeyCode::PageDown => {
            app.scroll_preview(half_page, viewport);
            true
        }
        KeyCode::PageUp => {
            app.scroll_preview(-half_page, viewport);
            true
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.scroll_preview(half_page, viewport);
            true
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.scroll_preview(-half_page, viewport);
            true
        }
        KeyCode::Char('g') => {
            app.scroll_preview_to_top();
            true
        }
        KeyCode::Char('G') => {
            app.scroll_preview_to_bottom(viewport);
            true
        }
        KeyCode::Char('v') => {
            app.toggle_preview_selection();
            true
        }
        KeyCode::Char('y') => {
            yank_preview(app, clipboard);
            true
        }
        _ => false,
    }
}

fn yank_preview<S: ClipboardSink>(app: &mut AppState, clipboard: &mut S) {
    let Some(text) = app.preview_yank_text() else {
        app.set_status("No preview selection");
        return;
    };
    if text.is_empty() {
        app.set_status("Selection is empty");
        return;
    }
    let line_count = text.matches('\n').count() + 1;
    match clipboard.copy(&text) {
        Ok(()) => {
            let plural = if line_count == 1 { "" } else { "s" };
            app.set_status(format!("{line_count} line{plural} copied"));
        }
        Err(err) => {
            app.set_error(format!("Clipboard error: {err}"));
        }
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
        // Outside the body, `:` opens the command bar so users can run
        // `:w` to save without learning the Ctrl-S chord. Inside the
        // body, `:` types a literal colon (matches existing behaviour).
        KeyCode::Char(':') if !composer_body_focused => {
            app.enter_command_mode();
            false
        }
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if app.begin_compose_attach() {
                app.set_status("Attach: type a path, Enter to add, Esc to cancel");
            }
            false
        }
        KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            // Ctrl-K removes the selected compose attachment. The
            // composer body already binds Ctrl-D for half-page-down,
            // so we use a different chord here to avoid clobbering it.
            match app.remove_selected_compose_attachment() {
                Some(name) => app.set_status(format!("Removed attachment: {name}")),
                None => app.set_status("No attachment to remove"),
            }
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

async fn handle_compose_attach_key(key: KeyEvent, app: &mut AppState) {
    match key.code {
        KeyCode::Esc => {
            app.cancel_compose_attach();
            app.set_status("Compose");
        }
        KeyCode::Enter => match app.confirm_compose_attach().await {
            Ok(name) => {
                app.set_status(format!("Attached {name}"));
            }
            Err(err) => {
                let text = err.toast_text();
                app.push_toast(app::ToastKind::Error, text.clone(), Instant::now());
                app.set_error(text);
                app.cancel_compose_attach();
            }
        },
        KeyCode::Backspace => {
            app.backspace_compose_attach();
        }
        KeyCode::Char(ch) => {
            if !key.modifiers.contains(KeyModifiers::CONTROL) {
                app.push_compose_attach_char(ch);
            }
        }
        _ => {}
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
        KeyCode::Tab => {
            handle_command_tab(app);
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

/// Tab-complete the current `:` input.
///
/// - Unique match: replace the prefix with the full command name and
///   append a trailing space so the user can type args.
/// - Multiple matches: extend the prefix to the longest common prefix
///   if it adds at least one character, then surface the candidate
///   list as a status hint so the user can disambiguate.
/// - No match / past the command name: no-op.
fn handle_command_tab(app: &mut AppState) {
    let Some(completion) = command::complete_command(&app.command_input) else {
        return;
    };
    if completion.unique {
        app.command_input.clear();
        for ch in completion.text.chars() {
            if !app.push_command_char(ch) {
                break;
            }
        }
        let _ = app.push_command_char(' ');
        app.set_status(format!("Command: {}", completion.text));
        return;
    }
    if completion.text.len() > app.command_input.len() {
        app.command_input = completion.text.clone();
    }
    app.set_status(format!("Matches: {}", completion.matches.join(" ")));
}

async fn run_command_line<C: Mailbox + ?Sized>(input: String, app: &mut AppState, client: &mut C) {
    if input.trim().is_empty() {
        // Empty / whitespace-only input is a silent no-op so an
        // accidental `:Enter` doesn't produce error noise.
        return;
    }
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
        Command::Archive => run_message_remove(app, client, MessageRemove::Archive).await,
        Command::Delete => {
            if let Some(message_id) = app.selected_message_id() {
                app.begin_delete_confirmation(message_id);
            } else {
                record_command_run_error(app, CommandRunError::MessageMissing);
            }
        }
        Command::Move(folder) => run_message_move(app, client, &folder).await,
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
        Command::Compose => match app.selected_account_id() {
            Some(account_id) => {
                app.enter_composer(account_id);
            }
            None => record_command_run_error(app, CommandRunError::AccountNotSelected),
        },
        Command::Reply => run_reply(app, client, false).await,
        Command::ReplyAll => run_reply(app, client, true).await,
        Command::Forward => run_forward(app, client).await,
        Command::Goto(folder) => run_goto_folder(app, client, &folder).await,
        Command::Account(name) => run_select_account(app, client, &name).await,
        Command::Search { account, query } => {
            run_search(app, client, account, query).await;
        }
        Command::Write => {
            if app.composer.is_some() {
                save_composer(app, client).await;
            } else {
                app.set_status(":w only valid while composing");
            }
        }
    }
}

async fn run_search<C: Mailbox + ?Sized>(
    app: &mut AppState,
    client: &mut C,
    account: Option<String>,
    query: String,
) {
    let scope_account = match account {
        Some(name) => match app.account_id_by_name(&name) {
            Some(id) => Some(id),
            None => {
                let message = format!("unknown account: {name}");
                app.push_toast(app::ToastKind::Error, message.clone(), Instant::now());
                app.set_error(message.clone());
                app.set_status(message);
                return;
            }
        },
        None => None,
    };
    submit_search(app, client, query, scope_account).await;
}

async fn submit_search<C: Mailbox + ?Sized>(
    app: &mut AppState,
    client: &mut C,
    query: String,
    scope_account: Option<Uuid>,
) {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        app.set_status("Search needs a query");
        return;
    }
    let now = Instant::now();
    app.push_toast(app::ToastKind::Info, "Searching…", now);
    app.begin_search(trimmed, scope_account);
    app.set_status(format!("Searching {trimmed}"));

    match client.search(trimmed, scope_account).await {
        Ok(hits) => {
            let count = hits.len();
            app.apply_search_hits(hits);
            app.push_toast(
                app::ToastKind::Info,
                format!("{count} result(s)"),
                Instant::now(),
            );
            app.set_status(format!("{count} search result(s)"));
        }
        Err(error) => {
            let message = error.to_string();
            app.push_toast(app::ToastKind::Error, message.clone(), Instant::now());
            app.set_error(message.clone());
            app.close_search();
            app.set_status(message);
        }
    }
}

async fn run_goto_folder<C: Mailbox + ?Sized>(
    app: &mut AppState,
    client: &mut C,
    folder_name: &str,
) {
    let folder_name = folder_name.trim();
    if folder_name.is_empty() {
        record_command_parse_error(app, "usage: goto <folder>".into());
        return;
    }
    if app.selected_account_id().is_none() {
        record_command_run_error(app, CommandRunError::AccountNotSelected);
        return;
    }
    if !app.select_folder_by_name(folder_name) {
        let message = format!("No folder named '{folder_name}' for current account");
        app.set_status(message.clone());
        app.set_error(message);
        return;
    }
    app.clear_error();
    app.set_status(format!("Folder: {folder_name}"));
    refresh_messages(app, client).await;
}

async fn run_select_account<C: Mailbox + ?Sized>(app: &mut AppState, client: &mut C, name: &str) {
    let name = name.trim();
    if name.is_empty() {
        record_command_parse_error(app, "usage: account <name|email>".into());
        return;
    }
    if !app.select_account_by_name(name) {
        let message = format!("No account named '{name}'");
        app.set_status(message.clone());
        app.set_error(message);
        return;
    }
    app.clear_error();
    app.set_status(format!("Account: {name}"));
    refresh_folders(app, client).await;
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
    #[error("no account selected")]
    AccountNotSelected,
    #[error("no folder selected")]
    FolderUnavailable,
    #[error("no message selected")]
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
    // Optimistic local update: revert if the daemon refuses.
    let previous_flags = app
        .selected_message()
        .map(|message| message.flags.clone())
        .unwrap_or_default();
    app.apply_message_flags(message_id, flags.clone());
    app.set_status("…");
    match client.set_flags(message_id, &flags).await {
        Ok(()) => {
            refresh_messages(app, client).await;
            if app.error.is_none() {
                app.set_status(success);
            }
        }
        Err(error) => {
            app.apply_message_flags(message_id, previous_flags);
            record_error(app, error);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MessageRemove {
    Archive,
    Delete,
}

impl MessageRemove {
    fn running_status(self) -> &'static str {
        match self {
            Self::Archive => "Archiving…",
            Self::Delete => "Deleting…",
        }
    }

    fn success_status(self) -> &'static str {
        match self {
            Self::Archive => "Archived",
            Self::Delete => "Deleted",
        }
    }
}

async fn run_message_remove<C: Mailbox + ?Sized>(
    app: &mut AppState,
    client: &mut C,
    op: MessageRemove,
) {
    let Some(message_id) = app.selected_message_id() else {
        record_command_run_error(app, CommandRunError::MessageMissing);
        return;
    };
    run_message_remove_for(app, client, op, message_id).await;
}

async fn run_message_remove_for<C: Mailbox + ?Sized>(
    app: &mut AppState,
    client: &mut C,
    op: MessageRemove,
    message_id: Uuid,
) {
    app.clear_error();
    let snapshot = app.snapshot_message_list();
    app.remove_message_locally(message_id);
    app.set_status(op.running_status());

    let result = match op {
        MessageRemove::Archive => client.archive_message(message_id).await,
        MessageRemove::Delete => client.delete_message(message_id).await,
    };

    match result {
        Ok(()) => {
            refresh_messages(app, client).await;
            if app.error.is_none() {
                app.set_status(op.success_status());
            }
        }
        Err(error) => {
            app.restore_message_list_snapshot(snapshot);
            record_error(app, error);
        }
    }
}

async fn run_message_move<C: Mailbox + ?Sized>(
    app: &mut AppState,
    client: &mut C,
    folder_name: &str,
) {
    let Some(message_id) = app.selected_message_id() else {
        record_command_run_error(app, CommandRunError::MessageMissing);
        return;
    };
    let folder_name = folder_name.trim();
    if folder_name.is_empty() {
        record_command_parse_error(app, "usage: move <folder>".into());
        return;
    }

    app.clear_error();
    let snapshot = app.snapshot_message_list();
    app.remove_message_locally(message_id);
    app.set_status(format!("Moving to {folder_name}…"));

    match client.move_message(message_id, folder_name).await {
        Ok(()) => {
            refresh_messages(app, client).await;
            if app.error.is_none() {
                app.set_status(format!("Moved to {folder_name}"));
            }
        }
        Err(error) => {
            app.restore_message_list_snapshot(snapshot);
            record_error(app, error);
        }
    }
}

fn message_list_focused(app: &AppState) -> bool {
    matches!(app.active, ActivePane::Messages | ActivePane::Threads)
}

/// True when the user is on the messages pane in a Drafts folder, so
/// the Enter / d / etc. keybindings act on the drafts list instead of
/// the regular message list.
fn drafts_list_focused(app: &AppState) -> bool {
    app.active == ActivePane::Messages && app.drafts_pane_active()
}

fn begin_message_delete(app: &mut AppState) {
    match app.selected_message_id() {
        Some(message_id) => app.begin_delete_confirmation(message_id),
        None => record_command_run_error(app, CommandRunError::MessageMissing),
    }
}

async fn handle_delete_confirmation_key<C: Mailbox + ?Sized>(
    key: KeyEvent,
    app: &mut AppState,
    client: &mut C,
) -> bool {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            if app.pending_delete_draft.is_some() {
                delete_pending_draft(app, client).await;
            } else if let Some(message_id) = app.take_pending_delete_message() {
                run_message_remove_for(app, client, MessageRemove::Delete, message_id).await;
            }
            false
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
            if app.pending_delete_draft.is_some() {
                app.cancel_pending_delete_draft();
                app.set_status("Delete cancelled");
            } else {
                app.cancel_delete_confirmation();
            }
            false
        }
        _ => false,
    }
}

/// Handle keys while typing into the `/` quick-search overlay. Esc
/// restores the previous pane; Enter submits the query (scoped to the
/// current account when one is selected); printable chars build up the
/// query buffer.
async fn handle_quick_search_key<C: Mailbox + ?Sized>(
    key: KeyEvent,
    app: &mut AppState,
    client: &mut C,
) -> bool {
    match key.code {
        KeyCode::Esc => {
            app.cancel_quick_search();
            false
        }
        KeyCode::Enter => {
            let query = app.finish_quick_search();
            let scope = app.selected_account_id();
            submit_search(app, client, query, scope).await;
            false
        }
        KeyCode::Backspace => {
            app.backspace_search();
            false
        }
        KeyCode::Char(ch) => {
            if !app.push_search_char(ch) {
                app.set_error(format!(
                    "search is limited to {} characters",
                    app::MAX_SEARCH_CHARS
                ));
            }
            false
        }
        _ => false,
    }
}

/// Handle keys while the Search pane is focused. Returns true when the
/// key was consumed; false lets the outer key handler fall through to
/// the normal-mode bindings (`/`, `:`, `q`, etc).
async fn handle_search_pane_key<C: Mailbox + ?Sized>(
    key: KeyEvent,
    app: &mut AppState,
    client: &mut C,
) -> bool {
    match key.code {
        KeyCode::Esc => {
            app.close_search();
            app.set_status("Search closed");
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.move_search_selection(1);
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.move_search_selection(-1);
            true
        }
        KeyCode::Enter => {
            if let Some(hit) = app.selected_search_hit().cloned() {
                if app.jump_to_hit(&hit) {
                    refresh_folders(app, client).await;
                    refresh_messages(app, client).await;
                    refresh_detail(app, client).await;
                    app.set_status(format!("Opened: {}", hit.subject));
                } else {
                    let message = "Could not jump to result".to_string();
                    app.set_error(message.clone());
                    app.set_status(message);
                }
            }
            true
        }
        KeyCode::Char('r') => {
            refresh_search(app, client).await;
            true
        }
        _ => false,
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

    let draft_id = match app.composer_draft_id() {
        Some(id) if !app.composer_is_dirty() => id,
        _ => match save_composer(app, client).await {
            Some(id) => id,
            None => return,
        },
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
        ActivePane::Messages => {
            if app.drafts_pane_active() {
                refresh_drafts(app, client).await;
            } else {
                refresh_messages(app, client).await;
            }
        }
        ActivePane::Details => {
            if app.drafts_pane_active() {
                // Detail pane is unused while viewing drafts.
                refresh_drafts(app, client).await;
            } else {
                refresh_detail(app, client).await;
            }
        }
        ActivePane::Attachments => refresh_attachments(app, client).await,
        ActivePane::Search => refresh_search(app, client).await,
    }
}

async fn refresh_after_selection_change<C: Mailbox + ?Sized>(app: &mut AppState, client: &mut C) {
    match app.active {
        ActivePane::Accounts => refresh_folders(app, client).await,
        ActivePane::Folders => {
            if app.drafts_pane_active() {
                refresh_drafts(app, client).await;
            } else {
                refresh_messages(app, client).await;
            }
        }
        ActivePane::Threads => refresh_detail(app, client).await,
        ActivePane::Messages => {
            if !app.drafts_pane_active() {
                refresh_detail(app, client).await;
            }
        }
        ActivePane::Details => refresh_detail(app, client).await,
        ActivePane::Attachments => refresh_attachment_preview(app, client).await,
        ActivePane::Search => {}
    }
}

/// Re-run the active search (used by `r` while the Search pane is
/// focused). No-op when no search is open.
async fn refresh_search<C: Mailbox + ?Sized>(app: &mut AppState, client: &mut C) {
    let Some(query) = app.search_query().map(str::to_owned) else {
        return;
    };
    let scope = app.search_scope_account();
    submit_search(app, client, query, scope).await;
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

async fn refresh_drafts<C: Mailbox + ?Sized>(app: &mut AppState, client: &mut C) {
    let Some(account_id) = app.selected_account_id() else {
        app.apply_drafts(Vec::new());
        app.set_status("No account selected");
        return;
    };
    app.set_status("Loading drafts");
    match client.list_drafts(account_id).await {
        Ok(drafts) => {
            let count = drafts.len();
            app.clear_error();
            app.apply_drafts(drafts);
            if count == 0 {
                app.set_status("No drafts");
            } else {
                app.set_status(format!("Loaded {count} draft(s)"));
            }
        }
        Err(error) => record_error(app, error),
    }
}

async fn open_selected_draft<C: Mailbox + ?Sized>(app: &mut AppState, client: &mut C) {
    let Some(draft_id) = app.selected_draft_id() else {
        app.set_status("No draft selected");
        return;
    };
    app.set_status("Loading draft");
    let summary = match client.get_draft(draft_id).await {
        Ok(Some(summary)) => summary,
        Ok(None) => {
            app.set_status("Draft no longer exists");
            return;
        }
        Err(error) => {
            record_error(app, error);
            return;
        }
    };
    let composer_draft = match composer_draft_from_summary(&summary).await {
        Ok(draft) => draft,
        Err(error) => {
            let message = format!("Cannot reopen draft: {error}");
            app.push_toast(app::ToastKind::Error, message.clone(), Instant::now());
            app.set_error(message);
            return;
        }
    };
    app.enter_composer_for_existing_draft(draft_id, composer_draft, app::ComposeField::Body);
    app.set_status("Compose (resumed)");
}

async fn delete_pending_draft<C: Mailbox + ?Sized>(app: &mut AppState, client: &mut C) {
    let Some(draft_id) = app.take_pending_delete_draft() else {
        return;
    };
    // Optimistic removal — restore on error.
    let removed = app.remove_draft_locally(draft_id);
    match client.delete_draft(draft_id).await {
        Ok(()) => {
            if removed {
                app.set_status(format!("Draft {draft_id} deleted"));
            }
        }
        Err(error) => {
            record_error(app, error);
            // Re-fetch authoritative list since we already mutated.
            refresh_drafts(app, client).await;
        }
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
        ArchiveMessage(Uuid),
        DeleteMessage(Uuid),
        MoveMessage(Uuid, String),
        ListMessages(Uuid),
        GetMessage(Uuid),
        ListAttachments(Uuid),
        PreviewAttachment(Uuid),
        ExportAttachment(Uuid, PathBuf),
        CreateDraft(app::ComposerDraft),
        UpdateDraft(Uuid, app::ComposerDraft),
        SendDraft(Uuid, Uuid),
        Search(String, Option<Uuid>),
        PrepareReply(Uuid, bool),
        PrepareForward(Uuid),
        FetchAttachmentForForward(Uuid),
        ListDrafts(Uuid),
        GetDraft(Uuid),
        DeleteDraft(Uuid),
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
        search_hits: Vec<app::SearchHit>,
        reply_prepared: Option<ipc::ReplyPrepared>,
        forward_prepared: Option<ipc::ForwardPrepared>,
        forward_attachment_bytes: Option<ipc::ForwardAttachmentBytes>,
        drafts: Vec<app::DraftItem>,
        draft_summary: Option<app::DraftSummary>,
        fail_sync: bool,
        fail_set_flags: bool,
        fail_archive: bool,
        fail_delete: bool,
        fail_move: bool,
        fail_draft: bool,
        fail_send: bool,
        fail_search: bool,
        fail_prepare_reply: bool,
        fail_prepare_forward: bool,
        fail_fetch_attachment_for_forward: bool,
        fail_list_drafts: bool,
        fail_get_draft: bool,
        fail_delete_draft: bool,
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

        async fn archive_message(&mut self, message_id: Uuid) -> Result<(), ipc::MailboxError> {
            self.calls.push(Call::ArchiveMessage(message_id));
            if self.fail_archive {
                Err(server_error("message.archive"))
            } else {
                Ok(())
            }
        }

        async fn delete_message(&mut self, message_id: Uuid) -> Result<(), ipc::MailboxError> {
            self.calls.push(Call::DeleteMessage(message_id));
            if self.fail_delete {
                Err(server_error("message.delete"))
            } else {
                Ok(())
            }
        }

        async fn move_message(
            &mut self,
            message_id: Uuid,
            folder_name: &str,
        ) -> Result<(), ipc::MailboxError> {
            self.calls
                .push(Call::MoveMessage(message_id, folder_name.to_string()));
            if self.fail_move {
                Err(server_error("message.move"))
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

        async fn search(
            &mut self,
            query: &str,
            account_id: Option<Uuid>,
        ) -> Result<Vec<app::SearchHit>, ipc::MailboxError> {
            self.calls.push(Call::Search(query.to_string(), account_id));
            if self.fail_search {
                Err(server_error("search"))
            } else {
                Ok(self.search_hits.clone())
            }
        }

        async fn prepare_reply(
            &mut self,
            message_id: Uuid,
            reply_all: bool,
        ) -> Result<ipc::ReplyPrepared, ipc::MailboxError> {
            self.calls.push(Call::PrepareReply(message_id, reply_all));
            if self.fail_prepare_reply {
                return Err(server_error("message.prepare_reply"));
            }
            Ok(self
                .reply_prepared
                .clone()
                .unwrap_or_else(|| ipc::ReplyPrepared {
                    message_id,
                    account_id: Uuid::nil(),
                    to: Vec::new(),
                    cc: Vec::new(),
                    subject: String::new(),
                    in_reply_to: String::new(),
                    references: String::new(),
                    quoted_body: String::new(),
                }))
        }

        async fn prepare_forward(
            &mut self,
            message_id: Uuid,
        ) -> Result<ipc::ForwardPrepared, ipc::MailboxError> {
            self.calls.push(Call::PrepareForward(message_id));
            if self.fail_prepare_forward {
                return Err(server_error("message.prepare_forward"));
            }
            Ok(self
                .forward_prepared
                .clone()
                .unwrap_or_else(|| ipc::ForwardPrepared {
                    message_id,
                    account_id: Uuid::nil(),
                    subject: String::new(),
                    forwarded_body: String::new(),
                    forwarded_attachments: Vec::new(),
                }))
        }

        async fn fetch_attachment_for_forward(
            &mut self,
            attachment_id: Uuid,
        ) -> Result<ipc::ForwardAttachmentBytes, ipc::MailboxError> {
            self.calls
                .push(Call::FetchAttachmentForForward(attachment_id));
            if self.fail_fetch_attachment_for_forward {
                return Err(server_error("attachment.fetch_for_forward"));
            }
            Ok(self.forward_attachment_bytes.clone().unwrap_or_else(|| {
                ipc::ForwardAttachmentBytes {
                    attachment_id,
                    filename: "att.bin".into(),
                    content_type: "application/octet-stream".into(),
                    size_bytes: 0,
                    content_base64: String::new(),
                }
            }))
        }

        async fn list_drafts(
            &mut self,
            account_id: Uuid,
        ) -> Result<Vec<app::DraftItem>, ipc::MailboxError> {
            self.calls.push(Call::ListDrafts(account_id));
            if self.fail_list_drafts {
                Err(server_error("draft.list"))
            } else {
                Ok(self.drafts.clone())
            }
        }

        async fn get_draft(
            &mut self,
            draft_id: Uuid,
        ) -> Result<Option<app::DraftSummary>, ipc::MailboxError> {
            self.calls.push(Call::GetDraft(draft_id));
            if self.fail_get_draft {
                Err(server_error("draft.get"))
            } else {
                Ok(self.draft_summary.clone())
            }
        }

        async fn delete_draft(&mut self, draft_id: Uuid) -> Result<(), ipc::MailboxError> {
            self.calls.push(Call::DeleteDraft(draft_id));
            if self.fail_delete_draft {
                Err(server_error("draft.delete"))
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
        assert_eq!(app.error.as_deref(), Some("no account selected"));

        app.clear_error();
        execute_command(Command::Seen, &mut app, &mut client).await;
        assert_eq!(app.error.as_deref(), Some("no message selected"));
        assert!(client.calls.is_empty());
    }

    #[tokio::test]
    async fn test_run_command_line_reports_parse_errors() {
        let mut app = AppState::default();
        let mut client = MockMailbox::default();

        run_command_line("theme solarized".into(), &mut app, &mut client).await;

        assert_eq!(
            app.error.as_deref(),
            Some("usage: theme next|light|dark|high-contrast")
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
                    attachments: Vec::new(),
                    in_reply_to: None,
                    references_header: None,
                }),
                Call::SendDraft(account_id, draft_id),
            ]
        );
        assert_eq!(app.mode, InputMode::Normal);
        assert!(app.composer.is_none());
        assert!(app.status.contains("<sent-1@postblox.local>"));
    }

    #[tokio::test]
    async fn test_handle_key_composer_ctrl_k_with_no_attachments_sets_empty_state_status() {
        let account_id = Uuid::new_v4();
        let mut app = AppState::default();
        app.apply_accounts(vec![account_item(account_id)]);
        let mut client = MockMailbox::default();

        handle_key(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE),
            &mut app,
            &mut client,
        )
        .await;
        assert_eq!(app.mode, InputMode::Compose);

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL),
                &mut app,
                &mut client,
            )
            .await
        );

        assert_eq!(app.status, "No attachment to remove");
        assert!(client.calls.is_empty());
    }

    fn app_with_message_list_focused(account_id: Uuid, folder_id: Uuid) -> (AppState, MessageItem) {
        let mut app = app_with_account_folder(account_id, folder_id);
        let message = message_item(Uuid::new_v4(), vec!["\\Seen"]);
        app.apply_folder_messages(vec![message.clone()]);
        app.active = ActivePane::Messages;
        (app, message)
    }

    #[tokio::test]
    async fn test_handle_key_e_archives_when_message_list_focused() {
        let account_id = Uuid::new_v4();
        let folder_id = Uuid::new_v4();
        let (mut app, message) = app_with_message_list_focused(account_id, folder_id);
        let mut client = MockMailbox::default();

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );

        assert!(client.calls.contains(&Call::ArchiveMessage(message.id)));
        assert_eq!(app.status, "Archived");
        assert!(app.messages.is_empty());
    }

    #[tokio::test]
    async fn test_handle_key_d_opens_delete_confirmation_when_message_list_focused() {
        let account_id = Uuid::new_v4();
        let folder_id = Uuid::new_v4();
        let (mut app, message) = app_with_message_list_focused(account_id, folder_id);
        let mut client = MockMailbox::default();

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );

        assert_eq!(app.mode, InputMode::ConfirmDelete);
        assert_eq!(app.pending_delete_message, Some(message.id));
        assert!(client.calls.is_empty());
    }

    #[tokio::test]
    async fn test_handle_key_confirm_delete_yes_deletes_and_clears_pending() {
        let account_id = Uuid::new_v4();
        let folder_id = Uuid::new_v4();
        let (mut app, message) = app_with_message_list_focused(account_id, folder_id);
        app.begin_delete_confirmation(message.id);
        let mut client = MockMailbox::default();

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );

        assert_eq!(app.mode, InputMode::Normal);
        assert!(app.pending_delete_message.is_none());
        assert!(client.calls.contains(&Call::DeleteMessage(message.id)));
        assert!(app.messages.is_empty());
        assert_eq!(app.status, "Deleted");
    }

    #[tokio::test]
    async fn test_handle_key_confirm_delete_no_cancels_without_calling_daemon() {
        let account_id = Uuid::new_v4();
        let folder_id = Uuid::new_v4();
        let (mut app, message) = app_with_message_list_focused(account_id, folder_id);
        app.begin_delete_confirmation(message.id);
        let mut client = MockMailbox::default();

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );

        assert_eq!(app.mode, InputMode::Normal);
        assert!(app.pending_delete_message.is_none());
        assert!(client.calls.is_empty());
        assert_eq!(app.messages.len(), 1);
    }

    #[tokio::test]
    async fn test_handle_key_confirm_delete_esc_cancels() {
        let account_id = Uuid::new_v4();
        let folder_id = Uuid::new_v4();
        let (mut app, message) = app_with_message_list_focused(account_id, folder_id);
        app.begin_delete_confirmation(message.id);
        let mut client = MockMailbox::default();

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );

        assert_eq!(app.mode, InputMode::Normal);
        assert!(app.pending_delete_message.is_none());
        assert!(client.calls.is_empty());
    }

    #[tokio::test]
    async fn test_archive_failure_restores_message_list() {
        let account_id = Uuid::new_v4();
        let folder_id = Uuid::new_v4();
        let (mut app, message) = app_with_message_list_focused(account_id, folder_id);
        let mut client = MockMailbox {
            fail_archive: true,
            ..Default::default()
        };

        execute_command(Command::Archive, &mut app, &mut client).await;

        assert_eq!(client.calls, vec![Call::ArchiveMessage(message.id)]);
        assert_eq!(app.messages.len(), 1);
        assert_eq!(app.messages[0].id, message.id);
        assert!(app.error.as_deref().unwrap().contains("boom"));
    }

    #[tokio::test]
    async fn test_handle_key_m_opens_command_bar_with_move_prefix() {
        let account_id = Uuid::new_v4();
        let folder_id = Uuid::new_v4();
        let (mut app, _) = app_with_message_list_focused(account_id, folder_id);
        let mut client = MockMailbox::default();

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );

        assert_eq!(app.mode, InputMode::Command);
        assert_eq!(app.command_input, "move ");
    }

    #[tokio::test]
    async fn test_execute_command_move_calls_daemon_with_folder_name() {
        let account_id = Uuid::new_v4();
        let folder_id = Uuid::new_v4();
        let (mut app, message) = app_with_message_list_focused(account_id, folder_id);
        let mut client = MockMailbox::default();

        execute_command(Command::Move("Archive".into()), &mut app, &mut client).await;

        assert!(client
            .calls
            .contains(&Call::MoveMessage(message.id, "Archive".into())));
        assert_eq!(app.status, "Moved to Archive");
    }

    #[tokio::test]
    async fn test_execute_command_move_failure_restores_message_list() {
        let account_id = Uuid::new_v4();
        let folder_id = Uuid::new_v4();
        let (mut app, message) = app_with_message_list_focused(account_id, folder_id);
        let mut client = MockMailbox {
            fail_move: true,
            ..Default::default()
        };

        execute_command(Command::Move("Archive".into()), &mut app, &mut client).await;

        assert_eq!(
            client.calls,
            vec![Call::MoveMessage(message.id, "Archive".into())]
        );
        assert_eq!(app.messages.len(), 1);
        assert_eq!(app.messages[0].id, message.id);
        assert!(app.error.as_deref().unwrap().contains("boom"));
    }

    #[tokio::test]
    async fn test_handle_key_star_toggles_flag_when_message_list_focused() {
        let account_id = Uuid::new_v4();
        let folder_id = Uuid::new_v4();
        let (mut app, message) = app_with_message_list_focused(account_id, folder_id);
        let mut client = MockMailbox::default();

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Char('*'), KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );

        assert!(client.calls.iter().any(|call| matches!(
            call,
            Call::SetFlags(id, flags) if *id == message.id && flags.contains(&"\\Flagged".to_string())
        )));
        assert_eq!(app.status, "Flagged message");
    }

    #[tokio::test]
    async fn test_handle_key_star_unflags_when_already_flagged() {
        let account_id = Uuid::new_v4();
        let folder_id = Uuid::new_v4();
        let mut app = app_with_account_folder(account_id, folder_id);
        let message = message_item(Uuid::new_v4(), vec!["\\Flagged"]);
        app.apply_messages(vec![message.clone()]);
        app.active = ActivePane::Messages;
        let mut client = MockMailbox::default();

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Char('*'), KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );

        assert!(client.calls.iter().any(|call| matches!(
            call,
            Call::SetFlags(id, flags) if *id == message.id && !flags.contains(&"\\Flagged".to_string())
        )));
        assert_eq!(app.status, "Unflagged message");
    }

    #[tokio::test]
    async fn test_handle_key_x_dismisses_newest_toast_in_normal_mode() {
        use std::time::Instant;
        let mut app = AppState::default();
        let now = Instant::now();
        app.push_toast(app::ToastKind::Info, "first", now);
        app.push_toast(app::ToastKind::Info, "second", now);
        let mut client = MockMailbox::default();

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );

        assert_eq!(app.toasts.len(), 1);
        assert_eq!(app.toasts.front().unwrap().text, "first");
    }

    #[tokio::test]
    async fn test_handle_key_capital_x_clears_all_toasts_in_normal_mode() {
        use std::time::Instant;
        let mut app = AppState::default();
        let now = Instant::now();
        app.push_toast(app::ToastKind::Info, "a", now);
        app.push_toast(app::ToastKind::Error, "b", now);
        let mut client = MockMailbox::default();

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Char('X'), KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );

        assert!(app.toasts.is_empty());
    }

    #[tokio::test]
    async fn test_handle_key_x_in_command_mode_inserts_text_and_keeps_toast() {
        use std::time::Instant;
        let mut app = AppState::default();
        app.enter_command_mode();
        app.push_toast(app::ToastKind::Info, "stay", Instant::now());
        let mut client = MockMailbox::default();

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
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

        assert_eq!(app.command_input, "xX");
        assert_eq!(app.toasts.len(), 1);
    }

    #[tokio::test]
    async fn test_handle_key_x_in_compose_mode_inserts_text_and_keeps_toast() {
        use std::time::Instant;
        let account_id = Uuid::new_v4();
        let mut app = AppState::default();
        app.enter_composer(account_id);
        app.push_toast(app::ToastKind::Info, "stay", Instant::now());
        let mut client = MockMailbox::default();

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );

        assert_eq!(app.composer.as_ref().unwrap().to, "x");
        assert_eq!(app.toasts.len(), 1);
    }

    #[tokio::test]
    async fn test_handle_key_x_in_confirm_delete_does_not_dismiss_toast() {
        use std::time::Instant;
        let account_id = Uuid::new_v4();
        let folder_id = Uuid::new_v4();
        let (mut app, message) = app_with_message_list_focused(account_id, folder_id);
        app.begin_delete_confirmation(message.id);
        app.push_toast(app::ToastKind::Info, "stay", Instant::now());
        let mut client = MockMailbox::default();

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await
        );

        assert_eq!(app.toasts.len(), 1);
        assert_eq!(app.mode, InputMode::ConfirmDelete);
    }

    fn app_with_specific_message(account_id: Uuid, folder_id: Uuid, message_id: Uuid) -> AppState {
        let mut app = app_with_account_folder(account_id, folder_id);
        let message = MessageItem {
            id: message_id,
            thread_id: None,
            subject: "Hello".into(),
            from: "alice@example.com".into(),
            date: "2026-05-07 10:00".into(),
            snippet: "Preview".into(),
            flags: vec!["\\Seen".into()],
        };
        app.apply_folder_messages(vec![message]);
        app.active = ActivePane::Messages;
        app
    }

    /// `:archive` must drive the same archive handler as the `e` key.
    /// Both paths must produce the same daemon-visible call sequence
    /// and post-state.
    #[tokio::test]
    async fn test_command_bar_archive_matches_e_keybinding() {
        let account_id = Uuid::new_v4();
        let folder_id = Uuid::new_v4();
        let message_id = Uuid::new_v4();

        let mut key_app = app_with_specific_message(account_id, folder_id, message_id);
        let mut key_client = MockMailbox::default();
        handle_key(
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
            &mut key_app,
            &mut key_client,
        )
        .await;

        let mut cmd_app = app_with_specific_message(account_id, folder_id, message_id);
        let mut cmd_client = MockMailbox::default();
        run_command_line("archive".into(), &mut cmd_app, &mut cmd_client).await;

        assert_eq!(key_client.calls, cmd_client.calls);
        assert_eq!(key_app.status, cmd_app.status);
        assert_eq!(key_app.messages.len(), cmd_app.messages.len());
        assert_eq!(key_app.error, cmd_app.error);
    }

    /// `:delete` followed by confirming `y` must drive the same
    /// confirm-modal + delete path as pressing `d` then `y`.
    #[tokio::test]
    async fn test_command_bar_delete_with_confirm_matches_d_keybinding() {
        let account_id = Uuid::new_v4();
        let folder_id = Uuid::new_v4();
        let message_id = Uuid::new_v4();

        let mut key_app = app_with_specific_message(account_id, folder_id, message_id);
        let mut key_client = MockMailbox::default();
        handle_key(
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
            &mut key_app,
            &mut key_client,
        )
        .await;
        handle_key(
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
            &mut key_app,
            &mut key_client,
        )
        .await;

        let mut cmd_app = app_with_specific_message(account_id, folder_id, message_id);
        let mut cmd_client = MockMailbox::default();
        run_command_line("delete".into(), &mut cmd_app, &mut cmd_client).await;
        handle_key(
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
            &mut cmd_app,
            &mut cmd_client,
        )
        .await;

        assert_eq!(key_client.calls, cmd_client.calls);
        assert_eq!(key_app.status, cmd_app.status);
        assert_eq!(key_app.messages.len(), cmd_app.messages.len());
    }

    /// `:move <folder>` parses the multi-word folder and dispatches via
    /// the same `Command::Move` path as the `m` key prefill.
    #[tokio::test]
    async fn test_command_bar_move_dispatches_move_message_call() {
        let account_id = Uuid::new_v4();
        let folder_id = Uuid::new_v4();
        let (mut app, message) = app_with_message_list_focused(account_id, folder_id);
        let mut client = MockMailbox::default();

        run_command_line("move INBOX/Receipts 2025".into(), &mut app, &mut client).await;

        assert!(client.calls.iter().any(|call| matches!(
            call,
            Call::MoveMessage(id, folder)
                if *id == message.id && folder == "INBOX/Receipts 2025"
        )));
        assert_eq!(app.status, "Moved to INBOX/Receipts 2025");
    }

    #[tokio::test]
    async fn test_command_bar_compose_opens_composer_for_current_account() {
        let account_id = Uuid::new_v4();
        let mut app = AppState::default();
        app.apply_accounts(vec![account_item(account_id)]);
        let mut client = MockMailbox::default();

        run_command_line("compose".into(), &mut app, &mut client).await;

        assert_eq!(app.mode, InputMode::Compose);
        assert_eq!(app.composer.as_ref().unwrap().account_id, account_id);
    }

    #[tokio::test]
    async fn test_command_bar_compose_without_account_records_error() {
        let mut app = AppState::default();
        let mut client = MockMailbox::default();

        run_command_line("compose".into(), &mut app, &mut client).await;

        assert!(app.composer.is_none());
        assert_eq!(app.error.as_deref(), Some("no account selected"));
    }

    #[tokio::test]
    async fn test_command_bar_reply_without_message_records_error() {
        let mut app = AppState::default();
        let mut client = MockMailbox::default();

        run_command_line("reply".into(), &mut app, &mut client).await;
        assert!(app.status.contains("no message selected"));
        run_command_line("reply-all".into(), &mut app, &mut client).await;
        assert!(app.status.contains("no message selected"));
        run_command_line("forward".into(), &mut app, &mut client).await;
        assert!(app.status.contains("no message selected"));
        // Without a selected message we never reach the daemon.
        assert!(client.calls.is_empty());
    }

    #[tokio::test]
    async fn test_command_bar_search_runs_unscoped_query() {
        let mut app = AppState::default();
        let mut client = MockMailbox {
            search_hits: vec![search_hit("test mail", Uuid::new_v4(), Uuid::new_v4())],
            ..Default::default()
        };

        run_command_line("search foo bar".into(), &mut app, &mut client).await;

        assert!(matches!(
            client.calls.last(),
            Some(Call::Search(query, None)) if query == "foo bar"
        ));
        assert!(app.search_pane_visible());
        assert_eq!(app.active, ActivePane::Search);
        assert!(app.status.contains("1 search result"));
    }

    #[tokio::test]
    async fn test_command_bar_search_scopes_to_named_account() {
        let work_id = Uuid::new_v4();
        let mut app = AppState::default();
        app.apply_accounts(vec![account_item(work_id)]);
        let mut client = MockMailbox::default();

        run_command_line(
            "search --account Work foo bar".into(),
            &mut app,
            &mut client,
        )
        .await;

        assert!(matches!(
            client.calls.last(),
            Some(Call::Search(query, Some(id))) if query == "foo bar" && *id == work_id
        ));
        assert!(app.search_pane_visible());
    }

    #[tokio::test]
    async fn test_command_bar_search_unknown_account_errors_and_no_call() {
        let mut app = AppState::default();
        app.apply_accounts(vec![account_item(Uuid::new_v4())]);
        let mut client = MockMailbox::default();

        run_command_line(
            "search --account Personal foo bar".into(),
            &mut app,
            &mut client,
        )
        .await;

        assert!(client.calls.is_empty());
        assert_eq!(app.error.as_deref(), Some("unknown account: Personal"));
        assert!(!app.search_pane_visible());
    }

    #[tokio::test]
    async fn test_quick_search_slash_collects_chars_then_submits_with_account_scope() {
        let work_id = Uuid::new_v4();
        let mut app = AppState::default();
        app.apply_accounts(vec![account_item(work_id)]);
        let mut client = MockMailbox::default();

        let consumed = handle_key(
            KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
            &mut app,
            &mut client,
        )
        .await;
        assert!(!consumed);
        assert_eq!(app.mode, InputMode::QuickSearch);

        for ch in "test".chars() {
            handle_key(
                KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await;
        }
        assert_eq!(app.search_input, "test");

        handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut app,
            &mut client,
        )
        .await;
        assert_eq!(app.mode, InputMode::Normal);
        assert!(matches!(
            client.calls.last(),
            Some(Call::Search(query, Some(id))) if query == "test" && *id == work_id
        ));
    }

    #[tokio::test]
    async fn test_quick_search_esc_cancels_and_restores_pane() {
        let mut app = AppState {
            active: ActivePane::Folders,
            ..AppState::default()
        };
        let mut client = MockMailbox::default();

        handle_key(
            KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
            &mut app,
            &mut client,
        )
        .await;
        for ch in "abc".chars() {
            handle_key(
                KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await;
        }
        handle_key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &mut app,
            &mut client,
        )
        .await;

        assert_eq!(app.mode, InputMode::Normal);
        assert!(app.search_input.is_empty());
        assert_eq!(app.active, ActivePane::Folders);
        assert!(client.calls.is_empty());
    }

    #[tokio::test]
    async fn test_quick_search_empty_query_does_not_call_daemon() {
        let mut app = AppState::default();
        let mut client = MockMailbox::default();

        handle_key(
            KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
            &mut app,
            &mut client,
        )
        .await;
        handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut app,
            &mut client,
        )
        .await;

        assert!(client.calls.is_empty());
        assert!(!app.search_pane_visible());
    }

    #[tokio::test]
    async fn test_search_pane_r_reruns_active_query_with_same_scope() {
        let work_id = Uuid::new_v4();
        let mut app = AppState::default();
        app.apply_accounts(vec![account_item(work_id)]);
        let mut client = MockMailbox::default();

        // Open the search pane via `/` + query + Enter; first call lands.
        handle_key(
            KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
            &mut app,
            &mut client,
        )
        .await;
        for ch in "alpha".chars() {
            handle_key(
                KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
                &mut app,
                &mut client,
            )
            .await;
        }
        handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut app,
            &mut client,
        )
        .await;
        assert_eq!(app.active, ActivePane::Search);
        assert_eq!(client.calls.len(), 1);
        assert!(matches!(
            client.calls[0],
            Call::Search(ref query, Some(id)) if query == "alpha" && id == work_id
        ));

        // `r` while the Search pane is focused re-runs the same query
        // with the same account scope.
        let consumed = handle_key(
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
            &mut app,
            &mut client,
        )
        .await;
        assert!(!consumed);
        assert_eq!(client.calls.len(), 2);
        assert!(matches!(
            client.calls[1],
            Call::Search(ref query, Some(id)) if query == "alpha" && id == work_id
        ));
    }

    fn search_hit(subject: &str, account_id: Uuid, folder_id: Uuid) -> app::SearchHit {
        app::SearchHit {
            message_id: Uuid::new_v4(),
            account_id,
            folder_id,
            subject: subject.into(),
            from: "alice@example.com".into(),
            snippet: "snippet".into(),
            date: "2026-05-09 10:00".into(),
        }
    }

    #[tokio::test]
    async fn test_command_bar_goto_switches_folder_and_loads_messages() {
        let account_id = Uuid::new_v4();
        let inbox_id = Uuid::new_v4();
        let archive_id = Uuid::new_v4();
        let mut app = AppState::default();
        app.apply_accounts(vec![account_item(account_id)]);
        app.apply_folders(vec![
            folder_item(inbox_id),
            FolderItem {
                id: archive_id,
                name: "Archive".into(),
                role: "archive".into(),
            },
        ]);
        let mut client = MockMailbox::default();

        run_command_line("goto Archive".into(), &mut app, &mut client).await;

        assert_eq!(app.selected_folder_id(), Some(archive_id));
        assert_eq!(app.active, ActivePane::Folders);
        assert!(client.calls.contains(&Call::ListMessages(archive_id)));
    }

    #[tokio::test]
    async fn test_command_bar_goto_unknown_folder_records_error() {
        let account_id = Uuid::new_v4();
        let inbox_id = Uuid::new_v4();
        let mut app = AppState::default();
        app.apply_accounts(vec![account_item(account_id)]);
        app.apply_folders(vec![folder_item(inbox_id)]);
        let mut client = MockMailbox::default();

        run_command_line("goto DoesNotExist".into(), &mut app, &mut client).await;

        assert_eq!(app.selected_folder_id(), Some(inbox_id));
        assert!(app
            .error
            .as_deref()
            .is_some_and(|e| e.contains("No folder named 'DoesNotExist'")));
        assert!(client.calls.is_empty());
    }

    #[tokio::test]
    async fn test_command_bar_account_switches_active_account() {
        let work_id = Uuid::new_v4();
        let home_id = Uuid::new_v4();
        let mut app = AppState::default();
        app.apply_accounts(vec![
            AccountItem {
                id: work_id,
                label: "Work".into(),
                email: "work@example.com".into(),
                status: "idle".into(),
            },
            AccountItem {
                id: home_id,
                label: "Home".into(),
                email: "home@example.com".into(),
                status: "idle".into(),
            },
        ]);
        let mut client = MockMailbox::default();

        run_command_line("account home".into(), &mut app, &mut client).await;

        assert_eq!(app.selected_account_id(), Some(home_id));
        assert_eq!(app.active, ActivePane::Accounts);
    }

    #[tokio::test]
    async fn test_command_bar_account_unknown_records_error() {
        let work_id = Uuid::new_v4();
        let mut app = AppState::default();
        app.apply_accounts(vec![account_item(work_id)]);
        let mut client = MockMailbox::default();

        run_command_line("account ghost@example.com".into(), &mut app, &mut client).await;

        assert_eq!(app.selected_account_id(), Some(work_id));
        assert!(app
            .error
            .as_deref()
            .is_some_and(|e| e.contains("No account named")));
    }

    #[tokio::test]
    async fn test_handle_command_key_unknown_command_emits_error() {
        let mut app = AppState::default();
        let mut client = MockMailbox::default();
        app.enter_command_mode();
        for ch in "wololo".chars() {
            app.push_command_char(ch);
        }
        handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut app,
            &mut client,
        )
        .await;

        assert!(app
            .error
            .as_deref()
            .is_some_and(|e| e.contains("unknown command")));
        assert!(client.calls.is_empty());
    }

    #[tokio::test]
    async fn test_handle_command_key_empty_input_is_no_op() {
        let mut app = AppState::default();
        let mut client = MockMailbox::default();
        app.enter_command_mode();
        handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut app,
            &mut client,
        )
        .await;
        // Empty input goes through the parser → CommandError::Empty,
        // which currently surfaces as a status-line note. The important
        // contract is that no daemon op fires.
        assert!(client.calls.is_empty());
        assert_eq!(app.mode, InputMode::Normal);
    }

    #[tokio::test]
    async fn test_handle_command_key_tab_completes_unique_prefix_with_trailing_space() {
        let mut app = AppState::default();
        let mut client = MockMailbox::default();
        app.enter_command_mode();
        for ch in "m".chars() {
            app.push_command_char(ch);
        }

        handle_key(
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            &mut app,
            &mut client,
        )
        .await;

        assert_eq!(app.command_input, "move ");
    }

    #[tokio::test]
    async fn test_handle_command_key_tab_for_g_completes_to_goto() {
        let mut app = AppState::default();
        let mut client = MockMailbox::default();
        app.enter_command_mode();
        app.push_command_char('g');

        handle_key(
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            &mut app,
            &mut client,
        )
        .await;

        assert_eq!(app.command_input, "goto ");
    }

    #[tokio::test]
    async fn test_handle_command_key_tab_for_empty_input_is_no_op() {
        let mut app = AppState::default();
        let mut client = MockMailbox::default();
        app.enter_command_mode();

        handle_key(
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            &mut app,
            &mut client,
        )
        .await;

        assert!(app.command_input.is_empty());
    }

    #[tokio::test]
    async fn test_handle_command_key_tab_with_multiple_matches_extends_prefix_and_lists() {
        let mut app = AppState::default();
        let mut client = MockMailbox::default();
        app.enter_command_mode();
        app.push_command_char('s');

        handle_key(
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            &mut app,
            &mut client,
        )
        .await;

        // 's' is the longest common prefix; status surfaces the
        // candidate list.
        assert_eq!(app.command_input, "s");
        assert!(app.status.starts_with("Matches:"));
        assert!(app.status.contains("sync"));
        assert!(app.status.contains("search"));
    }

    fn reply_prepared_fixture(message_id: Uuid, account_id: Uuid) -> ipc::ReplyPrepared {
        ipc::ReplyPrepared {
            message_id,
            account_id,
            to: vec!["alice@example.com".into()],
            cc: Vec::new(),
            subject: "Re: Hello".into(),
            in_reply_to: "<orig@example.com>".into(),
            references: "<orig@example.com>".into(),
            quoted_body: "On Sat, alice@example.com wrote:\r\n> Hi".into(),
        }
    }

    fn forward_prepared_fixture(message_id: Uuid, account_id: Uuid) -> ipc::ForwardPrepared {
        ipc::ForwardPrepared {
            message_id,
            account_id,
            subject: "Fwd: Hello".into(),
            forwarded_body: "---------- Forwarded message ----------\r\n".into(),
            forwarded_attachments: Vec::new(),
        }
    }

    #[tokio::test]
    async fn test_handle_key_capital_r_runs_reply_and_seeds_composer() {
        let account_id = Uuid::new_v4();
        let folder_id = Uuid::new_v4();
        let (mut app, message) = app_with_message_list_focused(account_id, folder_id);
        let mut client = MockMailbox {
            reply_prepared: Some(reply_prepared_fixture(message.id, account_id)),
            ..Default::default()
        };

        assert!(
            !handle_key(
                KeyEvent::new(KeyCode::Char('R'), KeyModifiers::SHIFT),
                &mut app,
                &mut client,
            )
            .await
        );

        assert!(matches!(
            client.calls.first(),
            Some(Call::PrepareReply(id, false)) if *id == message.id
        ));
        assert_eq!(app.mode, InputMode::Compose);
        let composer = app.composer.as_ref().unwrap();
        assert_eq!(composer.account_id, account_id);
        assert_eq!(composer.in_reply_to_msg, Some(message.id));
        assert_eq!(composer.in_reply_to.as_deref(), Some("<orig@example.com>"));
        assert!(composer.subject.starts_with("Re: "));
        assert!(composer.body.contains("> Hi"));
        assert_eq!(app.status, "Reply");
    }

    #[tokio::test]
    async fn test_handle_key_capital_a_runs_reply_all_and_passes_flag() {
        let account_id = Uuid::new_v4();
        let folder_id = Uuid::new_v4();
        let (mut app, message) = app_with_message_list_focused(account_id, folder_id);
        let mut client = MockMailbox {
            reply_prepared: Some(reply_prepared_fixture(message.id, account_id)),
            ..Default::default()
        };

        handle_key(
            KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT),
            &mut app,
            &mut client,
        )
        .await;

        assert!(matches!(
            client.calls.first(),
            Some(Call::PrepareReply(id, true)) if *id == message.id
        ));
        assert_eq!(app.status, "Reply-all");
    }

    #[tokio::test]
    async fn test_handle_key_capital_f_runs_forward_and_seeds_composer() {
        let account_id = Uuid::new_v4();
        let folder_id = Uuid::new_v4();
        let (mut app, message) = app_with_message_list_focused(account_id, folder_id);
        let mut client = MockMailbox {
            forward_prepared: Some(forward_prepared_fixture(message.id, account_id)),
            ..Default::default()
        };

        handle_key(
            KeyEvent::new(KeyCode::Char('F'), KeyModifiers::SHIFT),
            &mut app,
            &mut client,
        )
        .await;

        assert!(matches!(
            client.calls.first(),
            Some(Call::PrepareForward(id)) if *id == message.id
        ));
        assert_eq!(app.mode, InputMode::Compose);
        let composer = app.composer.as_ref().unwrap();
        assert!(composer.subject.starts_with("Fwd: "));
        assert!(composer.to.is_empty());
        assert_eq!(app.status, "Forward");
    }

    #[tokio::test]
    async fn test_handle_key_reply_without_message_records_status_and_skips_call() {
        let mut app = AppState::default();
        let mut client = MockMailbox::default();

        handle_key(
            KeyEvent::new(KeyCode::Char('R'), KeyModifiers::SHIFT),
            &mut app,
            &mut client,
        )
        .await;

        assert!(app.status.contains("no message selected"));
        assert!(client.calls.is_empty());
    }

    #[tokio::test]
    async fn test_handle_key_reply_failure_surfaces_toast_and_keeps_normal_mode() {
        let account_id = Uuid::new_v4();
        let folder_id = Uuid::new_v4();
        let (mut app, _message) = app_with_message_list_focused(account_id, folder_id);
        let mut client = MockMailbox {
            fail_prepare_reply: true,
            ..Default::default()
        };

        handle_key(
            KeyEvent::new(KeyCode::Char('R'), KeyModifiers::SHIFT),
            &mut app,
            &mut client,
        )
        .await;

        assert_eq!(app.mode, InputMode::Normal);
        assert!(app.composer.is_none());
        assert!(app.error.is_some());
    }

    // -- Slice 8: drafts list / reopen / delete -----------------------

    fn drafts_folder_item(id: Uuid) -> FolderItem {
        FolderItem {
            id,
            name: "[Gmail]/Drafts".into(),
            role: "drafts".into(),
        }
    }

    fn draft_item(id: Uuid, account_id: Uuid, subject: &str) -> app::DraftItem {
        app::DraftItem {
            id,
            account_id,
            subject: subject.into(),
            to: "bob@x.com".into(),
            date: "2026-05-09 12:00".into(),
            snippet: "draft body".into(),
        }
    }

    fn draft_summary(account_id: Uuid, draft_id: Uuid) -> app::DraftSummary {
        use chrono::Utc;
        app::DraftSummary {
            draft: crate::models::Draft {
                id: draft_id,
                account_id,
                in_reply_to_msg: None,
                to_addrs: json!(["bob@x.com"]),
                cc_addrs: json!([]),
                bcc_addrs: json!([]),
                subject: Some("Resume".into()),
                text_body: Some("partial body".into()),
                html_body: None,
                in_reply_to: None,
                references_header: None,
                remote_folder_id: None,
                remote_uid: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            },
            attachments: Vec::new(),
        }
    }

    #[tokio::test]
    async fn test_drafts_pane_enter_reopens_draft_into_composer() {
        let account_id = Uuid::new_v4();
        let drafts_id = Uuid::new_v4();
        let draft_id = Uuid::new_v4();
        let mut app = AppState::default();
        app.apply_accounts(vec![account_item(account_id)]);
        app.apply_folders(vec![drafts_folder_item(drafts_id)]);
        app.apply_drafts(vec![draft_item(draft_id, account_id, "Resume")]);
        app.active = ActivePane::Messages;

        let mut client = MockMailbox {
            draft_summary: Some(draft_summary(account_id, draft_id)),
            ..Default::default()
        };

        let _ = handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut app,
            &mut client,
        )
        .await;

        assert!(client.calls.contains(&Call::GetDraft(draft_id)));
        assert_eq!(app.mode, InputMode::Compose);
        let composer = app.composer.as_ref().expect("composer opened");
        assert_eq!(composer.draft_id, Some(draft_id));
        assert_eq!(composer.subject, "Resume");
        assert!(!composer.dirty);
    }

    #[tokio::test]
    async fn test_drafts_pane_d_then_y_deletes_via_daemon_and_removes_locally() {
        let account_id = Uuid::new_v4();
        let drafts_id = Uuid::new_v4();
        let draft_id = Uuid::new_v4();
        let other_id = Uuid::new_v4();
        let mut app = AppState::default();
        app.apply_accounts(vec![account_item(account_id)]);
        app.apply_folders(vec![drafts_folder_item(drafts_id)]);
        app.apply_drafts(vec![
            draft_item(draft_id, account_id, "to-delete"),
            draft_item(other_id, account_id, "keeper"),
        ]);
        app.active = ActivePane::Messages;
        let mut client = MockMailbox::default();

        let _ = handle_key(
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
            &mut app,
            &mut client,
        )
        .await;
        assert_eq!(app.mode, InputMode::ConfirmDelete);
        assert_eq!(app.pending_delete_draft, Some(draft_id));

        let _ = handle_key(
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
            &mut app,
            &mut client,
        )
        .await;
        assert!(client.calls.contains(&Call::DeleteDraft(draft_id)));
        assert_eq!(app.drafts.len(), 1);
        assert_eq!(app.drafts[0].id, other_id);
    }

    #[tokio::test]
    async fn test_drafts_pane_d_then_n_cancels_without_calling_daemon() {
        let account_id = Uuid::new_v4();
        let drafts_id = Uuid::new_v4();
        let draft_id = Uuid::new_v4();
        let mut app = AppState::default();
        app.apply_accounts(vec![account_item(account_id)]);
        app.apply_folders(vec![drafts_folder_item(drafts_id)]);
        app.apply_drafts(vec![draft_item(draft_id, account_id, "keep me")]);
        app.active = ActivePane::Messages;
        let mut client = MockMailbox::default();

        let _ = handle_key(
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
            &mut app,
            &mut client,
        )
        .await;
        let _ = handle_key(
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
            &mut app,
            &mut client,
        )
        .await;

        assert!(!client
            .calls
            .iter()
            .any(|c| matches!(c, Call::DeleteDraft(_))));
        assert_eq!(app.drafts.len(), 1);
        assert_eq!(app.mode, InputMode::Normal);
        assert!(app.pending_delete_draft.is_none());
    }

    #[tokio::test]
    async fn test_command_bar_w_inside_composer_calls_create_draft() {
        let account_id = Uuid::new_v4();
        let mut app = AppState::default();
        app.apply_accounts(vec![account_item(account_id)]);
        app.enter_composer(account_id);
        // Type a single char so the composer is non-empty.
        let mut client = MockMailbox {
            draft_id: Some(Uuid::new_v4()),
            ..Default::default()
        };
        let _ = handle_key(
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
            &mut app,
            &mut client,
        )
        .await;

        run_command_line("w".into(), &mut app, &mut client).await;

        assert!(matches!(client.calls.last(), Some(Call::CreateDraft(_))));
        assert_eq!(app.mode, InputMode::Compose);
        assert!(app.composer.is_some());
    }

    #[tokio::test]
    async fn test_command_bar_w_outside_composer_is_a_no_op_with_status() {
        let mut app = AppState::default();
        let mut client = MockMailbox::default();
        run_command_line("w".into(), &mut app, &mut client).await;
        assert!(client.calls.is_empty());
        assert!(app.status.contains(":w only valid"));
    }
}
