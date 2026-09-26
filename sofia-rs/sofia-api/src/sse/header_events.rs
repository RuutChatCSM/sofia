//! Transport-level events derived from response headers.
//!
//! These are wire-format independent: any OpenAI-compatible provider can return
//! them, not just providers speaking the Responses grammar. Both the Responses
//! and Chat Completions stream parsers emit them so downstream consumers (rate
//! limit display, server-model mismatch warnings, compaction) behave identically
//! regardless of which wire API a provider is configured for.

use crate::common::ResponseEvent;
use crate::common::ResponseStream;
use crate::error::ApiError;
use crate::rate_limits::parse_all_rate_limits;
use sofia_protocol::protocol::RateLimitSnapshot;
use crate::safety_buffering::treatment_from_headers;
use crate::telemetry::SseTelemetry;
use http::HeaderMap;
use sofia_client::ByteStream;
use sofia_client::StreamResponse;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::mpsc;

pub(crate) const X_REASONING_INCLUDED_HEADER: &str = "x-reasoning-included";
pub(crate) const X_CODEX_TURN_STATE_HEADER: &str = "x-sofia-turn-state";
pub(crate) const OPENAI_MODEL_HEADER: &str = "openai-model";
pub(crate) const REQUEST_ID_HEADER: &str = "x-request-id";
const MODELS_ETAG_HEADER: &str = "X-Models-Etag";

/// Header-derived events captured before any SSE payload is parsed.
#[derive(Debug, Clone)]
pub(crate) struct HeaderEvents {
    pub(crate) server_model: Option<String>,
    pub(crate) rate_limit_snapshots: Vec<RateLimitSnapshot>,
    pub(crate) models_etag: Option<String>,
    pub(crate) reasoning_included: bool,
    pub(crate) safety_buffering_treatment: crate::common::SafetyBufferingTreatment,
    pub(crate) upstream_request_id: Option<String>,
    pub(crate) turn_state: Option<String>,
}

impl HeaderEvents {
    pub(crate) fn from_headers(headers: &HeaderMap) -> Self {
        let upstream_request_id = headers
            .get(REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let turn_state = headers
            .get(X_CODEX_TURN_STATE_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        Self {
            server_model: headers
                .get(OPENAI_MODEL_HEADER)
                .and_then(|v| v.to_str().ok())
                .map(ToString::to_string),
            rate_limit_snapshots: parse_all_rate_limits(headers),
            models_etag: headers
                .get(MODELS_ETAG_HEADER)
                .and_then(|v| v.to_str().ok())
                .map(ToString::to_string),
            reasoning_included: headers.contains_key(X_REASONING_INCLUDED_HEADER),
            safety_buffering_treatment: treatment_from_headers(headers).unwrap_or_default(),
            upstream_request_id,
            turn_state,
        }
    }

    /// Emit the header-derived events onto a freshly created channel.
    pub(crate) async fn emit(self, tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>) {
        if let Some(model) = self.server_model {
            let _ = tx_event.send(Ok(ResponseEvent::ServerModel(model))).await;
        }
        for snapshot in self.rate_limit_snapshots {
            let _ = tx_event.send(Ok(ResponseEvent::RateLimits(snapshot))).await;
        }
        if let Some(etag) = self.models_etag {
            let _ = tx_event.send(Ok(ResponseEvent::ModelsEtag(etag))).await;
        }
        if self.reasoning_included {
            let _ = tx_event
                .send(Ok(ResponseEvent::ServerReasoningIncluded(true)))
                .await;
        }
    }
}

/// Shared scaffolding for a wire-format SSE stream.
///
/// Owns the event channel, header-derived events, turn-state capture, idle
/// timeout, and telemetry hooks. The wire-specific part is only the closure that
/// consumes the byte stream and emits [`ResponseEvent`]s, so both parsers get
/// identical channel semantics and terminal behaviour.
pub(crate) fn spawn_wire_stream<F, Fut>(

    stream_response: StreamResponse,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
    turn_state: Option<Arc<OnceLock<String>>>,
    channel_capacity: usize,
    parse: F,
) -> ResponseStream
where
    F: Send + 'static + FnOnce(

            ByteStream,
            mpsc::Sender<Result<ResponseEvent, ApiError>>,
            Duration,
            Option<Arc<dyn SseTelemetry>>,
            crate::common::SafetyBufferingTreatment,
        ) -> Fut,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let header_events = HeaderEvents::from_headers(&stream_response.headers);
    if let (Some(turn_state), Some(captured)) = (turn_state.as_ref(), header_events.turn_state.as_ref())
    {
        let _ = turn_state.set(captured.clone());
    }
    let upstream_request_id = header_events.upstream_request_id.clone();
    let bytes = stream_response.bytes;
    let emit_events = header_events.clone();
    let safety_buffering_treatment = header_events.safety_buffering_treatment;
    let (tx_event, rx_event) = mpsc::channel::<Result<ResponseEvent, ApiError>>(channel_capacity);
    tokio::spawn(async move {
        emit_events.emit(&tx_event).await;
        parse(
            bytes,
            tx_event,
            idle_timeout,
            telemetry,
            safety_buffering_treatment,
        )
        .await;
    });

    ResponseStream {
        rx_event,
        upstream_request_id,
    }
}
