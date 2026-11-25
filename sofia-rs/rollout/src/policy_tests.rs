use pretty_assertions::assert_eq;
use sofia_protocol::protocol::EventMsg;
use sofia_protocol::protocol::StreamErrorEvent;
use sofia_protocol::protocol::ThreadHistoryMode;
use sofia_protocol::protocol::WarningEvent;

use super::should_persist_event_msg;

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
