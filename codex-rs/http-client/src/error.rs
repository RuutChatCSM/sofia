//! Errors returned by the shared Codex HTTP transport.

use crate::client::HttpError;
use http::HeaderMap;
use http::StatusCode;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("http {status}: {body:?}")]
    Http {
        status: StatusCode,
        url: Option<String>,
        headers: Option<HeaderMap>,
        body: Option<String>,
    },
    #[error("retry limit reached")]
    RetryLimit,
    #[error("timeout")]
    Timeout,
    #[error("connection failed: {0}")]
    Connection(#[source] HttpError),
    #[error("network error: {0}")]
    Network(String),
    #[error("request build error: {0}")]
    Build(String),
}

impl TransportError {
    /// Returns true if this transport error is transient and should be retried.
    pub fn is_retryable(&self) -> bool {
        match self {
            TransportError::Http { status, .. } => {
                status.is_server_error()
                    || *status == StatusCode::TOO_MANY_REQUESTS
                    || *status == StatusCode::REQUEST_TIMEOUT
            }
            TransportError::RetryLimit
            | TransportError::Timeout
            | TransportError::Connection(_)
            | TransportError::Network(_) => true,
            TransportError::Build(_) => false,
        }
    }
}

#[derive(Debug, Error)]
pub enum StreamError {
    #[error("stream failed: {0}")]
    Stream(String),
    #[error("timeout")]
    Timeout,
}
