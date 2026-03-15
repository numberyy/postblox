use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::KeybindingOverrides;
use crate::state::{Mode, Panel};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    MoveUp,
    MoveDown,
    MoveTop,
    MoveBottom,
    PanelLeft,
    PanelRight,
    CyclePanel,
    CyclePanelBack,
    Select,
    Back,
    Compose,
    Reply,
    StartSearch,
    Send,
    ShowHelp,
    ShowBriefing,
    ShowAllInboxes,
    SlopToggle,
    ApproveSelected,
    RejectSelected,
    Refresh,
    QuickJump(u8),
    OpenEditor,
    DownloadAttachment,
    OpenAttachment,
    NextAttachment,
    PrevAttachment,
    AddAttachment,
    RemoveAttachment,
}

#[cfg(test)]
pub fn resolve(key: KeyEvent, mode: Mode, focus: Panel, vim_mode: bool) -> Option<Action> {
    resolve_with_overrides(key, mode, focus, vim_mode, None)
}

pub fn resolve_with_overrides(
    key: KeyEvent,
    mode: Mode,
    focus: Panel,
    vim_mode: bool,
    overrides: Option<&KeybindingOverrides>,
) -> Option<Action> {
    // Ctrl combos — always active, all modes
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('c') => Some(Action::Quit),
            KeyCode::Char('n') if mode == Mode::Normal => Some(Action::Compose),
            KeyCode::Char('r') if mode == Mode::Normal => Some(Action::Reply),
            KeyCode::Char('f') if mode == Mode::Normal => Some(Action::StartSearch),
            KeyCode::Char('e') if mode == Mode::Compose => Some(Action::OpenEditor),
            KeyCode::Char('a') if mode == Mode::Compose => Some(Action::AddAttachment),
            KeyCode::Char('d') if mode == Mode::Compose => Some(Action::RemoveAttachment),
            KeyCode::Enter if mode == Mode::Compose => Some(Action::Send),
            _ => None,
        };
    }

    // Compose mode — only Esc works
    if mode == Mode::Compose {
        return match key.code {
            KeyCode::Esc => Some(Action::Back),
            _ => None,
        };
    }

    // Search mode — Esc, Enter, and arrow navigation
    if mode == Mode::Search {
        return match key.code {
            KeyCode::Esc => Some(Action::Back),
            KeyCode::Enter => Some(Action::Select),
            KeyCode::Up => Some(Action::MoveUp),
            KeyCode::Down => Some(Action::MoveDown),
            _ => None,
        };
    }

    // Help mode — any key dismisses
    if mode == Mode::Help {
        return Some(Action::Back);
    }

    // Normal mode — universal keys
    let universal = match key.code {
        KeyCode::Up => Some(Action::MoveUp),
        KeyCode::Down => Some(Action::MoveDown),
        KeyCode::Left => Some(Action::PanelLeft),
        KeyCode::Right => Some(Action::PanelRight),
        KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => Some(Action::CyclePanelBack),
        KeyCode::Tab => Some(Action::CyclePanel),
        KeyCode::Enter => Some(Action::Select),
        KeyCode::Esc => Some(Action::Back),
        KeyCode::Char('?') => Some(Action::ShowHelp),
        KeyCode::Char('[') if focus == Panel::Preview => Some(Action::PrevAttachment),
        KeyCode::Char(']') if focus == Panel::Preview => Some(Action::NextAttachment),
        _ => None,
    };

    if universal.is_some() {
        return universal;
    }

    // Check user overrides before vim defaults
    if let KeyCode::Char(c) = key.code {
        if let Some(action) = resolve_override(c, focus, overrides) {
            return Some(action);
        }
    }

    // Vim layer
    if !vim_mode {
        return None;
    }

    match key.code {
        KeyCode::Char('j') => Some(Action::MoveDown),
        KeyCode::Char('k') => Some(Action::MoveUp),
        KeyCode::Char('g') => Some(Action::MoveTop),
        KeyCode::Char('G') => Some(Action::MoveBottom),
        KeyCode::Char('h') => Some(Action::PanelLeft),
        KeyCode::Char('l') => Some(Action::PanelRight),
        KeyCode::Char('c') => Some(Action::Compose),
        KeyCode::Char('r') => Some(Action::Reply),
        KeyCode::Char('/') => Some(Action::StartSearch),
        KeyCode::Char('s') => Some(Action::SlopToggle),
        KeyCode::Char('b') => Some(Action::ShowBriefing),
        KeyCode::Char('a') => Some(Action::ShowAllInboxes),
        KeyCode::Char('q') => Some(Action::Quit),
        KeyCode::Char('R') => Some(Action::Refresh),
        KeyCode::Char('y') if focus == Panel::MessageList => Some(Action::ApproveSelected),
        KeyCode::Char('n') if focus == Panel::MessageList => Some(Action::RejectSelected),
        KeyCode::Char('d') if focus == Panel::Preview => Some(Action::DownloadAttachment),
        KeyCode::Char('o') if focus == Panel::Preview => Some(Action::OpenAttachment),
        KeyCode::Char(c @ '1'..='9') => Some(Action::QuickJump(c as u8 - b'0')),
        _ => None,
    }
}

fn resolve_override(
    c: char,
    focus: Panel,
    overrides: Option<&KeybindingOverrides>,
) -> Option<Action> {
    let kb = overrides?;
    for (action_name, &bound_char) in &kb.0 {
        if bound_char != c {
            continue;
        }
        let action = match action_name.as_str() {
            "quit" => Some(Action::Quit),
            "compose" => Some(Action::Compose),
            "reply" => Some(Action::Reply),
            "search" => Some(Action::StartSearch),
            "refresh" => Some(Action::Refresh),
            "approve" if focus == Panel::MessageList => Some(Action::ApproveSelected),
            "reject" if focus == Panel::MessageList => Some(Action::RejectSelected),
            "slop_toggle" => Some(Action::SlopToggle),
            "help" => Some(Action::ShowHelp),
            "briefing" => Some(Action::ShowBriefing),
            _ => None,
        };
        if action.is_some() {
            return action;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn shift(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::SHIFT,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    // Universal keys
    #[test]
    fn test_arrow_keys_universal() {
        assert_eq!(
            resolve(key(KeyCode::Up), Mode::Normal, Panel::Sidebar, false),
            Some(Action::MoveUp)
        );
        assert_eq!(
            resolve(key(KeyCode::Down), Mode::Normal, Panel::Sidebar, false),
            Some(Action::MoveDown)
        );
    }

    #[test]
    fn test_tab_cycles_panel() {
        assert_eq!(
            resolve(key(KeyCode::Tab), Mode::Normal, Panel::Sidebar, false),
            Some(Action::CyclePanel)
        );
        assert_eq!(
            resolve(shift(KeyCode::Tab), Mode::Normal, Panel::Sidebar, false),
            Some(Action::CyclePanelBack)
        );
    }

    #[test]
    fn test_enter_select_esc_back() {
        assert_eq!(
            resolve(key(KeyCode::Enter), Mode::Normal, Panel::Sidebar, false),
            Some(Action::Select)
        );
        assert_eq!(
            resolve(key(KeyCode::Esc), Mode::Normal, Panel::Sidebar, false),
            Some(Action::Back)
        );
    }

    #[test]
    fn test_question_mark_help() {
        assert_eq!(
            resolve(key(KeyCode::Char('?')), Mode::Normal, Panel::Sidebar, false),
            Some(Action::ShowHelp)
        );
    }

    // Ctrl combos
    #[test]
    fn test_ctrl_n_compose() {
        assert_eq!(
            resolve(
                ctrl(KeyCode::Char('n')),
                Mode::Normal,
                Panel::Sidebar,
                false
            ),
            Some(Action::Compose)
        );
    }

    #[test]
    fn test_ctrl_n_ignored_in_compose_mode() {
        assert_eq!(
            resolve(
                ctrl(KeyCode::Char('n')),
                Mode::Compose,
                Panel::Sidebar,
                false
            ),
            None
        );
    }

    #[test]
    fn test_ctrl_enter_sends_in_compose() {
        assert_eq!(
            resolve(ctrl(KeyCode::Enter), Mode::Compose, Panel::Preview, false),
            Some(Action::Send)
        );
    }

    #[test]
    fn test_ctrl_c_always_quits() {
        assert_eq!(
            resolve(
                ctrl(KeyCode::Char('c')),
                Mode::Normal,
                Panel::Sidebar,
                false
            ),
            Some(Action::Quit)
        );
        assert_eq!(
            resolve(
                ctrl(KeyCode::Char('c')),
                Mode::Compose,
                Panel::Preview,
                false
            ),
            Some(Action::Quit)
        );
        assert_eq!(
            resolve(
                ctrl(KeyCode::Char('c')),
                Mode::Search,
                Panel::Sidebar,
                false
            ),
            Some(Action::Quit)
        );
    }

    // Compose mode
    #[test]
    fn test_compose_mode_esc_back() {
        assert_eq!(
            resolve(key(KeyCode::Esc), Mode::Compose, Panel::Preview, false),
            Some(Action::Back)
        );
    }

    #[test]
    fn test_compose_mode_ignores_other_keys() {
        assert_eq!(
            resolve(key(KeyCode::Char('j')), Mode::Compose, Panel::Preview, true),
            None
        );
    }

    // Search mode
    #[test]
    fn test_search_mode_esc_and_enter() {
        assert_eq!(
            resolve(key(KeyCode::Esc), Mode::Search, Panel::Sidebar, false),
            Some(Action::Back)
        );
        assert_eq!(
            resolve(key(KeyCode::Enter), Mode::Search, Panel::Sidebar, false),
            Some(Action::Select)
        );
    }

    #[test]
    fn test_search_mode_arrow_navigation() {
        assert_eq!(
            resolve(key(KeyCode::Up), Mode::Search, Panel::Sidebar, false),
            Some(Action::MoveUp)
        );
        assert_eq!(
            resolve(key(KeyCode::Down), Mode::Search, Panel::Sidebar, false),
            Some(Action::MoveDown)
        );
    }

    #[test]
    fn test_search_mode_chars_not_mapped() {
        assert_eq!(
            resolve(key(KeyCode::Char('j')), Mode::Search, Panel::Sidebar, true),
            None
        );
    }

    // Help mode
    #[test]
    fn test_help_mode_any_key_dismisses() {
        assert_eq!(
            resolve(key(KeyCode::Char('x')), Mode::Help, Panel::Sidebar, false),
            Some(Action::Back)
        );
        assert_eq!(
            resolve(key(KeyCode::Enter), Mode::Help, Panel::Sidebar, false),
            Some(Action::Back)
        );
    }

    // Vim mode OFF
    #[test]
    fn test_vim_keys_ignored_when_off() {
        assert_eq!(
            resolve(key(KeyCode::Char('j')), Mode::Normal, Panel::Sidebar, false),
            None
        );
        assert_eq!(
            resolve(key(KeyCode::Char('q')), Mode::Normal, Panel::Sidebar, false),
            None
        );
    }

    // Vim mode ON
    #[test]
    fn test_vim_jk_navigation() {
        assert_eq!(
            resolve(key(KeyCode::Char('j')), Mode::Normal, Panel::Sidebar, true),
            Some(Action::MoveDown)
        );
        assert_eq!(
            resolve(key(KeyCode::Char('k')), Mode::Normal, Panel::Sidebar, true),
            Some(Action::MoveUp)
        );
    }

    #[test]
    fn test_vim_hl_panel_switch() {
        assert_eq!(
            resolve(
                key(KeyCode::Char('h')),
                Mode::Normal,
                Panel::MessageList,
                true
            ),
            Some(Action::PanelLeft)
        );
        assert_eq!(
            resolve(key(KeyCode::Char('l')), Mode::Normal, Panel::Sidebar, true),
            Some(Action::PanelRight)
        );
    }

    #[test]
    fn test_vim_g_top_bottom() {
        assert_eq!(
            resolve(key(KeyCode::Char('g')), Mode::Normal, Panel::Sidebar, true),
            Some(Action::MoveTop)
        );
        assert_eq!(
            resolve(key(KeyCode::Char('G')), Mode::Normal, Panel::Sidebar, true),
            Some(Action::MoveBottom)
        );
    }

    #[test]
    fn test_vim_compose_reply_search() {
        assert_eq!(
            resolve(key(KeyCode::Char('c')), Mode::Normal, Panel::Sidebar, true),
            Some(Action::Compose)
        );
        assert_eq!(
            resolve(key(KeyCode::Char('r')), Mode::Normal, Panel::Sidebar, true),
            Some(Action::Reply)
        );
        assert_eq!(
            resolve(key(KeyCode::Char('/')), Mode::Normal, Panel::Sidebar, true),
            Some(Action::StartSearch)
        );
    }

    #[test]
    fn test_vim_quick_jump() {
        assert_eq!(
            resolve(key(KeyCode::Char('1')), Mode::Normal, Panel::Sidebar, true),
            Some(Action::QuickJump(1))
        );
        assert_eq!(
            resolve(key(KeyCode::Char('9')), Mode::Normal, Panel::Sidebar, true),
            Some(Action::QuickJump(9))
        );
    }

    #[test]
    fn test_vim_approve_reject_only_in_message_list() {
        assert_eq!(
            resolve(
                key(KeyCode::Char('y')),
                Mode::Normal,
                Panel::MessageList,
                true
            ),
            Some(Action::ApproveSelected)
        );
        assert_eq!(
            resolve(
                key(KeyCode::Char('n')),
                Mode::Normal,
                Panel::MessageList,
                true
            ),
            Some(Action::RejectSelected)
        );
        // Not in sidebar
        assert_eq!(
            resolve(key(KeyCode::Char('y')), Mode::Normal, Panel::Sidebar, true),
            None
        );
    }

    #[test]
    fn test_vim_quit() {
        assert_eq!(
            resolve(key(KeyCode::Char('q')), Mode::Normal, Panel::Sidebar, true),
            Some(Action::Quit)
        );
    }

    #[test]
    fn test_vim_slop_briefing_all() {
        assert_eq!(
            resolve(key(KeyCode::Char('s')), Mode::Normal, Panel::Sidebar, true),
            Some(Action::SlopToggle)
        );
        assert_eq!(
            resolve(key(KeyCode::Char('b')), Mode::Normal, Panel::Sidebar, true),
            Some(Action::ShowBriefing)
        );
        assert_eq!(
            resolve(key(KeyCode::Char('a')), Mode::Normal, Panel::Sidebar, true),
            Some(Action::ShowAllInboxes)
        );
    }

    // ctrl+r and ctrl+f
    #[test]
    fn test_ctrl_r_reply_ctrl_f_search() {
        assert_eq!(
            resolve(
                ctrl(KeyCode::Char('r')),
                Mode::Normal,
                Panel::Sidebar,
                false
            ),
            Some(Action::Reply)
        );
        assert_eq!(
            resolve(
                ctrl(KeyCode::Char('f')),
                Mode::Normal,
                Panel::Sidebar,
                false
            ),
            Some(Action::StartSearch)
        );
    }

    #[test]
    fn test_ctrl_e_opens_editor_in_compose() {
        assert_eq!(
            resolve(
                ctrl(KeyCode::Char('e')),
                Mode::Compose,
                Panel::Preview,
                false
            ),
            Some(Action::OpenEditor)
        );
    }

    #[test]
    fn test_ctrl_e_not_in_normal() {
        assert_eq!(
            resolve(
                ctrl(KeyCode::Char('e')),
                Mode::Normal,
                Panel::Sidebar,
                false
            ),
            None
        );
    }

    #[test]
    fn test_bracket_attachment_nav_in_preview() {
        assert_eq!(
            resolve(key(KeyCode::Char('[')), Mode::Normal, Panel::Preview, false),
            Some(Action::PrevAttachment)
        );
        assert_eq!(
            resolve(key(KeyCode::Char(']')), Mode::Normal, Panel::Preview, false),
            Some(Action::NextAttachment)
        );
    }

    #[test]
    fn test_bracket_not_in_other_panels() {
        assert_eq!(
            resolve(key(KeyCode::Char('[')), Mode::Normal, Panel::Sidebar, false),
            None
        );
        assert_eq!(
            resolve(
                key(KeyCode::Char(']')),
                Mode::Normal,
                Panel::MessageList,
                false
            ),
            None
        );
    }

    #[test]
    fn test_vim_d_download_in_preview() {
        assert_eq!(
            resolve(key(KeyCode::Char('d')), Mode::Normal, Panel::Preview, true),
            Some(Action::DownloadAttachment)
        );
    }

    #[test]
    fn test_vim_o_open_in_preview() {
        assert_eq!(
            resolve(key(KeyCode::Char('o')), Mode::Normal, Panel::Preview, true),
            Some(Action::OpenAttachment)
        );
    }

    #[test]
    fn test_vim_d_not_in_sidebar() {
        assert_eq!(
            resolve(key(KeyCode::Char('d')), Mode::Normal, Panel::Sidebar, true),
            None
        );
    }

    #[test]
    fn test_vim_o_not_in_message_list() {
        assert_eq!(
            resolve(
                key(KeyCode::Char('o')),
                Mode::Normal,
                Panel::MessageList,
                true
            ),
            None
        );
    }

    #[test]
    fn test_d_o_not_active_without_vim() {
        assert_eq!(
            resolve(key(KeyCode::Char('d')), Mode::Normal, Panel::Preview, false),
            None
        );
        assert_eq!(
            resolve(key(KeyCode::Char('o')), Mode::Normal, Panel::Preview, false),
            None
        );
    }

    #[test]
    fn test_ctrl_a_add_attachment_in_compose() {
        assert_eq!(
            resolve(
                ctrl(KeyCode::Char('a')),
                Mode::Compose,
                Panel::Preview,
                false
            ),
            Some(Action::AddAttachment)
        );
    }

    #[test]
    fn test_ctrl_a_not_in_normal() {
        assert_eq!(
            resolve(
                ctrl(KeyCode::Char('a')),
                Mode::Normal,
                Panel::Sidebar,
                false
            ),
            None
        );
    }

    // Override tests
    #[test]
    fn test_override_replaces_vim_key() {
        use crate::config::KeybindingOverrides;
        use std::collections::HashMap;

        let mut map = HashMap::new();
        map.insert("quit".to_string(), 'x');
        let kb = KeybindingOverrides(map);

        assert_eq!(
            resolve_with_overrides(
                key(KeyCode::Char('x')),
                Mode::Normal,
                Panel::Sidebar,
                true,
                Some(&kb)
            ),
            Some(Action::Quit)
        );
    }

    #[test]
    fn test_override_without_vim_mode() {
        use crate::config::KeybindingOverrides;
        use std::collections::HashMap;

        let mut map = HashMap::new();
        map.insert("compose".to_string(), 'n');
        let kb = KeybindingOverrides(map);

        // Without vim mode, 'n' would normally do nothing
        assert_eq!(
            resolve_with_overrides(
                key(KeyCode::Char('n')),
                Mode::Normal,
                Panel::Sidebar,
                false,
                Some(&kb)
            ),
            Some(Action::Compose)
        );
    }

    #[test]
    fn test_override_none_falls_through_to_vim() {
        use crate::config::KeybindingOverrides;

        let kb = KeybindingOverrides::default();

        // No overrides, vim mode 'j' still works
        assert_eq!(
            resolve_with_overrides(
                key(KeyCode::Char('j')),
                Mode::Normal,
                Panel::Sidebar,
                true,
                Some(&kb)
            ),
            Some(Action::MoveDown)
        );
    }

    #[test]
    fn test_override_approve_only_in_message_list() {
        use crate::config::KeybindingOverrides;
        use std::collections::HashMap;

        let mut map = HashMap::new();
        map.insert("approve".to_string(), 'A');
        let kb = KeybindingOverrides(map);

        assert_eq!(
            resolve_with_overrides(
                key(KeyCode::Char('A')),
                Mode::Normal,
                Panel::MessageList,
                false,
                Some(&kb)
            ),
            Some(Action::ApproveSelected)
        );
        assert_eq!(
            resolve_with_overrides(
                key(KeyCode::Char('A')),
                Mode::Normal,
                Panel::Sidebar,
                false,
                Some(&kb)
            ),
            None
        );
    }

    #[test]
    fn test_override_universal_keys_take_priority() {
        use crate::config::KeybindingOverrides;
        use std::collections::HashMap;

        // Even with override, Esc should still be Back
        let mut map = HashMap::new();
        map.insert("quit".to_string(), 'q');
        let kb = KeybindingOverrides(map);

        assert_eq!(
            resolve_with_overrides(
                key(KeyCode::Esc),
                Mode::Normal,
                Panel::Sidebar,
                false,
                Some(&kb)
            ),
            Some(Action::Back)
        );
    }
}
