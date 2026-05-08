use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use super::app::{
    ActivePane, AppState, ComposeField, InputMode, SyncStateUi, ToastKind, FLAGGED_FLAG,
    ICON_ERROR, ICON_IDLE, ICON_POLLING, ICON_SYNCING, MAX_SELECTED_ERROR_CHARS, SEEN_FLAG,
};
use super::theme::Theme;

pub fn render(frame: &mut Frame<'_>, app: &AppState) {
    let theme = app.theme.theme();
    if app.composer.is_some() {
        render_composer(frame, app, &theme);
        return;
    }

    let toast_rows = app.toasts.len() as u16;
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),
            Constraint::Length(toast_rows),
            Constraint::Length(1),
        ])
        .split(frame.area());

    let main = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(root[0]);

    if app.threads_pane_visible() {
        let top = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(20),
                Constraint::Percentage(20),
                Constraint::Percentage(26),
                Constraint::Percentage(34),
            ])
            .split(main[0]);

        render_accounts(frame, top[0], app, &theme);
        render_folders(frame, top[1], app, &theme);
        render_threads(frame, top[2], app, &theme);
        render_messages(frame, top[3], app, &theme);
    } else {
        let top = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(25),
                Constraint::Percentage(25),
                Constraint::Percentage(50),
            ])
            .split(main[0]);

        render_accounts(frame, top[0], app, &theme);
        render_folders(frame, top[1], app, &theme);
        render_messages(frame, top[2], app, &theme);
    }
    if app.attachments_pane_visible() {
        render_detail_with_attachments(frame, main[1], app, &theme);
    } else {
        render_detail(frame, main[1], app, &theme);
    }
    render_toasts(frame, root[1], app, &theme);
    render_status(frame, root[2], app, &theme);
}

fn render_toasts(frame: &mut Frame<'_>, area: Rect, app: &AppState, theme: &Theme) {
    if app.toasts.is_empty() || area.height == 0 {
        return;
    }
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(vec![Constraint::Length(1); app.toasts.len()])
        .split(area);
    let max_width = area.width as usize;
    for (toast, row) in app.toasts.iter().zip(rows.iter()) {
        let style = match toast.kind {
            ToastKind::Info => theme.status,
            ToastKind::Success => theme.unread,
            ToastKind::Warn => theme.flagged,
            ToastKind::Error => theme.error,
        };
        let text = truncate_for_width(&toast.text, max_width);
        frame.render_widget(Paragraph::new(text).style(style), *row);
    }
}

fn truncate_for_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let mut chars = text.chars().count();
    if chars <= max_width {
        return text.to_string();
    }
    let keep = max_width.saturating_sub(1);
    let mut out: String = text.chars().take(keep).collect();
    out.push('…');
    chars = out.chars().count();
    debug_assert!(chars <= max_width);
    out
}

fn render_accounts(frame: &mut Frame<'_>, area: Rect, app: &AppState, theme: &Theme) {
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
                    Span::styled(format!(" [{}]", account.status), theme.muted),
                ]))
            })
            .collect()
    };
    let mut state = selection_state(app.accounts.len(), app.selected_account);
    let list = List::new(items)
        .block(pane_block(
            "Accounts",
            app.active == ActivePane::Accounts,
            theme,
        ))
        .style(theme.text)
        .highlight_style(theme.selection)
        .highlight_symbol("› ");
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_folders(frame: &mut Frame<'_>, area: Rect, app: &AppState, theme: &Theme) {
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
                    Span::styled(format!(" [{}]", folder.role), theme.muted),
                ]))
            })
            .collect()
    };
    let mut state = selection_state(app.folders.len(), app.selected_folder);
    let list = List::new(items)
        .block(pane_block(
            "Folders",
            app.active == ActivePane::Folders,
            theme,
        ))
        .style(theme.text)
        .highlight_style(theme.selection)
        .highlight_symbol("› ");
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_threads(frame: &mut Frame<'_>, area: Rect, app: &AppState, theme: &Theme) {
    let items: Vec<ListItem<'_>> = if app.threads.is_empty() {
        let text = if app.folders.is_empty() {
            "Select a folder"
        } else {
            "No threads"
        };
        vec![ListItem::new(text)]
    } else {
        app.threads
            .iter()
            .map(|thread| {
                let subject_style = if thread.unread {
                    theme.unread
                } else {
                    theme.text
                };
                ListItem::new(Line::from(vec![
                    Span::styled(if thread.unread { "● " } else { "  " }, theme.unread),
                    Span::styled(if thread.flagged { "★ " } else { "  " }, theme.flagged),
                    Span::styled(
                        thread.subject.clone(),
                        subject_style.add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!(" ({})", thread.message_count)),
                    Span::styled(format!(" {}", thread.latest_date), theme.muted),
                ]))
            })
            .collect()
    };
    let mut state = selection_state(app.threads.len(), app.selected_thread);
    let list = List::new(items)
        .block(pane_block(
            "Threads",
            app.active == ActivePane::Threads,
            theme,
        ))
        .style(theme.text)
        .highlight_style(theme.selection)
        .highlight_symbol("› ");
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_messages(frame: &mut Frame<'_>, area: Rect, app: &AppState, theme: &Theme) {
    let items: Vec<ListItem<'_>> = if app.messages.is_empty() {
        let text = if !app.threads_pane_visible() {
            if app.folders.is_empty() {
                "Select a folder"
            } else {
                "No messages in folder"
            }
        } else {
            "No messages in thread"
        };
        vec![ListItem::new(text)]
    } else {
        app.messages
            .iter()
            .map(|message| {
                let unread = !message.has_flag(SEEN_FLAG);
                let flagged = message.has_flag(FLAGGED_FLAG);
                let subject_style = if unread { theme.unread } else { theme.text };
                ListItem::new(Line::from(vec![
                    Span::styled(if unread { "● " } else { "  " }, theme.unread),
                    Span::styled(if flagged { "★ " } else { "  " }, theme.flagged),
                    Span::styled(
                        message.subject.clone(),
                        subject_style.add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!(" — {}", message.from)),
                    Span::styled(format!(" {}", message.date), theme.muted),
                ]))
            })
            .collect()
    };
    let mut state = selection_state(app.messages.len(), app.selected_message);
    let list = List::new(items)
        .block(pane_block(
            "Messages",
            app.active == ActivePane::Messages,
            theme,
        ))
        .style(theme.text)
        .highlight_style(theme.selection)
        .highlight_symbol("› ");
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_detail(frame: &mut Frame<'_>, area: Rect, app: &AppState, theme: &Theme) {
    if app.detail.is_some() && app.error.is_none() {
        render_detail_viewport(frame, area, app, theme);
        return;
    }

    let text = detail_text(app);
    let paragraph = Paragraph::new(text)
        .block(pane_block(
            "Detail",
            app.active == ActivePane::Details,
            theme,
        ))
        .style(if app.error.is_some() {
            theme.error
        } else {
            theme.text
        })
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn render_detail_viewport(frame: &mut Frame<'_>, area: Rect, app: &AppState, theme: &Theme) {
    let active = app.active == ActivePane::Details;
    let viewport_height = area.height.saturating_sub(2) as usize;
    let lines = app.detail_lines();
    let line_count = lines.len().max(1);
    let scroll = app.detail_visible_scroll(viewport_height);
    let (cursor_line, cursor_column) = app.detail_cursor_line_column();
    let selection = app.detail_selected_line_range();
    let visual_indicator = if selection.is_some() { " • VIS" } else { "" };
    let title = format!(
        "Detail Ln {}/{}{}",
        cursor_line.saturating_add(1).min(line_count),
        line_count,
        visual_indicator
    );
    let visible_lines = lines
        .iter()
        .enumerate()
        .skip(scroll)
        .take(viewport_height.max(1))
        .map(|(line_index, line)| {
            if selection
                .as_ref()
                .is_some_and(|range| range.contains(&line_index))
            {
                let visible = if line.is_empty() { " " } else { line };
                Line::styled(visible.to_string(), theme.selection)
            } else {
                Line::raw(line.clone())
            }
        })
        .collect::<Vec<_>>();

    let paragraph = Paragraph::new(visible_lines)
        .block(pane_block_owned(title, active, theme))
        .style(theme.text);
    frame.render_widget(paragraph, area);

    if active && cursor_line >= scroll {
        set_cursor_in_area(frame, area, cursor_column, cursor_line - scroll);
    }
}

fn detail_text(app: &AppState) -> String {
    if let Some(error) = &app.error {
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
    }
}

fn render_detail_with_attachments(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &AppState,
    theme: &Theme,
) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(area);
    render_detail(frame, columns[0], app, theme);

    let attachment_columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(columns[1]);
    render_attachment_list(frame, attachment_columns[0], app, theme);
    render_attachment_preview(frame, attachment_columns[1], app, theme);
}

fn render_attachment_list(frame: &mut Frame<'_>, area: Rect, app: &AppState, theme: &Theme) {
    let items: Vec<ListItem<'_>> = app
        .attachments
        .iter()
        .map(|attachment| {
            ListItem::new(Line::from(vec![
                Span::raw(attachment.filename.clone()),
                Span::styled(
                    format!(
                        " [{} • {} bytes]",
                        attachment.content_type, attachment.size_bytes
                    ),
                    theme.muted,
                ),
            ]))
        })
        .collect();
    let mut state = selection_state(app.attachments.len(), app.selected_attachment);
    let list = List::new(items)
        .block(pane_block(
            "Attachments",
            app.active == ActivePane::Attachments,
            theme,
        ))
        .style(theme.text)
        .highlight_style(theme.selection)
        .highlight_symbol("› ");
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_attachment_preview(frame: &mut Frame<'_>, area: Rect, app: &AppState, theme: &Theme) {
    let text = if let Some(preview) = &app.attachment_preview {
        let mut text = preview.message.clone();
        if let Some(body) = &preview.text {
            text.push_str("\n\n");
            text.push_str(body);
        }
        if preview.truncated {
            text.push_str("\n\n[truncated]");
        }
        text
    } else {
        "Select an attachment".into()
    };
    let paragraph = Paragraph::new(text)
        .block(
            Block::default()
                .title("Preview")
                .borders(Borders::ALL)
                .border_style(theme.pane)
                .title_style(theme.pane),
        )
        .style(theme.text)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn render_composer(frame: &mut Frame<'_>, app: &AppState, theme: &Theme) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(1)])
        .split(frame.area());
    let Some(composer) = &app.composer else {
        render_status(frame, root[1], app, theme);
        return;
    };

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(5),
        ])
        .split(root[0]);

    render_composer_field(
        frame,
        rows[0],
        "To",
        &composer.to,
        composer.to_cursor,
        composer.focused == ComposeField::To,
        theme,
    );
    render_composer_field(
        frame,
        rows[1],
        "Cc",
        &composer.cc,
        composer.cc_cursor,
        composer.focused == ComposeField::Cc,
        theme,
    );
    render_composer_field(
        frame,
        rows[2],
        "Bcc",
        &composer.bcc,
        composer.bcc_cursor,
        composer.focused == ComposeField::Bcc,
        theme,
    );
    render_composer_field(
        frame,
        rows[3],
        "Subject",
        &composer.subject,
        composer.subject_cursor,
        composer.focused == ComposeField::Subject,
        theme,
    );
    render_composer_body(
        frame,
        rows[4],
        composer.focused == ComposeField::Body,
        composer,
        theme,
    );
    render_status(frame, root[1], app, theme);
}

fn render_composer_field(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &'static str,
    text: &str,
    cursor: usize,
    active: bool,
    theme: &Theme,
) {
    let paragraph = Paragraph::new(text.to_string())
        .block(pane_block(title, active, theme))
        .style(theme.text)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
    if active {
        set_cursor_in_area(frame, area, cursor.min(text.chars().count()), 0);
    }
}

fn render_composer_body(
    frame: &mut Frame<'_>,
    area: Rect,
    active: bool,
    composer: &super::app::ComposerState,
    theme: &Theme,
) {
    let viewport_height = area.height.saturating_sub(2) as usize;
    let lines = composer.body_lines();
    let line_count = lines.len().max(1);
    let scroll = composer.body_visible_scroll(viewport_height);
    let (cursor_line, cursor_column) = composer.body_cursor_line_column();
    let selection = composer.body_selected_line_range();
    let visual_indicator = if selection.is_some() { " • VIS" } else { "" };
    let title = format!(
        "Body Ln {}/{}{}",
        cursor_line.saturating_add(1).min(line_count),
        line_count,
        visual_indicator
    );
    let visible_lines = lines
        .iter()
        .enumerate()
        .skip(scroll)
        .take(viewport_height.max(1))
        .map(|(line_index, line)| {
            if selection
                .as_ref()
                .is_some_and(|range| range.contains(&line_index))
            {
                let visible = if line.is_empty() { " " } else { line };
                Line::styled(visible.to_string(), theme.selection)
            } else {
                Line::raw((*line).to_string())
            }
        })
        .collect::<Vec<_>>();

    let paragraph = Paragraph::new(visible_lines)
        .block(pane_block_owned(title, active, theme))
        .style(theme.text);
    frame.render_widget(paragraph, area);

    if active && cursor_line >= scroll {
        set_cursor_in_area(frame, area, cursor_column, cursor_line - scroll);
    }
}

fn set_cursor_in_area(frame: &mut Frame<'_>, area: Rect, column: usize, row: usize) {
    let inner_width = area.width.saturating_sub(2);
    let inner_height = area.height.saturating_sub(2);
    if inner_width == 0 || inner_height == 0 {
        return;
    }
    let x = area.x + 1 + (column as u16).min(inner_width - 1);
    let y = area.y + 1 + (row as u16).min(inner_height - 1);
    frame.set_cursor_position(Position::new(x, y));
}

fn render_status(frame: &mut Frame<'_>, area: Rect, app: &AppState, theme: &Theme) {
    let (text, style) = match app.mode {
        InputMode::Command => (
            format!(" :{} | Enter run • Esc cancel ", app.command_input),
            theme.command,
        ),
        InputMode::Compose => (
            format!(
                " {} | Tab fields • arrows/Home/End edit • Enter next/newline • PgUp/PgDn body • v select • Ctrl-S save • Ctrl-X send • Esc cancel ",
                app.status
            ),
            theme.command,
        ),
        InputMode::ConfirmDiscard => (
            " Discard unsaved compose? y/n ".to_string(),
            theme.command,
        ),
        InputMode::ConfirmDelete => (" Delete? y/n ".to_string(), theme.command),
        InputMode::Normal => {
            let status = if let Some(error) = &app.error {
                format!("Error: {error}")
            } else {
                app.status.clone()
            };
            let body = if app.active == ActivePane::Details {
                format!(
                    " {status} | Details: Tab pane • d details • ↑/↓/j/k lines • PgUp/PgDn/Ctrl-U/D page • ←/→ cursor • Home/End line • v select • Esc clear VIS • a attach • q quit "
                )
            } else {
                format!(
                    " {status} | q quit • c compose • : command • ←/→ pane • Tab pane • ↑/↓ move • j/k move • Enter open • d details/delete • a attach • e export/archive • m move • o open • r refresh • s sync • u seen • f/* flag • t theme "
                )
            };
            let icons = sync_state_prefix(app);
            let text = compose_status_text(&icons, &body, area.width as usize);
            let style = if app.error.is_some() {
                theme.error
            } else {
                theme.status
            };
            (text, style)
        }
    };
    frame.render_widget(Paragraph::new(text).style(style), area);
}

/// Build the per-account icon prefix shown at the start of the normal-mode
/// status line. Returns an empty string if no accounts have a known state.
fn sync_state_prefix(app: &AppState) -> String {
    let selected_id = app.selected_account_id();
    let mut parts: Vec<String> = Vec::new();
    for account in &app.accounts {
        let Some(status) = app.account_states.get(&account.id) else {
            continue;
        };
        let icon = match status.state {
            SyncStateUi::Idle => ICON_IDLE,
            SyncStateUi::Polling => ICON_POLLING,
            SyncStateUi::Syncing => ICON_SYNCING,
            SyncStateUi::Error => ICON_ERROR,
        };
        let mut piece = format!("{icon} {}", account.label);
        if Some(account.id) == selected_id && status.state == SyncStateUi::Error {
            if let Some(err) = status.last_error.as_deref() {
                let trimmed: String = err.chars().take(MAX_SELECTED_ERROR_CHARS).collect();
                if !trimmed.is_empty() {
                    piece.push_str(": ");
                    piece.push_str(&trimmed);
                }
            }
        }
        parts.push(piece);
    }
    parts.join(" · ")
}

fn compose_status_text(icons: &str, body: &str, width: usize) -> String {
    if icons.is_empty() {
        return body.to_string();
    }
    if width == 0 {
        return icons.to_string();
    }
    let prefix = format!(" {icons} ");
    let prefix_chars = prefix.chars().count();
    let body_chars = body.chars().count();
    if prefix_chars + body_chars <= width {
        let mut out = prefix;
        out.push_str(body);
        return out;
    }
    if prefix_chars >= width {
        // Icons take priority; truncate icons to the available width.
        return prefix.chars().take(width).collect();
    }
    let remaining = width - prefix_chars;
    let mut out = prefix;
    out.extend(body.chars().take(remaining));
    out
}

fn pane_block(title: &'static str, active: bool, theme: &Theme) -> Block<'static> {
    pane_block_owned(title.to_string(), active, theme)
}

fn pane_block_owned(title: String, active: bool, theme: &Theme) -> Block<'static> {
    let chrome = if active {
        theme.active_pane
    } else {
        theme.pane
    };
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(theme.text)
        .border_style(chrome)
        .title_style(chrome)
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
    use ratatui::backend::{Backend, TestBackend};
    use ratatui::buffer::Buffer;
    use ratatui::style::Color;
    use ratatui::Terminal;
    use uuid::Uuid;

    use super::*;
    use crate::tui::app::{
        AccountItem, AttachmentItem, AttachmentPreviewItem, FolderItem, MessageDetail, MessageItem,
    };
    use crate::tui::theme::ThemeName;

    fn render_to_buffer(app: &AppState) -> Buffer {
        let backend = TestBackend::new(140, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn render_to_buffer_and_cursor(app: &AppState) -> (Buffer, Position) {
        let backend = TestBackend::new(140, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        let cursor = terminal.backend_mut().get_cursor_position().unwrap();
        (terminal.backend().buffer().clone(), cursor)
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
        assert!(text.contains("←/→ pane"));
        assert!(text.contains("↑/↓ move"));
        assert!(text.contains("Connected to /tmp/postblox.sock"));
    }

    #[test]
    fn test_render_loaded_state_shows_lists_and_detail() {
        let mut app = AppState::default();
        let selected_id = Uuid::new_v4();
        let thread_id = Uuid::new_v4();
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
        app.apply_folder_messages(vec![
            MessageItem {
                id: Uuid::new_v4(),
                thread_id: Some(thread_id),
                subject: "Launch plan reply".into(),
                from: "alice@example.com".into(),
                date: "2026-05-07 11:00".into(),
                snippet: "Preview".into(),
                flags: vec!["\\Flagged".into()],
            },
            MessageItem {
                id: selected_id,
                thread_id: Some(thread_id),
                subject: "Launch plan".into(),
                from: "alice@example.com".into(),
                date: "2026-05-07 10:00".into(),
                snippet: "Preview".into(),
                flags: vec!["\\Seen".into()],
            },
        ]);
        app.apply_detail(Some(MessageDetail {
            id: selected_id,
            subject: "Launch plan".into(),
            from: "alice@example.com".into(),
            snippet: "Preview".into(),
            body: "Full launch details".into(),
            flags: vec!["\\Flagged".into()],
        }));

        let text = buffer_text(&render_to_buffer(&app));

        assert!(text.contains("Work <work@example.com>"));
        assert!(text.contains("INBOX"));
        assert!(text.contains("Threads"));
        assert!(text.contains("(2)"));
        assert!(text.contains("●"));
        assert!(text.contains("★"));
        assert!(text.contains("Launch plan"));
        assert!(text.contains("Full launch details"));
    }

    #[test]
    fn test_render_hides_threads_title_for_singleton_only_folder() {
        let mut app = AppState::default();
        app.apply_folders(vec![FolderItem {
            id: Uuid::new_v4(),
            name: "INBOX".into(),
            role: "inbox".into(),
        }]);
        app.apply_folder_messages(vec![MessageItem {
            id: Uuid::new_v4(),
            thread_id: None,
            subject: "Solo update".into(),
            from: "alice@example.com".into(),
            date: "2026-05-07 10:00".into(),
            snippet: "Preview".into(),
            flags: Vec::new(),
        }]);

        let text = buffer_text(&render_to_buffer(&app));

        assert!(!text.contains("Threads"));
        assert!(text.contains("Messages"));
        assert!(text.contains("Solo update"));
    }

    #[test]
    fn test_render_error_state_is_visible() {
        let mut app = AppState::default();
        app.set_error("message.list_by_folder returned malformed data");

        let text = buffer_text(&render_to_buffer(&app));

        assert!(text.contains("Error"));
        assert!(text.contains("malformed data"));
    }

    #[test]
    fn test_render_command_mode_shows_command_bar() {
        let mut app = AppState::default();
        app.enter_command_mode();
        for ch in "theme next".chars() {
            assert!(app.push_command_char(ch));
        }

        let text = buffer_text(&render_to_buffer(&app));

        assert!(text.contains(":theme next"));
        assert!(text.contains("Esc cancel"));
    }

    #[test]
    fn test_render_high_contrast_status_uses_theme_style() {
        let mut app = AppState::default();
        app.set_theme(ThemeName::HighContrast);
        app.set_status("Ready");

        let buffer = render_to_buffer(&app);
        let status = buffer.cell((1, 23)).unwrap().style();

        assert_eq!(status.fg, Some(Color::Black));
        assert_eq!(status.bg, Some(Color::White));
    }

    #[test]
    fn test_render_command_bar_uses_command_theme_style() {
        let mut app = AppState::default();
        app.set_theme(ThemeName::HighContrast);
        app.enter_command_mode();
        assert!(app.push_command_char('s'));

        let buffer = render_to_buffer(&app);
        let command = buffer.cell((1, 23)).unwrap().style();

        assert_eq!(command.fg, Some(Color::Black));
        assert_eq!(command.bg, Some(Color::Cyan));
    }

    #[test]
    fn test_render_selection_uses_theme_selection_style() {
        let mut app = AppState::default();
        app.set_theme(ThemeName::HighContrast);
        app.apply_accounts(vec![AccountItem {
            id: Uuid::new_v4(),
            label: "Work".into(),
            email: "work@example.com".into(),
            status: "idle".into(),
        }]);

        let buffer = render_to_buffer(&app);
        let selected = buffer.cell((1, 1)).unwrap().style();

        assert_eq!(selected.fg, Some(Color::Black));
        assert_eq!(selected.bg, Some(Color::Yellow));
    }

    #[test]
    fn test_render_attachment_split_preview() {
        let mut app = AppState::default();
        let message_id = Uuid::new_v4();
        let attachment_id = Uuid::new_v4();
        app.apply_folder_messages(vec![MessageItem {
            id: message_id,
            thread_id: None,
            subject: "With attachment".into(),
            from: "alice@example.com".into(),
            date: "2026-05-07 10:00".into(),
            snippet: "Preview".into(),
            flags: Vec::new(),
        }]);
        app.apply_detail(Some(MessageDetail {
            id: message_id,
            subject: "With attachment".into(),
            from: "alice@example.com".into(),
            snippet: "Preview".into(),
            body: "Body beside attachments".into(),
            flags: Vec::new(),
        }));
        app.apply_attachments(vec![AttachmentItem {
            id: attachment_id,
            message_id,
            filename: "notes.txt".into(),
            content_type: "text/plain".into(),
            size_bytes: 18,
            disposition: "attachment".into(),
            storage_path: "/tmp/notes.txt".into(),
        }]);
        app.apply_attachment_preview(AttachmentPreviewItem {
            attachment_id,
            text: Some("safe text preview".into()),
            message: "Inline preview".into(),
            truncated: false,
            preview_bytes: 17,
        });
        app.active = ActivePane::Details;

        let text = buffer_text(&render_to_buffer(&app));

        assert!(text.contains("Detail Ln 1/5"));
        assert!(text.contains("Body beside attachments"));
        assert!(text.contains("Attachments"));
        assert!(text.contains("notes.txt"));
        assert!(text.contains("safe text preview"));
    }

    #[test]
    fn test_render_long_details_viewport_indicator_cursor_and_selection() {
        let mut app = AppState::default();
        app.set_theme(ThemeName::HighContrast);
        app.active = ActivePane::Details;
        app.apply_detail(Some(MessageDetail {
            id: Uuid::new_v4(),
            subject: "Long detail".into(),
            from: "alice@example.com".into(),
            snippet: "Preview".into(),
            body: (1..=20)
                .map(|line| format!("detail line {line:02}"))
                .collect::<Vec<_>>()
                .join("\n"),
            flags: Vec::new(),
        }));
        app.detail_cursor = app.detail_line_start(15);
        app.detail_scroll = 13;
        app.detail_selection_anchor = Some(15);
        app.detail_selection_focus = 16;

        let (buffer, cursor) = render_to_buffer_and_cursor(&app);
        let text = buffer_text(&buffer);

        assert!(text.contains("Detail Ln 16/24"));
        assert!(text.contains("VIS"));
        assert!(text.contains("detail line 10"));
        assert!(text.contains("detail line 12"));
        assert!(text.contains("detail line 13"));
        assert!(!text.contains("detail line 01"));
        assert!(text.contains("d details"));

        assert_eq!(cursor, Position::new(1, 16));
        let selected = buffer.cell((1, 16)).unwrap().style();
        assert_eq!(selected.fg, Some(Color::Black));
        assert_eq!(selected.bg, Some(Color::Yellow));
    }

    #[test]
    fn test_render_full_screen_composer() {
        let mut app = AppState::default();
        app.enter_composer(Uuid::new_v4());
        for ch in "bob@example.com".chars() {
            assert!(app.push_composer_char(ch));
        }
        app.next_composer_field();
        app.next_composer_field();
        app.next_composer_field();
        for ch in "Hello".chars() {
            assert!(app.push_composer_char(ch));
        }
        app.next_composer_field();
        for ch in "Composer body".chars() {
            assert!(app.push_composer_char(ch));
        }

        let text = buffer_text(&render_to_buffer(&app));

        assert!(text.contains("Compose"));
        assert!(text.contains("To"));
        assert!(text.contains("bob@example.com"));
        assert!(text.contains("Subject"));
        assert!(text.contains("Hello"));
        assert!(text.contains("Composer body"));
        assert!(text.contains("Ctrl-S save"));
        assert!(!text.contains("Accounts"));
        assert!(!text.contains("Messages"));
    }

    #[test]
    fn test_render_long_composer_body_scroll_indicator_cursor_and_selection() {
        let mut app = AppState::default();
        app.set_theme(ThemeName::HighContrast);
        app.enter_composer(Uuid::new_v4());
        let composer = app.composer.as_mut().unwrap();
        composer.focused = ComposeField::Body;
        composer.body = (1..=20)
            .map(|line| format!("body line {line:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        composer.body_cursor = composer.body_line_start(15);
        composer.body_scroll = 13;
        composer.body_selection_anchor = Some(15);
        composer.body_selection_focus = 16;

        let buffer = render_to_buffer(&app);
        let text = buffer_text(&buffer);

        assert!(text.contains("Body Ln 16/20"));
        assert!(text.contains("VIS"));
        assert!(text.contains("body line 14"));
        assert!(text.contains("body line 16"));
        assert!(text.contains("body line 17"));
        assert!(!text.contains("body line 01"));
        assert!(text.contains("PgUp/PgDn"));
        assert!(text.contains("v select"));

        let selected = buffer.cell((1, 17)).unwrap().style();
        assert_eq!(selected.fg, Some(Color::Black));
        assert_eq!(selected.bg, Some(Color::Yellow));
    }

    #[test]
    fn test_render_status_prefixes_account_sync_icons() {
        use std::time::Instant;
        let mut app = AppState::default();
        let personal = AccountItem {
            id: Uuid::new_v4(),
            label: "Personal".into(),
            email: "p@example.com".into(),
            status: "idle".into(),
        };
        let work = AccountItem {
            id: Uuid::new_v4(),
            label: "Work".into(),
            email: "w@example.com".into(),
            status: "idle".into(),
        };
        let side = AccountItem {
            id: Uuid::new_v4(),
            label: "Side".into(),
            email: "s@example.com".into(),
            status: "idle".into(),
        };
        let personal_id = personal.id;
        let work_id = work.id;
        let side_id = side.id;
        app.apply_accounts(vec![personal, work, side]);
        let now = Instant::now();
        app.apply_sync_state(personal_id, SyncStateUi::Idle, None, now);
        app.apply_sync_state(work_id, SyncStateUi::Polling, None, now);
        app.apply_sync_state(side_id, SyncStateUi::Error, Some("auth failed".into()), now);

        let text = buffer_text(&render_to_buffer(&app));

        assert!(text.contains("● Personal"));
        assert!(text.contains("~ Work"));
        assert!(text.contains("! Side"));
        assert!(text.contains(" · "));
    }

    #[test]
    fn test_render_status_appends_selected_error_text() {
        use std::time::Instant;
        let mut app = AppState::default();
        let acct = AccountItem {
            id: Uuid::new_v4(),
            label: "Work".into(),
            email: "w@example.com".into(),
            status: "idle".into(),
        };
        let acct_id = acct.id;
        app.apply_accounts(vec![acct]);
        app.apply_sync_state(
            acct_id,
            SyncStateUi::Error,
            Some("server says no".into()),
            Instant::now(),
        );
        // The default selected_account is 0, so the only account is selected.
        let text = buffer_text(&render_to_buffer(&app));

        assert!(text.contains("! Work: server says no"));
    }

    #[test]
    fn test_render_selected_error_text_truncated_to_60_chars() {
        use std::time::Instant;
        let mut app = AppState::default();
        let acct = AccountItem {
            id: Uuid::new_v4(),
            label: "Work".into(),
            email: "w@example.com".into(),
            status: "idle".into(),
        };
        let acct_id = acct.id;
        app.apply_accounts(vec![acct]);
        let long_error = "a".repeat(120);
        app.apply_sync_state(
            acct_id,
            SyncStateUi::Error,
            Some(long_error.clone()),
            Instant::now(),
        );
        // Suppress the toast that apply_sync_state pushed so it can't
        // bleed into our buffer text check.
        app.clear_toasts();

        // Use a wide terminal so truncation isn't happening because of
        // viewport width.
        let backend = TestBackend::new(240, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer();
        // Read only the bottom (status) row.
        let height = 12u16;
        let status_y = height - 1;
        let status_text: String = (0..240)
            .map(|x| buffer.cell((x, status_y)).unwrap().symbol().to_owned())
            .collect();

        assert!(status_text.contains(&"a".repeat(60)));
        assert!(!status_text.contains(&"a".repeat(61)));
    }

    #[test]
    fn test_render_renders_toast_row_above_status_line() {
        use std::time::Instant;
        let mut app = AppState::default();
        let now = Instant::now();
        app.push_toast(ToastKind::Info, "Synced Work", now);
        app.push_toast(ToastKind::Error, "Work: login refused", now);

        let buffer = render_to_buffer(&app);
        let text = buffer_text(&buffer);

        assert!(text.contains("Synced Work"));
        assert!(text.contains("Work: login refused"));
        // Toasts must sit just above the status line — i.e. should
        // appear once each in the buffer.
        let synced_count = text.matches("Synced Work").count();
        assert_eq!(synced_count, 1);
    }
}
