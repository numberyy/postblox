use sqlx::PgPool;
use uuid::Uuid;

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
            "ntfy" => {
                let url = match nc.config.get("url").and_then(|v| v.as_str()) {
                    Some(u) => u.to_string(),
                    None => {
                        tracing::warn!(config_id = %nc.id, "ntfy config missing 'url'");
                        continue;
                    }
                };
                let client = client.clone();
                let title = title.to_string();
                let body = body.to_string();
                tokio::spawn(async move {
                    if let Err(e) = client
                        .post(&url)
                        .header("Title", &title)
                        .body(body)
                        .send()
                        .await
                    {
                        tracing::error!("ntfy dispatch failed: {e}");
                    }
                });
            }
            "webhook" => {
                let url = match nc.config.get("url").and_then(|v| v.as_str()) {
                    Some(u) => u.to_string(),
                    None => {
                        tracing::warn!(config_id = %nc.id, "webhook notification config missing 'url'");
                        continue;
                    }
                };
                let client = client.clone();
                let title = title.to_string();
                let body = body.to_string();
                tokio::spawn(async move {
                    if let Err(e) = client
                        .post(&url)
                        .json(&serde_json::json!({"title": title, "body": body}))
                        .send()
                        .await
                    {
                        tracing::error!("webhook notification dispatch failed: {e}");
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
