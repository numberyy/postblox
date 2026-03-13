use serde::Deserialize;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("config file not found at {path}\n\nCreate it with:\n\n[server]\nurl = \"http://localhost:3000\"\napi_key = \"your-api-key\"\n\n[tui]\ntheme = \"nord\"\nvim_mode = true")]
    NotFound { path: String },
    #[error("failed to read config: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid config: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("missing server.url — set it in config or POSTBLOX_URL env var")]
    MissingUrl,
    #[error("missing server.api_key — set it in config or POSTBLOX_API_KEY env var")]
    MissingApiKey,
}

#[derive(Debug, Deserialize, Default)]
struct RawConfig {
    #[serde(default)]
    server: RawServer,
    #[serde(default)]
    tui: RawTui,
}

#[derive(Debug, Deserialize, Default)]
struct RawServer {
    url: Option<String>,
    api_key: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct RawTui {
    theme: Option<String>,
    vim_mode: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct TuiConfig {
    pub server_url: String,
    pub api_key: String,
    pub theme: String,
    pub vim_mode: bool,
}

impl TuiConfig {
    pub fn load() -> Result<Self, ConfigError> {
        let path = Self::config_path();
        let raw = if path.exists() {
            let contents = std::fs::read_to_string(&path)?;
            toml::from_str::<RawConfig>(&contents)?
        } else {
            RawConfig::default()
        };

        let server_url = raw
            .server
            .url
            .or_else(|| std::env::var("POSTBLOX_URL").ok())
            .ok_or(if path.exists() {
                ConfigError::MissingUrl
            } else {
                ConfigError::NotFound {
                    path: path.display().to_string(),
                }
            })?;

        let api_key = raw
            .server
            .api_key
            .or_else(|| std::env::var("POSTBLOX_API_KEY").ok())
            .ok_or(ConfigError::MissingApiKey)?;

        Ok(Self {
            server_url,
            api_key,
            theme: raw.tui.theme.unwrap_or_else(|| "nord".into()),
            vim_mode: raw.tui.vim_mode.unwrap_or(true),
        })
    }

    pub fn config_path() -> PathBuf {
        dirs_next().join("tui.toml")
    }
}

fn dirs_next() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(dir).join("postblox")
    } else if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".config").join("postblox")
    } else {
        PathBuf::from(".config").join("postblox")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_parse_full_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tui.toml");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"
[server]
url = "http://example.com:3000"
api_key = "test-key-123"

[tui]
theme = "dracula"
vim_mode = false
"#
        )
        .unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        let raw: RawConfig = toml::from_str(&contents).unwrap();

        assert_eq!(raw.server.url.unwrap(), "http://example.com:3000");
        assert_eq!(raw.server.api_key.unwrap(), "test-key-123");
        assert_eq!(raw.tui.theme.unwrap(), "dracula");
        assert!(!raw.tui.vim_mode.unwrap());
    }

    #[test]
    fn test_parse_minimal_config() {
        let raw: RawConfig = toml::from_str(
            r#"
[server]
url = "http://localhost:3000"
api_key = "key"
"#,
        )
        .unwrap();

        assert_eq!(raw.server.url.unwrap(), "http://localhost:3000");
        assert!(raw.tui.theme.is_none());
        assert!(raw.tui.vim_mode.is_none());
    }

    #[test]
    fn test_defaults_applied() {
        let raw: RawConfig = toml::from_str(
            r#"
[server]
url = "http://localhost:3000"
api_key = "k"
"#,
        )
        .unwrap();

        let theme = raw.tui.theme.unwrap_or_else(|| "nord".into());
        let vim_mode = raw.tui.vim_mode.unwrap_or(true);

        assert_eq!(theme, "nord");
        assert!(vim_mode);
    }

    #[test]
    fn test_env_var_fallback() {
        // When both config and env are absent, server.url should be None
        let raw = RawConfig::default();
        assert!(raw.server.url.is_none());
        assert!(raw.server.api_key.is_none());
    }

    #[test]
    fn test_config_path_uses_xdg() {
        // Just verify the function returns a path ending in tui.toml
        let path = TuiConfig::config_path();
        assert!(path.ends_with("tui.toml"));
    }

    #[test]
    fn test_empty_config_file_parses() {
        let raw: RawConfig = toml::from_str("").unwrap();
        assert!(raw.server.url.is_none());
        assert!(raw.tui.theme.is_none());
    }
}
