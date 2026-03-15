use serde::Deserialize;
use std::collections::HashMap;
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
    download_dir: Option<String>,
    #[serde(default)]
    keybindings: RawKeybindings,
}

#[derive(Debug, Deserialize, Default)]
struct RawKeybindings {
    quit: Option<String>,
    compose: Option<String>,
    reply: Option<String>,
    search: Option<String>,
    refresh: Option<String>,
    approve: Option<String>,
    reject: Option<String>,
    slop_toggle: Option<String>,
    help: Option<String>,
    briefing: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TuiConfig {
    pub server_url: String,
    pub api_key: String,
    pub theme: String,
    pub vim_mode: bool,
    pub download_dir: PathBuf,
    pub keybindings: KeybindingOverrides,
}

#[derive(Debug, Clone, Default)]
pub struct KeybindingOverrides(pub HashMap<String, char>);

impl KeybindingOverrides {
    fn from_raw(raw: RawKeybindings) -> Self {
        let mut map = HashMap::new();
        let pairs = [
            ("quit", raw.quit),
            ("compose", raw.compose),
            ("reply", raw.reply),
            ("search", raw.search),
            ("refresh", raw.refresh),
            ("approve", raw.approve),
            ("reject", raw.reject),
            ("slop_toggle", raw.slop_toggle),
            ("help", raw.help),
            ("briefing", raw.briefing),
        ];
        for (action, val) in pairs {
            if let Some(s) = val {
                if let Some(c) = s.chars().next() {
                    if s.chars().count() == 1 {
                        map.insert(action.to_string(), c);
                    }
                }
            }
        }
        Self(map)
    }

    #[cfg(test)]
    pub fn get(&self, action: &str) -> Option<char> {
        self.0.get(action).copied()
    }
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

        let download_dir = raw
            .tui
            .download_dir
            .map(PathBuf::from)
            .unwrap_or_else(default_download_dir);

        let keybindings = KeybindingOverrides::from_raw(raw.tui.keybindings);

        Ok(Self {
            server_url,
            api_key,
            theme: raw.tui.theme.unwrap_or_else(|| "nord".into()),
            vim_mode: raw.tui.vim_mode.unwrap_or(true),
            download_dir,
            keybindings,
        })
    }

    pub fn config_path() -> PathBuf {
        dirs_next().join("tui.toml")
    }
}

fn default_download_dir() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join("Downloads")
    } else {
        PathBuf::from(".")
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

    #[test]
    fn test_download_dir_default() {
        let raw: RawConfig = toml::from_str(
            r#"
[server]
url = "http://localhost:3000"
api_key = "k"
"#,
        )
        .unwrap();
        assert!(raw.tui.download_dir.is_none());
        let dir = raw
            .tui
            .download_dir
            .map(PathBuf::from)
            .unwrap_or_else(default_download_dir);
        // Should end with "Downloads" (unless HOME is not set)
        let dir_str = dir.display().to_string();
        assert!(
            dir_str.ends_with("Downloads") || dir_str == ".",
            "got: {dir_str}"
        );
    }

    #[test]
    fn test_download_dir_custom() {
        let raw: RawConfig = toml::from_str(
            r#"
[server]
url = "http://localhost:3000"
api_key = "k"

[tui]
download_dir = "/tmp/my-downloads"
"#,
        )
        .unwrap();
        assert_eq!(raw.tui.download_dir.unwrap(), "/tmp/my-downloads");
    }

    #[test]
    fn test_keybindings_default_empty() {
        let raw: RawConfig = toml::from_str(
            r#"
[server]
url = "http://localhost:3000"
api_key = "k"
"#,
        )
        .unwrap();
        let kb = KeybindingOverrides::from_raw(raw.tui.keybindings);
        assert!(kb.0.is_empty());
    }

    #[test]
    fn test_keybindings_custom_overrides() {
        let raw: RawConfig = toml::from_str(
            r#"
[server]
url = "http://localhost:3000"
api_key = "k"

[tui.keybindings]
quit = "x"
compose = "n"
refresh = "R"
"#,
        )
        .unwrap();
        let kb = KeybindingOverrides::from_raw(raw.tui.keybindings);
        assert_eq!(kb.get("quit"), Some('x'));
        assert_eq!(kb.get("compose"), Some('n'));
        assert_eq!(kb.get("refresh"), Some('R'));
        assert_eq!(kb.get("search"), None);
    }

    #[test]
    fn test_keybindings_ignores_multichar() {
        let raw: RawConfig = toml::from_str(
            r#"
[server]
url = "http://localhost:3000"
api_key = "k"

[tui.keybindings]
quit = "qq"
compose = "c"
"#,
        )
        .unwrap();
        let kb = KeybindingOverrides::from_raw(raw.tui.keybindings);
        assert_eq!(kb.get("quit"), None);
        assert_eq!(kb.get("compose"), Some('c'));
    }

    #[test]
    fn test_keybindings_all_actions() {
        let raw: RawConfig = toml::from_str(
            r#"
[server]
url = "http://localhost:3000"
api_key = "k"

[tui.keybindings]
quit = "Q"
compose = "C"
reply = "R"
search = "S"
refresh = "F"
approve = "A"
reject = "N"
slop_toggle = "T"
help = "H"
briefing = "B"
"#,
        )
        .unwrap();
        let kb = KeybindingOverrides::from_raw(raw.tui.keybindings);
        assert_eq!(kb.0.len(), 10);
        assert_eq!(kb.get("quit"), Some('Q'));
        assert_eq!(kb.get("briefing"), Some('B'));
    }
}
