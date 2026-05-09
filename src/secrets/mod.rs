//! Secret storage for account credentials.
//!
//! Behind a `SecretStore` trait so the daemon can swap backends:
//! - file-backed AES-256-GCM (this PR — see [`file::FileSecretStore`]),
//! - OS keyring (R5),
//! - Bitwarden (R7+).

pub mod file;
pub mod keyring;

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

    #[error("keyring: {0}")]
    Keyring(#[from] keyring::KeyringSecretError),
}

/// Storage for per-account secrets. Implementations must serialise
/// concurrent writes — callers may call `put` from multiple tasks.
#[async_trait]
pub trait SecretStore: Send + Sync {
    /// Persist `secret` for `account_id`, overwriting any prior value.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`SecretError::Io`] if the backend cannot read/write its
    ///   underlying storage (file backend) or its temp file.
    /// - [`SecretError::Crypto`] if the file backend cannot derive the
    ///   key or AEAD-encrypt the payload.
    /// - [`SecretError::Decode`] if the file backend cannot serialise
    ///   the in-memory map.
    /// - [`SecretError::Keyring`] if the OS keyring rejects the write
    ///   (no storage, platform error, attribute too long, ambiguous
    ///   entry, etc.).
    /// - [`SecretError::Backend`] if the configured backend is
    ///   unavailable (e.g. `UnconfiguredSecretStore`).
    async fn put(&self, account_id: Uuid, secret: Secret) -> Result<(), SecretError>;

    /// Read the secret for `account_id`. Missing entries return
    /// `Ok(None)`.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`SecretError::Io`] if the backend cannot read its underlying
    ///   storage (file backend).
    /// - [`SecretError::Crypto`] if the file backend cannot derive the
    ///   key or AEAD-decrypt the payload (wrong passphrase, tampered
    ///   ciphertext).
    /// - [`SecretError::Decode`] if the file backend payload is
    ///   structurally invalid (truncated header, bad version byte,
    ///   non-UTF-8 plaintext).
    /// - [`SecretError::Keyring`] if the OS keyring backend reports a
    ///   platform failure or bad encoding.
    /// - [`SecretError::Backend`] if the configured backend is
    ///   unavailable (e.g. `UnconfiguredSecretStore`).
    async fn get(&self, account_id: Uuid) -> Result<Option<Secret>, SecretError>;

    /// Remove the secret for `account_id`. A missing entry is not an
    /// error.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`SecretError::Io`] if the backend cannot read/write its
    ///   underlying storage (file backend).
    /// - [`SecretError::Crypto`] / [`SecretError::Decode`] if the file
    ///   backend cannot decrypt or parse the existing map before
    ///   removing the entry.
    /// - [`SecretError::Keyring`] if the OS keyring rejects the delete
    ///   (platform errors; `NoEntry` is treated as success).
    /// - [`SecretError::Backend`] if the configured backend is
    ///   unavailable (e.g. `UnconfiguredSecretStore`).
    async fn delete(&self, account_id: Uuid) -> Result<(), SecretError>;

    fn secret_ref(&self, account_id: Uuid) -> String {
        account_secret_ref(account_id)
    }
}

pub fn account_secret_ref(account_id: Uuid) -> String {
    format!("account:{account_id}")
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
