//! Daemon configuration loaded from `postblox.toml`.
//!
//! Kept intentionally minimal: only the sections the daemon needs at
//! the current phase. Add fields as features land.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::tui::theme::ThemeName;

/// Daemon configuration loaded from `postblox.toml`.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Config {
    /// Secret-storage configuration.
    pub secrets: SecretsConfig,
    /// TUI-specific configuration.
    pub tui: TuiConfig,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("secrets", &self.secrets)
            .field("tui", &self.tui)
            .finish()
    }
}

/// TUI-specific configuration block.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct TuiConfig {
    /// Override the startup theme. `None` means the TUI uses
    /// `ThemeName::default()`.
    pub theme: Option<ThemeName>,
}

/// Secret-storage configuration block.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretsConfig {
    /// Storage backend used by the daemon.
    pub backend: SecretsBackend,
    /// Passphrase required when `backend = "file"`.
    pub passphrase: Option<String>,
    /// Optional override for where the encrypted secrets file lives.
    /// Defaults to `<data_dir>/secrets.bin`.
    pub path: Option<PathBuf>,
}

impl Default for SecretsConfig {
    fn default() -> Self {
        Self {
            backend: SecretsBackend::Keyring,
            passphrase: None,
            path: None,
        }
    }
}

impl fmt::Debug for SecretsConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretsConfig")
            .field("backend", &self.backend)
            .field("passphrase", &redacted(&self.passphrase))
            .field("path", &self.path)
            .finish()
    }
}

/// Backend selector for [`SecretsConfig::backend`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretsBackend {
    /// Use the OS keyring backend.
    Keyring,
    /// Use the encrypted-file backend.
    File,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    secrets: Option<RawSecretsConfig>,
    #[serde(default)]
    tui: Option<RawTuiConfig>,
}

impl fmt::Debug for RawConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawConfig")
            .field("secrets", &self.secrets)
            .field("tui", &self.tui)
            .finish()
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTuiConfig {
    theme: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSecretsConfig {
    backend: Option<SecretsBackend>,
    passphrase: Option<String>,
    path: Option<PathBuf>,
}

impl fmt::Debug for RawSecretsConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawSecretsConfig")
            .field("backend", &self.backend)
            .field("passphrase", &redacted(&self.passphrase))
            .field("path", &self.path)
            .finish()
    }
}

/// Error returned by [`load`].
#[derive(Debug, Error)]
pub enum ConfigError {
    /// Filesystem I/O failed while reading the config file.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// File contents are not valid TOML or use unknown fields.
    #[error("toml: {0}")]
    Toml(#[from] toml::de::Error),

    /// Parsed config is internally inconsistent.
    #[error("invalid config: {0}")]
    Invalid(String),
}

/// Load config from `path`. A missing file is treated as an empty
/// config so first-run users don't have to create one.
///
/// # Errors
///
/// Returns:
/// - [`ConfigError::Io`] if the file exists but cannot be read.
/// - [`ConfigError::Toml`] if the file is not valid TOML or contains
///   unknown fields (the schema uses `deny_unknown_fields`).
/// - [`ConfigError::Invalid`] if the parsed config is internally
///   inconsistent — for example `[secrets] backend = "file"` without a
///   passphrase, a `path` set without `backend = "file"`, or an
///   unknown TUI theme name.
pub fn load(path: &Path) -> Result<Config, ConfigError> {
    match std::fs::read_to_string(path) {
        Ok(s) => parse(&s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(ConfigError::Io(e)),
    }
}

fn redacted(secret: &Option<String>) -> Option<&'static str> {
    secret.as_ref().map(|_| "<redacted>")
}

fn parse(input: &str) -> Result<Config, ConfigError> {
    let raw: RawConfig = toml::from_str(input)?;
    normalize(raw)
}

fn normalize(raw: RawConfig) -> Result<Config, ConfigError> {
    let tui = normalize_tui(raw.tui)?;

    let Some(secrets) = raw.secrets else {
        return Ok(Config {
            tui,
            ..Config::default()
        });
    };

    let backend = secrets.backend.unwrap_or_else(|| {
        if secrets.passphrase.is_some() {
            SecretsBackend::File
        } else {
            SecretsBackend::Keyring
        }
    });
    if backend == SecretsBackend::Keyring && secrets.path.is_some() {
        return Err(ConfigError::Invalid(
            "[secrets] path requires backend = \"file\" with passphrase; remove path for keyring"
                .into(),
        ));
    }
    if backend == SecretsBackend::File
        && secrets
            .passphrase
            .as_deref()
            .map_or(true, |passphrase| passphrase.is_empty())
    {
        return Err(ConfigError::Invalid(
            "[secrets] backend = \"file\" requires non-empty passphrase".into(),
        ));
    }

    Ok(Config {
        secrets: SecretsConfig {
            backend,
            passphrase: secrets.passphrase,
            path: secrets.path,
        },
        tui,
    })
}

fn normalize_tui(raw: Option<RawTuiConfig>) -> Result<TuiConfig, ConfigError> {
    let Some(raw) = raw else {
        return Ok(TuiConfig::default());
    };
    let theme = match raw.theme.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(name) => Some(name.parse::<ThemeName>().map_err(|err| {
            ConfigError::Invalid(format!(
                "[tui] {err}; valid: light, dark, high-contrast (or hc)"
            ))
        })?),
    };
    Ok(TuiConfig { theme })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn missing_file_returns_default() {
        let dir = TempDir::new().unwrap();
        let cfg = load(&dir.path().join("nope.toml")).unwrap();
        assert_eq!(cfg.secrets.backend, SecretsBackend::Keyring);
        assert!(cfg.secrets.passphrase.is_none());
        assert!(cfg.tui.theme.is_none());
    }

    #[test]
    fn empty_file_returns_default() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("postblox.toml");
        std::fs::write(&p, "").unwrap();
        let cfg = load(&p).unwrap();
        assert_eq!(cfg.secrets.backend, SecretsBackend::Keyring);
        assert!(cfg.secrets.passphrase.is_none());
        assert!(cfg.tui.theme.is_none());
    }

    #[test]
    fn parses_tui_theme_dark() {
        let cfg = parse(
            r#"
            [tui]
            theme = "dark"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.tui.theme, Some(ThemeName::Dark));
    }

    #[test]
    fn parses_tui_theme_light() {
        let cfg = parse(
            r#"
            [tui]
            theme = "light"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.tui.theme, Some(ThemeName::Light));
    }

    #[test]
    fn parses_tui_theme_high_contrast_and_hc_alias() {
        let canonical = parse(
            r#"
            [tui]
            theme = "high-contrast"
            "#,
        )
        .unwrap();
        let alias = parse(
            r#"
            [tui]
            theme = "hc"
            "#,
        )
        .unwrap();
        assert_eq!(canonical.tui.theme, Some(ThemeName::HighContrast));
        assert_eq!(alias.tui.theme, Some(ThemeName::HighContrast));
    }

    #[test]
    fn empty_tui_theme_string_falls_back_to_default() {
        let cfg = parse(
            r#"
            [tui]
            theme = ""
            "#,
        )
        .unwrap();
        assert!(cfg.tui.theme.is_none());
    }

    #[test]
    fn unknown_tui_theme_is_an_error() {
        let err = parse(
            r#"
            [tui]
            theme = "wat"
            "#,
        )
        .unwrap_err();
        match err {
            ConfigError::Invalid(message) => {
                assert!(message.contains("unknown theme"), "got: {message}");
                assert!(message.contains("[tui]"), "got: {message}");
            }
            other => panic!("expected invalid config error, got {other:?}"),
        }
    }

    #[test]
    fn missing_tui_section_keeps_default_theme() {
        let cfg = parse(
            r#"
            [secrets]
            backend = "keyring"
            "#,
        )
        .unwrap();
        assert!(cfg.tui.theme.is_none());
    }

    #[test]
    fn unknown_tui_field_is_an_error() {
        let err = parse(
            r#"
            [tui]
            theme = "dark"
            extra = "nope"
            "#,
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::Toml(_)));
    }

    #[test]
    fn legacy_secrets_passphrase_selects_file_backend() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("postblox.toml");
        std::fs::write(
            &p,
            r#"
            [secrets]
            passphrase = "hunter2"
            "#,
        )
        .unwrap();
        let cfg = load(&p).unwrap();
        let s = cfg.secrets;
        assert_eq!(s.backend, SecretsBackend::File);
        assert_eq!(s.passphrase.as_deref(), Some("hunter2"));
        assert!(s.path.is_none());
    }

    #[test]
    fn parses_keyring_backend_without_passphrase() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("postblox.toml");
        std::fs::write(
            &p,
            r#"
            [secrets]
            backend = "keyring"
            "#,
        )
        .unwrap();
        let cfg = load(&p).unwrap();
        assert_eq!(cfg.secrets.backend, SecretsBackend::Keyring);
        assert!(cfg.secrets.passphrase.is_none());
    }

    #[test]
    fn path_without_file_backend_or_passphrase_is_an_error() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("postblox.toml");
        std::fs::write(
            &p,
            r#"
            [secrets]
            path = "/var/lib/postblox/secrets.bin"
            "#,
        )
        .unwrap();
        let err = load(&p).unwrap_err();
        match err {
            ConfigError::Invalid(message) => {
                assert!(message.contains("[secrets] path"));
                assert!(message.contains("backend = \"file\""));
                assert!(message.contains("passphrase"));
            }
            other => panic!("expected invalid config error, got {other:?}"),
        }
    }

    #[test]
    fn keyring_backend_with_path_is_an_error() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("postblox.toml");
        std::fs::write(
            &p,
            r#"
            [secrets]
            backend = "keyring"
            path = "/var/lib/postblox/secrets.bin"
            "#,
        )
        .unwrap();
        let err = load(&p).unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
    }

    #[test]
    fn parses_secrets_with_path_override() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("postblox.toml");
        std::fs::write(
            &p,
            r#"
            [secrets]
            passphrase = "x"
            path = "/var/lib/postblox/secrets.bin"
            "#,
        )
        .unwrap();
        let cfg = load(&p).unwrap();
        assert_eq!(cfg.secrets.backend, SecretsBackend::File);
        assert_eq!(
            cfg.secrets.path.unwrap(),
            PathBuf::from("/var/lib/postblox/secrets.bin")
        );
    }

    #[test]
    fn debug_redacts_file_backend_passphrase() {
        let fake_passphrase = "obvious-fake-passphrase-for-debug-test";
        let input = format!(
            r#"
            [secrets]
            backend = "file"
            passphrase = "{fake_passphrase}"
            path = "/var/lib/postblox/secrets.bin"
            "#
        );
        let raw: RawConfig = toml::from_str(&input).unwrap();
        let raw_debug = format!("{raw:?}");
        assert!(raw_debug.contains("<redacted>"));
        assert!(!raw_debug.contains(fake_passphrase));

        let cfg = normalize(raw).unwrap();
        let secrets_debug = format!("{:?}", cfg.secrets);
        let cfg_debug = format!("{cfg:?}");
        assert!(secrets_debug.contains("<redacted>"));
        assert!(cfg_debug.contains("<redacted>"));
        assert!(!secrets_debug.contains(fake_passphrase));
        assert!(!cfg_debug.contains(fake_passphrase));
    }

    #[test]
    fn file_backend_requires_passphrase() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("postblox.toml");
        std::fs::write(
            &p,
            r#"
            [secrets]
            backend = "file"
            "#,
        )
        .unwrap();
        let err = load(&p).unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
    }

    #[test]
    fn unknown_field_is_an_error() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("postblox.toml");
        std::fs::write(
            &p,
            r#"
            [secrets]
            passphrase = "x"
            extra = "nope"
            "#,
        )
        .unwrap();
        let err = load(&p).unwrap_err();
        assert!(matches!(err, ConfigError::Toml(_)));
    }
}
