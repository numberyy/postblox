use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use crate::theme::Theme;

#[derive(Debug, Clone)]
pub struct ThreadMessage {
    pub from: String,
    pub date: String,
    pub body: String,
}

#[derive(Debug)]
pub struct ThreadPanel {
    pub messages: Vec<ThreadMessage>,
    pub state: ListState,
    pub body_scroll: u16,
}

impl ThreadPanel {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            state: ListState::default(),
            body_scroll: 0,
        }
    }

    pub fn set_messages(&mut self, messages: Vec<ThreadMessage>) {
        let has_any = !messages.is_empty();
        self.messages = messages;
        self.body_scroll = 0;
        if has_any {
            self.state.select(Some(0));
        } else {
            self.state.select(None);
        }
    }

    pub fn clear(&mut self) {
        self.messages.clear();
        self.state.select(None);
        self.body_scroll = 0;
    }

    pub fn selected(&self) -> usize {
        self.state.selected().unwrap_or(0)
    }

    pub fn select_next(&mut self) {
        if self.messages.is_empty() {
            return;
        }
        let cur = self.selected();
        if cur + 1 < self.messages.len() {
            self.state.select(Some(cur + 1));
            self.body_scroll = 0;
        }
    }

    pub fn select_prev(&mut self) {
        let cur = self.selected();
        if cur > 0 {
            self.state.select(Some(cur - 1));
            self.body_scroll = 0;
        }
    }

    pub fn select_first(&mut self) {
        if !self.messages.is_empty() {
            self.state.select(Some(0));
            self.body_scroll = 0;
        }
    }

    pub fn select_last(&mut self) {
        if !self.messages.is_empty() {
            self.state.select(Some(self.messages.len() - 1));
            self.body_scroll = 0;
        }
    }

    pub fn scroll_down(&mut self) {
        if let Some(msg) = self.messages.get(self.selected()) {
            let max = msg.body.lines().count().saturating_sub(1) as u16;
            if self.body_scroll < max {
                self.body_scroll = self.body_scroll.saturating_add(1);
            }
        }
    }

    pub fn scroll_up(&mut self) {
        self.body_scroll = self.body_scroll.saturating_sub(1);
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme, focused: bool) {
        let border_color = if focused {
            theme.border_focused
        } else {
            theme.border
        };

        let title = format!(" Thread ({} messages) ", self.messages.len());
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border_color));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        if self.messages.is_empty() || inner.height < 3 {
            return;
        }

        let list_height = (self.messages.len() as u16).min(inner.height / 3).max(2);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(list_height), Constraint::Min(1)])
            .split(inner);

        let items: Vec<ListItem> = self
            .messages
            .iter()
            .map(|msg| {
                ListItem::new(Line::from(vec![
                    Span::styled(&msg.from, Style::default().fg(theme.fg)),
                    Span::styled(format!("  {}", msg.date), Style::default().fg(theme.muted)),
                ]))
            })
            .collect();

        let list = List::new(items).highlight_style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        );

        frame.render_stateful_widget(list, chunks[0], &mut self.state);

        if let Some(msg) = self.messages.get(self.selected()) {
            let header = Line::from(vec![
                Span::styled(
                    "From: ",
                    Style::default()
                        .fg(theme.muted)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(&msg.from, Style::default().fg(theme.fg)),
                Span::styled(format!("  {}", msg.date), Style::default().fg(theme.muted)),
            ]);

            let sep = Line::from(Span::styled(
                "─".repeat(chunks[1].width as usize),
                Style::default().fg(theme.muted),
            ));

            let mut lines = vec![header, sep];
            for line in msg.body.lines() {
                lines.push(Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(theme.fg),
                )));
            }

            let body = Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .scroll((self.body_scroll, 0));

            frame.render_widget(body, chunks[1]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_messages(count: usize) -> Vec<ThreadMessage> {
        (0..count)
            .map(|i| ThreadMessage {
                from: format!("user{i}@example.com"),
                date: format!("2026-03-1{i} 10:00"),
                body: format!("Message body {i}\nLine 2\nLine 3"),
            })
            .collect()
    }

    #[test]
    fn test_new_starts_empty() {
        let panel = ThreadPanel::new();
        assert!(panel.messages.is_empty());
        assert_eq!(panel.state.selected(), None);
        assert_eq!(panel.body_scroll, 0);
    }

    #[test]
    fn test_set_messages_selects_first() {
        let mut panel = ThreadPanel::new();
        panel.set_messages(make_messages(3));
        assert_eq!(panel.messages.len(), 3);
        assert_eq!(panel.state.selected(), Some(0));
        assert_eq!(panel.body_scroll, 0);
    }

    #[test]
    fn test_set_messages_empty_selects_none() {
        let mut panel = ThreadPanel::new();
        panel.set_messages(make_messages(3));
        panel.set_messages(Vec::new());
        assert!(panel.messages.is_empty());
        assert_eq!(panel.state.selected(), None);
    }

    #[test]
    fn test_select_next_prev() {
        let mut panel = ThreadPanel::new();
        panel.set_messages(make_messages(4));
        assert_eq!(panel.selected(), 0);

        panel.select_next();
        assert_eq!(panel.selected(), 1);

        panel.select_next();
        assert_eq!(panel.selected(), 2);

        panel.select_prev();
        assert_eq!(panel.selected(), 1);
    }

    #[test]
    fn test_select_next_capped() {
        let mut panel = ThreadPanel::new();
        panel.set_messages(make_messages(3));
        panel.select_next();
        panel.select_next();
        panel.select_next(); // should not go past 2
        panel.select_next();
        assert_eq!(panel.selected(), 2);
    }

    #[test]
    fn test_select_prev_at_zero() {
        let mut panel = ThreadPanel::new();
        panel.set_messages(make_messages(3));
        panel.select_prev(); // already at 0
        assert_eq!(panel.selected(), 0);
    }

    #[test]
    fn test_select_first_last() {
        let mut panel = ThreadPanel::new();
        panel.set_messages(make_messages(5));
        panel.select_last();
        assert_eq!(panel.selected(), 4);
        panel.select_first();
        assert_eq!(panel.selected(), 0);
    }

    #[test]
    fn test_scroll_down_up() {
        let mut panel = ThreadPanel::new();
        panel.set_messages(make_messages(2));
        assert_eq!(panel.body_scroll, 0);

        panel.scroll_down();
        assert_eq!(panel.body_scroll, 1);

        panel.scroll_down();
        assert_eq!(panel.body_scroll, 2);

        panel.scroll_up();
        assert_eq!(panel.body_scroll, 1);
    }

    #[test]
    fn test_scroll_up_at_zero() {
        let mut panel = ThreadPanel::new();
        panel.set_messages(make_messages(1));
        panel.scroll_up();
        assert_eq!(panel.body_scroll, 0);
    }

    #[test]
    fn test_scroll_resets_on_select() {
        let mut panel = ThreadPanel::new();
        panel.set_messages(make_messages(3));
        panel.scroll_down();
        panel.scroll_down();
        assert!(panel.body_scroll > 0);

        panel.select_next();
        assert_eq!(panel.body_scroll, 0);
    }

    #[test]
    fn test_clear_resets_all() {
        let mut panel = ThreadPanel::new();
        panel.set_messages(make_messages(3));
        panel.select_next();
        panel.scroll_down();

        panel.clear();
        assert!(panel.messages.is_empty());
        assert_eq!(panel.state.selected(), None);
        assert_eq!(panel.body_scroll, 0);
    }

    #[test]
    fn test_scroll_capped_at_body_lines() {
        let mut panel = ThreadPanel::new();
        panel.set_messages(vec![ThreadMessage {
            from: "a@b.com".into(),
            date: "2026-01-01".into(),
            body: "line1\nline2".into(), // 2 lines → max scroll = 1
        }]);
        panel.scroll_down();
        assert_eq!(panel.body_scroll, 1);
        panel.scroll_down(); // should stay at 1
        assert_eq!(panel.body_scroll, 1);
    }

    #[test]
    fn test_select_next_on_empty() {
        let mut panel = ThreadPanel::new();
        panel.select_next();
        assert_eq!(panel.state.selected(), None);
    }

    #[test]
    fn test_select_first_last_on_empty() {
        let mut panel = ThreadPanel::new();
        panel.select_first();
        panel.select_last();
        assert_eq!(panel.state.selected(), None);
    }
}
