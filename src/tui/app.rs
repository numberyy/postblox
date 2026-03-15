use std::io;
use std::time::Duration;

use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event as CtEvent, EventStream, KeyEventKind,
    MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use ratatui::Terminal;
use tokio::sync::{mpsc, watch};
use uuid::Uuid;

use std::collections::HashMap;
use std::path::PathBuf;

use crate::client::{Approval, Attachment, Briefing, Draft, Inbox, Message, PostbloxClient};
use crate::components::approvals::ApprovalPanel;
use crate::components::briefing::BriefingPanel;
use crate::components::compose::Compose;
use crate::components::drafts::DraftPanel;
use crate::components::inbox_list::InboxList;
use crate::components::message_list::MessageList;
use crate::components::preview::AttachmentInfo;
use crate::components::preview::Preview;
use crate::components::search::SearchPanel;
use crate::components::status_bar::StatusBar;
use crate::components::thread_panel::{ThreadMessage, ThreadPanel};
use crate::config::{LayoutConfig, TuiConfig};
use crate::keys::{self, Action};
use crate::layout;
use crate::state::{Mode, Panel};
use crate::theme::Theme;
use crate::ws::{self, WsEvent};

fn msg_body_text(text_body: &Option<String>, html_body: &Option<String>) -> String {
    text_body
        .as_deref()
        .filter(|t| !t.is_empty())
        .map(String::from)
        .or_else(|| {
            html_body
                .as_deref()
                .filter(|h| !h.is_empty())
                .map(crate::components::preview::html_to_plaintext)
        })
        .unwrap_or_default()
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        // Best-effort terminal restore during panic/drop — nothing useful to do on failure.
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
    }
}

enum AppMsg {
    InboxesLoaded(Result<Vec<Inbox>, String>),
    MessagesLoaded(Result<Vec<Message>, String>),
    AllInboxMessagesLoaded(Result<Vec<Message>, String>),
    ApprovalsLoaded(Result<Vec<Approval>, String>),
    DraftsLoaded(Result<Vec<Draft>, String>),
    BriefingLoaded(Result<Briefing, String>),
    SearchResults(Result<Vec<Message>, String>),
    MessageSent(Result<(), String>),
    ApprovalActioned {
        id: Uuid,
        approved: bool,
        result: Result<(), String>,
    },
    ThreadLoaded(Result<Vec<Message>, String>),
    ApprovalMessageLoaded(Result<Message, String>),
    AttachmentsLoaded {
        message_id: Uuid,
        result: Result<Vec<Attachment>, String>,
    },
    AttachmentDownloaded(Result<String, String>),
    Ws(WsEvent),
}

enum Command {
    LoadInboxes,
    LoadMessages(Uuid),
    LoadAllInboxMessages {
        inbox_ids: Vec<Uuid>,
    },
    LoadApprovals,
    LoadDrafts(Uuid),
    LoadBriefing,
    SendMessage {
        inbox_id: Uuid,
        to: String,
        subject: String,
        body: String,
        attachments: Vec<PathBuf>,
    },
    Approve(Uuid),
    Reject(Uuid),
    Search(String),
    LoadThread {
        inbox_id: Uuid,
        thread_id: Uuid,
    },
    LoadApprovalMessage {
        inbox_id: Uuid,
        message_id: Uuid,
    },
    LoadAttachments {
        inbox_id: Uuid,
        message_id: Uuid,
    },
    DownloadAttachment {
        inbox_id: Uuid,
        message_id: Uuid,
        attachment_id: Uuid,
        filename: String,
        dest: PathBuf,
    },
}

pub struct App {
    // UI components
    inbox_list: InboxList,
    message_list: MessageList,
    preview: Preview,
    compose: Compose,
    approvals: ApprovalPanel,
    drafts: DraftPanel,
    search: SearchPanel,
    briefing: BriefingPanel,
    status_bar: StatusBar,
    thread_panel: ThreadPanel,

    // UI state
    theme: Theme,
    focus: Panel,
    mode: Mode,
    vim_mode: bool,
    sidebar_view: SidebarView,
    running: bool,

    // Data
    client: PostbloxClient,
    inboxes: Vec<Inbox>,
    messages: Vec<Message>,
    displayed_messages: Vec<Message>,
    approval_data: Vec<Approval>,
    selected_inbox_id: Option<Uuid>,
    show_slop: bool,
    status_text: Option<String>,
    thread_counts: HashMap<Uuid, usize>,

    // Async
    msg_tx: mpsc::Sender<AppMsg>,
    msg_rx: mpsc::Receiver<AppMsg>,
    ws_shutdown: watch::Sender<bool>,

    // Search
    search_deadline: Option<tokio::time::Instant>,
    pending_select_message: Option<Uuid>,

    // Layout cache for mouse handling
    last_layout: Option<layout::AppLayout>,

    // Attachments
    current_message_id: Option<Uuid>,
    current_attachments: Vec<Attachment>,
    known_attachment_messages: std::collections::HashSet<Uuid>,

    // Editor
    editor_requested: bool,

    // Config
    download_dir: PathBuf,
    keybinding_overrides: crate::config::KeybindingOverrides,
    layout_config: LayoutConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SidebarView {
    Inboxes,
    Approvals,
    Drafts,
    Briefing,
    Search,
}

impl App {
    pub fn new(config: &TuiConfig, client: PostbloxClient) -> Self {
        let (msg_tx, msg_rx) = mpsc::channel(64);
        let (ws_shutdown, _) = watch::channel(false);

        Self {
            inbox_list: InboxList::new(),
            message_list: MessageList::new(),
            preview: Preview::new(),
            compose: Compose::new(),
            approvals: ApprovalPanel::new(),
            drafts: DraftPanel::new(),
            search: SearchPanel::new(),
            briefing: BriefingPanel::new(),
            status_bar: StatusBar::new(config.vim_mode),
            thread_panel: ThreadPanel::new(),
            theme: Theme::from_name(&config.theme),
            focus: Panel::Sidebar,
            mode: Mode::Normal,
            vim_mode: config.vim_mode,
            sidebar_view: SidebarView::Inboxes,
            running: true,
            client,
            inboxes: Vec::new(),
            messages: Vec::new(),
            displayed_messages: Vec::new(),
            approval_data: Vec::new(),
            selected_inbox_id: None,
            show_slop: true,
            status_text: Some("Loading…".into()),
            thread_counts: HashMap::new(),
            msg_tx,
            msg_rx,
            ws_shutdown,
            search_deadline: None,
            pending_select_message: None,
            last_layout: None,
            current_message_id: None,
            current_attachments: Vec::new(),
            known_attachment_messages: std::collections::HashSet::new(),
            editor_requested: false,
            download_dir: config.download_dir.clone(),
            keybinding_overrides: config.keybindings.clone(),
            layout_config: config.layout,
        }
    }

    pub async fn run(&mut self) -> anyhow::Result<()> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let _guard = TerminalGuard;

        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        let mut event_stream = EventStream::new();
        let tick_rate = Duration::from_millis(250);

        self.dispatch(Command::LoadInboxes);
        self.dispatch(Command::LoadApprovals);
        self.dispatch(Command::LoadBriefing);
        self.spawn_ws();

        while self.running {
            terminal.draw(|frame| self.render(frame))?;

            let msg_rx = &mut self.msg_rx;

            enum Tick {
                Key(crossterm::event::KeyEvent),
                Mouse(crossterm::event::MouseEvent),
                Msg(Box<AppMsg>),
                Timeout,
            }

            let tick = tokio::select! {
                _ = tokio::time::sleep(tick_rate) => Tick::Timeout,
                maybe_event = event_stream.next() => {
                    match maybe_event {
                        Some(Ok(CtEvent::Key(key))) if key.kind == KeyEventKind::Press => Tick::Key(key),
                        Some(Ok(CtEvent::Mouse(mouse))) => Tick::Mouse(mouse),
                        Some(Ok(CtEvent::Resize(_, _))) => Tick::Timeout,
                        _ => Tick::Timeout,
                    }
                }
                Some(msg) = msg_rx.recv() => Tick::Msg(Box::new(msg)),
            };

            if let Some(deadline) = self.search_deadline {
                if tokio::time::Instant::now() >= deadline {
                    self.search_deadline = None;
                    let query = self.search.query.clone();
                    if !query.is_empty() {
                        self.dispatch(Command::Search(query));
                    }
                }
            }

            match tick {
                Tick::Key(key) => {
                    if let Some(cmd) = self.handle_key(key) {
                        self.dispatch(cmd);
                    }
                }
                Tick::Mouse(mouse) => {
                    if let Some(cmd) = self.handle_mouse(mouse) {
                        self.dispatch(cmd);
                    }
                }
                Tick::Msg(msg) => self.handle_msg(*msg),
                Tick::Timeout => {}
            }

            if self.editor_requested {
                self.editor_requested = false;
                self.spawn_editor(&mut terminal)?;
            }
        }

        // WS task may already be gone if it exited cleanly; ignore send error.
        let _ = self.ws_shutdown.send(true);

        Ok(())
    }

    fn spawn_ws(&self) {
        let ws_url = self.client.ws_url();
        let app_tx = self.msg_tx.clone();
        let shutdown_rx = self.ws_shutdown.subscribe();
        tokio::spawn(async move {
            let (ws_tx, mut ws_rx) = mpsc::channel::<WsEvent>(32);
            let ws_fut = ws::run(ws_url, ws_tx, shutdown_rx);
            tokio::pin!(ws_fut);
            loop {
                tokio::select! {
                    Some(ev) = ws_rx.recv() => {
                        if app_tx.send(AppMsg::Ws(ev)).await.is_err() {
                            break;
                        }
                    }
                    () = &mut ws_fut => break,
                }
            }
        });
    }

    // All tx.send() below use `let _ =` because the receiver being dropped means
    // the App is shutting down; there is nothing useful to do with the error.
    fn dispatch(&self, cmd: Command) {
        let client = self.client.clone();
        let tx = self.msg_tx.clone();
        tokio::spawn(async move {
            match cmd {
                Command::LoadInboxes => {
                    let result = client.list_inboxes().await.map_err(|e| e.to_string());
                    let _ = tx.send(AppMsg::InboxesLoaded(result)).await;
                }
                Command::LoadMessages(inbox_id) => {
                    let result = client
                        .list_messages(inbox_id, 50, 0)
                        .await
                        .map_err(|e| e.to_string());
                    let _ = tx.send(AppMsg::MessagesLoaded(result)).await;
                }
                Command::LoadApprovals => {
                    let result = client.list_approvals().await.map_err(|e| e.to_string());
                    let _ = tx.send(AppMsg::ApprovalsLoaded(result)).await;
                }
                Command::LoadBriefing => {
                    let result = client.briefing("24h").await.map_err(|e| e.to_string());
                    let _ = tx.send(AppMsg::BriefingLoaded(result)).await;
                }
                Command::SendMessage {
                    inbox_id,
                    to,
                    subject,
                    body,
                    attachments,
                } => {
                    let result = if attachments.is_empty() {
                        client.send_message(inbox_id, &to, &subject, &body).await
                    } else {
                        client
                            .send_message_with_attachments(
                                inbox_id,
                                &to,
                                &subject,
                                &body,
                                &attachments,
                            )
                            .await
                    };
                    let _ = tx
                        .send(AppMsg::MessageSent(
                            result.map(|_| ()).map_err(|e| e.to_string()),
                        ))
                        .await;
                }
                Command::Approve(id) => {
                    let result = client
                        .approve(id)
                        .await
                        .map(|_| ())
                        .map_err(|e| e.to_string());
                    let _ = tx
                        .send(AppMsg::ApprovalActioned {
                            id,
                            approved: true,
                            result,
                        })
                        .await;
                }
                Command::Reject(id) => {
                    let result = client
                        .reject(id)
                        .await
                        .map(|_| ())
                        .map_err(|e| e.to_string());
                    let _ = tx
                        .send(AppMsg::ApprovalActioned {
                            id,
                            approved: false,
                            result,
                        })
                        .await;
                }
                Command::Search(query) => {
                    let result = client.search(&query).await.map_err(|e| e.to_string());
                    let _ = tx.send(AppMsg::SearchResults(result)).await;
                }
                Command::LoadDrafts(inbox_id) => {
                    let result = client
                        .list_drafts(inbox_id)
                        .await
                        .map_err(|e| e.to_string());
                    let _ = tx.send(AppMsg::DraftsLoaded(result)).await;
                }
                Command::LoadApprovalMessage {
                    inbox_id,
                    message_id,
                } => {
                    let result = client
                        .get_message(inbox_id, message_id)
                        .await
                        .map_err(|e| e.to_string());
                    let _ = tx.send(AppMsg::ApprovalMessageLoaded(result)).await;
                }
                Command::LoadThread {
                    inbox_id,
                    thread_id,
                } => {
                    let result = client
                        .get_thread_messages(inbox_id, thread_id)
                        .await
                        .map_err(|e| e.to_string());
                    let _ = tx.send(AppMsg::ThreadLoaded(result)).await;
                }
                Command::LoadAllInboxMessages { inbox_ids } => {
                    let futs = inbox_ids.into_iter().map(|id| {
                        let c = client.clone();
                        async move { c.list_messages(id, 50, 0).await }
                    });
                    let results = futures::future::join_all(futs).await;
                    let mut all: Vec<Message> = Vec::new();
                    let mut failed = 0usize;
                    for result in results {
                        match result {
                            Ok(msgs) => all.extend(msgs),
                            Err(e) => {
                                tracing::warn!("failed to load inbox messages: {e}");
                                failed += 1;
                            }
                        }
                    }
                    if failed > 0 && all.is_empty() {
                        let _ = tx
                            .send(AppMsg::AllInboxMessagesLoaded(Err(format!(
                                "failed to load messages from {failed} inbox(es)"
                            ))))
                            .await;
                    } else {
                        all.sort_by(|a, b| b.created_at.cmp(&a.created_at));
                        let _ = tx.send(AppMsg::AllInboxMessagesLoaded(Ok(all))).await;
                    }
                }
                Command::LoadAttachments {
                    inbox_id,
                    message_id,
                } => {
                    let result = client
                        .list_attachments(inbox_id, message_id)
                        .await
                        .map_err(|e| e.to_string());
                    let _ = tx
                        .send(AppMsg::AttachmentsLoaded { message_id, result })
                        .await;
                }
                Command::DownloadAttachment {
                    inbox_id,
                    message_id,
                    attachment_id,
                    filename,
                    dest,
                } => {
                    let result = async {
                        let data = client
                            .get_attachment(inbox_id, message_id, attachment_id)
                            .await
                            .map_err(|e| e.to_string())?;
                        tokio::fs::create_dir_all(&dest)
                            .await
                            .map_err(|e| e.to_string())?;
                        let safe_name = std::path::Path::new(&filename)
                            .file_name()
                            .ok_or_else(|| "invalid filename".to_string())?;
                        let path = dest.join(safe_name);
                        tokio::fs::write(&path, &data)
                            .await
                            .map_err(|e| e.to_string())?;
                        Ok(path.display().to_string())
                    }
                    .await;
                    let _ = tx.send(AppMsg::AttachmentDownloaded(result)).await;
                }
            }
        });
    }

    fn handle_msg(&mut self, msg: AppMsg) {
        match msg {
            AppMsg::InboxesLoaded(Ok(inboxes)) => {
                self.inboxes = inboxes;
                let pending = self.approval_data.len();
                self.inbox_list.set_inboxes(&self.inboxes, pending);
                self.status_text = None;
                self.update_status_bar();
            }
            AppMsg::InboxesLoaded(Err(e)) => {
                self.status_text = Some(format!("Failed to load inboxes: {e}"));
                self.update_status_bar();
            }
            AppMsg::MessagesLoaded(Ok(messages)) => {
                self.messages = messages;
                self.refresh_message_display();
                if let Some(target_id) = self.pending_select_message.take() {
                    if let Some(idx) = self
                        .displayed_messages
                        .iter()
                        .position(|m| m.id == target_id)
                    {
                        self.message_list.state.select(Some(idx));
                        self.update_preview();
                    }
                }
                self.status_text = None;
                self.update_status_bar();
            }
            AppMsg::MessagesLoaded(Err(e)) => {
                self.status_text = Some(format!("Failed to load messages: {e}"));
                self.update_status_bar();
            }
            AppMsg::ApprovalsLoaded(Ok(approvals)) => {
                self.approval_data = approvals;
                self.approvals.set_entries(&self.approval_data);
                let pending = self.approval_data.len();
                self.inbox_list.set_inboxes(&self.inboxes, pending);
            }
            AppMsg::ApprovalsLoaded(Err(e)) => {
                self.status_text = Some(format!("Failed to load approvals: {e}"));
                self.update_status_bar();
            }
            AppMsg::DraftsLoaded(Ok(drafts)) => {
                self.drafts.set_entries(&drafts);
                self.status_text = None;
                self.update_status_bar();
            }
            AppMsg::DraftsLoaded(Err(e)) => {
                self.status_text = Some(format!("Failed to load drafts: {e}"));
                self.update_status_bar();
            }
            AppMsg::BriefingLoaded(Ok(briefing)) => {
                self.briefing.set_data(&briefing);
            }
            AppMsg::BriefingLoaded(Err(e)) => {
                self.status_text = Some(format!("Failed to load briefing: {e}"));
                self.update_status_bar();
            }
            AppMsg::SearchResults(Ok(messages)) => {
                self.search.set_results(&messages);
            }
            AppMsg::SearchResults(Err(e)) => {
                self.status_text = Some(format!("Search failed: {e}"));
                self.update_status_bar();
            }
            AppMsg::MessageSent(Ok(())) => {
                self.status_text = Some("Message sent".into());
                self.update_status_bar();
                if let Some(id) = self.selected_inbox_id {
                    self.dispatch(Command::LoadMessages(id));
                }
            }
            AppMsg::MessageSent(Err(e)) => {
                self.status_text = Some(format!("Send failed: {e}"));
                self.update_status_bar();
            }
            AppMsg::ApprovalActioned {
                id,
                approved,
                result: Ok(()),
            } => {
                let action = if approved { "Approved" } else { "Rejected" };
                self.status_text = Some(format!("{action} message"));
                self.update_status_bar();
                self.approval_data.retain(|a| a.id != id);
                self.approvals.set_entries(&self.approval_data);
                let pending = self.approval_data.len();
                self.inbox_list.set_inboxes(&self.inboxes, pending);
            }
            AppMsg::ApprovalActioned {
                approved,
                result: Err(e),
                ..
            } => {
                let action = if approved { "Approve" } else { "Reject" };
                self.status_text = Some(format!("{action} failed: {e}"));
                self.update_status_bar();
            }
            AppMsg::ApprovalMessageLoaded(Ok(msg)) => {
                if self.sidebar_view == SidebarView::Approvals {
                    let body = msg_body_text(&msg.text_body, &msg.html_body);
                    self.preview.set_content(
                        &msg.from_addr,
                        msg.subject.as_deref().unwrap_or("(no subject)"),
                        &msg.created_at.format("%Y-%m-%d %H:%M UTC").to_string(),
                        &body,
                    );
                }
            }
            AppMsg::ApprovalMessageLoaded(Err(e)) => {
                self.status_text = Some(format!("Failed to load approval message: {e}"));
                self.update_status_bar();
            }
            AppMsg::ThreadLoaded(Ok(messages)) => {
                let count = messages.len();
                let thread_msgs: Vec<ThreadMessage> = messages
                    .iter()
                    .map(|msg| ThreadMessage {
                        from: msg.from_addr.clone(),
                        date: msg.created_at.format("%Y-%m-%d %H:%M").to_string(),
                        body: msg_body_text(&msg.text_body, &msg.html_body),
                    })
                    .collect();
                self.thread_panel.set_messages(thread_msgs);
                self.mode = Mode::Thread;
                self.focus = Panel::Preview;

                if let Some(first) = messages.first() {
                    if let Some(thread_id) = first.thread_id {
                        self.thread_counts.insert(thread_id, count);
                        self.message_list.set_thread_count(
                            thread_id,
                            count,
                            &self.displayed_messages,
                        );
                    }
                }
            }
            AppMsg::ThreadLoaded(Err(e)) => {
                self.status_text = Some(format!("Failed to load thread: {e}"));
                self.update_status_bar();
            }
            AppMsg::AllInboxMessagesLoaded(Ok(messages)) => {
                self.messages = messages;
                self.refresh_message_display();
                let inbox_map: HashMap<Uuid, String> = self
                    .inboxes
                    .iter()
                    .map(|i| (i.id, i.email.clone()))
                    .collect();
                self.message_list
                    .set_inbox_labels_from_messages(&self.displayed_messages, &inbox_map);
                self.status_text = None;
                self.update_status_bar();
            }
            AppMsg::AllInboxMessagesLoaded(Err(e)) => {
                self.status_text = Some(format!("Failed to load messages: {e}"));
                self.update_status_bar();
            }
            AppMsg::AttachmentsLoaded { message_id, result } => match result {
                Ok(attachments) => {
                    if !attachments.is_empty() {
                        self.known_attachment_messages.insert(message_id);
                        self.message_list
                            .mark_has_attachments(message_id, &self.displayed_messages);
                    }
                    if self.current_message_id == Some(message_id) {
                        self.current_attachments = attachments.clone();
                        let infos = attachments
                            .into_iter()
                            .map(|a| AttachmentInfo {
                                id: a.id,
                                filename: a.filename,
                                content_type: a.content_type,
                                size_bytes: a.size_bytes,
                            })
                            .collect();
                        self.preview.set_attachments(infos);
                    }
                }
                Err(e) => {
                    tracing::warn!("failed to load attachments: {e}");
                }
            },
            AppMsg::AttachmentDownloaded(Ok(path)) => {
                self.status_text = Some(format!("Saved to {path}"));
                self.update_status_bar();
            }
            AppMsg::AttachmentDownloaded(Err(e)) => {
                self.status_text = Some(format!("Download failed: {e}"));
                self.update_status_bar();
            }
            AppMsg::Ws(WsEvent::Connected) => {
                self.status_bar.connected = true;
            }
            AppMsg::Ws(WsEvent::Disconnected) => {
                self.status_bar.connected = false;
            }
            AppMsg::Ws(WsEvent::MessageReceived { inbox_id }) => {
                if self.selected_inbox_id == Some(inbox_id) {
                    self.dispatch(Command::LoadMessages(inbox_id));
                }
            }
            AppMsg::Ws(WsEvent::ApprovalRequested) => {
                self.dispatch(Command::LoadApprovals);
            }
            AppMsg::Ws(WsEvent::TrustChanged) => {
                self.dispatch(Command::LoadApprovals);
            }
        }
    }

    fn refresh_message_display(&mut self) {
        self.displayed_messages = if self.show_slop {
            self.messages.clone()
        } else {
            self.messages
                .iter()
                .filter(|m| m.triage_status.as_deref() != Some("slopified"))
                .cloned()
                .collect()
        };
        self.message_list.set_entries(&self.displayed_messages);
        self.update_preview();
    }

    fn update_preview(&mut self) {
        match self.sidebar_view {
            SidebarView::Inboxes => {
                let idx = self.message_list.selected();
                if let Some(msg) = self.displayed_messages.get(idx) {
                    let body = msg_body_text(&msg.text_body, &msg.html_body);
                    self.preview.set_content(
                        &msg.from_addr,
                        msg.subject.as_deref().unwrap_or("(no subject)"),
                        &msg.created_at.format("%Y-%m-%d %H:%M UTC").to_string(),
                        &body,
                    );
                    self.current_message_id = Some(msg.id);
                    self.dispatch(Command::LoadAttachments {
                        inbox_id: msg.inbox_id,
                        message_id: msg.id,
                    });
                } else {
                    self.preview
                        .set_content("", "", "", "Select an inbox to view messages");
                    self.current_message_id = None;
                }
            }
            SidebarView::Approvals => {
                let idx = self.approvals.selected();
                if let Some(approval) = self.approval_data.get(idx) {
                    self.preview.set_content(
                        approval.from_addr.as_deref().unwrap_or(""),
                        approval.subject.as_deref().unwrap_or("(no subject)"),
                        &approval.created_at.format("%Y-%m-%d %H:%M UTC").to_string(),
                        "Press y to approve, n to reject",
                    );
                    self.current_message_id = Some(approval.message_id);
                    self.dispatch(Command::LoadApprovalMessage {
                        inbox_id: approval.inbox_id,
                        message_id: approval.message_id,
                    });
                }
            }
            _ => {
                self.current_message_id = None;
            }
        }
    }

    fn update_status_bar(&mut self) {
        if let Some(ref text) = self.status_text {
            self.status_bar.inbox_name = text.clone();
            self.status_bar.inbox_count = 0;
        } else {
            match self.sidebar_view {
                SidebarView::Inboxes => {
                    if let Some(inbox_id) = self.selected_inbox_id {
                        if let Some(inbox) = self.inboxes.iter().find(|i| i.id == inbox_id) {
                            self.status_bar.inbox_name = inbox.email.clone();
                        }
                        self.status_bar.inbox_count = self.displayed_messages.len();
                    } else {
                        self.status_bar.inbox_name = "All Inboxes".into();
                        self.status_bar.inbox_count = self.inboxes.len();
                    }
                }
                SidebarView::Approvals => {
                    self.status_bar.inbox_name = "Approvals".into();
                    self.status_bar.inbox_count = self.approval_data.len();
                }
                SidebarView::Drafts => {
                    self.status_bar.inbox_name = "Drafts".into();
                    self.status_bar.inbox_count = self.drafts.entries.len();
                }
                SidebarView::Briefing => {
                    self.status_bar.inbox_name = "Briefing".into();
                    self.status_bar.inbox_count = 0;
                }
                SidebarView::Search => {
                    self.status_bar.inbox_name = "Search".into();
                    self.status_bar.inbox_count = self.search.results.len();
                }
            }
        }
    }

    fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Option<Command> {
        if self.mode == Mode::Thread {
            use crossterm::event::KeyCode;
            if key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL)
                && key.code == KeyCode::Char('c')
            {
                self.running = false;
                return None;
            }
            let action = keys::resolve_with_overrides(
                key,
                Mode::Normal,
                self.focus,
                self.vim_mode,
                Some(&self.keybinding_overrides),
            );
            if let Some(action) = action {
                match action {
                    Action::MoveUp => {
                        self.thread_panel.select_prev();
                    }
                    Action::MoveDown => {
                        self.thread_panel.select_next();
                    }
                    Action::MoveTop => {
                        self.thread_panel.select_first();
                    }
                    Action::MoveBottom => {
                        self.thread_panel.select_last();
                    }
                    Action::Back | Action::Quit => {
                        self.thread_panel.clear();
                        self.mode = Mode::Normal;
                        self.focus = Panel::MessageList;
                    }
                    _ => {}
                }
            }
            // Bracket keys for body scroll
            if let KeyCode::Char('[') = key.code {
                self.thread_panel.scroll_up();
            }
            if let KeyCode::Char(']') = key.code {
                self.thread_panel.scroll_down();
            }
            return None;
        }

        if self.mode == Mode::Compose {
            if let Some(action) = keys::resolve_with_overrides(
                key,
                self.mode,
                self.focus,
                self.vim_mode,
                Some(&self.keybinding_overrides),
            ) {
                match action {
                    Action::Back => {
                        self.compose.reset();
                        self.mode = Mode::Normal;
                    }
                    Action::Send => {
                        let cmd = self.build_send_command();
                        self.compose.reset();
                        self.mode = Mode::Normal;
                        if cmd.is_none() {
                            self.status_text = Some("Select an inbox first".into());
                            self.update_status_bar();
                        }
                        return cmd;
                    }
                    Action::OpenEditor => {
                        self.editor_requested = true;
                    }
                    Action::AddAttachment => {
                        self.compose.start_attachment_input();
                    }
                    Action::RemoveAttachment => {
                        self.compose.remove_last_attachment();
                    }
                    Action::Quit => self.running = false,
                    _ => {}
                }
            } else {
                self.compose.handle_key(key);
            }
            return None;
        }

        if self.mode == Mode::Search {
            if let Some(action) = keys::resolve_with_overrides(
                key,
                self.mode,
                self.focus,
                self.vim_mode,
                Some(&self.keybinding_overrides),
            ) {
                match action {
                    Action::Back => {
                        self.search.clear();
                        self.mode = Mode::Normal;
                        self.sidebar_view = SidebarView::Inboxes;
                    }
                    Action::Select => {
                        if let Some(result) = self.search.selected_result() {
                            let inbox_id = result.inbox_id;
                            let message_id = result.id;
                            self.selected_inbox_id = Some(inbox_id);
                            self.pending_select_message = Some(message_id);
                            if let Some(pos) = self.inboxes.iter().position(|i| i.id == inbox_id) {
                                self.inbox_list.select(pos + 1);
                            }
                            self.mode = Mode::Normal;
                            self.sidebar_view = SidebarView::Inboxes;
                            self.focus = Panel::MessageList;
                            self.status_text = Some("Loading…".into());
                            self.update_status_bar();
                            return Some(Command::LoadMessages(inbox_id));
                        }
                        self.mode = Mode::Normal;
                    }
                    Action::MoveUp => self.search.select_prev(),
                    Action::MoveDown => self.search.select_next(),
                    Action::Quit => self.running = false,
                    _ => {}
                }
            } else {
                use crossterm::event::KeyCode;
                match key.code {
                    KeyCode::Char(c) => {
                        self.search.push_char(c);
                        self.search_deadline =
                            Some(tokio::time::Instant::now() + Duration::from_millis(300));
                    }
                    KeyCode::Backspace => {
                        self.search.pop_char();
                        if self.search.query.is_empty() {
                            self.search_deadline = None;
                            self.search.set_results(&[]);
                        } else {
                            self.search_deadline =
                                Some(tokio::time::Instant::now() + Duration::from_millis(300));
                        }
                    }
                    _ => {}
                }
            }
            return None;
        }

        let action = keys::resolve_with_overrides(
            key,
            self.mode,
            self.focus,
            self.vim_mode,
            Some(&self.keybinding_overrides),
        )?;

        match action {
            Action::Quit => self.running = false,
            Action::MoveUp => {
                self.move_up();
                self.update_preview();
            }
            Action::MoveDown => {
                self.move_down();
                self.update_preview();
            }
            Action::MoveTop => {
                self.move_top();
                self.update_preview();
            }
            Action::MoveBottom => {
                self.move_bottom();
                self.update_preview();
            }
            Action::PanelLeft => self.cycle_focus_back(),
            Action::PanelRight => {
                if self.focus == Panel::Sidebar {
                    // If current inbox is already loaded, just move focus
                    let idx = self.inbox_list.logical_selected();
                    let inboxes_count = self.inbox_list.inbox_count();
                    if idx > 0 && idx < inboxes_count {
                        let inbox_id = self.inboxes.get(idx - 1).map(|i| i.id);
                        if inbox_id == self.selected_inbox_id
                            && self.sidebar_view == SidebarView::Inboxes
                        {
                            self.focus = Panel::MessageList;
                            return None;
                        }
                    }
                    return self.handle_select();
                }
                self.cycle_focus();
            }
            Action::CyclePanel => self.cycle_focus(),
            Action::CyclePanelBack => self.cycle_focus_back(),
            Action::Select => return self.handle_select(),
            Action::Back => self.handle_back(),
            Action::Compose => {
                self.compose.reset();
                self.mode = Mode::Compose;
            }
            Action::Reply => {
                let (to, subj) = (self.preview.from.clone(), self.preview.subject.clone());
                self.compose = Compose::new_reply(&to, &subj);
                self.mode = Mode::Compose;
            }
            Action::StartSearch => {
                self.search.clear();
                self.mode = Mode::Search;
                self.sidebar_view = SidebarView::Search;
            }
            Action::Send => {}
            Action::ShowHelp => self.mode = Mode::Help,
            Action::ShowBriefing => {
                self.sidebar_view = SidebarView::Briefing;
                self.focus = Panel::MessageList;
                self.preview.set_content("", "", "", "");
                self.status_text = Some("Briefing".into());
                self.update_status_bar();
                return Some(Command::LoadBriefing);
            }
            Action::ShowAllInboxes => {
                return self.load_all_inboxes();
            }
            Action::SlopToggle => {
                self.show_slop = !self.show_slop;
                self.refresh_message_display();
            }
            Action::Refresh => return self.handle_refresh(),
            Action::ApproveSelected => return self.handle_approve(),
            Action::RejectSelected => return self.handle_reject(),
            Action::QuickJump(n) => {
                let idx = n as usize;
                self.inbox_list.select(idx);
                if self.focus == Panel::Sidebar {
                    return self.handle_select();
                }
            }
            Action::OpenEditor | Action::AddAttachment | Action::RemoveAttachment => {}
            Action::DownloadAttachment => {
                return self.handle_download_attachment(false);
            }
            Action::OpenAttachment => {
                return self.handle_download_attachment(true);
            }
            Action::NextAttachment => {
                self.preview.select_next_attachment();
            }
            Action::PrevAttachment => {
                self.preview.select_prev_attachment();
            }
        }

        None
    }

    fn build_send_command(&self) -> Option<Command> {
        let inbox_id = self.selected_inbox_id?;
        let to = self.compose.to.trim().to_string();
        let subject = self.compose.subject.trim().to_string();
        let body = self.compose.body_text();
        if to.is_empty() {
            return None;
        }
        Some(Command::SendMessage {
            inbox_id,
            to,
            subject,
            body,
            attachments: self.compose.attachments.clone(),
        })
    }

    fn handle_approve(&self) -> Option<Command> {
        if self.sidebar_view != SidebarView::Approvals {
            return None;
        }
        let idx = self.approvals.selected();
        let approval = self.approval_data.get(idx)?;
        Some(Command::Approve(approval.id))
    }

    fn handle_reject(&self) -> Option<Command> {
        if self.sidebar_view != SidebarView::Approvals {
            return None;
        }
        let idx = self.approvals.selected();
        let approval = self.approval_data.get(idx)?;
        Some(Command::Reject(approval.id))
    }

    fn load_all_inboxes(&mut self) -> Option<Command> {
        self.sidebar_view = SidebarView::Inboxes;
        self.selected_inbox_id = None;
        self.inbox_list.select_first();
        self.focus = Panel::MessageList;
        if self.inboxes.is_empty() {
            self.messages.clear();
            self.refresh_message_display();
            self.status_text = None;
            self.update_status_bar();
            return None;
        }
        self.status_text = Some("Loading all inboxes…".into());
        self.update_status_bar();
        let inbox_ids: Vec<Uuid> = self.inboxes.iter().map(|i| i.id).collect();
        Some(Command::LoadAllInboxMessages { inbox_ids })
    }

    fn handle_download_attachment(&mut self, open_after: bool) -> Option<Command> {
        let att = self.preview.selected_attachment()?;
        let inbox_id = self.current_message_id.and_then(|mid| {
            self.displayed_messages
                .iter()
                .find(|m| m.id == mid)
                .map(|m| m.inbox_id)
        })?;
        let message_id = self.current_message_id?;
        let attachment_id = att.id;
        let filename = att.filename.clone();
        let dest = self.download_dir.clone();
        self.status_text = Some(format!(
            "{}…",
            if open_after { "Opening" } else { "Downloading" }
        ));
        self.update_status_bar();
        if open_after {
            let client = self.client.clone();
            let tx = self.msg_tx.clone();
            tokio::spawn(async move {
                let result = async {
                    let data = client
                        .get_attachment(inbox_id, message_id, attachment_id)
                        .await
                        .map_err(|e| e.to_string())?;
                    tokio::fs::create_dir_all(&dest)
                        .await
                        .map_err(|e| e.to_string())?;
                    let path = dest.join(&filename);
                    tokio::fs::write(&path, &data)
                        .await
                        .map_err(|e| e.to_string())?;
                    let opener = if cfg!(target_os = "macos") {
                        "open"
                    } else if cfg!(target_os = "windows") {
                        "start"
                    } else {
                        "xdg-open"
                    };
                    tokio::process::Command::new(opener)
                        .arg(&path)
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn()
                        .map_err(|e| format!("failed to open with {opener}: {e}"))?;
                    Ok(path.display().to_string())
                }
                .await;
                let _ = tx.send(AppMsg::AttachmentDownloaded(result)).await;
            });
            return None;
        }
        Some(Command::DownloadAttachment {
            inbox_id,
            message_id,
            attachment_id,
            filename,
            dest,
        })
    }

    fn spawn_editor(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> anyhow::Result<()> {
        let body = self.compose.body_text();
        let tmp_dir = std::env::temp_dir();
        let tmp_path = tmp_dir.join(format!("postblox-compose-{}.txt", std::process::id()));

        std::fs::write(&tmp_path, &body)?;

        // Suspend TUI
        disable_raw_mode()?;
        execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen)?;

        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".into());
        let status = std::process::Command::new(&editor).arg(&tmp_path).status();

        // Resume TUI regardless of editor result
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
        terminal.clear()?;

        match status {
            Ok(s) if s.success() => match std::fs::read_to_string(&tmp_path) {
                Ok(content) => {
                    self.compose.set_body_text(&content);
                    self.status_text = Some("Editor content loaded".into());
                }
                Err(e) => {
                    self.status_text = Some(format!("Failed to read editor output: {e}"));
                }
            },
            Ok(_) => {
                self.status_text = Some("Editor exited with error".into());
            }
            Err(e) => {
                self.status_text = Some(format!("Failed to launch editor: {e}"));
            }
        }
        self.update_status_bar();
        // Best-effort cleanup — temp file will be cleaned by OS eventually.
        let _ = std::fs::remove_file(&tmp_path);
        Ok(())
    }

    fn handle_refresh(&mut self) -> Option<Command> {
        self.status_text = Some("Refreshing…".into());
        self.update_status_bar();
        match self.sidebar_view {
            SidebarView::Inboxes => {
                self.dispatch(Command::LoadInboxes);
                if let Some(id) = self.selected_inbox_id {
                    return Some(Command::LoadMessages(id));
                } else if !self.inboxes.is_empty() {
                    let inbox_ids: Vec<Uuid> = self.inboxes.iter().map(|i| i.id).collect();
                    return Some(Command::LoadAllInboxMessages { inbox_ids });
                }
            }
            SidebarView::Approvals => return Some(Command::LoadApprovals),
            SidebarView::Drafts => {
                if let Some(id) = self
                    .selected_inbox_id
                    .or_else(|| self.inboxes.first().map(|i| i.id))
                {
                    return Some(Command::LoadDrafts(id));
                }
            }
            SidebarView::Briefing => return Some(Command::LoadBriefing),
            SidebarView::Search => {
                let query = self.search.query.clone();
                if !query.is_empty() {
                    return Some(Command::Search(query));
                }
            }
        }
        None
    }

    fn move_up(&mut self) {
        match self.focus {
            Panel::Sidebar => self.inbox_list.select_prev(),
            Panel::MessageList => match self.sidebar_view {
                SidebarView::Approvals => self.approvals.select_prev(),
                SidebarView::Drafts => self.drafts.select_prev(),
                SidebarView::Search => self.search.select_prev(),
                _ => self.message_list.select_prev(),
            },
            Panel::Preview => match self.sidebar_view {
                SidebarView::Briefing => self.briefing.scroll_up(),
                _ => self.preview.scroll_up(),
            },
        }
    }

    fn move_down(&mut self) {
        match self.focus {
            Panel::Sidebar => self.inbox_list.select_next(),
            Panel::MessageList => match self.sidebar_view {
                SidebarView::Approvals => self.approvals.select_next(),
                SidebarView::Drafts => self.drafts.select_next(),
                SidebarView::Search => self.search.select_next(),
                _ => self.message_list.select_next(),
            },
            Panel::Preview => match self.sidebar_view {
                SidebarView::Briefing => self.briefing.scroll_down(),
                _ => self.preview.scroll_down(),
            },
        }
    }

    fn move_top(&mut self) {
        match self.focus {
            Panel::Sidebar => self.inbox_list.select_first(),
            Panel::MessageList => self.message_list.select_first(),
            Panel::Preview => self.preview.scroll = 0,
        }
    }

    fn move_bottom(&mut self) {
        match self.focus {
            Panel::Sidebar => self.inbox_list.select_last(),
            Panel::MessageList => self.message_list.select_last(),
            Panel::Preview => {
                self.preview.scroll = self.preview.body.lines().count().saturating_sub(1) as u16;
            }
        }
    }

    fn cycle_focus(&mut self) {
        self.focus = match self.focus {
            Panel::Sidebar => Panel::MessageList,
            Panel::MessageList => Panel::Preview,
            Panel::Preview => Panel::Sidebar,
        };
    }

    fn cycle_focus_back(&mut self) {
        self.focus = match self.focus {
            Panel::Sidebar => Panel::Preview,
            Panel::MessageList => Panel::Sidebar,
            Panel::Preview => Panel::MessageList,
        };
    }

    fn handle_select(&mut self) -> Option<Command> {
        if self.focus == Panel::Sidebar {
            let idx = self.inbox_list.logical_selected();
            let inboxes_count = self.inbox_list.inbox_count();
            if idx < inboxes_count {
                if idx == 0 {
                    return self.load_all_inboxes();
                } else {
                    let inbox_id = self.inboxes.get(idx - 1).map(|i| i.id);
                    if let Some(id) = inbox_id {
                        self.selected_inbox_id = Some(id);
                        self.status_text = Some("Loading…".into());
                        self.update_status_bar();
                        self.sidebar_view = SidebarView::Inboxes;
                        self.focus = Panel::MessageList;
                        return Some(Command::LoadMessages(id));
                    }
                }
                self.sidebar_view = SidebarView::Inboxes;
                self.focus = Panel::MessageList;
                return None;
            } else {
                match idx - inboxes_count {
                    0 => {
                        self.sidebar_view = SidebarView::Approvals;
                        self.focus = Panel::MessageList;
                        self.preview
                            .set_content("", "", "", "Select an approval to preview");
                        self.status_text = Some("Approvals".into());
                        self.update_status_bar();
                        return Some(Command::LoadApprovals);
                    }
                    1 => {
                        self.sidebar_view = SidebarView::Drafts;
                        self.focus = Panel::MessageList;
                        self.preview
                            .set_content("", "", "", "Select a draft to preview");
                        self.status_text = Some("Drafts".into());
                        self.update_status_bar();
                        if let Some(id) = self
                            .selected_inbox_id
                            .or_else(|| self.inboxes.first().map(|i| i.id))
                        {
                            return Some(Command::LoadDrafts(id));
                        }
                    }
                    2 => {
                        self.sidebar_view = SidebarView::Briefing;
                        self.focus = Panel::MessageList;
                        self.preview.set_content("", "", "", "");
                        self.status_text = Some("Briefing".into());
                        self.update_status_bar();
                        return Some(Command::LoadBriefing);
                    }
                    3 => {
                        self.mode = Mode::Search;
                        self.sidebar_view = SidebarView::Search;
                        self.preview.set_content("", "", "", "");
                        self.status_text = Some("Search".into());
                        self.update_status_bar();
                    }
                    _ => {}
                }
            }
        }

        if self.focus == Panel::MessageList && self.sidebar_view == SidebarView::Inboxes {
            let idx = self.message_list.selected();
            if let Some(msg) = self.displayed_messages.get(idx) {
                if let (Some(thread_id), Some(inbox_id)) = (msg.thread_id, self.selected_inbox_id) {
                    return Some(Command::LoadThread {
                        inbox_id,
                        thread_id,
                    });
                }
                self.focus = Panel::Preview;
            }
        }

        None
    }

    fn handle_back(&mut self) {
        if self.mode == Mode::Help {
            self.mode = Mode::Normal;
            return;
        }
        match self.sidebar_view {
            SidebarView::Search => {
                self.search.clear();
                self.sidebar_view = SidebarView::Inboxes;
                self.focus = Panel::Sidebar;
                self.status_text = None;
                self.update_status_bar();
            }
            SidebarView::Approvals | SidebarView::Drafts | SidebarView::Briefing => {
                self.sidebar_view = SidebarView::Inboxes;
                self.focus = Panel::Sidebar;
                self.status_text = None;
                self.update_status_bar();
            }
            SidebarView::Inboxes => {
                if self.focus != Panel::Sidebar {
                    self.focus = Panel::Sidebar;
                }
            }
        }
    }

    fn handle_mouse(&mut self, event: crossterm::event::MouseEvent) -> Option<Command> {
        let layout = self.last_layout?;
        let col = event.column;
        let row = event.row;

        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if rect_contains(layout.sidebar, col, row) {
                    self.focus = Panel::Sidebar;
                    let border_y = layout.sidebar.y + 1;
                    if row > border_y {
                        let inner_y = (row - border_y) as usize;
                        let visual_idx = inner_y + self.inbox_list.state.offset();
                        if visual_idx < self.inbox_list.items.len() {
                            self.inbox_list.state.select(Some(visual_idx));
                            return self.handle_select();
                        }
                    }
                } else if rect_contains(layout.message_list, col, row) {
                    self.focus = Panel::MessageList;
                    let border_y = layout.message_list.y + 1;
                    if row > border_y {
                        let inner_y = (row - border_y) as usize;
                        match self.sidebar_view {
                            SidebarView::Inboxes => {
                                let idx = inner_y + self.message_list.state.offset();
                                if idx < self.displayed_messages.len() {
                                    self.message_list.state.select(Some(idx));
                                    self.update_preview();
                                }
                            }
                            SidebarView::Approvals => {
                                let idx = inner_y + self.approvals.state.offset();
                                if idx < self.approval_data.len() {
                                    self.approvals.state.select(Some(idx));
                                    self.update_preview();
                                }
                            }
                            SidebarView::Search => {
                                let idx = inner_y + self.search.state.offset();
                                if idx < self.search.results.len() {
                                    self.search.state.select(Some(idx));
                                }
                            }
                            _ => {}
                        }
                    }
                } else if rect_contains(layout.preview, col, row) {
                    self.focus = Panel::Preview;
                }
            }
            MouseEventKind::ScrollUp => {
                if rect_contains(layout.preview, col, row) {
                    self.preview.scroll_up();
                } else if rect_contains(layout.sidebar, col, row) {
                    self.inbox_list.select_prev();
                } else if rect_contains(layout.message_list, col, row) {
                    self.move_up();
                    self.update_preview();
                }
            }
            MouseEventKind::ScrollDown => {
                if rect_contains(layout.preview, col, row) {
                    self.preview.scroll_down();
                } else if rect_contains(layout.sidebar, col, row) {
                    self.inbox_list.select_next();
                } else if rect_contains(layout.message_list, col, row) {
                    self.move_down();
                    self.update_preview();
                }
            }
            _ => {}
        }
        None
    }

    fn render(&mut self, frame: &mut ratatui::Frame) {
        let theme = &self.theme;
        let area = frame.area();
        let layout = layout::compute_with_config(area, &self.layout_config);
        self.last_layout = Some(layout);

        self.inbox_list
            .render(frame, layout.sidebar, theme, self.focus == Panel::Sidebar);

        if self.mode == Mode::Compose {
            // Compose takes full right panel (message_list + preview area)
            let compose_area = Rect::new(
                layout.message_list.x,
                layout.message_list.y,
                area.width.saturating_sub(layout.message_list.x),
                layout.status_bar.y.saturating_sub(layout.message_list.y),
            );
            self.compose.render(frame, compose_area, theme);
        } else {
            match self.sidebar_view {
                SidebarView::Inboxes => {
                    self.message_list.render(
                        frame,
                        layout.message_list,
                        theme,
                        self.focus == Panel::MessageList,
                    );
                }
                SidebarView::Approvals => {
                    self.approvals.render(
                        frame,
                        layout.message_list,
                        theme,
                        self.focus == Panel::MessageList,
                    );
                }
                SidebarView::Drafts => {
                    self.drafts.render(
                        frame,
                        layout.message_list,
                        theme,
                        self.focus == Panel::MessageList,
                    );
                }
                SidebarView::Briefing => {
                    self.briefing.render(
                        frame,
                        layout.message_list,
                        theme,
                        self.focus == Panel::MessageList,
                    );
                }
                SidebarView::Search => {
                    self.search.render_results(
                        frame,
                        layout.message_list,
                        theme,
                        self.focus == Panel::MessageList,
                        self.mode == Mode::Search,
                    );
                }
            }

            if self.mode == Mode::Thread {
                self.thread_panel.render(frame, layout.preview, theme, true);
            } else {
                self.preview
                    .render(frame, layout.preview, theme, self.focus == Panel::Preview);
            }
        }

        self.status_bar
            .render(frame, layout.status_bar, theme, self.mode);

        if self.mode == Mode::Help {
            render_help_overlay(frame, area, theme);
        }
    }
}

fn render_help_overlay(frame: &mut ratatui::Frame, area: Rect, theme: &Theme) {
    let help_w = 50.min(area.width.saturating_sub(4));
    let help_h = 32.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(help_w)) / 2;
    let y = (area.height.saturating_sub(help_h)) / 2;
    let help_area = Rect::new(x, y, help_w, help_h);

    let block = Block::default()
        .title(" Keybindings ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent))
        .style(Style::default().bg(theme.bg));

    let text = vec![
        "",
        "  Navigation",
        "  ↑/↓ or j/k     Move up/down",
        "  Tab/Shift+Tab   Cycle panels",
        "  h/l or ←/→     Switch panels",
        "  g/G             Top/bottom",
        "  1-9             Quick jump to inbox",
        "",
        "  Actions",
        "  Enter           Select / open thread",
        "  Esc             Back",
        "  c or Ctrl+N     Compose",
        "  r or Ctrl+R     Reply",
        "  / or Ctrl+F     Search",
        "  Ctrl+Enter      Send message",
        "  Ctrl+E          Open $EDITOR (compose)",
        "  y/n             Approve/reject",
        "  R               Refresh",
        "  s               Toggle slop filter",
        "  b               Briefing",
        "  a               All Inboxes",
        "  q or Ctrl+C     Quit",
        "",
        "  Thread view",
        "  j/k             Navigate messages",
        "  [/]             Scroll message body",
        "  q/Esc           Exit thread view",
        "",
        "  Attachments (preview panel)",
        "  [/]             Prev/next attachment",
        "  d               Download attachment",
        "  o               Open attachment",
    ];

    let lines: Vec<Line> = text
        .iter()
        .map(|s| Line::from(Span::styled(*s, Style::default().fg(theme.fg))))
        .collect();

    let p = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, help_area);
    frame.render_widget(p, help_area);
}

fn rect_contains(rect: Rect, col: u16, row: u16) -> bool {
    col >= rect.x && col < rect.x + rect.width && row >= rect.y && row < rect.y + rect.height
}
