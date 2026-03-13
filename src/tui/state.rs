use std::collections::HashMap;
use uuid::Uuid;

use crate::client::{Approval, Inbox, Message};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Sidebar,
    MessageList,
    Preview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Compose,
    Search,
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarSelection {
    AllInboxes,
    Inbox(Uuid),
    Approvals,
    Briefing,
    Search,
}

pub struct TuiState {
    pub inboxes: Vec<Inbox>,
    pub messages: HashMap<Option<Uuid>, Vec<Message>>,
    pub selected_inbox: usize,
    pub selected_message: usize,
    pub focus: Panel,
    pub mode: Mode,
    pub pending_approvals: usize,
    pub approvals: Vec<Approval>,
    pub search_results: Vec<Message>,
    pub search_query: String,
}

impl TuiState {
    pub fn new() -> Self {
        Self {
            inboxes: Vec::new(),
            messages: HashMap::new(),
            selected_inbox: 0,
            selected_message: 0,
            focus: Panel::Sidebar,
            mode: Mode::Normal,
            pending_approvals: 0,
            approvals: Vec::new(),
            search_results: Vec::new(),
            search_query: String::new(),
        }
    }

    pub fn sidebar_selection(&self) -> SidebarSelection {
        if self.selected_inbox == 0 {
            SidebarSelection::AllInboxes
        } else if let Some(inbox) = self.inboxes.get(self.selected_inbox - 1) {
            SidebarSelection::Inbox(inbox.id)
        } else {
            // Virtual views: indices after real inboxes
            let virtual_idx = self.selected_inbox - 1 - self.inboxes.len();
            match virtual_idx {
                0 => SidebarSelection::Approvals,
                1 => SidebarSelection::Briefing,
                _ => SidebarSelection::Search,
            }
        }
    }

    pub fn current_inbox_id(&self) -> Option<Uuid> {
        match self.sidebar_selection() {
            SidebarSelection::Inbox(id) => Some(id),
            _ => None,
        }
    }

    pub fn current_messages(&self) -> &[Message] {
        match self.sidebar_selection() {
            SidebarSelection::AllInboxes => self
                .messages
                .get(&None)
                .map(|v| v.as_slice())
                .unwrap_or(&[]),
            SidebarSelection::Inbox(id) => self
                .messages
                .get(&Some(id))
                .map(|v| v.as_slice())
                .unwrap_or(&[]),
            _ => &[],
        }
    }

    pub fn current_message(&self) -> Option<&Message> {
        self.current_messages().get(self.selected_message)
    }

    // sidebar_len: "All Inboxes" + per-inbox + virtual views
    fn sidebar_len(&self) -> usize {
        // All Inboxes + N inboxes + Approvals + Briefing + Search
        1 + self.inboxes.len() + 3
    }

    pub fn select_inbox(&mut self, idx: usize) {
        let max = self.sidebar_len();
        if idx < max {
            self.selected_inbox = idx;
            self.selected_message = 0;
        }
    }

    pub fn select_next_inbox(&mut self) {
        let max = self.sidebar_len();
        if max > 0 && self.selected_inbox < max - 1 {
            self.select_inbox(self.selected_inbox + 1);
        }
    }

    pub fn select_prev_inbox(&mut self) {
        if self.selected_inbox > 0 {
            self.select_inbox(self.selected_inbox - 1);
        }
    }

    pub fn select_message(&mut self, idx: usize) {
        let len = self.current_messages().len();
        if idx < len {
            self.selected_message = idx;
        }
    }

    pub fn select_next_message(&mut self) {
        let len = self.current_messages().len();
        if len > 0 && self.selected_message < len - 1 {
            self.select_message(self.selected_message + 1);
        }
    }

    pub fn select_prev_message(&mut self) {
        if self.selected_message > 0 {
            self.select_message(self.selected_message - 1);
        }
    }

    pub fn cycle_focus(&mut self) {
        self.focus = match self.focus {
            Panel::Sidebar => Panel::MessageList,
            Panel::MessageList => Panel::Preview,
            Panel::Preview => Panel::Sidebar,
        };
    }

    pub fn cycle_focus_back(&mut self) {
        self.focus = match self.focus {
            Panel::Sidebar => Panel::Preview,
            Panel::MessageList => Panel::Sidebar,
            Panel::Preview => Panel::MessageList,
        };
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

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

    fn make_message(inbox_id: Uuid) -> Message {
        Message {
            id: Uuid::new_v4(),
            inbox_id,
            thread_id: None,
            from_addr: "alice@example.com".into(),
            to_addrs: serde_json::json!(["bob@example.com"]),
            subject: Some("Test".into()),
            text_body: Some("Hello".into()),
            html_body: None,
            direction: "inbound".into(),
            created_at: Utc::now(),
            slop_score: None,
            category: None,
            triage_status: None,
        }
    }

    #[test]
    fn test_new_state_defaults() {
        let s = TuiState::new();
        assert_eq!(s.selected_inbox, 0);
        assert_eq!(s.selected_message, 0);
        assert_eq!(s.focus, Panel::Sidebar);
        assert_eq!(s.mode, Mode::Normal);
        assert_eq!(s.pending_approvals, 0);
    }

    #[test]
    fn test_sidebar_selection_all_inboxes() {
        let s = TuiState::new();
        assert_eq!(s.sidebar_selection(), SidebarSelection::AllInboxes);
        assert_eq!(s.current_inbox_id(), None);
    }

    #[test]
    fn test_sidebar_selection_specific_inbox() {
        let mut s = TuiState::new();
        let inbox = make_inbox("test@pb.dev");
        let id = inbox.id;
        s.inboxes.push(inbox);
        s.selected_inbox = 1;
        assert_eq!(s.sidebar_selection(), SidebarSelection::Inbox(id));
        assert_eq!(s.current_inbox_id(), Some(id));
    }

    #[test]
    fn test_sidebar_selection_virtual_views() {
        let mut s = TuiState::new();
        s.inboxes.push(make_inbox("a@b.com"));
        // Index 0 = AllInboxes, 1 = inbox, 2+ = virtual
        s.selected_inbox = 2;
        assert_eq!(s.sidebar_selection(), SidebarSelection::Approvals);
        assert_eq!(s.current_inbox_id(), None);
        s.selected_inbox = 3;
        assert_eq!(s.sidebar_selection(), SidebarSelection::Briefing);
        s.selected_inbox = 4;
        assert_eq!(s.sidebar_selection(), SidebarSelection::Search);
    }

    #[test]
    fn test_current_messages_empty_for_virtual_views() {
        let mut s = TuiState::new();
        s.inboxes.push(make_inbox("a@b.com"));
        let msg = make_message(s.inboxes[0].id);
        s.messages.insert(None, vec![msg]);
        // "All Inboxes" should return messages
        assert_eq!(s.current_messages().len(), 1);
        // Virtual view should return empty
        s.selected_inbox = 2; // Approvals
        assert!(s.current_messages().is_empty());
    }

    #[test]
    fn test_select_inbox_resets_message() {
        let mut s = TuiState::new();
        s.inboxes.push(make_inbox("a@b.com"));
        s.selected_message = 5;
        s.select_inbox(1);
        assert_eq!(s.selected_inbox, 1);
        assert_eq!(s.selected_message, 0);
    }

    #[test]
    fn test_select_inbox_out_of_bounds() {
        let mut s = TuiState::new();
        s.select_inbox(999);
        assert_eq!(s.selected_inbox, 0);
    }

    #[test]
    fn test_select_next_prev_inbox() {
        let mut s = TuiState::new();
        s.inboxes.push(make_inbox("a@b.com"));
        s.inboxes.push(make_inbox("c@d.com"));

        s.select_next_inbox();
        assert_eq!(s.selected_inbox, 1);
        s.select_next_inbox();
        assert_eq!(s.selected_inbox, 2);
        s.select_prev_inbox();
        assert_eq!(s.selected_inbox, 1);
        s.select_prev_inbox();
        assert_eq!(s.selected_inbox, 0);
        s.select_prev_inbox();
        assert_eq!(s.selected_inbox, 0);
    }

    #[test]
    fn test_current_messages_empty() {
        let s = TuiState::new();
        assert!(s.current_messages().is_empty());
    }

    #[test]
    fn test_current_messages_with_data() {
        let mut s = TuiState::new();
        let inbox = make_inbox("a@b.com");
        let msg = make_message(inbox.id);
        s.messages.insert(None, vec![msg]);
        assert_eq!(s.current_messages().len(), 1);
    }

    #[test]
    fn test_select_next_prev_message() {
        let mut s = TuiState::new();
        let id = Uuid::new_v4();
        s.messages.insert(
            None,
            vec![make_message(id), make_message(id), make_message(id)],
        );

        s.select_next_message();
        assert_eq!(s.selected_message, 1);
        s.select_next_message();
        assert_eq!(s.selected_message, 2);
        s.select_next_message();
        assert_eq!(s.selected_message, 2); // capped
        s.select_prev_message();
        assert_eq!(s.selected_message, 1);
    }

    #[test]
    fn test_cycle_focus_forward() {
        let mut s = TuiState::new();
        assert_eq!(s.focus, Panel::Sidebar);
        s.cycle_focus();
        assert_eq!(s.focus, Panel::MessageList);
        s.cycle_focus();
        assert_eq!(s.focus, Panel::Preview);
        s.cycle_focus();
        assert_eq!(s.focus, Panel::Sidebar);
    }

    #[test]
    fn test_cycle_focus_back() {
        let mut s = TuiState::new();
        s.cycle_focus_back();
        assert_eq!(s.focus, Panel::Preview);
        s.cycle_focus_back();
        assert_eq!(s.focus, Panel::MessageList);
        s.cycle_focus_back();
        assert_eq!(s.focus, Panel::Sidebar);
    }

    #[test]
    fn test_set_mode() {
        let mut s = TuiState::new();
        s.set_mode(Mode::Compose);
        assert_eq!(s.mode, Mode::Compose);
        s.set_mode(Mode::Search);
        assert_eq!(s.mode, Mode::Search);
        s.set_mode(Mode::Help);
        assert_eq!(s.mode, Mode::Help);
        s.set_mode(Mode::Normal);
        assert_eq!(s.mode, Mode::Normal);
    }

    #[test]
    fn test_current_message_some() {
        let mut s = TuiState::new();
        let msg = make_message(Uuid::new_v4());
        let msg_id = msg.id;
        s.messages.insert(None, vec![msg]);
        assert_eq!(s.current_message().unwrap().id, msg_id);
    }

    #[test]
    fn test_current_message_none_when_empty() {
        let s = TuiState::new();
        assert!(s.current_message().is_none());
    }
}
