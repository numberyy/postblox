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
            entries: Vec::new(),
            state: ListState::default(),
        }
    }

    pub fn set_entries(&mut self, approvals: &[crate::client::Approval]) {
        self.entries = approvals
            .iter()
            .map(|a| ApprovalEntry {
                from: a.from_addr.clone().unwrap_or_default(),
                subject: a.subject.clone().unwrap_or_default(),
                inbox: a.inbox_email.clone().unwrap_or_default(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Approval;
    use chrono::Utc;
    use uuid::Uuid;

    fn make_approval(from: &str, subject: &str, inbox: &str) -> Approval {
        Approval {
            id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
            message_id: Uuid::new_v4(),
            status: "pending".into(),
            created_at: Utc::now(),
            subject: Some(subject.into()),
            from_addr: Some(from.into()),
            inbox_email: Some(inbox.into()),
        }
    }

    fn populated_panel() -> ApprovalPanel {
        let mut panel = ApprovalPanel::new();
        let approvals = vec![
            make_approval("agent-1@ai.local", "Invoice follow-up", "hello@pb.dev"),
            make_approval("agent-2@ai.local", "Partnership reply", "hello@pb.dev"),
            make_approval("agent-3@ai.local", "Support ticket #42", "support@pb.dev"),
        ];
        panel.set_entries(&approvals);
        panel
    }

    #[test]
    fn test_new_starts_empty() {
        let panel = ApprovalPanel::new();
        assert!(panel.entries.is_empty());
        assert_eq!(panel.state.selected(), None);
    }

    #[test]
    fn test_set_entries_selects_first() {
        let panel = populated_panel();
        assert_eq!(panel.selected(), 0);
        assert_eq!(panel.entries.len(), 3);
    }

    #[test]
    fn test_select_next_prev() {
        let mut panel = populated_panel();
        panel.select_next();
        assert_eq!(panel.selected(), 1);
        panel.select_prev();
        assert_eq!(panel.selected(), 0);
    }

    #[test]
    fn test_select_next_capped() {
        let mut panel = populated_panel();
        for _ in 0..100 {
            panel.select_next();
        }
        assert_eq!(panel.selected(), panel.entries.len() - 1);
    }

    #[test]
    fn test_select_prev_at_zero() {
        let mut panel = populated_panel();
        panel.select_prev();
        assert_eq!(panel.selected(), 0);
    }

    #[test]
    fn test_set_entries_empty_clears_selection() {
        let mut panel = populated_panel();
        panel.set_entries(&[]);
        assert!(panel.entries.is_empty());
        assert_eq!(panel.state.selected(), None);
    }
}
