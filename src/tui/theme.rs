//! TUI colour palettes and the [`ThemeName`] cycle.
//!
//! Three built-in themes — `light`, `dark`, `high-contrast` — each
//! resolved to a flat `Theme` of [`ratatui::style::Style`] slots
//! used by [`super::render`]. `ThemeName::next` drives the
//! `Ctrl-T` rotation and the `:theme` command. Parsing is sync and
//! infallible per CLAUDE.md's "async is for real I/O only" rule.

use std::fmt;
use std::str::FromStr;

use ratatui::style::{Color, Modifier, Style};
use thiserror::Error;

/// Named TUI colour palette.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ThemeName {
    /// Default dark palette — a cohesive curated RGB theme that reads
    /// well over the dark terminals most users run.
    #[default]
    Dark,
    /// Light palette tuned for light terminals.
    Light,
    /// High-contrast palette tuned for accessibility (honours the
    /// terminal's own 16-colour palette).
    HighContrast,
}

impl ThemeName {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
            Self::HighContrast => "high-contrast",
        }
    }

    pub(crate) fn next(self) -> Self {
        match self {
            Self::Dark => Self::Light,
            Self::Light => Self::HighContrast,
            Self::HighContrast => Self::Dark,
        }
    }

    pub(crate) fn theme(self) -> Theme {
        match self {
            // Curated RGB dark palette. Every slot paints an explicit fg AND
            // bg so panes share one consistent surface and nothing falls
            // through to a terminal-dependent default.
            Self::Dark => {
                let bg = Color::Rgb(0x1e, 0x1e, 0x2e); // base
                let fg = Color::Rgb(0xcd, 0xd6, 0xf4); // text
                let accent = Color::Rgb(0x89, 0xb4, 0xfa); // blue
                Theme {
                    text: Style::default().fg(fg).bg(bg),
                    muted: Style::default().fg(Color::Rgb(0x93, 0x99, 0xb2)).bg(bg),
                    pane: Style::default().fg(Color::Rgb(0x6c, 0x70, 0x86)).bg(bg),
                    active_pane: Style::default()
                        .fg(accent)
                        .bg(bg)
                        .add_modifier(Modifier::BOLD),
                    selection: Style::default()
                        .fg(fg)
                        .bg(Color::Rgb(0x45, 0x47, 0x5a))
                        .add_modifier(Modifier::BOLD),
                    status: Style::default().fg(fg).bg(Color::Rgb(0x31, 0x32, 0x44)),
                    error: Style::default()
                        .fg(bg)
                        .bg(Color::Rgb(0xf3, 0x8b, 0xa8))
                        .add_modifier(Modifier::BOLD),
                    command: Style::default().fg(Color::Rgb(0x11, 0x11, 0x1b)).bg(accent),
                    unread: Style::default()
                        .fg(Color::Rgb(0xa6, 0xe3, 0xa1))
                        .bg(bg)
                        .add_modifier(Modifier::BOLD),
                    flagged: Style::default()
                        .fg(Color::Rgb(0xf9, 0xe2, 0xaf))
                        .bg(bg)
                        .add_modifier(Modifier::BOLD),
                }
            }
            // Curated RGB light palette for light terminals.
            Self::Light => {
                let bg = Color::Rgb(0xef, 0xf1, 0xf5); // base
                let fg = Color::Rgb(0x4c, 0x4f, 0x69); // text
                let accent = Color::Rgb(0x1e, 0x66, 0xf5); // blue
                Theme {
                    text: Style::default().fg(fg).bg(bg),
                    muted: Style::default().fg(Color::Rgb(0x6c, 0x6f, 0x85)).bg(bg),
                    pane: Style::default().fg(Color::Rgb(0x8c, 0x8f, 0xa1)).bg(bg),
                    active_pane: Style::default()
                        .fg(accent)
                        .bg(bg)
                        .add_modifier(Modifier::BOLD),
                    selection: Style::default()
                        .fg(fg)
                        .bg(Color::Rgb(0xcc, 0xd0, 0xda))
                        .add_modifier(Modifier::BOLD),
                    status: Style::default().fg(fg).bg(Color::Rgb(0xcc, 0xd0, 0xda)),
                    error: Style::default()
                        .fg(bg)
                        .bg(Color::Rgb(0xd2, 0x0f, 0x39))
                        .add_modifier(Modifier::BOLD),
                    command: Style::default().fg(bg).bg(accent),
                    unread: Style::default()
                        .fg(Color::Rgb(0x40, 0xa0, 0x2b))
                        .bg(bg)
                        .add_modifier(Modifier::BOLD),
                    flagged: Style::default()
                        .fg(Color::Rgb(0xdf, 0x8e, 0x1d))
                        .bg(bg)
                        .add_modifier(Modifier::BOLD),
                }
            }
            // High-contrast accessibility theme: stays on the terminal's own
            // 16-colour palette (so it honours user overrides) but keeps a
            // visual hierarchy via modifiers rather than flattening every
            // slot to identical white-on-black.
            Self::HighContrast => Theme {
                text: Style::default().fg(Color::White).bg(Color::Black),
                muted: Style::default()
                    .fg(Color::White)
                    .bg(Color::Black)
                    .add_modifier(Modifier::DIM),
                pane: Style::default()
                    .fg(Color::White)
                    .bg(Color::Black)
                    .add_modifier(Modifier::DIM),
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
                    .fg(Color::White)
                    .bg(Color::Black)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
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

/// Errors produced when parsing a [`ThemeName`] from a string.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ThemeParseError {
    /// The supplied string did not match any known theme name.
    #[error("unknown theme '{0}'")]
    Unknown(String),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Theme {
    pub(crate) text: Style,
    pub(crate) muted: Style,
    pub(crate) pane: Style,
    pub(crate) active_pane: Style,
    pub(crate) selection: Style,
    pub(crate) status: Style,
    pub(crate) error: Style,
    pub(crate) command: Style,
    pub(crate) unread: Style,
    pub(crate) flagged: Style,
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
        assert_eq!(ThemeName::Dark.next(), ThemeName::Light);
        assert_eq!(ThemeName::Light.next(), ThemeName::HighContrast);
        assert_eq!(ThemeName::HighContrast.next(), ThemeName::Dark);
    }

    #[test]
    fn test_default_theme_name_is_dark() {
        assert_eq!(ThemeName::default(), ThemeName::Dark);
    }

    /// Legibility invariants: every slot must set a foreground AND a
    /// background that differ (rules out white-on-white / same-on-same),
    /// and the active pane must be distinguishable from an inactive pane
    /// by foreground colour — not by a modifier alone (the Light-theme
    /// regression where only BOLD differed).
    #[test]
    fn test_every_theme_has_legible_contrast() {
        for name in [ThemeName::Dark, ThemeName::Light, ThemeName::HighContrast] {
            let theme = name.theme();
            for (slot, style) in [
                ("text", theme.text),
                ("muted", theme.muted),
                ("pane", theme.pane),
                ("active_pane", theme.active_pane),
                ("selection", theme.selection),
                ("status", theme.status),
                ("error", theme.error),
                ("command", theme.command),
                ("unread", theme.unread),
                ("flagged", theme.flagged),
            ] {
                assert!(style.fg.is_some(), "{name}: {slot} has no fg");
                assert!(style.bg.is_some(), "{name}: {slot} has no bg");
                assert_ne!(style.fg, style.bg, "{name}: {slot} fg == bg");
            }
            assert_ne!(
                theme.active_pane.fg, theme.pane.fg,
                "{name}: active_pane must differ from pane by colour, not just a modifier"
            );
        }
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
