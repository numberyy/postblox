use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState};
use ratatui::Frame;

use crate::components::{themed_block, truncate};
use crate::theme::{Theme, ICON_APPROVAL, ICON_BRIEFING, ICON_INBOX, ICON_SEARCH};

pub struct InboxList {
    pub items: Vec<SidebarItem>,
    pub state: ListState,
}

pub enum SidebarItem {
    AllInboxes,
    Inbox {
        email: String,
        unread: usize,
        active: bool,
    },
    Divider,
    Approvals {
        pending: usize,
    },
    Briefing,
    Search,
}

impl InboxList {
    pub fn new() -> Self {
        Self {
            items: mock_items(),
            state: ListState::default().with_selected(Some(0)),
        }
    }

    #[allow(dead_code)] // Used when data layer wires in Round 3
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
        SidebarItem::Inbox {
            email,
            unread,
            active,
        } => {
            let marker = if *active { " ▶ " } else { "   " };
            let label = truncate(email, 14);
            let mut spans = vec![Span::styled(
                format!("{marker}{label}"),
                Style::default().fg(theme.fg),
            )];
            if *unread > 0 {
                spans.push(Span::styled(
                    format!(" ({unread})"),
                    Style::default().fg(theme.muted),
                ));
            }
            ListItem::new(Line::from(spans))
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

fn mock_items() -> Vec<SidebarItem> {
    vec![
        SidebarItem::AllInboxes,
        SidebarItem::Inbox {
            email: "hello@pb.dev".into(),
            unread: 12,
            active: true,
        },
        SidebarItem::Inbox {
            email: "support@pb.dev".into(),
            unread: 3,
            active: false,
        },
        SidebarItem::Inbox {
            email: "alerts@pb.dev".into(),
            unread: 0,
            active: false,
        },
        SidebarItem::Divider,
        SidebarItem::Approvals { pending: 3 },
        SidebarItem::Briefing,
        SidebarItem::Search,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_selects_first() {
        let list = InboxList::new();
        assert_eq!(list.selected(), 0);
    }

    #[test]
    fn test_select_next_skips_divider() {
        let mut list = InboxList::new();
        // Move to item 3 (alerts@pb.dev), next should skip divider to Approvals
        list.select(3);
        list.select_next();
        let vis = list.state.selected().unwrap();
        // Visual index 5 = Approvals (index 4 is Divider)
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
        let mut list = InboxList::new();
        list.select_last();
        let count = selectable_count(&list.items);
        assert_eq!(list.logical_selected(), count - 1);
    }

    #[test]
    fn test_select_first() {
        let mut list = InboxList::new();
        list.select(3);
        list.select_first();
        assert_eq!(list.logical_selected(), 0);
    }

    #[test]
    fn test_selectable_count_excludes_divider() {
        let items = mock_items();
        let total = items.len();
        let dividers = items
            .iter()
            .filter(|i| matches!(i, SidebarItem::Divider))
            .count();
        assert_eq!(selectable_count(&items), total - dividers);
    }

    #[test]
    fn test_visual_logical_roundtrip() {
        let items = mock_items();
        let count = selectable_count(&items);
        for i in 0..count {
            let vis = to_visual_index(&items, i);
            let back = from_visual_index(&items, vis);
            assert_eq!(back, i, "roundtrip failed for logical index {i}");
        }
    }

    #[test]
    fn test_select_out_of_bounds() {
        let mut list = InboxList::new();
        let max = selectable_count(&list.items);
        list.select(max + 10);
        // Should not change from default
        assert_eq!(list.logical_selected(), 0);
    }
}
