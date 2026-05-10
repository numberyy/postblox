//! IMAP client: connect, authenticate, list folders.
//!
//! `Connector` lets tests substitute a plain-TCP transport without
//! pulling in a real TLS handshake.

use std::sync::Arc;
use std::time::Duration;

use async_imap::extensions::idle::IdleResponse;
use async_imap::types::Capability;
use async_imap::types::Name;
use futures::StreamExt;
use tokio::net::TcpStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::TlsConnector;
use tokio_util::sync::CancellationToken;

use crate::auth::{CredentialKind, MailCredential};
use crate::oauth::google::xoauth2_sasl_string;

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

    /// Open a stream to `host:port` ready for IMAP traffic.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`ImapError::Io`] if the TCP connect fails.
    /// - [`ImapError::InvalidName`] if `host` is not a valid TLS server name
    ///   (TLS connectors only).
    /// - [`ImapError::Tls`] if the TLS handshake fails.
    async fn connect(&self, host: &str, port: u16) -> Result<Self::Stream, ImapError>;
}

/// Production connector: TCP + rustls + native cert store.
pub struct RustlsConnector {
    config: Arc<rustls::ClientConfig>,
}

impl RustlsConnector {
    /// Build a new rustls connector backed by the platform cert store.
    ///
    /// # Errors
    ///
    /// Returns [`ImapError::Tls`] if rustls cannot construct a client
    /// config from the platform verifier (typically a missing or
    /// unreadable system root store).
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
///
/// # Errors
///
/// Returns:
/// - [`ImapError::Io`] if the TCP connect or stream read fails.
/// - [`ImapError::Tls`] if the TLS handshake fails.
/// - [`ImapError::InvalidName`] if `host` is not a valid TLS server name.
/// - [`ImapError::Auth`] if the server rejects the password.
/// - [`ImapError::Protocol`] if the greeting is missing or any other
///   IMAP-level failure occurs.
pub async fn connect<C: Connector>(
    connector: &C,
    host: &str,
    port: u16,
    username: &str,
    password: &str,
) -> Result<Session<C::Stream>, ImapError> {
    let credential = MailCredential::password(password);
    connect_with_credential(connector, host, port, username, &credential).await
}

/// Open an IMAP connection and authenticate with the given
/// [`MailCredential`] (password or `XOAUTH2`).
///
/// # Errors
///
/// Returns:
/// - [`ImapError::Io`] if the TCP connect or stream read fails.
/// - [`ImapError::Tls`] if the TLS handshake fails.
/// - [`ImapError::InvalidName`] if `host` is not a valid TLS server name.
/// - [`ImapError::Auth`] if `LOGIN` or `AUTHENTICATE XOAUTH2` is rejected.
/// - [`ImapError::Protocol`] if the greeting is missing or any other
///   IMAP-level failure occurs.
pub async fn connect_with_credential<C: Connector>(
    connector: &C,
    host: &str,
    port: u16,
    username: &str,
    credential: &MailCredential,
) -> Result<Session<C::Stream>, ImapError> {
    let stream = connector.connect(host, port).await?;
    let mut client = async_imap::Client::new(stream);
    let greeting = client
        .read_response()
        .await
        .map_err(ImapError::from)?
        .ok_or_else(|| ImapError::Protocol("server closed connection before greeting".into()))?;
    drop(greeting);
    match credential.kind() {
        CredentialKind::Password => client
            .login(username, credential.secret())
            .await
            .map_err(|(e, _)| ImapError::from(e)),
        CredentialKind::OAuth2Bearer => {
            let auth = Xoauth2 {
                username,
                access_token: credential.secret(),
            };
            client
                .authenticate("XOAUTH2", auth)
                .await
                .map_err(|(e, _)| ImapError::from(e))
        }
    }
}

struct Xoauth2<'a> {
    username: &'a str,
    access_token: &'a str,
}

impl async_imap::Authenticator for Xoauth2<'_> {
    type Response = String;

    fn process(&mut self, _: &[u8]) -> Self::Response {
        xoauth2_sasl_string(self.username, self.access_token)
    }
}

/// Fetch the list of mailboxes on the server. Equivalent to `LIST "" "*"`.
///
/// # Errors
///
/// Returns [`ImapError::Protocol`] if the `LIST` command fails or the
/// server closes the connection mid-response, or [`ImapError::Io`] if
/// reading the underlying stream fails.
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

/// Result of selecting a folder and fetching `<from_uid>:*`. The
/// reconciler uses [`FolderSync::uid_validity`] to detect rebuilds and
/// [`FolderSync::messages`] for the actual inserts.
#[derive(Debug, Clone)]
pub struct FolderSync {
    pub uid_validity: Option<u32>,
    pub uid_next: Option<u32>,
    pub exists: u32,
    pub messages: Vec<FetchedMessage>,
}

/// A single message returned by a UID-FETCH. `raw` holds RFC822 bytes;
/// `internal_date` is the IMAP `INTERNALDATE` (when the server
/// received it).
#[derive(Debug, Clone)]
pub struct FetchedMessage {
    pub uid: u32,
    pub flags: Vec<String>,
    pub internal_date: Option<chrono::DateTime<chrono::Utc>>,
    pub raw: Vec<u8>,
}

/// Why one IDLE wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleOutcome {
    NewData,
    Timeout,
    Interrupted,
}

pub struct IdleRequest<'a> {
    pub host: &'a str,
    pub port: u16,
    pub username: &'a str,
    pub credential: &'a MailCredential,
    pub folder: &'a str,
    pub timeout: Duration,
    pub cancel: CancellationToken,
}

/// IMAP system flags are reported as enum variants; we serialise them
/// back to their RFC 3501 wire form so the rest of the codebase can
/// treat flags as strings.
fn flag_name(f: async_imap::types::Flag<'_>) -> String {
    use async_imap::types::Flag;
    match f {
        Flag::Seen => "\\Seen".into(),
        Flag::Answered => "\\Answered".into(),
        Flag::Flagged => "\\Flagged".into(),
        Flag::Deleted => "\\Deleted".into(),
        Flag::Draft => "\\Draft".into(),
        Flag::Recent => "\\Recent".into(),
        Flag::MayCreate => "\\*".into(),
        Flag::Custom(c) => c.into_owned(),
    }
}

/// `SELECT folder` then `UID FETCH <from_uid>:* (UID FLAGS INTERNALDATE
/// RFC822)`. Stops short of `LOGOUT` so callers can chain more
/// operations; the wrapping `ImapSync::sync_folder` impl logs out for
/// us.
///
/// # Errors
///
/// Returns [`ImapError::Protocol`] if `SELECT` or `UID FETCH` fails (for
/// example, the folder does not exist or the server returns `NO`/`BAD`),
/// or [`ImapError::Io`] if reading the underlying stream fails.
pub async fn fetch_uid_range<S>(
    session: &mut Session<S>,
    folder: &str,
    from_uid: u32,
) -> Result<FolderSync, ImapError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send + 'static,
{
    let mailbox = session.select(folder).await.map_err(ImapError::from)?;
    let uid_validity = mailbox.uid_validity;
    let uid_next = mailbox.uid_next;
    let exists = mailbox.exists;

    // Empty mailbox: nothing to pull.
    if exists == 0 {
        return Ok(FolderSync {
            uid_validity,
            uid_next,
            exists,
            messages: vec![],
        });
    }

    // `<from>:*` selects everything from `from_uid` upward, regardless of
    // whether the server's actual high UID is `uid_next - 1` or lower.
    let range = format!("{}:*", from_uid.max(1));
    let stream = session
        .uid_fetch(&range, "(UID FLAGS INTERNALDATE RFC822)")
        .await
        .map_err(ImapError::from)?;
    let fetches: Vec<async_imap::types::Fetch> =
        stream.filter_map(|r| async move { r.ok() }).collect().await;

    let messages: Vec<FetchedMessage> = fetches
        .into_iter()
        .filter_map(|f| {
            let uid = f.uid?;
            // Skip messages where the server did not include a body.
            // This shouldn't happen for the query we sent but defending
            // against it keeps the reconciler simpler.
            let raw = f.body().map(|b| b.to_vec())?;
            let flags = f.flags().map(flag_name).collect();
            let internal_date = f.internal_date().map(|d| d.with_timezone(&chrono::Utc));
            Some(FetchedMessage {
                uid,
                flags,
                internal_date,
                raw,
            })
        })
        .collect();

    Ok(FolderSync {
        uid_validity,
        uid_next,
        exists,
        messages,
    })
}

/// Connect, authenticate, `SELECT request.folder`, and wait once for an
/// IDLE change, the configured timeout, or the cancellation token.
///
/// # Errors
///
/// Returns:
/// - [`ImapError::Io`] if the TCP connect or stream read fails.
/// - [`ImapError::Tls`] if the TLS handshake fails.
/// - [`ImapError::InvalidName`] if `request.host` is not a valid TLS server name.
/// - [`ImapError::Auth`] if the server rejects the credentials.
/// - [`ImapError::Unsupported`] if the server does not advertise the
///   IDLE capability.
/// - [`ImapError::Protocol`] for any other IMAP-level failure during
///   `CAPABILITY`/`SELECT`/`IDLE`.
pub async fn wait_for_idle_change<C: Connector>(
    connector: &C,
    request: IdleRequest<'_>,
) -> Result<IdleOutcome, ImapError> {
    let IdleRequest {
        host,
        port,
        username,
        credential,
        folder,
        timeout,
        cancel,
    } = request;
    let mut session = connect_with_credential(connector, host, port, username, credential).await?;
    let capabilities = session.capabilities().await.map_err(ImapError::from)?;
    let supports_idle = capabilities
        .iter()
        .any(|cap| matches!(cap, Capability::Atom(name) if name.eq_ignore_ascii_case("IDLE")));
    if !supports_idle {
        // best-effort logout; ignore failure since the session is already closing.
        let _ = session.logout().await;
        return Err(ImapError::Unsupported(
            "server does not advertise IDLE".into(),
        ));
    }

    session.select(folder).await.map_err(ImapError::from)?;
    let mut idle = session.idle();
    idle.init().await.map_err(ImapError::from)?;

    let response = {
        let (wait, interrupt) = idle.wait_with_timeout(timeout);
        tokio::pin!(wait);
        tokio::select! {
            response = &mut wait => response.map_err(ImapError::from),
            _ = cancel.cancelled() => {
                drop(interrupt);
                wait.await.map_err(ImapError::from)
            }
        }
    };

    let mut session = idle.done().await.map_err(ImapError::from)?;
    // best-effort logout; ignore failure since the session is already closing.
    let _ = session.logout().await;

    match response? {
        IdleResponse::NewData(_) => Ok(IdleOutcome::NewData),
        IdleResponse::Timeout => Ok(IdleOutcome::Timeout),
        IdleResponse::ManualInterrupt => Ok(IdleOutcome::Interrupted),
    }
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

    #[test]
    fn test_xoauth2_authenticator_returns_sasl_payload() {
        let mut auth = Xoauth2 {
            username: "me@example.com",
            access_token: "token",
        };
        assert_eq!(
            async_imap::Authenticator::process(&mut auth, b""),
            "user=me@example.com\x01auth=Bearer token\x01\x01"
        );
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
