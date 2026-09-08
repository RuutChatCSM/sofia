use super::*;
use codex_extension_api::ExtensionData;
use codex_extension_api::TurnItemContributor;
use codex_protocol::ResponseItemId;
use codex_protocol::items::AgentMessageContent;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use tracing_subscriber::prelude::*;

struct RewriteAgentMessageContributor;

impl TurnItemContributor for RewriteAgentMessageContributor {
    fn contribute<'a>(
        &'a self,
        _thread_store: &'a ExtensionData,
        _turn_store: &'a ExtensionData,
        item: &'a mut TurnItem,
    ) -> codex_extension_api::ExtensionFuture<'a, Result<(), String>> {
        Box::pin(async move {
            if let TurnItem::AgentMessage(agent_message) = item {
                agent_message.content = vec![AgentMessageContent::Text {
                    text: "plan contributed assistant text".to_string(),
                }];
            }
            Ok(())
        })
    }
}

fn assistant_output_text(text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: Some(ResponseItemId::with_suffix("msg", "1")),
        role: "assistant".to_string(),
        content: vec![ContentItem::OutputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

#[test]
fn post_sampling_token_estimate_is_disabled_by_always_on_sinks() {
    // `tracing::event_enabled!` consults a process-global interest cache whose
    // entries parallel tests recompute from their own subscribers, so asserting
    // on it is racy. Emitting through the real always-on sinks and checking their
    // output is deterministic: each layer's own `Targets` filter is still
    // consulted per event even when the global cache short-circuits the interest
    // check, so neither sink can retain the estimate event.
    let feedback = codex_feedback::CodexFeedback::new();
    let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
    let writer = CaptureWriter(Arc::clone(&captured));
    let subscriber = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_filter(codex_state::log_db::default_filter()),
        )
        .with(feedback.logger_layer());

    tracing::subscriber::with_default(subscriber, || {
        tracing::trace!(
            target: POST_SAMPLING_TOKEN_ESTIMATE_TARGET,
            turn_id = "turn-1",
            estimated_token_count = 1u64,
            "post sampling token estimate"
        );
    });

    let captured_text = String::from_utf8(captured.lock().expect("capture mutex poisoned").clone())
        .expect("valid utf-8 captured logs");
    assert!(
        !captured_text.contains("post sampling token estimate"),
        "log_db sink retained the estimate event: {captured_text}"
    );
    let feedback_text = String::from_utf8(
        feedback
            .snapshot(/*session_id*/ None)
            .log_attachment(None)
            .buffer,
    )
    .expect("valid utf-8 feedback logs");
    assert!(
        !feedback_text.contains("post sampling token estimate"),
        "feedback sink retained the estimate event: {feedback_text}"
    );
}

#[derive(Clone, Default)]
struct CaptureWriter(Arc<std::sync::Mutex<Vec<u8>>>);

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CaptureWriter {
    type Writer = CaptureWriterGuard;

    fn make_writer(&'a self) -> Self::Writer {
        CaptureWriterGuard(Arc::clone(&self.0))
    }
}

struct CaptureWriterGuard(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for CaptureWriterGuard {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("capture mutex poisoned")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn plan_mode_uses_contributed_turn_item_for_last_agent_message() {
    let (mut session, turn_context) = crate::session::tests::make_session_and_context().await;
    let mut builder = codex_extension_api::ExtensionRegistryBuilder::new();
    builder.turn_item_contributor(Arc::new(RewriteAgentMessageContributor));
    session.services.extensions = Arc::new(builder.build());
    let turn_store = ExtensionData::new(turn_context.sub_id.clone());
    let mut state = PlanModeStreamState::new(&turn_context.sub_id);
    let mut last_agent_message = None;
    let item = assistant_output_text("original assistant text");

    let handled = handle_assistant_item_done_in_plan_mode(
        &session,
        &turn_context,
        &turn_store,
        &item,
        &mut state,
        /*previously_active_item*/ None,
        &mut last_agent_message,
    )
    .await;

    assert!(handled);
    assert_eq!(
        last_agent_message.as_deref(),
        Some("plan contributed assistant text")
    );
}

#[test]
fn realtime_user_verification_notice_excludes_request_payload() {
    let event = EventMsg::ElicitationRequest(codex_protocol::approvals::ElicitationRequestEvent {
        turn_id: None,
        server_name: "private-server-name".to_string(),
        id: codex_protocol::mcp::RequestId::String("private-request-id".to_string()),
        request: codex_protocol::approvals::ElicitationRequest::UserVerification {
            title: "private-title".to_string(),
            description: "private-description".to_string(),
            challenge: "private-challenge".to_string(),
        },
    });
    assert_eq!(
        realtime_text_for_event(&event),
        Some((
            "<user_verification_notice>User verification is required. Please respond in the app.</user_verification_notice>".to_string(),
            None,
        )),
    );
}

#[test]
fn forced_continuation_triggers_for_short_narration_only_stop() {
    let budget = AtomicU32::new(MAX_FORCED_CONTINUATIONS_PER_TURN);
    let narration = Some("Review complete; no changes were made.".to_string());
    assert!(!should_force_narration_continuation(
        &budget,
        /*chat_completions_wire*/ true,
        &narration,
        /*plan_mode*/ false,
        /*tool_calls_made*/ false,
        /*last_executed_tool*/ ExecutedToolOutcome::Unremarkable,
    ));
    // A completion report the same turn a tool failed is NOT terminal evidence:
    // stopping on an unresolved failure is still premature.
    assert!(should_force_narration_continuation(
        &budget,
        true,
        &narration,
        false,
        false,
        /*last_executed_tool*/ ExecutedToolOutcome::Failed,
    ));
    assert_eq!(
        budget.load(Ordering::Relaxed),
        MAX_FORCED_CONTINUATIONS_PER_TURN - 1
    );
}

#[test]
fn forced_continuation_triggers_for_long_narration_declaring_remaining_work() {
    // The observed premature stop from an ongoing session: a multi-sentence
    // narration :=>says what still needs doing, then ends without any tool call.
    let budget = AtomicU32::new(MAX_FORCED_CONTINUATIONS_PER_TURN);
    let narration = "The controller still calls the renamed method without the new parameter. \
        I need to restore the old method as a public wrapper and check what the identity block \
        method outputs now, then run the relevant specs before continuing."
        .to_string();
    assert!(
        narration.chars().count() > NARRATION_ONLY_CONTINUATION_THRESHOLD_CHARS,
        "this narration must exercise the long-narration path"
    );
    assert!(should_force_narration_continuation(
        &budget,
        /*chat_completions_wire*/ true,
        &Some(narration),
        /*plan_mode*/ false,
        /*tool_calls_made*/ false,
        /*last_executed_tool*/ ExecutedToolOutcome::Unremarkable,
    ));
    assert_eq!(
        budget.load(Ordering::Relaxed),
        MAX_FORCED_CONTINUATIONS_PER_TURN - 1
    );
}

#[test]
fn forced_continuation_refuses_long_ambiguous_or_completed_narration_and_questions() {
    let budget = AtomicU32::new(MAX_FORCED_CONTINUATIONS_PER_TURN);
    let expected = budget.load(Ordering::Relaxed);

    let ambiguous_long = Some("a".repeat(NARRATION_ONLY_CONTINUATION_THRESHOLD_CHARS + 1));
    assert!(!should_force_narration_continuation(
        &budget,
        true,
        &ambiguous_long,
        false,
        false,
        /*last_executed_tool*/ ExecutedToolOutcome::Unremarkable,
    ));

    let completed = Some(
        "The migration is complete and the full suite is green; nothing else remains. \
        There is no outstanding work left to do here. The controller was verified end to \
        end, all pages render in both color modes, and no further changes are planned."
            .to_string(),
    );
    assert!(
        completed.as_ref().expect("present").chars().count()
            > NARRATION_ONLY_CONTINUATION_THRESHOLD_CHARS
    );
    assert!(!should_force_narration_continuation(
        &budget,
        true,
        &completed,
        false,
        false,
        /*last_executed_tool*/ ExecutedToolOutcome::Unremarkable,
    ));

    // Long "let me know"-style closings read as completions, not remaining work
    // (they must exercise the long-narration path for the guard to matter).
    let closing = Some(
        "All pages were verified across both color modes and every viewport, the media \
        picker, revision history, and navigation items all behave as expected, and the \
        full regression suite is green. Let me know if you want any adjustments to the layout."
            .to_string(),
    );
    assert!(
        closing.as_ref().expect("present").chars().count()
            > NARRATION_ONLY_CONTINUATION_THRESHOLD_CHARS
    );
    assert!(!should_force_narration_continuation(
        &budget,
        true,
        &closing,
        false,
        false,
        /*last_executed_tool*/ ExecutedToolOutcome::Unremarkable,
    ));

    let question = Some("This deletes the staging database. Should I proceed?".to_string());
    assert!(!should_force_narration_continuation(
        &budget,
        true,
        &question,
        false,
        false,
        /*last_executed_tool*/ ExecutedToolOutcome::Unremarkable,
    ));

    assert_eq!(budget.load(Ordering::Relaxed), expected);
}

#[test]
fn forced_continuation_never_fires_for_responses_wire_plan_mode_or_tool_calls() {
    let budget = AtomicU32::new(MAX_FORCED_CONTINUATIONS_PER_TURN);
    let narration = Some("Now patching the API route definitions.".to_string());

    assert!(!should_force_narration_continuation(
        &budget,
        /*chat_completions_wire*/ false,
        &narration,
        /*plan_mode*/ false,
        /*tool_calls_made*/ false,
        /*last_executed_tool*/ ExecutedToolOutcome::Unremarkable,
    ));
    assert!(!should_force_narration_continuation(
        &budget,
        true,
        &narration,
        /*plan_mode*/ true,
        /*tool_calls_made*/ false,
        /*last_executed_tool*/ ExecutedToolOutcome::Unremarkable,
    ));
    assert!(!should_force_narration_continuation(
        &budget,
        true,
        &narration,
        /*plan_mode*/ false,
        /*tool_calls_made*/ true,
        /*last_executed_tool*/ ExecutedToolOutcome::Unremarkable,
    ));
    assert!(!should_force_narration_continuation(
        &budget,
        true,
        &None,
        /*plan_mode*/ false,
        /*tool_calls_made*/ false,
        /*last_executed_tool*/ ExecutedToolOutcome::Unremarkable,
    ));

    assert_eq!(
        budget.load(Ordering::Relaxed),
        MAX_FORCED_CONTINUATIONS_PER_TURN
    );
}

#[test]
fn forced_continuation_is_bounded_by_per_turn_budget() {
    let budget = AtomicU32::new(2);
    let narration = Some("Now patching the API route definitions.".to_string());

    assert!(should_force_narration_continuation(
        &budget,
        true,
        &narration,
        false,
        false,
        /*last_executed_tool*/ ExecutedToolOutcome::Unremarkable,
    ));
    assert!(should_force_narration_continuation(
        &budget,
        true,
        &narration,
        false,
        false,
        /*last_executed_tool*/ ExecutedToolOutcome::Unremarkable,
    ));
    assert!(!should_force_narration_continuation(
        &budget,
        true,
        &narration,
        false,
        false,
        /*last_executed_tool*/ ExecutedToolOutcome::Unremarkable,
    ));
}

#[test]
fn forced_continuation_refuses_short_narration_reporting_completed_outcome() {
    // A genuine one-line report after tool work must terminate, not re-sample:
    // this is the text that regressed when short narration was blindly continued.
    let budget = AtomicU32::new(MAX_FORCED_CONTINUATIONS_PER_TURN);
    let narration = Some("Review complete; no changes were made.".to_string());
    assert!(!should_force_narration_continuation(
        &budget,
        /*chat_completions_wire*/ true,
        &narration,
        /*plan_mode*/ false,
        /*tool_calls_made*/ false,
        /*last_executed_tool*/ ExecutedToolOutcome::Unremarkable,
    ));
    // A completion report the same turn a tool failed is NOT terminal evidence:
    // stopping on an unresolved failure is still premature.
    assert!(should_force_narration_continuation(
        &budget,
        true,
        &narration,
        false,
        false,
        /*last_executed_tool*/ ExecutedToolOutcome::Failed,
    ));
    assert_eq!(
        budget.load(Ordering::Relaxed),
        MAX_FORCED_CONTINUATIONS_PER_TURN - 1
    );
}

#[test]
fn forced_continuation_triggers_after_failed_tool_without_forward_markers() {
    // A long narration describing the failure with no forward-looking markers is
    // still premature: the turn ends on an unresolved failed tool result.
    let budget = AtomicU32::new(MAX_FORCED_CONTINUATIONS_PER_TURN);
    let narration = "The verification command reported a nonzero exit, so the workspace state \
        was not measured and no configuration was applied. The harness returned early, and \
        the previously recorded baseline remains in place without any attempt to continue \
        the checks."
        .to_string();
    assert!(narration.chars().count() > NARRATION_ONLY_CONTINUATION_THRESHOLD_CHARS);
    assert!(should_force_narration_continuation(
        &budget,
        /*chat_completions_wire*/ true,
        &Some(narration),
        /*plan_mode*/ false,
        /*tool_calls_made*/ false,
        /*last_executed_tool*/ ExecutedToolOutcome::Failed,
    ));
    assert_eq!(
        budget.load(Ordering::Relaxed),
        MAX_FORCED_CONTINUATIONS_PER_TURN - 1
    );
}

#[test]
fn forced_continuation_refuses_failed_tool_when_narration_asks_question() {
    let budget = AtomicU32::new(MAX_FORCED_CONTINUATIONS_PER_TURN);
    let narration = Some(
        "The commit failed because the working tree has uncommitted changes. \
        Shall I stash them and retry?"
            .to_string(),
    );
    assert!(!should_force_narration_continuation(
        &budget,
        true,
        &narration,
        false,
        false,
        /*last_executed_tool*/ ExecutedToolOutcome::Failed,
    ));
    assert_eq!(
        budget.load(Ordering::Relaxed),
        MAX_FORCED_CONTINUATIONS_PER_TURN
    );
}

#[test]
fn forced_continuation_triggers_after_mutation_with_short_completed_outcome() {
    // A bare mutation (apply_patch/plan update) followed by a one-line completion
    // report is NOT terminal evidence: the mutation still needs verification.
    let budget = AtomicU32::new(MAX_FORCED_CONTINUATIONS_PER_TURN);
    let narration = Some("Patch applied; the change is complete.".to_string());
    assert!(should_force_narration_continuation(
        &budget,
        true,
        &narration,
        false,
        false,
        /*last_executed_tool*/ ExecutedToolOutcome::Mutated,
    ));
    assert_eq!(
        budget.load(Ordering::Relaxed),
        MAX_FORCED_CONTINUATIONS_PER_TURN - 1
    );
}

#[test]
fn forced_continuation_triggers_after_mutation_with_long_completed_outcome() {
    // The stage-3 flagship case: a long narration that CLAIMS completion right
    // after a mutation is force-continued for verification evidence, even though
    // the same text is final when the last tool succeeded read-only.
    let budget = AtomicU32::new(MAX_FORCED_CONTINUATIONS_PER_TURN);
    let narration = "The migration is complete and the full suite is green; nothing else \
        remains. There is no outstanding work left to do here. The controller was verified \
        end to end, all pages render in both color modes, and no further changes are planned."
        .to_string();
    assert!(narration.chars().count() > NARRATION_ONLY_CONTINUATION_THRESHOLD_CHARS);
    assert!(reports_completed_outcome(&narration));
    assert!(should_force_narration_continuation(
        &budget,
        true,
        &Some(narration),
        false,
        false,
        /*last_executed_tool*/ ExecutedToolOutcome::Mutated,
    ));
    assert_eq!(
        budget.load(Ordering::Relaxed),
        MAX_FORCED_CONTINUATIONS_PER_TURN - 1
    );
}

#[test]
fn forced_continuation_refuses_completed_narration_when_verification_ran() {
    // The identical text is final once a non-mutating tool (tests, grep-diff)
    // executed after the mutation: verification evidence backs the claim.
    let budget = AtomicU32::new(MAX_FORCED_CONTINUATIONS_PER_TURN);
    let narration = "The migration is complete and the full suite is green; nothing else \
        remains. There is no outstanding work left to do here. The controller was verified \
        end to end, all pages render in both color modes, and no further changes are planned."
        .to_string();
    assert!(!should_force_narration_continuation(
        &budget,
        true,
        &Some(narration),
        false,
        false,
        /*last_executed_tool*/ ExecutedToolOutcome::Unremarkable,
    ));
    assert_eq!(
        budget.load(Ordering::Relaxed),
        MAX_FORCED_CONTINUATIONS_PER_TURN
    );
}

#[test]
fn forced_continuation_refuses_mutation_when_narration_asks_question() {
    let budget = AtomicU32::new(MAX_FORCED_CONTINUATIONS_PER_TURN);
    let narration = Some("The patch is in. Shall I run the integration suite now?".to_string());
    assert!(!should_force_narration_continuation(
        &budget,
        true,
        &narration,
        false,
        false,
        /*last_executed_tool*/ ExecutedToolOutcome::Mutated,
    ));
    assert_eq!(
        budget.load(Ordering::Relaxed),
        MAX_FORCED_CONTINUATIONS_PER_TURN
    );
}
