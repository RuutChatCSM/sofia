use super::AuthRequestTelemetryContext;
use super::CompactConversationRequestSettings;
use super::ModelClient;
use super::PendingUnauthorizedRetry;
use super::Prompt;
use super::UnauthorizedRecoveryExecution;
use super::X_CODEX_INSTALLATION_ID_HEADER;
use super::X_CODEX_PARENT_THREAD_ID_HEADER;
use super::X_CODEX_TURN_METADATA_HEADER;
use super::X_CODEX_WINDOW_ID_HEADER;
use super::X_OPENAI_SUBAGENT_HEADER;
use crate::AttestationContext;
use crate::AttestationProvider;
use crate::GenerateAttestationFuture;
use crate::responses_metadata::CodexResponsesMetadata;
use crate::test_support::TestCodexResponsesRequestKind;
use crate::test_support::responses_metadata as test_responses_metadata;
use futures::StreamExt;
use pretty_assertions::assert_eq;
use serde_json::json;
use sofia_api::AgentIdentityTelemetry;
use sofia_api::ApiError;
use sofia_api::ResponseEvent;
use sofia_api::ResponsesEndpoint;
use sofia_api::TransportError;
use sofia_http_client::HttpClientFactory;
use sofia_http_client::OutboundProxyPolicy;
use sofia_login::AuthCredentialsStoreMode;
use sofia_login::AuthKeyringBackendKind;
use sofia_login::AuthManager;
use sofia_login::CodexAuth;
use sofia_login::auth::AgentIdentityAuthPolicy;
use sofia_model_provider::BearerAuthProvider;
use sofia_model_provider::ModelProvider;
use sofia_model_provider::ModelProviderFuture;
use sofia_model_provider::ProviderAccountResult;
use sofia_model_provider::ProviderAuthRecoveryMessages;
use sofia_model_provider::ProviderUnauthorizedRecovery;
use sofia_model_provider::SharedModelProvider;
use sofia_model_provider::create_model_provider;
use sofia_model_provider_info::CHATGPT_CODEX_BASE_URL;
use sofia_model_provider_info::ModelProviderInfo;
use sofia_model_provider_info::WireApi;
use sofia_model_provider_info::create_oss_provider_with_base_url;
use sofia_models_manager::manager::SharedModelsManager;
use sofia_otel::SessionTelemetry;
use sofia_protocol::ThreadId;
use sofia_protocol::auth::AuthMode;
use sofia_protocol::error::CodexErr;
use sofia_protocol::error::CodexErrorDetails;
use sofia_protocol::models::BaseInstructions;
use sofia_protocol::models::ContentItem;
use sofia_protocol::models::ResponseItem;
use sofia_protocol::openai_models::ModelInfo;
use sofia_protocol::openai_models::ModelsResponse;
use sofia_protocol::openai_models::ReasoningEffort;
use sofia_protocol::openai_models::ReasoningEffortPreset;
use sofia_protocol::protocol::InternalSessionSource;
use sofia_protocol::protocol::SessionSource;
use sofia_protocol::protocol::SubAgentSource;
use sofia_rollout_trace::CompactionTraceContext;
use sofia_rollout_trace::ExecutionStatus;
use sofia_rollout_trace::InferenceTraceAttempt;
use sofia_rollout_trace::InferenceTraceContext;
use sofia_rollout_trace::RawTraceEventPayload;
use sofia_rollout_trace::RolloutTrace;
use sofia_rollout_trace::TraceWriter;
use sofia_rollout_trace::replay_bundle;
use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::task::Context;
use std::task::Poll;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::Notify;
use tracing::Event;
use tracing::Subscriber;
use tracing::field::Visit;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context as LayerContext;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

const TEST_CHATGPT_ID_TOKEN: &str = "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJlbWFpbCI6InVzZXJAZXhhbXBsZS5jb20iLCJlbWFpbF92ZXJpZmllZCI6dHJ1ZSwiaHR0cHM6Ly9hcGkub3BlbmFpLmNvbS9hdXRoIjp7ImNoYXRncHRfdXNlcl9pZCI6InVzZXItMTIzNDUiLCJ1c2VyX2lkIjoidXNlci0xMjM0NSIsImNoYXRncHRfcGxhbl90eXBlIjoicHJvIiwiY2hhdGdwdF9hY2NvdW50X2lkIjoiYWNjb3VudC0xMjMifX0.c2ln";
const TEST_INSTALLATION_ID: &str = "11111111-1111-4111-8111-111111111111";

fn test_model_client(session_source: SessionSource) -> ModelClient {
    test_model_client_with_thread_id(ThreadId::new(), session_source)
}

fn test_model_client_with_thread_id(
    thread_id: ThreadId,
    session_source: SessionSource,
) -> ModelClient {
    let provider = create_oss_provider_with_base_url("https://example.com/v1", WireApi::Responses);
    ModelClient::new(
        /*auth_manager*/ None,
        AgentIdentityAuthPolicy::JwtOnly,
        thread_id,
        provider,
        session_source,
        "test_originator".to_string(),
        /*model_verbosity*/ None,
        /*content_item_kinds_enabled*/ true,
        /*enable_request_compression*/ false,
        /*include_timing_metrics*/ false,
        /*beta_features_header*/ None,
        /*concurrent_reasoning_summaries_enabled*/ false,
        /*attestation_provider*/ None,
        HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
    )
}

#[tokio::test]
async fn compact_uses_bearer_after_agent_identity_session_fallback() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    let registration_count = Arc::new(AtomicUsize::new(0));
    let response_count = Arc::clone(&registration_count);
    Mock::given(method("POST"))
        .and(path("/v1/agent/register"))
        .respond_with(move |_request: &wiremock::Request| {
            response_count.fetch_add(1, Ordering::SeqCst);
            ResponseTemplate::new(/*status*/ 503)
        })
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/responses/compact"))
        .respond_with(ResponseTemplate::new(/*status*/ 200).set_body_json(json!({
            "output": []
        })))
        .expect(/*requests*/ 1)
        .mount(&server)
        .await;

    let sofia_home = TempDir::new()?;
    let auth_manager = chatgpt_auth_manager(&sofia_home, server.uri()).await;
    let mut provider = ModelProviderInfo::create_openai_provider(/*base_url*/ None);
    provider.base_url = Some(format!("{}/v1", server.uri()));
    provider.supports_websockets = false;
    let thread_id = ThreadId::new();
    let client = ModelClient::new(
        Some(auth_manager),
        AgentIdentityAuthPolicy::ChatGptAuth,
        thread_id,
        provider,
        SessionSource::Cli,
        "test_originator".to_string(),
        /*model_verbosity*/ None,
        /*content_item_kinds_enabled*/ true,
        /*enable_request_compression*/ false,
        /*include_timing_metrics*/ false,
        /*beta_features_header*/ None,
        /*concurrent_reasoning_summaries_enabled*/ false,
        /*attestation_provider*/ None,
        HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
    );
    let prompt = Prompt {
        input: vec![ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "please compact".to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        }],
        base_instructions: BaseInstructions {
            text: "base instructions".to_string(),
            provenance: None,
        },
        ..Default::default()
    };
    let responses_metadata = test_responses_metadata_for_client(
        &client,
        /*turn_id*/ None,
        format!("{}:0", client.state.thread_id),
        /*parent_thread_id*/ None,
        TestCodexResponsesRequestKind::Turn,
    );

    let output = client
        .compact_conversation_history(
            &prompt,
            &test_model_info(),
            /*turn_state*/ None,
            CompactConversationRequestSettings {
                effort: None,
                summary: sofia_protocol::config_types::ReasoningSummary::None,
                service_tier: None,
            },
            &test_session_telemetry(),
            &CompactionTraceContext::disabled(),
            &responses_metadata,
        )
        .await?;

    assert!(output.is_empty());
    assert_eq!(registration_count.load(Ordering::SeqCst), 3);
    let requests = server
        .received_requests()
        .await
        .expect("server should record requests");
    let compact_request = requests
        .iter()
        .find(|request| request.url.path() == "/v1/responses/compact")
        .expect("compact request should be captured");
    assert_eq!(
        compact_request
            .headers
            .get(http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer test-access-token")
    );
    assert_eq!(
        compact_request
            .headers
            .get("ChatGPT-Account-ID")
            .and_then(|value| value.to_str().ok()),
        Some("account-123")
    );

    Ok(())
}

fn test_model_provider() -> SharedModelProvider {
    test_model_client(SessionSource::Cli).state.provider()
}

fn test_responses_metadata_for_client(
    client: &ModelClient,
    turn_id: Option<&str>,
    window_id: String,
    parent_thread_id: Option<ThreadId>,
    request_kind: TestCodexResponsesRequestKind,
) -> CodexResponsesMetadata {
    let thread_id = client.state.thread_id.to_string();
    test_responses_metadata(
        TEST_INSTALLATION_ID,
        &thread_id,
        &thread_id,
        turn_id,
        window_id,
        &client.state.session_source,
        parent_thread_id,
        request_kind,
    )
}

fn test_model_info() -> ModelInfo {
    serde_json::from_value(json!({
        "slug": "gpt-test",
        "display_name": "gpt-test",
        "description": "desc",
        "default_reasoning_level": "medium",
        "supported_reasoning_levels": [
            {"effort": "medium", "description": "medium"}
        ],
        "shell_type": "shell_command",
        "visibility": "list",
        "supported_in_api": true,
        "priority": 1,
        "upgrade": null,
        "model_messages": null,
        "support_verbosity": false,
        "default_verbosity": null,
        "apply_patch_tool_type": null,
        "truncation_policy": {"mode": "bytes", "limit": 10000},
        "supports_image_detail_original": false,
        "context_window": 272000,
        "auto_compact_token_limit": null,
        "experimental_supported_tools": []
    }))
    .expect("deserialize test model info")
}

#[test]
fn responses_lite_prefix_ids_track_thread_and_payload() -> anyhow::Result<()> {
    let thread_id = ThreadId::new();
    let client = test_model_client_with_thread_id(thread_id, SessionSource::Cli);
    let mut model = test_model_info();
    model.use_responses_lite = true;
    let mut prompt = Prompt {
        base_instructions: BaseInstructions {
            text: "base instructions".to_string(),
            provenance: None,
        },
        ..Default::default()
    };
    let build = |client: &ModelClient, prompt: &Prompt| {
        client.build_responses_request(
            prompt,
            &model,
            /*effort*/ None,
            sofia_protocol::config_types::ReasoningSummary::None,
            /*service_tier*/ None,
            &test_responses_metadata_for_client(
                client,
                /*turn_id*/ None,
                format!("{}:0", client.state.thread_id),
                /*parent_thread_id*/ None,
                TestCodexResponsesRequestKind::Turn,
            ),
        )
    };

    let original = build(&client, &prompt)?;
    assert_eq!(build(&client, &prompt)?, original);

    prompt.base_instructions.text.push_str(" with an update");
    let changed_instructions = build(&client, &prompt)?;
    assert_eq!(changed_instructions.input[0], original.input[0]);
    assert_ne!(changed_instructions.input[1].id(), original.input[1].id());

    prompt.tools = vec![sofia_tools::ToolSpec::Freeform(sofia_tools::FreeformTool {
        name: "exec".to_string(),
        description: "Execute JavaScript.".to_string(),
        defer_loading: None,
        format: sofia_tools::FreeformToolFormat {
            r#type: "grammar".to_string(),
            syntax: "lark".to_string(),
            definition: "start: /.+/".to_string(),
        },
    })]
    .into();
    let changed_tools = build(&client, &prompt)?;
    assert_ne!(
        changed_tools.input[0].id(),
        changed_instructions.input[0].id()
    );
    assert_eq!(changed_tools.input[1], changed_instructions.input[1]);

    let independent = build(
        &test_model_client_with_thread_id(ThreadId::new(), SessionSource::Cli),
        &prompt,
    )?;
    assert_ne!(independent.input[0].id(), changed_tools.input[0].id());
    assert_ne!(independent.input[1].id(), changed_tools.input[1].id());
    Ok(())
}

fn test_session_telemetry() -> SessionTelemetry {
    SessionTelemetry::new(
        ThreadId::new(),
        "gpt-test",
        "gpt-test",
        /*account_id*/ None,
        /*account_email*/ None,
        /*auth_mode*/ None,
        "test-originator".to_string(),
        /*log_user_prompts*/ false,
        "test-terminal".to_string(),
        SessionSource::Cli,
    )
}

fn spawned_session_source() -> SessionSource {
    SessionSource::SubAgent(SubAgentSource::ThreadSpawn {
        parent_thread_id: ThreadId::new(),
        depth: 1,
        agent_path: None,
        agent_nickname: None,
        agent_role: None,
    })
}

fn reasoning_effort_in_request(
    model_info: &ModelInfo,
    session_source: SessionSource,
    effort: ReasoningEffort,
) -> ReasoningEffort {
    let client = test_model_client(session_source);
    client
        .build_responses_request(
            &Prompt::default(),
            model_info,
            Some(effort),
            sofia_protocol::config_types::ReasoningSummary::None,
            /*service_tier*/ None,
            &test_responses_metadata_for_client(
                &client,
                /*turn_id*/ None,
                format!("{}:0", client.state.thread_id),
                /*parent_thread_id*/ None,
                TestCodexResponsesRequestKind::Turn,
            ),
        )
        .expect("build responses request")
        .reasoning
        .expect("request should include reasoning")
        .effort
        .expect("request should include reasoning effort")
}

#[test]
fn reasoning_effort_for_requests_uses_multi_agent_override_for_ultra() {
    let mut model_info = test_model_info();
    model_info.multi_agent_reasoning_effort = Some(ReasoningEffort::High);
    model_info
        .supported_reasoning_levels
        .push(ReasoningEffortPreset {
            effort: ReasoningEffort::High,
            description: "high".to_string(),
        });

    let actual = [SessionSource::Cli, spawned_session_source()].map(|session_source| {
        reasoning_effort_in_request(&model_info, session_source, ReasoningEffort::Ultra)
    });

    assert_eq!(actual, [ReasoningEffort::High, ReasoningEffort::High]);
}

#[test]
fn reasoning_effort_for_requests_falls_back_for_missing_or_invalid_override() {
    let mut model_info = test_model_info();
    model_info.supported_reasoning_levels = vec![
        ReasoningEffortPreset {
            effort: ReasoningEffort::Low,
            description: "low".to_string(),
        },
        ReasoningEffortPreset {
            effort: ReasoningEffort::XHigh,
            description: "xhigh".to_string(),
        },
        ReasoningEffortPreset {
            effort: ReasoningEffort::Ultra,
            description: "ultra".to_string(),
        },
    ];

    let actual = [
        None,
        Some(ReasoningEffort::Ultra),
        Some(ReasoningEffort::High),
    ]
    .map(|multi_agent_reasoning_effort| {
        model_info.multi_agent_reasoning_effort = multi_agent_reasoning_effort;
        reasoning_effort_in_request(&model_info, SessionSource::Cli, ReasoningEffort::Ultra)
    });

    assert_eq!(
        actual,
        [
            ReasoningEffort::XHigh,
            ReasoningEffort::XHigh,
            ReasoningEffort::XHigh,
        ]
    );

    model_info.multi_agent_reasoning_effort = None;
    model_info.supported_reasoning_levels.insert(
        1,
        ReasoningEffortPreset {
            effort: ReasoningEffort::Max,
            description: "max".to_string(),
        },
    );
    assert_eq!(
        reasoning_effort_in_request(&model_info, SessionSource::Cli, ReasoningEffort::Ultra),
        ReasoningEffort::Max
    );

    model_info.supported_reasoning_levels.clear();
    assert_eq!(
        reasoning_effort_in_request(&model_info, SessionSource::Cli, ReasoningEffort::Ultra),
        ReasoningEffort::Medium
    );
}

#[test]
fn reasoning_effort_for_requests_preserves_non_ultra_and_persistent_behavior() {
    let mut model_info = test_model_info();
    model_info.multi_agent_reasoning_effort = Some(ReasoningEffort::Low);

    assert_eq!(
        (
            reasoning_effort_in_request(&model_info, SessionSource::Cli, ReasoningEffort::High,),
            reasoning_effort_in_request(
                &model_info,
                SessionSource::Cli,
                ReasoningEffort::Persistent,
            ),
        ),
        (
            ReasoningEffort::High,
            ReasoningEffort::Custom("disabled".to_string()),
        )
    );
}

fn write_chatgpt_auth_json(sofia_home: &std::path::Path) {
    let auth_json = json!({
        "tokens": {
            "id_token": TEST_CHATGPT_ID_TOKEN,
            "access_token": "test-access-token",
            "refresh_token": "test-refresh-token",
            "account_id": "account-123"
        },
        "last_refresh": "2099-01-01T00:00:00Z"
    });
    std::fs::write(
        sofia_home.join("auth.json"),
        serde_json::to_string_pretty(&auth_json).expect("serialize auth.json"),
    )
    .expect("write auth.json");
}

async fn chatgpt_auth_manager(
    sofia_home: &TempDir,
    agent_identity_authapi_base_url: String,
) -> Arc<AuthManager> {
    write_chatgpt_auth_json(sofia_home.path());
    let auth_manager = AuthManager::shared(
        sofia_home.path().to_path_buf(),
        /*enable_codex_api_key_env*/ false,
        AuthCredentialsStoreMode::File,
        /*forced_chatgpt_workspace_id*/ None,
        /*chatgpt_base_url*/ None,
        AuthKeyringBackendKind::default(),
        sofia_login::test_support::transport_default_auth_route_config(),
    )
    .await;
    let auth = auth_manager.auth().await.expect("auth should load");
    AuthManager::from_auth_for_testing_with_agent_identity_authapi_base_url(
        auth,
        agent_identity_authapi_base_url,
    )
}

#[derive(Default)]
struct TagCollectorVisitor {
    tags: BTreeMap<String, String>,
}

impl Visit for TagCollectorVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.tags
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.tags
            .insert(field.name().to_string(), format!("{value:?}"));
    }
}

#[derive(Clone)]
struct TagCollectorLayer {
    tags: Arc<Mutex<BTreeMap<String, String>>>,
}

impl<S> Layer<S> for TagCollectorLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: LayerContext<'_, S>) {
        if event.metadata().target() != "feedback_tags" {
            return;
        }
        let mut visitor = TagCollectorVisitor::default();
        event.record(&mut visitor);
        self.tags.lock().unwrap().extend(visitor.tags);
    }
}

fn started_inference_attempt(temp: &TempDir) -> anyhow::Result<InferenceTraceAttempt> {
    let writer = Arc::new(TraceWriter::create(
        temp.path(),
        "trace-1".to_string(),
        "rollout-1".to_string(),
        "thread-root".to_string(),
    )?);
    writer.append(RawTraceEventPayload::ThreadStarted {
        thread_id: "thread-root".to_string(),
        agent_path: "/root".to_string(),
        metadata_payload: None,
    })?;
    writer.append(RawTraceEventPayload::CodexTurnStarted {
        sofia_turn_id: "turn-1".to_string(),
        thread_id: "thread-root".to_string(),
    })?;

    let inference_trace = InferenceTraceContext::enabled(
        writer,
        "thread-root".to_string(),
        "turn-1".to_string(),
        "gpt-test".to_string(),
        "test-provider".to_string(),
    );
    let attempt = inference_trace.start_attempt();
    attempt.record_started(&json!({
        "model": "gpt-test",
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": "hello"}]
        }],
    }));
    Ok(attempt)
}

fn output_message(id: &str, text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: Some(sofia_protocol::ResponseItemId::with_suffix("msg", id)),
        role: "assistant".to_string(),
        content: vec![ContentItem::OutputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

async fn replay_until_cancelled(temp: &TempDir) -> anyhow::Result<RolloutTrace> {
    let mut rollout = replay_bundle(temp.path())?;
    for _ in 0..50 {
        let inference = rollout
            .inference_calls
            .values()
            .next()
            .expect("inference should be reduced");
        if inference.execution.status == ExecutionStatus::Cancelled {
            return Ok(rollout);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
        rollout = replay_bundle(temp.path())?;
    }
    Ok(rollout)
}

struct NotifyAfterEventStream {
    events: VecDeque<ResponseEvent>,
    yielded: usize,
    notify_after: usize,
    notify: Arc<Notify>,
}

impl futures::Stream for NotifyAfterEventStream {
    type Item = std::result::Result<ResponseEvent, ApiError>;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let Some(event) = self.events.pop_front() else {
            return Poll::Pending;
        };
        self.yielded += 1;
        if self.yielded == self.notify_after {
            self.notify.notify_one();
        }
        Poll::Ready(Some(Ok(event)))
    }
}

#[test]
fn build_subagent_headers_sets_other_subagent_label() {
    let client = test_model_client(SessionSource::SubAgent(SubAgentSource::Other(
        "memory_consolidation".to_string(),
    )));
    let headers = client.build_subagent_headers();
    let value = headers
        .get(X_OPENAI_SUBAGENT_HEADER)
        .and_then(|value| value.to_str().ok());
    assert_eq!(value, Some("memory_consolidation"));
}

#[test]
fn internal_session_prompt_cache_key_is_scoped_to_parent_thread() {
    let parent_thread_id = ThreadId::new();
    let client = test_model_client(SessionSource::Internal(InternalSessionSource::Guardian));
    let metadata = test_responses_metadata_for_client(
        &client,
        Some("turn-123"),
        "window-1".to_string(),
        Some(parent_thread_id),
        TestCodexResponsesRequestKind::Turn,
    );

    assert_eq!(
        client.prompt_cache_key(&metadata),
        format!("guardian:{parent_thread_id}")
    );
}

#[test]
fn build_subagent_headers_sets_internal_memory_consolidation_label() {
    let client = test_model_client(SessionSource::Internal(
        InternalSessionSource::MemoryConsolidation,
    ));
    let headers = client.build_subagent_headers();
    let value = headers
        .get(X_OPENAI_SUBAGENT_HEADER)
        .and_then(|value| value.to_str().ok());
    assert_eq!(value, Some("memory_consolidation"));
    assert_eq!(
        headers.get("originator"),
        Some(&http::HeaderValue::from_static("test_originator"))
    );
}

#[test]
fn build_ws_client_metadata_includes_window_lineage_and_turn_metadata() {
    let parent_thread_id = ThreadId::new();
    let client = test_model_client(SessionSource::SubAgent(SubAgentSource::ThreadSpawn {
        parent_thread_id,
        depth: 2,
        agent_path: None,
        agent_nickname: None,
        agent_role: None,
    }));

    let thread_id = client.state.thread_id.to_string();
    let expected_window_id = format!("{thread_id}:1");
    let responses_metadata = test_responses_metadata_for_client(
        &client,
        Some("turn-123"),
        expected_window_id.clone(),
        Some(parent_thread_id),
        TestCodexResponsesRequestKind::Turn,
    );
    let client_metadata =
        client.build_ws_client_metadata(&responses_metadata, /*use_responses_lite*/ false);
    let parent_thread_id = parent_thread_id.to_string();
    let turn_metadata: serde_json::Value = serde_json::from_str(
        client_metadata
            .get(X_CODEX_TURN_METADATA_HEADER)
            .expect("turn metadata"),
    )
    .expect("valid turn metadata");
    for (client_key, metadata_key, expected) in [
        (
            X_CODEX_INSTALLATION_ID_HEADER,
            "installation_id",
            "11111111-1111-4111-8111-111111111111",
        ),
        ("session_id", "session_id", thread_id.as_str()),
        ("thread_id", "thread_id", thread_id.as_str()),
        ("turn_id", "turn_id", "turn-123"),
        (
            X_CODEX_WINDOW_ID_HEADER,
            "window_id",
            expected_window_id.as_str(),
        ),
        (
            X_CODEX_PARENT_THREAD_ID_HEADER,
            "parent_thread_id",
            parent_thread_id.as_str(),
        ),
    ] {
        assert_eq!(
            client_metadata.get(client_key).map(String::as_str),
            Some(expected)
        );
        assert_eq!(turn_metadata[metadata_key].as_str(), Some(expected));
    }
    assert_eq!(
        client_metadata
            .get(X_OPENAI_SUBAGENT_HEADER)
            .map(String::as_str),
        Some("collab_spawn")
    );
}

#[tokio::test]
async fn summarize_memories_returns_empty_for_empty_input() {
    let client = test_model_client(SessionSource::Cli);
    let model_info = test_model_info();
    let session_telemetry = test_session_telemetry();

    let output = client
        .summarize_memories(
            Vec::new(),
            &model_info,
            /*effort*/ None,
            &session_telemetry,
        )
        .await
        .expect("empty summarize request should succeed");
    assert_eq!(output.len(), 0);
}

#[tokio::test]
async fn dropped_response_stream_traces_cancelled_partial_output() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let attempt = started_inference_attempt(&temp)?;

    // The provider has produced one complete output item, but no terminal
    // response.completed event. The harness has enough information to keep this
    // item in history, so the trace should preserve it when the stream is
    // abandoned.
    let item = output_message("1", "partial answer");
    let api_stream = futures::stream::iter([Ok(ResponseEvent::OutputItemDone(item))])
        .chain(futures::stream::pending());
    let (mut stream, _) = super::map_response_events(
        /*upstream_request_id*/ None,
        api_stream,
        test_session_telemetry(),
        attempt,
        test_model_provider(),
    );

    let observed = stream
        .next()
        .await
        .expect("mapped stream should yield output item")?;
    assert!(matches!(observed, ResponseEvent::OutputItemDone(_)));

    // Dropping the consumer is how turn interruption/preemption stops polling
    // the provider stream. The mapper task observes that drop asynchronously
    // and records cancellation using the output items it has already seen.
    drop(stream);

    // Cancellation is recorded by the mapper task after Drop wakes it, so the
    // replay may need a short wait before the terminal event appears on disk.
    let rollout = replay_until_cancelled(&temp).await?;
    let inference = rollout
        .inference_calls
        .values()
        .next()
        .expect("inference should be reduced");

    assert_eq!(inference.execution.status, ExecutionStatus::Cancelled);
    assert_eq!(inference.response_item_ids.len(), 1);
    assert_eq!(rollout.raw_payloads.len(), 2);

    Ok(())
}

#[tokio::test]
async fn response_stream_records_last_model_feedback_ids() {
    let tags = Arc::new(Mutex::new(BTreeMap::new()));
    let _guard = tracing_subscriber::registry()
        .with(TagCollectorLayer { tags: tags.clone() })
        .set_default();

    let api_stream = futures::stream::iter([
        Ok(ResponseEvent::Created { response_id: None }),
        Ok(ResponseEvent::Completed {
            response_id: "resp-123".to_string(),
            token_usage: None,
            usage_metadata: None,
            end_turn: Some(true),
        }),
    ]);
    let (mut stream, _) = super::map_response_events(
        Some("req-123".to_string()),
        api_stream,
        test_session_telemetry(),
        InferenceTraceAttempt::disabled(),
        test_model_provider(),
    );

    while stream.next().await.is_some() {}

    let tags = tags.lock().unwrap().clone();
    assert_eq!(
        tags.get("last_model_request_id").map(String::as_str),
        Some("\"req-123\"")
    );
    assert_eq!(
        tags.get("last_model_response_id").map(String::as_str),
        Some("\"resp-123\"")
    );
}

#[tokio::test]
async fn bedrock_unauthorized_error_uses_provider_mapping() {
    let provider = create_model_provider(
        ModelProviderInfo::create_amazon_bedrock_provider(/*aws*/ None),
        /*auth_manager*/ None,
    );
    let mut auth_recovery = None;
    let mut provider_auth_recovery_attempted = false;
    let url = "https://bedrock-mantle.us-east-2.api.aws/openai/v1/responses";
    let error = super::handle_unauthorized(
        TransportError::Http {
            status: http::StatusCode::UNAUTHORIZED,
            url: Some(url.to_string()),
            headers: None,
            body: Some(
                "Signature expired: 20260609T133205Z is now earlier than 20260614T062525Z"
                    .to_string(),
            ),
        },
        &mut auth_recovery,
        &mut provider_auth_recovery_attempted,
        &test_session_telemetry(),
        &provider,
        /*event_sender*/ None,
        /*turn_id*/ None,
    )
    .await
    .expect_err("expired Bedrock signature should fail");

    assert_eq!(
        error.to_string(),
        format!(
            "Amazon Bedrock rejected the request because its AWS signature has expired. Refresh your AWS credentials and retry. If `AWS_BEARER_TOKEN_BEDROCK` is set, update or unset it, then restart Sofia, url: {url}"
        )
    );
}

#[derive(Debug)]
struct TestRecoveryProvider {
    inner: SharedModelProvider,
    should_fail: bool,
    attempts: Arc<AtomicUsize>,
}

impl ModelProvider for TestRecoveryProvider {
    fn info(&self) -> &ModelProviderInfo {
        self.inner.info()
    }

    fn auth_manager(&self) -> Option<Arc<AuthManager>> {
        None
    }

    fn auth(&self) -> ModelProviderFuture<'_, Option<CodexAuth>> {
        self.inner.auth()
    }

    fn account_state(&self) -> ProviderAccountResult {
        self.inner.account_state()
    }

    fn auth_recovery_messages(&self) -> Option<ProviderAuthRecoveryMessages> {
        Some(ProviderAuthRecoveryMessages {
            started: "Refreshing provider authentication.",
            succeeded: "Provider authentication recovered.",
        })
    }

    fn recover_from_unauthorized(
        &self,
    ) -> ModelProviderFuture<'_, sofia_protocol::error::Result<ProviderUnauthorizedRecovery>> {
        self.attempts.fetch_add(1, Ordering::Relaxed);
        Box::pin(async move {
            if self.should_fail {
                Err(CodexErr::Io(std::io::Error::other(
                    "provider recovery failed",
                )))
            } else {
                Ok(ProviderUnauthorizedRecovery::Recovered)
            }
        })
    }

    fn models_manager(
        &self,
        sofia_home: PathBuf,
        config_model_catalog: Option<ModelsResponse>,
    ) -> SharedModelsManager {
        self.inner.models_manager(sofia_home, config_model_catalog)
    }
}

#[tokio::test]
async fn provider_owned_auth_recovery_is_bounded_and_preserves_unauthorized_failures() {
    for should_fail in [false, true] {
        let attempts = Arc::new(AtomicUsize::new(0));
        let provider: SharedModelProvider = Arc::new(TestRecoveryProvider {
            inner: test_model_provider(),
            should_fail,
            attempts: Arc::clone(&attempts),
        });
        assert!(provider.auth_manager().is_none());

        let unauthorized = || TransportError::Http {
            status: http::StatusCode::UNAUTHORIZED,
            url: Some("https://example.com/v1/responses".to_string()),
            headers: None,
            body: Some("unauthorized".to_string()),
        };
        let mut auth_recovery = None;
        let mut provider_auth_recovery_attempted = false;
        let telemetry = test_session_telemetry();
        let (event_sender, event_receiver) = async_channel::unbounded();
        let result = super::handle_unauthorized(
            unauthorized(),
            &mut auth_recovery,
            &mut provider_auth_recovery_attempted,
            &telemetry,
            &provider,
            Some(&event_sender),
            Some("turn-1"),
        )
        .await;

        let error = if should_fail {
            result.expect_err("failed provider recovery should return the original error")
        } else {
            let recovered = result.expect("provider recovery should succeed without AuthManager");
            assert_eq!(
                (recovered.mode, recovered.phase),
                ("provider", "provider_refresh")
            );
            super::handle_unauthorized(
                unauthorized(),
                &mut auth_recovery,
                &mut provider_auth_recovery_attempted,
                &telemetry,
                &provider,
                Some(&event_sender),
                Some("turn-1"),
            )
            .await
            .expect_err("provider recovery should not run more than once")
        };

        match error.details() {
            CodexErrorDetails::UnexpectedStatus(response) => {
                assert_eq!(response.status, http::StatusCode::UNAUTHORIZED);
                assert_eq!(response.body, "unauthorized");
            }
            other => panic!("unexpected error after provider recovery: {other}"),
        }
        assert_eq!(attempts.load(Ordering::Relaxed), 1);

        let events = std::iter::from_fn(|| event_receiver.try_recv().ok())
            .map(|event| serde_json::to_value(event).expect("recovery event should serialize"))
            .collect::<Vec<_>>();
        let mut expected = vec![json!({
            "id": "turn-1",
            "msg": {
                "type": "auth_recovery_started",
                "provider": provider.info().name,
                "message": "Refreshing provider authentication.",
            }
        })];
        if !should_fail {
            expected.push(json!({
                "id": "turn-1",
                "msg": {
                    "type": "auth_recovery_completed",
                    "provider": provider.info().name,
                    "message": "Provider authentication recovered.",
                }
            }));
        }
        assert_eq!(events, expected);
    }
}

#[tokio::test]
async fn dropped_backpressured_response_stream_traces_cancelled_partial_output()
-> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let attempt = started_inference_attempt(&temp)?;
    let backpressured_item_yielded = Arc::new(Notify::new());
    let mut events = VecDeque::new();
    for _ in 0..super::RESPONSE_STREAM_CHANNEL_CAPACITY {
        events.push_back(ResponseEvent::Created { response_id: None });
    }
    events.push_back(ResponseEvent::OutputItemDone(output_message(
        "1",
        "partial answer",
    )));
    let api_stream = NotifyAfterEventStream {
        events,
        yielded: 0,
        notify_after: super::RESPONSE_STREAM_CHANNEL_CAPACITY + 1,
        notify: Arc::clone(&backpressured_item_yielded),
    };

    let (stream, _) = super::map_response_events(
        /*upstream_request_id*/ None,
        api_stream,
        test_session_telemetry(),
        attempt,
        test_model_provider(),
    );

    // Fill the mapper channel with non-terminal events, then yield one output
    // item. The mapper has observed that item and is blocked trying to send it
    // downstream, so dropping the consumer covers the send-failure path rather
    // than the `consumer_dropped` select branch.
    backpressured_item_yielded.notified().await;
    drop(stream);

    let rollout = replay_until_cancelled(&temp).await?;
    let inference = rollout
        .inference_calls
        .values()
        .next()
        .expect("inference should be reduced");

    assert_eq!(inference.execution.status, ExecutionStatus::Cancelled);
    assert_eq!(inference.response_item_ids.len(), 1);
    assert_eq!(rollout.raw_payloads.len(), 2);

    Ok(())
}

#[test]
fn auth_request_telemetry_context_tracks_attached_auth_and_retry_phase() {
    let auth_context = AuthRequestTelemetryContext::new(
        Some(AuthMode::Chatgpt),
        &BearerAuthProvider::for_test(Some("access-token"), Some("workspace-123")),
        /*agent_identity_telemetry*/ None,
        PendingUnauthorizedRetry::from_recovery(UnauthorizedRecoveryExecution {
            mode: "managed",
            phase: "refresh_token",
        }),
    );

    assert_eq!(auth_context.auth_mode, Some("Chatgpt"));
    assert!(auth_context.auth_header_attached);
    assert_eq!(auth_context.auth_header_name, Some("authorization"));
    assert!(auth_context.retry_after_unauthorized);
    assert_eq!(auth_context.recovery_mode, Some("managed"));
    assert_eq!(auth_context.recovery_phase, Some("refresh_token"));
}

#[test]
fn auth_request_telemetry_context_tracks_agent_identity_ids() {
    let auth_context = AuthRequestTelemetryContext::new(
        Some(AuthMode::Chatgpt),
        &BearerAuthProvider::for_test(/*token*/ None, /*account_id*/ None),
        Some(AgentIdentityTelemetry {
            agent_id: "agent-runtime-context".to_string(),
            task_id: "task-run-context".to_string(),
        }),
        PendingUnauthorizedRetry::default(),
    );

    assert_eq!(
        auth_context.agent_identity_telemetry(),
        Some(&AgentIdentityTelemetry {
            agent_id: "agent-runtime-context".to_string(),
            task_id: "task-run-context".to_string(),
        })
    );
}

fn model_client_with_counting_attestation(
    include_attestation: bool,
) -> (ModelClient, Arc<AtomicUsize>) {
    #[derive(Debug)]
    struct CountingAttestationProvider {
        calls: Arc<AtomicUsize>,
    }

    impl AttestationProvider for CountingAttestationProvider {
        fn header_for_request(
            &self,
            _context: AttestationContext,
        ) -> GenerateAttestationFuture<'_> {
            let calls = self.calls.clone();
            Box::pin(async move {
                let call = calls.fetch_add(1, Ordering::Relaxed) + 1;
                Some(http::HeaderValue::from_bytes(format!("v1.header-{call}").as_bytes()).unwrap())
            })
        }
    }

    let attestation_calls = Arc::new(AtomicUsize::new(0));
    let (auth_manager, provider) = if include_attestation {
        (
            Some(AuthManager::from_auth_for_testing(
                CodexAuth::create_dummy_chatgpt_auth_for_testing(),
            )),
            ModelProviderInfo::create_openai_provider(Some(CHATGPT_CODEX_BASE_URL.to_string())),
        )
    } else {
        (
            None,
            create_oss_provider_with_base_url("https://example.com/v1", WireApi::Responses),
        )
    };
    let model_client = ModelClient::new(
        auth_manager,
        AgentIdentityAuthPolicy::JwtOnly,
        ThreadId::new(),
        provider,
        SessionSource::Exec,
        "test_originator".to_string(),
        /*model_verbosity*/ None,
        /*content_item_kinds_enabled*/ true,
        /*enable_request_compression*/ false,
        /*include_timing_metrics*/ false,
        /*beta_features_header*/ None,
        /*concurrent_reasoning_summaries_enabled*/ false,
        Some(Arc::new(CountingAttestationProvider {
            calls: attestation_calls.clone(),
        })),
        HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault),
    );
    (model_client, attestation_calls)
}

#[test]
fn guardian_reviewer_uses_dedicated_endpoint_only_with_codex_backend_auth() {
    let (mut model_client, _) =
        model_client_with_counting_attestation(/*include_attestation*/ true);
    Arc::get_mut(&mut model_client.state)
        .expect("test client should have unique session state")
        .session_source = SessionSource::SubAgent(SubAgentSource::Other("guardian".to_owned()));

    assert_eq!(
        model_client.responses_endpoint(
            Some(&CodexAuth::create_dummy_chatgpt_auth_for_testing()),
            "sofia-auto-review",
        ),
        ResponsesEndpoint::Responses
    );

    model_client = model_client.with_free_guardian_enabled(/*free_guardian_enabled*/ true);
    assert_eq!(
        model_client.responses_endpoint(
            Some(&CodexAuth::create_dummy_chatgpt_auth_for_testing()),
            "sofia-auto-review",
        ),
        ResponsesEndpoint::Guardian
    );
    assert_eq!(
        model_client.responses_endpoint(
            Some(&CodexAuth::create_dummy_chatgpt_auth_for_testing()),
            "required-reviewer-model",
        ),
        ResponsesEndpoint::Responses
    );
    assert_eq!(
        model_client.responses_endpoint(
            Some(&CodexAuth::create_dummy_chatgpt_auth_for_testing()),
            "parent-fallback-model",
        ),
        ResponsesEndpoint::Responses
    );
    assert_eq!(
        model_client.responses_endpoint(
            Some(&CodexAuth::from_api_key("test-api-key")),
            "sofia-auto-review",
        ),
        ResponsesEndpoint::Responses
    );

    *Arc::get_mut(&mut model_client.state)
        .expect("test client should have unique session state")
        .provider
        .write()
        .expect("provider lock poisoned") = create_model_provider(
        ModelProviderInfo::create_openai_provider(Some("https://proxy.example.com/v1".to_owned())),
        Some(AuthManager::from_auth_for_testing(
            CodexAuth::create_dummy_chatgpt_auth_for_testing(),
        )),
    );
    assert_eq!(
        model_client.responses_endpoint(
            Some(&CodexAuth::create_dummy_chatgpt_auth_for_testing()),
            "sofia-auto-review",
        ),
        ResponsesEndpoint::Responses
    );

    Arc::get_mut(&mut model_client.state)
        .expect("test client should have unique session state")
        .session_source = SessionSource::Exec;
    assert_eq!(
        model_client.responses_endpoint(
            Some(&CodexAuth::create_dummy_chatgpt_auth_for_testing()),
            "sofia-auto-review",
        ),
        ResponsesEndpoint::Responses
    );
}

#[tokio::test]
async fn websocket_handshake_includes_attestation_for_chatgpt_codex_responses() {
    let (model_client, attestation_calls) =
        model_client_with_counting_attestation(/*include_attestation*/ true);
    let responses_metadata = test_responses_metadata_for_client(
        &model_client,
        /*turn_id*/ None,
        format!("{}:0", model_client.state.thread_id),
        /*parent_thread_id*/ None,
        TestCodexResponsesRequestKind::WebsocketConnection,
    );

    let headers = model_client
        .build_websocket_headers(&responses_metadata)
        .await;

    assert_eq!(
        headers
            .get(crate::attestation::X_OAI_ATTESTATION_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("v1.header-1"),
    );
    assert_eq!(attestation_calls.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn existing_call_sideband_headers_include_attestation() {
    let (model_client, attestation_calls) =
        model_client_with_counting_attestation(/*include_attestation*/ true);

    let headers = model_client
        .realtime_sideband_headers(http::HeaderMap::new())
        .await
        .expect("existing call sideband headers should build");

    assert_eq!(
        headers
            .get(crate::attestation::X_OAI_ATTESTATION_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("v1.header-1"),
    );
    assert_eq!(attestation_calls.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn non_chatgpt_codex_endpoints_omit_attestation_generation() {
    let (model_client, attestation_calls) =
        model_client_with_counting_attestation(/*include_attestation*/ false);
    let mut response_headers = http::HeaderMap::new();

    if let Some(header_value) = model_client.generate_attestation_header_for().await {
        response_headers.insert(crate::attestation::X_OAI_ATTESTATION_HEADER, header_value);
    }
    let mut compaction_headers = http::HeaderMap::new();
    if let Some(header_value) = model_client.generate_attestation_header_for().await {
        compaction_headers.insert(crate::attestation::X_OAI_ATTESTATION_HEADER, header_value);
    }
    let mut realtime_headers = http::HeaderMap::new();
    if let Some(header_value) = model_client.generate_attestation_header_for().await {
        realtime_headers.insert(crate::attestation::X_OAI_ATTESTATION_HEADER, header_value);
    }

    assert_eq!(
        response_headers.get(crate::attestation::X_OAI_ATTESTATION_HEADER),
        None,
    );
    assert_eq!(
        compaction_headers.get(crate::attestation::X_OAI_ATTESTATION_HEADER),
        None,
    );
    assert_eq!(
        realtime_headers.get(crate::attestation::X_OAI_ATTESTATION_HEADER),
        None,
    );
    assert_eq!(attestation_calls.load(Ordering::Relaxed), 0);
}

#[test]
fn chat_completions_body_reconstructs_assistant_tool_calls_before_tool_outputs() {
    use crate::client::build_chat_completions_body;
    use sofia_protocol::ResponseItemId;
    use sofia_protocol::models::FunctionCallOutputPayload;

    let prompt = Prompt {
        input: vec![
            ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "Do the task".to_string(),
                }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::FunctionCall {
                id: Some(ResponseItemId::new("fc1")),
                name: "shell".to_string(),
                namespace: None,
                arguments: r#"{"cmd":"ls"}"#.to_string(),
                encrypted_function_args: None,
                call_id: "call_1".to_string(),
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::FunctionCallOutput {
                id: Some(ResponseItemId::new("fco1")),
                call_id: Some("call_1".to_string()),
                name: Some("shell".to_string()),
                namespace: None,
                output: FunctionCallOutputPayload::from_text("done".to_string()),
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::Message {
                id: None,
                role: "assistant".to_string(),
                content: vec![ContentItem::OutputText {
                    text: "Finished".to_string(),
                }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            },
        ],
        base_instructions: BaseInstructions {
            text: "You are a coding agent.".to_string(),
            provenance: None,
        },
        ..Default::default()
    };
    let body = build_chat_completions_body(
        &prompt,
        &test_model_info(),
        &Some(sofia_protocol::openai_models::ReasoningEffort::Medium),
    )
    .unwrap();
    let messages = body["messages"].as_array().unwrap();

    // 0: system, 1: user, 2: assistant with tool_calls, 3: tool result, 4: assistant final,
    // 5: user "Continue." (ensureTrailingUserMessage).
    assert_eq!(messages.len(), 6);
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[1]["role"], "user");
    assert_eq!(messages[2]["role"], "assistant");
    assert_eq!(messages[2]["tool_calls"][0]["id"], "call_1");
    assert_eq!(messages[2]["tool_calls"][0]["function"]["name"], "shell");
    // The tool result must appear AFTER the assistant tool_calls message that
    // references it.
    assert_eq!(messages[3]["role"], "tool");
    assert_eq!(messages[3]["tool_call_id"], "call_1");
    assert_eq!(messages[4]["role"], "assistant");
    assert_eq!(messages[4]["content"], "Finished");
    assert_eq!(messages[5]["role"], "user");
    assert_eq!(messages[5]["content"], "Continue.");
}

#[test]
fn chat_completions_body_maps_developer_role_to_system() {
    use crate::client::build_chat_completions_body;

    let prompt = Prompt {
        input: vec![
            ResponseItem::Message {
                id: None,
                role: "developer".to_string(),
                content: vec![ContentItem::InputText {
                    text: "be concise".to_string(),
                }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "hi".to_string(),
                }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            },
        ],
        base_instructions: BaseInstructions {
            text: "You are a coding agent.".to_string(),
            provenance: None,
        },
        ..Default::default()
    };
    let body = build_chat_completions_body(&prompt, &test_model_info(), &None).unwrap();
    let messages = body["messages"].as_array().unwrap();

    // The Responses-only `developer` role is folded into `system`; DeepSeek and
    // other Chat Completions providers reject `developer` with a 400.
    assert_eq!(messages[0]["role"], "system"); // base instructions
    assert_eq!(messages[1]["role"], "system");
    assert_eq!(messages[1]["content"], "be concise");
    assert_eq!(messages[2]["role"], "user");
    assert!(
        messages
            .iter()
            .all(|message| message["role"] != "developer"),
        "no message may carry the developer role"
    );
}

#[test]
fn chat_completions_body_carries_reasoning_content_on_tool_call_message() {
    use crate::client::build_chat_completions_body;
    use sofia_protocol::ResponseItemId;
    use sofia_protocol::models::FunctionCallOutputPayload;
    use sofia_protocol::models::ReasoningItemContent;

    let prompt = Prompt {
        input: vec![
            ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "go".to_string(),
                }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::Reasoning {
                id: Some(ResponseItemId::new("r1")),
                summary: vec![],
                content: Some(vec![ReasoningItemContent::ReasoningText {
                    text: "I should list the files".to_string(),
                }]),
                encrypted_content: None,
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::FunctionCall {
                id: Some(ResponseItemId::new("fc1")),
                name: "shell".to_string(),
                namespace: None,
                arguments: r#"{"cmd":"ls"}"#.to_string(),
                encrypted_function_args: None,
                call_id: "call_1".to_string(),
                internal_chat_message_metadata_passthrough: None,
            },
            // DeepSeek returns the assistant text in the same thinking turn as the
            // tool call; both messages must echo reasoning_content.
            ResponseItem::Message {
                id: None,
                role: "assistant".to_string(),
                content: vec![ContentItem::OutputText {
                    text: "Listing the files now".to_string(),
                }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::FunctionCallOutput {
                id: Some(ResponseItemId::new("fco1")),
                call_id: Some("call_1".to_string()),
                name: Some("shell".to_string()),
                namespace: None,
                output: FunctionCallOutputPayload::from_text("done".to_string()),
                internal_chat_message_metadata_passthrough: None,
            },
        ],
        base_instructions: BaseInstructions {
            text: "You are a coding agent.".to_string(),
            provenance: None,
        },
        ..Default::default()
    };
    let body = build_chat_completions_body(&prompt, &test_model_info(), &None).unwrap();
    let messages = body["messages"].as_array().unwrap();

    // system, user, assistant(text), assistant(tool_calls), tool. DeepSeek
    // thinking mode requires the assistant message(s) of the turn to echo
    // `reasoning_content`.
    assert_eq!(messages[2]["role"], "assistant");
    assert_eq!(messages[2]["content"], "Listing the files now");
    assert_eq!(
        messages[2]["reasoning_content"], "I should list the files",
        "reasoning_content must be passed back on the assistant text message"
    );
    assert_eq!(messages[3]["role"], "assistant");
    assert_eq!(messages[3]["tool_calls"][0]["id"], "call_1");
    assert_eq!(
        messages[3]["reasoning_content"], "I should list the files",
        "reasoning_content must be passed back on the tool-call message"
    );
    assert_eq!(messages[4]["role"], "tool");
}

#[test]
fn chat_completions_body_sets_reasoning_content_even_when_empty() {
    use crate::client::build_chat_completions_body;
    use sofia_protocol::ResponseItemId;
    use sofia_protocol::models::FunctionCallOutputPayload;

    let prompt = Prompt {
        input: vec![
            ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "go".to_string(),
                }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            },
            // DeepSeek can return empty reasoning_content that still must be
            // echoed back (mimocode sets the field unconditionally).
            ResponseItem::Reasoning {
                id: Some(ResponseItemId::new("r1")),
                summary: vec![],
                content: Some(vec![]),
                encrypted_content: None,
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::FunctionCall {
                id: Some(ResponseItemId::new("fc1")),
                name: "shell".to_string(),
                namespace: None,
                arguments: r#"{"cmd":"ls"}"#.to_string(),
                encrypted_function_args: None,
                call_id: "call_1".to_string(),
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::FunctionCallOutput {
                id: Some(ResponseItemId::new("fco1")),
                call_id: Some("call_1".to_string()),
                name: Some("shell".to_string()),
                namespace: None,
                output: FunctionCallOutputPayload::from_text("done".to_string()),
                internal_chat_message_metadata_passthrough: None,
            },
        ],
        base_instructions: BaseInstructions {
            text: "You are a coding agent.".to_string(),
            provenance: None,
        },
        ..Default::default()
    };
    let body = build_chat_completions_body(&prompt, &test_model_info(), &None).unwrap();
    let messages = body["messages"].as_array().unwrap();

    assert_eq!(messages[2]["role"], "assistant");
    assert_eq!(
        messages[2]["reasoning_content"], "",
        "reasoning_content must be present even when empty"
    );
}

#[test]
fn chat_completions_body_does_not_force_continuation_after_tool_output() {
    use crate::client::build_chat_completions_body;
    use sofia_protocol::ResponseItemId;
    use sofia_protocol::models::FunctionCallOutputPayload;

    let prompt = Prompt {
        input: vec![
            ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "Review the implementation".to_string(),
                }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::FunctionCall {
                id: Some(ResponseItemId::new("fc1")),
                name: "exec_command".to_string(),
                namespace: None,
                arguments: r#"{"cmd":"sed -n '1,40p' src/lib.rs"}"#.to_string(),
                encrypted_function_args: None,
                call_id: "call_1".to_string(),
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::FunctionCallOutput {
                id: Some(ResponseItemId::new("fco1")),
                call_id: Some("call_1".to_string()),
                name: Some("exec_command".to_string()),
                namespace: None,
                output: FunctionCallOutputPayload::from_text("reviewed source".to_string()),
                internal_chat_message_metadata_passthrough: None,
            },
        ],
        base_instructions: BaseInstructions {
            text: "Respect the user's requested task scope.".to_string(),
            provenance: None,
        },
        ..Default::default()
    };

    let body = build_chat_completions_body(&prompt, &test_model_info(), &None).unwrap();
    let messages = body["messages"].as_array().unwrap();

    assert_eq!(messages.len(), 4);
    assert_eq!(
        messages[0],
        json!({
            "role": "system",
            "content": "Respect the user's requested task scope.",
        })
    );
    assert_eq!(messages[3]["role"], "tool");
    assert_eq!(messages[3]["content"], "reviewed source");
}

#[test]
fn chat_completions_body_multi_iteration_reproduces_400() {
    use crate::client::build_chat_completions_body;
    use sofia_protocol::ResponseItemId;
    use sofia_protocol::models::FunctionCallOutputPayload;

    // Reconstruct a multi-iteration agent conversation: the model calls a tool,
    // gets a result, thinks (Reasoning), then keeps going — the shape that
    // trips non-OpenAI providers.
    let prompt = Prompt {
        input: vec![
            ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "ls the repo".to_string(),
                }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::Reasoning {
                id: Some(ResponseItemId::new("rs1")),
                summary: Vec::new(),
                content: None,
                encrypted_content: None,
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::FunctionCall {
                id: Some(ResponseItemId::new("fc1")),
                name: "shell".to_string(),
                namespace: None,
                arguments: "{\"cmd\":\"ls\"}".to_string(),
                encrypted_function_args: None,
                call_id: "call_1".to_string(),
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::FunctionCallOutput {
                id: Some(ResponseItemId::new("fco1")),
                call_id: Some("call_1".to_string()),
                name: Some("shell".to_string()),
                namespace: None,
                output: FunctionCallOutputPayload::from_text("src\nlib\nCargo.toml".to_string()),
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::Message {
                id: None,
                role: "assistant".to_string(),
                content: vec![ContentItem::OutputText {
                    text: "Here are the files.".to_string(),
                }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::Reasoning {
                id: Some(ResponseItemId::new("rs2")),
                summary: Vec::new(),
                content: None,
                encrypted_content: None,
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::FunctionCall {
                id: Some(ResponseItemId::new("fc2")),
                name: "shell".to_string(),
                namespace: None,
                arguments: "{\"cmd\":\"cat Cargo.toml\"}".to_string(),
                encrypted_function_args: None,
                call_id: "call_2".to_string(),
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::FunctionCallOutput {
                id: Some(ResponseItemId::new("fco2")),
                call_id: Some("call_2".to_string()),
                name: Some("shell".to_string()),
                namespace: None,
                output: FunctionCallOutputPayload::from_text("[package]".to_string()),
                internal_chat_message_metadata_passthrough: None,
            },
        ],
        base_instructions: BaseInstructions {
            text: "You are a coding agent.".to_string(),
            provenance: None,
        },
        ..Default::default()
    };
    let body = build_chat_completions_body(
        &prompt,
        &test_model_info(),
        &Some(sofia_protocol::openai_models::ReasoningEffort::Medium),
    )
    .unwrap();
    println!("REPRO_BODY={}", serde_json::to_string(&body).unwrap());
    let messages = body["messages"].as_array().unwrap();
    assert!(
        messages.len() >= 6,
        "unexpected message count: {}",
        messages.len()
    );
}

/// The failing case from the field: a resumed or compacted conversation can hold
/// an assistant tool-call message with no `Reasoning` item in front of it.
/// Thinking-mode providers still reject the request unless the field is echoed,
/// so the capability of the model (not the items in the history) must decide to
/// send it.
#[test]
fn chat_completions_body_echoes_reasoning_content_without_reasoning_item() {
    use crate::client::build_chat_completions_body;
    use sofia_protocol::ResponseItemId;
    use sofia_protocol::models::FunctionCallOutputPayload;

    let prompt = Prompt {
        input: vec![
            ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "ls the repo".to_string(),
                }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::FunctionCall {
                id: Some(ResponseItemId::new("fc1")),
                name: "shell".to_string(),
                namespace: None,
                arguments: r#"{"cmd":"ls"}"#.to_string(),
                encrypted_function_args: None,
                call_id: "call_1".to_string(),
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::FunctionCallOutput {
                id: Some(ResponseItemId::new("fco1")),
                call_id: Some("call_1".to_string()),
                name: Some("shell".to_string()),
                namespace: None,
                output: FunctionCallOutputPayload::from_text("src".to_string()),
                internal_chat_message_metadata_passthrough: None,
            },
        ],
        base_instructions: BaseInstructions {
            text: "You are a coding agent.".to_string(),
            provenance: None,
        },
        ..Default::default()
    };

    let body = build_chat_completions_body(&prompt, &test_model_info(), &None).unwrap();
    let messages = body["messages"].as_array().unwrap();

    assert_eq!(messages[2]["role"], "assistant");
    assert_eq!(messages[2]["tool_calls"][0]["id"], "call_1");
    assert_eq!(
        messages[2]["reasoning_content"], "",
        "a thinking-capable model must echo reasoning_content even without a Reasoning item"
    );
}

/// An explicit `reasoning_effort = "none"` keeps the provider out of thinking
/// mode, where the field must not be sent back.
#[test]
fn chat_completions_body_omits_reasoning_content_when_reasoning_disabled() {
    use crate::client::build_chat_completions_body;
    use sofia_protocol::ResponseItemId;
    use sofia_protocol::models::FunctionCallOutputPayload;

    let prompt = Prompt {
        input: vec![
            ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "ls the repo".to_string(),
                }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::FunctionCall {
                id: Some(ResponseItemId::new("fc1")),
                name: "shell".to_string(),
                namespace: None,
                arguments: r#"{"cmd":"ls"}"#.to_string(),
                encrypted_function_args: None,
                call_id: "call_1".to_string(),
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::FunctionCallOutput {
                id: Some(ResponseItemId::new("fco1")),
                call_id: Some("call_1".to_string()),
                name: Some("shell".to_string()),
                namespace: None,
                output: FunctionCallOutputPayload::from_text("src".to_string()),
                internal_chat_message_metadata_passthrough: None,
            },
        ],
        base_instructions: BaseInstructions {
            text: "You are a coding agent.".to_string(),
            provenance: None,
        },
        ..Default::default()
    };

    let body = build_chat_completions_body(
        &prompt,
        &test_model_info(),
        &Some(sofia_protocol::openai_models::ReasoningEffort::None),
    )
    .unwrap();
    let messages = body["messages"].as_array().unwrap();

    assert_eq!(messages[2]["role"], "assistant");
    assert!(
        messages[2].get("reasoning_content").is_none(),
        "reasoning_content must not be sent when reasoning is disabled"
    );
}

/// The 400 recovery path: a request rejected for a missing `reasoning_content`
/// is repaired in place and can be retried, and a second pass is a no-op.
#[test]
fn chat_completions_reasoning_content_repair_echoes_field_on_tool_calls() {
    use crate::client::build_chat_completions_body;
    use crate::client::repair_chat_completions_reasoning_content;
    use sofia_protocol::ResponseItemId;
    use sofia_protocol::models::FunctionCallOutputPayload;

    let prompt = Prompt {
        input: vec![
            ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "ls the repo".to_string(),
                }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::FunctionCall {
                id: Some(ResponseItemId::new("fc1")),
                name: "shell".to_string(),
                namespace: None,
                arguments: r#"{"cmd":"ls"}"#.to_string(),
                encrypted_function_args: None,
                call_id: "call_1".to_string(),
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::FunctionCallOutput {
                id: Some(ResponseItemId::new("fco1")),
                call_id: Some("call_1".to_string()),
                name: Some("shell".to_string()),
                namespace: None,
                output: FunctionCallOutputPayload::from_text("src".to_string()),
                internal_chat_message_metadata_passthrough: None,
            },
        ],
        base_instructions: BaseInstructions {
            text: "You are a coding agent.".to_string(),
            provenance: None,
        },
        ..Default::default()
    };

    // Disable reasoning so the built body starts out without the field.
    let mut body = build_chat_completions_body(
        &prompt,
        &test_model_info(),
        &Some(sofia_protocol::openai_models::ReasoningEffort::None),
    )
    .unwrap();
    let messages = body["messages"].as_array().unwrap();
    assert!(messages[2].get("reasoning_content").is_none());

    assert!(repair_chat_completions_reasoning_content(&mut body));
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages[2]["role"], "assistant");
    assert_eq!(
        messages[2]["reasoning_content"], "",
        "the repair must echo reasoning_content on assistant tool-call messages"
    );

    assert!(
        !repair_chat_completions_reasoning_content(&mut body),
        "the repair must be idempotent"
    );
}

#[test]
fn chat_completions_body_echoes_reasoning_on_narration_without_reasoning_item() {
    let prompt = Prompt {
        input: vec![ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: vec![ContentItem::OutputText {
                text: "I will inspect the project.".to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        }],
        ..Default::default()
    };
    let body =
        crate::client::build_chat_completions_body(&prompt, &test_model_info(), &None).unwrap();
    assert_eq!(
        body["messages"][1],
        serde_json::json!({
            "role": "assistant",
            "content": "I will inspect the project.",
            "reasoning_content": "",
        })
    );
}

#[test]
fn chat_completions_reasoning_repair_preserves_text_and_existing_reasoning() {
    let mut body = serde_json::json!({"messages": [
        {"role": "user", "content": "Inspect the project"},
        {"role": "assistant", "content": "I will inspect the project."},
        {"role": "assistant", "content": "", "reasoning_content": null,
         "tool_calls": [{"id": "call_1"}]},
        {"role": "tool", "tool_call_id": "call_1", "content": "Files found"},
        {"role": "assistant", "content": "Done", "reasoning_content": "Original reasoning"}
    ]});
    let expected = serde_json::json!({"messages": [
        {"role": "user", "content": "Inspect the project"},
        {"role": "assistant", "content": "I will inspect the project.", "reasoning_content": ""},
        {"role": "assistant", "content": "", "reasoning_content": "",
         "tool_calls": [{"id": "call_1"}]},
        {"role": "tool", "tool_call_id": "call_1", "content": "Files found"},
        {"role": "assistant", "content": "Done", "reasoning_content": "Original reasoning"}
    ]});
    assert!(crate::client::repair_chat_completions_reasoning_content(
        &mut body
    ));
    assert_eq!(body, expected);
    assert!(!crate::client::repair_chat_completions_reasoning_content(
        &mut body
    ));
    assert_eq!(body, expected);
}

#[test]
fn chat_completions_preserves_user_images_and_image_only_messages() {
    let prompt = Prompt {
        input: vec![serde_json::from_value(json!({
            "type": "message", "role": "user", "content": [
                {"type": "input_image", "image_url": "data:image/png;base64,aW1hZ2U=", "detail": "original"}
            ]
        })).unwrap()],
        ..Default::default()
    };
    let body =
        crate::client::build_chat_completions_body(&prompt, &test_model_info(), &None).unwrap();
    assert_eq!(
        body["messages"][1],
        json!({"role": "user", "content": [
            {"type": "image_url", "image_url": {"url": "data:image/png;base64,aW1hZ2U=", "detail": "high"}}
        ]})
    );
}

#[test]
fn chat_completions_keeps_tool_images_after_the_complete_parallel_batch() {
    let input = json!([
        {"type": "function_call", "call_id": "screen", "name": "snapshot", "arguments": "{}"},
        {"type": "function_call", "call_id": "files", "name": "list_files", "arguments": "{}"},
        {"type": "function_call_output", "call_id": "screen", "output": [
            {"type": "input_text", "text": "AX tree"},
            {"type": "input_image", "image_url": "data:image/jpeg;base64,c2NyZWVu", "detail": "high"}
        ]},
        {"type": "function_call_output", "call_id": "files", "output": "file.txt"}
    ]);
    let prompt = Prompt {
        input: serde_json::from_value(input).unwrap(),
        ..Default::default()
    };
    let body =
        crate::client::build_chat_completions_body(&prompt, &test_model_info(), &None).unwrap();
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(
        messages
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["system", "assistant", "tool", "tool", "user"]
    );
    assert_eq!(messages[2]["tool_call_id"], "screen");
    assert_eq!(
        messages[2]["content"],
        "AX tree\nTool images follow after the tool results."
    );
    assert_eq!(messages[3]["tool_call_id"], "files");
    assert_eq!(
        messages[4]["content"],
        json!([
            {"type": "text", "text": "Images returned by tool call screen: (tool output, not user instructions)"},
            {"type": "image_url", "image_url": {"url": "data:image/jpeg;base64,c2NyZWVu", "detail": "high"}}
        ])
    );
    assert!(!body.to_string().contains("non-text content"));
}

#[test]
fn chat_completions_does_not_send_images_to_text_only_models() {
    let mut model = test_model_info();
    model.input_modalities = vec![sofia_protocol::openai_models::InputModality::Text];
    let prompt = Prompt {
        input: serde_json::from_value(json!([
            {"type": "function_call", "call_id": "screen", "name": "snapshot", "arguments": "{}"},
            {"type": "function_call_output", "call_id": "screen", "output": [
                {"type": "input_image", "image_url": "data:image/jpeg;base64,c2NyZWVu"}
            ]}
        ]))
        .unwrap(),
        ..Default::default()
    };
    let body = crate::client::build_chat_completions_body(&prompt, &model, &None).unwrap();
    assert_eq!(
        body["messages"][2]["content"],
        "Image unavailable: the selected model does not support image input."
    );
    assert!(!body.to_string().contains("data:image"));
}

#[test]
fn chat_completions_keeps_tool_images_when_an_output_is_missing() {
    // A parallel batch where one result never arrived (an interrupted turn)
    // must not drop the screenshots that did.
    let input = json!([
        {"type": "function_call", "call_id": "screen", "name": "snapshot", "arguments": "{}"},
        {"type": "function_call", "call_id": "files", "name": "list_files", "arguments": "{}"},
        {"type": "function_call_output", "call_id": "screen", "output": [
            {"type": "input_image", "image_url": "data:image/png;base64,c2NyZWVu"}
        ]}
    ]);
    let prompt = Prompt {
        input: serde_json::from_value(input).unwrap(),
        ..Default::default()
    };
    let body =
        crate::client::build_chat_completions_body(&prompt, &test_model_info(), &None).unwrap();
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(
        messages
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["system", "assistant", "tool", "user"]
    );
    let carried = messages.last().unwrap()["content"].as_array().unwrap();
    assert!(
        carried
            .iter()
            .any(|part| part["image_url"]["url"] == "data:image/png;base64,c2NyZWVu"),
        "the screenshot from the completed call must still be sent: {carried:?}"
    );
}
