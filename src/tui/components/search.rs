use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::components::truncate;
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

    #[allow(dead_code)] // Used when data layer wires in Round 3
    pub fn set_query(&mut self, query: String) {
        self.query = query;
        self.results = if self.query.is_empty() {
            Vec::new()
        } else {
            mock_results()
        };
        if !self.results.is_empty() {
            self.state.select(Some(0));
        } else {
            self.state.select(None);
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

    #[allow(dead_code)] // Used when data layer wires in Round 3
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
        let border_color = if focused {
            theme.border_focused
        } else {
            theme.border
        };

        let block = Block::default()
            .title(format!(" {ICON_SEARCH} Search Results "))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border_color));

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

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1)])
            .split(block.inner(area));

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

        // Show snippet for selected
        if let Some(idx) = self.state.selected() {
            if let Some(result) = self.results.get(idx) {
                if chunks[0].height > 2 {
                    let snippet_area = Rect {
                        y: chunks[0].y + chunks[0].height.saturating_sub(1),
                        height: 1,
                        ..chunks[0]
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

#[allow(dead_code)] // Used by set_query which is wired in Round 3
fn mock_results() -> Vec<SearchResult> {
    vec![
        SearchResult {
            from: "alice@company.com".into(),
            subject: "Re: Meeting tomorrow".into(),
            snippet: "confirm our 3pm meeting tomorrow".into(),
        },
        SearchResult {
            from: "bob@example.com".into(),
            subject: "Invoice #4821".into(),
            snippet: "please find attached the invoice".into(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_empty() {
        let s = SearchPanel::new();
        assert!(s.query.is_empty());
        assert!(s.results.is_empty());
        assert_eq!(s.selected(), None);
    }

    #[test]
    fn test_set_query_populates_results() {
        let mut s = SearchPanel::new();
        s.set_query("meeting".into());
        assert!(!s.results.is_empty());
        assert_eq!(s.selected(), Some(0));
    }

    #[test]
    fn test_set_query_empty_clears() {
        let mut s = SearchPanel::new();
        s.set_query("meeting".into());
        s.set_query(String::new());
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
        s.set_query("test".into());
        s.clear();
        assert!(s.query.is_empty());
        assert!(s.results.is_empty());
    }

    #[test]
    fn test_select_next_prev() {
        let mut s = SearchPanel::new();
        s.set_query("test".into());
        assert_eq!(s.selected(), Some(0));
        s.select_next();
        assert_eq!(s.selected(), Some(1));
        s.select_prev();
        assert_eq!(s.selected(), Some(0));
    }

    #[test]
    fn test_select_prev_at_zero() {
        let mut s = SearchPanel::new();
        s.set_query("test".into());
        s.select_prev();
        assert_eq!(s.selected(), Some(0));
    }

    #[test]
    fn test_select_next_capped() {
        let mut s = SearchPanel::new();
        s.set_query("test".into());
        for _ in 0..100 {
            s.select_next();
        }
        assert_eq!(s.selected(), Some(s.results.len() - 1));
    }
}
