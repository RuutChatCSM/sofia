use super::*;
use pretty_assertions::assert_eq;
use sofia_protocol::models::FunctionCallOutputPayload;

#[test]
fn parse_verdict_accepts_satisfied_and_impossible() {
    assert_eq!(parse_verdict("SATISFIED"), GoalVerdict::AcceptStop);
    assert_eq!(parse_verdict("  satisfied  \n"), GoalVerdict::AcceptStop);
    assert_eq!(
        parse_verdict("IMPOSSIBLE: no network access"),
        GoalVerdict::AcceptStop
    );
}

#[test]
fn parse_verdict_continues_on_incomplete_with_reason() {
    assert_eq!(
        parse_verdict("INCOMPLETE: the idempotency header was never added"),
        GoalVerdict::Continue("the idempotency header was never added".to_string())
    );
    // Missing reason still continues with a sensible fallback.
    assert_eq!(
        parse_verdict("INCOMPLETE"),
        GoalVerdict::Continue("The task is not yet complete.".to_string())
    );
}

#[test]
fn parse_verdict_fails_open_on_unparseable_output() {
    assert_eq!(
        parse_verdict("I think it's probably done"),
        GoalVerdict::FailOpen
    );
    assert_eq!(parse_verdict(""), GoalVerdict::FailOpen);
}

#[test]
fn render_transcript_includes_goal_tool_activity_and_final_message() {
    let items = [
        ResponseItem::FunctionCall {
            id: None,
            name: "exec_command".to_string(),
            namespace: None,
            arguments: r#"{"cmd":"grep idempotency"}"#.to_string(),
            encrypted_function_args: None,
            call_id: "call_1".to_string(),
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::FunctionCallOutput {
            id: None,
            call_id: Some("call_1".to_string()),
            name: Some("exec_command".to_string()),
            namespace: None,
            output: FunctionCallOutputPayload::from_text("no matches".to_string()),
            internal_chat_message_metadata_passthrough: None,
        },
    ];

    let transcript = render_transcript(
        &items.iter().collect::<Vec<_>>(),
        "Add the Idempotency-Key header",
        Some("Let me add it to bankTransfer."),
    );

    assert!(transcript.contains("Add the Idempotency-Key header"));
    assert!(transcript.contains("[tool call] exec_command"));
    assert!(transcript.contains("[tool result] no matches"));
    assert!(transcript.contains("AGENT'S FINAL MESSAGE:"));
    assert!(transcript.contains("Let me add it to bankTransfer."));
}

#[test]
fn build_continuation_message_is_a_user_message_naming_the_reason() {
    let item = build_continuation_message("the header is still missing");
    match item {
        ResponseItem::Message { role, content, .. } => {
            assert_eq!(role, "user");
            let text = match &content[0] {
                ContentItem::InputText { text } => text.clone(),
                other => panic!("expected input text, got {other:?}"),
            };
            assert!(text.contains("the header is still missing"));
            assert!(text.contains("goal"));
        }
        other => panic!("expected a message, got {other:?}"),
    }
}

#[test]
fn continuation_messages_are_hidden_from_the_transcript() {
    use crate::context::is_contextual_user_fragment;

    for item in [
        build_continuation_message("still missing the header"),
        build_invalid_output_message(),
    ] {
        let ResponseItem::Message { content, .. } = item else {
            panic!("expected a message");
        };
        assert!(
            content.iter().any(is_contextual_user_fragment),
            "the injected message must be recognized as a hidden contextual fragment"
        );
    }
}
