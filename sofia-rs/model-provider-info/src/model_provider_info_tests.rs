use super::*;
use pretty_assertions::assert_eq;
use sofia_utils_absolute_path::AbsolutePathBuf;
use sofia_utils_absolute_path::AbsolutePathBufGuard;
use std::num::NonZeroU64;
use tempfile::tempdir;

#[test]
fn test_deserialize_ollama_model_provider_toml() {
    let azure_provider_toml = r#"
name = "Ollama"
base_url = "http://localhost:11434/v1"
        "#;
    let expected_provider = ModelProviderInfo {
        name: "Ollama".into(),
        base_url: Some("http://localhost:11434/v1".into()),
        env_key: None,
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        aws: None,
        wire_api: WireApi::Responses,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: None,
        stream_max_retries: None,
        stream_idle_timeout_ms: None,
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
        supports_standalone_web_search: false,
    };

    let provider: ModelProviderInfo = toml::from_str(azure_provider_toml).unwrap();
    assert_eq!(expected_provider, provider);
}

#[test]
fn test_deserialize_azure_model_provider_toml() {
    let azure_provider_toml = r#"
name = "Azure"
base_url = "https://xxxxx.openai.azure.com/openai"
env_key = "AZURE_OPENAI_API_KEY"
query_params = { api-version = "2025-04-01-preview" }
        "#;
    let expected_provider = ModelProviderInfo {
        name: "Azure".into(),
        base_url: Some("https://xxxxx.openai.azure.com/openai".into()),
        env_key: Some("AZURE_OPENAI_API_KEY".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        aws: None,
        wire_api: WireApi::Responses,
        query_params: Some(maplit::hashmap! {
            "api-version".to_string() => "2025-04-01-preview".into(),
        }),
        http_headers: None,
        env_http_headers: None,
        request_max_retries: None,
        stream_max_retries: None,
        stream_idle_timeout_ms: None,
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
        supports_standalone_web_search: false,
    };

    let provider: ModelProviderInfo = toml::from_str(azure_provider_toml).unwrap();
    assert_eq!(expected_provider, provider);
}

#[test]
fn test_deserialize_example_model_provider_toml() {
    let azure_provider_toml = r#"
name = "Example"
base_url = "https://example.com"
env_key = "API_KEY"
http_headers = { "X-Example-Header" = "example-value" }
env_http_headers = { "X-Example-Env-Header" = "EXAMPLE_ENV_VAR" }
supports_standalone_web_search = true
        "#;
    let expected_provider = ModelProviderInfo {
        name: "Example".into(),
        base_url: Some("https://example.com".into()),
        env_key: Some("API_KEY".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        aws: None,
        wire_api: WireApi::Responses,
        query_params: None,
        http_headers: Some(maplit::hashmap! {
            "X-Example-Header".to_string() => "example-value".into(),
        }),
        env_http_headers: Some(maplit::hashmap! {
            "X-Example-Env-Header".to_string() => "EXAMPLE_ENV_VAR".to_string(),
        }),
        request_max_retries: None,
        stream_max_retries: None,
        stream_idle_timeout_ms: None,
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
        supports_standalone_web_search: true,
    };

    let provider: ModelProviderInfo = toml::from_str(azure_provider_toml).unwrap();
    assert_eq!(expected_provider, provider);
}

#[test]
fn test_deserialize_chat_wire_api_shows_helpful_error() {
    let provider_toml = r#"
name = "OpenAI using Chat Completions"
base_url = "https://api.openai.com/v1"
env_key = "OPENAI_API_KEY"
wire_api = "chat"
        "#;

    // "chat" is now accepted as ChatCompletions (backward compat alias).
    let provider = toml::from_str::<ModelProviderInfo>(provider_toml).unwrap();
    assert_eq!(provider.wire_api, WireApi::ChatCompletions);
}

#[test]
fn test_deserialize_websocket_connect_timeout() {
    let provider_toml = r#"
name = "OpenAI"
base_url = "https://api.openai.com/v1"
websocket_connect_timeout_ms = 15000
supports_websockets = true
        "#;

    let provider: ModelProviderInfo = toml::from_str(provider_toml).unwrap();
    assert_eq!(provider.websocket_connect_timeout_ms, Some(15_000));
}

#[test]
fn test_personal_access_token_uses_chatgpt_codex_base_url() {
    let api_provider = ModelProviderInfo::create_openai_provider(/*base_url*/ None)
        .to_api_provider(Some(AuthMode::PersonalAccessToken))
        .expect("OpenAI provider should build API provider");

    assert_eq!(api_provider.base_url, CHATGPT_CODEX_BASE_URL);
}

#[test]
fn test_header_auth_uses_chatgpt_codex_base_url() {
    let api_provider = ModelProviderInfo::create_openai_provider(/*base_url*/ None)
        .to_api_provider(Some(AuthMode::Headers))
        .expect("OpenAI provider should build API provider");

    assert_eq!(api_provider.base_url, CHATGPT_CODEX_BASE_URL);
}

#[test]
fn sofia_backend_routes_require_codex_base_url() {
    for (base_url, expected) in [
        (None, true),
        (Some(CHATGPT_CODEX_BASE_URL), true),
        (Some("https://chatgpt-staging.com/backend-api/sofia/"), true),
        (Some("https://proxy.example.com/v1"), false),
    ] {
        let provider = ModelProviderInfo::create_openai_provider(base_url.map(str::to_owned));
        assert_eq!(provider.supports_codex_backend_routes(), expected);
    }
}

#[test]
fn test_uses_openai_actor_authorization() {
    let mut provider = ModelProviderInfo {
        http_headers: Some(maplit::hashmap! {
            "X-OpenAI-Actor-Authorization".to_string() => "actor-token".into(),
        }),
        ..ModelProviderInfo::default()
    };
    assert!(provider.uses_openai_actor_authorization());

    provider.http_headers = None;
    assert!(!provider.uses_openai_actor_authorization());

    provider.http_headers = Some(maplit::hashmap! {
        OPENAI_ACTOR_AUTHORIZATION_HEADER.to_string() => "  ".into(),
    });
    assert!(!provider.uses_openai_actor_authorization());

    provider.http_headers = Some(maplit::hashmap! {
        OPENAI_ACTOR_AUTHORIZATION_HEADER.to_string() => "actor-token".into(),
    });
    provider.requires_openai_auth = true;
    assert!(!provider.uses_openai_actor_authorization());
}

#[test]
fn test_deserialize_provider_auth_config_defaults() {
    let base_dir = tempdir().unwrap();
    let provider_toml = r#"
name = "Corp"

[auth]
command = "./scripts/print-token"
args = ["--format=text"]
        "#;

    let provider: ModelProviderInfo = {
        let _guard = AbsolutePathBufGuard::new(base_dir.path());
        toml::from_str(provider_toml).unwrap()
    };

    assert_eq!(
        provider.auth,
        Some(ModelProviderAuthInfo {
            command: "./scripts/print-token".to_string(),
            args: vec!["--format=text".into()],
            timeout_ms: NonZeroU64::new(5_000).unwrap(),
            refresh_interval_ms: 300_000,
            cwd: AbsolutePathBuf::resolve_path_against_base(".", base_dir.path()),
        })
    );
}

#[test]
fn test_deserialize_provider_aws_config() {
    let provider_toml = r#"
name = "Amazon Bedrock"
base_url = "https://bedrock.example.com/v1"

[aws]
profile = "sofia-bedrock"
region = "us-west-2"

[aws.auth_refresh]
command = "aws"
args = ["login", "--profile", "sofia-bedrock"]
        "#;

    let provider: ModelProviderInfo = toml::from_str(provider_toml).unwrap();

    assert_eq!(
        provider.aws,
        Some(ModelProviderAwsAuthInfo {
            profile: Some("sofia-bedrock".to_string()),
            region: Some("us-west-2".to_string()),
            auth_refresh: Some(AwsAuthRefreshConfig {
                command: "aws".to_string(),
                args: vec!["login".into(), "--profile".into(), "sofia-bedrock".into()],
                timeout_ms: NonZeroU64::new(300_000).expect("timeout should be non-zero"),
            }),
        })
    );
}

#[test]
fn test_create_amazon_bedrock_provider() {
    assert_eq!(
        ModelProviderInfo::create_amazon_bedrock_provider(/*aws*/ None),
        ModelProviderInfo {
            name: "Amazon Bedrock".to_string(),
            base_url: None,
            env_key: None,
            env_key_instructions: None,
            experimental_bearer_token: None,
            auth: None,
            aws: Some(ModelProviderAwsAuthInfo {
                profile: None,
                region: None,
                auth_refresh: None,
            }),
            wire_api: WireApi::Responses,
            query_params: None,
            http_headers: Some(maplit::hashmap! {
                AMAZON_BEDROCK_MANTLE_CLIENT_AGENT_HEADER.to_string() =>
                    AMAZON_BEDROCK_MANTLE_CLIENT_AGENT_VALUE.into(),
            }),
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_openai_auth: false,
            supports_websockets: false,
            supports_standalone_web_search: false,
        }
    );
}

#[test]
fn test_create_amazon_bedrock_runtime_provider() {
    let mut expected = ModelProviderInfo::create_amazon_bedrock_provider(/*aws*/ None);
    expected.name = "Amazon Bedrock Runtime".to_string();
    expected.http_headers = None;

    assert_eq!(
        ModelProviderInfo::create_amazon_bedrock_runtime_provider(/*aws*/ None),
        expected
    );
}

#[test]
fn test_create_amazon_bedrock_runtime_provider_with_aws_configuration() {
    let provider =
        ModelProviderInfo::create_amazon_bedrock_runtime_provider(Some(ModelProviderAwsAuthInfo {
            profile: Some("runtime-profile".to_string()),
            region: Some("us-west-2".to_string()),
            auth_refresh: None,
        }));

    assert_eq!(
        (
            provider.name.as_str(),
            provider.aws,
            provider.http_headers,
            provider.supports_standalone_web_search,
        ),
        (
            "Amazon Bedrock Runtime",
            Some(ModelProviderAwsAuthInfo {
                profile: Some("runtime-profile".to_string()),
                region: Some("us-west-2".to_string()),
                auth_refresh: None,
            }),
            None,
            false,
        )
    );
}

fn provider_auth_for_test() -> ModelProviderAuthInfo {
    ModelProviderAuthInfo {
        command: "token-fetcher".to_string(),
        args: vec!["fetch".into()],
        timeout_ms: NonZeroU64::new(5_000).expect("timeout should be non-zero"),
        refresh_interval_ms: 300_000,
        cwd: std::env::current_dir()
            .expect("current directory should be available")
            .try_into()
            .expect("current directory should be absolute"),
    }
}

#[test]
fn test_amazon_bedrock_provider_adds_mantle_client_agent_header() {
    let api_provider = ModelProviderInfo::create_amazon_bedrock_provider(/*aws*/ None)
        .to_api_provider(/*auth_mode*/ None)
        .expect("Amazon Bedrock provider should build API provider");

    assert_eq!(
        api_provider
            .headers
            .get(AMAZON_BEDROCK_MANTLE_CLIENT_AGENT_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(AMAZON_BEDROCK_MANTLE_CLIENT_AGENT_VALUE)
    );
}

#[test]
fn test_built_in_model_providers_include_amazon_bedrock_endpoints() {
    let providers = built_in_model_providers(/*openai_base_url*/ None);

    assert_eq!(
        [
            AMAZON_BEDROCK_PROVIDER_ID,
            AMAZON_BEDROCK_RUNTIME_PROVIDER_ID
        ]
        .into_iter()
        .map(|provider_id| {
            providers
                .get(provider_id)
                .map(ModelProviderInfo::is_amazon_bedrock)
        })
        .collect::<Vec<_>>(),
        vec![Some(true), Some(true)]
    );
}

#[test]
fn test_built_in_model_providers_include_amazon_bedrock_runtime() {
    let providers = built_in_model_providers(/*openai_base_url*/ None);
    let runtime = providers
        .get(AMAZON_BEDROCK_RUNTIME_PROVIDER_ID)
        .expect("Amazon Bedrock Runtime provider should be built in");

    assert!(runtime.is_amazon_bedrock());
    assert!(runtime.is_amazon_bedrock_runtime());
    assert!(
        !providers
            .get(AMAZON_BEDROCK_PROVIDER_ID)
            .expect("Amazon Bedrock provider should be built in")
            .is_amazon_bedrock_runtime()
    );
}

#[test]
fn test_merge_configured_model_providers_adds_custom_provider() {
    let custom_provider = ModelProviderInfo {
        name: "Custom".to_string(),
        base_url: Some("https://example.com/v1".to_string()),
        ..ModelProviderInfo::default()
    };
    let configured_model_providers =
        std::collections::HashMap::from([("custom".to_string(), custom_provider.clone())]);

    let mut expected = built_in_model_providers(/*openai_base_url*/ None);
    expected.insert("custom".to_string(), custom_provider);

    assert_eq!(
        merge_configured_model_providers(
            built_in_model_providers(/*openai_base_url*/ None),
            configured_model_providers,
        ),
        Ok(expected)
    );
}

#[test]
fn test_merge_configured_model_providers_applies_amazon_bedrock_aws_override() {
    let auth_refresh = AwsAuthRefreshConfig {
        command: "aws".to_string(),
        args: vec!["login".into(), "--profile".into(), "sofia-bedrock".into()],
        timeout_ms: NonZeroU64::new(10_000).expect("timeout should be non-zero"),
    };
    let configured_model_providers = std::collections::HashMap::from([(
        AMAZON_BEDROCK_PROVIDER_ID.to_string(),
        ModelProviderInfo {
            aws: Some(ModelProviderAwsAuthInfo {
                profile: Some("sofia-bedrock".to_string()),
                region: Some("us-west-2".to_string()),
                auth_refresh: Some(auth_refresh.clone()),
            }),
            ..ModelProviderInfo::default()
        },
    )]);

    let mut expected = built_in_model_providers(/*openai_base_url*/ None);
    expected
        .get_mut(AMAZON_BEDROCK_PROVIDER_ID)
        .expect("Amazon Bedrock provider should be built in")
        .aws = Some(ModelProviderAwsAuthInfo {
        profile: Some("sofia-bedrock".to_string()),
        region: Some("us-west-2".to_string()),
        auth_refresh: Some(auth_refresh),
    });

    assert_eq!(
        merge_configured_model_providers(
            built_in_model_providers(/*openai_base_url*/ None),
            configured_model_providers,
        ),
        Ok(expected)
    );
}

#[test]
fn test_merge_configured_model_providers_applies_runtime_overrides_independently() {
    let runtime_aws = ModelProviderAwsAuthInfo {
        profile: Some("runtime-profile".to_string()),
        region: Some("eu-west-1".to_string()),
        auth_refresh: None,
    };
    let configured_model_providers = std::collections::HashMap::from([(
        AMAZON_BEDROCK_RUNTIME_PROVIDER_ID.to_string(),
        ModelProviderInfo {
            base_url: Some("https://runtime.example.com/openai/v1".to_string()),
            aws: Some(runtime_aws.clone()),
            ..ModelProviderInfo::default()
        },
    )]);
    let mut expected = built_in_model_providers(/*openai_base_url*/ None);
    let expected_runtime = expected
        .get_mut(AMAZON_BEDROCK_RUNTIME_PROVIDER_ID)
        .expect("Amazon Bedrock Runtime provider should be built in");
    expected_runtime.base_url = Some("https://runtime.example.com/openai/v1".to_string());
    expected_runtime.aws = Some(runtime_aws);

    assert_eq!(
        merge_configured_model_providers(
            built_in_model_providers(/*openai_base_url*/ None),
            configured_model_providers,
        ),
        Ok(expected)
    );
}

#[test]
fn test_merge_configured_model_providers_applies_amazon_bedrock_transport_overrides() {
    let auth = provider_auth_for_test();
    let configured_model_providers = std::collections::HashMap::from([(
        AMAZON_BEDROCK_PROVIDER_ID.to_string(),
        ModelProviderInfo {
            base_url: Some("https://proxy.example.com/v1".to_string()),
            auth: Some(auth.clone()),
            aws: Some(ModelProviderAwsAuthInfo {
                profile: Some("sofia-bedrock".to_string()),
                region: Some("us-west-2".to_string()),
                auth_refresh: None,
            }),
            http_headers: Some(maplit::hashmap! {
                "x-example-header".to_string() => "value".into(),
            }),
            ..ModelProviderInfo::default()
        },
    )]);

    let mut expected = built_in_model_providers(/*openai_base_url*/ None);
    let expected_provider = expected
        .get_mut(AMAZON_BEDROCK_PROVIDER_ID)
        .expect("Amazon Bedrock provider should be built in");
    expected_provider.base_url = Some("https://proxy.example.com/v1".to_string());
    expected_provider.auth = Some(auth);
    expected_provider.aws = Some(ModelProviderAwsAuthInfo {
        profile: Some("sofia-bedrock".to_string()),
        region: Some("us-west-2".to_string()),
        auth_refresh: None,
    });
    expected_provider
        .http_headers
        .get_or_insert_default()
        .insert("x-example-header".to_string(), "value".into());

    assert_eq!(
        merge_configured_model_providers(
            built_in_model_providers(/*openai_base_url*/ None),
            configured_model_providers,
        ),
        Ok(expected)
    );
}

#[test]
fn test_merge_configured_model_providers_rejects_amazon_bedrock_non_default_fields() {
    let configured_model_providers = std::collections::HashMap::from([(
        AMAZON_BEDROCK_PROVIDER_ID.to_string(),
        ModelProviderInfo {
            name: "Custom Bedrock".to_string(),
            aws: Some(ModelProviderAwsAuthInfo {
                profile: Some("sofia-bedrock".to_string()),
                region: None,
                auth_refresh: None,
            }),
            ..ModelProviderInfo::default()
        },
    )]);

    assert_eq!(
        merge_configured_model_providers(
            built_in_model_providers(/*openai_base_url*/ None),
            configured_model_providers,
        ),
        Err(
            "model_providers.amazon-bedrock only supports changing `base_url`, `auth`, `http_headers`, `aws.profile`, `aws.region`, and `aws.auth_refresh`; other non-default provider fields are not supported"
                .to_string()
        )
    );
}

#[test]
fn test_merge_configured_model_providers_allows_amazon_bedrock_default_fields() {
    let configured_model_providers = std::collections::HashMap::from([(
        AMAZON_BEDROCK_PROVIDER_ID.to_string(),
        ModelProviderInfo {
            aws: Some(ModelProviderAwsAuthInfo {
                profile: None,
                region: None,
                auth_refresh: None,
            }),
            wire_api: WireApi::Responses,
            ..ModelProviderInfo::default()
        },
    )]);

    assert_eq!(
        merge_configured_model_providers(
            built_in_model_providers(/*openai_base_url*/ None),
            configured_model_providers,
        ),
        Ok(built_in_model_providers(/*openai_base_url*/ None))
    );
}

#[test]
fn test_validate_provider_aws_rejects_conflicting_auth() {
    let provider = ModelProviderInfo {
        aws: Some(ModelProviderAwsAuthInfo {
            profile: None,
            region: None,
            auth_refresh: None,
        }),
        env_key: Some("AWS_BEARER_TOKEN_BEDROCK".to_string()),
        supports_websockets: false,
        ..ModelProviderInfo::create_openai_provider(/*base_url*/ None)
    };

    assert_eq!(
        provider.validate(),
        Err("provider aws cannot be combined with env_key, requires_openai_auth".to_string())
    );
}

#[test]
fn test_validate_provider_aws_rejects_websockets() {
    let provider = ModelProviderInfo {
        aws: Some(ModelProviderAwsAuthInfo {
            profile: None,
            region: None,
            auth_refresh: None,
        }),
        requires_openai_auth: false,
        supports_websockets: true,
        ..ModelProviderInfo::create_openai_provider(/*base_url*/ None)
    };

    assert_eq!(
        provider.validate(),
        Err("provider aws cannot be combined with supports_websockets".to_string())
    );
}

#[test]
fn test_validate_provider_aws_auth_refresh_command() {
    for (command, expected) in [
        (
            "  ",
            Err("provider aws.auth_refresh.command must not be empty".to_string()),
        ),
        (
            "other-command",
            Err("provider aws.auth_refresh.command must be `aws`".to_string()),
        ),
        ("aws", Ok(())),
    ] {
        let provider =
            ModelProviderInfo::create_amazon_bedrock_provider(Some(ModelProviderAwsAuthInfo {
                profile: None,
                region: None,
                auth_refresh: Some(AwsAuthRefreshConfig {
                    command: command.to_string(),
                    args: Vec::new(),
                    timeout_ms: NonZeroU64::new(300_000).expect("timeout should be non-zero"),
                }),
            }));

        assert_eq!(provider.validate(), expected);
    }
}

#[test]
fn test_deserialize_provider_auth_config_allows_zero_refresh_interval() {
    let base_dir = tempdir().unwrap();
    let provider_toml = r#"
name = "Corp"

[auth]
command = "./scripts/print-token"
refresh_interval_ms = 0
        "#;

    let provider: ModelProviderInfo = {
        let _guard = AbsolutePathBufGuard::new(base_dir.path());
        toml::from_str(provider_toml).unwrap()
    };

    let auth = provider.auth.expect("auth config should deserialize");
    assert_eq!(auth.refresh_interval_ms, 0);
    assert_eq!(auth.refresh_interval(), None);
}

#[test]
fn test_api_key_falls_back_to_sofia_auth_file() {
    use std::io::Write;

    let base_dir = tempdir().unwrap();
    let auth_file = base_dir.path().join("sofia-auth.json");
    let mut file = std::fs::File::create(&auth_file).unwrap();
    write!(file, r#"{{ "DEEPSEEK_API_KEY": "sk-from-file" }}"#).unwrap();

    let provider = ModelProviderInfo {
        name: "DeepSeek".into(),
        base_url: Some("https://api.deepseek.com".into()),
        env_key: Some("DEEPSEEK_API_KEY".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        aws: None,
        wire_api: WireApi::Responses,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: None,
        stream_max_retries: None,
        stream_idle_timeout_ms: None,
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
        supports_standalone_web_search: false,
    };

    // Ensure the env var is not set so the file fallback is what resolves.
    let previous = std::env::var_os("DEEPSEEK_API_KEY");
    let previous_home = std::env::var_os("SOFIA_HOME");
    // Edition 2024 marks these env mutators unsafe; the test runs single-threaded
    // and restores the previous values, so the block is sound.
    unsafe {
        std::env::remove_var("DEEPSEEK_API_KEY");
        std::env::set_var("SOFIA_HOME", base_dir.path());
    }

    let result = provider.api_key();

    // Restore env.
    unsafe {
        if let Some(value) = previous {
            std::env::set_var("DEEPSEEK_API_KEY", value);
        } else {
            std::env::remove_var("DEEPSEEK_API_KEY");
        }
        if let Some(value) = previous_home {
            std::env::set_var("SOFIA_HOME", value);
        } else {
            std::env::remove_var("SOFIA_HOME");
        }
    }

    let key = result.expect("api_key should resolve from sofia-auth.json");
    assert_eq!(key.as_deref(), Some("sk-from-file"));
}

#[test]
fn test_api_key_prefers_credential_store_over_env() {
    use std::io::Write;

    let base_dir = tempdir().unwrap();
    // Write the credential store under `$SOFIA_HOME`, and also set the env var
    // to a *different* value. The credential store must win.
    let auth_file = base_dir.path().join("sofia-auth.json");
    let mut file = std::fs::File::create(&auth_file).unwrap();
    write!(file, r#"{{ "DEEPSEEK_API_KEY": "sk-from-store" }}"#).unwrap();

    let provider = ModelProviderInfo {
        name: "DeepSeek".into(),
        base_url: Some("https://api.deepseek.com".into()),
        env_key: Some("DEEPSEEK_API_KEY".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        aws: None,
        wire_api: WireApi::Responses,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: None,
        stream_max_retries: None,
        stream_idle_timeout_ms: None,
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
        supports_standalone_web_search: false,
    };

    let previous_home = std::env::var_os("SOFIA_HOME");
    unsafe {
        std::env::set_var("SOFIA_HOME", base_dir.path());
        std::env::set_var("DEEPSEEK_API_KEY", "sk-from-env");
    }

    let result = provider.api_key();

    unsafe {
        if let Some(value) = previous_home {
            std::env::set_var("SOFIA_HOME", value);
        } else {
            std::env::remove_var("SOFIA_HOME");
        }
        std::env::remove_var("DEEPSEEK_API_KEY");
    }

    // The credential store should take precedence over the env var.
    let key = result.expect("api_key should resolve");
    assert_eq!(key.as_deref(), Some("sk-from-store"));
}

#[test]
fn test_api_key_continues_past_missing_candidate_homes() {
    use std::io::Write;

    // Point `$SOFIA_HOME` at a directory that does *not* contain sofia-auth.json,
    // but place the file under `~/.sofia`. The lookup must continue past the
    // missing `$SOFIA_HOME` candidate and find it under `~/.sofia`.
    let sofia_home = tempdir().unwrap();
    let real_home = tempdir().unwrap();
    let sofia_home = real_home.path().join(".sofia");
    std::fs::create_dir_all(&sofia_home).unwrap();
    let auth_file = sofia_home.join("sofia-auth.json");
    let mut file = std::fs::File::create(&auth_file).unwrap();
    write!(file, r#"{{ "DEEPSEEK_API_KEY": "sk-from-sofia-home" }}"#).unwrap();

    let provider = ModelProviderInfo {
        name: "DeepSeek".into(),
        base_url: Some("https://api.deepseek.com".into()),
        env_key: Some("DEEPSEEK_API_KEY".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        aws: None,
        wire_api: WireApi::Responses,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: None,
        stream_max_retries: None,
        stream_idle_timeout_ms: None,
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
        supports_standalone_web_search: false,
    };

    let previous_home = std::env::var_os("SOFIA_HOME");
    let previous_user_home = std::env::var_os("HOME");
    unsafe {
        std::env::set_var("SOFIA_HOME", sofia_home.path());
        std::env::set_var("HOME", real_home.path());
    }

    let result = provider.api_key();

    unsafe {
        if let Some(value) = previous_home {
            std::env::set_var("SOFIA_HOME", value);
        } else {
            std::env::remove_var("SOFIA_HOME");
        }
        if let Some(value) = previous_user_home {
            std::env::set_var("HOME", value);
        } else {
            std::env::remove_var("HOME");
        }
    }

    let key = result.expect("api_key should resolve from ~/.sofia/sofia-auth.json");
    assert_eq!(key.as_deref(), Some("sk-from-sofia-home"));
}

// ---------------------------------------------------------------------------
// WireApi ChatCompletions tests
// ---------------------------------------------------------------------------

#[test]
fn test_wire_api_deserialize_chat_completions() {
    let toml_str = r#"
name = "Test"
wire_api = "chat_completions"
"#;
    let provider: ModelProviderInfo = toml::from_str(toml_str).unwrap();
    assert_eq!(provider.wire_api, WireApi::ChatCompletions);
}

#[test]
fn test_wire_api_deserialize_chat_alias() {
    let toml_str = r#"
name = "Test"
wire_api = "chat"
"#;
    let provider: ModelProviderInfo = toml::from_str(toml_str).unwrap();
    assert_eq!(provider.wire_api, WireApi::ChatCompletions);
}

#[test]
fn test_wire_api_deserialize_responses() {
    let toml_str = r#"
name = "Test"
wire_api = "responses"
"#;
    let provider: ModelProviderInfo = toml::from_str(toml_str).unwrap();
    assert_eq!(provider.wire_api, WireApi::Responses);
}

#[test]
fn test_wire_api_default_is_responses() {
    let toml_str = r#"
name = "Test"
"#;
    let provider: ModelProviderInfo = toml::from_str(toml_str).unwrap();
    assert_eq!(provider.wire_api, WireApi::Responses);
}

#[test]
fn test_wire_api_display() {
    assert_eq!(WireApi::Responses.to_string(), "responses");
    assert_eq!(WireApi::ChatCompletions.to_string(), "chat_completions");
}

#[test]
fn test_wire_api_unknown_variant() {
    let toml_str = r#"
name = "Test"
wire_api = "grpc"
"#;
    let err = toml::from_str::<ModelProviderInfo>(toml_str).unwrap_err();
    assert!(err.to_string().contains("unknown variant"));
}

// ---------------------------------------------------------------------------
// WellKnownProvider tests
// ---------------------------------------------------------------------------

#[test]
fn test_well_known_providers_count() {
    let providers = well_known_providers();
    assert!(
        providers.len() >= 10,
        "expected at least 10 well-known providers"
    );
}

#[test]
fn test_well_known_providers_have_unique_ids() {
    let providers = well_known_providers();
    let mut ids: Vec<&str> = providers.iter().map(|p| p.provider_id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), providers.len(), "provider IDs must be unique");
}

#[test]
fn test_well_known_providers_responses_api_flag() {
    let providers = well_known_providers();
    // Anthropic and OpenRouter should support Responses API
    let anthropic = providers
        .iter()
        .find(|p| p.provider_id == "anthropic")
        .unwrap();
    assert!(anthropic.supports_responses_api);
    let openrouter = providers
        .iter()
        .find(|p| p.provider_id == "openrouter")
        .unwrap();
    assert!(openrouter.supports_responses_api);
    // MiMo should not (Chat Completions only)
    let xiaomi = providers
        .iter()
        .find(|p| p.provider_id == "xiaomi")
        .unwrap();
    assert!(!xiaomi.supports_responses_api);
}

// ---------------------------------------------------------------------------
// discover_providers_from_env tests
// ---------------------------------------------------------------------------

#[test]
fn test_discover_providers_from_env_with_key() {
    unsafe { std::env::set_var("XIAOMI_API_KEY", "test-key-123") };
    let providers = discover_providers_from_env();
    let xiaomi = providers
        .get("xiaomi")
        .expect("xiaomi should be discovered");
    assert_eq!(xiaomi.name, "Xiaomi (MiMo)");
    assert_eq!(
        xiaomi.base_url.as_deref(),
        Some("https://api.xiaomimimo.com/v1")
    );
    assert_eq!(xiaomi.wire_api, WireApi::ChatCompletions);
    assert_eq!(xiaomi.env_key.as_deref(), Some("XIAOMI_API_KEY"));
    unsafe { std::env::remove_var("XIAOMI_API_KEY") };
}

#[test]
fn test_discover_providers_prefers_responses_api() {
    // Anthropic supports Responses API — should be preferred
    unsafe { std::env::set_var("ANTHROPIC_API_KEY", "test-key") };
    let providers = discover_providers_from_env();
    let anthropic = providers
        .get("anthropic")
        .expect("anthropic should be discovered");
    assert_eq!(anthropic.wire_api, WireApi::Responses);
    unsafe { std::env::remove_var("ANTHROPIC_API_KEY") };
}

#[test]
fn test_discover_providers_no_empty_keys() {
    unsafe { std::env::set_var("GROQ_API_KEY", "   ") };
    let providers = discover_providers_from_env();
    assert!(
        !providers.contains_key("groq"),
        "empty/whitespace key should not register provider"
    );
    unsafe { std::env::remove_var("GROQ_API_KEY") };
}

#[test]
fn test_discover_providers_generic_openai_compatible() {
    unsafe {
        std::env::set_var("OPENAI_BASE_URL", "https://my-proxy.example.com/v1");
        std::env::set_var("OPENAI_API_KEY", "sk-custom");
        std::env::set_var("OPENAI_PROVIDER_ID", "my-proxy");
        std::env::set_var("OPENAI_PROVIDER_NAME", "My Proxy");
    }
    let providers = discover_providers_from_env();
    let proxy = providers
        .get("my-proxy")
        .expect("custom provider should be discovered");
    assert_eq!(
        proxy.base_url.as_deref(),
        Some("https://my-proxy.example.com/v1")
    );
    assert_eq!(proxy.env_key.as_deref(), Some("OPENAI_API_KEY"));
    unsafe {
        std::env::remove_var("OPENAI_BASE_URL");
        std::env::remove_var("OPENAI_API_KEY");
        std::env::remove_var("OPENAI_PROVIDER_ID");
        std::env::remove_var("OPENAI_PROVIDER_NAME");
    }
}

// ---------------------------------------------------------------------------
// create_custom_provider tests
// ---------------------------------------------------------------------------

#[test]
fn test_create_custom_provider() {
    let provider = create_custom_provider(
        "My LLM",
        "https://api.example.com/v1",
        Some("MY_LLM_API_KEY"),
        WireApi::ChatCompletions,
    );
    assert_eq!(provider.name, "My LLM");
    assert_eq!(
        provider.base_url.as_deref(),
        Some("https://api.example.com/v1")
    );
    assert_eq!(provider.env_key.as_deref(), Some("MY_LLM_API_KEY"));
    assert_eq!(provider.wire_api, WireApi::ChatCompletions);
}

#[test]
fn test_create_custom_provider_no_auth() {
    let provider = create_custom_provider(
        "Local LLM",
        "http://localhost:8080/v1",
        None,
        WireApi::Responses,
    );
    assert_eq!(provider.name, "Local LLM");
    assert!(provider.env_key.is_none());
    assert_eq!(provider.wire_api, WireApi::Responses);
}

// ---------------------------------------------------------------------------
// built_in_model_providers includes auto-discovered
// ---------------------------------------------------------------------------

#[test]
fn test_built_in_includes_discovered_providers() {
    // Test that built_in_model_providers merges auto-discovered providers.
    // We can't reliably test env-var-based discovery in parallel tests due to
    // thread safety, so test the merge logic directly.
    let mut custom = HashMap::new();
    custom.insert(
        "custom-provider".to_string(),
        ModelProviderInfo {
            name: "Custom".to_string(),
            base_url: Some("https://custom.example.com/v1".to_string()),
            wire_api: WireApi::ChatCompletions,
            ..ModelProviderInfo::default()
        },
    );
    let merged = merge_configured_model_providers(built_in_model_providers(None), custom).unwrap();
    assert!(merged.contains_key("openai"));
    assert!(merged.contains_key("custom-provider"));
    let custom = merged.get("custom-provider").unwrap();
    assert_eq!(custom.wire_api, WireApi::ChatCompletions);
}
