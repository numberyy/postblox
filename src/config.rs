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
    pub stalwart_smtp_host: Option<String>,
    pub stalwart_smtp_port: Option<u16>,
    pub relay_host: Option<String>,
    pub relay_port: Option<u16>,
    pub relay_username: Option<String>,
    pub relay_password: Option<String>,
    #[serde(default)]
    pub relay_starttls: bool,
    pub guard_patterns: Option<Vec<GuardPatternConfig>>,
    pub embedding_url: Option<String>,
    pub embedding_model: Option<String>,
    pub embedding_api_key: Option<String>,
    pub embedding_dimension: Option<usize>,
    #[serde(default = "default_trust_threshold")]
    pub trust_auto_upgrade_threshold: i32,
    pub hooks: Option<Vec<crate::hooks::HookConfig>>,
    #[serde(default)]
    pub rate_limit: RateLimitConfig,
    #[serde(default = "default_attachment_storage_path")]
    pub attachment_storage_path: PathBuf,
    #[serde(default = "default_max_attachment_size")]
    pub max_attachment_size_bytes: i64,
    #[serde(default)]
    pub content_filter: ContentFilterConfig,
}

#[derive(Debug, Deserialize, Default)]
pub struct ContentFilterConfig {
    pub allowed_types: Option<Vec<String>>,
    pub blocked_types: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct RateLimitConfig {
    #[serde(default = "default_rate_per_minute")]
    pub requests_per_minute: u32,
    #[serde(default = "default_rate_per_hour")]
    pub requests_per_hour: u32,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            requests_per_minute: default_rate_per_minute(),
            requests_per_hour: default_rate_per_hour(),
        }
    }
}

fn default_rate_per_minute() -> u32 {
    60
}

fn default_rate_per_hour() -> u32 {
    1000
}

#[derive(Debug, Deserialize)]
pub struct GuardPatternConfig {
    pub name: String,
    pub pattern: String,
}

fn default_trust_threshold() -> i32 {
    10
}

fn default_attachment_storage_path() -> PathBuf {
    PathBuf::from("data/attachments")
}

fn default_max_attachment_size() -> i64 {
    25 * 1024 * 1024
}

fn default_host() -> String {
    "0.0.0.0".into()
}

fn default_port() -> u16 {
    3000
}

fn parse_env_or_default<T: std::str::FromStr>(name: &str, default_fn: fn() -> T) -> T {
    parse_env_opt(name).unwrap_or_else(default_fn)
}

fn parse_env_opt<T: std::str::FromStr>(name: &str) -> Option<T> {
    let v = std::env::var(name).ok()?;
    match v.parse() {
        Ok(parsed) => Some(parsed),
        Err(_) => {
            tracing::warn!("{name}={v:?} is not valid, ignoring");
            None
        }
    }
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
                port: parse_env_or_default("POSTBLOX_PORT", default_port),
                database_url: std::env::var("DATABASE_URL")
                    .map_err(|_| anyhow::anyhow!("DATABASE_URL or postblox.toml required"))?,
                stalwart_url: std::env::var("STALWART_URL").ok(),
                stalwart_admin_user: std::env::var("STALWART_ADMIN_USER").ok(),
                stalwart_admin_token: std::env::var("STALWART_ADMIN_TOKEN").ok(),
                stalwart_inbound_token: std::env::var("STALWART_INBOUND_TOKEN").ok(),
                stalwart_smtp_host: std::env::var("STALWART_SMTP_HOST").ok(),
                stalwart_smtp_port: parse_env_opt("STALWART_SMTP_PORT"),
                relay_host: std::env::var("RELAY_HOST").ok(),
                relay_port: parse_env_opt("RELAY_PORT"),
                relay_username: std::env::var("RELAY_USERNAME").ok(),
                relay_password: std::env::var("RELAY_PASSWORD").ok(),
                relay_starttls: parse_env_or_default("RELAY_STARTTLS", || false),
                guard_patterns: None,
                embedding_url: std::env::var("EMBEDDING_URL").ok(),
                embedding_model: std::env::var("EMBEDDING_MODEL").ok(),
                embedding_api_key: std::env::var("EMBEDDING_API_KEY").ok(),
                embedding_dimension: parse_env_opt("EMBEDDING_DIMENSION"),
                hooks: None,
                rate_limit: RateLimitConfig {
                    requests_per_minute: parse_env_or_default(
                        "RATE_LIMIT_PER_MINUTE",
                        default_rate_per_minute,
                    ),
                    requests_per_hour: parse_env_or_default(
                        "RATE_LIMIT_PER_HOUR",
                        default_rate_per_hour,
                    ),
                },
                trust_auto_upgrade_threshold: parse_env_or_default(
                    "TRUST_AUTO_UPGRADE_THRESHOLD",
                    default_trust_threshold,
                ),
                attachment_storage_path: std::env::var("ATTACHMENT_STORAGE_PATH")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| default_attachment_storage_path()),
                max_attachment_size_bytes: parse_env_or_default(
                    "MAX_ATTACHMENT_SIZE_BYTES",
                    default_max_attachment_size,
                ),
                content_filter: ContentFilterConfig::default(),
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

    #[test]
    fn test_config_rate_limit_defaults() {
        let toml_str = r#"database_url = "postgres://localhost/postblox""#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.rate_limit.requests_per_minute, 60);
        assert_eq!(config.rate_limit.requests_per_hour, 1000);
    }

    #[test]
    fn test_config_rate_limit_custom() {
        let toml_str = r#"
            database_url = "postgres://localhost/postblox"

            [rate_limit]
            requests_per_minute = 120
            requests_per_hour = 5000
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.rate_limit.requests_per_minute, 120);
        assert_eq!(config.rate_limit.requests_per_hour, 5000);
    }

    #[test]
    fn test_config_rate_limit_partial_override() {
        let toml_str = r#"
            database_url = "postgres://localhost/postblox"

            [rate_limit]
            requests_per_minute = 200
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.rate_limit.requests_per_minute, 200);
        assert_eq!(config.rate_limit.requests_per_hour, 1000);
    }

    #[test]
    fn test_config_attachment_defaults() {
        let toml_str = r#"database_url = "postgres://localhost/postblox""#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.attachment_storage_path,
            PathBuf::from("data/attachments")
        );
        assert_eq!(config.max_attachment_size_bytes, 25 * 1024 * 1024);
    }

    #[test]
    fn test_config_relay_defaults() {
        let toml_str = r#"database_url = "postgres://localhost/postblox""#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.relay_host.is_none());
        assert!(config.relay_port.is_none());
        assert!(config.relay_username.is_none());
        assert!(config.relay_password.is_none());
        assert!(!config.relay_starttls);
    }

    #[test]
    fn test_config_relay_custom() {
        let toml_str = r#"
            database_url = "postgres://localhost/postblox"
            relay_host = "smtp.mailgun.org"
            relay_port = 587
            relay_username = "postmaster@mg.example.com"
            relay_password = "secret"
            relay_starttls = true
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.relay_host.as_deref(), Some("smtp.mailgun.org"));
        assert_eq!(config.relay_port, Some(587));
        assert_eq!(
            config.relay_username.as_deref(),
            Some("postmaster@mg.example.com")
        );
        assert_eq!(config.relay_password.as_deref(), Some("secret"));
        assert!(config.relay_starttls);
    }

    #[test]
    fn test_config_attachment_custom() {
        let toml_str = r#"
            database_url = "postgres://localhost/postblox"
            attachment_storage_path = "/var/data/attachments"
            max_attachment_size_bytes = 10485760
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.attachment_storage_path,
            PathBuf::from("/var/data/attachments")
        );
        assert_eq!(config.max_attachment_size_bytes, 10_485_760);
    }

    #[test]
    fn test_config_content_filter_default() {
        let toml_str = r#"database_url = "postgres://localhost/postblox""#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.content_filter.allowed_types.is_none());
        assert!(config.content_filter.blocked_types.is_none());
    }

    #[test]
    fn test_config_content_filter_allowlist() {
        let toml_str = r#"
            database_url = "postgres://localhost/postblox"

            [content_filter]
            allowed_types = ["image/*", "application/pdf"]
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let allowed = config.content_filter.allowed_types.unwrap();
        assert_eq!(allowed, vec!["image/*", "application/pdf"]);
        assert!(config.content_filter.blocked_types.is_none());
    }

    #[test]
    fn test_config_content_filter_blocklist() {
        let toml_str = r#"
            database_url = "postgres://localhost/postblox"

            [content_filter]
            blocked_types = ["application/x-executable", "application/x-shellscript"]
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.content_filter.allowed_types.is_none());
        let blocked = config.content_filter.blocked_types.unwrap();
        assert_eq!(
            blocked,
            vec!["application/x-executable", "application/x-shellscript"]
        );
    }

    #[test]
    fn test_config_content_filter_both() {
        let toml_str = r#"
            database_url = "postgres://localhost/postblox"

            [content_filter]
            allowed_types = ["image/*", "application/pdf"]
            blocked_types = ["image/svg+xml"]
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.content_filter.allowed_types.is_some());
        assert!(config.content_filter.blocked_types.is_some());
    }
}
