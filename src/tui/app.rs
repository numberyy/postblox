use std::io;
use std::time::Duration;

use crossterm::event::{Event as CtEvent, EventStream, KeyEventKind};
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

use crate::client::{Approval, Briefing, Inbox, Message, PostbloxClient};
use crate::components::approvals::ApprovalPanel;
use crate::components::briefing::BriefingPanel;
use crate::components::compose::{Compose, ComposeField};
use crate::components::inbox_list::InboxList;
use crate::components::message_list::MessageList;
use crate::components::preview::Preview;
use crate::components::search::SearchPanel;
use crate::components::status_bar::StatusBar;
use crate::config::TuiConfig;
use crate::keys::{self, Action};
use crate::layout;
use crate::state::{Mode, Panel};
use crate::theme::Theme;
use crate::ws::{self, WsEvent};

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        // Best-effort terminal restore during panic/drop — nothing useful to do on failure.
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

enum AppMsg {
    InboxesLoaded(Result<Vec<Inbox>, String>),
    MessagesLoaded(Result<Vec<Message>, String>),
    ApprovalsLoaded(Result<Vec<Approval>, String>),
    BriefingLoaded(Result<Briefing, String>),
    SearchResults(Result<Vec<Message>, String>),
    MessageSent(Result<(), String>),
    ApprovalActioned {
        id: Uuid,
        approved: bool,
        result: Result<(), String>,
    },
    ThreadLoaded(Result<Vec<Message>, String>),
    Ws(WsEvent),
}

enum Command {
    LoadInboxes,
    LoadMessages(Uuid),
    LoadApprovals,
    LoadBriefing,
    SendMessage {
        inbox_id: Uuid,
        to: String,
        subject: String,
        body: String,
    },
    Approve(Uuid),
    Reject(Uuid),
    Search(String),
    LoadThread {
        inbox_id: Uuid,
        thread_id: Uuid,
    },
}

pub struct App {
    // UI components
    inbox_list: InboxList,
    message_list: MessageList,
    preview: Preview,
    compose: Compose,
    approvals: ApprovalPanel,
    search: SearchPanel,
    briefing: BriefingPanel,
    status_bar: StatusBar,

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

    // Async
    msg_tx: mpsc::Sender<AppMsg>,
    msg_rx: mpsc::Receiver<AppMsg>,
    ws_shutdown: watch::Sender<bool>,

    // Search debounce
    search_deadline: Option<tokio::time::Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SidebarView {
    Inboxes,
    Approvals,
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
            search: SearchPanel::new(),
            briefing: BriefingPanel::new(),
            status_bar: StatusBar::new(config.vim_mode),
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
            msg_tx,
            msg_rx,
            ws_shutdown,
            search_deadline: None,
        }
    }

    pub async fn run(&mut self) -> anyhow::Result<()> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let _guard = TerminalGuard;

        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        let mut event_stream = EventStream::new();
        let tick_rate = Duration::from_millis(250);

        // Load initial data
        self.dispatch(Command::LoadInboxes);
        self.dispatch(Command::LoadApprovals);
        self.dispatch(Command::LoadBriefing);
        self.spawn_ws();

        while self.running {
            terminal.draw(|frame| self.render(frame))?;

            let msg_rx = &mut self.msg_rx;

            enum Tick {
                Key(crossterm::event::KeyEvent),
                Msg(AppMsg),
                Timeout,
            }

            let tick = tokio::select! {
                _ = tokio::time::sleep(tick_rate) => Tick::Timeout,
                maybe_event = event_stream.next() => {
                    match maybe_event {
                        Some(Ok(CtEvent::Key(key))) if key.kind == KeyEventKind::Press => Tick::Key(key),
                        _ => Tick::Timeout,
                    }
                }
                Some(msg) = msg_rx.recv() => Tick::Msg(msg),
            };

            // Check search debounce on every iteration
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
                Tick::Msg(msg) => self.handle_msg(msg),
                Tick::Timeout => {}
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
                } => {
                    let result = client
                        .send_message(inbox_id, &to, &subject, &body)
                        .await
                        .map(|_| ())
                        .map_err(|e| e.to_string());
                    let _ = tx.send(AppMsg::MessageSent(result)).await;
                }
                Command::Approve(id) => {
                    let result = client.approve(id).await.map_err(|e| e.to_string());
                    let _ = tx
                        .send(AppMsg::ApprovalActioned {
                            id,
                            approved: true,
                            result,
                        })
                        .await;
                }
                Command::Reject(id) => {
                    let result = client.reject(id).await.map_err(|e| e.to_string());
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
            AppMsg::ThreadLoaded(Ok(messages)) => {
                let mut body = String::new();
                for msg in &messages {
                    body.push_str(&format!("From: {}\n", msg.from_addr));
                    body.push_str(&format!(
                        "Date: {}\n",
                        msg.created_at.format("%Y-%m-%d %H:%M")
                    ));
                    body.push_str(&"─".repeat(40));
                    body.push('\n');
                    if let Some(ref text) = msg.text_body {
                        body.push_str(text);
                    }
                    body.push_str("\n\n");
                }
                if let Some(first) = messages.first() {
                    self.preview.set_content(
                        &first.from_addr,
                        first.subject.as_deref().unwrap_or("(no subject)"),
                        &first.created_at.format("%Y-%m-%d %H:%M UTC").to_string(),
                        &body,
                    );
                }
                self.focus = Panel::Preview;
            }
            AppMsg::ThreadLoaded(Err(e)) => {
                self.status_text = Some(format!("Failed to load thread: {e}"));
                self.update_status_bar();
            }
            AppMsg::Ws(WsEvent::Connected) => {
                self.status_bar.connected = true;
            }
            AppMsg::Ws(WsEvent::Disconnected) => {
                self.status_bar.connected = false;
            }
            AppMsg::Ws(WsEvent::MessageReceived { inbox_id, .. }) => {
                if self.selected_inbox_id == Some(inbox_id) {
                    self.dispatch(Command::LoadMessages(inbox_id));
                }
            }
            AppMsg::Ws(WsEvent::ApprovalRequested { .. }) => {
                self.dispatch(Command::LoadApprovals);
            }
            AppMsg::Ws(WsEvent::TrustChanged { .. }) => {
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
        if self.sidebar_view != SidebarView::Inboxes {
            return;
        }
        let idx = self.message_list.selected();
        if let Some(msg) = self.displayed_messages.get(idx) {
            self.preview.set_content(
                &msg.from_addr,
                msg.subject.as_deref().unwrap_or("(no subject)"),
                &msg.created_at.format("%Y-%m-%d %H:%M UTC").to_string(),
                msg.text_body.as_deref().unwrap_or(""),
            );
        } else {
            self.preview.set_content("", "", "", "");
        }
    }

    fn update_status_bar(&mut self) {
        if let Some(ref text) = self.status_text {
            self.status_bar.inbox_name = text.clone();
            self.status_bar.inbox_count = 0;
        } else if let Some(inbox_id) = self.selected_inbox_id {
            if let Some(inbox) = self.inboxes.iter().find(|i| i.id == inbox_id) {
                self.status_bar.inbox_name = inbox.email.clone();
            }
            self.status_bar.inbox_count = self.displayed_messages.len();
        } else {
            self.status_bar.inbox_name = "All Inboxes".into();
            self.status_bar.inbox_count = self.inboxes.len();
        }
    }

    fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Option<Command> {
        if self.mode == Mode::Compose {
            if let Some(action) = keys::resolve(key, self.mode, self.focus, self.vim_mode) {
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
                    Action::Quit => self.running = false,
                    _ => {}
                }
            } else {
                match self.compose.field {
                    ComposeField::Body => self.compose.handle_key_for_body(key),
                    _ => self.compose.handle_key_for_header(key),
                }
            }
            return None;
        }

        if self.mode == Mode::Search {
            if let Some(action) = keys::resolve(key, self.mode, self.focus, self.vim_mode) {
                match action {
                    Action::Back => {
                        self.search.clear();
                        self.mode = Mode::Normal;
                        self.sidebar_view = SidebarView::Inboxes;
                    }
                    Action::Select => {
                        self.mode = Mode::Normal;
                    }
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

        let action = keys::resolve(key, self.mode, self.focus, self.vim_mode)?;

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
            Action::PanelLeft => self.focus = Panel::Sidebar,
            Action::PanelRight => {
                self.focus = if self.focus == Panel::Sidebar {
                    Panel::MessageList
                } else {
                    Panel::Preview
                };
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
                return Some(Command::LoadBriefing);
            }
            Action::ShowAllInboxes => {
                self.sidebar_view = SidebarView::Inboxes;
                self.inbox_list.select_first();
            }
            Action::SlopToggle => {
                self.show_slop = !self.show_slop;
                self.refresh_message_display();
            }
            Action::ApproveSelected => return self.handle_approve(),
            Action::RejectSelected => return self.handle_reject(),
            Action::QuickJump(n) => {
                self.inbox_list.select(n as usize);
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

    fn move_up(&mut self) {
        match self.focus {
            Panel::Sidebar => self.inbox_list.select_prev(),
            Panel::MessageList => match self.sidebar_view {
                SidebarView::Approvals => self.approvals.select_prev(),
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
            Panel::Preview => {}
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
                    // All Inboxes
                    self.selected_inbox_id = None;
                    self.messages.clear();
                    self.refresh_message_display();
                    self.update_status_bar();
                } else {
                    let inbox_id = self.inboxes.get(idx - 1).map(|i| i.id);
                    if let Some(id) = inbox_id {
                        self.selected_inbox_id = Some(id);
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
                        return Some(Command::LoadApprovals);
                    }
                    1 => {
                        self.sidebar_view = SidebarView::Briefing;
                        self.focus = Panel::MessageList;
                        return Some(Command::LoadBriefing);
                    }
                    2 => {
                        self.mode = Mode::Search;
                        self.sidebar_view = SidebarView::Search;
                    }
                    _ => {}
                }
            }
        }

        // Enter on message list: load thread if available
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
            SidebarView::Approvals | SidebarView::Briefing | SidebarView::Search => {
                self.sidebar_view = SidebarView::Inboxes;
                self.focus = Panel::Sidebar;
            }
            SidebarView::Inboxes => {
                if self.focus != Panel::Sidebar {
                    self.focus = Panel::Sidebar;
                }
            }
        }
    }

    fn render(&mut self, frame: &mut ratatui::Frame) {
        let theme = &self.theme;
        let area = frame.area();
        let layout = layout::compute(area);

        self.inbox_list
            .render(frame, layout.sidebar, theme, self.focus == Panel::Sidebar);

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
                );
            }
        }

        if self.mode == Mode::Compose {
            self.compose
                .render(frame, layout.preview, theme, self.focus == Panel::Preview);
        } else {
            self.preview
                .render(frame, layout.preview, theme, self.focus == Panel::Preview);
        }

        self.status_bar
            .render(frame, layout.status_bar, theme, self.mode);

        if self.mode == Mode::Search {
            self.search.render_input(frame, layout.status_bar, theme);
        }

        if self.mode == Mode::Help {
            render_help_overlay(frame, area, theme);
        }
    }
}

fn render_help_overlay(frame: &mut ratatui::Frame, area: Rect, theme: &Theme) {
    let help_w = 50.min(area.width.saturating_sub(4));
    let help_h = 18.min(area.height.saturating_sub(4));
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
        "  1-9             Quick jump",
        "",
        "  Actions",
        "  Enter           Select",
        "  Esc             Back",
        "  c or Ctrl+N     Compose",
        "  r or Ctrl+R     Reply",
        "  / or Ctrl+F     Search",
        "  Ctrl+Enter      Send message",
        "  y/n             Approve/reject",
        "  q or Ctrl+C     Quit",
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
