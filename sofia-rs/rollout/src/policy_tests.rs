use super::should_persist_event_msg;
use crate::RolloutItem;
use crate::slim_rollout_item_for_persistence;
use pretty_assertions::assert_eq;
use sofia_protocol::ThreadId;
use sofia_protocol::items::CommandExecutionItem;
use sofia_protocol::items::CommandExecutionStatus;
use sofia_protocol::items::PlanItem;
use sofia_protocol::items::TurnItem;
use sofia_protocol::protocol::EventMsg;
use sofia_protocol::protocol::ExecCommandSource;
use sofia_protocol::protocol::ItemCompletedEvent;
use sofia_protocol::protocol::StreamErrorEvent;
use sofia_protocol::protocol::ThreadHistoryMode;
use sofia_protocol::protocol::WarningEvent;

fn command_execution(
    stdout: Option<&str>,
    aggregated_output: Option<&str>,
    formatted_output: Option<&str>,
) -> CommandExecutionItem {
    CommandExecutionItem {
        id: "exec-1".to_string(),
        plugin_id: None,
        script_path: None,
        process_id: None,
        command: vec!["echo".to_string(), "done".to_string()],
        cwd: serde_json::from_str("\"file:///tmp\"").expect("path uri"),
        parsed_cmd: Vec::new(),
        source: ExecCommandSource::Agent,
        interaction_input: None,
        status: CommandExecutionStatus::Completed,
        stdout: stdout.map(str::to_string),
        stderr: None,
        aggregated_output: aggregated_output.map(str::to_string),
        exit_code: Some(0),
        duration: None,
        formatted_output: formatted_output.map(str::to_string),
    }
}

fn item_completed(thread_id: ThreadId, item: TurnItem) -> RolloutItem {
    RolloutItem::EventMsg(EventMsg::ItemCompleted(ItemCompletedEvent {
        thread_id,
        turn_id: "turn-1".to_string(),
        item,
        started_at_ms: None,
        completed_at_ms: 0,
    }))
}

fn assert_items_eq(actual: &RolloutItem, expected: &RolloutItem) {
    assert_eq!(
        serde_json::to_value(actual).expect("serialize actual"),
        serde_json::to_value(expected).expect("serialize expected")
    );
}

#[test]
fn slim_rollout_item_drops_redundant_stdout_and_formatted_output() {
    let thread_id = ThreadId::new();
    let mut actual = item_completed(
        thread_id,
        TurnItem::CommandExecution(command_execution(
            Some("done\n"),
            Some("done\n"),
            Some("done\n"),
        )),
    );
    let expected = item_completed(
        thread_id,
        TurnItem::CommandExecution(command_execution(None, Some("done\n"), None)),
    );

    slim_rollout_item_for_persistence(&mut actual);

    assert_items_eq(&actual, &expected);
}

#[test]
fn slim_rollout_item_keeps_stdout_when_it_differs_from_aggregated_output() {
    let thread_id = ThreadId::new();
    let mut actual = item_completed(
        thread_id,
        TurnItem::CommandExecution(command_execution(
            Some("partial\n"),
            Some("partial\ncommand failed"),
            Some("partial\n"),
        )),
    );
    let expected = item_completed(
        thread_id,
        TurnItem::CommandExecution(command_execution(
            Some("partial\n"),
            Some("partial\ncommand failed"),
            None,
        )),
    );

    slim_rollout_item_for_persistence(&mut actual);

    assert_items_eq(&actual, &expected);
}

#[test]
fn slim_rollout_item_leaves_other_items_unchanged() {
    let mut actual = item_completed(
        ThreadId::new(),
        TurnItem::Plan(PlanItem {
            id: "plan-1".to_string(),
            text: "keep me".to_string(),
        }),
    );
    let expected = actual.clone();

    slim_rollout_item_for_persistence(&mut actual);

    assert_items_eq(&actual, &expected);
}

#[test]
fn stream_errors_are_persisted_in_both_history_modes() {
    let stream_error = EventMsg::StreamError(StreamErrorEvent {
        message: "Reconnecting... 1/5".to_string(),
        sofia_error_info: None,
        additional_details: None,
    });

    let persisted: Vec<bool> = [ThreadHistoryMode::Legacy, ThreadHistoryMode::Paginated]
        .into_iter()
        .map(|mode| should_persist_event_msg(&stream_error, mode))
        .collect();

    assert_eq!(persisted, vec![true, true]);
}

#[test]
fn warnings_remain_transient() {
    let warning = EventMsg::Warning(WarningEvent {
        message: "heads up".to_string(),
    });

    let persisted: Vec<bool> = [ThreadHistoryMode::Legacy, ThreadHistoryMode::Paginated]
        .into_iter()
        .map(|mode| should_persist_event_msg(&warning, mode))
        .collect();

    assert_eq!(persisted, vec![false, false]);
}
