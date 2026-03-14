use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState};
use ratatui::Frame;

use crate::components::{themed_block, truncate};
use crate::theme::{Theme, ICON_DRAFTS};

pub struct DraftPanel {
    pub entries: Vec<DraftEntry>,
    pub state: ListState,
}

pub struct DraftEntry {
    pub to: String,
    pub subject: String,
}

impl DraftPanel {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            state: ListState::default(),
        }
    }

    pub fn set_entries(&mut self, drafts: &[crate::client::Draft]) {
        self.entries = drafts
            .iter()
            .map(|d| {
                let to = d
                    .to_addrs
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                DraftEntry {
                    to,
                    subject: d.subject.clone().unwrap_or_default(),
                }
            })
            .collect();
        if self.entries.is_empty() {
            self.state.select(None);
        } else {
            self.state.select(Some(0));
        }
    }

    pub fn selected(&self) -> usize {
        self.state.selected().unwrap_or(0)
    }

    pub fn select_next(&mut self) {
        let len = self.entries.len();
        if len > 0 {
            let cur = self.selected();
            if cur + 1 < len {
                self.state.select(Some(cur + 1));
            }
        }
    }

    pub fn select_prev(&mut self) {
        let cur = self.selected();
        if cur > 0 {
            self.state.select(Some(cur - 1));
        }
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme, focused: bool) {
        let block = themed_block(format!(" {ICON_DRAFTS} Drafts "), theme, focused);

        if self.entries.is_empty() {
            let inner = block.inner(area);
            frame.render_widget(block, area);
            let p = ratatui::widgets::Paragraph::new(Line::from(Span::styled(
                "  No drafts",
                Style::default().fg(theme.muted),
            )));
            frame.render_widget(p, inner);
            return;
        }

        let items: Vec<ListItem> = self
            .entries
            .iter()
            .map(|e| {
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!(" {:<20}", truncate(&e.to, 20)),
                        Style::default().fg(theme.fg),
                    ),
                    Span::styled(
                        format!(" {}", truncate(&e.subject, 40)),
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Draft;
    use chrono::Utc;
    use uuid::Uuid;

    fn make_draft(to: &str, subject: &str) -> Draft {
        Draft {
            id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
            to_addrs: serde_json::json!([to]),
            subject: Some(subject.into()),
            text_body: Some("draft body".into()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn test_new_starts_empty() {
        let panel = DraftPanel::new();
        assert!(panel.entries.is_empty());
        assert_eq!(panel.state.selected(), None);
    }

    #[test]
    fn test_set_entries_selects_first() {
        let mut panel = DraftPanel::new();
        panel.set_entries(&[
            make_draft("alice@co.com", "Draft 1"),
            make_draft("bob@co.com", "Draft 2"),
        ]);
        assert_eq!(panel.selected(), 0);
        assert_eq!(panel.entries.len(), 2);
    }

    #[test]
    fn test_nav() {
        let mut panel = DraftPanel::new();
        panel.set_entries(&[make_draft("a@co.com", "A"), make_draft("b@co.com", "B")]);
        panel.select_next();
        assert_eq!(panel.selected(), 1);
        panel.select_prev();
        assert_eq!(panel.selected(), 0);
    }
}
