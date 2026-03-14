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
    pub direction: String,
    pub category: Option<String>,
}

impl MessageList {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            state: ListState::default(),
        }
    }

    pub fn set_entries(&mut self, messages: &[crate::client::Message]) {
        self.entries = messages
            .iter()
            .map(|m| MessageEntry {
                from: m.from_addr.clone(),
                subject: m.subject.clone().unwrap_or_default(),
                date: m.created_at,
                is_slop: m.triage_status.as_deref() == Some("slopified"),
                direction: m.direction.clone(),
                category: m.category.clone(),
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

    #[cfg(test)]
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
    let dir = if entry.direction == "outbound" {
        "→"
    } else {
        "←"
    };
    let from = truncate(&entry.from, 15);
    let subject = truncate(&entry.subject, 26);
    let age = format_age(entry.date);

    let mut spans = vec![
        Span::styled(
            format!(" {dir}"),
            Style::default().fg(if entry.direction == "outbound" {
                theme.accent
            } else {
                theme.muted
            }),
        ),
        Span::styled(format!(" {from:<15}"), Style::default().fg(theme.fg)),
        Span::styled(format!(" {subject:<26}"), Style::default().fg(theme.muted)),
        Span::styled(format!(" {age:>6}"), Style::default().fg(theme.muted)),
    ];
    if entry.is_slop {
        spans.push(Span::styled(
            format!(" {ICON_SLOP}"),
            Style::default().fg(theme.warning),
        ));
    }
    if let Some(ref cat) = entry.category {
        spans.push(Span::styled(
            format!(" [{cat}]"),
            Style::default().fg(theme.accent),
        ));
    }
    ListItem::new(Line::from(spans))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Message;
    use uuid::Uuid;

    fn make_message(from: &str, subject: &str, slop: bool) -> Message {
        Message {
            id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
            thread_id: None,
            from_addr: from.into(),
            to_addrs: serde_json::json!(["bob@example.com"]),
            subject: Some(subject.into()),
            text_body: Some("body".into()),
            html_body: None,
            direction: "inbound".into(),
            created_at: Utc::now(),
            slop_score: None,
            category: None,
            triage_status: if slop { Some("slopified".into()) } else { None },
        }
    }

    #[test]
    fn test_direction_and_category_set() {
        let mut list = MessageList::new();
        let mut msg = make_message("alice@co.com", "Test", false);
        msg.direction = "outbound".into();
        msg.category = Some("sales".into());
        list.set_entries(&[msg]);
        assert_eq!(list.entries[0].direction, "outbound");
        assert_eq!(list.entries[0].category.as_deref(), Some("sales"));
    }

    fn populated_list() -> MessageList {
        let mut list = MessageList::new();
        let messages = vec![
            make_message("alice@co.com", "Re: Meeting", false),
            make_message("bob@ex.com", "Invoice #4821", false),
            make_message("carol@news.io", "Weekly digest", true),
            make_message("dave@startup.co", "Partnership", false),
            make_message("eve@spam.biz", "You've won!", true),
        ];
        list.set_entries(&messages);
        list
    }

    #[test]
    fn test_new_starts_empty() {
        let list = MessageList::new();
        assert!(list.entries.is_empty());
        assert_eq!(list.state.selected(), None);
    }

    #[test]
    fn test_set_entries_selects_first() {
        let list = populated_list();
        assert_eq!(list.selected(), 0);
        assert_eq!(list.entries.len(), 5);
    }

    #[test]
    fn test_set_entries_marks_slop() {
        let list = populated_list();
        assert!(!list.entries[0].is_slop);
        assert!(list.entries[2].is_slop);
    }

    #[test]
    fn test_select_next_prev() {
        let mut list = populated_list();
        list.select_next();
        assert_eq!(list.selected(), 1);
        list.select_next();
        assert_eq!(list.selected(), 2);
        list.select_prev();
        assert_eq!(list.selected(), 1);
    }

    #[test]
    fn test_select_next_capped() {
        let mut list = populated_list();
        for _ in 0..100 {
            list.select_next();
        }
        assert_eq!(list.selected(), list.entries.len() - 1);
    }

    #[test]
    fn test_select_prev_at_zero() {
        let mut list = populated_list();
        list.select_prev();
        assert_eq!(list.selected(), 0);
    }

    #[test]
    fn test_select_first_last() {
        let mut list = populated_list();
        list.select_last();
        assert_eq!(list.selected(), list.entries.len() - 1);
        list.select_first();
        assert_eq!(list.selected(), 0);
    }

    #[test]
    fn test_select_out_of_bounds() {
        let mut list = populated_list();
        list.select(999);
        assert_eq!(list.selected(), 0); // unchanged
    }

    #[test]
    fn test_set_entries_empty_clears_selection() {
        let mut list = populated_list();
        list.set_entries(&[]);
        assert!(list.entries.is_empty());
        assert_eq!(list.state.selected(), None);
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
}
