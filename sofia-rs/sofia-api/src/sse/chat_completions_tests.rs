//! Conformance tests for the Chat Completions wire decoder.
//!
//! The Responses decoder has an equivalent suite in `responses.rs`. Between
//! them they pin the contract that both wire formats emit the *same*
// `ResponseEvent` vocabulary, so callers upstream of the wire layer cannot tell
//! which provider protocol produced a turn. Keep the two suites in sync when
//! adding a case: a scenario that only one wire format can express is exactly
//! the behavioural divergence this refactor exists to remove.

use super::*;
use crate::common::ResponseEvent;
use bytes::Bytes;
use http::HeaderMap;
use http::HeaderValue;
use http::StatusCode;
use serde_json::json;
use sofia_client::TransportError;

fn idle_timeout() -> Duration {
    Duration::from_secs(30)
}

fn stream_response(headers: HeaderMap, chunks: Vec<String>) -> StreamResponse {
    let body = chunks.concat();
    let bytes: ByteStream = Box::pin(futures::stream::iter(vec![Ok(Bytes::from(body))]));
    StreamResponse {
        status: StatusCode::OK,
        headers,
        bytes,
    }
}

fn sse_events(payloads: &[serde_json::Value]) -> Vec<String> {
    payloads
        .iter()
        .map(|payload| format!("data: {payload}\n\n"))
        .collect()
}

async fn collect(mut stream: ResponseStream) -> Vec<ResponseEvent> {
    let mut events = Vec::new();
    while let Some(event) = stream.rx_event.recv().await {
        events.push(event.expect("expected ok event"));
    }
    events
}

fn kinds(events: &[ResponseEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(|event| match event {
            ResponseEvent::Created { .. } => "Created",
            ResponseEvent::OutputItemAdded(_) => "OutputItemAdded",
            ResponseEvent::OutputItemDone(_) => "OutputItemDone",
            ResponseEvent::OutputTextDelta(_) => "OutputTextDelta",
            ResponseEvent::ReasoningContentDelta { .. } => "ReasoningContentDelta",
            ResponseEvent::ToolCallInputDelta { .. } => "ToolCallInputDelta",
            ResponseEvent::ServerModel(_) => "ServerModel",
            ResponseEvent::RateLimits(_) => "RateLimits",
            ResponseEvent::ModelsEtag(_) => "ModelsEtag",
            ResponseEvent::ServerReasoningIncluded(_) => "ServerReasoningIncluded",
            ResponseEvent::Completed { .. } => "Completed",
            other => {
                panic!("unexpected event in chat stream: {other:?}");
            }
        })
        .collect()
}

/// Header-derived events must reach consumers on the chat wire too, not just
/// the Responses one. Downstream code (`compact.rs`, `turn.rs`) reacts to these.
#[tokio::test]
async fn chat_stream_emits_header_events() {
    let mut headers = HeaderMap::new();
    headers.insert("x-request-id", HeaderValue::from_static("req-chat-1"));
    headers.insert("openai-model", HeaderValue::from_static("test-model"));
    headers.insert("X-Models-Etag", HeaderValue::from_static("etag-1"));
    headers.insert("x-reasoning-included", HeaderValue::from_static("1"));

    let stream = spawn_chat_completions_stream(
        stream_response(headers, Vec::new()),
        idle_timeout(),
        /*telemetry*/ None,
        /*turn_state*/ None,
    );

    // The upstream request id must be captured exactly as the Responses wire
    // does, so provider errors are diagnosable on chat providers.
    assert_eq!(stream.upstream_request_id.as_deref(), Some("req-chat-1"));

    let events = collect(stream).await;
    let kinds = kinds(&events);
    // `parse_all_rate_limits` always yields the default `sofia` snapshot, so it
    // is present here exactly as it is on the Responses wire.
    assert_eq!(
        kinds,
        vec![
            "ServerModel",
            "RateLimits",
            "ModelsEtag",
            "ServerReasoningIncluded",
            "Created",
            "Completed",
        ]
    );
    assert!(matches!(
        &events[0],
        ResponseEvent::ServerModel(model) if model == "test-model"
    ));
}

#[tokio::test]
async fn chat_stream_captures_turn_state_header() {
    let turn_state = Arc::new(OnceLock::new());
    let mut headers = HeaderMap::new();
    headers.insert("x-sofia-turn-state", HeaderValue::from_static("state-abc"));

    let _ = spawn_chat_completions_stream(
        stream_response(headers, Vec::new()),
        idle_timeout(),
        /*telemetry*/ None,
        Some(Arc::clone(&turn_state)),
    );

    assert_eq!(turn_state.get().map(String::as_str), Some("state-abc"));
}

#[tokio::test]
async fn chat_stream_text_and_completion() {
    let chunks = sse_events(&[
        json!({"choices":[{"delta":{"content":"Hello"}}]}),
        json!({"choices":[{"delta":{"content":" world"}}]}),
        json!({"choices":[{"delta":{},"finish_reason":"stop"}]}),
        json!({"usage":{"prompt_tokens":7,"completion_tokens":3,"total_tokens":10}}),
    ]);

    let events = collect(spawn_chat_completions_stream(
        stream_response(HeaderMap::new(), chunks),
        idle_timeout(),
        /*telemetry*/ None,
        /*turn_state*/ None,
    ))
    .await;

    assert_eq!(
        kinds(&events),
        vec![
            "RateLimits",
            "Created",
            "OutputItemAdded",
            "OutputTextDelta",
            "OutputTextDelta",
            "OutputItemDone",
            "Completed",
        ]
    );

    let text: String = events
        .iter()
        .filter_map(|event| match event {
            ResponseEvent::OutputTextDelta(delta) => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "Hello world");

    let ResponseEvent::Completed {
        token_usage,
        end_turn,
        ..
    } = events.last().expect("completed event")
    else {
        panic!("expected completed event");
    };
    assert_eq!(end_turn, &Some(true));
    let usage = token_usage
        .as_ref()
        .expect("token usage should be reported");
    assert_eq!(usage.input_tokens, 7);
    assert_eq!(usage.output_tokens, 3);
    assert_eq!(usage.total_tokens, 10);
}

#[tokio::test]
async fn chat_stream_tool_call_accumulates_arguments() {
    let chunks = sse_events(&[
        json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"shell"}}]}}]}),
        json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"cmd\":"}}]}}]}),
        json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"ls\"}"}}]}}]}),
        json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}),
    ]);

    let events = collect(spawn_chat_completions_stream(
        stream_response(HeaderMap::new(), chunks),
        idle_timeout(),
        /*telemetry*/ None,
        /*turn_state*/ None,
    ))
    .await;

    // `tool_calls` must not end the turn, otherwise the agent loop stops before
    // running the tool.
    let ResponseEvent::Completed { end_turn, .. } = events.last().expect("completed") else {
        panic!("expected completed event");
    };
    assert_eq!(end_turn, &Some(false));

    let done: Vec<&ResponseItem> = events
        .iter()
        .filter_map(|event| match event {
            ResponseEvent::OutputItemDone(item @ ResponseItem::FunctionCall { .. }) => Some(item),
            _ => None,
        })
        .collect();
    assert_eq!(done.len(), 1);
    let ResponseItem::FunctionCall {
        name, arguments, ..
    } = done[0]
    else {
        panic!("expected function call");
    };
    assert_eq!(name, "shell");
    assert_eq!(arguments, r#"{"cmd":"ls"}"#);

    // Argument fragments must stream as ToolCallInputDelta, matching the
    // Responses wire, so partial-tool-input consumers behave the same.
    let deltas: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            ResponseEvent::ToolCallInputDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(deltas, vec![r#"{"cmd":"#, r#""ls"}"#]);
}

#[tokio::test]
async fn chat_stream_length_finish_reason_keeps_turn_open() {
    let chunks = sse_events(&[
        json!({"choices":[{"delta":{"content":"partial"},"finish_reason":"length"}]}),
    ]);

    let events = collect(spawn_chat_completions_stream(
        stream_response(HeaderMap::new(), chunks),
        idle_timeout(),
        /*telemetry*/ None,
        /*turn_state*/ None,
    ))
    .await;

    let ResponseEvent::Completed { end_turn, .. } = events.last().expect("completed") else {
        panic!("expected completed event");
    };
    assert_eq!(
        end_turn,
        &Some(false),
        "a truncated response is incomplete and must not end the turn"
    );
}

#[tokio::test]
async fn chat_stream_reasoning_is_captured_for_echo() {
    let chunks = sse_events(&[
        json!({"choices":[{"delta":{"reasoning_content":"thinking..."}}]}),
        json!({"choices":[{"delta":{"content":"answer"}}]}),
        json!({"choices":[{"delta":{},"finish_reason":"stop"}]}),
    ]);

    let events = collect(spawn_chat_completions_stream(
        stream_response(HeaderMap::new(), chunks),
        idle_timeout(),
        /*telemetry*/ None,
        /*turn_state*/ None,
    ))
    .await;

    // Thinking-mode providers require the captured reasoning to be echoed back
    // on the next request, so the Reasoning item must be persisted.
    let done = events.iter().any(|event| {
        matches!(event, ResponseEvent::OutputItemDone(ResponseItem::Reasoning { content: Some(content), .. })
            if content.iter().any(|part| matches!(part, ReasoningItemContent::ReasoningText { text } if text == "thinking...")))
    });
    assert!(
        done,
        "reasoning text must be preserved in the completed Reasoning item"
    );
}

#[tokio::test]
async fn chat_stream_ignores_unparsable_payloads() {
    let chunks = vec![
        "data: not json\n\n".to_string(),
        "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n".to_string(),
        "data: [DONE]\n\n".to_string(),
    ];

    let events = collect(spawn_chat_completions_stream(
        stream_response(HeaderMap::new(), chunks),
        idle_timeout(),
        /*telemetry*/ None,
        /*turn_state*/ None,
    ))
    .await;

    let text: String = events
        .iter()
        .filter_map(|event| match event {
            ResponseEvent::OutputTextDelta(delta) => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "ok", "a malformed frame must not abort the stream");
    assert_eq!(kinds(&events).last(), Some(&"Completed"));
}

#[tokio::test]
async fn chat_stream_transport_error_surfaces_as_stream_error() {
    let bytes: ByteStream = Box::pin(futures::stream::iter(vec![Err(TransportError::Http {
        status: StatusCode::BAD_GATEWAY,
        url: Some("https://openrouter.ai/api/v1/chat/completions".to_string()),
        headers: None,
        body: Some("upstream returned 502".to_string()),
    })]));
    let stream_response = StreamResponse {
        status: StatusCode::OK,
        headers: HeaderMap::new(),
        bytes,
    };

    let mut stream = spawn_chat_completions_stream(
        stream_response,
        idle_timeout(),
        /*telemetry*/ None,
        /*turn_state*/ None,
    );

    let mut saw_error = false;
    while let Some(event) = stream.rx_event.recv().await {
        if event.is_err() {
            saw_error = true;
            break;
        }
    }
    assert!(
        saw_error,
        "a transport failure must be reported to the consumer"
    );
}
