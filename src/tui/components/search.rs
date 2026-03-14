use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::components::{themed_block, truncate};
use crate::theme::{Theme, ICON_SEARCH};

pub struct SearchPanel {
    pub query: String,
    pub results: Vec<SearchResult>,
    pub state: ListState,
}

pub struct SearchResult {
    pub from: String,
    pub subject: String,
    pub snippet: String,
}

impl SearchPanel {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            results: Vec::new(),
            state: ListState::default(),
        }
    }

    pub fn set_results(&mut self, messages: &[crate::client::Message]) {
        self.results = messages
            .iter()
            .map(|m| SearchResult {
                from: m.from_addr.clone(),
                subject: m.subject.clone().unwrap_or_default(),
                snippet: m.text_body.clone().unwrap_or_default(),
            })
            .collect();
        if self.results.is_empty() {
            self.state.select(None);
        } else {
            self.state.select(Some(0));
        }
    }

    pub fn push_char(&mut self, c: char) {
        self.query.push(c);
    }

    pub fn pop_char(&mut self) {
        self.query.pop();
    }

    pub fn clear(&mut self) {
        self.query.clear();
        self.results.clear();
        self.state.select(None);
    }

    #[cfg(test)]
    pub fn selected(&self) -> Option<usize> {
        self.state.selected()
    }

    pub fn select_next(&mut self) {
        let len = self.results.len();
        if len > 0 {
            let cur = self.state.selected().unwrap_or(0);
            if cur + 1 < len {
                self.state.select(Some(cur + 1));
            }
        }
    }

    pub fn select_prev(&mut self) {
        if let Some(cur) = self.state.selected() {
            if cur > 0 {
                self.state.select(Some(cur - 1));
            }
        }
    }

    pub fn render_input(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let input = Paragraph::new(Line::from(vec![
            Span::styled(
                format!(" {ICON_SEARCH} / "),
                Style::default().fg(theme.accent),
            ),
            Span::styled(&self.query, Style::default().fg(theme.fg)),
            Span::styled("│", Style::default().fg(theme.accent)),
        ]));
        frame.render_widget(input, area);
    }

    pub fn render_results(&mut self, frame: &mut Frame, area: Rect, theme: &Theme, focused: bool) {
        let block = themed_block(format!(" {ICON_SEARCH} Search Results "), theme, focused);

        if self.results.is_empty() {
            let inner = block.inner(area);
            frame.render_widget(block, area);
            let empty = if self.query.is_empty() {
                "Type to search…"
            } else {
                "No results"
            };
            let p = Paragraph::new(Line::from(Span::styled(
                format!("  {empty}"),
                Style::default().fg(theme.muted),
            )));
            frame.render_widget(p, inner);
            return;
        }

        let inner = block.inner(area);
        let has_snippet = inner.height > 3;
        let list_area = if has_snippet {
            Rect {
                height: inner.height.saturating_sub(1),
                ..inner
            }
        } else {
            inner
        };

        let items: Vec<ListItem> = self
            .results
            .iter()
            .map(|r| {
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!(" {:<18}", truncate(&r.from, 18)),
                        Style::default().fg(theme.fg),
                    ),
                    Span::styled(
                        format!(" {}", truncate(&r.subject, 40)),
                        Style::default().fg(theme.muted),
                    ),
                ]))
            })
            .collect();

        let list = List::new(items).block(block).highlight_style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        );

        frame.render_stateful_widget(list, area, &mut self.state);

        if has_snippet {
            if let Some(idx) = self.state.selected() {
                if let Some(result) = self.results.get(idx) {
                    let snippet_area = Rect {
                        y: list_area.y + list_area.height,
                        height: 1,
                        ..inner
                    };
                    let snippet = Paragraph::new(Line::from(Span::styled(
                        format!("  …{}", truncate(&result.snippet, 60)),
                        Style::default().fg(theme.muted),
                    )));
                    frame.render_widget(snippet, snippet_area);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Message;
    use chrono::Utc;
    use uuid::Uuid;

    fn make_messages() -> Vec<Message> {
        vec![
            Message {
                id: Uuid::new_v4(),
                inbox_id: Uuid::new_v4(),
                thread_id: None,
                from_addr: "alice@company.com".into(),
                to_addrs: serde_json::json!(["bob@ex.com"]),
                subject: Some("Re: Meeting tomorrow".into()),
                text_body: Some("confirm our 3pm meeting tomorrow".into()),
                html_body: None,
                direction: "inbound".into(),
                created_at: Utc::now(),
                slop_score: None,
                category: None,
                triage_status: None,
            },
            Message {
                id: Uuid::new_v4(),
                inbox_id: Uuid::new_v4(),
                thread_id: None,
                from_addr: "bob@example.com".into(),
                to_addrs: serde_json::json!(["alice@co.com"]),
                subject: Some("Invoice #4821".into()),
                text_body: Some("please find attached the invoice".into()),
                html_body: None,
                direction: "inbound".into(),
                created_at: Utc::now(),
                slop_score: None,
                category: None,
                triage_status: None,
            },
        ]
    }

    #[test]
    fn test_new_empty() {
        let s = SearchPanel::new();
        assert!(s.query.is_empty());
        assert!(s.results.is_empty());
        assert_eq!(s.selected(), None);
    }

    #[test]
    fn test_set_results_populates() {
        let mut s = SearchPanel::new();
        s.set_results(&make_messages());
        assert_eq!(s.results.len(), 2);
        assert_eq!(s.selected(), Some(0));
    }

    #[test]
    fn test_set_results_empty_clears() {
        let mut s = SearchPanel::new();
        s.set_results(&make_messages());
        s.set_results(&[]);
        assert!(s.results.is_empty());
        assert_eq!(s.selected(), None);
    }

    #[test]
    fn test_push_pop_char() {
        let mut s = SearchPanel::new();
        s.push_char('a');
        s.push_char('b');
        assert_eq!(s.query, "ab");
        s.pop_char();
        assert_eq!(s.query, "a");
    }

    #[test]
    fn test_clear() {
        let mut s = SearchPanel::new();
        s.push_char('x');
        s.set_results(&make_messages());
        s.clear();
        assert!(s.query.is_empty());
        assert!(s.results.is_empty());
    }

    #[test]
    fn test_select_next_prev() {
        let mut s = SearchPanel::new();
        s.set_results(&make_messages());
        assert_eq!(s.selected(), Some(0));
        s.select_next();
        assert_eq!(s.selected(), Some(1));
        s.select_prev();
        assert_eq!(s.selected(), Some(0));
    }

    #[test]
    fn test_select_prev_at_zero() {
        let mut s = SearchPanel::new();
        s.set_results(&make_messages());
        s.select_prev();
        assert_eq!(s.selected(), Some(0));
    }

    #[test]
    fn test_select_next_capped() {
        let mut s = SearchPanel::new();
        s.set_results(&make_messages());
        for _ in 0..100 {
            s.select_next();
        }
        assert_eq!(s.selected(), Some(s.results.len() - 1));
    }
}
