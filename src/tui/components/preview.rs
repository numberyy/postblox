use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;
use uuid::Uuid;

use crate::theme::{Theme, ICON_ATTACHMENT};

pub fn html_to_plaintext(html: &str) -> String {
    match html2text::from_read(html.as_bytes(), 80) {
        Ok(text) => text,
        Err(e) => {
            tracing::debug!("html-to-text conversion failed: {e}");
            String::new()
        }
    }
}

pub struct AttachmentInfo {
    pub id: Uuid,
    pub filename: String,
    pub content_type: String,
    pub size_bytes: i64,
}

pub struct Preview {
    pub from: String,
    pub subject: String,
    pub date: String,
    pub body: String,
    pub scroll: u16,
    pub attachments: Vec<AttachmentInfo>,
    pub attachment_state: ListState,
}

impl Preview {
    pub fn new() -> Self {
        Self {
            from: String::new(),
            subject: String::new(),
            date: String::new(),
            body: String::new(),
            scroll: 0,
            attachments: Vec::new(),
            attachment_state: ListState::default(),
        }
    }

    pub fn scroll_down(&mut self) {
        let max = self.body.lines().count().saturating_sub(1) as u16;
        if self.scroll < max {
            self.scroll = self.scroll.saturating_add(1);
        }
    }

    pub fn scroll_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
    }

    pub fn set_content(&mut self, from: &str, subject: &str, date: &str, body: &str) {
        self.from = from.into();
        self.subject = subject.into();
        self.date = date.into();
        self.body = body.into();
        self.scroll = 0;
        self.attachments.clear();
        self.attachment_state.select(None);
    }

    pub fn set_attachments(&mut self, attachments: Vec<AttachmentInfo>) {
        let has_any = !attachments.is_empty();
        self.attachments = attachments;
        if has_any {
            self.attachment_state.select(Some(0));
        } else {
            self.attachment_state.select(None);
        }
    }

    pub fn selected_attachment(&self) -> Option<&AttachmentInfo> {
        let idx = self.attachment_state.selected()?;
        self.attachments.get(idx)
    }

    pub fn select_next_attachment(&mut self) {
        if self.attachments.is_empty() {
            return;
        }
        let cur = self.attachment_state.selected().unwrap_or(0);
        if cur + 1 < self.attachments.len() {
            self.attachment_state.select(Some(cur + 1));
        }
    }

    pub fn select_prev_attachment(&mut self) {
        let cur = self.attachment_state.selected().unwrap_or(0);
        if cur > 0 {
            self.attachment_state.select(Some(cur - 1));
        }
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme, focused: bool) {
        let border_color = if focused {
            theme.border_focused
        } else {
            theme.border
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border_color));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        if inner.height < 2 {
            return;
        }

        let att_height = if self.attachments.is_empty() {
            0
        } else {
            (self.attachments.len() as u16 + 2).min(inner.height / 3)
        };

        let constraints = if att_height > 0 {
            vec![
                Constraint::Length(2),
                Constraint::Min(1),
                Constraint::Length(att_height),
            ]
        } else {
            vec![Constraint::Length(2), Constraint::Min(1)]
        };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(inner);

        // Header
        let header = Paragraph::new(vec![
            Line::from(vec![
                Span::styled(
                    "From: ",
                    Style::default()
                        .fg(theme.muted)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(&self.from, Style::default().fg(theme.fg)),
                Span::styled("  Subject: ", Style::default().fg(theme.muted)),
                Span::styled(&self.subject, Style::default().fg(theme.fg)),
            ]),
            Line::from(Span::styled(
                "─".repeat(inner.width as usize),
                Style::default().fg(theme.muted),
            )),
        ]);
        frame.render_widget(header, chunks[0]);

        // Body
        let body = Paragraph::new(self.body.as_str())
            .style(Style::default().fg(theme.fg))
            .wrap(Wrap { trim: false })
            .scroll((self.scroll, 0));
        frame.render_widget(body, chunks[1]);

        // Attachments
        if att_height > 0 {
            let items: Vec<ListItem> = self
                .attachments
                .iter()
                .map(|a| {
                    let size = format_size(a.size_bytes);
                    ListItem::new(Line::from(vec![
                        Span::styled(
                            format!("  {ICON_ATTACHMENT} "),
                            Style::default().fg(theme.accent),
                        ),
                        Span::styled(&a.filename, Style::default().fg(theme.fg)),
                        Span::styled(
                            format!("  ({size}, {})", a.content_type),
                            Style::default().fg(theme.muted),
                        ),
                    ]))
                })
                .collect();

            let att_block = Block::default()
                .title(format!(
                    " {ICON_ATTACHMENT} Attachments ({}) ",
                    self.attachments.len()
                ))
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.muted));

            let list = List::new(items).block(att_block).highlight_style(
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            );

            frame.render_stateful_widget(list, chunks[2], &mut self.attachment_state);
        }
    }
}

fn format_size(bytes: i64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_starts_empty() {
        let p = Preview::new();
        assert!(p.from.is_empty());
        assert!(p.body.is_empty());
        assert_eq!(p.scroll, 0);
    }

    #[test]
    fn test_scroll_down_up() {
        let mut p = Preview::new();
        p.set_content(
            "from",
            "subject",
            "date",
            "line1\nline2\nline3\nline4\nline5",
        );
        p.scroll_down();
        assert_eq!(p.scroll, 1);
        p.scroll_down();
        assert_eq!(p.scroll, 2);
        p.scroll_up();
        assert_eq!(p.scroll, 1);
    }

    #[test]
    fn test_scroll_up_at_zero() {
        let mut p = Preview::new();
        p.scroll_up();
        assert_eq!(p.scroll, 0);
    }

    #[test]
    fn test_set_content_resets_scroll() {
        let mut p = Preview::new();
        p.scroll_down();
        p.scroll_down();
        p.set_content("bob@ex.com", "New subject", "2026-01-01", "Body");
        assert_eq!(p.scroll, 0);
        assert_eq!(p.from, "bob@ex.com");
        assert_eq!(p.subject, "New subject");
        assert_eq!(p.body, "Body");
    }

    #[test]
    fn test_html_to_plaintext_basic() {
        let result = html_to_plaintext("<p>Hello</p>");
        assert!(result.contains("Hello"));
    }

    #[test]
    fn test_html_to_plaintext_links() {
        let result = html_to_plaintext(r#"<a href="https://example.com">click</a>"#);
        assert!(result.contains("click"));
    }

    #[test]
    fn test_html_to_plaintext_empty() {
        assert_eq!(html_to_plaintext(""), "");
    }

    #[test]
    fn test_html_to_plaintext_entities() {
        let result = html_to_plaintext("<p>&amp; &lt; &gt;</p>");
        assert!(result.contains('&'));
        assert!(result.contains('<'));
        assert!(result.contains('>'));
    }

    #[test]
    fn test_set_content_clears_attachments() {
        let mut p = Preview::new();
        p.set_attachments(vec![AttachmentInfo {
            id: Uuid::new_v4(),
            filename: "test.pdf".into(),
            content_type: "application/pdf".into(),
            size_bytes: 1024,
        }]);
        assert_eq!(p.attachments.len(), 1);
        p.set_content("from", "subj", "date", "body");
        assert!(p.attachments.is_empty());
        assert_eq!(p.attachment_state.selected(), None);
    }

    #[test]
    fn test_set_attachments_selects_first() {
        let mut p = Preview::new();
        p.set_attachments(vec![
            AttachmentInfo {
                id: Uuid::new_v4(),
                filename: "a.pdf".into(),
                content_type: "application/pdf".into(),
                size_bytes: 100,
            },
            AttachmentInfo {
                id: Uuid::new_v4(),
                filename: "b.png".into(),
                content_type: "image/png".into(),
                size_bytes: 200,
            },
        ]);
        assert_eq!(p.attachment_state.selected(), Some(0));
        assert_eq!(p.attachments.len(), 2);
    }

    #[test]
    fn test_set_attachments_empty_selects_none() {
        let mut p = Preview::new();
        p.set_attachments(vec![]);
        assert_eq!(p.attachment_state.selected(), None);
    }

    #[test]
    fn test_select_next_prev_attachment() {
        let mut p = Preview::new();
        let ids = [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
        p.set_attachments(
            ids.iter()
                .enumerate()
                .map(|(i, id)| AttachmentInfo {
                    id: *id,
                    filename: format!("file{i}.txt"),
                    content_type: "text/plain".into(),
                    size_bytes: 10,
                })
                .collect(),
        );
        assert_eq!(p.attachment_state.selected(), Some(0));
        p.select_next_attachment();
        assert_eq!(p.attachment_state.selected(), Some(1));
        p.select_next_attachment();
        assert_eq!(p.attachment_state.selected(), Some(2));
        p.select_next_attachment();
        assert_eq!(p.attachment_state.selected(), Some(2)); // capped
        p.select_prev_attachment();
        assert_eq!(p.attachment_state.selected(), Some(1));
        p.select_prev_attachment();
        assert_eq!(p.attachment_state.selected(), Some(0));
        p.select_prev_attachment();
        assert_eq!(p.attachment_state.selected(), Some(0)); // capped
    }

    #[test]
    fn test_selected_attachment() {
        let mut p = Preview::new();
        assert!(p.selected_attachment().is_none());
        let id = Uuid::new_v4();
        p.set_attachments(vec![AttachmentInfo {
            id,
            filename: "test.pdf".into(),
            content_type: "application/pdf".into(),
            size_bytes: 500,
        }]);
        let sel = p.selected_attachment().unwrap();
        assert_eq!(sel.id, id);
        assert_eq!(sel.filename, "test.pdf");
    }

    #[test]
    fn test_select_next_attachment_empty() {
        let mut p = Preview::new();
        p.select_next_attachment();
        assert_eq!(p.attachment_state.selected(), None);
    }

    #[test]
    fn test_format_size_bytes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1023), "1023 B");
    }

    #[test]
    fn test_format_size_kilobytes() {
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(10240), "10.0 KB");
    }

    #[test]
    fn test_format_size_megabytes() {
        assert_eq!(format_size(1048576), "1.0 MB");
        assert_eq!(format_size(5242880), "5.0 MB");
    }
}
