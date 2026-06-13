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
use zeroize::Zeroizing;

use crate::models::AccountId;

mod sealed {
    /// Private supertrait that prevents downstream crates from
    /// implementing [`super::SecretStore`]. New backends (Bitwarden,
    /// etc.) must live in this crate so they can opt in to `Sealed`.
    pub trait Sealed {}
}

/// A password held in memory. Wraps `Zeroizing<String>` so the buffer
/// is wiped on drop.
pub type Secret = Zeroizing<String>;

/// Error returned by [`SecretStore`] operations.
#[derive(Debug, Error)]
pub enum SecretError {
    /// Underlying filesystem I/O failed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// Cryptographic operation failed (key derivation or AEAD).
    #[error("crypto: {0}")]
    Crypto(String),

    /// On-disk payload could not be parsed.
    #[error("decode: {0}")]
    Decode(String),

    /// Backend is unavailable or rejected the call (e.g. unconfigured).
    #[error("backend: {0}")]
    Backend(String),

    /// OS keyring backend reported a platform-level error.
    #[error("keyring: {0}")]
    Keyring(#[from] keyring::KeyringSecretError),
}

/// Storage for per-account secrets. Implementations must serialise
/// concurrent writes — callers may call `put` from multiple tasks.
///
/// This trait is sealed: only backends inside the postblox crate may
/// implement it. Adding a new backend (e.g. Bitwarden, see R7+) means
/// adding a module here and an `impl sealed::Sealed for …` line.
#[async_trait]
pub trait SecretStore: sealed::Sealed + Send + Sync {
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
    ///   unavailable (e.g. an unconfigured backend).
    async fn put(&self, account_id: AccountId, secret: Secret) -> Result<(), SecretError>;

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
    ///   unavailable (e.g. an unconfigured backend).
    async fn get(&self, account_id: AccountId) -> Result<Option<Secret>, SecretError>;

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
    ///   unavailable (e.g. an unconfigured backend).
    async fn delete(&self, account_id: AccountId) -> Result<(), SecretError>;

    /// Stable reference string the daemon stores in `accounts.secret_ref`
    /// for the given account. Defaults to the canonical `account:<uuid>`
    /// form.
    fn secret_ref(&self, account_id: AccountId) -> String {
        format!("account:{account_id}")
    }
}
