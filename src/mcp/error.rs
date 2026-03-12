use thiserror::Error;

#[derive(Debug, Error)]
pub enum McpError {
    #[error("missing required argument: {0}")]
    MissingArgument(String),

    #[error("unknown tool: {0}")]
    UnknownTool(String),

    #[error("http request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Api(String),
}
