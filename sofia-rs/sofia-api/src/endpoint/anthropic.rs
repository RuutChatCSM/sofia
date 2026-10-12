//! Anthropic Messages (`/v1/messages`) endpoint client.
//!
//! Structurally identical to [`crate::endpoint::chat_completions::ChatCompletionsClient`]:
//! both wrap an [`EndpointSession`], which supplies the provider, auth provider,
//! retry policy, and request telemetry. Only the endpoint path and the event
//! decoder differ. Anthropic authenticates via `x-api-key` rather than a bearer
//! token; that is handled by the provider-scoped auth provider, so the session
//! machinery here is unchanged.

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
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::OnceLock;
use tracing::instrument;

/// Provider-relative path for the Anthropic Messages API.
pub const ANTHROPIC_MESSAGES_PATH: &str = "/messages";

pub struct AnthropicClient<T: HttpTransport> {
    session: EndpointSession<T>,
    sse_telemetry: Option<Arc<dyn SseTelemetry>>,
}

impl<T: HttpTransport> AnthropicClient<T> {
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

    /// Stream a turn from an already-encoded Messages API body.
    ///
    /// `freeform_tool_names` carries the flattened names of the freeform tools
    /// (for example `apply_patch`) that this request advertised. Anthropic has
    /// no freeform/custom tool type, so they are bridged as function tools with
    /// a single `input` string; the decoder needs the names to translate a
    /// `tool_use` block back into a `CustomToolCall` the engine can dispatch.
    #[instrument(
        name = "anthropic.stream",
        level = "info",
        skip_all,
        fields(
            transport = "anthropic_messages_http",
            http.method = "POST",
            api.path = ANTHROPIC_MESSAGES_PATH
        )
    )]
    pub async fn stream(
        &self,
        body: Value,
        extra_headers: HeaderMap,
        compression: Compression,
        turn_state: Option<Arc<OnceLock<String>>>,
        freeform_tool_names: Arc<HashSet<String>>,
    ) -> Result<ResponseStream, ApiError> {
        let body = EncodedJsonBody::encode(&body)
            .map_err(|e| ApiError::Stream(format!("failed to encode anthropic request: {e}")))?;
        self.stream_encoded(
            body,
            extra_headers,
            compression,
            turn_state,
            freeform_tool_names,
        )
        .await
    }

    async fn stream_encoded(
        &self,
        body: EncodedJsonBody,
        extra_headers: HeaderMap,
        compression: Compression,
        turn_state: Option<Arc<OnceLock<String>>>,
        freeform_tool_names: Arc<HashSet<String>>,
    ) -> Result<ResponseStream, ApiError> {
        let request_compression = match compression {
            Compression::None => sofia_client::RequestCompression::None,
            Compression::Zstd => sofia_client::RequestCompression::Zstd,
        };

        let stream_response = self
            .session
            .stream_encoded_json_with(
                Method::POST,
                ANTHROPIC_MESSAGES_PATH,
                extra_headers,
                Some(body),
                |req| {
                    req.headers
                        .entry("anthropic-version")
                        .or_insert(http::HeaderValue::from_static("2023-06-01"));
                    req.headers.insert(
                        http::header::ACCEPT,
                        http::HeaderValue::from_static("text/event-stream"),
                    );
                    req.compression = request_compression;
                },
            )
            .await?;

        Ok(crate::sse::spawn_anthropic_stream(
            stream_response,
            self.session.provider().stream_idle_timeout,
            self.sse_telemetry.clone(),
            turn_state,
            freeform_tool_names,
        ))
    }
}
