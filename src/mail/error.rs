use thiserror::Error;

#[derive(Debug, Error)]
pub enum MailError {
    #[error("failed to parse email: {0}")]
    Parse(String),
}
