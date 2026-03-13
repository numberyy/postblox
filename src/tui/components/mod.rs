pub mod approvals;
pub mod briefing;
pub mod compose;
pub mod inbox_list;
pub mod message_list;
pub mod preview;
pub mod search;
pub mod status_bar;

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max - 1).collect();
        format!("{cut}…")
    }
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
}
