#[derive(Debug, Clone)]
pub struct ContentFilter {
    allowed_types: Option<Vec<String>>,
    blocked_types: Option<Vec<String>>,
}

#[derive(Debug, PartialEq)]
pub enum FilterResult {
    Allow,
    Block(String),
}

impl ContentFilter {
    pub fn new(allowed_types: Option<Vec<String>>, blocked_types: Option<Vec<String>>) -> Self {
        Self {
            allowed_types: allowed_types
                .map(|v| v.into_iter().map(|s| s.to_ascii_lowercase()).collect()),
            blocked_types: blocked_types
                .map(|v| v.into_iter().map(|s| s.to_ascii_lowercase()).collect()),
        }
    }

    pub fn check(&self, content_type: &str) -> FilterResult {
        let ct = content_type.to_ascii_lowercase();

        if let Some(blocked) = &self.blocked_types {
            for pattern in blocked {
                if glob_match(&ct, pattern) {
                    return FilterResult::Block(format!(
                        "content type '{content_type}' blocked by pattern '{pattern}'"
                    ));
                }
            }
        }

        if let Some(allowed) = &self.allowed_types {
            for pattern in allowed {
                if glob_match(&ct, pattern) {
                    return FilterResult::Allow;
                }
            }
            return FilterResult::Block(format!("content type '{content_type}' not in allowlist"));
        }

        FilterResult::Allow
    }
}

fn glob_match(value: &str, pattern: &str) -> bool {
    if pattern == "*/*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        value.starts_with(prefix)
    } else {
        value == pattern
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(allowed: Option<Vec<&str>>, blocked: Option<Vec<&str>>) -> ContentFilter {
        ContentFilter::new(
            allowed.map(|v| v.into_iter().map(String::from).collect()),
            blocked.map(|v| v.into_iter().map(String::from).collect()),
        )
    }

    #[test]
    fn test_no_config_allows_all() {
        let f = filter(None, None);
        assert_eq!(f.check("application/pdf"), FilterResult::Allow);
        assert_eq!(f.check("image/png"), FilterResult::Allow);
        assert_eq!(f.check("text/html"), FilterResult::Allow);
    }

    #[test]
    fn test_allowlist_exact_match() {
        let f = filter(Some(vec!["application/pdf", "image/png"]), None);
        assert_eq!(f.check("application/pdf"), FilterResult::Allow);
        assert_eq!(f.check("image/png"), FilterResult::Allow);
        assert!(matches!(f.check("text/html"), FilterResult::Block(_)));
    }

    #[test]
    fn test_allowlist_glob() {
        let f = filter(Some(vec!["image/*"]), None);
        assert_eq!(f.check("image/png"), FilterResult::Allow);
        assert_eq!(f.check("image/jpeg"), FilterResult::Allow);
        assert!(matches!(f.check("application/pdf"), FilterResult::Block(_)));
    }

    #[test]
    fn test_allowlist_star_star() {
        let f = filter(Some(vec!["*/*"]), None);
        assert_eq!(f.check("anything/here"), FilterResult::Allow);
    }

    #[test]
    fn test_blocklist_exact() {
        let f = filter(None, Some(vec!["application/x-executable"]));
        assert!(matches!(
            f.check("application/x-executable"),
            FilterResult::Block(_)
        ));
        assert_eq!(f.check("application/pdf"), FilterResult::Allow);
    }

    #[test]
    fn test_blocklist_glob() {
        let f = filter(None, Some(vec!["application/x-*"]));
        assert!(matches!(
            f.check("application/x-executable"),
            FilterResult::Block(_)
        ));
        assert!(matches!(
            f.check("application/x-shellscript"),
            FilterResult::Block(_)
        ));
        assert_eq!(f.check("application/pdf"), FilterResult::Allow);
    }

    #[test]
    fn test_blocklist_priority_over_allowlist() {
        let f = filter(
            Some(vec!["application/*"]),
            Some(vec!["application/x-executable"]),
        );
        assert!(matches!(
            f.check("application/x-executable"),
            FilterResult::Block(_)
        ));
        assert_eq!(f.check("application/pdf"), FilterResult::Allow);
    }

    #[test]
    fn test_blocklist_glob_priority_over_allowlist() {
        let f = filter(Some(vec!["*/*"]), Some(vec!["application/x-*"]));
        assert!(matches!(
            f.check("application/x-executable"),
            FilterResult::Block(_)
        ));
        assert_eq!(f.check("image/png"), FilterResult::Allow);
    }

    #[test]
    fn test_case_insensitive() {
        let f = filter(Some(vec!["Image/PNG"]), None);
        assert_eq!(f.check("image/png"), FilterResult::Allow);
        assert_eq!(f.check("IMAGE/PNG"), FilterResult::Allow);
    }

    #[test]
    fn test_case_insensitive_blocklist() {
        let f = filter(None, Some(vec!["APPLICATION/X-EXECUTABLE"]));
        assert!(matches!(
            f.check("application/x-executable"),
            FilterResult::Block(_)
        ));
    }

    #[test]
    fn test_empty_allowlist_blocks_all() {
        let f = filter(Some(vec![]), None);
        assert!(matches!(f.check("image/png"), FilterResult::Block(_)));
    }

    #[test]
    fn test_empty_blocklist_allows_all() {
        let f = filter(None, Some(vec![]));
        assert_eq!(f.check("image/png"), FilterResult::Allow);
    }

    #[test]
    fn test_block_reason_includes_type_and_pattern() {
        let f = filter(None, Some(vec!["image/*"]));
        match f.check("image/png") {
            FilterResult::Block(reason) => {
                assert!(reason.contains("image/png"), "reason: {reason}");
                assert!(reason.contains("image/*"), "reason: {reason}");
            }
            FilterResult::Allow => panic!("expected block"),
        }
    }

    #[test]
    fn test_allowlist_not_in_list_reason() {
        let f = filter(Some(vec!["application/pdf"]), None);
        match f.check("text/html") {
            FilterResult::Block(reason) => {
                assert!(reason.contains("text/html"), "reason: {reason}");
                assert!(reason.contains("allowlist"), "reason: {reason}");
            }
            FilterResult::Allow => panic!("expected block"),
        }
    }

    #[test]
    fn test_glob_does_not_match_partial_prefix() {
        let f = filter(Some(vec!["image/*"]), None);
        assert!(matches!(f.check("images/png"), FilterResult::Block(_)));
        assert!(matches!(f.check("imag/png"), FilterResult::Block(_)));
    }

    #[test]
    fn test_glob_requires_slash_after_prefix() {
        let f = filter(Some(vec!["image/*"]), None);
        assert!(matches!(f.check("imagepng"), FilterResult::Block(_)));
    }

    #[test]
    fn test_multiple_blocklist_patterns() {
        let f = filter(
            None,
            Some(vec![
                "application/x-executable",
                "application/x-shellscript",
                "text/x-script",
            ]),
        );
        assert!(matches!(
            f.check("application/x-executable"),
            FilterResult::Block(_)
        ));
        assert!(matches!(
            f.check("application/x-shellscript"),
            FilterResult::Block(_)
        ));
        assert!(matches!(f.check("text/x-script"), FilterResult::Block(_)));
        assert_eq!(f.check("image/png"), FilterResult::Allow);
    }

    #[test]
    fn test_octet_stream_default_allowed() {
        let f = filter(None, None);
        assert_eq!(f.check("application/octet-stream"), FilterResult::Allow);
    }

    #[test]
    fn test_blocklist_then_allowlist_order() {
        let f = filter(Some(vec!["image/png"]), Some(vec!["image/png"]));
        assert!(matches!(f.check("image/png"), FilterResult::Block(_)));
    }
}
