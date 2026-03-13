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
            lines: Vec::new(),
            scroll: 0,
        }
    }

    pub fn set_data(&mut self, briefing: &crate::client::Briefing) {
        let mut lines = vec![
            BriefingLine::Header(format!("Summary ({})", briefing.period)),
            BriefingLine::Stat("Total received".into(), briefing.total_received.to_string()),
            BriefingLine::Stat("Total sent".into(), briefing.total_sent.to_string()),
            BriefingLine::Blank,
        ];
        if !briefing.by_inbox.is_empty() {
            lines.push(BriefingLine::Header("By Inbox".into()));
            for inbox in &briefing.by_inbox {
                lines.push(BriefingLine::Stat(
                    inbox.inbox_email.clone(),
                    format!("{} in / {} out", inbox.received, inbox.sent),
                ));
            }
            lines.push(BriefingLine::Blank);
        }
        if !briefing.top_senders.is_empty() {
            lines.push(BriefingLine::Header("Top Senders".into()));
            for sender in &briefing.top_senders {
                lines.push(BriefingLine::Stat(
                    sender.address.clone(),
                    format!("{} messages", sender.count),
                ));
            }
            lines.push(BriefingLine::Blank);
        }
        if !briefing.top_subjects.is_empty() {
            lines.push(BriefingLine::Header("Top Subjects".into()));
            for subj in &briefing.top_subjects {
                lines.push(BriefingLine::Stat(
                    subj.subject.clone(),
                    format!("{} messages", subj.count),
                ));
            }
        }
        self.lines = lines;
        self.scroll = 0;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{Briefing, InboxStats, SenderCount, SubjectCount};
    use chrono::Utc;
    use uuid::Uuid;

    fn sample_briefing() -> Briefing {
        Briefing {
            period: "24h".into(),
            since: Utc::now(),
            total_received: 47,
            total_sent: 12,
            by_inbox: vec![InboxStats {
                inbox_id: Uuid::new_v4(),
                inbox_email: "hello@pb.dev".into(),
                received: 32,
                sent: 8,
            }],
            top_senders: vec![SenderCount {
                address: "alice@co.com".into(),
                count: 8,
            }],
            top_subjects: vec![SubjectCount {
                subject: "Meeting".into(),
                count: 3,
            }],
        }
    }

    #[test]
    fn test_new_starts_empty() {
        let b = BriefingPanel::new();
        assert!(b.lines.is_empty());
        assert_eq!(b.scroll, 0);
    }

    #[test]
    fn test_set_data_populates_lines() {
        let mut b = BriefingPanel::new();
        b.set_data(&sample_briefing());
        assert!(!b.lines.is_empty());
        assert_eq!(b.scroll, 0);
    }

    #[test]
    fn test_set_data_resets_scroll() {
        let mut b = BriefingPanel::new();
        b.scroll_down();
        b.scroll_down();
        b.set_data(&sample_briefing());
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
