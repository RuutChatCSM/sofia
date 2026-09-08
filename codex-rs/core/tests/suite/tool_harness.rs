#![cfg(not(target_os = "windows"))]

use codex_core::TurnInputRequest;
use core_test_support::test_codex::local_selections;
use std::fs;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use assert_matches::assert_matches;
use codex_model_provider_info::WireApi;
use codex_protocol::config_types::CollaborationMode;
use codex_protocol::config_types::ModeKind;
use codex_protocol::config_types::Settings;
use codex_protocol::items::TurnItem;
use codex_protocol::models::PermissionProfile;
use codex_protocol::plan_tool::StepStatus;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::ThreadSettingsOverrides;
use codex_protocol::user_input::UserInput;
use core_test_support::TempDirExt;
use core_test_support::assert_regex_match;
use core_test_support::responses;
use core_test_support::responses::ResponsesRequest;
use core_test_support::responses::ev_apply_patch_custom_tool_call;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::test_codex;
use core_test_support::test_codex::turn_permission_fields;
use core_test_support::wait_for_event;
use core_test_support::wait_for_event_with_timeout;
use serde_json::Value;
use serde_json::json;
use std::time::Duration;
use wiremock::Mock;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;
fn call_output(req: &ResponsesRequest, call_id: &str) -> (String, Option<bool>) {
    let raw = req.function_call_output(call_id);
    assert_eq!(
        raw.get("call_id").and_then(Value::as_str),
        Some(call_id),
        "mismatched call_id in function_call_output"
    );
    let (content_opt, success) = req
        .function_call_output_content_and_success(call_id)
        .expect("function_call_output present");
    let content = content_opt.expect("function_call_output content present");
    (content, success)
}

fn custom_call_output(req: &ResponsesRequest, call_id: &str) -> (String, Option<bool>) {
    let raw = req.custom_tool_call_output(call_id);
    assert_eq!(
        raw.get("call_id").and_then(Value::as_str),
        Some(call_id),
        "mismatched call_id in custom_tool_call_output"
    );
    let (content_opt, success) = req
        .custom_tool_call_output_content_and_success(call_id)
        .expect("custom_tool_call_output present");
    let content = content_opt.expect("custom_tool_call_output content present");
    (content, success)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_command_tool_executes_command_and_streams_output() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;

    let mut builder = test_codex().with_model("test-gpt-5-codex");
    let TestCodex {
        codex,
        cwd,
        session_configured,
        ..
    } = builder.build(&server).await?;

    let call_id = "exec-command-tool-call";
    let command_args = json!({
        "cmd": "echo tool harness",
        "login": false,
    })
    .to_string();
    let first_response = sse(vec![
        ev_response_created("resp-1"),
        ev_function_call(call_id, "exec_command", &command_args),
        ev_completed("resp-1"),
    ]);
    responses::mount_sse_once(&server, first_response).await;

    let second_response = sse(vec![
        ev_assistant_message("msg-1", "all done"),
        ev_completed("resp-2"),
    ]);
    let second_mock = responses::mount_sse_once(&server, second_response).await;

    let session_model = session_configured.model.clone();
    let cwd_path = cwd.abs();
    let (sandbox_policy, permission_profile) =
        turn_permission_fields(PermissionProfile::Disabled, cwd_path.as_path());

    codex
        .start_or_steer_turn(
            TurnInputRequest::user_input(vec![UserInput::Text {
                text: "please run the shell command".into(),
                text_elements: Vec::new(),
            }])
            .with_thread_settings(ThreadSettingsOverrides {
                environments: Some(local_selections(cwd_path)),
                approval_policy: Some(AskForApproval::Never),
                sandbox_policy: Some(sandbox_policy),
                permission_profile,
                collaboration_mode: Some(CollaborationMode {
                    mode: ModeKind::Default,
                    settings: Settings {
                        model: session_model,
                        reasoning_effort: None,
                        developer_instructions: None,
                    },
                }),
                ..Default::default()
            }),
        )
        .await?;

    wait_for_event(&codex, |event| matches!(event, EventMsg::TurnComplete(_))).await;

    let req = second_mock.single_request();
    let (output_text, _) = call_output(&req, call_id);
    assert_regex_match(
        r"(?s)^(?:Chunk ID: [^\n]+\n)?Wall time: [0-9]+(?:\.[0-9]+)? seconds\nProcess exited with code 0\n(?:Original token count: \d+\n)?Output:\ntool harness\n?$",
        &output_text,
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_completions_tool_call_continues_and_final_text_stops() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let request_count = Arc::new(AtomicUsize::new(0));
    let response_count = Arc::clone(&request_count);
    let tool_arguments = json!({
        "cmd": "printf reviewed",
        "login": false,
    })
    .to_string();
    let tool_response = format!(
        "data: {}\n\ndata: [DONE]\n\n",
        json!({
            "id": "chatcmpl-tool",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "type": "function",
                        "function": {
                            "name": "exec_command",
                            "arguments": tool_arguments,
                        },
                    }],
                },
                "finish_reason": "tool_calls",
            }],
        })
    );
    let final_text = "Review complete; no changes were made.";
    let final_response = format!(
        "data: {}\n\ndata: [DONE]\n\n",
        json!({
            "id": "chatcmpl-final",
            "choices": [{
                "index": 0,
                "delta": { "content": final_text },
                "finish_reason": "stop",
            }],
        })
    );

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(move |_request: &wiremock::Request| {
            let body = if response_count.fetch_add(1, Ordering::SeqCst) == 0 {
                tool_response.clone()
            } else {
                final_response.clone()
            };
            ResponseTemplate::new(/*status*/ 200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body)
        })
        .expect(/*requests*/ 2)
        .mount(&server)
        .await;

    let mut builder = test_codex()
        .with_model("test-gpt-5-codex")
        .with_config(|config| {
            config.model_provider.wire_api = WireApi::ChatCompletions;
        });
    let TestCodex {
        codex,
        cwd,
        session_configured,
        ..
    } = builder.build_with_auto_env(&server).await?;
    let session_model = session_configured.model.clone();
    let cwd_path = cwd.abs();
    let (sandbox_policy, permission_profile) =
        turn_permission_fields(PermissionProfile::Disabled, cwd_path.as_path());

    codex
        .start_or_steer_turn(
            TurnInputRequest::user_input(vec![UserInput::Text {
                text: "Review the implementation and report your findings.".into(),
                text_elements: Vec::new(),
            }])
            .with_thread_settings(ThreadSettingsOverrides {
                environments: Some(local_selections(cwd_path)),
                approval_policy: Some(AskForApproval::Never),
                sandbox_policy: Some(sandbox_policy),
                permission_profile,
                collaboration_mode: Some(CollaborationMode {
                    mode: ModeKind::Default,
                    settings: Settings {
                        model: session_model,
                        reasoning_effort: None,
                        developer_instructions: None,
                    },
                }),
                ..Default::default()
            }),
        )
        .await?;

    let mut saw_final_text = false;
    wait_for_event(&codex, |event| match event {
        EventMsg::AgentMessage(message) if message.message == final_text => {
            saw_final_text = true;
            false
        }
        EventMsg::TurnComplete(_) => true,
        _ => false,
    })
    .await;

    assert!(saw_final_text);
    assert_eq!(request_count.load(Ordering::SeqCst), 2);
    server.verify().await;

    let requests = server.received_requests().await.unwrap();
    let second_request: Value = serde_json::from_slice(&requests[1].body)?;
    let messages = second_request["messages"].as_array().unwrap();
    assert_eq!(messages.last().unwrap()["role"], "tool");

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_completions_narration_only_stop_is_force_continued_and_bounded() -> anyhow::Result<()>
{
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let request_count = Arc::new(AtomicUsize::new(0));
    let response_count = Arc::clone(&request_count);
    // Four short narration-only stops. The first three are force-continued
    // (bounded budget of 3 per turn); the fourth exhausts the budget and the
    // turn ends instead of re-prompting forever.
    let narration_responses: Vec<String> = (0..4)
        .map(|_| {
            format!(
                "data: {}\n\ndata: [DONE]\n\n",
                json!({
                    "id": "chatcmpl-narration",
                    "choices": [{
                        "index": 0,
                        "delta": { "content": "I’ll review the implementation now." },
                        "finish_reason": "stop",
                    }],
                })
            )
        })
        .collect();

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(move |_request: &wiremock::Request| {
            let body = narration_responses[response_count.fetch_add(1, Ordering::SeqCst)].clone();
            ResponseTemplate::new(/*status*/ 200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body)
        })
        .expect(/*requests*/ 4)
        .mount(&server)
        .await;

    let mut builder = test_codex()
        .with_model("test-gpt-5-codex")
        .with_config(|config| {
            config.model_provider.wire_api = WireApi::ChatCompletions;
        });
    let TestCodex {
        codex,
        cwd,
        session_configured,
        ..
    } = builder.build_with_auto_env(&server).await?;
    let session_model = session_configured.model.clone();
    let cwd_path = cwd.abs();
    let (sandbox_policy, permission_profile) =
        turn_permission_fields(PermissionProfile::Disabled, cwd_path.as_path());

    codex
        .start_or_steer_turn(
            TurnInputRequest::user_input(vec![UserInput::Text {
                text: "Review the implementation and report your findings.".into(),
                text_elements: Vec::new(),
            }])
            .with_thread_settings(ThreadSettingsOverrides {
                environments: Some(local_selections(cwd_path)),
                approval_policy: Some(AskForApproval::Never),
                sandbox_policy: Some(sandbox_policy),
                permission_profile,
                collaboration_mode: Some(CollaborationMode {
                    mode: ModeKind::Default,
                    settings: Settings {
                        model: session_model,
                        reasoning_effort: None,
                        developer_instructions: None,
                    },
                }),
                ..Default::default()
            }),
        )
        .await?;

    wait_for_event_with_timeout(
        &codex,
        |event| matches!(event, EventMsg::TurnComplete(_)),
        Duration::from_secs(30),
    )
    .await;

    // Exactly the budgeted number of sampling requests: 1 original + 3 forced
    // continuations. The 4th narration-only stop is not re-prompted, proving
    // the continuation is bounded and cannot loop forever.
    assert_eq!(request_count.load(Ordering::SeqCst), 4);
    server.verify().await;

    let requests = server.received_requests().await.unwrap();
    for (index, request) in requests.iter().enumerate().skip(1) {
        let body: Value = serde_json::from_slice(&request.body)?;
        let messages = body["messages"].as_array().unwrap();
        // Each continuation feeds the prior narration-only assistant message
        // back so the re-sampled request continues from the same step.
        assert_eq!(
            messages.last().unwrap()["role"],
            "assistant",
            "request {index}"
        );
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_plan_tool_emits_plan_update_event() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;

    let mut builder = test_codex().with_config(|config| config.update_plan_enabled = true);
    let TestCodex {
        codex,
        cwd,
        session_configured,
        ..
    } = builder.build(&server).await?;

    let call_id = "plan-tool-call";
    let plan_args = json!({
        "explanation": "Tool harness check",
        "plan": [
            {"step": "Inspect workspace", "status": "in_progress"},
            {"step": "Report results", "status": "pending"},
        ],
    })
    .to_string();

    let first_response = sse(vec![
        ev_response_created("resp-1"),
        ev_function_call(call_id, "update_plan", &plan_args),
        ev_completed("resp-1"),
    ]);
    responses::mount_sse_once(&server, first_response).await;

    let second_response = sse(vec![
        ev_assistant_message("msg-1", "plan acknowledged"),
        ev_completed("resp-2"),
    ]);
    let second_mock = responses::mount_sse_once(&server, second_response).await;

    let session_model = session_configured.model.clone();
    let cwd_path = cwd.abs();
    let (sandbox_policy, permission_profile) =
        turn_permission_fields(PermissionProfile::Disabled, cwd_path.as_path());

    codex
        .start_or_steer_turn(
            TurnInputRequest::user_input(vec![UserInput::Text {
                text: "please update the plan".into(),
                text_elements: Vec::new(),
            }])
            .with_thread_settings(ThreadSettingsOverrides {
                environments: Some(local_selections(cwd_path)),
                approval_policy: Some(AskForApproval::Never),
                sandbox_policy: Some(sandbox_policy),
                permission_profile,
                collaboration_mode: Some(CollaborationMode {
                    mode: ModeKind::Default,
                    settings: Settings {
                        model: session_model,
                        reasoning_effort: None,
                        developer_instructions: None,
                    },
                }),
                ..Default::default()
            }),
        )
        .await?;

    let mut saw_plan_update = false;
    wait_for_event(&codex, |event| match event {
        EventMsg::PlanUpdate(update) => {
            saw_plan_update = true;
            assert_eq!(update.explanation.as_deref(), Some("Tool harness check"));
            assert_eq!(update.plan.len(), 2);
            assert_eq!(update.plan[0].step, "Inspect workspace");
            assert_matches!(update.plan[0].status, StepStatus::InProgress);
            assert_eq!(update.plan[1].step, "Report results");
            assert_matches!(update.plan[1].status, StepStatus::Pending);
            false
        }
        EventMsg::TurnComplete(_) => true,
        _ => false,
    })
    .await;

    assert!(saw_plan_update, "expected PlanUpdate event");

    let req = second_mock.single_request();
    let (output_text, _success_flag) = call_output(&req, call_id);
    assert_eq!(output_text, "Plan updated");

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_plan_tool_rejects_malformed_payload() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;

    let mut builder = test_codex().with_config(|config| config.update_plan_enabled = true);
    let TestCodex {
        codex,
        cwd,
        session_configured,
        ..
    } = builder.build(&server).await?;

    let call_id = "plan-tool-invalid";
    let invalid_args = json!({
        "explanation": "Missing plan data"
    })
    .to_string();

    let first_response = sse(vec![
        ev_response_created("resp-1"),
        ev_function_call(call_id, "update_plan", &invalid_args),
        ev_completed("resp-1"),
    ]);
    responses::mount_sse_once(&server, first_response).await;

    let second_response = sse(vec![
        ev_assistant_message("msg-1", "malformed plan payload"),
        ev_completed("resp-2"),
    ]);
    let second_mock = responses::mount_sse_once(&server, second_response).await;

    let session_model = session_configured.model.clone();
    let cwd_path = cwd.abs();
    let (sandbox_policy, permission_profile) =
        turn_permission_fields(PermissionProfile::Disabled, cwd_path.as_path());

    codex
        .start_or_steer_turn(
            TurnInputRequest::user_input(vec![UserInput::Text {
                text: "please update the plan".into(),
                text_elements: Vec::new(),
            }])
            .with_thread_settings(ThreadSettingsOverrides {
                environments: Some(local_selections(cwd_path)),
                approval_policy: Some(AskForApproval::Never),
                sandbox_policy: Some(sandbox_policy),
                permission_profile,
                collaboration_mode: Some(CollaborationMode {
                    mode: ModeKind::Default,
                    settings: Settings {
                        model: session_model,
                        reasoning_effort: None,
                        developer_instructions: None,
                    },
                }),
                ..Default::default()
            }),
        )
        .await?;

    let mut saw_plan_update = false;
    wait_for_event(&codex, |event| match event {
        EventMsg::PlanUpdate(_) => {
            saw_plan_update = true;
            false
        }
        EventMsg::TurnComplete(_) => true,
        _ => false,
    })
    .await;

    assert!(
        !saw_plan_update,
        "did not expect PlanUpdate event for malformed payload"
    );

    let req = second_mock.single_request();
    let (output_text, success_flag) = call_output(&req, call_id);
    assert!(
        output_text.contains("failed to parse function arguments"),
        "expected parse error message in output text, got {output_text:?}"
    );
    if let Some(success_flag) = success_flag {
        assert!(
            !success_flag,
            "expected tool output to mark success=false for malformed payload"
        );
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn apply_patch_tool_executes_and_emits_patch_events() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;

    let mut builder = test_codex();
    let TestCodex {
        codex,
        cwd,
        session_configured,
        ..
    } = builder.build(&server).await?;

    let file_name = "notes.txt";
    let file_path = cwd.path().join(file_name);
    let call_id = "apply-patch-call";
    let patch_content = format!(
        r#"*** Begin Patch
*** Add File: {file_name}
+Tool harness apply patch
*** End Patch"#
    );

    let first_response = sse(vec![
        ev_response_created("resp-1"),
        ev_apply_patch_custom_tool_call(call_id, &patch_content),
        ev_completed("resp-1"),
    ]);
    responses::mount_sse_once(&server, first_response).await;

    let second_response = sse(vec![
        ev_assistant_message("msg-1", "patch complete"),
        ev_completed("resp-2"),
    ]);
    let second_mock = responses::mount_sse_once(&server, second_response).await;

    let session_model = session_configured.model.clone();
    let cwd_path = cwd.abs();
    let (sandbox_policy, permission_profile) =
        turn_permission_fields(PermissionProfile::Disabled, cwd_path.as_path());

    codex
        .start_or_steer_turn(
            TurnInputRequest::user_input(vec![UserInput::Text {
                text: "please apply a patch".into(),
                text_elements: Vec::new(),
            }])
            .with_thread_settings(ThreadSettingsOverrides {
                environments: Some(local_selections(cwd_path)),
                approval_policy: Some(AskForApproval::Never),
                sandbox_policy: Some(sandbox_policy),
                permission_profile,
                collaboration_mode: Some(CollaborationMode {
                    mode: ModeKind::Default,
                    settings: Settings {
                        model: session_model,
                        reasoning_effort: None,
                        developer_instructions: None,
                    },
                }),
                ..Default::default()
            }),
        )
        .await?;

    let mut saw_file_change_started = false;
    let mut saw_file_change_completed = false;
    let mut saw_patch_begin = false;
    let mut patch_end_success = None;
    wait_for_event(&codex, |event| match event {
        EventMsg::ItemStarted(started) => {
            if let TurnItem::FileChange(item) = &started.item {
                saw_file_change_started = true;
                assert_eq!(item.id, call_id);
                assert_eq!(item.status, None);
            }
            false
        }
        EventMsg::ItemCompleted(completed) => {
            if let TurnItem::FileChange(item) = &completed.item {
                saw_file_change_completed = true;
                assert_eq!(item.id, call_id);
                assert_eq!(
                    item.status,
                    Some(codex_protocol::protocol::PatchApplyStatus::Completed)
                );
            }
            false
        }
        EventMsg::PatchApplyBegin(begin) => {
            saw_patch_begin = true;
            assert_eq!(begin.call_id, call_id);
            false
        }
        EventMsg::PatchApplyEnd(end) => {
            assert_eq!(end.call_id, call_id);
            patch_end_success = Some(end.success);
            false
        }
        EventMsg::TurnComplete(_) => true,
        _ => false,
    })
    .await;

    assert!(
        saw_file_change_started,
        "expected ItemStarted for TurnItem::FileChange"
    );
    assert!(
        saw_file_change_completed,
        "expected ItemCompleted for TurnItem::FileChange"
    );
    assert!(saw_patch_begin, "expected PatchApplyBegin event");
    let patch_end_success =
        patch_end_success.expect("expected PatchApplyEnd event to capture success flag");
    assert!(patch_end_success);

    let req = second_mock.single_request();
    let (output_text, _success_flag) = custom_call_output(&req, call_id);

    let expected_pattern = format!(
        r"(?s)^Exit code: 0
Wall time: [0-9]+(?:\.[0-9]+)? seconds
Output:
Success. Updated the following files:
A {file_name}
?$"
    );
    assert_regex_match(&expected_pattern, &output_text);

    let updated_contents = fs::read_to_string(file_path)?;
    assert_eq!(
        updated_contents, "Tool harness apply patch\n",
        "expected updated file content"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn apply_patch_reports_parse_diagnostics() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;

    let mut builder = test_codex();
    let TestCodex {
        codex,
        cwd,
        session_configured,
        ..
    } = builder.build(&server).await?;

    let call_id = "apply-patch-parse-error";
    let patch_content = r"*** Begin Patch
*** Update File: broken.txt
*** End Patch";

    let first_response = sse(vec![
        ev_response_created("resp-1"),
        ev_apply_patch_custom_tool_call(call_id, patch_content),
        ev_completed("resp-1"),
    ]);
    responses::mount_sse_once(&server, first_response).await;

    let second_response = sse(vec![
        ev_assistant_message("msg-1", "failed"),
        ev_completed("resp-2"),
    ]);
    let second_mock = responses::mount_sse_once(&server, second_response).await;

    let session_model = session_configured.model.clone();
    let cwd_path = cwd.abs();
    let (sandbox_policy, permission_profile) =
        turn_permission_fields(PermissionProfile::Disabled, cwd_path.as_path());

    codex
        .start_or_steer_turn(
            TurnInputRequest::user_input(vec![UserInput::Text {
                text: "please apply a patch".into(),
                text_elements: Vec::new(),
            }])
            .with_thread_settings(ThreadSettingsOverrides {
                environments: Some(local_selections(cwd_path)),
                approval_policy: Some(AskForApproval::Never),
                sandbox_policy: Some(sandbox_policy),
                permission_profile,
                collaboration_mode: Some(CollaborationMode {
                    mode: ModeKind::Default,
                    settings: Settings {
                        model: session_model,
                        reasoning_effort: None,
                        developer_instructions: None,
                    },
                }),
                ..Default::default()
            }),
        )
        .await?;

    wait_for_event(&codex, |event| matches!(event, EventMsg::TurnComplete(_))).await;

    let req = second_mock.single_request();
    let (output_text, success_flag) = custom_call_output(&req, call_id);

    assert!(
        output_text.contains("apply_patch verification failed"),
        "expected apply_patch verification failure message, got {output_text:?}"
    );
    assert!(
        output_text.contains("invalid hunk"),
        "expected parse diagnostics in output text, got {output_text:?}"
    );

    if let Some(success_flag) = success_flag {
        assert!(
            !success_flag,
            "expected tool output to mark success=false for parse failures"
        );
    }

    Ok(())
}
