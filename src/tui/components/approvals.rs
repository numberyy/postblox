use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState};
use ratatui::Frame;

use crate::components::{themed_block, truncate};
use crate::theme::{Theme, ICON_APPROVAL};

pub struct ApprovalPanel {
    pub entries: Vec<ApprovalEntry>,
    pub state: ListState,
}

pub struct ApprovalEntry {
    pub from: String,
    pub subject: String,
    pub inbox: String,
}

impl ApprovalPanel {
    pub fn new() -> Self {
        Self {
            entries: mock_approvals(),
            state: ListState::default().with_selected(Some(0)),
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
        let block = themed_block(format!(" {ICON_APPROVAL} Approvals "), theme, focused);

        let items: Vec<ListItem> = self
            .entries
            .iter()
            .map(|e| {
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!(" {:<20}", truncate(&e.from, 20)),
                        Style::default().fg(theme.fg),
                    ),
                    Span::styled(
                        format!(" {:<30}", truncate(&e.subject, 30)),
                        Style::default().fg(theme.muted),
                    ),
                    Span::styled(format!(" [{}]", e.inbox), Style::default().fg(theme.muted)),
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

fn mock_approvals() -> Vec<ApprovalEntry> {
    vec![
        ApprovalEntry {
            from: "agent-1@ai.local".into(),
            subject: "Outbound: Invoice follow-up".into(),
            inbox: "hello@pb.dev".into(),
        },
        ApprovalEntry {
            from: "agent-2@ai.local".into(),
            subject: "Outbound: Partnership reply".into(),
            inbox: "hello@pb.dev".into(),
        },
        ApprovalEntry {
            from: "agent-3@ai.local".into(),
            subject: "Outbound: Support ticket #42".into(),
            inbox: "support@pb.dev".into(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_selects_first() {
        let panel = ApprovalPanel::new();
        assert_eq!(panel.selected(), 0);
        assert_eq!(panel.entries.len(), 3);
    }

    #[test]
    fn test_select_next_prev() {
        let mut panel = ApprovalPanel::new();
        panel.select_next();
        assert_eq!(panel.selected(), 1);
        panel.select_prev();
        assert_eq!(panel.selected(), 0);
    }

    #[test]
    fn test_select_next_capped() {
        let mut panel = ApprovalPanel::new();
        for _ in 0..100 {
            panel.select_next();
        }
        assert_eq!(panel.selected(), panel.entries.len() - 1);
    }

    #[test]
    fn test_select_prev_at_zero() {
        let mut panel = ApprovalPanel::new();
        panel.select_prev();
        assert_eq!(panel.selected(), 0);
    }
}
