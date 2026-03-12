use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Rule {
    DomainAllowlist {
        domains: Vec<String>,
    },
    DomainBlocklist {
        domains: Vec<String>,
    },
    TimeWindow {
        start_hour: u8,
        end_hour: u8,
        timezone: String,
    },
    KeywordBlocklist {
        keywords: Vec<String>,
    },
    SlopThreshold {
        threshold: f64,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum RuleVerdict {
    Allow,
    Block { rule: String, reason: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleSet(pub Vec<Rule>);

impl RuleSet {
    pub fn evaluate(
        &self,
        to: &[String],
        subject: &str,
        text_body: &str,
        slop_score: Option<f64>,
    ) -> RuleVerdict {
        self.evaluate_at(to, subject, text_body, slop_score, chrono::Utc::now())
    }

    fn evaluate_at(
        &self,
        to: &[String],
        subject: &str,
        text_body: &str,
        slop_score: Option<f64>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> RuleVerdict {
        for rule in &self.0 {
            match rule {
                Rule::DomainAllowlist { domains } => {
                    if domains.is_empty() {
                        continue;
                    }
                    for addr in to {
                        let domain = addr.rsplit('@').next().unwrap_or("");
                        if !domains.iter().any(|d| d.eq_ignore_ascii_case(domain)) {
                            return RuleVerdict::Block {
                                rule: "domain_allowlist".into(),
                                reason: format!(
                                    "domain '{}' not in allowlist",
                                    domain.to_lowercase()
                                ),
                            };
                        }
                    }
                }
                Rule::DomainBlocklist { domains } => {
                    for addr in to {
                        let domain = addr.rsplit('@').next().unwrap_or("");
                        if domains.iter().any(|d| d.eq_ignore_ascii_case(domain)) {
                            return RuleVerdict::Block {
                                rule: "domain_blocklist".into(),
                                reason: format!("domain '{}' is blocked", domain.to_lowercase()),
                            };
                        }
                    }
                }
                Rule::TimeWindow {
                    start_hour,
                    end_hour,
                    timezone,
                } => {
                    if let Ok(tz) = timezone.parse::<chrono_tz::Tz>() {
                        use chrono::Timelike;
                        let local = now.with_timezone(&tz);
                        let hour = local.hour() as u8;
                        let in_window = if start_hour <= end_hour {
                            hour >= *start_hour && hour < *end_hour
                        } else {
                            hour >= *start_hour || hour < *end_hour
                        };
                        if !in_window {
                            return RuleVerdict::Block {
                                rule: "time_window".into(),
                                reason: format!(
                                    "current hour {hour} outside window {start_hour}-{end_hour} {timezone}"
                                ),
                            };
                        }
                    }
                }
                Rule::KeywordBlocklist { keywords } => {
                    if !keywords.is_empty() {
                        let subject_lower = subject.to_lowercase();
                        let body_lower = text_body.to_lowercase();
                        for kw in keywords {
                            let kw_lower = kw.to_lowercase();
                            if subject_lower.contains(&kw_lower) || body_lower.contains(&kw_lower) {
                                return RuleVerdict::Block {
                                    rule: "keyword_blocklist".into(),
                                    reason: format!("keyword '{kw}' found in message"),
                                };
                            }
                        }
                    }
                }
                Rule::SlopThreshold { threshold } => {
                    if let Some(score) = slop_score {
                        if score > *threshold {
                            return RuleVerdict::Block {
                                rule: "slop_threshold".into(),
                                reason: format!(
                                    "slop score {score:.2} exceeds threshold {threshold:.2}"
                                ),
                            };
                        }
                    }
                }
            }
        }
        RuleVerdict::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn addr(s: &str) -> String {
        s.to_string()
    }

    // === Empty ruleset ===

    #[test]
    fn test_empty_ruleset_allows_all() {
        let rs = RuleSet(vec![]);
        assert_eq!(
            rs.evaluate(&[addr("a@b.com")], "hi", "body", None),
            RuleVerdict::Allow,
        );
    }

    // === DomainAllowlist ===

    #[test]
    fn test_domain_allowlist_allows_matching() {
        let rs = RuleSet(vec![Rule::DomainAllowlist {
            domains: vec!["example.com".into()],
        }]);
        assert_eq!(
            rs.evaluate(&[addr("user@example.com")], "", "", None),
            RuleVerdict::Allow,
        );
    }

    #[test]
    fn test_domain_allowlist_blocks_non_matching() {
        let rs = RuleSet(vec![Rule::DomainAllowlist {
            domains: vec!["example.com".into()],
        }]);
        let v = rs.evaluate(&[addr("user@evil.com")], "", "", None);
        assert!(matches!(v, RuleVerdict::Block { rule, .. } if rule == "domain_allowlist"));
    }

    #[test]
    fn test_domain_allowlist_all_recipients_must_match() {
        let rs = RuleSet(vec![Rule::DomainAllowlist {
            domains: vec!["ok.com".into()],
        }]);
        let v = rs.evaluate(&[addr("a@ok.com"), addr("b@bad.com")], "", "", None);
        assert!(matches!(v, RuleVerdict::Block { .. }));
    }

    #[test]
    fn test_domain_allowlist_case_insensitive() {
        let rs = RuleSet(vec![Rule::DomainAllowlist {
            domains: vec!["Example.COM".into()],
        }]);
        assert_eq!(
            rs.evaluate(&[addr("user@example.com")], "", "", None),
            RuleVerdict::Allow,
        );
    }

    #[test]
    fn test_domain_allowlist_empty_domains_allows_all() {
        let rs = RuleSet(vec![Rule::DomainAllowlist { domains: vec![] }]);
        assert_eq!(
            rs.evaluate(&[addr("user@any.com")], "", "", None),
            RuleVerdict::Allow,
        );
    }

    // === DomainBlocklist ===

    #[test]
    fn test_domain_blocklist_blocks_matching() {
        let rs = RuleSet(vec![Rule::DomainBlocklist {
            domains: vec!["spam.com".into()],
        }]);
        let v = rs.evaluate(&[addr("user@spam.com")], "", "", None);
        assert!(matches!(v, RuleVerdict::Block { rule, .. } if rule == "domain_blocklist"));
    }

    #[test]
    fn test_domain_blocklist_allows_non_matching() {
        let rs = RuleSet(vec![Rule::DomainBlocklist {
            domains: vec!["spam.com".into()],
        }]);
        assert_eq!(
            rs.evaluate(&[addr("user@good.com")], "", "", None),
            RuleVerdict::Allow,
        );
    }

    #[test]
    fn test_domain_blocklist_any_recipient_triggers() {
        let rs = RuleSet(vec![Rule::DomainBlocklist {
            domains: vec!["bad.com".into()],
        }]);
        let v = rs.evaluate(&[addr("a@good.com"), addr("b@bad.com")], "", "", None);
        assert!(matches!(v, RuleVerdict::Block { .. }));
    }

    #[test]
    fn test_domain_blocklist_case_insensitive() {
        let rs = RuleSet(vec![Rule::DomainBlocklist {
            domains: vec!["BAD.COM".into()],
        }]);
        let v = rs.evaluate(&[addr("user@bad.com")], "", "", None);
        assert!(matches!(v, RuleVerdict::Block { .. }));
    }

    // === TimeWindow ===

    #[test]
    fn test_time_window_allows_within() {
        let rs = RuleSet(vec![Rule::TimeWindow {
            start_hour: 9,
            end_hour: 17,
            timezone: "UTC".into(),
        }]);
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
        assert_eq!(
            rs.evaluate_at(&[addr("a@b.com")], "", "", None, now),
            RuleVerdict::Allow,
        );
    }

    #[test]
    fn test_time_window_blocks_outside() {
        let rs = RuleSet(vec![Rule::TimeWindow {
            start_hour: 9,
            end_hour: 17,
            timezone: "UTC".into(),
        }]);
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 20, 0, 0).unwrap();
        let v = rs.evaluate_at(&[addr("a@b.com")], "", "", None, now);
        assert!(matches!(v, RuleVerdict::Block { rule, .. } if rule == "time_window"));
    }

    #[test]
    fn test_time_window_boundary_start_is_inclusive() {
        let rs = RuleSet(vec![Rule::TimeWindow {
            start_hour: 9,
            end_hour: 17,
            timezone: "UTC".into(),
        }]);
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 9, 0, 0).unwrap();
        assert_eq!(
            rs.evaluate_at(&[addr("a@b.com")], "", "", None, now),
            RuleVerdict::Allow,
        );
    }

    #[test]
    fn test_time_window_boundary_end_is_exclusive() {
        let rs = RuleSet(vec![Rule::TimeWindow {
            start_hour: 9,
            end_hour: 17,
            timezone: "UTC".into(),
        }]);
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 17, 0, 0).unwrap();
        let v = rs.evaluate_at(&[addr("a@b.com")], "", "", None, now);
        assert!(matches!(v, RuleVerdict::Block { .. }));
    }

    #[test]
    fn test_time_window_wraps_midnight() {
        let rs = RuleSet(vec![Rule::TimeWindow {
            start_hour: 22,
            end_hour: 6,
            timezone: "UTC".into(),
        }]);
        // 23:00 is within 22-6 window
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 23, 0, 0).unwrap();
        assert_eq!(
            rs.evaluate_at(&[addr("a@b.com")], "", "", None, now),
            RuleVerdict::Allow,
        );
        // 3:00 is within 22-6 window
        let now = Utc.with_ymd_and_hms(2026, 1, 2, 3, 0, 0).unwrap();
        assert_eq!(
            rs.evaluate_at(&[addr("a@b.com")], "", "", None, now),
            RuleVerdict::Allow,
        );
        // 12:00 is outside 22-6 window
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
        let v = rs.evaluate_at(&[addr("a@b.com")], "", "", None, now);
        assert!(matches!(v, RuleVerdict::Block { .. }));
    }

    #[test]
    fn test_time_window_respects_timezone() {
        let rs = RuleSet(vec![Rule::TimeWindow {
            start_hour: 9,
            end_hour: 17,
            timezone: "US/Eastern".into(),
        }]);
        // 14:00 UTC = 09:00 EST (in window)
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 14, 0, 0).unwrap();
        assert_eq!(
            rs.evaluate_at(&[addr("a@b.com")], "", "", None, now),
            RuleVerdict::Allow,
        );
        // 13:00 UTC = 08:00 EST (outside window)
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 13, 0, 0).unwrap();
        let v = rs.evaluate_at(&[addr("a@b.com")], "", "", None, now);
        assert!(matches!(v, RuleVerdict::Block { .. }));
    }

    #[test]
    fn test_time_window_invalid_timezone_skips() {
        let rs = RuleSet(vec![Rule::TimeWindow {
            start_hour: 9,
            end_hour: 17,
            timezone: "Fake/Timezone".into(),
        }]);
        assert_eq!(
            rs.evaluate(&[addr("a@b.com")], "", "", None),
            RuleVerdict::Allow,
        );
    }

    // === KeywordBlocklist ===

    #[test]
    fn test_keyword_blocklist_blocks_in_subject() {
        let rs = RuleSet(vec![Rule::KeywordBlocklist {
            keywords: vec!["casino".into()],
        }]);
        let v = rs.evaluate(&[addr("a@b.com")], "Win at the CASINO", "", None);
        assert!(matches!(v, RuleVerdict::Block { rule, .. } if rule == "keyword_blocklist"));
    }

    #[test]
    fn test_keyword_blocklist_blocks_in_body() {
        let rs = RuleSet(vec![Rule::KeywordBlocklist {
            keywords: vec!["viagra".into()],
        }]);
        let v = rs.evaluate(&[addr("a@b.com")], "Hello", "buy VIAGRA now", None);
        assert!(matches!(v, RuleVerdict::Block { .. }));
    }

    #[test]
    fn test_keyword_blocklist_allows_when_absent() {
        let rs = RuleSet(vec![Rule::KeywordBlocklist {
            keywords: vec!["casino".into()],
        }]);
        assert_eq!(
            rs.evaluate(&[addr("a@b.com")], "Hello", "world", None),
            RuleVerdict::Allow,
        );
    }

    #[test]
    fn test_keyword_blocklist_case_insensitive() {
        let rs = RuleSet(vec![Rule::KeywordBlocklist {
            keywords: vec!["SPAM".into()],
        }]);
        let v = rs.evaluate(&[addr("a@b.com")], "spam email", "", None);
        assert!(matches!(v, RuleVerdict::Block { .. }));
    }

    #[test]
    fn test_keyword_blocklist_empty_keywords_allows() {
        let rs = RuleSet(vec![Rule::KeywordBlocklist { keywords: vec![] }]);
        assert_eq!(
            rs.evaluate(&[addr("a@b.com")], "anything", "goes", None),
            RuleVerdict::Allow,
        );
    }

    // === SlopThreshold ===

    #[test]
    fn test_slop_threshold_blocks_above() {
        let rs = RuleSet(vec![Rule::SlopThreshold { threshold: 0.7 }]);
        let v = rs.evaluate(&[addr("a@b.com")], "", "", Some(0.9));
        assert!(matches!(v, RuleVerdict::Block { rule, .. } if rule == "slop_threshold"));
    }

    #[test]
    fn test_slop_threshold_allows_below() {
        let rs = RuleSet(vec![Rule::SlopThreshold { threshold: 0.7 }]);
        assert_eq!(
            rs.evaluate(&[addr("a@b.com")], "", "", Some(0.5)),
            RuleVerdict::Allow,
        );
    }

    #[test]
    fn test_slop_threshold_allows_at_exactly_threshold() {
        let rs = RuleSet(vec![Rule::SlopThreshold { threshold: 0.7 }]);
        assert_eq!(
            rs.evaluate(&[addr("a@b.com")], "", "", Some(0.7)),
            RuleVerdict::Allow,
        );
    }

    #[test]
    fn test_slop_threshold_no_score_allows() {
        let rs = RuleSet(vec![Rule::SlopThreshold { threshold: 0.7 }]);
        assert_eq!(
            rs.evaluate(&[addr("a@b.com")], "", "", None),
            RuleVerdict::Allow,
        );
    }

    // === First-match semantics ===

    #[test]
    fn test_first_rule_that_blocks_wins() {
        let rs = RuleSet(vec![
            Rule::DomainBlocklist {
                domains: vec!["evil.com".into()],
            },
            Rule::KeywordBlocklist {
                keywords: vec!["spam".into()],
            },
        ]);
        let v = rs.evaluate(&[addr("a@evil.com")], "spam", "", None);
        assert!(matches!(v, RuleVerdict::Block { rule, .. } if rule == "domain_blocklist"));
    }

    #[test]
    fn test_later_rule_blocks_if_earlier_passes() {
        let rs = RuleSet(vec![
            Rule::DomainBlocklist {
                domains: vec!["evil.com".into()],
            },
            Rule::KeywordBlocklist {
                keywords: vec!["spam".into()],
            },
        ]);
        let v = rs.evaluate(&[addr("a@good.com")], "spam here", "", None);
        assert!(matches!(v, RuleVerdict::Block { rule, .. } if rule == "keyword_blocklist"));
    }

    // === Serde roundtrip ===

    #[test]
    fn test_rule_serde_roundtrip() {
        let rules = vec![
            Rule::DomainAllowlist {
                domains: vec!["example.com".into()],
            },
            Rule::DomainBlocklist {
                domains: vec!["evil.com".into()],
            },
            Rule::TimeWindow {
                start_hour: 9,
                end_hour: 17,
                timezone: "UTC".into(),
            },
            Rule::KeywordBlocklist {
                keywords: vec!["spam".into()],
            },
            Rule::SlopThreshold { threshold: 0.8 },
        ];
        let json = serde_json::to_string(&rules).unwrap();
        let back: Vec<Rule> = serde_json::from_str(&json).unwrap();
        assert_eq!(rules, back);
    }

    #[test]
    fn test_ruleset_serde_roundtrip() {
        let rs = RuleSet(vec![Rule::SlopThreshold { threshold: 0.5 }]);
        let json = serde_json::to_string(&rs).unwrap();
        let back: RuleSet = serde_json::from_str(&json).unwrap();
        assert_eq!(rs, back);
    }

    #[test]
    fn test_rule_deserialize_tagged_format() {
        let json = r#"{"type":"domain_allowlist","domains":["a.com"]}"#;
        let rule: Rule = serde_json::from_str(json).unwrap();
        assert_eq!(
            rule,
            Rule::DomainAllowlist {
                domains: vec!["a.com".into()]
            }
        );
    }

    #[test]
    fn test_rule_deserialize_invalid_type_fails() {
        let json = r#"{"type":"unknown","foo":"bar"}"#;
        let result = serde_json::from_str::<Rule>(json);
        assert!(result.is_err());
    }

    // === Edge cases ===

    #[test]
    fn test_no_recipients_with_allowlist_allows() {
        let rs = RuleSet(vec![Rule::DomainAllowlist {
            domains: vec!["ok.com".into()],
        }]);
        assert_eq!(rs.evaluate(&[], "", "", None), RuleVerdict::Allow);
    }

    #[test]
    fn test_address_without_at_sign_domain_is_empty() {
        let rs = RuleSet(vec![Rule::DomainAllowlist {
            domains: vec!["ok.com".into()],
        }]);
        let v = rs.evaluate(&[addr("noemail")], "", "", None);
        assert!(matches!(v, RuleVerdict::Block { .. }));
    }

    #[test]
    fn test_multiple_rules_all_pass() {
        let rs = RuleSet(vec![
            Rule::DomainAllowlist {
                domains: vec!["ok.com".into()],
            },
            Rule::KeywordBlocklist {
                keywords: vec!["spam".into()],
            },
            Rule::SlopThreshold { threshold: 0.9 },
        ]);
        assert_eq!(
            rs.evaluate(&[addr("a@ok.com")], "hello", "world", Some(0.5)),
            RuleVerdict::Allow,
        );
    }
}
