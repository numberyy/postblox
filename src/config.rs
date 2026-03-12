use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub database_url: String,
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
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
}
