//! Conformance tests for the Anthropic Messages wire decoder.
//!
//! Mirrors `chat_completions_tests.rs`: both decoders must emit the same
//! `ResponseEvent` vocabulary so callers upstream of the wire layer cannot tell
//! which provider protocol produced a turn.

use super::*;
use crate::common::ResponseEvent;
use bytes::Bytes;
use http::HeaderMap;
use http::StatusCode;
use serde_json::json;
use std::collections::HashSet;

fn idle_timeout() -> Duration {
    Duration::from_secs(30)
}

fn stream_response(chunks: Vec<String>) -> StreamResponse {
    let body = chunks.concat();
    let bytes: ByteStream = Box::pin(futures::stream::iter(vec![Ok(Bytes::from(body))]));
    StreamResponse {
        status: StatusCode::OK,
        headers: HeaderMap::new(),
        bytes,
    }
}

fn sse_events(events: &[(&str, serde_json::Value)]) -> Vec<String> {
    events
        .iter()
        .map(|(event, payload)| format!("event: {event}\ndata: {payload}\n\n"))
        .collect()
}

fn no_freeform() -> Arc<HashSet<String>> {
    Arc::new(HashSet::new())
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
            other => panic!("unexpected event in anthropic stream: {other:?}"),
        })
        .collect()
}

#[tokio::test]
async fn anthropic_stream_text_and_completion() {
    let chunks = sse_events(&[
        (
            "message_start",
            json!({"message":{"id":"msg_1","model":"claude-test","usage":{"input_tokens":7,"output_tokens":0}}}),
        ),
        (
            "content_block_start",
            json!({"index":0,"content_block":{"type":"text","text":""}}),
        ),
        (
            "content_block_delta",
            json!({"index":0,"delta":{"type":"text_delta","text":"Hello"}}),
        ),
        (
            "content_block_delta",
            json!({"index":0,"delta":{"type":"text_delta","text":" world"}}),
        ),
        ("content_block_stop", json!({"index":0})),
        (
            "message_delta",
            json!({"delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}),
        ),
        ("message_stop", json!({})),
    ]);

    let events = collect(spawn_anthropic_stream(
        stream_response(chunks),
        idle_timeout(),
        /*telemetry*/ None,
        /*turn_state*/ None,
        no_freeform(),
    ))
    .await;

    assert_eq!(
        kinds(&events),
        vec![
            "RateLimits",
            "Created",
            "ServerModel",
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
        response_id,
        token_usage,
        end_turn,
        ..
    } = events.last().expect("completed event")
    else {
        panic!("expected completed event");
    };
    assert_eq!(response_id, "msg_1");
    assert_eq!(end_turn, &Some(true));
    let usage = token_usage
        .as_ref()
        .expect("token usage should be reported");
    assert_eq!(usage.input_tokens, 7);
    assert_eq!(usage.output_tokens, 3);
    assert_eq!(usage.total_tokens, 10);
}

#[tokio::test]
async fn anthropic_stream_tool_use_accumulates_input_json() {
    let chunks = sse_events(&[
        (
            "message_start",
            json!({"message":{"id":"msg_2","model":"claude-test","usage":{"input_tokens":5}}}),
        ),
        (
            "content_block_start",
            json!({"index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"shell","input":{}}}),
        ),
        (
            "content_block_delta",
            json!({"index":0,"delta":{"type":"input_json_delta","partial_json":"{\"cmd\":"}}),
        ),
        (
            "content_block_delta",
            json!({"index":0,"delta":{"type":"input_json_delta","partial_json":"\"ls\"}"}}),
        ),
        ("content_block_stop", json!({"index":0})),
        (
            "message_delta",
            json!({"delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":9}}),
        ),
        ("message_stop", json!({})),
    ]);

    let events = collect(spawn_anthropic_stream(
        stream_response(chunks),
        idle_timeout(),
        /*telemetry*/ None,
        /*turn_state*/ None,
        no_freeform(),
    ))
    .await;

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
        name,
        arguments,
        call_id,
        ..
    } = done[0]
    else {
        panic!("expected function call");
    };
    assert_eq!(name, "shell");
    assert_eq!(call_id, "toolu_1");
    assert_eq!(arguments, r#"{"cmd":"ls"}"#);

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
async fn anthropic_stream_freeform_tool_maps_to_custom_tool_call() {
    let mut freeform = HashSet::new();
    freeform.insert("apply_patch".to_string());
    let chunks = sse_events(&[
        (
            "message_start",
            json!({"message":{"id":"msg_3","model":"claude-test","usage":{"input_tokens":5}}}),
        ),
        (
            "content_block_start",
            json!({"index":0,"content_block":{"type":"tool_use","id":"toolu_2","name":"apply_patch","input":{}}}),
        ),
        (
            "content_block_delta",
            json!({"index":0,"delta":{"type":"input_json_delta","partial_json":"{\"input\":\"*** Begin Patch\"}"}}),
        ),
        ("content_block_stop", json!({"index":0})),
        (
            "message_delta",
            json!({"delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":9}}),
        ),
        ("message_stop", json!({})),
    ]);

    let events = collect(spawn_anthropic_stream(
        stream_response(chunks),
        idle_timeout(),
        /*telemetry*/ None,
        /*turn_state*/ None,
        Arc::new(freeform),
    ))
    .await;

    let done: Vec<&ResponseItem> = events
        .iter()
        .filter_map(|event| match event {
            ResponseEvent::OutputItemDone(item @ ResponseItem::CustomToolCall { .. }) => Some(item),
            _ => None,
        })
        .collect();
    assert_eq!(done.len(), 1);
    let ResponseItem::CustomToolCall {
        name,
        input,
        call_id,
        ..
    } = done[0]
    else {
        panic!("expected custom tool call");
    };
    assert_eq!(name, "apply_patch");
    assert_eq!(call_id, "toolu_2");
    assert_eq!(input, "*** Begin Patch");
}

#[tokio::test]
async fn anthropic_stream_thinking_delta_is_captured() {
    let chunks = sse_events(&[
        (
            "message_start",
            json!({"message":{"id":"msg_4","model":"claude-test","usage":{"input_tokens":5}}}),
        ),
        (
            "content_block_start",
            json!({"index":0,"content_block":{"type":"thinking","thinking":""}}),
        ),
        (
            "content_block_delta",
            json!({"index":0,"delta":{"type":"thinking_delta","thinking":"weighing options"}}),
        ),
        ("content_block_stop", json!({"index":0})),
        (
            "content_block_delta",
            json!({"index":1,"delta":{"type":"text_delta","text":"answer"}}),
        ),
        (
            "message_delta",
            json!({"delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}),
        ),
        ("message_stop", json!({})),
    ]);

    let events = collect(spawn_anthropic_stream(
        stream_response(chunks),
        idle_timeout(),
        /*telemetry*/ None,
        /*turn_state*/ None,
        no_freeform(),
    ))
    .await;

    let reasoning_deltas: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            ResponseEvent::ReasoningContentDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(reasoning_deltas, vec!["weighing options"]);

    let done = events.iter().any(|event| {
        matches!(event, ResponseEvent::OutputItemDone(ResponseItem::Reasoning { content: Some(content), .. })
            if content.iter().any(|part| matches!(part, ReasoningItemContent::ReasoningText { text } if text == "weighing options")))
    });
    assert!(
        done,
        "reasoning text must be preserved in the completed Reasoning item"
    );
}

#[tokio::test]
async fn anthropic_stream_max_tokens_keeps_turn_open() {
    let chunks = sse_events(&[
        (
            "message_start",
            json!({"message":{"id":"msg_5","usage":{"input_tokens":1}}}),
        ),
        (
            "content_block_delta",
            json!({"index":0,"delta":{"type":"text_delta","text":"partial"}}),
        ),
        (
            "message_delta",
            json!({"delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":1}}),
        ),
        ("message_stop", json!({})),
    ]);

    let events = collect(spawn_anthropic_stream(
        stream_response(chunks),
        idle_timeout(),
        /*telemetry*/ None,
        /*turn_state*/ None,
        no_freeform(),
    ))
    .await;

    let ResponseEvent::Completed { end_turn, .. } = events.last().expect("completed") else {
        panic!("expected completed event");
    };
    assert_eq!(end_turn, &Some(false));
}

#[tokio::test]
async fn anthropic_stream_error_event_surfaces_error() {
    let chunks = sse_events(&[
        (
            "message_start",
            json!({"message":{"id":"msg_6","usage":{"input_tokens":1}}}),
        ),
        (
            "error",
            json!({"error":{"type":"overloaded_error","message":"Overloaded"}}),
        ),
    ]);

    let mut stream = spawn_anthropic_stream(
        stream_response(chunks),
        idle_timeout(),
        /*telemetry*/ None,
        /*turn_state*/ None,
        no_freeform(),
    );

    let mut saw_error = false;
    while let Some(event) = stream.rx_event.recv().await {
        if let Err(err) = event {
            saw_error = true;
            assert!(matches!(err, ApiError::ServerOverloaded));
            break;
        }
    }
    assert!(saw_error, "an error event must be reported to the consumer");
}

#[tokio::test]
async fn anthropic_truncated_stream_is_not_completed() {
    let chunks = sse_events(&[
        ("message_start", json!({"message":{"id":"truncated"}})),
        (
            "content_block_delta",
            json!({"index":0,"delta":{"type":"text_delta","text":"partial"}}),
        ),
    ]);
    let mut stream = spawn_anthropic_stream(
        stream_response(chunks),
        idle_timeout(),
        /*telemetry*/ None,
        /*turn_state*/ None,
        no_freeform(),
    );
    let mut completed = false;
    let mut error = None;
    while let Some(event) = stream.rx_event.recv().await {
        match event {
            Ok(ResponseEvent::Completed { .. }) => completed = true,
            Err(value) => error = Some(value.to_string()),
            _ => {}
        }
    }
    assert!(!completed);
    assert!(
        error
            .as_deref()
            .is_some_and(|value| value.contains("before message_stop"))
    );
}

#[tokio::test]
async fn anthropic_preserves_signed_and_redacted_thinking_in_block_order() {
    let signed = json!({"type":"thinking","thinking":"reason","signature":"signature-one"});
    let redacted = json!({"type":"redacted_thinking","data":"opaque"});
    let chunks = sse_events(&[
        ("message_start", json!({"message":{"id":"signed"}})),
        (
            "content_block_start",
            json!({"index":0,"content_block":{"type":"thinking","thinking":""}}),
        ),
        (
            "content_block_delta",
            json!({"index":0,"delta":{"type":"thinking_delta","thinking":"reason"}}),
        ),
        (
            "content_block_delta",
            json!({"index":0,"delta":{"type":"signature_delta","signature":"signature-"}}),
        ),
        (
            "content_block_delta",
            json!({"index":0,"delta":{"type":"signature_delta","signature":"one"}}),
        ),
        (
            "content_block_start",
            json!({"index":1,"content_block":{"type":"tool_use","id":"call","name":"read","input":{}}}),
        ),
        (
            "content_block_start",
            json!({"index":2,"content_block":redacted}),
        ),
        ("message_delta", json!({"delta":{"stop_reason":"tool_use"}})),
        ("message_stop", json!({})),
    ]);
    let events = collect(spawn_anthropic_stream(
        stream_response(chunks),
        idle_timeout(),
        /*telemetry*/ None,
        /*turn_state*/ None,
        no_freeform(),
    ))
    .await;
    let done: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            ResponseEvent::OutputItemDone(item) => Some(item),
            _ => None,
        })
        .collect();
    assert_eq!(done.len(), 3);
    let ResponseItem::Reasoning {
        encrypted_content: Some(first),
        ..
    } = done[0]
    else {
        panic!("signed thinking first");
    };
    assert_eq!(crate::decode_anthropic_thinking(first), Some(vec![signed]));
    assert!(matches!(done[1], ResponseItem::FunctionCall { call_id, .. } if call_id == "call"));
    let ResponseItem::Reasoning {
        encrypted_content: Some(last),
        ..
    } = done[2]
    else {
        panic!("redacted thinking last");
    };
    assert_eq!(crate::decode_anthropic_thinking(last), Some(vec![redacted]));
}

#[tokio::test]
async fn anthropic_sse_context_overflow_enters_existing_compaction_error_path() {
    let chunks = sse_events(&[(
        "error",
        json!({"error":{"type":"invalid_request_error","message":"prompt is too long: 259628 tokens > 200000 maximum"}}),
    )]);
    let mut stream = spawn_anthropic_stream(
        stream_response(chunks),
        idle_timeout(),
        /*telemetry*/ None,
        /*turn_state*/ None,
        no_freeform(),
    );
    let mut mapped = None;
    while let Some(event) = stream.rx_event.recv().await {
        if let Err(error) = event {
            mapped = Some(crate::api_bridge::map_api_error(error));
        }
    }
    assert!(mapped.as_ref().is_some_and(|error| matches!(
        error.details(),
        sofia_protocol::error::CodexErrorDetails::ContextWindowExceeded
    )));
}

#[tokio::test]
async fn anthropic_cache_usage_counts_reads_and_writes_once() {
    let chunks = sse_events(&[
        (
            "message_start",
            json!({"message":{"id":"usage","usage":{"input_tokens":5,"cache_read_input_tokens":30,"cache_creation_input_tokens":20}}}),
        ),
        (
            "message_delta",
            json!({"delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7}}),
        ),
        ("message_stop", json!({})),
    ]);
    let events = collect(spawn_anthropic_stream(
        stream_response(chunks),
        idle_timeout(),
        /*telemetry*/ None,
        /*turn_state*/ None,
        no_freeform(),
    ))
    .await;
    let ResponseEvent::Completed {
        token_usage: Some(usage),
        ..
    } = events.last().unwrap()
    else {
        panic!("usage missing");
    };
    assert_eq!(
        usage,
        &TokenUsage {
            input_tokens: 55,
            cached_input_tokens: 30,
            cache_write_input_tokens: 20,
            output_tokens: 7,
            total_tokens: 62,
            ..Default::default()
        }
    );
}
