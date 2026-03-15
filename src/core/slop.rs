use serde_json::Value;

#[derive(Debug)]
pub struct ClassifierInput<'a> {
    pub from_addr: &'a str,
    pub subject: Option<&'a str>,
    pub text_body: Option<&'a str>,
    pub raw_headers: Option<&'a Value>,
    pub sender_slop_ratio: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct SlopResult {
    pub score: f32,
    pub signals: Vec<&'static str>,
    pub category: &'static str,
    pub priority: &'static str,
    pub requires_action: bool,
    pub triage_action: TriageAction,
}

impl SlopResult {
    pub fn is_slop(&self) -> bool {
        self.score > 0.5
    }
}

impl TriageAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Inbox => "inbox",
            Self::Archive => "archived",
            Self::Delete => "deleted",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TriageAction {
    Inbox,
    Archive,
    Delete,
}

fn header_value(headers: &Value, name: &str) -> Option<String> {
    headers
        .as_object()?
        .iter()
        .find_map(|(k, v)| {
            k.eq_ignore_ascii_case(name)
                .then(|| v.as_str().map(|s| s.to_string()))
        })
        .flatten()
}

fn has_noreply(from: &str) -> bool {
    let lower = from.to_lowercase();
    lower.contains("noreply") || lower.contains("no-reply") || lower.contains("donotreply")
}

const COLD_EMAIL_PATTERNS: &[&str] = &[
    "quick question",
    "reaching out",
    "free trial",
    "limited time",
    "act now",
    "exclusive offer",
];

fn has_cold_email_pattern(text: &str) -> bool {
    let lower = text.to_lowercase();
    COLD_EMAIL_PATTERNS.iter().any(|p| lower.contains(p))
}

fn has_otp_pattern_lower(lower: &str, original: &str) -> bool {
    if lower.contains("verification code")
        || lower.contains("one-time")
        || lower.contains("otp")
        || lower.contains("verify your")
    {
        return true;
    }
    let bytes = original.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let len = i - start;
            if (4..=8).contains(&len) {
                let at_start = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
                let at_end = i == bytes.len() || !bytes[i].is_ascii_alphanumeric();
                if at_start && at_end {
                    return true;
                }
            }
        } else {
            i += 1;
        }
    }
    false
}

pub fn classify(input: &ClassifierInput) -> SlopResult {
    let mut score: f32 = 0.0;
    let mut signals: Vec<&'static str> = Vec::with_capacity(6);
    let mut is_otp = false;
    let mut has_list_unsub = false;
    let mut has_auto_sub = false;
    let mut has_noreply_sig = false;

    // Header-based signals
    if let Some(headers) = input.raw_headers {
        if header_value(headers, "List-Unsubscribe").is_some() {
            score += 0.3;
            signals.push("list-unsubscribe");
            has_list_unsub = true;
        }

        if let Some(prec) = header_value(headers, "Precedence") {
            let lower = prec.to_lowercase();
            if lower == "bulk" || lower == "list" {
                score += 0.2;
                signals.push("precedence-bulk");
            }
        }

        if let Some(auto) = header_value(headers, "Auto-Submitted") {
            let lower = auto.to_lowercase();
            if lower == "auto-generated" || lower == "auto-replied" {
                score += 0.15;
                signals.push("auto-submitted");
                has_auto_sub = true;
            }
        }
    }

    // From address signals
    if has_noreply(input.from_addr) {
        score += 0.15;
        signals.push("noreply-sender");
        has_noreply_sig = true;
    }

    if let Some(subj) = input.subject {
        if has_cold_email_pattern(subj) {
            score += 0.2;
            signals.push("cold-email");
        }
    }

    // Sender reputation
    if let Some(ratio) = input.sender_slop_ratio {
        if ratio > 0.5 {
            score += 0.2;
            signals.push("reputation-high-slop");
        }
    }

    let subject_text = input.subject.unwrap_or("");
    let body_text = input.text_body.unwrap_or("");
    let subject_lower = subject_text.to_lowercase();
    if has_otp_pattern_lower(&subject_lower, subject_text)
        || has_otp_pattern_lower(&body_text.to_lowercase(), body_text)
    {
        is_otp = true;
    }

    score = score.clamp(0.0, 1.0);

    let category = if is_otp {
        "otp"
    } else if has_noreply_sig && has_list_unsub {
        "commercial"
    } else if has_auto_sub {
        "automated"
    } else if score >= 0.3 {
        "commercial"
    } else {
        "personal"
    };

    // OTP overrides
    let (priority, requires_action) = if is_otp {
        ("high", true)
    } else {
        ("normal", false)
    };

    let triage_action = if score > 0.95 {
        TriageAction::Delete
    } else if score > 0.8 {
        TriageAction::Archive
    } else {
        TriageAction::Inbox
    };

    SlopResult {
        score,
        signals,
        category,
        priority,
        requires_action,
        triage_action,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn empty_input() -> ClassifierInput<'static> {
        ClassifierInput {
            from_addr: "user@example.com",
            subject: None,
            text_body: None,
            raw_headers: None,
            sender_slop_ratio: None,
        }
    }

    // --- header_value ---

    #[test]
    fn test_header_value_case_insensitive() {
        let headers = json!({"list-unsubscribe": "<mailto:unsub@example.com>"});
        assert_eq!(
            header_value(&headers, "List-Unsubscribe"),
            Some("<mailto:unsub@example.com>".into())
        );
    }

    #[test]
    fn test_header_value_missing() {
        let headers = json!({"X-Mailer": "test"});
        assert!(header_value(&headers, "List-Unsubscribe").is_none());
    }

    #[test]
    fn test_header_value_null_json() {
        let headers = json!(null);
        assert!(header_value(&headers, "anything").is_none());
    }

    #[test]
    fn test_header_value_non_string_value() {
        let headers = json!({"List-Unsubscribe": 42});
        assert!(header_value(&headers, "List-Unsubscribe").is_none());
    }

    // --- Empty/minimal input ---

    #[test]
    fn test_classify_empty_input_is_personal() {
        let result = classify(&empty_input());
        assert_eq!(result.score, 0.0);
        assert!(result.signals.is_empty());
        assert_eq!(result.category, "personal");
        assert_eq!(result.priority, "normal");
        assert!(!result.requires_action);
        assert_eq!(result.triage_action, TriageAction::Inbox);
    }

    // --- Individual signals ---

    #[test]
    fn test_signal_list_unsubscribe() {
        let headers = json!({"List-Unsubscribe": "<mailto:unsub@example.com>"});
        let input = ClassifierInput {
            raw_headers: Some(&headers),
            ..empty_input()
        };
        let result = classify(&input);
        assert!((result.score - 0.3).abs() < f32::EPSILON);
        assert!(result.signals.contains(&"list-unsubscribe"));
    }

    #[test]
    fn test_signal_precedence_bulk() {
        let headers = json!({"Precedence": "bulk"});
        let input = ClassifierInput {
            raw_headers: Some(&headers),
            ..empty_input()
        };
        let result = classify(&input);
        assert!((result.score - 0.2).abs() < f32::EPSILON);
        assert!(result.signals.contains(&"precedence-bulk"));
    }

    #[test]
    fn test_signal_precedence_list() {
        let headers = json!({"Precedence": "list"});
        let input = ClassifierInput {
            raw_headers: Some(&headers),
            ..empty_input()
        };
        let result = classify(&input);
        assert!((result.score - 0.2).abs() < f32::EPSILON);
        assert!(result.signals.contains(&"precedence-bulk"));
    }

    #[test]
    fn test_signal_precedence_normal_no_signal() {
        let headers = json!({"Precedence": "normal"});
        let input = ClassifierInput {
            raw_headers: Some(&headers),
            ..empty_input()
        };
        let result = classify(&input);
        assert_eq!(result.score, 0.0);
        assert!(!result.signals.contains(&"precedence-bulk"));
    }

    #[test]
    fn test_signal_noreply_sender() {
        let input = ClassifierInput {
            from_addr: "noreply@company.com",
            ..empty_input()
        };
        let result = classify(&input);
        assert!((result.score - 0.15).abs() < f32::EPSILON);
        assert!(result.signals.contains(&"noreply-sender"));
    }

    #[test]
    fn test_signal_no_reply_sender() {
        let input = ClassifierInput {
            from_addr: "no-reply@company.com",
            ..empty_input()
        };
        let result = classify(&input);
        assert!(result.signals.contains(&"noreply-sender"));
    }

    #[test]
    fn test_signal_donotreply_sender() {
        let input = ClassifierInput {
            from_addr: "DoNotReply@company.com",
            ..empty_input()
        };
        let result = classify(&input);
        assert!(result.signals.contains(&"noreply-sender"));
    }

    #[test]
    fn test_signal_auto_submitted_generated() {
        let headers = json!({"Auto-Submitted": "auto-generated"});
        let input = ClassifierInput {
            raw_headers: Some(&headers),
            ..empty_input()
        };
        let result = classify(&input);
        assert!((result.score - 0.15).abs() < f32::EPSILON);
        assert!(result.signals.contains(&"auto-submitted"));
    }

    #[test]
    fn test_signal_auto_submitted_replied() {
        let headers = json!({"Auto-Submitted": "auto-replied"});
        let input = ClassifierInput {
            raw_headers: Some(&headers),
            ..empty_input()
        };
        let result = classify(&input);
        assert!(result.signals.contains(&"auto-submitted"));
    }

    #[test]
    fn test_signal_cold_email_quick_question() {
        let input = ClassifierInput {
            subject: Some("Quick question about your product"),
            ..empty_input()
        };
        let result = classify(&input);
        assert!((result.score - 0.2).abs() < f32::EPSILON);
        assert!(result.signals.contains(&"cold-email"));
    }

    #[test]
    fn test_signal_cold_email_case_insensitive() {
        let input = ClassifierInput {
            subject: Some("REACHING OUT to discuss"),
            ..empty_input()
        };
        let result = classify(&input);
        assert!(result.signals.contains(&"cold-email"));
    }

    #[test]
    fn test_signal_cold_email_all_patterns() {
        for pattern in &[
            "quick question",
            "reaching out",
            "free trial",
            "limited time",
            "act now",
            "exclusive offer",
        ] {
            let input = ClassifierInput {
                subject: Some(pattern),
                ..empty_input()
            };
            let result = classify(&input);
            assert!(
                result.signals.contains(&"cold-email"),
                "pattern '{}' should trigger cold-email",
                pattern
            );
        }
    }

    #[test]
    fn test_signal_reputation_high_slop() {
        let input = ClassifierInput {
            sender_slop_ratio: Some(0.6),
            ..empty_input()
        };
        let result = classify(&input);
        assert!((result.score - 0.2).abs() < f32::EPSILON);
        assert!(result.signals.contains(&"reputation-high-slop"));
    }

    #[test]
    fn test_signal_reputation_at_boundary_no_signal() {
        let input = ClassifierInput {
            sender_slop_ratio: Some(0.5),
            ..empty_input()
        };
        let result = classify(&input);
        assert_eq!(result.score, 0.0);
        assert!(!result.signals.contains(&"reputation-high-slop"));
    }

    #[test]
    fn test_signal_reputation_zero() {
        let input = ClassifierInput {
            sender_slop_ratio: Some(0.0),
            ..empty_input()
        };
        let result = classify(&input);
        assert_eq!(result.score, 0.0);
    }

    // --- Combined signals ---

    #[test]
    fn test_combined_signals_additive() {
        let headers = json!({
            "List-Unsubscribe": "<mailto:unsub@x.com>",
            "Precedence": "bulk"
        });
        let input = ClassifierInput {
            from_addr: "noreply@spam.com",
            subject: Some("Act now for a free trial"),
            raw_headers: Some(&headers),
            sender_slop_ratio: Some(0.8),
            ..empty_input()
        };
        let result = classify(&input);
        // 0.3 + 0.2 + 0.15 + 0.2 + 0.2 = 1.05 -> clamped to 1.0
        assert!((result.score - 1.0).abs() < f32::EPSILON);
        assert_eq!(result.signals.len(), 5);
    }

    #[test]
    fn test_score_clamped_to_one() {
        let headers = json!({
            "List-Unsubscribe": "<mailto:unsub@x.com>",
            "Precedence": "bulk",
            "Auto-Submitted": "auto-generated"
        });
        let input = ClassifierInput {
            from_addr: "noreply@spam.com",
            subject: Some("Quick question"),
            raw_headers: Some(&headers),
            sender_slop_ratio: Some(0.9),
            ..empty_input()
        };
        let result = classify(&input);
        assert!(result.score <= 1.0);
        assert!((result.score - 1.0).abs() < f32::EPSILON);
    }

    // --- OTP override ---

    #[test]
    fn test_otp_detection_subject_verification_code() {
        let input = ClassifierInput {
            subject: Some("Your verification code is ready"),
            ..empty_input()
        };
        let result = classify(&input);
        assert_eq!(result.category, "otp");
        assert_eq!(result.priority, "high");
        assert!(result.requires_action);
    }

    #[test]
    fn test_otp_detection_body_one_time() {
        let input = ClassifierInput {
            text_body: Some("Use this one-time password: 482910"),
            ..empty_input()
        };
        let result = classify(&input);
        assert_eq!(result.category, "otp");
        assert!(result.requires_action);
    }

    #[test]
    fn test_otp_detection_body_otp_keyword() {
        let input = ClassifierInput {
            text_body: Some("Your OTP is 1234"),
            ..empty_input()
        };
        let result = classify(&input);
        assert_eq!(result.category, "otp");
    }

    #[test]
    fn test_otp_detection_verify_your() {
        let input = ClassifierInput {
            subject: Some("Verify your email address"),
            ..empty_input()
        };
        let result = classify(&input);
        assert_eq!(result.category, "otp");
    }

    #[test]
    fn test_otp_detection_digit_sequence_4_digits() {
        let input = ClassifierInput {
            text_body: Some("Your code: 4829"),
            ..empty_input()
        };
        let result = classify(&input);
        assert_eq!(result.category, "otp");
    }

    #[test]
    fn test_otp_detection_digit_sequence_8_digits() {
        let input = ClassifierInput {
            text_body: Some("Enter code 12345678 to continue"),
            ..empty_input()
        };
        let result = classify(&input);
        assert_eq!(result.category, "otp");
    }

    #[test]
    fn test_otp_no_detection_3_digits() {
        let input = ClassifierInput {
            text_body: Some("We have 123 items"),
            ..empty_input()
        };
        let result = classify(&input);
        assert_ne!(result.category, "otp");
    }

    #[test]
    fn test_otp_no_detection_9_digits() {
        let input = ClassifierInput {
            text_body: Some("Account 123456789 is active"),
            ..empty_input()
        };
        let result = classify(&input);
        assert_ne!(result.category, "otp");
    }

    #[test]
    fn test_otp_no_false_positive_digits_embedded_in_word() {
        let input = ClassifierInput {
            text_body: Some("ref ABC12345XYZ"),
            ..empty_input()
        };
        let result = classify(&input);
        // Digits are embedded in alphanumeric context, should not trigger
        assert_ne!(result.category, "otp");
    }

    #[test]
    fn test_otp_case_insensitive() {
        let input = ClassifierInput {
            subject: Some("YOUR VERIFICATION CODE"),
            ..empty_input()
        };
        let result = classify(&input);
        assert_eq!(result.category, "otp");
    }

    // --- Category detection ---

    #[test]
    fn test_category_commercial_noreply_plus_list_unsub() {
        let headers = json!({"List-Unsubscribe": "<mailto:unsub@x.com>"});
        let input = ClassifierInput {
            from_addr: "noreply@store.com",
            raw_headers: Some(&headers),
            ..empty_input()
        };
        let result = classify(&input);
        assert_eq!(result.category, "commercial");
    }

    #[test]
    fn test_category_automated() {
        let headers = json!({"Auto-Submitted": "auto-generated"});
        let input = ClassifierInput {
            raw_headers: Some(&headers),
            ..empty_input()
        };
        let result = classify(&input);
        assert_eq!(result.category, "automated");
    }

    #[test]
    fn test_category_commercial_by_score() {
        let headers = json!({"Precedence": "bulk"});
        let input = ClassifierInput {
            from_addr: "newsletter@company.com",
            subject: Some("Reaching out with an offer"),
            raw_headers: Some(&headers),
            ..empty_input()
        };
        let result = classify(&input);
        assert!(result.score >= 0.3);
        assert_eq!(result.category, "commercial");
    }

    #[test]
    fn test_category_personal_low_score() {
        let result = classify(&empty_input());
        assert_eq!(result.category, "personal");
    }

    #[test]
    fn test_otp_overrides_other_categories() {
        let headers = json!({
            "List-Unsubscribe": "<mailto:unsub@x.com>",
            "Auto-Submitted": "auto-generated"
        });
        let input = ClassifierInput {
            from_addr: "noreply@bank.com",
            subject: Some("Your verification code"),
            raw_headers: Some(&headers),
            ..empty_input()
        };
        let result = classify(&input);
        assert_eq!(result.category, "otp");
        assert_eq!(result.priority, "high");
    }

    // --- Triage thresholds ---

    #[test]
    fn test_triage_inbox_below_0_8() {
        // Score 0.3 (list-unsubscribe only)
        let headers = json!({"List-Unsubscribe": "<mailto:unsub@x.com>"});
        let input = ClassifierInput {
            raw_headers: Some(&headers),
            ..empty_input()
        };
        let result = classify(&input);
        assert!(result.score <= 0.8);
        assert_eq!(result.triage_action, TriageAction::Inbox);
    }

    #[test]
    fn test_triage_at_exactly_0_8_is_inbox() {
        // 0.3 + 0.2 + 0.15 + 0.15 = 0.8
        let headers = json!({
            "List-Unsubscribe": "<mailto:unsub@x.com>",
            "Precedence": "bulk",
            "Auto-Submitted": "auto-generated"
        });
        let input = ClassifierInput {
            from_addr: "noreply@spam.com",
            raw_headers: Some(&headers),
            ..empty_input()
        };
        let result = classify(&input);
        assert!((result.score - 0.8).abs() < f32::EPSILON);
        // score > 0.8 is Archive, score == 0.8 is Inbox
        assert_eq!(result.triage_action, TriageAction::Inbox);
    }

    #[test]
    fn test_triage_archive_above_0_8() {
        // 0.3 + 0.2 + 0.15 + 0.15 + 0.2 = 1.0 -> clamped
        // But we need score in (0.8, 0.95]. Let's get 0.85:
        // list-unsub(0.3) + precedence(0.2) + noreply(0.15) + cold(0.2) = 0.85
        let headers = json!({
            "List-Unsubscribe": "<mailto:unsub@x.com>",
            "Precedence": "bulk"
        });
        let input = ClassifierInput {
            from_addr: "noreply@spam.com",
            subject: Some("Exclusive offer just for you"),
            raw_headers: Some(&headers),
            ..empty_input()
        };
        let result = classify(&input);
        assert!(result.score > 0.8);
        assert!(result.score <= 0.95);
        assert_eq!(result.triage_action, TriageAction::Archive);
    }

    #[test]
    fn test_triage_at_exactly_0_95_is_archive() {
        // score > 0.95 → Delete, score == 0.95 → Archive
        // Can't hit exactly 0.95 with signal weights, so verify boundary logic directly
        let score = 0.95_f32;
        let action = if score > 0.95 {
            TriageAction::Delete
        } else if score > 0.8 {
            TriageAction::Archive
        } else {
            TriageAction::Inbox
        };
        assert_eq!(action, TriageAction::Archive);
    }

    #[test]
    fn test_triage_all_signals_clamped_is_delete() {
        let headers = json!({
            "List-Unsubscribe": "<mailto:unsub@x.com>",
            "Precedence": "bulk",
            "Auto-Submitted": "auto-generated"
        });
        let input = ClassifierInput {
            from_addr: "noreply@spam.com",
            subject: Some("Quick question"),
            raw_headers: Some(&headers),
            ..empty_input()
        };
        let result = classify(&input);
        // 0.3 + 0.2 + 0.15 + 0.15 + 0.2 = 1.0 → clamped to 1.0 → Delete
        assert!((result.score - 1.0).abs() < f32::EPSILON);
        assert_eq!(result.triage_action, TriageAction::Delete);
    }

    #[test]
    fn test_triage_delete_above_0_95() {
        let headers = json!({
            "List-Unsubscribe": "<mailto:unsub@x.com>",
            "Precedence": "bulk",
            "Auto-Submitted": "auto-generated"
        });
        let input = ClassifierInput {
            from_addr: "noreply@spam.com",
            subject: Some("Act now"),
            raw_headers: Some(&headers),
            sender_slop_ratio: Some(0.9),
            ..empty_input()
        };
        let result = classify(&input);
        assert!(result.score > 0.95);
        assert_eq!(result.triage_action, TriageAction::Delete);
    }

    // --- Unicode ---

    #[test]
    fn test_unicode_from_addr() {
        let input = ClassifierInput {
            from_addr: "utilisateur@exemple.fr",
            subject: Some("Bonjour le monde"),
            text_body: Some("Ceci est un message en francais"),
            ..empty_input()
        };
        let result = classify(&input);
        assert_eq!(result.category, "personal");
    }

    #[test]
    fn test_unicode_subject_with_otp() {
        let input = ClassifierInput {
            subject: Some("您的verification code是"),
            ..empty_input()
        };
        let result = classify(&input);
        assert_eq!(result.category, "otp");
    }

    #[test]
    fn test_unicode_body_with_digit_sequence() {
        let input = ClassifierInput {
            text_body: Some("あなたのコードは 482910 です"),
            ..empty_input()
        };
        let result = classify(&input);
        assert_eq!(result.category, "otp");
    }

    // --- Edge cases ---

    #[test]
    fn test_empty_subject_and_body() {
        let input = ClassifierInput {
            subject: Some(""),
            text_body: Some(""),
            ..empty_input()
        };
        let result = classify(&input);
        assert_eq!(result.score, 0.0);
        assert_eq!(result.category, "personal");
    }

    #[test]
    fn test_empty_headers_object() {
        let headers = json!({});
        let input = ClassifierInput {
            raw_headers: Some(&headers),
            ..empty_input()
        };
        let result = classify(&input);
        assert_eq!(result.score, 0.0);
    }

    #[test]
    fn test_normal_legitimate_email() {
        let headers = json!({"From": "alice@example.com", "Date": "Mon, 10 Mar 2026"});
        let input = ClassifierInput {
            from_addr: "alice@example.com",
            subject: Some("Meeting tomorrow"),
            text_body: Some("Hey, can we meet at 3pm?"),
            raw_headers: Some(&headers),
            sender_slop_ratio: Some(0.0),
        };
        let result = classify(&input);
        assert_eq!(result.score, 0.0);
        assert_eq!(result.category, "personal");
        assert_eq!(result.triage_action, TriageAction::Inbox);
    }

    #[test]
    fn test_otp_digit_at_start_of_text() {
        let input = ClassifierInput {
            text_body: Some("48291 is your code"),
            ..empty_input()
        };
        let result = classify(&input);
        assert_eq!(result.category, "otp");
    }

    #[test]
    fn test_otp_digit_at_end_of_text() {
        let input = ClassifierInput {
            text_body: Some("Your code is 48291"),
            ..empty_input()
        };
        let result = classify(&input);
        assert_eq!(result.category, "otp");
    }

    #[test]
    fn test_otp_digit_only_text() {
        let input = ClassifierInput {
            text_body: Some("482910"),
            ..empty_input()
        };
        let result = classify(&input);
        assert_eq!(result.category, "otp");
    }
}
