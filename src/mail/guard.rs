use regex::Regex;

#[derive(Clone)]
pub struct GuardPattern {
    pub name: String,
    pub regex: Regex,
}

pub struct GuardViolation {
    pub pattern_name: String,
    pub field: String,
}

pub fn default_patterns() -> Vec<GuardPattern> {
    let defs: &[(&str, &str)] = &[
        ("aws_access_key", r"AKIA[0-9A-Z]{16}"),
        ("anthropic_api_key", r"sk-ant-[a-zA-Z0-9\-]{20,}"),
        ("openai_api_key", r"\bsk-[a-zA-Z0-9]{20,}\b"),
        ("github_token", r"(ghp|gho|ghu|ghs|ghk)_[a-zA-Z0-9]{36,}"),
        ("stripe_secret_key", r"(sk|rk)_live_[a-zA-Z0-9]{20,}"),
        ("private_key", r"-----BEGIN[A-Z ]*PRIVATE KEY-----"),
        ("ssn", r"\b\d{3}-\d{2}-\d{4}\b"),
        (
            "credit_card",
            r"\b\d{4}[\s\-]?\d{4}[\s\-]?\d{4}[\s\-]?\d{4}\b",
        ),
    ];

    defs.iter()
        .map(|(name, pattern)| GuardPattern {
            name: (*name).into(),
            regex: Regex::new(pattern).expect("built-in guard pattern must compile"),
        })
        .collect()
}

pub fn scan(
    subject: Option<&str>,
    text_body: Option<&str>,
    html_body: Option<&str>,
    patterns: &[GuardPattern],
) -> Result<(), Vec<GuardViolation>> {
    let fields: &[(&str, Option<&str>)] = &[
        ("subject", subject),
        ("text_body", text_body),
        ("html_body", html_body),
    ];

    let mut violations = Vec::new();

    for &(field_name, field_value) in fields {
        if let Some(text) = field_value {
            for pat in patterns {
                if pat.regex.is_match(text) {
                    violations.push(GuardViolation {
                        pattern_name: pat.name.clone(),
                        field: field_name.into(),
                    });
                }
            }
        }
    }

    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patterns() -> Vec<GuardPattern> {
        default_patterns()
    }

    #[test]
    fn test_guard_default_patterns_compile() {
        let pats = default_patterns();
        assert!(pats.len() >= 8);
    }

    #[test]
    fn test_guard_clean_message_passes() {
        let pats = patterns();
        assert!(scan(
            Some("Weekly report"),
            Some("Here is the summary of this week's activity."),
            None,
            &pats,
        )
        .is_ok());
    }

    #[test]
    fn test_guard_aws_key_detected() {
        let pats = patterns();
        let result = scan(None, Some("key: AKIAIOSFODNN7EXAMPLE"), None, &pats);
        let violations = result.unwrap_err();
        assert!(violations
            .iter()
            .any(|v| v.pattern_name == "aws_access_key"));
    }

    #[test]
    fn test_guard_aws_key_too_short_passes() {
        let pats = patterns();
        assert!(scan(None, Some("AKIA"), None, &pats).is_ok());
    }

    #[test]
    fn test_guard_openai_key_detected() {
        let pats = patterns();
        let result = scan(None, Some("sk-abcdefghijklmnopqrst"), None, &pats);
        let violations = result.unwrap_err();
        assert!(violations
            .iter()
            .any(|v| v.pattern_name == "openai_api_key"));
    }

    #[test]
    fn test_guard_openai_key_too_short_passes() {
        let pats = patterns();
        assert!(scan(None, Some("sk-abc"), None, &pats).is_ok());
    }

    #[test]
    fn test_guard_anthropic_key_detected() {
        let pats = patterns();
        let result = scan(None, Some("sk-ant-api03-abcdefghijklmnopqrst"), None, &pats);
        let violations = result.unwrap_err();
        assert!(violations
            .iter()
            .any(|v| v.pattern_name == "anthropic_api_key"));
    }

    #[test]
    fn test_guard_github_token_ghp_detected() {
        let pats = patterns();
        let token = format!("ghp_{}", "a".repeat(36));
        let result = scan(None, Some(&token), None, &pats);
        let violations = result.unwrap_err();
        assert!(violations.iter().any(|v| v.pattern_name == "github_token"));
    }

    #[test]
    fn test_guard_github_token_ghs_detected() {
        let pats = patterns();
        let token = format!("ghs_{}", "B".repeat(40));
        let result = scan(None, Some(&token), None, &pats);
        let violations = result.unwrap_err();
        assert!(violations.iter().any(|v| v.pattern_name == "github_token"));
    }

    #[test]
    fn test_guard_stripe_key_detected() {
        let pats = patterns();
        let result = scan(None, Some("sk_live_abcdefghijklmnopqrst"), None, &pats);
        let violations = result.unwrap_err();
        assert!(violations
            .iter()
            .any(|v| v.pattern_name == "stripe_secret_key"));
    }

    #[test]
    fn test_guard_stripe_rk_detected() {
        let pats = patterns();
        let result = scan(None, Some("rk_live_abcdefghijklmnopqrst"), None, &pats);
        let violations = result.unwrap_err();
        assert!(violations
            .iter()
            .any(|v| v.pattern_name == "stripe_secret_key"));
    }

    #[test]
    fn test_guard_private_key_detected() {
        let pats = patterns();
        let result = scan(None, Some("-----BEGIN RSA PRIVATE KEY-----"), None, &pats);
        let violations = result.unwrap_err();
        assert!(violations.iter().any(|v| v.pattern_name == "private_key"));
    }

    #[test]
    fn test_guard_private_key_ec_detected() {
        let pats = patterns();
        let result = scan(None, Some("-----BEGIN EC PRIVATE KEY-----"), None, &pats);
        let violations = result.unwrap_err();
        assert!(violations.iter().any(|v| v.pattern_name == "private_key"));
    }

    #[test]
    fn test_guard_ssn_detected() {
        let pats = patterns();
        let result = scan(None, Some("my ssn is 123-45-6789 thanks"), None, &pats);
        let violations = result.unwrap_err();
        assert!(violations.iter().any(|v| v.pattern_name == "ssn"));
    }

    #[test]
    fn test_guard_ssn_no_word_boundary_passes() {
        let pats = patterns();
        assert!(scan(None, Some("1123-45-67891"), None, &pats).is_ok());
    }

    #[test]
    fn test_guard_credit_card_spaces_detected() {
        let pats = patterns();
        let result = scan(None, Some("card: 4111 1111 1111 1111"), None, &pats);
        let violations = result.unwrap_err();
        assert!(violations.iter().any(|v| v.pattern_name == "credit_card"));
    }

    #[test]
    fn test_guard_credit_card_dashes_detected() {
        let pats = patterns();
        let result = scan(None, Some("card: 4111-1111-1111-1111"), None, &pats);
        let violations = result.unwrap_err();
        assert!(violations.iter().any(|v| v.pattern_name == "credit_card"));
    }

    #[test]
    fn test_guard_credit_card_no_separator_detected() {
        let pats = patterns();
        let result = scan(None, Some("card: 4111111111111111"), None, &pats);
        let violations = result.unwrap_err();
        assert!(violations.iter().any(|v| v.pattern_name == "credit_card"));
    }

    #[test]
    fn test_guard_multiple_violations() {
        let pats = patterns();
        let body = "key: AKIAIOSFODNN7EXAMPLE and ssn: 123-45-6789";
        let result = scan(None, Some(body), None, &pats);
        let violations = result.unwrap_err();
        assert!(violations
            .iter()
            .any(|v| v.pattern_name == "aws_access_key"));
        assert!(violations.iter().any(|v| v.pattern_name == "ssn"));
    }

    #[test]
    fn test_guard_secret_in_subject_detected() {
        let pats = patterns();
        let result = scan(Some("key: AKIAIOSFODNN7EXAMPLE"), None, None, &pats);
        let violations = result.unwrap_err();
        assert!(violations
            .iter()
            .any(|v| v.pattern_name == "aws_access_key" && v.field == "subject"));
    }

    #[test]
    fn test_guard_secret_in_html_detected() {
        let pats = patterns();
        let result = scan(None, None, Some("<p>key: AKIAIOSFODNN7EXAMPLE</p>"), &pats);
        let violations = result.unwrap_err();
        assert!(violations
            .iter()
            .any(|v| v.pattern_name == "aws_access_key" && v.field == "html_body"));
    }

    #[test]
    fn test_guard_empty_fields_passes() {
        let pats = patterns();
        assert!(scan(None, None, None, &pats).is_ok());
    }

    #[test]
    fn test_guard_custom_pattern() {
        let mut pats = patterns();
        pats.push(GuardPattern {
            name: "custom_token".into(),
            regex: Regex::new(r"CUSTOM_[A-Z]{10}").unwrap(),
        });
        let result = scan(None, Some("CUSTOM_ABCDEFGHIJ"), None, &pats);
        let violations = result.unwrap_err();
        assert!(violations.iter().any(|v| v.pattern_name == "custom_token"));
    }
}
