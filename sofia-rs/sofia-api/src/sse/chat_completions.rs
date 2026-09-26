//! Chat Completions (`/chat/completions`) SSE decoder.
//!
//! Emits the same [`ResponseEvent`] vocabulary as the Responses decoder so that
//! everything downstream of the wire layer — the turn loop, compaction, the UI —
//! behaves identically regardless of which wire API a provider is configured
//! for. Transport concerns (header events, idle timeout, channel semantics,
//! telemetry) are supplied by [`crate::sse::header_events::spawn_wire_stream`];
//! this module only translates `choices[]` deltas into events.
//!
//! The delta handling here is a faithful port of the original hand-rolled
//! parser in `core/src/client.rs`, including its provider quirks: incremental
//! tool-call argument accumulation, `reasoning_content` capture (which must be
//! echoed back on the next request for thinking-mode providers), and the
//! `finish_reason` to `end_turn` mapping.

use crate::common::ResponseEvent;
use crate::common::ResponseStream;
use crate::error::ApiError;
use crate::telemetry::SseTelemetry;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde_json::Value;
use sofia_client::ByteStream;
use sofia_client::StreamResponse;
use sofia_protocol::models::ContentItem;
use sofia_protocol::models::ResponseItem;
use sofia_protocol::ResponseItemId;
use sofia_protocol::models::ReasoningItemContent;
use sofia_protocol::protocol::TokenUsage;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio::time::timeout;
use tracing::debug;
use tracing::info;

/// Event channel capacity for Chat Completions streams.
const CHAT_EVENT_CHANNEL_CAPACITY: usize = 1600;

/// Spawn a Chat Completions event stream.
///
/// Mirrors [`crate::sse::responses::spawn_response_stream`] so both wire
/// formats produce a [`ResponseStream`] with identical semantics.
pub fn spawn_chat_completions_stream(
    stream_response: StreamResponse,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
    turn_state: Option<Arc<OnceLock<String>>>,
) -> ResponseStream {
    crate::sse::header_events::spawn_wire_stream(
        stream_response,
        idle_timeout,
        telemetry,
        turn_state,
        CHAT_EVENT_CHANNEL_CAPACITY,
        |bytes, tx_event, idle_timeout, telemetry, _treatment| {
            process_chat_completions_sse(bytes, tx_event, idle_timeout, telemetry)
        },
    )
}

/// Translate a Chat Completions SSE byte stream into [`ResponseEvent`]s.
pub async fn process_chat_completions_sse(
    byte_stream: ByteStream,
    tx_event: mpsc::Sender<Result<ResponseEvent, ApiError>>,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
) {
    let response_id = format!("chatcmpl-{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let text_item_id = ResponseItemId::new(&format!("{response_id}_msg"));

    let mut text_content = String::new();
    let mut text_started = false;
    let mut reasoning_started = false;
    let mut reasoning_item_id: Option<ResponseItemId> = None;
    let mut reasoning_text = String::new();
    let mut finish_reason: Option<String> = None;
    let mut token_usage: Option<TokenUsage> = None;
    // BTreeMap preserves tool call index ordering (HashMap is non-deterministic).
    let mut tool_call_buffers: BTreeMap<u64, (String, String, String)> = BTreeMap::new();

    let _ = tx_event
        .send(Ok(ResponseEvent::Created {
            response_id: None,
        }))
        .await;

    let mut chunk_count: u64 = 0;
    let mut text_delta_count: u64 = 0;
    let mut reasoning_delta_count: u64 = 0;
    let mut tool_call_index_count: u64 = 0;

    let mut stream = byte_stream.eventsource();
    let mut stream_ended = false;
    while !stream_ended {
        let start = Instant::now();
        let event = tokio::select! {
            biased;
            _ = tx_event.closed() => return,
            event = timeout(idle_timeout, stream.next()) => event,
        };
        if let Some(t) = telemetry.as_ref() {
            t.on_sse_poll(&event, start.elapsed());
        }
        let sse = match event {
            Ok(Some(Ok(sse))) => sse,
            Ok(Some(Err(e))) => {
                debug!("chat completions SSE error: {e:#}");
                let _ = tx_event
                    .send(Err(ApiError::Stream(e.to_string())))
                    .await;
                return;
            }
            Ok(None) => {
                // Upstream closed the stream. Fall through to finalization so the
                // turn still completes with whatever was accumulated.
                stream_ended = true;
                continue;
            }
            Err(_) => {
                let _ = tx_event
                    .send(Err(ApiError::Stream(
                        "chat completions stream idle timeout".into(),
                    )))
                    .await;
                return;
            }
        };
        chunk_count += 1;
        if sse.data.trim() == "[DONE]" {
            info!(
                response_id = %response_id,
                chunk_count,
                text_delta_count,
                reasoning_delta_count,
                tool_call_index_count,
                "chat completions: [DONE] received"
            );
            stream_ended = true;
            continue;
        }
        let parsed: Value = match serde_json::from_str(&sse.data) {
            Ok(value) => value,
            Err(_) => continue,
        };

        if let Some(choices) = parsed.get("choices").and_then(|c| c.as_array()) {
            for choice in choices {
                if let Some(delta) = choice.get("delta") {
                    // Text content.
                    if let Some(text) = delta.get("content").and_then(|c| c.as_str())
                        && !text.is_empty()
                    {
                        if !text_started {
                            text_started = true;
                            let _ = tx_event
                                .send(Ok(ResponseEvent::OutputItemAdded(
                                    ResponseItem::Message {
                                        id: Some(text_item_id.clone()),
                                        role: "assistant".to_string(),
                                        content: Vec::new(),
                                        phase: None,
                                        internal_chat_message_metadata_passthrough: None,
                                    },
                                )))
                                .await;
                        }
                        text_content.push_str(text);
                        text_delta_count += 1;
                        let _ = tx_event
                            .send(Ok(ResponseEvent::OutputTextDelta(text.to_string())))
                            .await;
                    }
                    // Reasoning content.
                    if let Some(reasoning) = delta
                        .get("reasoning_content")
                        .or_else(|| delta.get("reasoning"))
                        .and_then(|r| r.as_str())
                    {
                        // Even an empty field declares thinking mode. Keep a
                        // reasoning item so later tool requests echo it.
                        if !reasoning_started {
                            reasoning_started = true;
                            let rid = ResponseItemId::new("reasoning");
                            reasoning_item_id = Some(rid.clone());
                            let _ = tx_event
                                .send(Ok(ResponseEvent::OutputItemAdded(
                                    ResponseItem::Reasoning {
                                        id: Some(rid),
                                        summary: vec![],
                                        content: Some(vec![]),
                                        encrypted_content: None,
                                        internal_chat_message_metadata_passthrough: None,
                                    },
                                )))
                                .await;
                        }
                        if !reasoning.is_empty() {
                            reasoning_text.push_str(reasoning);
                            reasoning_delta_count += 1;
                            let _ = tx_event
                                .send(Ok(ResponseEvent::ReasoningContentDelta {
                                    delta: reasoning.to_string(),
                                    content_index: 0,
                                }))
                                .await;
                        }
                    }
                    // Tool call deltas — arguments stream incrementally. Emit
                    // OutputItemAdded as soon as the name arrives so the UI shows
                    // the tool call cell immediately, then finalize in
                    // OutputItemDone.
                    if let Some(tool_calls) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                        for tc in tool_calls {
                            let index = tc
                                .get("index")
                                .and_then(Value::as_u64)
                                .unwrap_or(0);
                            let entry = tool_call_buffers.entry(index).or_insert_with(|| {
                                (
                                    format!("{response_id}_tc_{index}"),
                                    String::new(),
                                    String::new(),
                                )
                            });
                            if let Some(name) = tc
                                .get("function")
                                .and_then(|f| f.get("name"))
                                .and_then(|n| n.as_str())
                                && !name.is_empty()
                                && entry.1.is_empty()
                            {
                                tool_call_index_count += 1;
                                debug!(name = %name, index, "chat completions: tool call started");
                                entry.1 = name.to_string();
                                let _ = tx_event
                                    .send(Ok(ResponseEvent::OutputItemAdded(
                                        ResponseItem::FunctionCall {
                                            id: None,
                                            name: name.to_string(),
                                            namespace: None,
                                            arguments: String::new(),
                                            encrypted_function_args: None,
                                            call_id: entry.0.clone(),
                                            internal_chat_message_metadata_passthrough: None,
                                        },
                                    )))
                                    .await;
                            }
                            if let Some(args) = tc
                                .get("function")
                                .and_then(|f| f.get("arguments"))
                                .and_then(|a| a.as_str())
                            {
                                // Stream the incremental argument fragment so
                                // consumers that render partial tool input behave
                                // the same as they do on the Responses wire.
                                let _ = tx_event
                                    .send(Ok(ResponseEvent::ToolCallInputDelta {
                                        item_id: entry.0.clone(),
                                        call_id: Some(entry.0.clone()),
                                        delta: args.to_string(),
                                    }))
                                    .await;
                                entry.2.push_str(args);
                            }
                        }
                    }
                }
                if let Some(fr) = choice.get("finish_reason").and_then(|r| r.as_str())
                    && !fr.is_empty()
                {
                    info!(finish_reason = %fr, "chat completions: finish_reason received");
                    finish_reason = Some(fr.to_string());
                }
            }
        }

        // Token usage arrives in the final chunk when
        // `stream_options.include_usage` is set.
        if let Some(usage_obj) = parsed.get("usage").and_then(|u| u.as_object()) {
            let input_tokens = usage_obj
                .get("prompt_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0) as i64;
            let output_tokens = usage_obj
                .get("completion_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0) as i64;
            let total_tokens = usage_obj
                .get("total_tokens")
                .and_then(Value::as_u64)
                .unwrap_or((input_tokens + output_tokens) as u64) as i64;
            token_usage = Some(TokenUsage {
                input_tokens,
                output_tokens,
                total_tokens,
                ..Default::default()
            });
        }
    }

    finalize_chat_turn(
        &tx_event,
        &response_id,
        text_item_id,
        text_content,
        text_started,
        reasoning_item_id,
        reasoning_started,
        reasoning_text,
        tool_call_buffers,
        finish_reason,
        token_usage,
    )
    .await;
}

/// Emit the terminal events for a Chat Completions turn.
#[allow(clippy::too_many_arguments)]
async fn finalize_chat_turn(
    tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    response_id: &str,
    text_item_id: ResponseItemId,
    text_content: String,
    _text_started: bool,
    reasoning_item_id: Option<ResponseItemId>,
    reasoning_started: bool,
    reasoning_text: String,
    tool_call_buffers: BTreeMap<u64, (String, String, String)>,
    finish_reason: Option<String>,
    token_usage: Option<TokenUsage>,
) {
    let reasoning_len = reasoning_text.len();
    let text_len = text_content.len();
    let tool_call_count = tool_call_buffers.len();

    // Close the reasoning item so accumulated text is persisted in history.
    if reasoning_started {
        let content = if reasoning_text.is_empty() {
            vec![]
        } else {
            vec![ReasoningItemContent::ReasoningText {
                text: reasoning_text,
            }]
        };
        let _ = tx_event
            .send(Ok(ResponseEvent::OutputItemDone(ResponseItem::Reasoning {
                id: reasoning_item_id,
                summary: vec![],
                content: Some(content),
                encrypted_content: None,
                internal_chat_message_metadata_passthrough: None,
            })))
            .await;
    }

    // `end_turn` is false when the model requests tool execution
    // (`finish_reason == "tool_calls"`) or was truncated by the output token
    // limit (`"length"`), so the turn loop continues instead of dropping
    // partial work.
    let end_turn = match finish_reason.as_deref() {
        Some("tool_calls") | Some("length") => Some(false),
        _ => Some(true),
    };

    info!(
        response_id = %response_id,
        finish_reason = ?finish_reason,
        end_turn = ?end_turn,
        text_len,
        reasoning_len,
        tool_calls = tool_call_count,
        token_usage = ?token_usage,
        "chat completions: turn completed"
    );

    // OutputItemAdded was already emitted on the first argument delta; finalize
    // each tool call with its complete name and accumulated arguments.
    for (_index, (call_id, name, arguments)) in tool_call_buffers {
        let _ = tx_event
            .send(Ok(ResponseEvent::OutputItemDone(
                ResponseItem::FunctionCall {
                    id: None,
                    name,
                    namespace: None,
                    arguments,
                    encrypted_function_args: None,
                    call_id,
                    internal_chat_message_metadata_passthrough: None,
                },
            )))
            .await;
    }

    if !text_content.is_empty() {
        let _ = tx_event
            .send(Ok(ResponseEvent::OutputItemDone(ResponseItem::Message {
                id: Some(text_item_id),
                role: "assistant".to_string(),
                content: vec![ContentItem::OutputText { text: text_content }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            })))
            .await;
    }

    let _ = tx_event
        .send(Ok(ResponseEvent::Completed {
            response_id: response_id.to_string(),
            token_usage: token_usage.clone(),
            end_turn,
            usage_metadata: None,
        }))
        .await;
}

#[cfg(test)]
#[path = "chat_completions_tests.rs"]
mod tests;
