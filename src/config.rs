use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub database_url: String,
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub stalwart_url: Option<String>,
    pub stalwart_admin_user: Option<String>,
    pub stalwart_admin_token: Option<String>,
    pub stalwart_inbound_token: Option<String>,
    pub guard_patterns: Option<Vec<GuardPatternConfig>>,
    pub embedding_url: Option<String>,
    pub embedding_model: Option<String>,
    pub embedding_api_key: Option<String>,
    pub relay: Option<RelayConfig>,
    #[serde(default = "default_trust_threshold")]
    pub trust_auto_upgrade_threshold: i32,
    pub hooks: Option<Vec<crate::hooks::HookConfig>>,
}

#[allow(dead_code)] // used once relay sending is wired up
#[derive(Debug, Deserialize, Clone)]
pub struct RelayConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    #[serde(default = "default_relay_starttls")]
    pub starttls: bool,
}

fn default_relay_starttls() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub struct GuardPatternConfig {
    pub name: String,
    pub pattern: String,
}

fn default_trust_threshold() -> i32 {
    10
}

fn default_host() -> String {
    "0.0.0.0".into()
}

fn default_port() -> u16 {
    3000
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let path = std::env::var("POSTBLOX_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("postblox.toml"));

        if path.exists() {
            let contents = std::fs::read_to_string(&path)?;
            Ok(toml::from_str(&contents)?)
        } else {
            Ok(Self {
                host: std::env::var("POSTBLOX_HOST").unwrap_or_else(|_| default_host()),
                port: std::env::var("POSTBLOX_PORT")
                    .ok()
                    .and_then(|p| p.parse().ok())
                    .unwrap_or_else(default_port),
                database_url: std::env::var("DATABASE_URL")
                    .map_err(|_| anyhow::anyhow!("DATABASE_URL or postblox.toml required"))?,
                stalwart_url: std::env::var("STALWART_URL").ok(),
                stalwart_admin_user: std::env::var("STALWART_ADMIN_USER").ok(),
                stalwart_admin_token: std::env::var("STALWART_ADMIN_TOKEN").ok(),
                stalwart_inbound_token: std::env::var("STALWART_INBOUND_TOKEN").ok(),
                guard_patterns: None,
                embedding_url: std::env::var("EMBEDDING_URL").ok(),
                embedding_model: std::env::var("EMBEDDING_MODEL").ok(),
                embedding_api_key: std::env::var("EMBEDDING_API_KEY").ok(),
                relay: None,
                hooks: None,
                trust_auto_upgrade_threshold: std::env::var("TRUST_AUTO_UPGRADE_THRESHOLD")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(default_trust_threshold),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_from_toml_with_defaults() {
        let toml_str = r#"database_url = "postgres://localhost/postblox""#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.port, 3000);
        assert_eq!(config.database_url, "postgres://localhost/postblox");
    }

    #[test]
    fn test_config_from_toml_with_overrides() {
        let toml_str = r#"
            database_url = "postgres://user:pass@db:5432/test"
            host = "127.0.0.1"
            port = 8080
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 8080);
    }

    #[test]
    fn test_config_trust_threshold_default() {
        let toml_str = r#"database_url = "postgres://localhost/postblox""#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.trust_auto_upgrade_threshold, 10);
    }

    #[test]
    fn test_config_trust_threshold_custom() {
        let toml_str = r#"
            database_url = "postgres://localhost/postblox"
            trust_auto_upgrade_threshold = 25
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.trust_auto_upgrade_threshold, 25);
    }

    #[test]
    fn test_config_with_hooks() {
        let toml_str = r#"
            database_url = "postgres://localhost/postblox"

            [[hooks]]
            event = "message.received"
            command = "/usr/local/bin/notify"
            args = ["--channel", "email"]
            timeout_secs = 5

            [[hooks]]
            event = "before_send"
            command = "/usr/local/bin/check"
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let hooks = config.hooks.unwrap();
        assert_eq!(hooks.len(), 2);
        assert_eq!(hooks[0].event, "message.received");
        assert_eq!(hooks[0].args, vec!["--channel", "email"]);
        assert_eq!(hooks[0].timeout_secs, 5);
        assert_eq!(hooks[1].event, "before_send");
        assert!(hooks[1].args.is_empty());
        assert_eq!(hooks[1].timeout_secs, 10);
    }

    #[test]
    fn test_config_without_hooks() {
        let toml_str = r#"database_url = "postgres://localhost/postblox""#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.hooks.is_none());
    }
}
