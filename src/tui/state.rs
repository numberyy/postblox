#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Sidebar,
    MessageList,
    Preview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Compose,
    Search,
    Help,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_panel_eq() {
        assert_eq!(Panel::Sidebar, Panel::Sidebar);
        assert_ne!(Panel::Sidebar, Panel::MessageList);
    }

    #[test]
    fn test_mode_eq() {
        assert_eq!(Mode::Normal, Mode::Normal);
        assert_ne!(Mode::Normal, Mode::Compose);
    }
}
