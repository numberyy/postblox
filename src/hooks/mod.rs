use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
pub struct HookConfig {
    pub event: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_timeout() -> u64 {
    10
}

#[derive(Debug, Serialize)]
pub struct HookInput {
    pub event: String,
    pub data: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct HookOutput {
    pub action: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum HookError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("hook timed out after {0}s")]
    Timeout(u64),
    #[error("invalid json from hook: {0}")]
    Json(#[from] serde_json::Error),
    #[error("blocked: {0}")]
    Blocked(String),
}

pub async fn run_one(
    hook: &HookConfig,
    event: &str,
    data: &serde_json::Value,
) -> Result<HookOutput, HookError> {
    let input = HookInput {
        event: event.to_string(),
        data: data.clone(),
    };
    let input_json = serde_json::to_vec(&input)?;

    let mut child = tokio::process::Command::new(&hook.command)
        .args(&hook.args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin.write_all(&input_json).await?;
    }

    let timeout = std::time::Duration::from_secs(hook.timeout_secs);
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| HookError::Timeout(hook.timeout_secs))??;

    if !output.stderr.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::debug!(command = %hook.command, %stderr, "hook stderr");
    }

    let hook_output: HookOutput = serde_json::from_slice(&output.stdout)?;
    Ok(hook_output)
}

pub fn run_event_hooks(hooks: &[HookConfig], event_name: &str, data: serde_json::Value) {
    let matching: Vec<HookConfig> = hooks
        .iter()
        .filter(|h| h.event == event_name)
        .cloned()
        .collect();

    if matching.is_empty() {
        return;
    }

    let data = std::sync::Arc::new(data);
    for hook in matching {
        let name = event_name.to_string();
        let payload = std::sync::Arc::clone(&data);
        tokio::spawn(async move {
            if let Err(e) = run_one(&hook, &name, &payload).await {
                tracing::warn!(command = %hook.command, event = %name, "event hook failed: {e}");
            }
        });
    }
}

async fn run_gate_hooks(
    hooks: &[HookConfig],
    event: &str,
    data: &serde_json::Value,
) -> Result<(), HookError> {
    for hook in hooks.iter().filter(|h| h.event == event) {
        match run_one(hook, event, data).await {
            Ok(output) => {
                if output.action.as_deref() == Some("block") {
                    let reason = output.reason.unwrap_or_else(|| "blocked by hook".into());
                    return Err(HookError::Blocked(reason));
                }
            }
            Err(e) => {
                let reason = format!("hook {} failed (fail-closed): {e}", hook.command);
                return Err(HookError::Blocked(reason));
            }
        }
    }
    Ok(())
}

pub async fn run_before_send_hooks(
    hooks: &[HookConfig],
    data: &serde_json::Value,
) -> Result<(), HookError> {
    run_gate_hooks(hooks, "before_send", data).await
}

pub async fn run_before_receive_hooks(
    hooks: &[HookConfig],
    data: &serde_json::Value,
) -> Result<(), HookError> {
    run_gate_hooks(hooks, "before_receive", data).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hook_config_deserialize() {
        let toml_str = r#"
            event = "message.received"
            command = "/usr/bin/echo"
            args = ["hello"]
            timeout_secs = 5
        "#;
        let config: HookConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.event, "message.received");
        assert_eq!(config.command, "/usr/bin/echo");
        assert_eq!(config.args, vec!["hello"]);
        assert_eq!(config.timeout_secs, 5);
    }

    #[test]
    fn test_hook_config_defaults() {
        let toml_str = r#"
            event = "message.sent"
            command = "notify"
        "#;
        let config: HookConfig = toml::from_str(toml_str).unwrap();
        assert!(config.args.is_empty());
        assert_eq!(config.timeout_secs, 10);
    }

    #[test]
    fn test_hook_input_serialization() {
        let input = HookInput {
            event: "message.received".into(),
            data: serde_json::json!({"message_id": "abc"}),
        };
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(json["event"], "message.received");
        assert_eq!(json["data"]["message_id"], "abc");
    }

    #[test]
    fn test_hook_output_deserialize_full() {
        let json = r#"{"action": "block", "reason": "spam detected"}"#;
        let output: HookOutput = serde_json::from_str(json).unwrap();
        assert_eq!(output.action.as_deref(), Some("block"));
        assert_eq!(output.reason.as_deref(), Some("spam detected"));
    }

    #[test]
    fn test_hook_output_deserialize_empty() {
        let json = r#"{}"#;
        let output: HookOutput = serde_json::from_str(json).unwrap();
        assert!(output.action.is_none());
        assert!(output.reason.is_none());
    }

    #[test]
    fn test_hook_output_deserialize_partial() {
        let json = r#"{"action": "allow"}"#;
        let output: HookOutput = serde_json::from_str(json).unwrap();
        assert_eq!(output.action.as_deref(), Some("allow"));
        assert!(output.reason.is_none());
    }

    #[tokio::test]
    async fn test_run_one_with_echo() {
        let hook = HookConfig {
            event: "test".into(),
            command: "cat".into(),
            args: vec![],
            timeout_secs: 5,
        };
        let data = serde_json::json!({"key": "value"});
        let result = run_one(&hook, "test", &data).await;
        // cat echoes HookInput JSON back — serde ignores unknown fields, action/reason stay None
        let output = result.unwrap();
        assert!(output.action.is_none());
    }

    #[tokio::test]
    async fn test_run_one_timeout() {
        let hook = HookConfig {
            event: "test".into(),
            command: "sleep".into(),
            args: vec!["60".into()],
            timeout_secs: 1,
        };
        let data = serde_json::json!({});
        let result = run_one(&hook, "test", &data).await;
        assert!(matches!(result, Err(HookError::Timeout(1))));
    }

    #[tokio::test]
    async fn test_run_before_send_hooks_no_matching() {
        let hooks = vec![HookConfig {
            event: "message.received".into(),
            command: "echo".into(),
            args: vec![],
            timeout_secs: 5,
        }];
        let data = serde_json::json!({"to": ["a@b.com"]});
        let result = run_before_send_hooks(&hooks, &data).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_run_before_send_hooks_fail_closed_on_bad_command() {
        let hooks = vec![HookConfig {
            event: "before_send".into(),
            command: "/nonexistent/command".into(),
            args: vec![],
            timeout_secs: 5,
        }];
        let data = serde_json::json!({"to": ["a@b.com"]});
        let result = run_before_send_hooks(&hooks, &data).await;
        assert!(matches!(result, Err(HookError::Blocked(_))));
    }

    #[tokio::test]
    async fn test_run_one_invalid_json_returns_error() {
        let hook = HookConfig {
            event: "test".into(),
            command: "bash".into(),
            args: vec!["-c".into(), "cat > /dev/null; echo 'not valid json'".into()],
            timeout_secs: 5,
        };
        let data = serde_json::json!({});
        let result = run_one(&hook, "test", &data).await;
        assert!(matches!(result, Err(HookError::Json(_))));
    }

    #[tokio::test]
    async fn test_run_before_send_fail_closed_on_invalid_json() {
        let hooks = vec![HookConfig {
            event: "before_send".into(),
            command: "bash".into(),
            args: vec!["-c".into(), "cat > /dev/null; echo 'garbage output'".into()],
            timeout_secs: 5,
        }];
        let data = serde_json::json!({"to": ["a@b.com"]});
        let result = run_before_send_hooks(&hooks, &data).await;
        assert!(
            matches!(result, Err(HookError::Blocked(ref msg)) if msg.contains("fail-closed")),
            "invalid JSON from hook should block (fail-closed), got: {result:?}"
        );
    }

    #[tokio::test]
    async fn test_run_before_send_block_action() {
        let hooks = vec![HookConfig {
            event: "before_send".into(),
            command: "bash".into(),
            args: vec![
                "-c".into(),
                r#"cat > /dev/null; echo '{"action":"block","reason":"no way"}'"#.into(),
            ],
            timeout_secs: 5,
        }];
        let data = serde_json::json!({"to": ["a@b.com"]});
        let result = run_before_send_hooks(&hooks, &data).await;
        assert!(
            matches!(result, Err(HookError::Blocked(ref msg)) if msg.contains("no way")),
            "got: {result:?}"
        );
    }

    #[tokio::test]
    async fn test_run_before_send_allow_action_passes() {
        let hooks = vec![HookConfig {
            event: "before_send".into(),
            command: "bash".into(),
            args: vec![
                "-c".into(),
                r#"cat > /dev/null; echo '{"action":"allow"}'"#.into(),
            ],
            timeout_secs: 5,
        }];
        let data = serde_json::json!({"to": ["a@b.com"]});
        let result = run_before_send_hooks(&hooks, &data).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_run_before_send_empty_json_passes() {
        let hooks = vec![HookConfig {
            event: "before_send".into(),
            command: "bash".into(),
            args: vec!["-c".into(), "cat > /dev/null; echo '{}'".into()],
            timeout_secs: 5,
        }];
        let data = serde_json::json!({"to": ["a@b.com"]});
        let result = run_before_send_hooks(&hooks, &data).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_run_before_send_multiple_hooks_first_blocks() {
        let hooks = vec![
            HookConfig {
                event: "before_send".into(),
                command: "bash".into(),
                args: vec![
                    "-c".into(),
                    r#"cat > /dev/null; echo '{"action":"block","reason":"first hook"}'"#.into(),
                ],
                timeout_secs: 5,
            },
            HookConfig {
                event: "before_send".into(),
                command: "bash".into(),
                args: vec![
                    "-c".into(),
                    r#"cat > /dev/null; echo '{"action":"allow"}'"#.into(),
                ],
                timeout_secs: 5,
            },
        ];
        let data = serde_json::json!({"to": ["a@b.com"]});
        let result = run_before_send_hooks(&hooks, &data).await;
        assert!(matches!(result, Err(HookError::Blocked(ref msg)) if msg.contains("first hook")));
    }

    #[test]
    fn test_run_event_hooks_no_matching_does_not_panic() {
        let hooks = vec![HookConfig {
            event: "message.sent".into(),
            command: "echo".into(),
            args: vec![],
            timeout_secs: 5,
        }];
        run_event_hooks(&hooks, "message.received", serde_json::json!({}));
    }

    #[tokio::test]
    async fn test_run_before_receive_no_matching() {
        let hooks = vec![HookConfig {
            event: "message.received".into(),
            command: "echo".into(),
            args: vec![],
            timeout_secs: 5,
        }];
        let data = serde_json::json!({"from": "a@b.com"});
        let result = run_before_receive_hooks(&hooks, &data).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_run_before_receive_allow_passes() {
        let hooks = vec![HookConfig {
            event: "before_receive".into(),
            command: "bash".into(),
            args: vec![
                "-c".into(),
                r#"cat > /dev/null; echo '{"action":"allow"}'"#.into(),
            ],
            timeout_secs: 5,
        }];
        let data = serde_json::json!({"from": "a@b.com"});
        let result = run_before_receive_hooks(&hooks, &data).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_run_before_receive_block() {
        let hooks = vec![HookConfig {
            event: "before_receive".into(),
            command: "bash".into(),
            args: vec![
                "-c".into(),
                r#"cat > /dev/null; echo '{"action":"block","reason":"virus detected"}'"#.into(),
            ],
            timeout_secs: 5,
        }];
        let data = serde_json::json!({"from": "a@b.com"});
        let result = run_before_receive_hooks(&hooks, &data).await;
        assert!(
            matches!(result, Err(HookError::Blocked(ref msg)) if msg.contains("virus detected")),
            "got: {result:?}"
        );
    }

    #[tokio::test]
    async fn test_run_before_receive_fail_closed_on_bad_command() {
        let hooks = vec![HookConfig {
            event: "before_receive".into(),
            command: "/nonexistent/scanner".into(),
            args: vec![],
            timeout_secs: 5,
        }];
        let data = serde_json::json!({"from": "a@b.com"});
        let result = run_before_receive_hooks(&hooks, &data).await;
        assert!(matches!(result, Err(HookError::Blocked(_))));
    }

    #[tokio::test]
    async fn test_run_before_receive_fail_closed_on_invalid_json() {
        let hooks = vec![HookConfig {
            event: "before_receive".into(),
            command: "bash".into(),
            args: vec!["-c".into(), "cat > /dev/null; echo 'not json'".into()],
            timeout_secs: 5,
        }];
        let data = serde_json::json!({"from": "a@b.com"});
        let result = run_before_receive_hooks(&hooks, &data).await;
        assert!(
            matches!(result, Err(HookError::Blocked(ref msg)) if msg.contains("fail-closed")),
            "got: {result:?}"
        );
    }

    #[tokio::test]
    async fn test_run_before_receive_multiple_hooks_first_blocks() {
        let hooks = vec![
            HookConfig {
                event: "before_receive".into(),
                command: "bash".into(),
                args: vec![
                    "-c".into(),
                    r#"cat > /dev/null; echo '{"action":"block","reason":"scanner 1 blocked"}'"#
                        .into(),
                ],
                timeout_secs: 5,
            },
            HookConfig {
                event: "before_receive".into(),
                command: "bash".into(),
                args: vec![
                    "-c".into(),
                    r#"cat > /dev/null; echo '{"action":"allow"}'"#.into(),
                ],
                timeout_secs: 5,
            },
        ];
        let data = serde_json::json!({"from": "a@b.com"});
        let result = run_before_receive_hooks(&hooks, &data).await;
        assert!(
            matches!(result, Err(HookError::Blocked(ref msg)) if msg.contains("scanner 1 blocked")),
            "got: {result:?}"
        );
    }

    #[tokio::test]
    async fn test_run_before_receive_empty_json_passes() {
        let hooks = vec![HookConfig {
            event: "before_receive".into(),
            command: "bash".into(),
            args: vec!["-c".into(), "cat > /dev/null; echo '{}'".into()],
            timeout_secs: 5,
        }];
        let data = serde_json::json!({"from": "a@b.com"});
        let result = run_before_receive_hooks(&hooks, &data).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_run_before_receive_timeout() {
        let hooks = vec![HookConfig {
            event: "before_receive".into(),
            command: "sleep".into(),
            args: vec!["60".into()],
            timeout_secs: 1,
        }];
        let data = serde_json::json!({"from": "a@b.com"});
        let result = run_before_receive_hooks(&hooks, &data).await;
        assert!(
            matches!(result, Err(HookError::Blocked(ref msg)) if msg.contains("fail-closed")),
            "timeout should result in fail-closed block, got: {result:?}"
        );
    }
}
