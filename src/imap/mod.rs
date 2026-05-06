//! IMAP transport: TLS connection wrapper, authentication, folder listing.
//!
//! Wraps `async-imap` so the rest of the codebase doesn't have to import
//! it. Behind a `Connector` trait so tests can substitute an in-process
//! IMAP listener, and behind an [`ImapAuth`] trait so the daemon
//! dispatcher doesn't carry a generic stream type.

pub mod client;
pub mod error;

pub use client::{connect, list_folders, Connector, FolderInfo, PlainConnector, RustlsConnector};
pub use error::ImapError;

use std::sync::Arc;

/// Erased entry point used by the daemon: log in, return folders, log
/// out. Hides the underlying stream type so it can sit behind a
/// `dyn` trait object.
#[async_trait::async_trait]
pub trait ImapAuth: Send + Sync {
    async fn test_login(
        &self,
        host: &str,
        port: u16,
        username: &str,
        password: &str,
    ) -> Result<Vec<FolderInfo>, ImapError>;
}

/// Concrete `ImapAuth` backed by a `Connector`.
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
        password: &str,
    ) -> Result<Vec<FolderInfo>, ImapError> {
        let mut session = connect(&self.connector, host, port, username, password).await?;
        let folders = list_folders(&mut session).await?;
        let _ = session.logout().await;
        Ok(folders)
    }
}

/// Default production binding: rustls + native cert store.
pub fn default_auth() -> Result<Arc<dyn ImapAuth>, ImapError> {
    Ok(Arc::new(ConnectorAuth::new(RustlsConnector::new()?)))
}
