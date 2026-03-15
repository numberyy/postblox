use crossterm::event::KeyEvent;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;
use std::path::PathBuf;
use tui_textarea::TextArea;

use crate::theme::Theme;

pub struct Compose {
    pub to: String,
    pub subject: String,
    pub textarea: TextArea<'static>,
    pub field: ComposeField,
    pub attachments: Vec<PathBuf>,
    pub attachment_input: String,
    pub entering_attachment: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposeField {
    To,
    Subject,
    Body,
}

impl Compose {
    pub fn new() -> Self {
        let mut textarea = TextArea::default();
        textarea.set_cursor_line_style(Style::default());
        Self {
            to: String::new(),
            subject: String::new(),
            textarea,
            field: ComposeField::To,
            attachments: Vec::new(),
            attachment_input: String::new(),
            entering_attachment: false,
        }
    }

    pub fn new_reply(to: &str, subject: &str) -> Self {
        let re_subject = if subject.starts_with("Re: ") || subject.starts_with("re: ") {
            subject.to_string()
        } else {
            format!("Re: {subject}")
        };
        let mut textarea = TextArea::default();
        textarea.set_cursor_line_style(Style::default());
        Self {
            to: to.into(),
            subject: re_subject,
            textarea,
            field: ComposeField::Body,
            attachments: Vec::new(),
            attachment_input: String::new(),
            entering_attachment: false,
        }
    }

    pub fn reset(&mut self) {
        self.to.clear();
        self.subject.clear();
        self.textarea = TextArea::default();
        self.textarea.set_cursor_line_style(Style::default());
        self.field = ComposeField::To;
        self.attachments.clear();
        self.attachment_input.clear();
        self.entering_attachment = false;
    }

    pub fn start_attachment_input(&mut self) {
        self.entering_attachment = true;
        self.attachment_input.clear();
    }

    pub fn confirm_attachment(&mut self) {
        let path = self.attachment_input.trim().to_string();
        if !path.is_empty() {
            self.attachments.push(PathBuf::from(path));
        }
        self.attachment_input.clear();
        self.entering_attachment = false;
    }

    pub fn cancel_attachment_input(&mut self) {
        self.attachment_input.clear();
        self.entering_attachment = false;
    }

    pub fn remove_last_attachment(&mut self) {
        self.attachments.pop();
    }

    pub fn attachment_summary(&self) -> Vec<String> {
        self.attachments
            .iter()
            .map(|p| {
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| p.display().to_string());
                match std::fs::metadata(p) {
                    Ok(m) => format!("{name} ({:.1} KB)", m.len() as f64 / 1024.0),
                    Err(_) => format!("{name} (?)"),
                }
            })
            .collect()
    }

    pub fn body_text(&self) -> String {
        self.textarea.lines().join("\n")
    }

    pub fn set_body_text(&mut self, text: &str) {
        let lines: Vec<String> = if text.is_empty() {
            vec![String::new()]
        } else {
            text.lines().map(String::from).collect()
        };
        self.textarea = TextArea::new(lines);
        self.textarea.set_cursor_line_style(Style::default());
    }

    pub fn next_field(&mut self) {
        self.field = match self.field {
            ComposeField::To => ComposeField::Subject,
            ComposeField::Subject => ComposeField::Body,
            ComposeField::Body => ComposeField::To,
        };
    }

    pub fn prev_field(&mut self) {
        self.field = match self.field {
            ComposeField::To => ComposeField::Body,
            ComposeField::Subject => ComposeField::To,
            ComposeField::Body => ComposeField::Subject,
        };
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        use crossterm::event::{KeyCode, KeyModifiers};

        if self.entering_attachment {
            match key.code {
                KeyCode::Enter => self.confirm_attachment(),
                KeyCode::Esc => self.cancel_attachment_input(),
                KeyCode::Backspace => {
                    self.attachment_input.pop();
                }
                KeyCode::Char(c) => self.attachment_input.push(c),
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => self.prev_field(),
            KeyCode::Tab => self.next_field(),
            KeyCode::BackTab => self.prev_field(),
            _ if self.field == ComposeField::Body => {
                self.textarea.input(key);
            }
            KeyCode::Char(c) => match self.field {
                ComposeField::To => self.to.push(c),
                ComposeField::Subject => self.subject.push(c),
                ComposeField::Body => {}
            },
            KeyCode::Backspace => match self.field {
                ComposeField::To => {
                    self.to.pop();
                }
                ComposeField::Subject => {
                    self.subject.pop();
                }
                ComposeField::Body => {}
            },
            _ => {}
        }
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let block = Block::default()
            .title(" Compose ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme.border_focused));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        if inner.height < 4 {
            return;
        }

        let att_lines = if self.attachments.is_empty() && !self.entering_attachment {
            0u16
        } else {
            (self.attachments.len() as u16)
                .saturating_add(if self.entering_attachment { 1 } else { 0 })
                .min(4)
        };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(att_lines),
                Constraint::Length(1),
            ])
            .split(inner);

        let to_style = field_style(theme, self.field == ComposeField::To);
        let subj_style = field_style(theme, self.field == ComposeField::Subject);

        let to_line = Paragraph::new(Line::from(vec![
            Span::styled("  To: ", Style::default().fg(theme.muted)),
            Span::styled(&self.to, to_style),
        ]));
        frame.render_widget(to_line, chunks[0]);

        let subj_line = Paragraph::new(Line::from(vec![
            Span::styled("  Subject: ", Style::default().fg(theme.muted)),
            Span::styled(&self.subject, subj_style),
        ]));
        frame.render_widget(subj_line, chunks[1]);

        self.textarea.set_style(Style::default().fg(theme.fg));
        if self.field == ComposeField::Body {
            self.textarea
                .set_cursor_line_style(Style::default().fg(theme.accent));
        } else {
            self.textarea.set_cursor_line_style(Style::default());
        }
        frame.render_widget(&self.textarea, chunks[2]);

        // Attachment area
        if att_lines > 0 {
            let mut lines: Vec<Line> = self
                .attachment_summary()
                .into_iter()
                .map(|s| {
                    Line::from(vec![
                        Span::styled("  📎 ", Style::default().fg(theme.muted)),
                        Span::styled(s, Style::default().fg(theme.fg)),
                    ])
                })
                .collect();
            if self.entering_attachment {
                lines.push(Line::from(vec![
                    Span::styled("  Path: ", Style::default().fg(theme.accent)),
                    Span::styled(
                        &self.attachment_input,
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::UNDERLINED),
                    ),
                ]));
            }
            let att_widget = Paragraph::new(lines);
            frame.render_widget(att_widget, chunks[3]);
        }

        let hint = Paragraph::new(Line::from(vec![Span::styled(
            "  Ctrl+Enter: send │ Ctrl+A: attach │ Ctrl+E: editor │ Tab: field │ Esc: cancel",
            Style::default().fg(theme.muted),
        )]));
        frame.render_widget(hint, chunks[4]);
    }
}

fn field_style(theme: &Theme, active: bool) -> Style {
    if active {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::UNDERLINED)
    } else {
        Style::default().fg(theme.fg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_defaults() {
        let c = Compose::new();
        assert!(c.to.is_empty());
        assert!(c.subject.is_empty());
        assert_eq!(c.field, ComposeField::To);
        assert!(c.body_text().is_empty());
    }

    #[test]
    fn test_new_reply_adds_re() {
        let c = Compose::new_reply("alice@co.com", "Meeting");
        assert_eq!(c.to, "alice@co.com");
        assert_eq!(c.subject, "Re: Meeting");
        assert_eq!(c.field, ComposeField::Body);
    }

    #[test]
    fn test_new_reply_no_double_re() {
        let c = Compose::new_reply("alice@co.com", "Re: Meeting");
        assert_eq!(c.subject, "Re: Meeting");
    }

    #[test]
    fn test_new_reply_no_double_re_lowercase() {
        let c = Compose::new_reply("alice@co.com", "re: Meeting");
        assert_eq!(c.subject, "re: Meeting");
    }

    #[test]
    fn test_next_field_wraps() {
        let mut c = Compose::new();
        assert_eq!(c.field, ComposeField::To);
        c.next_field();
        assert_eq!(c.field, ComposeField::Subject);
        c.next_field();
        assert_eq!(c.field, ComposeField::Body);
        c.next_field();
        assert_eq!(c.field, ComposeField::To);
    }

    #[test]
    fn test_prev_field_wraps() {
        let mut c = Compose::new();
        assert_eq!(c.field, ComposeField::To);
        c.prev_field();
        assert_eq!(c.field, ComposeField::Body);
        c.prev_field();
        assert_eq!(c.field, ComposeField::Subject);
        c.prev_field();
        assert_eq!(c.field, ComposeField::To);
    }

    #[test]
    fn test_reset_clears_all() {
        let mut c = Compose::new();
        c.to = "test@test.com".into();
        c.subject = "Hello".into();
        c.field = ComposeField::Body;
        c.reset();
        assert!(c.to.is_empty());
        assert!(c.subject.is_empty());
        assert_eq!(c.field, ComposeField::To);
    }

    #[test]
    fn test_set_body_text() {
        let mut c = Compose::new();
        c.set_body_text("Hello\nWorld\nFoo");
        assert_eq!(c.body_text(), "Hello\nWorld\nFoo");
    }

    #[test]
    fn test_set_body_text_empty() {
        let mut c = Compose::new();
        c.set_body_text("some content");
        c.set_body_text("");
        assert!(c.body_text().is_empty());
    }

    #[test]
    fn test_set_body_text_single_line() {
        let mut c = Compose::new();
        c.set_body_text("Just one line");
        assert_eq!(c.body_text(), "Just one line");
    }

    #[test]
    fn test_set_body_text_preserves_unicode() {
        let mut c = Compose::new();
        c.set_body_text("café ☕ 日本語");
        assert_eq!(c.body_text(), "café ☕ 日本語");
    }

    #[test]
    fn test_add_attachment() {
        let mut c = Compose::new();
        assert!(c.attachments.is_empty());
        c.start_attachment_input();
        assert!(c.entering_attachment);
        c.attachment_input = "/tmp/test.pdf".into();
        c.confirm_attachment();
        assert!(!c.entering_attachment);
        assert_eq!(c.attachments.len(), 1);
        assert_eq!(c.attachments[0], PathBuf::from("/tmp/test.pdf"));
    }

    #[test]
    fn test_add_attachment_empty_path_ignored() {
        let mut c = Compose::new();
        c.start_attachment_input();
        c.attachment_input = "   ".into();
        c.confirm_attachment();
        assert!(c.attachments.is_empty());
    }

    #[test]
    fn test_cancel_attachment_input() {
        let mut c = Compose::new();
        c.start_attachment_input();
        c.attachment_input = "/tmp/test.pdf".into();
        c.cancel_attachment_input();
        assert!(!c.entering_attachment);
        assert!(c.attachments.is_empty());
    }

    #[test]
    fn test_remove_last_attachment() {
        let mut c = Compose::new();
        c.attachments.push(PathBuf::from("/tmp/a.txt"));
        c.attachments.push(PathBuf::from("/tmp/b.txt"));
        c.remove_last_attachment();
        assert_eq!(c.attachments.len(), 1);
        assert_eq!(c.attachments[0], PathBuf::from("/tmp/a.txt"));
    }

    #[test]
    fn test_remove_last_attachment_empty() {
        let mut c = Compose::new();
        c.remove_last_attachment();
        assert!(c.attachments.is_empty());
    }

    #[test]
    fn test_reset_clears_attachments() {
        let mut c = Compose::new();
        c.attachments.push(PathBuf::from("/tmp/file.txt"));
        c.entering_attachment = true;
        c.attachment_input = "something".into();
        c.reset();
        assert!(c.attachments.is_empty());
        assert!(c.attachment_input.is_empty());
        assert!(!c.entering_attachment);
    }

    #[test]
    fn test_attachment_input_handles_keys() {
        use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

        let mut c = Compose::new();
        c.start_attachment_input();

        // Type characters
        let char_key = |ch| KeyEvent {
            code: KeyCode::Char(ch),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        c.handle_key(char_key('/'));
        c.handle_key(char_key('t'));
        c.handle_key(char_key('m'));
        c.handle_key(char_key('p'));
        assert_eq!(c.attachment_input, "/tmp");

        // Backspace
        let bs = KeyEvent {
            code: KeyCode::Backspace,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        c.handle_key(bs);
        assert_eq!(c.attachment_input, "/tm");

        // Enter confirms
        let enter = KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        c.handle_key(enter);
        assert!(!c.entering_attachment);
        assert_eq!(c.attachments.len(), 1);
    }

    #[test]
    fn test_attachment_summary_with_nonexistent_file() {
        let mut c = Compose::new();
        c.attachments.push(PathBuf::from("/nonexistent/file.txt"));
        let summary = c.attachment_summary();
        assert_eq!(summary.len(), 1);
        assert!(summary[0].contains("file.txt"));
        assert!(summary[0].contains("(?)"));
    }

    #[test]
    fn test_multiple_attachments() {
        let mut c = Compose::new();
        c.attachments.push(PathBuf::from("/tmp/a.txt"));
        c.attachments.push(PathBuf::from("/tmp/b.pdf"));
        c.attachments.push(PathBuf::from("/tmp/c.png"));
        assert_eq!(c.attachments.len(), 3);
        let summary = c.attachment_summary();
        assert_eq!(summary.len(), 3);
    }
}
