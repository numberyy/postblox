use serde_json::Value;
use uuid::Uuid;

use crate::models::{GateAction, McpGate};

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum GateDecision {
    AutoAllow { gate_id: Option<Uuid> },
    Deny { gate_id: Option<Uuid> },
    Require { gate_id: Option<Uuid> },
}

pub fn decide(gates: &[McpGate], args: &Value) -> GateDecision {
    for gate in gates {
        if gate_matches(gate, args) {
            return match gate.action {
                GateAction::AutoAllow => GateDecision::AutoAllow {
                    gate_id: Some(gate.id),
                },
                GateAction::Deny => GateDecision::Deny {
                    gate_id: Some(gate.id),
                },
                GateAction::Require => GateDecision::Require {
                    gate_id: Some(gate.id),
                },
            };
        }
    }
    GateDecision::Require { gate_id: None }
}

pub fn gate_matches(gate: &McpGate, args: &Value) -> bool {
    let Some(pattern) = &gate.arg_pattern else {
        return true;
    };
    let Ok(pattern) = serde_json::from_str::<Value>(pattern) else {
        return false;
    };
    pattern_matches(&pattern, args)
}

fn pattern_matches(pattern: &Value, args: &Value) -> bool {
    let Some(pattern) = pattern.as_object() else {
        return false;
    };
    let Some(args) = args.as_object() else {
        return false;
    };

    pattern.iter().all(|(field, expected)| {
        args.get(field)
            .is_some_and(|actual| value_matches(expected, actual))
    })
}

fn value_matches(expected: &Value, actual: &Value) -> bool {
    match (expected.as_str(), actual.as_str()) {
        (Some(pattern), Some(actual)) if pattern.contains('*') => wildcard_matches(pattern, actual),
        _ => expected == actual,
    }
}

fn wildcard_matches(pattern: &str, actual: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if pattern.matches('*').count() != 1 {
        return false;
    }
    let Some((prefix, suffix)) = pattern.split_once('*') else {
        return false;
    };
    actual.starts_with(prefix)
        && actual.ends_with(suffix)
        && actual.len() >= prefix.len() + suffix.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;

    fn gate(pattern: Option<&str>, action: GateAction) -> McpGate {
        McpGate {
            id: Uuid::new_v4(),
            tool: "postblox_message_send".into(),
            arg_pattern: pattern.map(str::to_string),
            action,
            note: None,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn test_null_pattern_matches_any_args() {
        assert!(gate_matches(
            &gate(None, GateAction::AutoAllow),
            &json!({"draft_id": "x"})
        ));
    }

    #[test]
    fn test_exact_pattern_matches_top_level_field_values() {
        let gate = gate(
            Some(r#"{"account_id":"00000000-0000-0000-0000-000000000001","urgent":true}"#),
            GateAction::AutoAllow,
        );
        assert!(gate_matches(
            &gate,
            &json!({
                "account_id": "00000000-0000-0000-0000-000000000001",
                "urgent": true,
                "extra": "ignored"
            })
        ));
        assert!(!gate_matches(
            &gate,
            &json!({
                "account_id": "00000000-0000-0000-0000-000000000001",
                "urgent": false
            })
        ));
    }

    #[test]
    fn test_string_pattern_supports_single_star_prefix_suffix_wildcard() {
        let email_gate = gate(Some(r#"{"to":"*@example.com"}"#), GateAction::AutoAllow);
        assert!(gate_matches(
            &email_gate,
            &json!({"to": "alice@example.com"})
        ));
        assert!(!gate_matches(
            &email_gate,
            &json!({"to": "alice@example.net"})
        ));

        let subject_gate = gate(Some(r#"{"subject":"Re:*"}"#), GateAction::AutoAllow);
        assert!(gate_matches(
            &subject_gate,
            &json!({"subject": "Re: hello"})
        ));
        assert!(!gate_matches(
            &subject_gate,
            &json!({"subject": "Fwd: hello"})
        ));
    }

    #[test]
    fn test_invalid_pattern_does_not_match() {
        assert!(!gate_matches(
            &gate(Some("not json"), GateAction::AutoAllow),
            &json!({})
        ));
        assert!(!gate_matches(
            &gate(Some(r#"["not", "object"]"#), GateAction::AutoAllow),
            &json!({})
        ));
    }

    #[test]
    fn test_first_matching_gate_decides_action_and_default_requires() {
        let deny = gate(Some(r#"{"id":"x"}"#), GateAction::Deny);
        let allow = gate(None, GateAction::AutoAllow);
        assert_eq!(
            decide(&[deny.clone(), allow.clone()], &json!({"id": "x"})),
            GateDecision::Deny {
                gate_id: Some(deny.id)
            }
        );
        assert_eq!(
            decide(&[deny, allow.clone()], &json!({"id": "y"})),
            GateDecision::AutoAllow {
                gate_id: Some(allow.id)
            }
        );
        assert_eq!(
            decide(&[], &json!({})),
            GateDecision::Require { gate_id: None }
        );
    }
}
