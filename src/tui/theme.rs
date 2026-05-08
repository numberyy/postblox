use std::fmt;
use std::str::FromStr;

use ratatui::style::{Color, Modifier, Style};
use thiserror::Error;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ThemeName {
    #[default]
    Light,
    Dark,
    HighContrast,
}

impl ThemeName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
            Self::HighContrast => "high-contrast",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Light => Self::Dark,
            Self::Dark => Self::HighContrast,
            Self::HighContrast => Self::Light,
        }
    }

    pub fn theme(self) -> Theme {
        match self {
            Self::Light => Theme {
                text: Style::default().fg(Color::Black).bg(Color::White),
                muted: Style::default().fg(Color::DarkGray).bg(Color::White),
                pane: Style::default().fg(Color::Blue).bg(Color::White),
                active_pane: Style::default()
                    .fg(Color::Blue)
                    .bg(Color::White)
                    .add_modifier(Modifier::BOLD),
                selection: Style::default().fg(Color::White).bg(Color::Blue),
                status: Style::default().fg(Color::White).bg(Color::Blue),
                error: Style::default()
                    .fg(Color::White)
                    .bg(Color::Red)
                    .add_modifier(Modifier::BOLD),
                command: Style::default().fg(Color::White).bg(Color::Magenta),
                unread: Style::default()
                    .fg(Color::Blue)
                    .bg(Color::White)
                    .add_modifier(Modifier::BOLD),
                flagged: Style::default()
                    .fg(Color::Yellow)
                    .bg(Color::White)
                    .add_modifier(Modifier::BOLD),
            },
            Self::Dark => Theme {
                text: Style::default().fg(Color::White).bg(Color::Black),
                muted: Style::default().fg(Color::DarkGray).bg(Color::Black),
                pane: Style::default().fg(Color::Gray).bg(Color::Black),
                active_pane: Style::default()
                    .fg(Color::LightCyan)
                    .bg(Color::Black)
                    .add_modifier(Modifier::BOLD),
                selection: Style::default().fg(Color::Black).bg(Color::LightBlue),
                status: Style::default().fg(Color::White).bg(Color::Blue),
                error: Style::default().fg(Color::White).bg(Color::Red),
                command: Style::default().fg(Color::Black).bg(Color::LightCyan),
                unread: Style::default()
                    .fg(Color::LightGreen)
                    .bg(Color::Black)
                    .add_modifier(Modifier::BOLD),
                flagged: Style::default().fg(Color::LightYellow).bg(Color::Black),
            },
            Self::HighContrast => Theme {
                text: Style::default().fg(Color::White).bg(Color::Black),
                muted: Style::default().fg(Color::White).bg(Color::Black),
                pane: Style::default().fg(Color::White).bg(Color::Black),
                active_pane: Style::default()
                    .fg(Color::Black)
                    .bg(Color::White)
                    .add_modifier(Modifier::BOLD),
                selection: Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
                status: Style::default().fg(Color::Black).bg(Color::White),
                error: Style::default()
                    .fg(Color::White)
                    .bg(Color::Red)
                    .add_modifier(Modifier::BOLD),
                command: Style::default().fg(Color::Black).bg(Color::Cyan),
                unread: Style::default()
                    .fg(Color::Black)
                    .bg(Color::White)
                    .add_modifier(Modifier::BOLD),
                flagged: Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            },
        }
    }
}

impl fmt::Display for ThemeName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ThemeName {
    type Err = ThemeParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "light" => Ok(Self::Light),
            "dark" => Ok(Self::Dark),
            "high-contrast" | "hc" => Ok(Self::HighContrast),
            other => Err(ThemeParseError::Unknown(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ThemeParseError {
    #[error("unknown theme '{0}'")]
    Unknown(String),
}

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub text: Style,
    pub muted: Style,
    pub pane: Style,
    pub active_pane: Style,
    pub selection: Style,
    pub status: Style,
    pub error: Style,
    pub command: Style,
    pub unread: Style,
    pub flagged: Style,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_theme_name_parses_supported_names() {
        assert_eq!("light".parse::<ThemeName>().unwrap(), ThemeName::Light);
        assert_eq!("dark".parse::<ThemeName>().unwrap(), ThemeName::Dark);
        assert_eq!(
            "high-contrast".parse::<ThemeName>().unwrap(),
            ThemeName::HighContrast
        );
    }

    #[test]
    fn test_theme_name_parses_hc_alias() {
        assert_eq!("hc".parse::<ThemeName>().unwrap(), ThemeName::HighContrast);
    }

    #[test]
    fn test_theme_name_rejects_unknown_name() {
        let err = "solarized".parse::<ThemeName>().unwrap_err();

        assert_eq!(err.to_string(), "unknown theme 'solarized'");
    }

    #[test]
    fn test_theme_name_rejects_legacy_default_name() {
        // "default" is no longer a recognized theme name; "light" is the
        // default. This guards against tests or configs that still ship
        // the old name.
        assert!("default".parse::<ThemeName>().is_err());
    }

    #[test]
    fn test_theme_cycle_order_wraps() {
        assert_eq!(ThemeName::Light.next(), ThemeName::Dark);
        assert_eq!(ThemeName::Dark.next(), ThemeName::HighContrast);
        assert_eq!(ThemeName::HighContrast.next(), ThemeName::Light);
    }

    #[test]
    fn test_default_theme_name_is_light() {
        assert_eq!(ThemeName::default(), ThemeName::Light);
    }

    /// Every named palette must populate every Theme field with a
    /// non-empty Style; rendering must never fall through to a bare
    /// `Style::default()` because that's terminal-dependent and easy to
    /// leave illegible (e.g. white-on-white).
    #[test]
    fn test_every_theme_populates_every_field() {
        for name in [ThemeName::Light, ThemeName::Dark, ThemeName::HighContrast] {
            let theme = name.theme();
            let bare = Style::default();
            assert_ne!(theme.text, bare, "{name}: text");
            assert_ne!(theme.muted, bare, "{name}: muted");
            assert_ne!(theme.pane, bare, "{name}: pane");
            assert_ne!(theme.active_pane, bare, "{name}: active_pane");
            assert_ne!(theme.selection, bare, "{name}: selection");
            assert_ne!(theme.status, bare, "{name}: status");
            assert_ne!(theme.error, bare, "{name}: error");
            assert_ne!(theme.command, bare, "{name}: command");
            assert_ne!(theme.unread, bare, "{name}: unread");
            assert_ne!(theme.flagged, bare, "{name}: flagged");
        }
    }
}
