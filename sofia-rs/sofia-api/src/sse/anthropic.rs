//! Anthropic Messages (`/v1/messages`) SSE decoder.
//!
//! Emits the same [`ResponseEvent`] vocabulary as the Responses and Chat
//! Completions decoders so everything downstream of the wire layer behaves
//! identically regardless of which provider protocol produced a turn.
//!
//! Anthropic streams named events (`message_start`, `content_block_delta`, ...)
//! whose payloads are tagged by `type`; this module folds those into the same
//! item/delta events the engine already consumes. Freeform tools are bridged as
//! function tools carrying an `input` string, so when a `tool_use` block names a
//! freeform tool the accumulated JSON input is unwrapped back into a
//! [`ResponseItem::CustomToolCall`] the tool runtime expects.

use crate::common::ResponseEvent;
use crate::common::ResponseStream;
use crate::error::ApiError;
use crate::telemetry::SseTelemetry;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde_json::Value;
use sofia_client::ByteStream;
use sofia_client::StreamResponse;
use sofia_protocol::ResponseItemId;
use sofia_protocol::models::ContentItem;
use sofia_protocol::models::MessagePhase;
use sofia_protocol::models::ReasoningItemContent;
use sofia_protocol::models::ResponseItem;
use sofia_protocol::protocol::TokenUsage;
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio::time::timeout;

/// Event channel capacity for Anthropic streams.
const ANTHROPIC_EVENT_CHANNEL_CAPACITY: usize = 1600;

/// A `tool_use` content block accumulated across `input_json_delta` events.
#[derive(Debug, Default)]
struct ToolUseBuffer {
    call_id: String,
    name: String,
    initial_input: Value,
    partial_json: String,
}

/// Spawn an Anthropic Messages event stream.
pub fn spawn_anthropic_stream(
    stream_response: StreamResponse,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
    turn_state: Option<Arc<OnceLock<String>>>,
    freeform_tool_names: Arc<HashSet<String>>,
) -> ResponseStream {
    crate::sse::header_events::spawn_wire_stream(
        stream_response,
        idle_timeout,
        telemetry,
        turn_state,
        ANTHROPIC_EVENT_CHANNEL_CAPACITY,
        move |bytes, tx_event, idle_timeout, telemetry, _treatment| {
            let freeform_tool_names = Arc::clone(&freeform_tool_names);
            async move {
                process_anthropic_sse(
                    bytes,
                    tx_event,
                    idle_timeout,
                    telemetry,
                    freeform_tool_names,
                )
                .await
            }
        },
    )
}

#[derive(Debug)]
enum AnthropicBlock {
    Text { id: ResponseItemId, text: String },
    Thinking { id: ResponseItemId, block: Value },
    Tool(ToolUseBuffer),
}

/// Translate ordered Messages API blocks without discarding signed thinking.
pub async fn process_anthropic_sse(
    byte_stream: ByteStream,
    tx_event: mpsc::Sender<Result<ResponseEvent, ApiError>>,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
    freeform_tool_names: Arc<HashSet<String>>,
) {
    let mut response_id = format!("msg_{}", uuid::Uuid::new_v4());
    let mut blocks = BTreeMap::<u64, AnthropicBlock>::new();
    let mut stop_reason: Option<String> = None;
    let mut usage = AnthropicUsage {
        input_tokens: 0,
        cached_input_tokens: 0,
        cache_write_input_tokens: 0,
        output_tokens: 0,
    };
    let _ = tx_event
        .send(Ok(ResponseEvent::Created { response_id: None }))
        .await;
    let mut stream = byte_stream.eventsource();
    loop {
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
            Ok(Some(Err(error))) => {
                let _ = tx_event
                    .send(Err(ApiError::Stream(error.to_string())))
                    .await;
                return;
            }
            Ok(None) => {
                let _ = tx_event
                    .send(Err(ApiError::Stream(
                        "Anthropic stream ended before message_stop".to_string(),
                    )))
                    .await;
                return;
            }
            Err(_) => {
                let _ = tx_event
                    .send(Err(ApiError::Stream(
                        "Anthropic stream idle timeout".to_string(),
                    )))
                    .await;
                return;
            }
        };
        let parsed: Value = match serde_json::from_str(&sse.data) {
            Ok(value) => value,
            Err(error) => {
                let _ = tx_event
                    .send(Err(ApiError::Stream(format!(
                        "invalid Anthropic event JSON: {error}"
                    ))))
                    .await;
                return;
            }
        };
        let kind = parsed
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or(&sse.event);
        let index = parsed.get("index").and_then(Value::as_u64).unwrap_or(0);
        match kind {
            "message_start" => {
                if let Some(message) = parsed.get("message") {
                    if let Some(id) = message.get("id").and_then(Value::as_str) {
                        response_id = id.to_string();
                    }
                    if let Some(model) = message.get("model").and_then(Value::as_str) {
                        let _ = tx_event
                            .send(Ok(ResponseEvent::ServerModel(model.to_string())))
                            .await;
                    }
                    if let Some(value) = message.get("usage") {
                        usage.input_tokens = u64_field(value, "input_tokens") as i64;
                        usage.cached_input_tokens =
                            u64_field(value, "cache_read_input_tokens") as i64;
                        usage.cache_write_input_tokens =
                            u64_field(value, "cache_creation_input_tokens") as i64;
                        usage.output_tokens = u64_field(value, "output_tokens") as i64;
                    }
                }
            }
            "content_block_start" => {
                let Some(block) = parsed.get("content_block") else {
                    continue;
                };
                let id = ResponseItemId::new(&format!("{response_id}_{index}"));
                match block["type"].as_str() {
                    Some("text") => {
                        let text = block["text"].as_str().unwrap_or_default().to_string();
                        let _ = tx_event
                            .send(Ok(ResponseEvent::OutputItemAdded(ResponseItem::Message {
                                id: Some(id.clone()),
                                role: "assistant".to_string(),
                                content: vec![],
                                phase: None,
                                internal_chat_message_metadata_passthrough: None,
                            })))
                            .await;
                        if !text.is_empty() {
                            let _ = tx_event
                                .send(Ok(ResponseEvent::OutputTextDelta(text.clone())))
                                .await;
                        }
                        blocks.insert(index, AnthropicBlock::Text { id, text });
                    }
                    Some("thinking" | "redacted_thinking") => {
                        let _ = tx_event
                            .send(Ok(ResponseEvent::OutputItemAdded(
                                ResponseItem::Reasoning {
                                    id: Some(id.clone()),
                                    summary: vec![],
                                    content: Some(vec![]),
                                    encrypted_content: None,
                                    internal_chat_message_metadata_passthrough: None,
                                },
                            )))
                            .await;
                        if let Some(text) =
                            block["thinking"].as_str().filter(|text| !text.is_empty())
                        {
                            let _ = tx_event
                                .send(Ok(ResponseEvent::ReasoningContentDelta {
                                    delta: text.to_string(),
                                    content_index: 0,
                                }))
                                .await;
                        }
                        blocks.insert(
                            index,
                            AnthropicBlock::Thinking {
                                id,
                                block: block.clone(),
                            },
                        );
                    }
                    Some("tool_use") => {
                        let call_id = block["id"].as_str().unwrap_or_default().to_string();
                        let name = block["name"].as_str().unwrap_or_default().to_string();
                        emit_tool_call_added(&tx_event, &call_id, &name, &freeform_tool_names)
                            .await;
                        blocks.insert(
                            index,
                            AnthropicBlock::Tool(ToolUseBuffer {
                                call_id,
                                name,
                                initial_input: block.get("input").cloned().unwrap_or(Value::Null),
                                partial_json: String::new(),
                            }),
                        );
                    }
                    _ => {}
                }
            }
            "content_block_delta" => {
                let Some(delta) = parsed.get("delta") else {
                    continue;
                };
                match delta["type"].as_str() {
                    Some("text_delta") => {
                        if let std::collections::btree_map::Entry::Vacant(e) = blocks.entry(index) {
                            let id = ResponseItemId::new(&format!("{response_id}_{index}"));
                            let _ = tx_event
                                .send(Ok(ResponseEvent::OutputItemAdded(ResponseItem::Message {
                                    id: Some(id.clone()),
                                    role: "assistant".to_string(),
                                    content: vec![],
                                    phase: None,
                                    internal_chat_message_metadata_passthrough: None,
                                })))
                                .await;
                            e.insert(AnthropicBlock::Text {
                                id,
                                text: String::new(),
                            });
                        }
                        if let Some(AnthropicBlock::Text { text, .. }) = blocks.get_mut(&index)
                            && let Some(value) = delta["text"].as_str()
                        {
                            text.push_str(value);
                            let _ = tx_event
                                .send(Ok(ResponseEvent::OutputTextDelta(value.to_string())))
                                .await;
                        }
                    }
                    Some("thinking_delta" | "signature_delta") => {
                        if let Some(AnthropicBlock::Thinking { block, .. }) = blocks.get_mut(&index)
                        {
                            let field = if delta["type"] == "thinking_delta" {
                                "thinking"
                            } else {
                                "signature"
                            };
                            if let Some(value) = delta[field].as_str() {
                                let accumulated =
                                    format!("{}{value}", block[field].as_str().unwrap_or_default());
                                block[field] = Value::String(accumulated);
                                if field == "thinking" {
                                    let _ = tx_event
                                        .send(Ok(ResponseEvent::ReasoningContentDelta {
                                            delta: value.to_string(),
                                            content_index: 0,
                                        }))
                                        .await;
                                }
                            }
                        }
                    }
                    Some("input_json_delta") => {
                        if let Some(AnthropicBlock::Tool(buffer)) = blocks.get_mut(&index)
                            && let Some(value) = delta["partial_json"].as_str()
                        {
                            buffer.partial_json.push_str(value);
                            let _ = tx_event
                                .send(Ok(ResponseEvent::ToolCallInputDelta {
                                    item_id: buffer.call_id.clone(),
                                    call_id: Some(buffer.call_id.clone()),
                                    delta: value.to_string(),
                                }))
                                .await;
                        }
                    }
                    _ => {}
                }
            }
            "message_delta" => {
                if let Some(reason) = parsed["delta"]["stop_reason"].as_str() {
                    stop_reason = Some(reason.to_string());
                }
                if let Some(value) = parsed.get("usage") {
                    if value.get("output_tokens").is_some() {
                        usage.output_tokens = u64_field(value, "output_tokens") as i64;
                    }
                    if value.get("input_tokens").is_some() {
                        usage.input_tokens = u64_field(value, "input_tokens") as i64;
                    }
                    if value.get("cache_read_input_tokens").is_some() {
                        usage.cached_input_tokens =
                            u64_field(value, "cache_read_input_tokens") as i64;
                    }
                    if value.get("cache_creation_input_tokens").is_some() {
                        usage.cache_write_input_tokens =
                            u64_field(value, "cache_creation_input_tokens") as i64;
                    }
                }
            }
            "message_stop" => break,
            "error" => {
                let message = parsed["error"]["message"]
                    .as_str()
                    .unwrap_or("Anthropic stream error")
                    .to_string();
                let error = match parsed["error"]["type"].as_str() {
                    Some("overloaded_error") => ApiError::ServerOverloaded,
                    Some("rate_limit_error") => ApiError::RateLimitExceeded {
                        message,
                        delay: None,
                    },
                    Some("invalid_request_error") => ApiError::Api {
                        status: http::StatusCode::BAD_REQUEST,
                        message: parsed.to_string(),
                    },
                    _ => ApiError::Stream(message),
                };
                let _ = tx_event.send(Err(error)).await;
                return;
            }
            "content_block_stop" | "ping" => {}
            _ => {}
        }
    }
    let end_turn = !matches!(
        stop_reason.as_deref(),
        Some("tool_use" | "max_tokens" | "pause_turn")
    );
    let has_tools = blocks
        .values()
        .any(|block| matches!(block, AnthropicBlock::Tool(_)));
    let phase = if end_turn && !has_tools {
        MessagePhase::FinalAnswer
    } else {
        MessagePhase::Commentary
    };
    for (_, block) in blocks {
        let item = match block {
            AnthropicBlock::Text { id, text } => ResponseItem::Message {
                id: Some(id),
                role: "assistant".to_string(),
                content: vec![ContentItem::OutputText { text }],
                phase: Some(phase.clone()),
                internal_chat_message_metadata_passthrough: None,
            },
            AnthropicBlock::Thinking { id, block } => {
                let text = block["thinking"].as_str().unwrap_or_default().to_string();
                let encoded = format!(
                    "{}{}",
                    crate::ANTHROPIC_THINKING_PREFIX,
                    serde_json::json!([block])
                );
                ResponseItem::Reasoning {
                    id: Some(id),
                    summary: vec![],
                    content: Some(if text.is_empty() {
                        vec![]
                    } else {
                        vec![ReasoningItemContent::ReasoningText { text }]
                    }),
                    encrypted_content: Some(encoded),
                    internal_chat_message_metadata_passthrough: None,
                }
            }
            AnthropicBlock::Tool(buffer) => {
                let input = if buffer.partial_json.trim().is_empty() {
                    buffer.initial_input
                } else {
                    match serde_json::from_str::<Value>(&buffer.partial_json) {
                        Ok(value) => value,
                        Err(error) => {
                            let _ = tx_event
                                .send(Err(ApiError::Stream(format!(
                                    "invalid Anthropic tool input: {error}"
                                ))))
                                .await;
                            return;
                        }
                    }
                };
                if freeform_tool_names.contains(&buffer.name) {
                    ResponseItem::CustomToolCall {
                        id: None,
                        status: None,
                        call_id: buffer.call_id,
                        name: buffer.name,
                        namespace: None,
                        input: unwrap_freeform_input(input),
                        internal_chat_message_metadata_passthrough: None,
                    }
                } else {
                    ResponseItem::FunctionCall {
                        id: None,
                        name: buffer.name,
                        namespace: None,
                        arguments: input.to_string(),
                        encrypted_function_args: Some(Vec::new()),
                        call_id: buffer.call_id,
                        internal_chat_message_metadata_passthrough: None,
                    }
                }
            }
        };
        let _ = tx_event.send(Ok(ResponseEvent::OutputItemDone(item))).await;
    }
    let token_usage = (usage.input_tokens > 0
        || usage.output_tokens > 0
        || usage.cached_input_tokens > 0
        || usage.cache_write_input_tokens > 0)
        .then(|| usage.into_token_usage());
    let _ = tx_event
        .send(Ok(ResponseEvent::Completed {
            response_id,
            token_usage,
            end_turn: Some(end_turn),
            usage_metadata: None,
        }))
        .await;
}

async fn emit_tool_call_added(
    tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    call_id: &str,
    name: &str,
    freeform_tool_names: &HashSet<String>,
) {
    let item = if freeform_tool_names.contains(name) {
        ResponseItem::CustomToolCall {
            id: None,
            status: None,
            call_id: call_id.to_string(),
            name: name.to_string(),
            namespace: None,
            input: String::new(),
            internal_chat_message_metadata_passthrough: None,
        }
    } else {
        ResponseItem::FunctionCall {
            id: None,
            name: name.to_string(),
            namespace: None,
            arguments: String::new(),
            encrypted_function_args: Some(Vec::new()),
            call_id: call_id.to_string(),
            internal_chat_message_metadata_passthrough: None,
        }
    };
    let _ = tx_event
        .send(Ok(ResponseEvent::OutputItemAdded(item)))
        .await;
}

/// Unwrap the `{"input": "..."}` object used to bridge a freeform tool through
/// Anthropic's function-tool schema back into the raw text the tool expects.
fn unwrap_freeform_input(input: Value) -> String {
    match input {
        Value::Object(mut map) => match map.remove("input") {
            Some(Value::String(text)) => text,
            Some(other) => other.to_string(),
            None => Value::Object(map).to_string(),
        },
        Value::String(text) => text,
        other => other.to_string(),
    }
}

fn u64_field(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

struct AnthropicUsage {
    input_tokens: i64,
    cached_input_tokens: i64,
    cache_write_input_tokens: i64,
    output_tokens: i64,
}

impl AnthropicUsage {
    fn into_token_usage(self) -> TokenUsage {
        // Anthropic reports non-cached input separately from cache reads and
        // writes; the engine's `input_tokens` is the total prompt size, so the
        // three are summed while the cache components stay available.
        let total_input =
            self.input_tokens + self.cached_input_tokens + self.cache_write_input_tokens;
        TokenUsage {
            input_tokens: total_input,
            cached_input_tokens: self.cached_input_tokens,
            cache_write_input_tokens: self.cache_write_input_tokens,
            output_tokens: self.output_tokens,
            total_tokens: total_input + self.output_tokens,
            ..Default::default()
        }
    }
}

#[cfg(test)]
#[path = "anthropic_tests.rs"]
mod tests;
