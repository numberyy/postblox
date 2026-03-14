use sqlx::PgPool;
use uuid::Uuid;

/// Dispatches notifications to all active configs for the given org.
/// Spawns tasks for each notification — errors are logged, not propagated.
pub async fn notify_org(
    pool: &PgPool,
    org_id: Uuid,
    title: &str,
    body: &str,
    client: &reqwest::Client,
) {
    let configs = match crate::db::notifications::list_active(pool, org_id).await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("failed to fetch notification configs: {e}");
            return;
        }
    };

    for nc in configs {
        match nc.provider.as_str() {
            "ntfy" | "webhook" => {
                let url = match nc.config.get("url").and_then(|v| v.as_str()) {
                    Some(u) => u.to_string(),
                    None => {
                        tracing::warn!(config_id = %nc.id, provider = nc.provider, "config missing 'url'");
                        continue;
                    }
                };
                let provider = nc.provider.clone();
                let client = client.clone();
                let title = title.to_string();
                let body = body.to_string();
                tokio::spawn(async move {
                    let req = if provider == "ntfy" {
                        client.post(&url).header("Title", &title).body(body)
                    } else {
                        client
                            .post(&url)
                            .json(&serde_json::json!({"title": title, "body": body}))
                    };
                    match req.send().await {
                        Err(e) => tracing::error!("{provider} dispatch failed: {e}"),
                        Ok(resp) if !resp.status().is_success() => {
                            tracing::error!("{provider} returned {}", resp.status());
                        }
                        _ => {}
                    }
                });
            }
            "desktop" => {
                let title = title.to_string();
                let body = body.to_string();
                tokio::spawn(async move {
                    match tokio::process::Command::new("notify-send")
                        .arg(&title)
                        .arg(&body)
                        .output()
                        .await
                    {
                        Ok(out) if !out.status.success() => {
                            tracing::warn!("notify-send exited with {}", out.status);
                        }
                        Err(e) => {
                            tracing::warn!(
                                "desktop notification failed (notify-send not available?): {e}"
                            );
                        }
                        _ => {}
                    }
                });
            }
            "email" => {
                tracing::debug!(config_id = %nc.id, "email notification provider not yet implemented");
            }
            other => {
                tracing::warn!(config_id = %nc.id, provider = other, "unknown notification provider");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{CreateNotificationConfig, NotificationProvider};

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_notify_org_no_configs_is_noop() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Notify Noop Org")
            .await
            .unwrap();
        let client = reqwest::Client::new();
        // Should return without panicking
        notify_org(&pool, org.id, "Test", "Body", &client).await;
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_notify_org_webhook_missing_url_skips() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Notify Missing URL Org")
            .await
            .unwrap();
        // Webhook config without "url" key
        crate::db::notifications::create(
            &pool,
            &CreateNotificationConfig {
                org_id: org.id,
                provider: NotificationProvider::Webhook,
                config: serde_json::json!({"channel": "#alerts"}),
            },
        )
        .await
        .unwrap();
        let client = reqwest::Client::new();
        // Should log warning but not panic
        notify_org(&pool, org.id, "Test", "Body", &client).await;
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_notify_org_email_provider_is_noop() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Notify Email Noop Org")
            .await
            .unwrap();
        crate::db::notifications::create(
            &pool,
            &CreateNotificationConfig {
                org_id: org.id,
                provider: NotificationProvider::Email,
                config: serde_json::json!({"to": "admin@example.com"}),
            },
        )
        .await
        .unwrap();
        let client = reqwest::Client::new();
        // Email is not yet implemented — should log debug and continue
        notify_org(&pool, org.id, "Test", "Body", &client).await;
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_notify_org_nonexistent_org_is_noop() {
        let pool = crate::db::test_pool().await;
        let client = reqwest::Client::new();
        // Random org ID with no configs — should return cleanly
        notify_org(&pool, Uuid::new_v4(), "Test", "Body", &client).await;
    }
}
