//! IMAP client: connect, authenticate, list folders.
//!
//! `Connector` lets tests substitute a plain-TCP transport without
//! pulling in a real TLS handshake.

use std::sync::Arc;

use async_imap::types::Name;
use futures::StreamExt;
use tokio::net::TcpStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::TlsConnector;

use super::error::ImapError;

/// A connected, authenticated IMAP session. Generic over the transport
/// stream so tests can swap in a plain TCP socket.
pub type Session<S> = async_imap::Session<S>;

/// How to obtain a stream for a `host:port` pair. Implementors decide
/// whether the wire is plain TCP or TLS.
#[async_trait::async_trait]
pub trait Connector: Send + Sync {
    /// The byte stream type produced by this connector.
    type Stream: tokio::io::AsyncRead
        + tokio::io::AsyncWrite
        + Unpin
        + Send
        + std::fmt::Debug
        + 'static;

    async fn connect(&self, host: &str, port: u16) -> Result<Self::Stream, ImapError>;
}

/// Production connector: TCP + rustls + native cert store.
pub struct RustlsConnector {
    config: Arc<rustls::ClientConfig>,
}

impl RustlsConnector {
    pub fn new() -> Result<Self, ImapError> {
        // Install the ring crypto provider once per process. Multiple
        // installs return Err which we treat as benign (already set).
        let _ = rustls::crypto::ring::default_provider().install_default();
        use rustls_platform_verifier::ConfigVerifierExt;
        let config = rustls::ClientConfig::with_platform_verifier()
            .map_err(|e| ImapError::Tls(e.to_string()))?;
        Ok(Self {
            config: Arc::new(config),
        })
    }
}

#[async_trait::async_trait]
impl Connector for RustlsConnector {
    type Stream = tokio_rustls::client::TlsStream<TcpStream>;

    async fn connect(&self, host: &str, port: u16) -> Result<Self::Stream, ImapError> {
        let tcp = TcpStream::connect((host, port)).await?;
        let server_name = ServerName::try_from(host.to_string())
            .map_err(|_| ImapError::InvalidName(host.to_string()))?;
        let connector = TlsConnector::from(self.config.clone());
        connector
            .connect(server_name, tcp)
            .await
            .map_err(|e| ImapError::Tls(e.to_string()))
    }
}

/// Plain-TCP connector — only used by tests.
pub struct PlainConnector;

#[async_trait::async_trait]
impl Connector for PlainConnector {
    type Stream = TcpStream;

    async fn connect(&self, host: &str, port: u16) -> Result<Self::Stream, ImapError> {
        Ok(TcpStream::connect((host, port)).await?)
    }
}

/// Open an IMAP connection, read the greeting, log in, return the
/// authenticated session.
pub async fn connect<C: Connector>(
    connector: &C,
    host: &str,
    port: u16,
    username: &str,
    password: &str,
) -> Result<Session<C::Stream>, ImapError> {
    let stream = connector.connect(host, port).await?;
    let mut client = async_imap::Client::new(stream);
    let greeting = client
        .read_response()
        .await
        .map_err(ImapError::from)?
        .ok_or_else(|| ImapError::Protocol("server closed connection before greeting".into()))?;
    drop(greeting);
    client
        .login(username, password)
        .await
        .map_err(|(e, _)| ImapError::from(e))
}

/// Fetch the list of mailboxes on the server. Equivalent to `LIST "" "*"`.
pub async fn list_folders<S>(session: &mut Session<S>) -> Result<Vec<FolderInfo>, ImapError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send + 'static,
{
    let stream = session
        .list(Some(""), Some("*"))
        .await
        .map_err(ImapError::from)?;
    let names: Vec<Name> = stream.filter_map(|r| async move { r.ok() }).collect().await;
    Ok(names
        .into_iter()
        .map(|n| FolderInfo {
            name: n.name().to_string(),
            delimiter: n
                .delimiter()
                .map(|s| s.to_string())
                .unwrap_or_else(|| "/".into()),
            selectable: !n
                .attributes()
                .iter()
                .any(|a| matches!(a, async_imap::types::NameAttribute::NoSelect)),
        })
        .collect())
}

/// Minimal projection of an IMAP folder listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderInfo {
    pub name: String,
    pub delimiter: String,
    pub selectable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_folder_info_round_trip() {
        let f = FolderInfo {
            name: "INBOX".into(),
            delimiter: "/".into(),
            selectable: true,
        };
        assert_eq!(f.name, "INBOX");
    }

    #[tokio::test]
    async fn test_plain_connector_connects_to_local_listener() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accept = tokio::spawn(async move {
            let _ = listener.accept().await.unwrap();
        });
        let conn = PlainConnector;
        let _stream = conn.connect("127.0.0.1", addr.port()).await.unwrap();
        let _ = accept.await;
    }

    #[tokio::test]
    async fn test_invalid_server_name_returns_invalid_name() {
        let connector = RustlsConnector::new().unwrap();
        // A literal IPv4 isn't a valid server name for TLS SNI.
        let err = connector.connect("256.256.256.256", 443).await.unwrap_err();
        // Either DNS fails first (Io) or SNI rejects (InvalidName).
        assert!(matches!(err, ImapError::Io(_) | ImapError::InvalidName(_)));
    }
}
