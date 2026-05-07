//! Daemon configuration loaded from `postblox.toml`.
//!
//! Kept intentionally minimal: only the sections the daemon needs at
//! the current phase. Add fields as features land.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

#[derive(Clone, Default, PartialEq, Eq)]
pub struct Config {
    pub secrets: SecretsConfig,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("secrets", &self.secrets)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SecretsConfig {
    pub backend: SecretsBackend,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretsBackend {
    Keyring,
    File,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    secrets: Option<RawSecretsConfig>,
}

impl fmt::Debug for RawConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawConfig")
            .field("secrets", &self.secrets)
            .finish()
    }
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

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("toml: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("invalid config: {0}")]
    Invalid(String),
}

/// Load config from `path`. A missing file is treated as an empty
/// config so first-run users don't have to create one.
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
    let Some(secrets) = raw.secrets else {
        return Ok(Config::default());
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
    })
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
    }

    #[test]
    fn empty_file_returns_default() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("postblox.toml");
        std::fs::write(&p, "").unwrap();
        let cfg = load(&p).unwrap();
        assert_eq!(cfg.secrets.backend, SecretsBackend::Keyring);
        assert!(cfg.secrets.passphrase.is_none());
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
