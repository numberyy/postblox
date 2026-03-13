use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::theme::Theme;

pub struct Preview {
    pub from: String,
    pub subject: String,
    #[allow(dead_code)] // Used when data layer wires in Round 3
    pub date: String,
    pub body: String,
    pub scroll: u16,
}

impl Preview {
    pub fn new() -> Self {
        Self {
            from: "alice@company.com".into(),
            subject: "Re: Meeting tomorrow".into(),
            date: "2026-03-13 10:30 UTC".into(),
            body: "Hey,\n\nJust wanted to confirm our 3pm meeting tomorrow. \
                   I'll bring the project update slides.\n\n\
                   Also, could you share the latest metrics report before then?\n\n\
                   Thanks,\nAlice"
                .into(),
            scroll: 0,
        }
    }

    pub fn scroll_down(&mut self) {
        self.scroll = self.scroll.saturating_add(1);
    }

    pub fn scroll_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
    }

    #[allow(dead_code)] // Used when data layer wires in Round 3
    pub fn set_content(&mut self, from: &str, subject: &str, date: &str, body: &str) {
        self.from = from.into();
        self.subject = subject.into();
        self.date = date.into();
        self.body = body.into();
        self.scroll = 0;
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: &Theme, focused: bool) {
        let border_color = if focused {
            theme.border_focused
        } else {
            theme.border
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border_color));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        if inner.height < 2 {
            return;
        }

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Min(1)])
            .split(inner);

        // Header
        let header = Paragraph::new(vec![
            Line::from(vec![
                Span::styled(
                    "From: ",
                    Style::default()
                        .fg(theme.muted)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(&self.from, Style::default().fg(theme.fg)),
                Span::styled("  Subject: ", Style::default().fg(theme.muted)),
                Span::styled(&self.subject, Style::default().fg(theme.fg)),
            ]),
            Line::from(Span::styled(
                "─".repeat(inner.width as usize),
                Style::default().fg(theme.muted),
            )),
        ]);
        frame.render_widget(header, chunks[0]);

        // Body
        let body = Paragraph::new(self.body.as_str())
            .style(Style::default().fg(theme.fg))
            .wrap(Wrap { trim: false })
            .scroll((self.scroll, 0));
        frame.render_widget(body, chunks[1]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_has_mock_content() {
        let p = Preview::new();
        assert!(!p.from.is_empty());
        assert!(!p.body.is_empty());
        assert_eq!(p.scroll, 0);
    }

    #[test]
    fn test_scroll_down_up() {
        let mut p = Preview::new();
        p.scroll_down();
        assert_eq!(p.scroll, 1);
        p.scroll_down();
        assert_eq!(p.scroll, 2);
        p.scroll_up();
        assert_eq!(p.scroll, 1);
    }

    #[test]
    fn test_scroll_up_at_zero() {
        let mut p = Preview::new();
        p.scroll_up();
        assert_eq!(p.scroll, 0);
    }

    #[test]
    fn test_set_content_resets_scroll() {
        let mut p = Preview::new();
        p.scroll_down();
        p.scroll_down();
        p.set_content("bob@ex.com", "New subject", "2026-01-01", "Body");
        assert_eq!(p.scroll, 0);
        assert_eq!(p.from, "bob@ex.com");
        assert_eq!(p.subject, "New subject");
        assert_eq!(p.body, "Body");
    }
}
