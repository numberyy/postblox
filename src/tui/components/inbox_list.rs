use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState};
use ratatui::Frame;

use crate::components::{themed_block, truncate};
use crate::theme::{Theme, ICON_APPROVAL, ICON_BRIEFING, ICON_DRAFTS, ICON_INBOX, ICON_SEARCH};

pub struct InboxList {
    pub items: Vec<SidebarItem>,
    pub state: ListState,
}

pub enum SidebarItem {
    AllInboxes,
    Inbox { email: String, active: bool },
    Divider,
    Approvals { pending: usize },
    Drafts,
    Briefing,
    Search,
}

impl InboxList {
    pub fn new() -> Self {
        Self {
            items: default_items(),
            state: ListState::default().with_selected(Some(0)),
        }
    }

    pub fn set_inboxes(&mut self, inboxes: &[crate::client::Inbox], pending_approvals: usize) {
        let prev = self.logical_selected();
        let mut items = Vec::with_capacity(inboxes.len() + 5);
        items.push(SidebarItem::AllInboxes);
        for inbox in inboxes {
            items.push(SidebarItem::Inbox {
                email: inbox.email.clone(),
                active: inbox.active,
            });
        }
        items.push(SidebarItem::Divider);
        items.push(SidebarItem::Approvals {
            pending: pending_approvals,
        });
        items.push(SidebarItem::Drafts);
        items.push(SidebarItem::Briefing);
        items.push(SidebarItem::Search);
        self.items = items;
        let max = selectable_count(&self.items);
        if prev < max {
            self.select(prev);
        } else if max > 0 {
            self.select(0);
        }
    }

    #[cfg(test)]
    pub fn selected(&self) -> usize {
        self.state.selected().unwrap_or(0)
    }

    pub fn select(&mut self, idx: usize) {
        let max = selectable_count(&self.items);
        if idx < max {
            self.state.select(Some(to_visual_index(&self.items, idx)));
        }
    }

    pub fn select_next(&mut self) {
        let max = selectable_count(&self.items);
        let cur = self.logical_selected();
        if cur + 1 < max {
            self.select(cur + 1);
        }
    }

    pub fn select_prev(&mut self) {
        let cur = self.logical_selected();
        if cur > 0 {
            self.select(cur - 1);
        }
    }

    pub fn select_first(&mut self) {
        self.select(0);
    }

    pub fn select_last(&mut self) {
        let max = selectable_count(&self.items);
        if max > 0 {
            self.select(max - 1);
        }
    }

    pub fn inbox_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| matches!(i, SidebarItem::AllInboxes | SidebarItem::Inbox { .. }))
            .count()
    }

    pub fn logical_selected(&self) -> usize {
        let vis = self.state.selected().unwrap_or(0);
        from_visual_index(&self.items, vis)
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme, focused: bool) {
        let block = themed_block(format!(" {ICON_INBOX} postblox "), theme, focused);

        let items: Vec<ListItem> = self
            .items
            .iter()
            .map(|item| render_item(item, theme))
            .collect();

        let list = List::new(items).block(block).highlight_style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        );

        frame.render_stateful_widget(list, area, &mut self.state);
    }
}

fn render_item<'a>(item: &SidebarItem, theme: &Theme) -> ListItem<'a> {
    match item {
        SidebarItem::AllInboxes => ListItem::new(Line::from(vec![Span::styled(
            "  All Inboxes",
            Style::default().fg(theme.fg),
        )])),
        SidebarItem::Inbox { email, active } => {
            let marker = if *active { " ▶ " } else { "   " };
            let label = truncate(email, 14);
            ListItem::new(Line::from(Span::styled(
                format!("{marker}{label}"),
                Style::default().fg(theme.fg),
            )))
        }
        SidebarItem::Divider => ListItem::new(Line::from(Span::styled(
            " ─────────────────",
            Style::default().fg(theme.muted),
        ))),
        SidebarItem::Approvals { pending } => {
            let badge = if *pending > 0 {
                format!(" ({pending})")
            } else {
                String::new()
            };
            ListItem::new(Line::from(vec![Span::styled(
                format!("  {ICON_APPROVAL} Approvals{badge}"),
                Style::default().fg(theme.fg),
            )]))
        }
        SidebarItem::Drafts => ListItem::new(Line::from(vec![Span::styled(
            format!("  {ICON_DRAFTS} Drafts"),
            Style::default().fg(theme.fg),
        )])),
        SidebarItem::Briefing => ListItem::new(Line::from(vec![Span::styled(
            format!("  {ICON_BRIEFING} Briefing"),
            Style::default().fg(theme.fg),
        )])),
        SidebarItem::Search => ListItem::new(Line::from(vec![Span::styled(
            format!("  {ICON_SEARCH} Search"),
            Style::default().fg(theme.fg),
        )])),
    }
}

fn selectable_count(items: &[SidebarItem]) -> usize {
    items
        .iter()
        .filter(|i| !matches!(i, SidebarItem::Divider))
        .count()
}

fn to_visual_index(items: &[SidebarItem], logical: usize) -> usize {
    let mut count = 0;
    for (i, item) in items.iter().enumerate() {
        if matches!(item, SidebarItem::Divider) {
            continue;
        }
        if count == logical {
            return i;
        }
        count += 1;
    }
    0
}

fn from_visual_index(items: &[SidebarItem], visual: usize) -> usize {
    let mut count = 0;
    for (i, item) in items.iter().enumerate() {
        if i == visual {
            return count;
        }
        if !matches!(item, SidebarItem::Divider) {
            count += 1;
        }
    }
    count
}

fn default_items() -> Vec<SidebarItem> {
    vec![
        SidebarItem::AllInboxes,
        SidebarItem::Divider,
        SidebarItem::Approvals { pending: 0 },
        SidebarItem::Drafts,
        SidebarItem::Briefing,
        SidebarItem::Search,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Inbox;
    use chrono::Utc;
    use uuid::Uuid;

    fn make_inbox(email: &str) -> Inbox {
        Inbox {
            id: Uuid::new_v4(),
            email: email.into(),
            display_name: None,
            inbox_type: "standard".into(),
            active: true,
            created_at: Utc::now(),
        }
    }

    fn populated_list() -> InboxList {
        let mut list = InboxList::new();
        let inboxes = vec![
            make_inbox("hello@pb.dev"),
            make_inbox("support@pb.dev"),
            make_inbox("alerts@pb.dev"),
        ];
        list.set_inboxes(&inboxes, 3);
        list
    }

    #[test]
    fn test_new_selects_first() {
        let list = InboxList::new();
        assert_eq!(list.selected(), 0);
    }

    #[test]
    fn test_set_inboxes_populates_items() {
        let list = populated_list();
        // AllInboxes + 3 inboxes
        assert_eq!(list.inbox_count(), 4);
    }

    #[test]
    fn test_select_next_skips_divider() {
        let mut list = populated_list();
        // Item 3 = alerts@pb.dev (logical), next skips Divider to Approvals
        list.select(3);
        list.select_next();
        let vis = list.state.selected().unwrap();
        // Visual: 0=AllInboxes, 1=hello, 2=support, 3=alerts, 4=Divider, 5=Approvals
        assert_eq!(vis, 5);
    }

    #[test]
    fn test_select_prev_from_start() {
        let mut list = InboxList::new();
        list.select_prev();
        assert_eq!(list.logical_selected(), 0);
    }

    #[test]
    fn test_select_last() {
        let mut list = populated_list();
        list.select_last();
        let count = selectable_count(&list.items);
        assert_eq!(list.logical_selected(), count - 1);
    }

    #[test]
    fn test_select_first() {
        let mut list = populated_list();
        list.select(3);
        list.select_first();
        assert_eq!(list.logical_selected(), 0);
    }

    #[test]
    fn test_selectable_count_excludes_divider() {
        let list = populated_list();
        let total = list.items.len();
        let dividers = list
            .items
            .iter()
            .filter(|i| matches!(i, SidebarItem::Divider))
            .count();
        assert_eq!(selectable_count(&list.items), total - dividers);
    }

    #[test]
    fn test_visual_logical_roundtrip() {
        let list = populated_list();
        let count = selectable_count(&list.items);
        for i in 0..count {
            let vis = to_visual_index(&list.items, i);
            let back = from_visual_index(&list.items, vis);
            assert_eq!(back, i, "roundtrip failed for logical index {i}");
        }
    }

    #[test]
    fn test_select_out_of_bounds() {
        let mut list = populated_list();
        list.select(selectable_count(&list.items) + 10);
        assert_eq!(list.logical_selected(), 0);
    }

    #[test]
    fn test_set_inboxes_preserves_selection() {
        let mut list = populated_list();
        list.select(2); // support@pb.dev
        let inboxes = vec![
            make_inbox("hello@pb.dev"),
            make_inbox("support@pb.dev"),
            make_inbox("alerts@pb.dev"),
            make_inbox("new@pb.dev"),
        ];
        list.set_inboxes(&inboxes, 0);
        assert_eq!(list.logical_selected(), 2);
    }

    #[test]
    fn test_default_items_structure() {
        let list = InboxList::new();
        // AllInboxes, Divider, Approvals, Drafts, Briefing, Search
        assert_eq!(list.items.len(), 6);
        assert_eq!(list.inbox_count(), 1); // only AllInboxes
    }
}
