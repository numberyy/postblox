use chrono::{DateTime, Utc};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState};
use ratatui::Frame;

use crate::components::{themed_block, truncate};
use crate::theme::{Theme, ICON_MESSAGE, ICON_SLOP};

pub struct MessageList {
    pub entries: Vec<MessageEntry>,
    pub state: ListState,
}

pub struct MessageEntry {
    pub from: String,
    pub subject: String,
    pub date: DateTime<Utc>,
    pub is_slop: bool,
}

impl MessageList {
    pub fn new() -> Self {
        Self {
            entries: mock_messages(),
            state: ListState::default().with_selected(Some(0)),
        }
    }

    pub fn selected(&self) -> usize {
        self.state.selected().unwrap_or(0)
    }

    #[allow(dead_code)] // Used when data layer wires in Round 3
    pub fn select(&mut self, idx: usize) {
        if idx < self.entries.len() {
            self.state.select(Some(idx));
        }
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

    pub fn select_first(&mut self) {
        if !self.entries.is_empty() {
            self.state.select(Some(0));
        }
    }

    pub fn select_last(&mut self) {
        if !self.entries.is_empty() {
            self.state.select(Some(self.entries.len() - 1));
        }
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme, focused: bool) {
        let block = themed_block(format!(" {ICON_MESSAGE} Messages "), theme, focused);

        let items: Vec<ListItem> = self
            .entries
            .iter()
            .map(|e| render_entry(e, theme))
            .collect();

        let list = List::new(items).block(block).highlight_style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        );

        frame.render_stateful_widget(list, area, &mut self.state);
    }
}

fn render_entry<'a>(entry: &MessageEntry, theme: &Theme) -> ListItem<'a> {
    let from = truncate(&entry.from, 16);
    let subject = truncate(&entry.subject, 28);
    let age = format_age(entry.date);
    let slop = if entry.is_slop {
        format!(" {ICON_SLOP}")
    } else {
        String::new()
    };

    ListItem::new(Line::from(vec![
        Span::styled(format!(" {from:<16}"), Style::default().fg(theme.fg)),
        Span::styled(format!(" {subject:<28}"), Style::default().fg(theme.muted)),
        Span::styled(format!(" {age:>6}"), Style::default().fg(theme.muted)),
        Span::styled(slop, Style::default().fg(theme.warning)),
    ]))
}

pub fn format_age(date: DateTime<Utc>) -> String {
    format_age_from(Utc::now(), date)
}

fn format_age_from(now: DateTime<Utc>, date: DateTime<Utc>) -> String {
    let secs = now.signed_duration_since(date).num_seconds().max(0);
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}

fn mock_messages() -> Vec<MessageEntry> {
    let now = Utc::now();
    vec![
        MessageEntry {
            from: "alice@company.com".into(),
            subject: "Re: Meeting tomorrow".into(),
            date: now - chrono::Duration::minutes(3),
            is_slop: false,
        },
        MessageEntry {
            from: "bob@example.com".into(),
            subject: "Invoice #4821".into(),
            date: now - chrono::Duration::hours(1),
            is_slop: false,
        },
        MessageEntry {
            from: "carol@newsletter.io".into(),
            subject: "Weekly digest".into(),
            date: now - chrono::Duration::hours(2),
            is_slop: true,
        },
        MessageEntry {
            from: "dave@startup.co".into(),
            subject: "Partnership proposal".into(),
            date: now - chrono::Duration::hours(5),
            is_slop: false,
        },
        MessageEntry {
            from: "eve@spam.biz".into(),
            subject: "You've won $1,000,000!".into(),
            date: now - chrono::Duration::days(1),
            is_slop: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_selects_first() {
        let list = MessageList::new();
        assert_eq!(list.selected(), 0);
    }

    #[test]
    fn test_select_next_prev() {
        let mut list = MessageList::new();
        list.select_next();
        assert_eq!(list.selected(), 1);
        list.select_next();
        assert_eq!(list.selected(), 2);
        list.select_prev();
        assert_eq!(list.selected(), 1);
    }

    #[test]
    fn test_select_next_capped() {
        let mut list = MessageList::new();
        for _ in 0..100 {
            list.select_next();
        }
        assert_eq!(list.selected(), list.entries.len() - 1);
    }

    #[test]
    fn test_select_prev_at_zero() {
        let mut list = MessageList::new();
        list.select_prev();
        assert_eq!(list.selected(), 0);
    }

    #[test]
    fn test_select_first_last() {
        let mut list = MessageList::new();
        list.select_last();
        assert_eq!(list.selected(), list.entries.len() - 1);
        list.select_first();
        assert_eq!(list.selected(), 0);
    }

    #[test]
    fn test_select_out_of_bounds() {
        let mut list = MessageList::new();
        list.select(999);
        assert_eq!(list.selected(), 0); // unchanged
    }

    #[test]
    fn test_format_age_seconds() {
        let date = Utc::now() - chrono::Duration::seconds(30);
        let age = format_age(date);
        assert!(age.contains("s ago"), "got: {age}");
    }

    #[test]
    fn test_format_age_minutes() {
        let date = Utc::now() - chrono::Duration::minutes(15);
        let age = format_age(date);
        assert!(age.contains("m ago"), "got: {age}");
    }

    #[test]
    fn test_format_age_hours() {
        let date = Utc::now() - chrono::Duration::hours(5);
        let age = format_age(date);
        assert!(age.contains("h ago"), "got: {age}");
    }

    #[test]
    fn test_format_age_days() {
        let date = Utc::now() - chrono::Duration::days(3);
        let age = format_age(date);
        assert!(age.contains("d ago"), "got: {age}");
    }

    #[test]
    fn test_format_age_future_date_clamps_to_zero() {
        let date = Utc::now() + chrono::Duration::hours(1);
        let age = format_age(date);
        assert_eq!(age, "0s ago");
    }

    #[test]
    fn test_mock_messages_not_empty() {
        let msgs = mock_messages();
        assert!(!msgs.is_empty());
        assert!(msgs.iter().any(|m| m.is_slop));
        assert!(msgs.iter().any(|m| !m.is_slop));
    }
}
