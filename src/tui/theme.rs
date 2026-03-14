use ratatui::style::Color;

pub struct Theme {
    pub bg: Color,
    pub fg: Color,
    pub accent: Color,
    pub muted: Color,
    pub border: Color,
    pub border_focused: Color,
    pub error: Color,
    pub success: Color,
    pub warning: Color,
}

impl Theme {
    pub fn from_name(name: &str) -> Self {
        match name {
            "dracula" => Self::dracula(),
            "catppuccin" => Self::catppuccin(),
            "tokyo_night" | "tokyo-night" => Self::tokyo_night(),
            _ => Self::nord(),
        }
    }

    fn nord() -> Self {
        Self {
            bg: Color::Rgb(46, 52, 64),
            fg: Color::Rgb(216, 222, 233),
            accent: Color::Rgb(136, 192, 208),
            muted: Color::Rgb(76, 86, 106),
            border: Color::Rgb(59, 66, 82),
            border_focused: Color::Rgb(136, 192, 208),
            error: Color::Rgb(191, 97, 106),
            success: Color::Rgb(163, 190, 140),
            warning: Color::Rgb(235, 203, 139),
        }
    }

    fn dracula() -> Self {
        Self {
            bg: Color::Rgb(40, 42, 54),
            fg: Color::Rgb(248, 248, 242),
            accent: Color::Rgb(189, 147, 249),
            muted: Color::Rgb(98, 114, 164),
            border: Color::Rgb(68, 71, 90),
            border_focused: Color::Rgb(189, 147, 249),
            error: Color::Rgb(255, 85, 85),
            success: Color::Rgb(80, 250, 123),
            warning: Color::Rgb(241, 250, 140),
        }
    }

    fn catppuccin() -> Self {
        Self {
            bg: Color::Rgb(30, 30, 46),
            fg: Color::Rgb(205, 214, 244),
            accent: Color::Rgb(137, 180, 250),
            muted: Color::Rgb(88, 91, 112),
            border: Color::Rgb(49, 50, 68),
            border_focused: Color::Rgb(137, 180, 250),
            error: Color::Rgb(243, 139, 168),
            success: Color::Rgb(166, 227, 161),
            warning: Color::Rgb(249, 226, 175),
        }
    }

    fn tokyo_night() -> Self {
        Self {
            bg: Color::Rgb(26, 27, 38),
            fg: Color::Rgb(169, 177, 214),
            accent: Color::Rgb(122, 162, 247),
            muted: Color::Rgb(65, 72, 104),
            border: Color::Rgb(41, 46, 66),
            border_focused: Color::Rgb(122, 162, 247),
            error: Color::Rgb(247, 118, 142),
            success: Color::Rgb(158, 206, 106),
            warning: Color::Rgb(224, 175, 104),
        }
    }
}

// Nerd Font icons
pub const ICON_INBOX: &str = "\u{f01ee}"; // 󰇮
pub const ICON_MESSAGE: &str = "\u{f01a7}"; // 󰆧
pub const ICON_APPROVAL: &str = "\u{f0134}"; // 󰄴
pub const ICON_SEARCH: &str = "\u{f002}"; //
pub const ICON_BRIEFING: &str = "\u{f0f76}"; // 󰽶
pub const ICON_SLOP: &str = "\u{f071}"; //
pub const ICON_DRAFTS: &str = "\u{f01c1}"; // 󰇁
pub const ICON_CONNECTED: &str = "\u{f0219}"; // 󰈙
pub const ICON_DISCONNECTED: &str = "\u{f0378}"; // 󰍸

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_name_nord_default() {
        let theme = Theme::from_name("nord");
        assert_eq!(theme.bg, Color::Rgb(46, 52, 64));
        assert_eq!(theme.accent, Color::Rgb(136, 192, 208));
    }

    #[test]
    fn test_from_name_dracula() {
        let theme = Theme::from_name("dracula");
        assert_eq!(theme.bg, Color::Rgb(40, 42, 54));
        assert_eq!(theme.accent, Color::Rgb(189, 147, 249));
    }

    #[test]
    fn test_from_name_catppuccin() {
        let theme = Theme::from_name("catppuccin");
        assert_eq!(theme.bg, Color::Rgb(30, 30, 46));
        assert_eq!(theme.accent, Color::Rgb(137, 180, 250));
    }

    #[test]
    fn test_from_name_tokyo_night_underscore() {
        let theme = Theme::from_name("tokyo_night");
        assert_eq!(theme.bg, Color::Rgb(26, 27, 38));
        assert_eq!(theme.accent, Color::Rgb(122, 162, 247));
    }

    #[test]
    fn test_from_name_tokyo_night_hyphen() {
        let theme = Theme::from_name("tokyo-night");
        assert_eq!(theme.bg, Color::Rgb(26, 27, 38));
    }

    #[test]
    fn test_from_name_unknown_defaults_to_nord() {
        let theme = Theme::from_name("nonexistent");
        assert_eq!(theme.bg, Color::Rgb(46, 52, 64));
    }

    #[test]
    fn test_theme_has_all_colors() {
        let theme = Theme::from_name("nord");
        // Verify all fields are distinct (not all the same color)
        let colors = [
            theme.bg,
            theme.fg,
            theme.accent,
            theme.muted,
            theme.border,
            theme.error,
            theme.success,
            theme.warning,
        ];
        // At least 5 distinct colors
        let mut unique = colors.to_vec();
        unique.sort_by_key(|c| format!("{c:?}"));
        unique.dedup();
        assert!(unique.len() >= 5);
    }

    #[test]
    fn test_icon_constants_non_empty() {
        assert!(!ICON_INBOX.is_empty());
        assert!(!ICON_MESSAGE.is_empty());
        assert!(!ICON_APPROVAL.is_empty());
        assert!(!ICON_SEARCH.is_empty());
        assert!(!ICON_BRIEFING.is_empty());
        assert!(!ICON_SLOP.is_empty());
        assert!(!ICON_CONNECTED.is_empty());
        assert!(!ICON_DISCONNECTED.is_empty());
    }
}
