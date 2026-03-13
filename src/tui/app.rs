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

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        // Best-effort terminal restore during panic/drop — nothing useful to do on failure.
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

pub struct App {
    inbox_list: InboxList,
    message_list: MessageList,
    preview: Preview,
    compose: Compose,
    approvals: ApprovalPanel,
    search: SearchPanel,
    briefing: BriefingPanel,
    status_bar: StatusBar,
    theme: Theme,
    focus: Panel,
    mode: Mode,
    vim_mode: bool,
    sidebar_view: SidebarView,
    running: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SidebarView {
    Inboxes,
    Approvals,
    Briefing,
    Search,
}

impl App {
    pub fn new(config: &TuiConfig) -> Self {
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

        while self.running {
            terminal.draw(|frame| self.render(frame))?;

            tokio::select! {
                _ = tokio::time::sleep(tick_rate) => {}
                maybe_event = event_stream.next() => {
                    if let Some(Ok(CtEvent::Key(key))) = maybe_event {
                        if key.kind == KeyEventKind::Press {
                            self.handle_key(key);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
        if self.mode == Mode::Compose {
            if let Some(action) = keys::resolve(key, self.mode, self.focus, self.vim_mode) {
                match action {
                    Action::Back => {
                        self.compose.reset();
                        self.mode = Mode::Normal;
                    }
                    Action::Send => {
                        self.compose.reset();
                        self.mode = Mode::Normal;
                    }
                    Action::Quit => self.running = false,
                    _ => {}
                }
            } else {
                match self.compose.field {
                    ComposeField::Body => {
                        self.compose.handle_key_for_body(key);
                    }
                    _ => {
                        self.compose.handle_key_for_header(key);
                    }
                }
            }
            return;
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
                    KeyCode::Char(c) => self.search.push_char(c),
                    KeyCode::Backspace => self.search.pop_char(),
                    _ => {}
                }
            }
            return;
        }

        let Some(action) = keys::resolve(key, self.mode, self.focus, self.vim_mode) else {
            return;
        };

        match action {
            Action::Quit => self.running = false,
            Action::MoveUp => self.move_up(),
            Action::MoveDown => self.move_down(),
            Action::MoveTop => self.move_top(),
            Action::MoveBottom => self.move_bottom(),
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
            Action::Select => self.handle_select(),
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
            }
            Action::ShowAllInboxes => {
                self.sidebar_view = SidebarView::Inboxes;
                self.inbox_list.select_first();
            }
            Action::SlopToggle => {}
            Action::ApproveSelected => {}
            Action::RejectSelected => {}
            Action::QuickJump(n) => {
                self.inbox_list.select(n as usize);
            }
        }
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

    fn handle_select(&mut self) {
        if self.focus == Panel::Sidebar {
            let idx = self.inbox_list.logical_selected();
            let inboxes_count = self.inbox_list.inbox_count();
            if idx < inboxes_count {
                self.sidebar_view = SidebarView::Inboxes;
                self.focus = Panel::MessageList;
            } else {
                match idx - inboxes_count {
                    0 => {
                        self.sidebar_view = SidebarView::Approvals;
                        self.focus = Panel::MessageList;
                    }
                    1 => {
                        self.sidebar_view = SidebarView::Briefing;
                        self.focus = Panel::MessageList;
                    }
                    2 => {
                        self.mode = Mode::Search;
                        self.sidebar_view = SidebarView::Search;
                    }
                    _ => {}
                }
            }
        }
    }

    fn handle_back(&mut self) {
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
