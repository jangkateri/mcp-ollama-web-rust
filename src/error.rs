//! Error vocabulary shared by the Ollama client, the tool layer, and startup.
//!
//! Codes are stable strings so callers and tests can branch on a code instead
//! of matching human-readable text.

use std::fmt;

/// Stable error classifications for every Ollama call this server makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OllamaErrorCode {
    /// The host could not be parsed into an http(s) base URL.
    InvalidHost,
    /// A tool argument was missing, empty, or malformed.
    InvalidArgument,
    /// TCP/DNS failure or a refused connection to the daemon.
    Unreachable,
    /// The daemon answered, but with a non-2xx status.
    HttpError,
    /// The endpoint rejected the request as unauthorized (HTTP 401/403).
    Unauthorized,
    /// The daemon answered 2xx with a body this server cannot use.
    BadResponse,
    /// Neither the experimental nor the stable endpoint exists on this daemon.
    NoEndpoint,
    /// The request exceeded the configured per-call timeout.
    Timeout,
    /// The caller cancelled the request.
    Aborted,
}

impl OllamaErrorCode {
    /// The stable wire-facing name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidHost => "OLLAMA_INVALID_HOST",
            Self::InvalidArgument => "OLLAMA_INVALID_ARGUMENT",
            Self::Unreachable => "OLLAMA_UNREACHABLE",
            Self::HttpError => "OLLAMA_HTTP_ERROR",
            Self::Unauthorized => "OLLAMA_UNAUTHORIZED",
            Self::BadResponse => "OLLAMA_BAD_RESPONSE",
            Self::NoEndpoint => "OLLAMA_NO_ENDPOINT",
            Self::Timeout => "OLLAMA_TIMEOUT",
            Self::Aborted => "OLLAMA_ABORTED",
        }
    }
}

impl fmt::Display for OllamaErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Error raised by every Ollama call this server makes.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct OllamaError {
    /// Human-readable text; surfaced to the model in an `isError` tool result.
    pub message: String,
    /// Stable classification.
    pub code: OllamaErrorCode,
    /// HTTP status, when the failure came from a response.
    pub status: Option<u16>,
    /// Internal: a 404 whose body says the *route* is absent, so try the next spelling.
    pub endpoint_missing: bool,
}

impl OllamaError {
    pub fn new(message: impl Into<String>, code: OllamaErrorCode) -> Self {
        Self {
            message: message.into(),
            code,
            status: None,
            endpoint_missing: false,
        }
    }

    pub fn with_status(mut self, status: u16) -> Self {
        self.status = Some(status);
        self
    }

    /// Mark a 404 as "this endpoint spelling does not exist here".
    pub fn as_endpoint_missing(mut self) -> Self {
        self.endpoint_missing = true;
        self
    }

    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new(message, OllamaErrorCode::InvalidArgument)
    }

    pub fn aborted(kind: &str) -> Self {
        Self::new(format!("Ollama {kind} cancelled"), OllamaErrorCode::Aborted)
    }
}

/// Raised when a tool argument is missing or malformed; mapped to JSON-RPC Invalid Params.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ToolArgumentError(pub String);

/// Raised when `tools/call` names a tool this server does not publish.
#[derive(Debug, thiserror::Error)]
#[error("Unknown tool: {name:?}. Available tools: web_search, web_fetch.")]
pub struct UnknownToolError {
    pub name: String,
}

/// Raised when a credentials file cannot be read or does not yield a value.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ApiKeyFileError(pub String);

/// Raised when configuration cannot be resolved before serving starts.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ConfigError(pub String);
