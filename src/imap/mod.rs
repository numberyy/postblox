//! IMAP transport: TLS connection wrapper, authentication, folder
//! listing, and one-shot folder sync.
//!
//! Wraps `async-imap` so the rest of the codebase doesn't have to import
//! it. Behind a `Connector` trait so tests can substitute an in-process
//! IMAP listener, and behind two consumer traits so the daemon
//! dispatcher doesn't carry a generic stream type:
//!
//! - [`ImapAuth`] — login + folder list (used by `account.test_login`).
//! - [`ImapSync`] — pull a UID range (used by `account.sync_folder`).

pub mod client;
pub mod error;

pub use client::{
    connect, connect_with_credential, fetch_uid_range, list_folders, wait_for_idle_change,
    Connector, FetchedMessage, FolderInfo, FolderSync, IdleOutcome, IdleRequest, PlainConnector,
    RustlsConnector,
};
pub use error::ImapError;

use std::sync::Arc;

use crate::auth::MailCredential;

/// Erased entry point for [auth + folder list]: hides the underlying
/// stream type so it can sit behind a `dyn` trait object.
#[async_trait::async_trait]
pub trait ImapAuth: Send + Sync {
    async fn test_login(
        &self,
        host: &str,
        port: u16,
        username: &str,
        credential: &MailCredential,
    ) -> Result<Vec<FolderInfo>, ImapError>;
}

/// Erased entry point for [select + fetch]. Each call opens a fresh
/// connection — pooling/reuse lands with the IDLE worker (R3b-3b).
#[async_trait::async_trait]
pub trait ImapSync: Send + Sync {
    async fn sync_folder(
        &self,
        host: &str,
        port: u16,
        username: &str,
        credential: &MailCredential,
        folder: &str,
        from_uid: u32,
    ) -> Result<FolderSync, ImapError>;
}

/// Erased entry point for one bounded IMAP IDLE wait.
#[async_trait::async_trait]
pub trait ImapIdle: Send + Sync {
    async fn idle_once(&self, request: IdleRequest<'_>) -> Result<IdleOutcome, ImapError>;
}

/// Concrete impl backed by a `Connector`. One struct services both
/// `ImapAuth` and `ImapSync` so the daemon needs only one `Arc`.
pub struct ConnectorAuth<C: Connector> {
    connector: C,
}

impl<C: Connector> ConnectorAuth<C> {
    pub fn new(connector: C) -> Self {
        Self { connector }
    }
}

#[async_trait::async_trait]
impl<C: Connector> ImapAuth for ConnectorAuth<C> {
    async fn test_login(
        &self,
        host: &str,
        port: u16,
        username: &str,
        credential: &MailCredential,
    ) -> Result<Vec<FolderInfo>, ImapError> {
        let mut session =
            client::connect_with_credential(&self.connector, host, port, username, credential)
                .await?;
        let folders = list_folders(&mut session).await?;
        // best-effort logout; ignore failure since the session is already closing.
        let _ = session.logout().await;
        Ok(folders)
    }
}

#[async_trait::async_trait]
impl<C: Connector> ImapSync for ConnectorAuth<C> {
    async fn sync_folder(
        &self,
        host: &str,
        port: u16,
        username: &str,
        credential: &MailCredential,
        folder: &str,
        from_uid: u32,
    ) -> Result<FolderSync, ImapError> {
        let mut session =
            client::connect_with_credential(&self.connector, host, port, username, credential)
                .await?;
        let out = fetch_uid_range(&mut session, folder, from_uid).await;
        // best-effort logout; ignore failure since the session is already closing.
        let _ = session.logout().await;
        out
    }
}

#[async_trait::async_trait]
impl<C: Connector> ImapIdle for ConnectorAuth<C> {
    async fn idle_once(&self, request: IdleRequest<'_>) -> Result<IdleOutcome, ImapError> {
        wait_for_idle_change(&self.connector, request).await
    }
}

/// Default production binding: rustls + native cert store. Returns the
/// same `Arc` typed two ways so callers don't have to construct twice.
pub fn default_auth() -> Result<Arc<dyn ImapAuth>, ImapError> {
    Ok(Arc::new(ConnectorAuth::new(RustlsConnector::new()?)))
}

pub fn default_sync() -> Result<Arc<dyn ImapSync>, ImapError> {
    Ok(Arc::new(ConnectorAuth::new(RustlsConnector::new()?)))
}

pub fn default_idle() -> Result<Arc<dyn ImapIdle>, ImapError> {
    Ok(Arc::new(ConnectorAuth::new(RustlsConnector::new()?)))
}
