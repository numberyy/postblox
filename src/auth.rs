//! Auth state shared by the IMAP and SMTP clients.
//!
//! [`MailCredential`] is the single carrier type: either a password or
//! an OAuth2 bearer token, both wrapped in [`zeroize::Zeroizing`] so
//! the secret bytes are scrubbed on drop. The hand-rolled `Debug` impl
//! prints `<redacted>` for the secret payload so tracing and panic
//! backtraces can never leak credentials. [`CredentialKind`] lets the
//! transport layer pick the right SASL mechanism (`AUTH PLAIN`/`LOGIN`
//! vs. `XOAUTH2`) without re-inspecting the secret.

use zeroize::Zeroizing;

use crate::secrets::Secret;

/// Kind of credential carried by [`MailCredential`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialKind {
    /// Username + password (`AUTH PLAIN` / `AUTH LOGIN`).
    Password,
    /// OAuth2 bearer token (`XOAUTH2`).
    OAuth2Bearer,
}

/// Credential carrier shared by the IMAP and SMTP clients.
#[derive(Clone)]
pub enum MailCredential {
    /// Password credential.
    Password(Secret),
    /// OAuth2 bearer-token credential.
    OAuth2Bearer(Secret),
}

impl std::fmt::Debug for MailCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MailCredential")
            .field("kind", &self.kind())
            .field("secret", &"<redacted>")
            .finish()
    }
}

impl MailCredential {
    /// Construct a password credential from a plain string.
    pub fn password(secret: impl Into<String>) -> Self {
        Self::Password(Zeroizing::new(secret.into()))
    }

    /// Construct a password credential from an existing [`Secret`].
    pub fn password_secret(secret: Secret) -> Self {
        Self::Password(secret)
    }

    /// Construct an OAuth2 bearer-token credential.
    pub fn oauth2_bearer(access_token: impl Into<String>) -> Self {
        Self::OAuth2Bearer(Zeroizing::new(access_token.into()))
    }

    /// Return the variant tag without exposing the secret.
    pub fn kind(&self) -> CredentialKind {
        match self {
            Self::Password(_) => CredentialKind::Password,
            Self::OAuth2Bearer(_) => CredentialKind::OAuth2Bearer,
        }
    }

    /// Borrow the secret payload as a `&str`.
    pub fn secret(&self) -> &str {
        match self {
            Self::Password(secret) | Self::OAuth2Bearer(secret) => secret.as_str(),
        }
    }

    /// `true` when the underlying secret is empty.
    pub fn is_empty(&self) -> bool {
        self.secret().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mail_credential_reports_kind_and_secret() {
        let password = MailCredential::password("p");
        assert_eq!(password.kind(), CredentialKind::Password);
        assert_eq!(password.secret(), "p");

        let token = MailCredential::oauth2_bearer("token");
        assert_eq!(token.kind(), CredentialKind::OAuth2Bearer);
        assert_eq!(token.secret(), "token");
    }

    #[test]
    fn test_mail_credential_debug_redacts_secret() {
        let printed = format!("{:?}", MailCredential::oauth2_bearer("token"));
        assert!(printed.contains("OAuth2Bearer"));
        assert!(printed.contains("<redacted>"));
        assert!(!printed.contains("token"));
    }
}
