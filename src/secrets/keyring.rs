//! OS keyring-backed [`SecretStore`].

use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::Mutex;
use zeroize::Zeroizing;

use crate::models::AccountId;

use super::{Secret, SecretError, SecretStore};

pub const DEFAULT_SERVICE: &str = "postblox";

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum KeyringSecretError {
    #[error("platform failure: {0}")]
    Platform(String),

    #[error("storage unavailable: {0}")]
    StorageUnavailable(String),

    #[error("entry not found")]
    NotFound,

    #[error("secret was not UTF-8")]
    BadEncoding,

    #[error("attribute '{name}' exceeded platform limit {limit}")]
    TooLong { name: String, limit: u32 },

    #[error("attribute '{name}' is invalid: {reason}")]
    Invalid { name: String, reason: String },

    #[error("multiple matching entries")]
    Ambiguous,

    #[error("keyring task failed: {0}")]
    Task(String),
}

#[derive(Debug, Clone)]
pub struct KeyringSecretStore {
    service: String,
    write_lock: Arc<Mutex<()>>,
}

impl Default for KeyringSecretStore {
    fn default() -> Self {
        Self::new(DEFAULT_SERVICE)
    }
}

impl KeyringSecretStore {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            write_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn service(&self) -> &str {
        &self.service
    }

    pub fn account_key(account_id: AccountId) -> String {
        account_id.to_string()
    }

    pub fn keyring_ref(&self, account_id: AccountId) -> String {
        format!("keyring:{}:{}", self.service, Self::account_key(account_id))
    }

    fn entry(service: &str, account_id: AccountId) -> Result<::keyring::Entry, KeyringSecretError> {
        ::keyring::Entry::new(service, &Self::account_key(account_id)).map_err(map_keyring_error)
    }
}

#[async_trait]
impl SecretStore for KeyringSecretStore {
    async fn put(&self, account_id: AccountId, secret: Secret) -> Result<(), SecretError> {
        let _guard = self.write_lock.lock().await;
        let service = self.service.clone();
        tokio::task::spawn_blocking(move || {
            let entry = Self::entry(&service, account_id)?;
            entry
                .set_password(secret.as_str())
                .map_err(map_keyring_error)
        })
        .await
        .map_err(|e| KeyringSecretError::Task(e.to_string()))??;
        Ok(())
    }

    async fn get(&self, account_id: AccountId) -> Result<Option<Secret>, SecretError> {
        let service = self.service.clone();
        let password = tokio::task::spawn_blocking(move || {
            let entry = Self::entry(&service, account_id)?;
            match entry.get_password() {
                Ok(secret) => Ok(Some(Zeroizing::new(secret))),
                Err(::keyring::Error::NoEntry) => Ok(None),
                Err(err) => Err(map_keyring_error(err)),
            }
        })
        .await
        .map_err(|e| KeyringSecretError::Task(e.to_string()))??;
        Ok(password)
    }

    async fn delete(&self, account_id: AccountId) -> Result<(), SecretError> {
        let _guard = self.write_lock.lock().await;
        let service = self.service.clone();
        tokio::task::spawn_blocking(move || {
            let entry = Self::entry(&service, account_id)?;
            match entry.delete_credential() {
                Ok(()) | Err(::keyring::Error::NoEntry) => Ok(()),
                Err(err) => Err(map_keyring_error(err)),
            }
        })
        .await
        .map_err(|e| KeyringSecretError::Task(e.to_string()))??;
        Ok(())
    }

    fn secret_ref(&self, account_id: AccountId) -> String {
        self.keyring_ref(account_id)
    }
}

pub(crate) fn map_keyring_error(err: ::keyring::Error) -> KeyringSecretError {
    match err {
        ::keyring::Error::PlatformFailure(err) => KeyringSecretError::Platform(err.to_string()),
        ::keyring::Error::NoStorageAccess(err) => {
            KeyringSecretError::StorageUnavailable(err.to_string())
        }
        ::keyring::Error::NoEntry => KeyringSecretError::NotFound,
        ::keyring::Error::BadEncoding(_) => KeyringSecretError::BadEncoding,
        ::keyring::Error::TooLong(name, limit) => KeyringSecretError::TooLong { name, limit },
        ::keyring::Error::Invalid(name, reason) => KeyringSecretError::Invalid { name, reason },
        ::keyring::Error::Ambiguous(_) => KeyringSecretError::Ambiguous,
        other => KeyringSecretError::Platform(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_account_key_is_uuid_string() {
        let id =
            AccountId::from(uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000001").unwrap());
        assert_eq!(
            KeyringSecretStore::account_key(id),
            "00000000-0000-4000-8000-000000000001"
        );
    }

    #[test]
    fn test_secret_ref_includes_service_and_account_key() {
        let id =
            AccountId::from(uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000002").unwrap());
        let store = KeyringSecretStore::new("postblox-test");
        assert_eq!(
            store.secret_ref(id),
            "keyring:postblox-test:00000000-0000-4000-8000-000000000002"
        );
    }

    #[test]
    fn test_keyring_error_mapping_is_deterministic() {
        assert_eq!(
            map_keyring_error(::keyring::Error::NoEntry),
            KeyringSecretError::NotFound
        );
        assert_eq!(
            map_keyring_error(::keyring::Error::BadEncoding(vec![0xff])),
            KeyringSecretError::BadEncoding
        );
        assert_eq!(
            map_keyring_error(::keyring::Error::TooLong("service".into(), 64)),
            KeyringSecretError::TooLong {
                name: "service".into(),
                limit: 64,
            }
        );
        assert_eq!(
            map_keyring_error(::keyring::Error::Invalid("service".into(), "empty".into())),
            KeyringSecretError::Invalid {
                name: "service".into(),
                reason: "empty".into(),
            }
        );
    }

    #[tokio::test]
    #[ignore = "requires POSTBLOX_TEST_KEYRING=1 and a writable desktop/session keyring"]
    async fn live_keyring_put_get_overwrite_delete_requires_postblox_test_keyring_env() {
        if std::env::var("POSTBLOX_TEST_KEYRING").as_deref() != Ok("1") {
            eprintln!("set POSTBLOX_TEST_KEYRING=1 to run the live OS keyring test");
            return;
        }

        let service = format!("postblox-test-{}", uuid::Uuid::new_v4());
        let store = KeyringSecretStore::new(service);
        let id = AccountId::new();

        store.put(id, Zeroizing::new("v1".into())).await.unwrap();
        assert_eq!(store.get(id).await.unwrap().unwrap().as_str(), "v1");
        store.put(id, Zeroizing::new("v2".into())).await.unwrap();
        assert_eq!(store.get(id).await.unwrap().unwrap().as_str(), "v2");
        store.delete(id).await.unwrap();
        assert!(store.get(id).await.unwrap().is_none());
    }
}
