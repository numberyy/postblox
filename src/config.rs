//! Daemon configuration loaded from `postblox.toml`.
//!
//! Kept intentionally minimal: only the sections the daemon needs at
//! the current phase. Add fields as features land.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub secrets: Option<SecretsConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretsConfig {
    /// Passphrase used by the file-backed `SecretStore`. Required when
    /// using the file backend; the OS keyring backend (R5) ignores it.
    pub passphrase: String,
    /// Optional override for where the encrypted secrets file lives.
    /// Defaults to `<data_dir>/secrets.bin`.
    pub path: Option<PathBuf>,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("toml: {0}")]
    Toml(#[from] toml::de::Error),
}

/// Load config from `path`. A missing file is treated as an empty
/// config so first-run users don't have to create one.
pub fn load(path: &Path) -> Result<Config, ConfigError> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(toml::from_str(&s)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(ConfigError::Io(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn missing_file_returns_default() {
        let dir = TempDir::new().unwrap();
        let cfg = load(&dir.path().join("nope.toml")).unwrap();
        assert!(cfg.secrets.is_none());
    }

    #[test]
    fn empty_file_returns_default() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("postblox.toml");
        std::fs::write(&p, "").unwrap();
        let cfg = load(&p).unwrap();
        assert!(cfg.secrets.is_none());
    }

    #[test]
    fn parses_secrets_passphrase() {
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
        let s = cfg.secrets.unwrap();
        assert_eq!(s.passphrase, "hunter2");
        assert!(s.path.is_none());
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
        assert_eq!(
            cfg.secrets.unwrap().path.unwrap(),
            PathBuf::from("/var/lib/postblox/secrets.bin")
        );
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
