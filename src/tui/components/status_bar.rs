use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::state::Mode;
use crate::theme::{Theme, ICON_CONNECTED, ICON_DISCONNECTED};

pub struct StatusBar {
    pub connected: bool,
    pub inbox_name: String,
    pub inbox_count: usize,
    pub vim_mode: bool,
}

impl StatusBar {
    pub fn new(vim_mode: bool) -> Self {
        Self {
            connected: false,
            inbox_name: String::new(),
            inbox_count: 0,
            vim_mode,
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: &Theme, mode: Mode) {
        let (conn_icon, conn_color) = if self.connected {
            (ICON_CONNECTED, theme.success)
        } else {
            (ICON_DISCONNECTED, theme.error)
        };

        let hints = mode_hints(mode, self.vim_mode);

        let info = if self.inbox_count > 0 {
            format!("{} ({})", self.inbox_name, self.inbox_count)
        } else {
            self.inbox_name.clone()
        };

        let line = Line::from(vec![
            Span::styled(format!(" {conn_icon}"), Style::default().fg(conn_color)),
            Span::styled(" │ ", Style::default().fg(theme.muted)),
            Span::styled(info, Style::default().fg(theme.fg)),
            Span::styled(" │ ", Style::default().fg(theme.muted)),
            Span::styled(hints, Style::default().fg(theme.muted)),
        ]);

        let p = Paragraph::new(line).style(Style::default().bg(theme.bg));
        frame.render_widget(p, area);
    }
}

fn mode_hints(mode: Mode, vim_mode: bool) -> &'static str {
    match mode {
        Mode::Compose => "Ctrl+Enter: send │ Tab: field │ Esc: cancel",
        Mode::Search => "Enter: select │ Esc: cancel",
        Mode::Help => "Press any key to close",
        Mode::Thread => "j/k: messages │ [/]: scroll │ q/Esc: back",
        Mode::Normal if vim_mode => "j/k ↑↓ Tab c r / ? q",
        Mode::Normal => "↑↓ Tab Enter Esc Ctrl+N/R/F ?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_defaults() {
        let sb = StatusBar::new(true);
        assert!(!sb.connected);
        assert!(sb.inbox_name.is_empty());
        assert!(sb.vim_mode);
    }

    #[test]
    fn test_mode_hints_normal_vim() {
        let hints = mode_hints(Mode::Normal, true);
        assert!(hints.contains("j/k"));
        assert!(hints.contains("q"));
    }

    #[test]
    fn test_mode_hints_normal_no_vim() {
        let hints = mode_hints(Mode::Normal, false);
        assert!(!hints.contains("j/k"));
        assert!(hints.contains("Tab"));
    }

    #[test]
    fn test_mode_hints_compose() {
        let hints = mode_hints(Mode::Compose, true);
        assert!(hints.contains("send"));
        assert!(hints.contains("Esc"));
    }

    #[test]
    fn test_mode_hints_search() {
        let hints = mode_hints(Mode::Search, false);
        assert!(hints.contains("Enter"));
        assert!(hints.contains("Esc"));
    }

    #[test]
    fn test_mode_hints_help() {
        let hints = mode_hints(Mode::Help, false);
        assert!(hints.contains("any key"));
    }
}
