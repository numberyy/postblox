//! Pure rendering of [`super::app::AppState`] onto a ratatui [`Frame`].
//!
//! The single entry point is `render`. It chooses between the normal
//! conversation-first layout (accounts / folders / conversations) and
//! the full-screen composer view, then delegates to per-pane helpers.
//! Rendering is read-only: it never mutates the app or talks to the
//! daemon. Theme styling comes from `super::theme::Theme`; no colour
//! literals leak into this module.

use std::fmt::Write as _;

use chrono::Utc;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use super::app::{
    human_size, ActivePane, AppState, ComposeField, InputMode, SyncStateUi, ToastKind, ICON_ERROR,
    ICON_IDLE, ICON_POLLING, ICON_SYNCING, MAX_COMPOSE_ATTACHMENT_BYTES, MAX_SELECTED_ERROR_CHARS,
};
use super::help::{HelpEntry, HELP_ROWS};
use super::theme::Theme;

/// Footer text rendered along the bottom border of the modal help
/// overlay. Kept as a `const` so the snapshot tests can pin the exact
/// wording without scraping the layout.
pub(crate) const HELP_FOOTER_TEXT: &str =
    "j/k scroll · PgUp/PgDn page · Home/End jump · Esc / ? close";

/// Title rendered at the top of the modal help overlay.
pub(crate) const HELP_TITLE_TEXT: &str = "Help — ? to close";

pub(crate) fn render(frame: &mut Frame<'_>, app: &AppState) {
    let theme = app.theme.theme();
    if app.composer.is_some() {
        render_composer(frame, app, &theme);
        if app.help_open {
            render_help_overlay(frame, frame.area(), app, &theme);
        }
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
    render_conversations(frame, top[2], app, &theme);
    if app.search_pane_visible() {
        render_search(frame, main[1], app, &theme);
    } else if app.approvals_folder_selected() {
        render_approval_detail(frame, main[1], app, &theme);
    } else if app.attachments_pane_visible() {
        render_detail_with_attachments(frame, main[1], app, &theme);
    } else {
        render_detail(frame, main[1], app, &theme);
    }
    render_toasts(frame, root[1], app, &theme);
    render_status(frame, root[2], app, &theme);
    // Help overlay is drawn last so it sits on top of every other
    // pane, including the toast row and status bar.
    if app.help_open {
        render_help_overlay(frame, frame.area(), app, &theme);
    }
}

fn render_search(frame: &mut Frame<'_>, area: Rect, app: &AppState, theme: &Theme) {
    let active = app.active == ActivePane::Search;
    let query = app.search_query().unwrap_or("");
    let scope_label: &str = app
        .search_scope_account()
        .and_then(|id| {
            app.accounts
                .iter()
                .find(|account| account.id == id)
                .map(|account| account.label.as_str())
        })
        .unwrap_or("all accounts");
    let title = if app.search_is_pending() {
        format!("Search '{query}' — {scope_label} • loading…")
    } else {
        let count = app
            .search
            .as_ref()
            .map(|state| state.hits.len())
            .unwrap_or(0);
        format!("Search '{query}' — {scope_label} • {count} hit(s)")
    };

    let items: Vec<ListItem<'_>> = match app.search.as_ref() {
        Some(state) if state.pending => vec![ListItem::new("Loading…")],
        Some(state) if state.hits.is_empty() => vec![ListItem::new("No results")],
        Some(state) => state
            .hits
            .iter()
            .map(|hit| {
                let mut line = vec![
                    Span::styled(
                        hit.subject.as_str(),
                        theme.text.add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!(" — {}", hit.from)),
                    Span::styled(format!(" {}", hit.date), theme.muted),
                ];
                if !hit.snippet.is_empty() {
                    line.push(Span::raw("  "));
                    line.push(Span::styled(hit.snippet.as_str(), theme.muted));
                }
                ListItem::new(Line::from(line))
            })
            .collect(),
        None => vec![ListItem::new("No search open")],
    };

    let selected = app.search.as_ref().map(|state| state.selected).unwrap_or(0);
    let len = app
        .search
        .as_ref()
        .map(|state| state.hits.len())
        .unwrap_or(0);
    let mut state = selection_state(len, selected);
    let list = List::new(items)
        .block(pane_block_owned(title, active, theme))
        .style(theme.text)
        .highlight_style(theme.selection)
        .highlight_symbol("› ");
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_approval_list(frame: &mut Frame<'_>, area: Rect, app: &AppState, theme: &Theme) {
    let active = app.active == ActivePane::Conversations;
    let count = app.approvals.items.len();
    let title = if app.approvals.pending {
        format!("Approvals • {count} pending • loading…")
    } else {
        format!("Approvals • {count} pending")
    };
    let now = Utc::now();
    let items: Vec<ListItem<'_>> = if app.approvals.pending && app.approvals.items.is_empty() {
        vec![ListItem::new("Loading approvals…")]
    } else if app.approvals.items.is_empty() {
        vec![ListItem::new("No pending approvals")]
    } else {
        app.approvals
            .items
            .iter()
            .map(|approval| {
                let mut lines = vec![Line::from(vec![
                    Span::styled(
                        approval.tool_label(),
                        theme.text.add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!(" {}", approval.age_label_at(now)), theme.muted),
                ])];
                if let Some(summary) = approval.row_summary() {
                    lines.push(Line::styled(summary, theme.muted));
                }
                ListItem::new(lines)
            })
            .collect()
    };
    let mut state = selection_state(count, app.approvals.selected);
    let list = List::new(items)
        .block(pane_block_owned(title, active, theme))
        .style(theme.text)
        .highlight_style(theme.selection)
        .highlight_symbol("› ");
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_approval_detail(frame: &mut Frame<'_>, area: Rect, app: &AppState, theme: &Theme) {
    let lines = approval_detail_lines(app, theme);
    let paragraph = Paragraph::new(lines)
        .block(pane_block_owned(
            approval_detail_title(app),
            app.active == ActivePane::Details,
            theme,
        ))
        .style(theme.text)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
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
                let label_span = if account.label == account.email {
                    Span::raw(account.email.as_str())
                } else {
                    Span::raw(format!("{} <{}>", account.label, account.email))
                };
                ListItem::new(Line::from(vec![
                    label_span,
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
                if folder.is_approvals_virtual() {
                    ListItem::new(Line::from(vec![
                        Span::raw(folder.name.as_str()),
                        Span::styled(format!(" ({})", app.approvals_pending_count()), theme.muted),
                    ]))
                } else {
                    ListItem::new(Line::from(vec![
                        Span::raw(folder.name.as_str()),
                        Span::styled(format!(" [{}]", folder.role), theme.muted),
                    ]))
                }
            })
            .collect()
    };
    let mut state = selection_state(app.folders.len(), app.selected_folder);
    // Surface pending approvals alongside the bare "Folders" title so
    // that a user on Inbox can still see N > 0 without scrolling to
    // the virtual approvals row. The badge is suppressed at N = 0 to
    // avoid allocating a per-frame title string in the common case.
    let pending = app.approvals_pending_count();
    let block = if pending > 0 {
        pane_block_owned(
            format!("Folders · Approvals ({pending})"),
            app.active == ActivePane::Folders,
            theme,
        )
    } else {
        pane_block("Folders", app.active == ActivePane::Folders, theme)
    };
    let list = List::new(items)
        .block(block)
        .style(theme.text)
        .highlight_style(theme.selection)
        .highlight_symbol("› ");
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_conversations(frame: &mut Frame<'_>, area: Rect, app: &AppState, theme: &Theme) {
    if app.approvals_folder_selected() {
        render_approval_list(frame, area, app, theme);
        return;
    }

    if app.drafts_pane_active() {
        render_drafts(frame, area, app, theme);
        return;
    }

    let items: Vec<ListItem<'_>> = if app.threads.is_empty() {
        let text = if app.folders.is_empty() {
            "Select a folder"
        } else {
            "No conversations"
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
                let mut line = vec![
                    Span::styled(if thread.unread { "● " } else { "  " }, theme.unread),
                    Span::styled(if thread.flagged { "★ " } else { "  " }, theme.flagged),
                    Span::styled(
                        thread.subject.as_str(),
                        subject_style.add_modifier(Modifier::BOLD),
                    ),
                ];
                if thread.message_count > 1 {
                    line.push(Span::raw(format!(" ({})", thread.message_count)));
                }
                line.extend([
                    Span::raw(format!(" — {}", thread.latest_from)),
                    Span::styled(format!(" {}", thread.latest_date), theme.muted),
                ]);
                ListItem::new(Line::from(line))
            })
            .collect()
    };
    let mut state = selection_state(app.threads.len(), app.selected_thread);
    let list = List::new(items)
        .block(pane_block(
            "Conversations",
            app.active == ActivePane::Conversations,
            theme,
        ))
        .style(theme.text)
        .highlight_style(theme.selection)
        .highlight_symbol("› ");
    frame.render_stateful_widget(list, area, &mut state);
}

/// Render the Drafts pane in place of the conversations list when
/// the active folder has the `drafts` role. Same widget shape as
/// `render_conversations` so it slots into the existing layout, but the
/// rows show the recipient + the first body line so users can spot
/// the draft they want to resume.
fn render_drafts(frame: &mut Frame<'_>, area: Rect, app: &AppState, theme: &Theme) {
    let items: Vec<ListItem<'_>> = if app.drafts.is_empty() {
        vec![ListItem::new("No drafts")]
    } else {
        app.drafts
            .iter()
            .map(|draft| {
                ListItem::new(Line::from(vec![
                    Span::styled(
                        draft.subject.as_str(),
                        theme.text.add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!(" → {}", draft.to)),
                    Span::raw(format!(" — {}", draft.snippet)),
                    Span::styled(format!(" {}", draft.date), theme.muted),
                ]))
            })
            .collect()
    };
    let mut state = selection_state(app.drafts.len(), app.selected_draft);
    let list = List::new(items)
        .block(pane_block(
            "Drafts",
            app.active == ActivePane::Conversations,
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
    let line_count = app.detail_line_count().max(1);
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
    let visible_lines = (scroll..line_count)
        .take(viewport_height.max(1))
        .map(|line_index| {
            let line = app.detail_line_text(line_index).unwrap_or("");
            if selection
                .as_ref()
                .is_some_and(|range| range.contains(&line_index))
            {
                let visible = if line.is_empty() { " " } else { line };
                Line::styled(visible, theme.selection)
            } else {
                Line::raw(line)
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
    } else if let Some(text) = app.detail_text_content() {
        text.to_owned()
    } else if app.messages.is_empty() {
        "No message selected".into()
    } else {
        "Press Enter to open the selected message".into()
    }
}

fn approval_detail_lines<'a>(app: &'a AppState, theme: &Theme) -> Vec<Line<'a>> {
    if app.approvals.pending && app.approvals.items.is_empty() {
        return vec![Line::styled("Loading approvals…", theme.muted)];
    }
    let Some(approval) = app.selected_approval() else {
        return vec![Line::styled("No pending approvals", theme.muted)];
    };

    let action_label = approval.tool_label();
    let target = approval.target.as_ref();
    let header_subject = target
        .and_then(super::app::ApprovalTargetContext::target)
        .map(str::to_owned);
    let header_attachment = target
        .filter(|_| header_subject.is_none())
        .and_then(super::app::ApprovalTargetContext::attachment)
        .map(str::to_owned);

    let mut lines: Vec<Line<'a>> = Vec::new();

    // Header block: subject (bold) + " — <action>" (muted). When no
    // resolved target subject exists, fall back to the action label as
    // the headline.
    if let Some(subject) = header_subject.as_deref().or(header_attachment.as_deref()) {
        lines.push(Line::from(vec![
            Span::styled(subject.to_owned(), theme.text.add_modifier(Modifier::BOLD)),
            Span::styled(format!(" — {action_label}"), theme.muted),
        ]));
    } else {
        lines.push(Line::from(vec![Span::styled(
            action_label.clone(),
            theme.text.add_modifier(Modifier::BOLD),
        )]));
    }

    if let Some(meta) = approval_header_meta_line(target) {
        lines.push(Line::styled(meta, theme.muted));
    }

    // Couldn't-resolve hint: when the approval references an entity by
    // id (message/draft/attachment) but the daemon didn't return a
    // human-readable target, surface a low-emphasis note.
    if target.is_none() {
        if let Some(note) = approval_unresolved_target_note(approval) {
            lines.push(Line::styled(note, theme.muted));
        }
    }

    // Snippet block.
    let snippet = target.and_then(super::app::ApprovalTargetContext::snippet);
    if let Some(snippet) = snippet {
        lines.push(Line::raw(""));
        for chunk in snippet.lines() {
            lines.push(Line::from(vec![
                Span::styled("▎ ", theme.muted),
                Span::styled(chunk.to_owned(), theme.muted),
            ]));
        }
    }

    // Gate context.
    if let Some(summary) = approval.summary.as_deref() {
        lines.push(Line::raw(""));
        lines.push(Line::styled(format!("policy: {summary}"), theme.muted));
    }

    // Debug block — low-emphasis tool/args metadata + shortened JSON.
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        format!(
            "tool={} · args={} · {}",
            approval.tool,
            approval_arg_count_label(approval),
            approval.age_label_at(Utc::now())
        ),
        theme.muted,
    ));
    let shortened = shorten_uuid_in_json(&approval.args_json);
    for chunk in shortened.lines() {
        lines.push(Line::styled(chunk.to_owned(), theme.muted));
    }

    lines
}

fn approval_detail_title(app: &AppState) -> String {
    let Some(approval) = app.selected_approval() else {
        return "Approval detail".into();
    };
    let age = approval.age_label_at(Utc::now());
    format!("{} — {age}", approval.tool_label())
}

/// Build the muted "from … · to …" line under the approval header.
/// Returns `None` when neither field is resolved.
fn approval_header_meta_line(target: Option<&super::app::ApprovalTargetContext>) -> Option<String> {
    let target = target?;
    let from = target.from();
    let to = target.to();
    match (from, to) {
        (Some(from), Some(to)) => Some(format!("from {from} · to {to}")),
        (Some(from), None) => Some(format!("from {from}")),
        (None, Some(to)) => Some(format!("to {to}")),
        (None, None) => None,
    }
}

/// Best-effort note when an approval references an entity by id but
/// the daemon hasn't enriched the target context yet. Renders only
/// when the approval payload looks like it carried an id reference,
/// so already-direct approvals (e.g. a `subject` argument) stay quiet.
fn approval_unresolved_target_note(approval: &super::app::ApprovalItem) -> Option<&'static str> {
    let value = approval.args_value()?;
    let has_id = value.get("message_id").is_some()
        || value.get("draft_id").is_some()
        || value.get("attachment_id").is_some();
    has_id.then_some("couldn't load message/draft target")
}

/// Compact "<n> keys" label for the muted debug line. Falls back to a
/// generic "args" string when the payload is not a JSON object.
fn approval_arg_count_label(approval: &super::app::ApprovalItem) -> String {
    match approval.args_value() {
        Some(serde_json::Value::Object(map)) => {
            let n = map.len();
            format!("{n} {}", if n == 1 { "key" } else { "keys" })
        }
        _ => "payload".into(),
    }
}

/// Rewrite bare 36-char UUIDs inside a pretty-printed JSON blob to a
/// compact `first8…last4` form. Keeps the JSON shape (quotes,
/// indentation, commas) intact so consumers can still see which field
/// the id belongs to.
fn shorten_uuid_in_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 36 <= bytes.len() && looks_like_uuid(&bytes[i..i + 36]) {
            // Replace the 36-char run with `<first8>…<last4>`.
            // Both ends are pure ASCII because they came from a UUID.
            out.push_str(&s[i..i + 8]);
            out.push('…');
            out.push_str(&s[i + 32..i + 36]);
            i += 36;
        } else {
            // SAFETY: byte index is always at a char boundary because
            // we either advanced by 36 ASCII chars above or by exactly
            // one valid UTF-8 character below.
            let ch = s[i..].chars().next().expect("non-empty remainder");
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

fn looks_like_uuid(bytes: &[u8]) -> bool {
    if bytes.len() != 36 {
        return false;
    }
    for (index, byte) in bytes.iter().enumerate() {
        let is_hyphen = matches!(index, 8 | 13 | 18 | 23);
        if is_hyphen {
            if *byte != b'-' {
                return false;
            }
        } else if !byte.is_ascii_hexdigit() {
            return false;
        }
    }
    true
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
                Span::raw(attachment.filename.as_str()),
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
    let preview_focused = app.is_preview_focus_active();
    let block_title = if preview_focused {
        if app.preview_selection.is_some() {
            "Preview • VIS"
        } else {
            "Preview •"
        }
    } else {
        "Preview"
    };
    let block = Block::default()
        .title(block_title)
        .borders(Borders::ALL)
        .border_style(theme.pane)
        .title_style(theme.pane);

    let Some(preview_text) = app.preview_text() else {
        // Mirror the composer-attachments affordance pattern: name the
        // chord that brings this pane to life so a first-time user
        // doesn't have to read the source to discover Enter focuses
        // the preview and j/k scroll it.
        let paragraph = Paragraph::new("Select an attachment (Enter to focus, j/k to scroll)")
            .block(block)
            .style(theme.text)
            .wrap(Wrap { trim: false });
        frame.render_widget(paragraph, area);
        return;
    };
    let lines: Vec<&str> = preview_text.split('\n').collect();
    let viewport_height = area.height.saturating_sub(2) as usize;
    let scroll = app.preview_visible_scroll(viewport_height.max(1));
    let selection = app.preview_selected_line_range();
    // Prepend a single muted hint line when a preview is loaded but
    // the pane is not focused. The hint sits inside the bordered area
    // so it consumes one viewport row only; the body still wraps and
    // scrolls per its original rules.
    let mut visible: Vec<Line> = Vec::new();
    if !preview_focused {
        visible.push(Line::styled(
            "Preview ready — Enter to focus, j/k to scroll, v to select, y to copy".to_string(),
            theme.muted,
        ));
    }
    visible.extend(
        lines
            .iter()
            .enumerate()
            .skip(scroll)
            .take(viewport_height.max(1))
            .map(|(idx, line)| {
                if selection.as_ref().is_some_and(|range| range.contains(&idx)) {
                    let visible = if line.is_empty() { " " } else { *line };
                    Line::styled(visible.to_string(), theme.selection)
                } else {
                    Line::raw((*line).to_string())
                }
            }),
    );
    let paragraph = Paragraph::new(visible)
        .block(block)
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

    // Attachments panel is fixed-height: header + up to 5 rows + footer.
    // 7 total when present; 0 rows are reserved when empty (panel is
    // always visible so the user knows the slot exists and the
    // shortcut hint applies).
    let attachments_height = compose_attachments_panel_height(composer);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(attachments_height),
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
    render_compose_attachments(frame, rows[5], app, composer, theme);
    render_status(frame, root[1], app, theme);
}

/// Visible-row height for the attachments panel. Attachments scroll
/// after 5 entries; the bordered block adds 2 rows so the title +
/// summary stay legible.
fn compose_attachments_panel_height(composer: &super::app::ComposerState) -> u16 {
    const VISIBLE_ROWS: u16 = 5;
    let count = composer.attachments.len() as u16;
    let inner = count.clamp(1, VISIBLE_ROWS);
    inner.saturating_add(2)
}

fn render_compose_attachments(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &AppState,
    composer: &super::app::ComposerState,
    theme: &Theme,
) {
    let total = composer.aggregate_attachment_size();
    let count = composer.attachments.len();
    let summary = format!(
        "Attachments: {count} file{plural}, {used} / {cap}",
        plural = if count == 1 { "" } else { "s" },
        used = human_size(total),
        cap = human_size(MAX_COMPOSE_ATTACHMENT_BYTES),
    );

    if app.mode == InputMode::ComposeAttachPath {
        let prompt = format!("Attach: {}", composer.attach_input);
        let block = pane_block_owned(format!(" {summary} "), false, theme);
        let paragraph = Paragraph::new(prompt).block(block).style(theme.command);
        frame.render_widget(paragraph, area);
        // Position the cursor at the end of the typed path. "Attach: "
        // is 8 chars; account for the bordered inset.
        let cursor_col = "Attach: ".len() + composer.attach_input.chars().count();
        set_cursor_in_area(frame, area, cursor_col, 0);
        return;
    }

    if composer.attachments.is_empty() {
        let block = pane_block_owned(format!(" {summary} "), false, theme);
        let hint = "(Ctrl-A to attach a file)";
        let paragraph = Paragraph::new(hint).block(block).style(theme.text);
        frame.render_widget(paragraph, area);
        return;
    }

    let items: Vec<ListItem> = composer
        .attachments
        .iter()
        .map(|att| {
            let line = Line::from(vec![
                Span::raw(att.filename.as_str()),
                Span::raw("  "),
                Span::styled(human_size(att.size_bytes), theme.muted),
            ]);
            ListItem::new(line)
        })
        .collect();
    let mut state = selection_state(count, composer.selected_attachment);
    // Append `[i/total]` so the user can see which row the highlight
    // sits on without counting. The selected index is 0-based in
    // state but human-readable here, hence the +1. Empty and
    // attach-prompt branches above keep the plain summary.
    let current = composer.selected_attachment.min(count.saturating_sub(1)) + 1;
    let summary = format!("{summary} [{current}/{count}]");
    let list = List::new(items)
        .block(pane_block_owned(format!(" {summary} "), false, theme))
        .highlight_style(theme.selection)
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, area, &mut state);
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
    let line_count = composer.body_line_count().max(1);
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
    let visible_lines = (scroll..line_count)
        .take(viewport_height.max(1))
        .map(|line_index| {
            let line = composer.body_line_text(line_index).unwrap_or("");
            if selection
                .as_ref()
                .is_some_and(|range| range.contains(&line_index))
            {
                let visible = if line.is_empty() { " " } else { line };
                Line::styled(visible, theme.selection)
            } else {
                Line::raw(line)
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
        InputMode::QuickSearch => (
            format!(" /{} | Enter search • Esc cancel ", app.search_input),
            theme.command,
        ),
        InputMode::Compose => (
            format!(
                " {} | Tab fields • PgUp/PgDn body • v select • Ctrl-A attach • Ctrl-K remove • Ctrl-S save • Ctrl-X send • Esc cancel ",
                app.status
            ),
            theme.command,
        ),
        InputMode::ComposeAttachPath => (
            format!(
                " Attach: {} | Enter add • Esc cancel ",
                app.compose_attach_input().unwrap_or("")
            ),
            theme.command,
        ),
        InputMode::ConfirmDiscard => (
            " Discard unsaved compose? y/n ".to_string(),
            theme.command,
        ),
        InputMode::ConfirmDelete => (" Delete? y/n ".to_string(), theme.command),
        InputMode::Normal => {
            let error_status: Option<String> = app
                .error
                .as_ref()
                .map(|error| format!("Error: {error}"));
            let status: &str = error_status
                .as_deref()
                .unwrap_or(app.status.as_str());
            // Compact summary: `<theme> · <pane> · <account>/<folder> · ? for help`.
            // The verbose key manual now lives in the modal help overlay
            // (src/tui/help.rs::HELP_ROWS); the status bar is reserved
            // for what the user is looking at, not how to navigate it.
            // Preview focus keeps its existing one-liner because the
            // selection/yank chords are not in the rest of the app.
            let body = if app.is_preview_focus_active() {
                format!(" {status} | Preview: j/k scroll • v select • y copy • Esc cancel ")
            } else {
                let pane_label = pane_label_for_status(app);
                let summary = compose_summary_text(app, pane_label);
                format!(" {status} · {summary} ")
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

/// Human-readable label for the currently focused pane, used in the
/// compact status-bar summary. Approvals and Drafts share the
/// `Conversations` pane but render different list bodies — surface
/// the virtual-folder name so the user can tell which is active.
fn pane_label_for_status(app: &AppState) -> &'static str {
    match app.active {
        ActivePane::Accounts => "Accounts",
        ActivePane::Folders => "Folders",
        ActivePane::Conversations => {
            if app.approvals_folder_selected() {
                "Approvals"
            } else if app.drafts_pane_active() {
                "Drafts"
            } else {
                "Conversations"
            }
        }
        ActivePane::Details => "Details",
        ActivePane::Attachments => "Attachments",
        ActivePane::Search => "Search",
    }
}

/// Compact `<theme> · <pane> · <account>/<folder> · ? for help` summary
/// rendered on the right-hand side of the bottom status bar. Approval
/// folder mode collapses the account/folder pair into `Approvals` so
/// users see the virtual folder rather than the underlying account.
fn compose_summary_text(app: &AppState, pane_label: &str) -> String {
    let theme = app.theme.to_string();
    if app.approvals_folder_selected() {
        return format!("{theme} · Approvals · ? for help");
    }
    let account = app
        .accounts
        .get(app.selected_account)
        .map(|a| a.label.as_str())
        .unwrap_or("(no account)");
    let folder = app.selected_folder_name().unwrap_or("(no folder)");
    format!("{theme} · {pane_label} · {account}/{folder} · ? for help")
}

/// Draw the modal help overlay on top of the current frame.
///
/// The overlay is centered (~75% width × ~80% height with a 16-row
/// minimum), wipes its area with [`Clear`] so background panes don't
/// bleed through, and renders [`HELP_ROWS`] inside a bordered
/// [`Block`]. The footer line documents the scroll bindings; the
/// title shows the close hint. Scroll state lives on [`AppState`];
/// the renderer clamps it via [`AppState::clamp_help_scroll`] so a
/// terminal resize can't strand the offset off the end.
pub(crate) fn render_help_overlay(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &AppState,
    theme: &Theme,
) {
    let overlay_area = help_overlay_rect(area);
    let lines = build_help_lines();
    let total_lines = lines.len();
    // Inner viewport height = block height minus two border rows.
    let viewport_height = overlay_area.height.saturating_sub(2) as usize;
    let max_scroll = total_lines.saturating_sub(viewport_height);
    // The render path cannot mutate AppState, but we still need to
    // clamp the displayed offset against the current viewport. Read
    // the raw value and clamp here for display only.
    let scroll = app.help_scroll.min(max_scroll);

    let title = Line::from(Span::styled(
        format!(" {HELP_TITLE_TEXT} "),
        theme.command.add_modifier(Modifier::BOLD),
    ));
    let footer = Line::from(Span::styled(format!(" {HELP_FOOTER_TEXT} "), theme.command));
    let block = Block::default()
        .title(title)
        .title_bottom(footer)
        .borders(Borders::ALL)
        .style(theme.text)
        .border_style(theme.command);

    let paragraph = Paragraph::new(lines)
        .block(block)
        .style(theme.text)
        .wrap(Wrap { trim: false })
        .scroll((scroll as u16, 0));

    // Clear the underlying region so the overlay never looks
    // translucent — `Clear` paints the background style of whatever
    // block follows it.
    frame.render_widget(Clear, overlay_area);
    frame.render_widget(paragraph, overlay_area);
}

/// Compute the centered rectangle for the help overlay. Targets ~75%
/// width × ~80% height with a 16-row minimum; falls back to the full
/// area minus one trailing row when the frame is too small.
fn help_overlay_rect(area: Rect) -> Rect {
    const MIN_HEIGHT: u16 = 16;
    const MIN_WIDTH: u16 = 40;

    if area.width < MIN_WIDTH || area.height < 6 {
        // Frame is too small for a centered overlay — fall back to the
        // whole area minus one trailing row to keep the status line
        // visible. Callers should still get a sane render.
        let height = area.height.saturating_sub(1).max(1);
        return Rect::new(area.x, area.y, area.width, height);
    }

    let width = (area.width.saturating_mul(75) / 100).max(MIN_WIDTH);
    let mut height = (area.height.saturating_mul(80) / 100).max(MIN_HEIGHT);
    if height > area.height {
        height = area.height;
    }

    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect::new(x, y, width, height)
}

/// Render [`HELP_ROWS`] into the line list shown inside the overlay.
fn build_help_lines() -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut first = true;
    for section in HELP_ROWS {
        if !first {
            lines.push(Line::raw(""));
        }
        first = false;
        lines.push(Line::from(Span::styled(
            section.title,
            ratatui::style::Style::default().add_modifier(Modifier::BOLD),
        )));
        for entry in section.entries {
            lines.push(help_entry_line(entry));
        }
    }
    lines.push(Line::raw(""));
    lines.push(Line::raw(
        "Source: src/tui/help.rs::HELP_ROWS — keep parser & overlay in sync.",
    ));
    lines
}

/// Format a single [`HelpEntry`] as `  <keys>  —  <summary>`.
fn help_entry_line(entry: &HelpEntry) -> Line<'static> {
    Line::raw(format!("  {}  —  {}", entry.keys, entry.summary))
}

/// Build the per-account icon prefix shown at the start of the normal-mode
/// status line. Returns an empty string if no accounts have a known state.
fn sync_state_prefix(app: &AppState) -> String {
    let selected_id = app.selected_account_id();
    // Pre-size for the common case: an `icon + ' ' + label` per account
    // plus `" · "` separators. Slight over-estimate is fine; under-
    // estimating just means one realloc.
    let mut out = String::with_capacity(app.accounts.len() * 24);
    let mut first = true;
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
        if !first {
            out.push_str(" · ");
        }
        first = false;
        // write! into a String never fails — `String`'s Write impl is infallible.
        let _ = write!(out, "{icon} {}", account.label);
        if Some(account.id) == selected_id && status.state == SyncStateUi::Error {
            if let Some(err) = status.last_error.as_deref() {
                let mut wrote_separator = false;
                for ch in err.chars().take(MAX_SELECTED_ERROR_CHARS) {
                    if !wrote_separator {
                        out.push_str(": ");
                        wrote_separator = true;
                    }
                    out.push(ch);
                }
            }
        }
    }
    out
}

fn compose_status_text(icons: &str, body: &str, width: usize) -> String {
    if icons.is_empty() {
        return body.to_string();
    }
    if width == 0 {
        return icons.to_string();
    }
    let mut prefix = String::with_capacity(icons.len() + 2);
    prefix.push(' ');
    prefix.push_str(icons);
    prefix.push(' ');
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

    use super::*;
    use crate::models::{AccountId, AttachmentId, FolderId, MessageId, ThreadId};
    use crate::tui::app::{
        compact_args_summary, AccountItem, ApprovalItem, ApprovalTargetContext, AttachmentItem,
        AttachmentPreviewItem, FolderItem, FolderKind, MessageDetail, MessageItem,
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

    fn render_approval_list_to_buffer(app: &AppState) -> Buffer {
        let backend = TestBackend::new(140, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let theme = app.theme.theme();
                render_approval_list(frame, frame.area(), app, &theme);
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn render_approval_detail_to_buffer(app: &AppState) -> Buffer {
        let backend = TestBackend::new(120, 16);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let theme = app.theme.theme();
                render_approval_detail(frame, frame.area(), app, &theme);
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn buffer_text(buffer: &Buffer) -> String {
        buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    fn threaded_message(
        thread_id: ThreadId,
        subject: &str,
        from: &str,
        date: &str,
        snippet: &str,
    ) -> MessageItem {
        MessageItem {
            id: MessageId::new(),
            thread_id: Some(thread_id),
            subject: subject.into(),
            from: from.into(),
            date: date.into(),
            snippet: snippet.into(),
            flags: Vec::new(),
        }
    }

    fn detail_for(message: &MessageItem, body: &str) -> MessageDetail {
        MessageDetail {
            id: message.id,
            subject: message.subject.clone(),
            from: message.from.clone(),
            snippet: message.snippet.clone(),
            body: body.into(),
            flags: message.flags.clone(),
        }
    }

    #[test]
    fn test_render_empty_state_shows_friendly_accounts_message() {
        let mut app = AppState::default();
        app.set_status("Connected to /tmp/postblox.sock");

        let text = buffer_text(&render_to_buffer(&app));

        assert!(text.contains("No accounts yet"));
        // The compact status bar advertises `? for help` instead of
        // the per-key manual that used to live here.
        assert!(text.contains("? for help"));
        assert!(text.contains("Connected to /tmp/postblox.sock"));
    }

    #[test]
    fn test_render_loaded_state_shows_lists_and_detail() {
        let mut app = AppState::default();
        let selected_id = MessageId::new();
        let thread_id = ThreadId::new();
        app.apply_accounts(vec![AccountItem {
            id: AccountId::new(),
            label: "Work".into(),
            email: "work@example.com".into(),
            status: "idle".into(),
        }]);
        app.apply_folders(vec![FolderItem {
            kind: FolderKind::Mail,
            id: FolderId::new(),
            name: "INBOX".into(),
            role: "inbox".into(),
        }]);
        app.apply_folder_messages(vec![
            MessageItem {
                id: selected_id,
                thread_id: Some(thread_id),
                subject: "Launch plan reply".into(),
                from: "alice@example.com".into(),
                date: "2026-05-07 11:00".into(),
                snippet: "Preview".into(),
                flags: vec!["\\Flagged".into()],
            },
            MessageItem {
                id: MessageId::new(),
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
            subject: "Launch plan reply".into(),
            from: "alice@example.com".into(),
            snippet: "Preview".into(),
            body: "Full launch details".into(),
            flags: vec!["\\Flagged".into()],
        }));

        let text = buffer_text(&render_to_buffer(&app));

        assert!(text.contains("Work <work@example.com>"));
        assert!(text.contains("INBOX"));
        assert!(text.contains("Conversations"));
        assert!(text.contains("(2)"));
        assert!(text.contains("●"));
        assert!(text.contains("★"));
        assert!(text.contains("Launch plan"));
        assert!(text.contains("Full launch details"));
    }

    #[test]
    fn test_folders_title_shows_approval_count_when_pending() {
        let mut app = AppState::default();
        app.apply_folders(vec![FolderItem {
            kind: FolderKind::Mail,
            id: FolderId::new(),
            name: "INBOX".into(),
            role: "inbox".into(),
        }]);
        app.apply_approvals(vec![
            ApprovalItem {
                id: uuid::Uuid::new_v4(),
                tool: "postblox_message_send".into(),
                args_summary: String::new(),
                args_json: "{}".into(),
                summary: None,
                target: None,
                created_at: Utc::now(),
            },
            ApprovalItem {
                id: uuid::Uuid::new_v4(),
                tool: "postblox_message_delete".into(),
                args_summary: String::new(),
                args_json: "{}".into(),
                summary: None,
                target: None,
                created_at: Utc::now(),
            },
        ]);

        let text = buffer_text(&render_to_buffer(&app));

        assert!(
            text.contains("Approvals (2)"),
            "folders title must surface pending count; got:\n{text}"
        );
    }

    #[test]
    fn test_folders_title_omits_approvals_when_zero() {
        let mut app = AppState::default();
        app.apply_folders(vec![FolderItem {
            kind: FolderKind::Mail,
            id: FolderId::new(),
            name: "INBOX".into(),
            role: "inbox".into(),
        }]);

        let text = buffer_text(&render_to_buffer(&app));

        assert!(text.contains("Folders"));
        // The virtual approvals row still renders (`Approvals (0)`),
        // but the pane title must NOT carry the badge — pinned via
        // the exact `Folders · Approvals` separator that the badge
        // injects.
        assert!(
            !text.contains("Folders · Approvals"),
            "folders title must not show the badge at zero pending; got:\n{text}"
        );
    }

    #[test]
    fn test_render_one_message_conversation_detail_expanded_without_collapsed_header() {
        let mut app = AppState::default();
        let thread_id = ThreadId::new();
        let message = threaded_message(
            thread_id,
            "Solo update",
            "alice@example.com",
            "2026-05-07 10:00",
            "Solo preview",
        );
        app.apply_folder_messages(vec![message.clone()]);
        app.apply_detail(Some(detail_for(&message, "Solo body")));

        let text = buffer_text(&render_to_buffer(&app));

        assert!(text.contains("Solo body"));
        assert!(!text.contains("[+]"));
    }

    #[test]
    fn test_render_three_message_conversation_detail_collapses_older_messages() {
        let mut app = AppState::default();
        let thread_id = ThreadId::new();
        let oldest = threaded_message(
            thread_id,
            "Start",
            "alice@example.com",
            "2026-05-07 09:00",
            "Oldest snippet",
        );
        let middle = threaded_message(
            thread_id,
            "Middle",
            "bob@example.com",
            "2026-05-07 10:00",
            "Middle snippet",
        );
        let newest = threaded_message(
            thread_id,
            "Latest",
            "carol@example.com",
            "2026-05-07 11:00",
            "Newest snippet",
        );
        app.apply_folder_messages(vec![newest.clone(), oldest, middle]);
        app.apply_detail(Some(detail_for(&newest, "Newest body")));

        let text = buffer_text(&render_to_buffer(&app));

        assert_eq!(text.matches("[+]").count(), 2);
        assert!(text.contains("[-] carol@example.com · 2026-05-07 11:00"));
        assert!(text.contains("Newest body"));
    }

    #[test]
    fn test_render_toggled_older_message_expanded_with_body() {
        let mut app = AppState::default();
        let thread_id = ThreadId::new();
        let oldest = threaded_message(
            thread_id,
            "Start",
            "alice@example.com",
            "2026-05-07 09:00",
            "Oldest snippet",
        );
        let middle = threaded_message(
            thread_id,
            "Middle",
            "bob@example.com",
            "2026-05-07 10:00",
            "Middle snippet",
        );
        let newest = threaded_message(
            thread_id,
            "Latest",
            "carol@example.com",
            "2026-05-07 11:00",
            "Newest snippet",
        );
        app.apply_folder_messages(vec![newest.clone(), oldest.clone(), middle]);
        app.apply_detail(Some(detail_for(&newest, "Newest body")));
        assert!(app.move_conversation_detail_focus(-2));
        assert_eq!(app.toggle_focused_message_expansion(), Some(true));
        app.apply_detail(Some(detail_for(&oldest, "Oldest expanded body")));

        let text = buffer_text(&render_to_buffer(&app));

        assert!(text.contains("[-] alice@example.com · 2026-05-07 09:00"));
        assert!(text.contains("Oldest expanded body"));
    }

    #[test]
    fn test_render_detail_viewport_shows_focused_stack_marker() {
        let mut app = AppState::default();
        let thread_id = ThreadId::new();
        let oldest = threaded_message(
            thread_id,
            "Start",
            "alice@example.com",
            "2026-05-07 09:00",
            "Oldest snippet",
        );
        let newest = threaded_message(
            thread_id,
            "Latest",
            "carol@example.com",
            "2026-05-07 11:00",
            "Newest snippet",
        );
        app.apply_folder_messages(vec![newest.clone(), oldest]);
        app.apply_detail(Some(detail_for(&newest, "Newest body")));

        let text = buffer_text(&render_to_buffer(&app));

        assert!(
            text.contains('▶'),
            "rendered buffer must contain the focused-stack marker glyph"
        );
    }

    #[test]
    fn test_render_singleton_conversation_has_no_count_badge() {
        let mut app = AppState::default();
        app.apply_folders(vec![FolderItem {
            kind: FolderKind::Mail,
            id: FolderId::new(),
            name: "INBOX".into(),
            role: "inbox".into(),
        }]);
        app.apply_folder_messages(vec![MessageItem {
            id: MessageId::new(),
            thread_id: None,
            subject: "Solo update".into(),
            from: "alice@example.com".into(),
            date: "2026-05-07 10:00".into(),
            snippet: "Preview".into(),
            flags: Vec::new(),
        }]);

        let text = buffer_text(&render_to_buffer(&app));

        assert!(text.contains("Conversations"));
        assert!(!text.contains("(1)"));
        assert!(text.contains("Solo update"));
    }

    #[test]
    fn test_render_three_message_conversation_shows_count_badge() {
        let mut app = AppState::default();
        let thread_id = ThreadId::new();
        app.apply_folders(vec![FolderItem {
            kind: FolderKind::Mail,
            id: FolderId::new(),
            name: "INBOX".into(),
            role: "inbox".into(),
        }]);
        app.apply_folder_messages(vec![
            MessageItem {
                id: MessageId::new(),
                thread_id: Some(thread_id),
                subject: "Launch reply two".into(),
                from: "carol@example.com".into(),
                date: "2026-05-07 12:00".into(),
                snippet: "Preview".into(),
                flags: Vec::new(),
            },
            MessageItem {
                id: MessageId::new(),
                thread_id: Some(thread_id),
                subject: "Launch reply".into(),
                from: "bob@example.com".into(),
                date: "2026-05-07 11:00".into(),
                snippet: "Preview".into(),
                flags: Vec::new(),
            },
            MessageItem {
                id: MessageId::new(),
                thread_id: Some(thread_id),
                subject: "Launch".into(),
                from: "alice@example.com".into(),
                date: "2026-05-07 10:00".into(),
                snippet: "Preview".into(),
                flags: Vec::new(),
            },
        ]);

        let text = buffer_text(&render_to_buffer(&app));

        assert!(text.contains("Conversations"));
        assert!(text.contains("(3)"));
        assert!(text.contains("carol@example.com"));
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
    fn test_render_empty_approvals_folder_says_no_pending_approvals() {
        let mut app = AppState::default();
        app.select_approvals_folder();
        app.apply_approvals(Vec::new());

        let text = buffer_text(&render_to_buffer(&app));

        assert!(text.contains("Approvals"));
        assert!(text.contains("Approvals (0)"));
        assert!(!text.contains("[system]"));
        assert!(text.contains("No pending approvals"));
    }

    #[test]
    fn test_render_approvals_folder_list_and_detail_use_target_context() {
        let mut app = AppState::default();
        app.select_approvals_folder();
        let message_id = "00000000-0000-0000-0000-0000000000bb";
        let target = serde_json::json!({
            "subject": "Quarterly review draft",
            "from": "contact-0-1@demo.example",
            "to": "alice@demo.local",
            "snippet": "Please review before Friday.",
        });
        app.apply_approvals(vec![ApprovalItem {
            id: uuid::Uuid::new_v4(),
            tool: "postblox_message_delete".into(),
            args_summary: compact_args_summary(&serde_json::json!({"message_id": message_id})),
            args_json: format!("{{\n  \"message_id\": \"{message_id}\"\n}}"),
            summary: Some("demo: never auto-delete from Trash".into()),
            target: ApprovalTargetContext::from_args(&target),
            created_at: Utc::now(),
        }]);

        let list_text = buffer_text(&render_approval_list_to_buffer(&app));
        assert!(list_text.contains("Delete message"));
        assert!(list_text.contains("\"Quarterly review draft\" from contact-0-1@demo.example"));
        assert!(list_text.contains("demo: never auto-delete from Trash"));
        assert!(!list_text.contains("message=…00bb"));
        assert!(!list_text.contains(message_id));

        let detail_text = buffer_text(&render_approval_detail_to_buffer(&app));
        // Pane title leads with the human action, not "Approval: …".
        assert!(detail_text.contains("Delete message"));
        assert!(!detail_text.contains("Approval: Delete message"));
        // Resolved subject appears once — as the bold header, NOT as
        // a "Target: …" body row.
        assert_eq!(detail_text.matches("Quarterly review draft").count(), 1);
        assert!(!detail_text.contains("Action:"));
        assert!(!detail_text.contains("Target:"));
        assert!(!detail_text.contains("Keys:"));
        // Muted context lines use lowercase prepositions.
        assert!(detail_text.contains("from contact-0-1@demo.example"));
        assert!(detail_text.contains("to alice@demo.local"));
        // Snippet renders under a quote glyph, no leading "Snippet:".
        assert!(detail_text.contains("▎ Please review before Friday."));
        assert!(!detail_text.contains("Snippet:"));
        // Policy summary is muted with a lower-case prefix.
        assert!(detail_text.contains("policy: demo: never auto-delete from Trash"));
        // Debug block: compact tool=… line, not a top-level "Tool: …" row.
        assert!(detail_text.contains("tool=postblox_message_delete"));
        assert!(!detail_text.contains("Tool: postblox_message_delete"));
        assert!(detail_text.contains("args=1 key"));
    }

    #[test]
    fn test_render_approval_detail_send_message_redesigned_layout() {
        let mut app = AppState::default();
        app.select_approvals_folder();
        let account_id = "5accb2f6-0000-4000-8000-00000000aaaa";
        let draft_id = "7c8051bd-0000-4000-8000-000000bbbbbb";
        let target = serde_json::json!({
            "subject": "Draft: weekly update",
            "to": "partner-0@demo.example",
            "snippet": "Hi team,",
        });
        let args = serde_json::json!({
            "account_id": account_id,
            "draft_id": draft_id,
        });
        app.apply_approvals(vec![ApprovalItem {
            id: uuid::Uuid::new_v4(),
            tool: "postblox_message_send".into(),
            args_summary: compact_args_summary(&args),
            args_json: serde_json::to_string_pretty(&args).unwrap(),
            summary: Some("demo: auto-allow internal sends".into()),
            target: ApprovalTargetContext::from_args(&target),
            created_at: Utc::now(),
        }]);

        let text = buffer_text(&render_approval_detail_to_buffer(&app));

        // Header carries the subject in bold + ` — Send message`, with
        // the action only in the pane title afterwards.
        assert!(text.contains("Draft: weekly update — Send message"));
        assert_eq!(text.matches("Draft: weekly update").count(), 1);
        assert!(text.contains("to partner-0@demo.example"));
        assert!(text.contains("▎ Hi team,"));
        assert!(text.contains("policy: demo: auto-allow internal sends"));
        // Compact debug line replaces the "Tool: …" / "Created: …" rows.
        assert!(text.contains("tool=postblox_message_send"));
        assert!(text.contains("args=2 keys"));
        // UUIDs in the JSON are abbreviated; the full string must not appear.
        assert!(!text.contains(account_id));
        assert!(!text.contains(draft_id));
        assert!(text.contains("5accb2f6…aaaa"));
        assert!(text.contains("7c8051bd…bbbb"));
        // Forbidden labels.
        for forbidden in ["Action:", "Target:", "Snippet:", "Keys:", "Approval:"] {
            assert!(
                !text.contains(forbidden),
                "approval detail should not contain {forbidden:?}"
            );
        }
    }

    #[test]
    fn test_render_approvals_folder_list_uses_human_label_and_short_id() {
        let mut app = AppState::default();
        app.select_approvals_folder();
        let message_id = "00000000-0000-0000-0000-0000000000bb";
        app.apply_approvals(vec![ApprovalItem {
            id: uuid::Uuid::new_v4(),
            tool: "postblox_message_delete".into(),
            args_summary: compact_args_summary(&serde_json::json!({"message_id": message_id})),
            args_json: format!("{{\n  \"message_id\": \"{message_id}\"\n}}"),
            summary: Some("demo: never auto-delete from Trash".into()),
            target: None,
            created_at: Utc::now(),
        }]);

        let text = buffer_text(&render_approval_list_to_buffer(&app));

        assert!(text.contains("Delete message"));
        assert!(text.contains("message=…00bb"));
        assert!(text.contains("demo: never auto-delete from Trash"));
        assert!(!text.contains("postblox_message_delete"));
        assert!(!text.contains(message_id));

        let full_text = buffer_text(&render_to_buffer(&app));
        assert!(full_text.contains("Approvals (1)"));
        assert!(!full_text.contains("[system]"));
    }

    #[test]
    fn test_render_approvals_folder_detail_shows_fallback_header_when_target_unresolved() {
        let mut app = AppState::default();
        app.select_approvals_folder();
        let message_id = "00000000-0000-0000-0000-0000000000bb";
        app.apply_approvals(vec![ApprovalItem {
            id: uuid::Uuid::new_v4(),
            tool: "postblox_message_delete".into(),
            args_summary: compact_args_summary(&serde_json::json!({"message_id": message_id})),
            args_json: format!("{{\n  \"message_id\": \"{message_id}\"\n}}"),
            summary: Some("demo: never auto-delete from Trash".into()),
            target: None,
            created_at: Utc::now(),
        }]);

        let text = buffer_text(&render_approval_detail_to_buffer(&app));

        // Title is action-first ("<action> — <age>") with no "Approval: …" prefix.
        assert!(text.contains("Delete message"));
        assert!(!text.contains("Approval: Delete message"));
        // Header falls back to the action label, plus the unresolved hint.
        assert!(text.contains("couldn't load message/draft target"));
        // Compact debug block surfaces the raw tool name once.
        assert!(text.contains("tool=postblox_message_delete"));
        assert!(!text.contains("Tool: postblox_message_delete"));
        // The full UUID is replaced with the abbreviated form.
        assert!(!text.contains(message_id));
        assert!(text.contains("00000000…00bb"));
        assert!(text.contains("policy: demo: never auto-delete from Trash"));
        for forbidden in ["Action:", "Target:", "Snippet:", "Keys:"] {
            assert!(
                !text.contains(forbidden),
                "approval detail should not contain {forbidden:?}"
            );
        }
    }

    #[test]
    fn test_render_approval_detail_loading_renders_single_muted_line() {
        let mut app = AppState::default();
        app.select_approvals_folder();
        app.approvals.pending = true;
        app.approvals.items.clear();

        let text = buffer_text(&render_approval_detail_to_buffer(&app));
        assert!(text.contains("Loading approvals…"));
        assert!(!text.contains("Keys:"));
        assert!(!text.contains("Approval:"));
    }

    #[test]
    fn test_render_approval_detail_empty_renders_single_muted_line() {
        let mut app = AppState::default();
        app.select_approvals_folder();
        app.apply_approvals(Vec::new());

        let text = buffer_text(&render_approval_detail_to_buffer(&app));
        assert!(text.contains("No pending approvals"));
        assert!(!text.contains("Keys:"));
    }

    #[test]
    fn test_shorten_uuid_in_json_abbreviates_bare_uuids_only() {
        let input =
            "{\n  \"id\": \"5accb2f6-1111-4222-8333-44444444aaaa\",\n  \"note\": \"keep-this\"\n}";
        let shortened = super::shorten_uuid_in_json(input);
        assert!(shortened.contains("\"5accb2f6…aaaa\""));
        assert!(!shortened.contains("5accb2f6-1111-4222-8333-44444444aaaa"));
        assert!(shortened.contains("\"note\": \"keep-this\""));
        // Non-uuid strings are untouched.
        let none = super::shorten_uuid_in_json("\"plain text without ids\"");
        assert_eq!(none, "\"plain text without ids\"");
    }

    #[test]
    fn test_render_approvals_folder_replaces_conversations_list() {
        let mut app = AppState::default();
        app.apply_folder_messages(vec![MessageItem {
            id: MessageId::new(),
            thread_id: None,
            subject: "Mail subject".into(),
            from: "alice@example.com".into(),
            date: "2026-05-07 10:00".into(),
            snippet: "Preview".into(),
            flags: Vec::new(),
        }]);
        app.apply_approvals(vec![ApprovalItem {
            id: uuid::Uuid::new_v4(),
            tool: "postblox_draft_delete".into(),
            args_summary: "subject=Delete me".into(),
            args_json: "{\"subject\":\"Delete me\"}".into(),
            summary: None,
            target: None,
            created_at: Utc::now(),
        }]);
        app.select_approvals_folder();

        let text = buffer_text(&render_to_buffer(&app));

        assert!(text.contains("Draft delete"));
        assert!(!text.contains("Mail subject"));
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
            id: AccountId::new(),
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
        let message_id = MessageId::new();
        let attachment_id = AttachmentId::new();
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
    fn test_attachment_preview_empty_shows_in_panel_hint() {
        let mut app = AppState::default();
        let message_id = MessageId::new();
        let attachment_id = AttachmentId::new();
        app.apply_folder_messages(vec![MessageItem {
            id: message_id,
            thread_id: None,
            subject: "No preview yet".into(),
            from: "alice@example.com".into(),
            date: "2026-05-07 10:00".into(),
            snippet: "Preview".into(),
            flags: Vec::new(),
        }]);
        app.apply_detail(Some(MessageDetail {
            id: message_id,
            subject: "No preview yet".into(),
            from: "alice@example.com".into(),
            snippet: "Preview".into(),
            body: "Body".into(),
            flags: Vec::new(),
        }));
        // Attachments exist so the split-preview pane is visible, but
        // no preview is loaded yet — the empty branch is the one that
        // must surface the affordances hint.
        app.apply_attachments(vec![AttachmentItem {
            id: attachment_id,
            message_id,
            filename: "notes.txt".into(),
            content_type: "text/plain".into(),
            size_bytes: 12,
            disposition: "attachment".into(),
            storage_path: "/tmp/notes.txt".into(),
        }]);
        app.active = ActivePane::Attachments;

        let text = buffer_text(&render_to_buffer(&app));

        // The hint wraps inside the narrow preview pane, so the
        // line-break can split the assertion fragment. Pin only the
        // chord keyword that survives the soft-wrap.
        assert!(
            text.contains("Enter to"),
            "empty preview pane must name its affordances; got:\n{text}"
        );
    }

    #[test]
    fn test_render_preview_focus_applies_scroll_offset_and_selection_style() {
        let mut app = AppState::default();
        let message_id = MessageId::new();
        let attachment_id = AttachmentId::new();
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
            body: "Body".into(),
            flags: Vec::new(),
        }));
        app.apply_attachments(vec![AttachmentItem {
            id: attachment_id,
            message_id,
            filename: "notes.txt".into(),
            content_type: "text/plain".into(),
            size_bytes: 12,
            disposition: "attachment".into(),
            storage_path: "/tmp/notes.txt".into(),
        }]);
        let body = (0..20).map(|n| format!("LN{n:02}")).collect::<Vec<_>>();
        app.apply_attachment_preview(AttachmentPreviewItem {
            attachment_id,
            text: Some(body.join("\n")),
            message: "Inline preview".into(),
            truncated: false,
            preview_bytes: 0,
        });
        app.active = ActivePane::Attachments;
        assert!(app.focus_preview());
        app.preview_scroll = 5;
        // Anchor selection at line 5; extend to line 6 with j.
        assert!(app.toggle_preview_selection());
        assert!(app.move_preview_line(1));

        let buffer = render_to_buffer(&app);
        let text = buffer_text(&buffer);

        // Lines 0..2 are header + blank, body lines start at idx 2.
        // After scroll=5, the first visible line is `LN03`. The
        // earlier lines (`LN00`..`LN02`) must not appear.
        assert!(!text.contains("LN00"));
        assert!(!text.contains("LN01"));
        assert!(!text.contains("LN02"));
        assert!(text.contains("LN03"));
        // Block title flips to indicate visual mode.
        assert!(text.contains("Preview • VIS"));
    }

    #[test]
    fn test_render_long_details_viewport_indicator_cursor_and_selection() {
        let mut app = AppState::default();
        app.set_theme(ThemeName::HighContrast);
        app.active = ActivePane::Details;
        app.apply_detail(Some(MessageDetail {
            id: MessageId::new(),
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
        // The compact status bar replaced per-pane key hints with
        // `? for help`; the long manual no longer leaks through.
        assert!(text.contains("? for help"));

        assert_eq!(cursor, Position::new(1, 16));
        let selected = buffer.cell((1, 16)).unwrap().style();
        assert_eq!(selected.fg, Some(Color::Black));
        assert_eq!(selected.bg, Some(Color::Yellow));
    }

    #[test]
    fn test_render_full_screen_composer() {
        let mut app = AppState::default();
        app.enter_composer(AccountId::new());
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
        assert!(!text.contains("Conversations"));
    }

    #[test]
    fn test_render_long_composer_body_scroll_indicator_cursor_and_selection() {
        let mut app = AppState::default();
        app.set_theme(ThemeName::HighContrast);
        app.enter_composer(AccountId::new());
        let composer = app.composer.as_mut().unwrap();
        composer.focused = ComposeField::Body;
        composer.body = (1..=20)
            .map(|line| format!("body line {line:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        composer.refresh_body_line_cache();
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

        // Selected lines render at the rows where line 15 / line 16 land
        // after the visible-scroll clamp; coordinates depend on the
        // composer layout (which now reserves a fixed-height attachments
        // panel below the body).
        let selected = buffer.cell((1, 15)).unwrap().style();
        assert_eq!(selected.fg, Some(Color::Black));
        assert_eq!(selected.bg, Some(Color::Yellow));
    }

    #[test]
    fn test_compose_attachments_summary_includes_index_over_total() {
        use crate::tui::app::ComposerAttachment;
        use std::path::PathBuf;

        let mut app = AppState::default();
        app.enter_composer(AccountId::new());
        let composer = app.composer.as_mut().unwrap();
        composer.attachments.push(ComposerAttachment {
            path: PathBuf::from("/tmp/a.txt"),
            filename: "a.txt".into(),
            size_bytes: 1,
            content_type: "text/plain".into(),
        });
        composer.attachments.push(ComposerAttachment {
            path: PathBuf::from("/tmp/b.txt"),
            filename: "b.txt".into(),
            size_bytes: 1,
            content_type: "text/plain".into(),
        });
        composer.selected_attachment = 1;

        let text = buffer_text(&render_to_buffer(&app));

        assert!(
            text.contains("[2/2]"),
            "compose attachments summary must include `[i/total]`; got:\n{text}"
        );
    }

    #[test]
    fn test_compose_attachments_summary_empty_omits_index() {
        let mut app = AppState::default();
        app.enter_composer(AccountId::new());

        let text = buffer_text(&render_to_buffer(&app));

        // Empty + attach-prompt states keep the existing summary; the
        // `[i/total]` fragment must not appear.
        assert!(
            !text.contains('['),
            "empty composer attachments must not render `[i/total]`; got:\n{text}"
        );
    }

    #[test]
    fn test_render_status_prefixes_account_sync_icons() {
        use std::time::Instant;
        let mut app = AppState::default();
        let personal = AccountItem {
            id: AccountId::new(),
            label: "Personal".into(),
            email: "p@example.com".into(),
            status: "idle".into(),
        };
        let work = AccountItem {
            id: AccountId::new(),
            label: "Work".into(),
            email: "w@example.com".into(),
            status: "idle".into(),
        };
        let side = AccountItem {
            id: AccountId::new(),
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
            id: AccountId::new(),
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
            id: AccountId::new(),
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

    #[test]
    fn test_render_status_compact_replaces_verbose_manual() {
        let app = AppState::default();
        let buffer = render_to_buffer(&app);
        let text = buffer_text(&buffer);

        // The compact status bar must advertise the new help affordance.
        assert!(
            text.contains("? for help"),
            "compact status bar must mention `? for help`; got:\n{text}"
        );
        // And the old verbose key manual must be gone — pin to a stable
        // canary fragment that lived in the old long status string.
        assert!(
            !text.contains("e export/archive"),
            "old verbose key manual leaked into the status bar"
        );
    }

    #[test]
    fn test_render_help_overlay_includes_header_and_footer_when_open() {
        let mut app = AppState::default();
        app.open_help();
        let buffer = render_to_buffer(&app);
        let text = buffer_text(&buffer);

        // Overlay title (canonical), footer hint, and at least one
        // section header from HELP_ROWS must all render.
        assert!(text.contains(HELP_TITLE_TEXT), "title missing:\n{text}");
        assert!(text.contains("j/k scroll"), "footer hint missing:\n{text}");
        assert!(
            text.contains("Panes & navigation"),
            "section header missing:\n{text}"
        );
    }

    #[test]
    fn test_render_help_overlay_hidden_by_default() {
        let app = AppState::default();
        let buffer = render_to_buffer(&app);
        let text = buffer_text(&buffer);
        assert!(!text.contains(HELP_TITLE_TEXT));
    }
}
