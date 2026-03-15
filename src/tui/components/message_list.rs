use chrono::{DateTime, Utc};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::components::{themed_block, truncate};
use crate::theme::{Theme, ICON_ATTACHMENT, ICON_MESSAGE, ICON_SLOP};

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
    pub has_thread: bool,
    pub has_attachments: bool,
    pub inbox_label: Option<String>,
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
                has_thread: m.thread_id.is_some(),
                has_attachments: false,
                inbox_label: None,
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

    pub fn set_inbox_labels_from_messages(
        &mut self,
        messages: &[crate::client::Message],
        inbox_map: &std::collections::HashMap<uuid::Uuid, String>,
    ) {
        for (entry, msg) in self.entries.iter_mut().zip(messages.iter()) {
            entry.inbox_label = inbox_map.get(&msg.inbox_id).cloned();
        }
    }

    pub fn mark_has_attachments(
        &mut self,
        message_id: uuid::Uuid,
        messages: &[crate::client::Message],
    ) {
        for (entry, msg) in self.entries.iter_mut().zip(messages.iter()) {
            if msg.id == message_id {
                entry.has_attachments = true;
                break;
            }
        }
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme, focused: bool) {
        let block = themed_block(format!(" {ICON_MESSAGE} Messages "), theme, focused);

        if self.entries.is_empty() {
            let inner = block.inner(area);
            frame.render_widget(block, area);
            let p = Paragraph::new(Line::from(Span::styled(
                "  Select an inbox or press ? for help",
                Style::default().fg(theme.muted),
            )));
            frame.render_widget(p, inner);
            return;
        }

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

    let mut spans = vec![Span::styled(
        format!(" {dir}"),
        Style::default().fg(if entry.direction == "outbound" {
            theme.accent
        } else {
            theme.muted
        }),
    )];

    if let Some(ref label) = entry.inbox_label {
        let short = truncate(label, 10);
        spans.push(Span::styled(
            format!(" [{short}]"),
            Style::default().fg(theme.accent),
        ));
    }

    let from = truncate(&entry.from, 15);
    let subject = truncate(&entry.subject, 26);
    let age = format_age(entry.date);

    spans.push(Span::styled(
        format!(" {from:<15}"),
        Style::default().fg(theme.fg),
    ));
    spans.push(Span::styled(
        format!(" {subject:<26}"),
        Style::default().fg(theme.muted),
    ));
    spans.push(Span::styled(
        format!(" {age:>6}"),
        Style::default().fg(theme.muted),
    ));
    if entry.has_thread {
        spans.push(Span::styled(" ⤷", Style::default().fg(theme.muted)));
    }
    if entry.has_attachments {
        spans.push(Span::styled(
            format!(" {ICON_ATTACHMENT}"),
            Style::default().fg(theme.accent),
        ));
    }
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
    fn test_set_entries_marks_thread() {
        let mut list = MessageList::new();
        let mut msg = make_message("alice@co.com", "Test", false);
        msg.thread_id = Some(Uuid::new_v4());
        list.set_entries(&[msg]);
        assert!(list.entries[0].has_thread);
    }

    #[test]
    fn test_set_entries_no_thread_when_none() {
        let list = populated_list();
        assert!(!list.entries[0].has_thread);
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
    fn test_set_entries_defaults_no_attachments() {
        let list = populated_list();
        assert!(!list.entries[0].has_attachments);
        assert!(list.entries[0].inbox_label.is_none());
    }

    #[test]
    fn test_mark_has_attachments() {
        let mut list = MessageList::new();
        let msgs = vec![
            make_message("alice@co.com", "A", false),
            make_message("bob@co.com", "B", false),
        ];
        list.set_entries(&msgs);
        assert!(!list.entries[0].has_attachments);
        assert!(!list.entries[1].has_attachments);
        list.mark_has_attachments(msgs[1].id, &msgs);
        assert!(!list.entries[0].has_attachments);
        assert!(list.entries[1].has_attachments);
    }

    #[test]
    fn test_mark_has_attachments_unknown_id() {
        let mut list = MessageList::new();
        let msgs = vec![make_message("alice@co.com", "A", false)];
        list.set_entries(&msgs);
        list.mark_has_attachments(Uuid::new_v4(), &msgs);
        assert!(!list.entries[0].has_attachments);
    }

    #[test]
    fn test_set_inbox_labels_from_messages() {
        let mut list = MessageList::new();
        let inbox_id1 = Uuid::new_v4();
        let inbox_id2 = Uuid::new_v4();
        let mut msg1 = make_message("alice@co.com", "A", false);
        let mut msg2 = make_message("bob@co.com", "B", false);
        msg1.inbox_id = inbox_id1;
        msg2.inbox_id = inbox_id2;
        let msgs = vec![msg1, msg2];
        list.set_entries(&msgs);

        let mut inbox_map = std::collections::HashMap::new();
        inbox_map.insert(inbox_id1, "hello@pb.dev".to_string());
        inbox_map.insert(inbox_id2, "support@pb.dev".to_string());

        list.set_inbox_labels_from_messages(&msgs, &inbox_map);
        assert_eq!(list.entries[0].inbox_label.as_deref(), Some("hello@pb.dev"));
        assert_eq!(
            list.entries[1].inbox_label.as_deref(),
            Some("support@pb.dev")
        );
    }
}
