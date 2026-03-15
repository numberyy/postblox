use ratatui::layout::{Constraint, Direction, Layout, Rect};

use crate::config::LayoutConfig;

#[derive(Clone, Copy)]
pub struct AppLayout {
    pub sidebar: Rect,
    pub message_list: Rect,
    pub preview: Rect,
    pub status_bar: Rect,
}

const STATUS_BAR_HEIGHT: u16 = 1;

#[cfg(test)]
pub fn compute(area: Rect) -> AppLayout {
    compute_with_config(area, &LayoutConfig::default())
}

pub fn compute_with_config(area: Rect, config: &LayoutConfig) -> AppLayout {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(6), Constraint::Length(STATUS_BAR_HEIGHT)])
        .split(area);

    let main_area = vertical[0];
    let status_bar = vertical[1];

    // Responsive: < 60 cols → hide sidebar, stack message_list and preview vertically
    if area.width < 60 {
        let halves = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(main_area);

        return AppLayout {
            sidebar: Rect::new(main_area.x, main_area.y, 0, 0),
            message_list: halves[0],
            preview: halves[1],
            status_bar,
        };
    }

    // Responsive: < 80 cols → hide sidebar
    let hide_sidebar = area.width < 80;
    let sidebar_width = if hide_sidebar {
        0
    } else {
        config.sidebar_width
    };
    let preview_pct = config.preview_height;

    let upper_lower = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(100 - preview_pct),
            Constraint::Percentage(preview_pct),
        ])
        .split(main_area);

    let upper = upper_lower[0];
    let preview = upper_lower[1];

    if hide_sidebar {
        AppLayout {
            sidebar: Rect::new(upper.x, upper.y, 0, upper.height),
            message_list: upper,
            preview,
            status_bar,
        }
    } else {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(sidebar_width), Constraint::Min(30)])
            .split(upper);

        AppLayout {
            sidebar: cols[0],
            message_list: cols[1],
            preview,
            status_bar,
        }
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
        let cfg = LayoutConfig::default();
        let l = compute_with_config(rect(100, 40), &cfg);
        assert_eq!(l.sidebar.width, cfg.sidebar_width);
        assert_eq!(l.status_bar.height, STATUS_BAR_HEIGHT);
        assert_eq!(l.message_list.x, cfg.sidebar_width);
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
        // Below 60 cols: sidebar hidden
        assert_eq!(l.sidebar.width, 0);
        assert!(l.message_list.width > 0);
        assert_eq!(l.status_bar.height, 1);
    }

    #[test]
    fn test_layout_covers_full_area() {
        let area = rect(100, 40);
        let l = compute(area);
        assert_eq!(l.status_bar.y + l.status_bar.height, area.height);
        assert_eq!(l.sidebar.y, l.message_list.y);
    }

    #[test]
    fn test_layout_responsive_hide_sidebar_below_80() {
        let l = compute(rect(75, 30));
        assert_eq!(l.sidebar.width, 0);
        assert!(l.message_list.width > 0);
    }

    #[test]
    fn test_layout_responsive_stack_below_60() {
        let l = compute(rect(50, 30));
        assert_eq!(l.sidebar.width, 0);
        // Message list and preview stacked vertically, both using full width
        assert_eq!(l.message_list.width, 50);
        assert_eq!(l.preview.width, 50);
        assert!(l.message_list.y < l.preview.y);
    }

    #[test]
    fn test_layout_custom_sidebar_width() {
        let cfg = LayoutConfig {
            sidebar_width: 30,
            preview_height: 40,
        };
        let l = compute_with_config(rect(100, 40), &cfg);
        assert_eq!(l.sidebar.width, 30);
        assert_eq!(l.message_list.x, 30);
    }

    #[test]
    fn test_layout_custom_preview_height() {
        let cfg = LayoutConfig {
            sidebar_width: 22,
            preview_height: 60,
        };
        let l = compute_with_config(rect(100, 40), &cfg);
        // Preview should be larger than default (60% vs 40%)
        let default_l = compute(rect(100, 40));
        assert!(l.preview.height > default_l.preview.height);
    }
}
