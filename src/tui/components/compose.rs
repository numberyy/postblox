use crossterm::event::KeyEvent;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;
use tui_textarea::TextArea;

use crate::theme::Theme;

pub struct Compose {
    pub to: String,
    pub subject: String,
    pub textarea: TextArea<'static>,
    pub field: ComposeField,
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
        }
    }

    pub fn reset(&mut self) {
        self.to.clear();
        self.subject.clear();
        self.textarea = TextArea::default();
        self.textarea.set_cursor_line_style(Style::default());
        self.field = ComposeField::To;
    }

    pub fn body_text(&self) -> String {
        self.textarea.lines().join("\n")
    }

    pub fn next_field(&mut self) {
        self.field = match self.field {
            ComposeField::To => ComposeField::Subject,
            ComposeField::Subject => ComposeField::Body,
            ComposeField::Body => ComposeField::Body,
        };
    }

    pub fn handle_key_for_header(&mut self, key: KeyEvent) {
        use crossterm::event::KeyCode;
        match key.code {
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
            KeyCode::Tab => self.next_field(),
            _ => {}
        }
    }

    pub fn handle_key_for_body(&mut self, key: KeyEvent) {
        self.textarea.input(key);
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme, _focused: bool) {
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

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
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

        let hint = Paragraph::new(Line::from(vec![Span::styled(
            "  Ctrl+Enter: send │ Tab: next field │ Esc: cancel",
            Style::default().fg(theme.muted),
        )]));
        frame.render_widget(hint, chunks[3]);
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
    fn test_next_field_cycles() {
        let mut c = Compose::new();
        assert_eq!(c.field, ComposeField::To);
        c.next_field();
        assert_eq!(c.field, ComposeField::Subject);
        c.next_field();
        assert_eq!(c.field, ComposeField::Body);
        c.next_field();
        assert_eq!(c.field, ComposeField::Body); // stays at body
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
}
