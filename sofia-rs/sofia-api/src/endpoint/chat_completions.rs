//! Chat Completions (`/chat/completions`) endpoint client.
//!
//! Structurally identical to [`crate::endpoint::responses::ResponsesClient`]:
//! both wrap an [`EndpointSession`], which supplies the provider, auth provider,
//! retry policy, and request telemetry. Only the endpoint path and the event
//! decoder differ. Keeping the two clients on the same session is what makes the
//! wire formats interchangeable — a provider switched between `responses` and
//! `chat_completions` keeps identical retry, auth, and telemetry behaviour.

use crate::Compression;
use crate::auth::SharedAuthProvider;
use crate::common::ResponseStream;
use crate::endpoint::session::EndpointSession;
use crate::error::ApiError;
use crate::provider::Provider;
use crate::telemetry::SseTelemetry;
use http::HeaderMap;
use http::Method;
use serde_json::Value;
use sofia_client::EncodedJsonBody;
use sofia_client::HttpTransport;
use sofia_client::RequestTelemetry;
use std::sync::Arc;
use std::sync::OnceLock;
use tracing::instrument;

/// Provider-relative path for Chat Completions inference.
pub const CHAT_COMPLETIONS_PATH: &str = "/chat/completions";

pub struct ChatCompletionsClient<T: HttpTransport> {
    session: EndpointSession<T>,
    sse_telemetry: Option<Arc<dyn SseTelemetry>>,
}

impl<T: HttpTransport> ChatCompletionsClient<T> {
    pub fn new(transport: T, provider: Provider, auth: SharedAuthProvider) -> Self {
        Self {
            session: EndpointSession::new(transport, provider, auth),
            sse_telemetry: None,
        }
    }

    pub fn with_telemetry(
        self,
        request: Option<Arc<dyn RequestTelemetry>>,
        sse: Option<Arc<dyn SseTelemetry>>,
    ) -> Self {
        Self {
            session: self.session.with_request_telemetry(request),
            sse_telemetry: sse,
        }
    }

    /// Stream a turn from an already-encoded Chat Completions body.
    #[instrument(
        name = "chat_completions.stream",
        level = "info",
        skip_all,
        fields(
            transport = "chat_completions_http",
            http.method = "POST",
            api.path = CHAT_COMPLETIONS_PATH
        )
    )]
    pub async fn stream(
        &self,
        body: Value,
        extra_headers: HeaderMap,
        compression: Compression,
        turn_state: Option<Arc<OnceLock<String>>>,
    ) -> Result<ResponseStream, ApiError> {
        let body = EncodedJsonBody::encode(&body).map_err(|e| {
            ApiError::Stream(format!("failed to encode chat completions request: {e}"))
        })?;
        self.stream_encoded(body, extra_headers, compression, turn_state)
            .await
    }

    async fn stream_encoded(
        &self,
        body: EncodedJsonBody,
        extra_headers: HeaderMap,
        compression: Compression,
        turn_state: Option<Arc<OnceLock<String>>>,
    ) -> Result<ResponseStream, ApiError> {
        let request_compression = match compression {
            Compression::None => sofia_client::RequestCompression::None,
            Compression::Zstd => sofia_client::RequestCompression::Zstd,
        };

        let stream_response = self
            .session
            .stream_encoded_json_with(
                Method::POST,
                CHAT_COMPLETIONS_PATH,
                extra_headers,
                Some(body),
                |req| {
                    req.headers.insert(
                        http::header::ACCEPT,
                        http::HeaderValue::from_static("text/event-stream"),
                    );
                    req.compression = request_compression;
                },
            )
            .await?;

        Ok(crate::sse::spawn_chat_completions_stream(
            stream_response,
            self.session.provider().stream_idle_timeout,
            self.sse_telemetry.clone(),
            turn_state,
        ))
    }
}
