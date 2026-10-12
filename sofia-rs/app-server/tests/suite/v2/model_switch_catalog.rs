//! End-to-end coverage for provider switches and the model catalog.
//!
//! A provider switch must replace the catalog, not just the provider. The catalog
//! is provider-owned: it is fetched from the active provider's `/models` and is
//! keyed by that provider on disk. Swapping only the provider left the previous
//! provider's catalog installed, so later turns resolved model metadata — and
//! therefore the entire tool plan, including web search — against the provider the
//! user had just navigated away from.
//!
//! Each provider is backed by its own mock server serving its own `/models`
//! catalog, so a stale catalog shows up in the tool plan itself.
//! `shell_type` is the signal: a `Disabled` entry drops the shell tool from the
//! advertised plan, so a catalog resolved from the wrong provider produces a
//! visibly different plan — and a switched session must match a fresh session on
//! the destination provider tool-for-tool.
//!
//! Providers use command auth rather than an `env_key` API key on purpose:
//! `should_refresh_models()` requires `uses_codex_backend() || has_command_auth()`,
//! and `AuthMode::ApiKey.uses_codex_backend()` is false. Under API-key auth no
//! provider fetches `/models`, both catalogs stay empty, and every tool-plan
//! comparison below becomes vacuous.

use anyhow::Context;
use anyhow::Result;
use app_test_support::MockResponsesConfig;
use app_test_support::TestAppServer;
use core_test_support::responses;
use core_test_support::responses::ResponseMock;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use sofia_app_server_protocol::ThreadSettingsUpdateParams;
use sofia_app_server_protocol::ThreadSettingsUpdateResponse;
use sofia_app_server_protocol::ThreadSettingsUpdatedNotification;
use sofia_app_server_protocol::ThreadStartParams;
use sofia_app_server_protocol::ThreadStartResponse;
use sofia_app_server_protocol::TurnStartParams;
use sofia_app_server_protocol::TurnStartResponse;
use sofia_app_server_protocol::UserInput as V2UserInput;
use sofia_features::Feature;
use sofia_models_manager::bundled_models_response;
use sofia_protocol::openai_models::ConfigShellToolType;
use sofia_protocol::openai_models::ModelInfo;
use sofia_protocol::openai_models::ModelsResponse;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path_regex;

const DEEPSEEK_PROVIDER: &str = "provider_deepseek";
const OPENROUTER_PROVIDER: &str = "provider_openrouter";
const DEEPSEEK_MODEL: &str = "switch-model-deepseek";
const SPACE_BUNNY_MODEL: &str = "switch-model-space-bunny";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);

/// Command auth makes the models manager fetch this provider's `/models`
/// catalog. An `env_key` API key does not: `AuthMode::ApiKey.uses_codex_backend()`
/// is false, which would pin the manager to the bundled/static catalog and hide
/// the very staleness these tests exist to catch.
const COMMAND_AUTH: &str = "auth = { command = \"echo\", args = [\"test-token\"] }";

/// A provider switch must resolve the destination model's metadata from the
/// destination provider's catalog, not the provider the session started on.
#[tokio::test]
async fn provider_switch_resolves_destination_model_metadata() -> Result<()> {
    let mut harness = Harness::start(
        vec![TurnScript::Message("deepseek turn")],
        vec![TurnScript::Message("space bunny turn")],
    )
    .await?;

    let thread = harness
        .start_thread(DEEPSEEK_PROVIDER, DEEPSEEK_MODEL)
        .await?;
    harness.run_turn(&thread, "turn on deepseek").await?;
    let deepseek_tools = harness.deepseek.last_tools()?;
    assert!(
        tool_names(&deepseek_tools).contains(&"exec_command".to_string()),
        "the outgoing provider's catalog advertises the shell tool; if this fails \
         the test's discriminating signal is not working"
    );

    harness
        .switch_provider(&thread, OPENROUTER_PROVIDER, SPACE_BUNNY_MODEL)
        .await?;
    harness
        .run_turn_expecting_catalog(&thread, "turn on space bunny")
        .await?;
    let switched_tools = harness.openrouter.last_tools()?;

    assert_eq!(
        harness.openrouter.last_model()?,
        SPACE_BUNNY_MODEL,
        "the switched turn must be served by the new provider"
    );
    assert!(
        !tool_names(&switched_tools).contains(&"exec_command".to_string()),
        "the switched turn still advertises the outgoing provider's shell tool, so \
         it resolved model metadata from the stale catalog. Got {:?}",
        tool_names(&switched_tools)
    );

    Ok(())
}

/// The switched-to model must resolve identically whether it was reached by a
/// mid-thread switch or by starting a fresh session on that provider. This
/// parity assertion fails when only the provider is swapped.
#[tokio::test]
async fn switched_model_matches_fresh_session_model_plan() -> Result<()> {
    let mut harness = Harness::start(
        vec![TurnScript::Message("deepseek turn")],
        vec![
            TurnScript::Message("space bunny turn"),
            TurnScript::Message("fresh space bunny turn"),
        ],
    )
    .await?;

    // Path A: start on DeepSeek, then switch to Space Bunny mid-thread.
    let switched_thread = harness
        .start_thread(DEEPSEEK_PROVIDER, DEEPSEEK_MODEL)
        .await?;
    harness.run_turn(&switched_thread, "deepseek turn").await?;
    harness
        .switch_provider(&switched_thread, OPENROUTER_PROVIDER, SPACE_BUNNY_MODEL)
        .await?;
    harness
        .run_turn_expecting_catalog(&switched_thread, "space bunny turn")
        .await?;
    let switched = harness.openrouter.last_tools()?;

    // Path B: a brand new session that starts directly on Space Bunny.
    let fresh_thread = harness
        .start_thread(OPENROUTER_PROVIDER, SPACE_BUNNY_MODEL)
        .await?;
    harness
        .run_turn(&fresh_thread, "fresh space bunny turn")
        .await?;
    let fresh = harness.openrouter.last_tools()?;

    // Compare tool names so a failure names the differing tool instead of
    // dumping two multi-thousand-line JSON blobs.
    let switched_names = tool_names(&switched);
    let fresh_names = tool_names(&fresh);
    assert_eq!(
        switched_names, fresh_names,
        "a switched-to model must produce the same tool plan as a session that \
         started on that provider; only the provider was swapped, not the catalog"
    );

    Ok(())
}

/// The reverse direction must work too, otherwise a one-way fix hides the bug.
#[tokio::test]
async fn provider_switch_back_resolves_original_model_metadata() -> Result<()> {
    let mut harness = Harness::start(
        vec![TurnScript::Message("deepseek turn again")],
        vec![TurnScript::Message("space bunny turn")],
    )
    .await?;

    let thread = harness
        .start_thread(OPENROUTER_PROVIDER, SPACE_BUNNY_MODEL)
        .await?;
    harness.run_turn(&thread, "space bunny turn").await?;
    let space_bunny_tools = harness.openrouter.last_tools()?;
    assert!(
        !tool_names(&space_bunny_tools).contains(&"exec_command".to_string()),
        "the starting provider's catalog disables the shell tool"
    );

    harness
        .switch_provider(&thread, DEEPSEEK_PROVIDER, DEEPSEEK_MODEL)
        .await?;
    harness
        .run_turn_expecting_catalog(&thread, "deepseek turn again")
        .await?;
    let deepseek_tools = harness.deepseek.last_tools()?;

    assert!(
        tool_names(&deepseek_tools).contains(&"exec_command".to_string()),
        "switching back to {DEEPSEEK_MODEL} did not restore \
         {DEEPSEEK_PROVIDER}'s catalog: the shell tool is still missing. Got {:?}",
        tool_names(&deepseek_tools)
    );

    Ok(())
}

/// A switch that happens *after* the session already ran a tool must not carry
/// stale tool state: the destination turn's tool plan is rebuilt from the
/// destination provider's model metadata rather than reused from the earlier
/// DeepSeek step.
#[tokio::test]
async fn provider_switch_after_tool_use_rebuilds_tool_plan() -> Result<()> {
    let mut harness = Harness::start(
        vec![
            TurnScript::ToolCall("run a tool on deepseek"),
            TurnScript::Message("deepseek follow-up"),
        ],
        vec![TurnScript::Message("space bunny turn after tool use")],
    )
    .await?;

    let thread = harness
        .start_thread(DEEPSEEK_PROVIDER, DEEPSEEK_MODEL)
        .await?;
    harness.run_turn(&thread, "run a tool on deepseek").await?;
    let before_switch = harness.deepseek.last_tools()?;
    assert!(
        tool_names(&before_switch).contains(&"exec_command".to_string()),
        "the pre-switch turn should have advertised the shell tool"
    );

    harness
        .switch_provider(&thread, OPENROUTER_PROVIDER, SPACE_BUNNY_MODEL)
        .await?;
    harness
        .run_turn_expecting_catalog(&thread, "space bunny turn after tool use")
        .await?;
    let after_switch = harness.openrouter.last_tools()?;

    assert!(
        !tool_names(&after_switch).contains(&"exec_command".to_string()),
        "the turn after a provider switch reused the pre-switch tool plan; stale \
         tool state survived the switch. Got {:?}",
        tool_names(&after_switch)
    );

    Ok(())
}

/// The user-visible symptom: a real web search must work on both sides of a
/// provider switch, each provider serving its own search request.
///
/// This is the end-to-end shape of the bug. The standalone search tool is
/// contributed per turn and executes through the active provider's own search
/// client, so a session still resolving the outgoing provider's catalog fails
/// exactly here.
#[tokio::test]
async fn web_search_works_before_and_after_a_provider_switch() -> Result<()> {
    let mut harness = Harness::start_with_shell_on_both(
        vec![TurnScript::WebSearch("deepsearch")],
        vec![TurnScript::WebSearch("bunnysearch")],
    )
    .await?;

    let thread = harness
        .start_thread(DEEPSEEK_PROVIDER, DEEPSEEK_MODEL)
        .await?;

    harness
        .run_turn_watching_for_fallback_metadata(&thread, "search the web on deepseek")
        .await?;
    let before = harness.last_web_search()?;
    assert_eq!(
        before.query, "deepsearch",
        "web search must work before the switch, served by the pre-switch provider"
    );
    assert!(
        before.results > 0,
        "the pre-switch search must return results, not just run"
    );

    harness
        .switch_provider(&thread, OPENROUTER_PROVIDER, SPACE_BUNNY_MODEL)
        .await?;
    let after_warnings = harness
        .run_turn_watching_for_fallback_metadata(&thread, "search the web on space bunny")
        .await?;
    assert_eq!(
        after_warnings,
        Vec::<String>::new(),
        "the switched-to model must resolve from the destination catalog; falling \
         back to default metadata is what breaks web search after a provider switch"
    );
    let after = harness.last_web_search()?;
    assert_eq!(
        after.query, "bunnysearch",
        "web search must keep working after switching providers"
    );
    assert!(
        after.results > 0,
        "the post-switch search must return results, not just run"
    );

    Ok(())
}

/// What a single scripted `/responses` body should do.
enum TurnScript {
    Message(&'static str),
    /// Emit a namespaced `web.run` call, so the turn really exercises the
    /// standalone search tool before completing.
    WebSearch(&'static str),
    /// Emit a shell tool call, so the turn exercises tool execution.
    ToolCall(&'static str),
}

impl TurnScript {
    fn into_events(self, index: usize) -> Vec<Value> {
        let call_id = format!("call-{index}");
        let completed = responses::ev_completed(&format!("r{index}"));
        match self {
            Self::Message(text) => vec![
                responses::ev_assistant_message(&format!("m{index}"), text),
                completed,
            ],
            // A tool call makes the model run again once the tool result is in,
            // so the sequence needs a trailing body for that follow-up request.
            Self::ToolCall(_text) => vec![
                responses::ev_function_call(
                    &call_id,
                    "shell",
                    &json!({ "command": ["bash", "-lc", "echo deepseek"] }).to_string(),
                ),
                responses::ev_completed(&format!("r{index}-tool")),
            ],
            Self::WebSearch(query) => vec![
                responses::ev_function_call_with_namespace(
                    &call_id,
                    "web",
                    "run",
                    &json!({ "search_query": [{ "q": query }] }).to_string(),
                ),
                responses::ev_completed(&format!("r{index}-tool")),
            ],
        }
    }
}

/// One provider bound to one mock server: `/models` for the catalog, `/responses`
/// for turns, and `alpha/search` for the standalone web-search tool.
struct Provider {
    server: MockServer,
    responses: ResponseMock,
}

impl Provider {
    async fn start(
        model: &str,
        shell_type: ConfigShellToolType,
        turns: Vec<TurnScript>,
    ) -> Result<Self> {
        let server = MockServer::start().await;
        // Serve this provider's catalog for *every* `/models` request. The shared
        // `mount_models_once` helper is capped at one call, which would leave the
        // second fetch falling through to `start_mock_server`'s empty catalog and
        // silently reintroduce fallback model metadata.
        mount_models_catalog(
            &server,
            ModelsResponse {
                models: vec![model_info(model, shell_type)?],
            },
        )
        .await;
        mount_search_response(&server).await;

        let bodies = turns
            .into_iter()
            .enumerate()
            .map(|(index, turn)| responses::sse(turn.into_events(index)))
            .collect();
        let response_mock = responses::mount_sse_sequence(&server, bodies).await;
        Ok(Self {
            server,
            responses: response_mock,
        })
    }

    fn last_tools(&self) -> Result<Vec<Value>> {
        Ok(self.last_body()?["tools"]
            .as_array()
            .cloned()
            .unwrap_or_default())
    }

    fn last_model(&self) -> Result<String> {
        let body = self.last_body()?;
        Ok(body["model"].as_str().unwrap_or_default().to_string())
    }

    fn last_body(&self) -> Result<Value> {
        let request = self
            .responses
            .last_request()
            .context("expected a model request for the turn")?;
        Ok(request.body_json())
    }
}

struct Harness {
    app: TestAppServer,
    deepseek: Provider,
    openrouter: Provider,
    web_searches: Vec<CompletedWebSearch>,
    _sofia_home: TempDir,
}

impl Harness {
    /// Build a harness whose two providers serve exactly the scripted turns.
    /// `mount_sse_sequence` asserts an exact call count, so each test declares
    /// the requests it will actually issue.
    async fn start(
        deepseek_turns: Vec<TurnScript>,
        openrouter_turns: Vec<TurnScript>,
    ) -> Result<Self> {
        Self::start_with_catalogs(
            /*deepseek_shell_type*/ ConfigShellToolType::UnifiedExec,
            /*openrouter_shell_type*/ ConfigShellToolType::Disabled,
            deepseek_turns,
            openrouter_turns,
        )
        .await
    }

    /// Both models keep the shell tool, so the standalone search tool is
    /// advertised on either side of a provider switch.
    async fn start_with_shell_on_both(
        deepseek_turns: Vec<TurnScript>,
        openrouter_turns: Vec<TurnScript>,
    ) -> Result<Self> {
        Self::start_with_catalogs(
            ConfigShellToolType::UnifiedExec,
            ConfigShellToolType::UnifiedExec,
            deepseek_turns,
            openrouter_turns,
        )
        .await
    }

    async fn start_with_catalogs(
        deepseek_shell_type: ConfigShellToolType,
        openrouter_shell_type: ConfigShellToolType,
        deepseek_turns: Vec<TurnScript>,
        openrouter_turns: Vec<TurnScript>,
    ) -> Result<Self> {
        let deepseek = Provider::start(DEEPSEEK_MODEL, deepseek_shell_type, deepseek_turns).await?;
        let openrouter =
            Provider::start(SPACE_BUNNY_MODEL, openrouter_shell_type, openrouter_turns).await?;

        let sofia_home = TempDir::new()?;
        config(&deepseek.server.uri(), &openrouter.server.uri()).write(sofia_home.path())?;

        let app = TestAppServer::builder()
            .with_codex_home(sofia_home.path())
            // Keep the child hermetic: an ambient machine config registers real
            // providers (and a real OpenRouter key), whose failed catalog fetch
            // would mask what this suite is actually testing.
            .without_managed_config()
            .build_initialized_with_timeout(DEFAULT_TIMEOUT)
            .await?;

        Ok(Self {
            app,
            deepseek,
            openrouter,
            web_searches: Vec::new(),
            _sofia_home: sofia_home,
        })
    }

    async fn start_thread(&mut self, provider: &str, model: &str) -> Result<String> {
        let request = self
            .app
            .send_thread_start_request_with_auto_env(ThreadStartParams {
                model: Some(model.to_string()),
                model_provider: Some(provider.to_string()),
                ..Default::default()
            })
            .await?;
        let ThreadStartResponse { thread, .. } =
            timeout(DEFAULT_TIMEOUT, self.app.read_response(request)).await??;
        Ok(thread.id)
    }

    async fn switch_provider(
        &mut self,
        thread_id: &str,
        provider: &str,
        model: &str,
    ) -> Result<()> {
        let request = self
            .app
            .send_thread_settings_update_request(ThreadSettingsUpdateParams {
                thread_id: thread_id.to_string(),
                model: Some(model.to_string()),
                model_provider: Some(provider.to_string()),
                ..Default::default()
            })
            .await?;
        let _: ThreadSettingsUpdateResponse =
            timeout(DEFAULT_TIMEOUT, self.app.read_response(request)).await??;
        // The response only acknowledges that the update was queued, so wait for
        // it to land; otherwise the next turn can still observe the old catalog.
        let _: ThreadSettingsUpdatedNotification = timeout(
            DEFAULT_TIMEOUT,
            self.app.read_notification("thread/settings/updated"),
        )
        .await??;
        Ok(())
    }

    async fn run_turn(&mut self, thread_id: &str, text: &str) -> Result<()> {
        self.start_turn(thread_id, text).await?;
        timeout(
            DEFAULT_TIMEOUT,
            self.app
                .read_stream_until_notification_message("turn/completed"),
        )
        .await??;
        Ok(())
    }

    /// Run one turn and return any "fallback model metadata" warnings it produced.
    ///
    /// A provider switch that leaves the previous catalog installed makes the
    /// destination model unresolvable, which is the original symptom: the turn
    /// runs on fallback metadata instead of the destination catalog.
    async fn run_turn_watching_for_fallback_metadata(
        &mut self,
        thread_id: &str,
        text: &str,
    ) -> Result<Vec<String>> {
        self.start_turn(thread_id, text).await?;
        let mut warnings = Vec::new();
        loop {
            let notification = timeout(
                DEFAULT_TIMEOUT,
                self.app
                    .read_stream_until_matching_notification("turn stream", |notification| {
                        matches!(
                            notification.method.as_str(),
                            "turn/completed" | "warning" | "item/completed"
                        )
                    }),
            )
            .await??;
            let Some(params) = notification.params else {
                continue;
            };
            if notification.method == "turn/completed" {
                return Ok(warnings);
            }
            if notification.method == "item/completed"
                && let Some(item) = params.get("item")
                && item.get("type").and_then(Value::as_str) == Some("webSearch")
            {
                self.note_web_search(CompletedWebSearch {
                    query: item
                        .get("query")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    results: item
                        .get("results")
                        .and_then(Value::as_array)
                        .map_or(0, Vec::len),
                });
                continue;
            }
            if let Some(message) = params.get("message").and_then(Value::as_str)
                && message.contains("fallback metadata")
            {
                warnings.push(message.to_string());
            }
        }
    }

    /// Record the most recent completed web-search item, for later assertion.
    fn note_web_search(&mut self, item: CompletedWebSearch) {
        self.web_searches.push(item);
    }

    /// Run one turn that must resolve from the active provider's catalog.
    ///
    /// Fallback metadata makes a switched turn look superficially fine — the
    /// defaults happen to include the shell tool — so asserting on the
    /// fallback warning is what distinguishes a correct switch from a stale
    /// catalog in both directions.
    async fn run_turn_expecting_catalog(&mut self, thread_id: &str, text: &str) -> Result<()> {
        let warnings = self
            .run_turn_watching_for_fallback_metadata(thread_id, text)
            .await?;
        assert_eq!(
            warnings,
            Vec::<String>::new(),
            "the turn resolved fallback metadata instead of the active provider's \
             catalog; the tool plan may be stale"
        );
        Ok(())
    }

    fn last_web_search(&self) -> Result<CompletedWebSearch> {
        self.web_searches
            .last()
            .cloned()
            .context("expected a completed webSearch item")
    }

    async fn start_turn(&mut self, thread_id: &str, text: &str) -> Result<()> {
        let request = self
            .app
            .send_turn_start_request(TurnStartParams {
                thread_id: thread_id.to_string(),
                client_user_message_id: None,
                input: vec![V2UserInput::Text {
                    text: text.to_string(),
                    text_elements: Vec::new(),
                }],
                ..Default::default()
            })
            .await?;
        let _: TurnStartResponse =
            timeout(DEFAULT_TIMEOUT, self.app.read_response(request)).await??;
        Ok(())
    }
}

/// What a completed web-search item proves: the tool ran on the active provider
/// and came back with results.
#[derive(Clone)]
struct CompletedWebSearch {
    query: String,
    results: usize,
}

/// Advertised tool names, in order. Used for readable parity assertions.
fn tool_names(tools: &[Value]) -> Vec<String> {
    tools
        .iter()
        .filter_map(|tool| {
            tool.get("name")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| tool.get("type").and_then(Value::as_str).map(str::to_string))
        })
        .collect()
}

fn config(deepseek_uri: &str, openrouter_uri: &str) -> MockResponsesConfig {
    MockResponsesConfig::new(deepseek_uri)
        .with_model(DEEPSEEK_MODEL)
        .with_model_provider(DEEPSEEK_PROVIDER)
        // The goal judge issues its own `/responses` call after each turn; leaving
        // it on consumes the mocked sequence and makes tool-plan comparisons empty.
        .with_root_config("goal_judge_enabled = false\n")
        .with_provider_config("supports_websockets = false")
        .with_provider_config("supports_standalone_web_search = true")
        .with_provider_config(COMMAND_AUTH)
        .with_extra_config(&format!(
            r#"
[model_providers.{OPENROUTER_PROVIDER}]
name = "OpenRouter"
base_url = "{openrouter_uri}/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
supports_websockets = false
supports_standalone_web_search = true
{COMMAND_AUTH}
"#
        ))
        .enable_feature(Feature::StandaloneWebSearch)
}

fn model_info(slug: &str, shell_type: ConfigShellToolType) -> Result<ModelInfo> {
    let mut model = bundled_models_response()?
        .models
        .into_iter()
        .find(|model| model.slug == "gpt-5.4")
        .context("bundled catalog should include gpt-5.4")?;
    model.slug = slug.to_string();
    model.shell_type = shell_type;
    Ok(model)
}

/// Mount a provider's `/models` catalog for every request, not just the first.
async fn mount_models_catalog(server: &MockServer, catalog: ModelsResponse) {
    Mock::given(method("GET"))
        .and(path_regex(".*/models$"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_json(catalog),
        )
        .mount(server)
        .await;
}

async fn mount_search_response(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path_regex(".*/alpha/search$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "encrypted_output": "ciphertext",
            "output": "Search result",
            "results": [{
                "type": "text_result",
                "ref_id": "turn0search0",
                "url": "https://example.com/search-result",
                "title": "Search Result",
                "snippet": "A result snippet",
            }],
        })))
        .mount(server)
        .await;
}
