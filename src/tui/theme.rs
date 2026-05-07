use std::fmt;
use std::str::FromStr;

use ratatui::style::{Color, Modifier, Style};
use thiserror::Error;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ThemeName {
    #[default]
    Default,
    Dark,
    HighContrast,
}

impl ThemeName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Dark => "dark",
            Self::HighContrast => "high-contrast",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Default => Self::Dark,
            Self::Dark => Self::HighContrast,
            Self::HighContrast => Self::Default,
        }
    }

    pub fn theme(self) -> Theme {
        match self {
            Self::Default => Theme {
                text: Style::default().fg(Color::Reset),
                muted: Style::default().fg(Color::Gray),
                pane: Style::default(),
                active_pane: Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
                selection: Style::default().add_modifier(Modifier::REVERSED),
                status: Style::default().fg(Color::Black).bg(Color::Gray),
                error: Style::default().fg(Color::White).bg(Color::Red),
                command: Style::default().fg(Color::Black).bg(Color::Cyan),
                unread: Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
                flagged: Style::default().fg(Color::Yellow),
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
            "default" => Ok(Self::Default),
            "dark" => Ok(Self::Dark),
            "high-contrast" => Ok(Self::HighContrast),
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
        assert_eq!("default".parse::<ThemeName>().unwrap(), ThemeName::Default);
        assert_eq!("dark".parse::<ThemeName>().unwrap(), ThemeName::Dark);
        assert_eq!(
            "high-contrast".parse::<ThemeName>().unwrap(),
            ThemeName::HighContrast
        );
    }

    #[test]
    fn test_theme_name_rejects_unknown_name() {
        let err = "solarized".parse::<ThemeName>().unwrap_err();

        assert_eq!(err.to_string(), "unknown theme 'solarized'");
    }

    #[test]
    fn test_theme_cycle_order_wraps() {
        assert_eq!(ThemeName::Default.next(), ThemeName::Dark);
        assert_eq!(ThemeName::Dark.next(), ThemeName::HighContrast);
        assert_eq!(ThemeName::HighContrast.next(), ThemeName::Default);
    }
}
