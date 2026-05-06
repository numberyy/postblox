//! Secret storage for account credentials.
//!
//! Behind a `SecretStore` trait so the daemon can swap backends:
//! - file-backed AES-256-GCM (this PR — see [`file::FileSecretStore`]),
//! - OS keyring (R5),
//! - Bitwarden (R7+).

pub mod file;

use async_trait::async_trait;
use thiserror::Error;
use uuid::Uuid;
use zeroize::Zeroizing;

/// A password held in memory. Wraps `Zeroizing<String>` so the buffer
/// is wiped on drop.
pub type Secret = Zeroizing<String>;

#[derive(Debug, Error)]
pub enum SecretError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("crypto: {0}")]
    Crypto(String),

    #[error("decode: {0}")]
    Decode(String),

    #[error("backend: {0}")]
    Backend(String),
}

/// Storage for per-account secrets. Implementations must serialise
/// concurrent writes — callers may call `put` from multiple tasks.
#[async_trait]
pub trait SecretStore: Send + Sync {
    async fn put(&self, account_id: Uuid, secret: Secret) -> Result<(), SecretError>;
    async fn get(&self, account_id: Uuid) -> Result<Option<Secret>, SecretError>;
    async fn delete(&self, account_id: Uuid) -> Result<(), SecretError>;
}

/// Fallback used when no backend is configured. Every operation
/// returns a `Backend("not configured")` error; this surfaces
/// misconfiguration through the IPC layer instead of letting it
/// silently appear to succeed.
#[derive(Debug, Default)]
pub struct UnconfiguredSecretStore;

#[async_trait]
impl SecretStore for UnconfiguredSecretStore {
    async fn put(&self, _: Uuid, _: Secret) -> Result<(), SecretError> {
        Err(SecretError::Backend(
            "secrets backend not configured".into(),
        ))
    }
    async fn get(&self, _: Uuid) -> Result<Option<Secret>, SecretError> {
        Err(SecretError::Backend(
            "secrets backend not configured".into(),
        ))
    }
    async fn delete(&self, _: Uuid) -> Result<(), SecretError> {
        Err(SecretError::Backend(
            "secrets backend not configured".into(),
        ))
    }
}
