use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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
    QuickJump(u8),
}

pub fn resolve(key: KeyEvent, mode: Mode, focus: Panel, vim_mode: bool) -> Option<Action> {
    // Ctrl combos — always active, all modes
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('c') => Some(Action::Quit),
            KeyCode::Char('n') if mode == Mode::Normal => Some(Action::Compose),
            KeyCode::Char('r') if mode == Mode::Normal => Some(Action::Reply),
            KeyCode::Char('f') if mode == Mode::Normal => Some(Action::StartSearch),
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

    // Search mode — Esc and Enter
    if mode == Mode::Search {
        return match key.code {
            KeyCode::Esc => Some(Action::Back),
            KeyCode::Enter => Some(Action::Select),
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
        _ => None,
    };

    if universal.is_some() {
        return universal;
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
        KeyCode::Char('y') if focus == Panel::MessageList => Some(Action::ApproveSelected),
        KeyCode::Char('n') if focus == Panel::MessageList => Some(Action::RejectSelected),
        KeyCode::Char(c @ '1'..='9') => Some(Action::QuickJump(c as u8 - b'0')),
        _ => None,
    }
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
}
