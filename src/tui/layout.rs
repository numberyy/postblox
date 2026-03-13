use ratatui::layout::{Constraint, Direction, Layout, Rect};

pub struct AppLayout {
    pub sidebar: Rect,
    pub message_list: Rect,
    pub preview: Rect,
    pub status_bar: Rect,
}

const SIDEBAR_WIDTH: u16 = 22;
const STATUS_BAR_HEIGHT: u16 = 1;

pub fn compute(area: Rect) -> AppLayout {
    // Vertical: [main area] [status bar]
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(6), Constraint::Length(STATUS_BAR_HEIGHT)])
        .split(area);

    let main_area = vertical[0];
    let status_bar = vertical[1];

    // Main: upper (sidebar | message list) and lower (preview)
    let upper_lower = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(main_area);

    let upper = upper_lower[0];
    let preview = upper_lower[1];

    // Upper: sidebar | message list
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(SIDEBAR_WIDTH), Constraint::Min(30)])
        .split(upper);

    AppLayout {
        sidebar: cols[0],
        message_list: cols[1],
        preview,
        status_bar,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(w: u16, h: u16) -> Rect {
        Rect::new(0, 0, w, h)
    }

    #[test]
    fn test_layout_basic_dimensions() {
        let l = compute(rect(100, 40));
        assert_eq!(l.sidebar.width, SIDEBAR_WIDTH);
        assert_eq!(l.status_bar.height, STATUS_BAR_HEIGHT);
        assert_eq!(l.message_list.x, SIDEBAR_WIDTH);
        assert!(l.message_list.width > 0);
        assert!(l.preview.height > 0);
    }

    #[test]
    fn test_layout_status_bar_at_bottom() {
        let l = compute(rect(80, 30));
        assert_eq!(l.status_bar.y + l.status_bar.height, 30);
    }

    #[test]
    fn test_layout_sidebar_full_height_of_upper() {
        let l = compute(rect(80, 40));
        assert_eq!(l.sidebar.y, 0);
        assert_eq!(l.sidebar.height, l.message_list.height);
    }

    #[test]
    fn test_layout_preview_below_upper() {
        let l = compute(rect(80, 40));
        assert!(l.preview.y >= l.sidebar.height);
        assert_eq!(l.preview.width, 80);
    }

    #[test]
    fn test_layout_small_terminal() {
        let l = compute(rect(40, 10));
        assert!(l.sidebar.width <= 22);
        assert!(l.message_list.width > 0);
        assert!(l.status_bar.height == 1);
    }

    #[test]
    fn test_layout_covers_full_area() {
        let area = rect(100, 40);
        let l = compute(area);
        // Status bar bottom edge matches area bottom
        assert_eq!(l.status_bar.y + l.status_bar.height, area.height);
        // Sidebar and message_list share the same row span
        assert_eq!(l.sidebar.y, l.message_list.y);
    }
}
