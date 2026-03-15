use std::borrow::Cow;

use ratatui::style::Style;
use ratatui::widgets::{Block, BorderType, Borders};

use crate::theme::Theme;

pub mod approvals;
pub mod briefing;
pub mod compose;
pub mod drafts;
pub mod inbox_list;
pub mod message_list;
pub mod preview;
pub mod search;
pub mod status_bar;
pub mod thread_panel;

pub fn truncate<'a>(s: &'a str, max: usize) -> Cow<'a, str> {
    if max == 0 {
        return Cow::Borrowed("");
    }
    if s.chars().count() <= max {
        Cow::Borrowed(s)
    } else {
        let cut: String = s.chars().take(max - 1).collect();
        Cow::Owned(format!("{cut}…"))
    }
}

pub fn themed_block(title: String, theme: &Theme, focused: bool) -> Block<'_> {
    let border_color = if focused {
        theme.border_focused
    } else {
        theme.border
    };
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_short_string() {
        assert_eq!(truncate("abc", 10), "abc");
    }

    #[test]
    fn test_truncate_long_string() {
        assert_eq!(truncate("hello@pb.dev", 8), "hello@p…");
    }

    #[test]
    fn test_truncate_multibyte_no_panic() {
        assert_eq!(truncate("café@example.com", 6), "café@…");
    }

    #[test]
    fn test_truncate_exact_length() {
        assert_eq!(truncate("abcde", 5), "abcde");
    }

    #[test]
    fn test_truncate_empty() {
        assert_eq!(truncate("", 5), "");
    }

    #[test]
    fn test_truncate_unicode_emoji() {
        assert_eq!(truncate("🎉🎊🎈🎁🎂🎃", 4), "🎉🎊🎈…");
    }

    #[test]
    fn test_truncate_max_zero() {
        assert_eq!(truncate("hello", 0), "");
    }

    #[test]
    fn test_truncate_max_one() {
        assert_eq!(truncate("hello", 1), "…");
    }
}
