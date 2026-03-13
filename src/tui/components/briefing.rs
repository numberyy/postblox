use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use crate::components::themed_block;
use crate::theme::{Theme, ICON_BRIEFING};

pub struct BriefingPanel {
    pub lines: Vec<BriefingLine>,
    pub scroll: u16,
}

pub enum BriefingLine {
    Header(String),
    Stat(String, String),
    Blank,
}

impl BriefingPanel {
    pub fn new() -> Self {
        Self {
            lines: mock_briefing(),
            scroll: 0,
        }
    }

    pub fn scroll_down(&mut self) {
        self.scroll = self.scroll.saturating_add(1);
    }

    pub fn scroll_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: &Theme, focused: bool) {
        let block = themed_block(format!(" {ICON_BRIEFING} Daily Briefing "), theme, focused);

        let content: Vec<Line> = self
            .lines
            .iter()
            .map(|line| match line {
                BriefingLine::Header(text) => Line::from(Span::styled(
                    format!("  {text}"),
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                )),
                BriefingLine::Stat(label, value) => Line::from(vec![
                    Span::styled(format!("    {label}: "), Style::default().fg(theme.muted)),
                    Span::styled(value, Style::default().fg(theme.fg)),
                ]),
                BriefingLine::Blank => Line::from(""),
            })
            .collect();

        let p = Paragraph::new(content)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((self.scroll, 0));

        frame.render_widget(p, area);
    }
}

fn mock_briefing() -> Vec<BriefingLine> {
    vec![
        BriefingLine::Header("Summary (last 24h)".into()),
        BriefingLine::Stat("Total received".into(), "47".into()),
        BriefingLine::Stat("Total sent".into(), "12".into()),
        BriefingLine::Blank,
        BriefingLine::Header("By Inbox".into()),
        BriefingLine::Stat("hello@pb.dev".into(), "32 in / 8 out".into()),
        BriefingLine::Stat("support@pb.dev".into(), "15 in / 4 out".into()),
        BriefingLine::Blank,
        BriefingLine::Header("Top Senders".into()),
        BriefingLine::Stat("alice@company.com".into(), "8 messages".into()),
        BriefingLine::Stat("bob@example.com".into(), "5 messages".into()),
        BriefingLine::Stat("newsletter@io".into(), "4 messages".into()),
        BriefingLine::Blank,
        BriefingLine::Header("Pending Approvals".into()),
        BriefingLine::Stat("Count".into(), "3".into()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_has_content() {
        let b = BriefingPanel::new();
        assert!(!b.lines.is_empty());
        assert_eq!(b.scroll, 0);
    }

    #[test]
    fn test_scroll() {
        let mut b = BriefingPanel::new();
        b.scroll_down();
        assert_eq!(b.scroll, 1);
        b.scroll_up();
        assert_eq!(b.scroll, 0);
        b.scroll_up();
        assert_eq!(b.scroll, 0);
    }
}
