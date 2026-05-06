use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use super::app::{ActivePane, AppState};

pub fn render(frame: &mut Frame<'_>, app: &AppState) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(1)])
        .split(frame.area());

    let main = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(root[0]);

    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(24),
            Constraint::Percentage(24),
            Constraint::Percentage(52),
        ])
        .split(main[0]);

    render_accounts(frame, top[0], app);
    render_folders(frame, top[1], app);
    render_messages(frame, top[2], app);
    render_detail(frame, main[1], app);
    render_status(frame, root[1], app);
}

fn render_accounts(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let items: Vec<ListItem<'_>> = if app.accounts.is_empty() {
        vec![ListItem::new("No accounts yet")]
    } else {
        app.accounts
            .iter()
            .map(|account| {
                let label = if account.label == account.email {
                    account.email.clone()
                } else {
                    format!("{} <{}>", account.label, account.email)
                };
                ListItem::new(Line::from(vec![
                    Span::raw(label),
                    Span::styled(
                        format!(" [{}]", account.status),
                        Style::default().fg(Color::Gray),
                    ),
                ]))
            })
            .collect()
    };
    let mut state = selection_state(app.accounts.len(), app.selected_account);
    let list = List::new(items)
        .block(pane_block("Accounts", app.active == ActivePane::Accounts))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("› ");
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_folders(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let items: Vec<ListItem<'_>> = if app.folders.is_empty() {
        let text = if app.accounts.is_empty() {
            "Select an account"
        } else {
            "No folders"
        };
        vec![ListItem::new(text)]
    } else {
        app.folders
            .iter()
            .map(|folder| {
                ListItem::new(Line::from(vec![
                    Span::raw(folder.name.clone()),
                    Span::styled(
                        format!(" [{}]", folder.role),
                        Style::default().fg(Color::Gray),
                    ),
                ]))
            })
            .collect()
    };
    let mut state = selection_state(app.folders.len(), app.selected_folder);
    let list = List::new(items)
        .block(pane_block("Folders", app.active == ActivePane::Folders))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("› ");
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_messages(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let items: Vec<ListItem<'_>> = if app.messages.is_empty() {
        let text = if app.folders.is_empty() {
            "Select a folder"
        } else {
            "No messages"
        };
        vec![ListItem::new(text)]
    } else {
        app.messages
            .iter()
            .map(|message| {
                ListItem::new(Line::from(vec![
                    Span::styled(
                        message.subject.clone(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!(" — {}", message.from)),
                    Span::styled(
                        format!(" {}", message.date),
                        Style::default().fg(Color::Gray),
                    ),
                ]))
            })
            .collect()
    };
    let mut state = selection_state(app.messages.len(), app.selected_message);
    let list = List::new(items)
        .block(pane_block("Messages", app.active == ActivePane::Messages))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("› ");
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_detail(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let text = if let Some(error) = &app.error {
        format!("Error: {error}")
    } else if let Some(detail) = &app.detail {
        format!(
            "Subject: {}\nFrom: {}\nSnippet: {}\n\n{}",
            detail.subject, detail.from, detail.snippet, detail.body
        )
    } else if app.messages.is_empty() {
        "No message selected".into()
    } else {
        "Press Enter to open the selected message".into()
    };

    let paragraph = Paragraph::new(text)
        .block(Block::default().title("Detail").borders(Borders::ALL))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn render_status(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let status = if let Some(error) = &app.error {
        format!("Error: {error}")
    } else {
        app.status.clone()
    };
    let text = format!(" {status} | Tab pane • ↑/↓/j/k move • Enter open • r refresh • q quit ");
    let style = if app.error.is_some() {
        Style::default().fg(Color::White).bg(Color::Red)
    } else {
        Style::default().fg(Color::Black).bg(Color::Gray)
    };
    frame.render_widget(Paragraph::new(text).style(style), area);
}

fn pane_block(title: &'static str, active: bool) -> Block<'static> {
    let style = if active {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(style)
}

fn selection_state(len: usize, selected: usize) -> ListState {
    let mut state = ListState::default();
    if len > 0 {
        state.select(Some(selected.min(len - 1)));
    }
    state
}

#[cfg(test)]
mod tests {
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::Terminal;
    use uuid::Uuid;

    use super::*;
    use crate::tui::app::{AccountItem, FolderItem, MessageDetail, MessageItem};

    fn render_to_buffer(app: &AppState) -> Buffer {
        let backend = TestBackend::new(140, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn buffer_text(buffer: &Buffer) -> String {
        buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    #[test]
    fn test_render_empty_state_shows_friendly_accounts_message() {
        let mut app = AppState::default();
        app.set_status("Connected to /tmp/postblox.sock");

        let text = buffer_text(&render_to_buffer(&app));

        assert!(text.contains("No accounts yet"));
        assert!(text.contains("q quit"));
        assert!(text.contains("Connected to /tmp/postblox.sock"));
    }

    #[test]
    fn test_render_loaded_state_shows_lists_and_detail() {
        let mut app = AppState::default();
        let message_id = Uuid::new_v4();
        app.apply_accounts(vec![AccountItem {
            id: Uuid::new_v4(),
            label: "Work".into(),
            email: "work@example.com".into(),
            status: "idle".into(),
        }]);
        app.apply_folders(vec![FolderItem {
            id: Uuid::new_v4(),
            name: "INBOX".into(),
            role: "inbox".into(),
        }]);
        app.apply_messages(vec![MessageItem {
            id: message_id,
            subject: "Launch plan".into(),
            from: "alice@example.com".into(),
            date: "2026-05-07 10:00".into(),
            snippet: "Preview".into(),
        }]);
        app.apply_detail(Some(MessageDetail {
            id: message_id,
            subject: "Launch plan".into(),
            from: "alice@example.com".into(),
            snippet: "Preview".into(),
            body: "Full launch details".into(),
        }));

        let text = buffer_text(&render_to_buffer(&app));

        assert!(text.contains("Work <work@example.com>"));
        assert!(text.contains("INBOX"));
        assert!(text.contains("Launch plan"));
        assert!(text.contains("Full launch details"));
    }

    #[test]
    fn test_render_error_state_is_visible() {
        let mut app = AppState::default();
        app.set_error("message.list_by_folder returned malformed data");

        let text = buffer_text(&render_to_buffer(&app));

        assert!(text.contains("Error"));
        assert!(text.contains("malformed data"));
    }
}
