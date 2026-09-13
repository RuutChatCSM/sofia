use super::*;

fn sample_provider() -> ProviderConfig {
    ProviderConfig {
        api_key: "sk-test".to_string(),
        base_url: "https://api.xiaomimimo.com/v1".to_string(),
        wire_api: "chat_completions".to_string(),
        name: "Xiaomi (MiMo)".to_string(),
        models: Vec::new(),
    }
}

#[test]
fn provider_selection_is_written_at_top_level() {
    let existing = "model = \"gpt-5.1-codex\"\nmodel_reasoning_effort = \"high\"\n";
    let out =
        build_provider_config_toml(existing, "xiaomi", "mimo-v2.5", "max", &sample_provider())
            .unwrap();
    let doc: toml::Table = toml::from_str(&out).unwrap();

    // `model_provider` must resolve at the top level so the engine actually
    // switches providers.
    assert_eq!(
        doc.get("model_provider").and_then(|v| v.as_str()),
        Some("xiaomi")
    );
    assert_eq!(doc.get("model").and_then(|v| v.as_str()), Some("mimo-v2.5"));
    assert_eq!(
        doc.get("model_reasoning_effort").and_then(|v| v.as_str()),
        Some("max")
    );
}

#[test]
fn provider_entry_is_registered_under_model_providers() {
    let out =
        build_provider_config_toml("", "xiaomi", "mimo-v2.5", "max", &sample_provider()).unwrap();
    let doc: toml::Table = toml::from_str(&out).unwrap();

    let providers = doc
        .get("model_providers")
        .and_then(|v| v.as_table())
        .unwrap();
    let xiaomi = providers.get("xiaomi").and_then(|v| v.as_table()).unwrap();
    assert_eq!(
        xiaomi.get("name").and_then(|v| v.as_str()),
        Some("Xiaomi (MiMo)")
    );
    assert_eq!(
        xiaomi.get("base_url").and_then(|v| v.as_str()),
        Some("https://api.xiaomimimo.com/v1")
    );
    assert_eq!(
        xiaomi.get("env_key").and_then(|v| v.as_str()),
        Some("XIAOMI_API_KEY")
    );
    assert_eq!(
        xiaomi.get("wire_api").and_then(|v| v.as_str()),
        Some("chat_completions")
    );

    // `model_provider` stays top-level even when a provider table already exists.
    assert_eq!(
        doc.get("model_provider").and_then(|v| v.as_str()),
        Some("xiaomi")
    );
}

#[test]
fn provider_selection_is_not_nested_in_existing_section() {
    // Regression: appending keys after a `[model_providers.X]` header used to
    // nest `model_provider` inside that table. Ensure it stays top-level.
    let existing = "model = \"old\"\n\n[model_providers.xiaomi]\nname = \"X\"\n";
    let out =
        build_provider_config_toml(existing, "xiaomi", "mimo-v2.5", "max", &sample_provider())
            .unwrap();
    let doc: toml::Table = toml::from_str(&out).unwrap();

    assert_eq!(
        doc.get("model_provider").and_then(|v| v.as_str()),
        Some("xiaomi")
    );
    let providers = doc
        .get("model_providers")
        .and_then(|v| v.as_table())
        .unwrap();
    let xiaomi = providers.get("xiaomi").and_then(|v| v.as_table()).unwrap();
    assert_eq!(
        xiaomi.get("name").and_then(|v| v.as_str()),
        Some("Xiaomi (MiMo)")
    );
}

#[test]
fn preserves_unrelated_top_level_keys() {
    let existing = "model = \"old\"\nfoo = \"bar\"\n";
    let out =
        build_provider_config_toml(existing, "xiaomi", "mimo-v2.5", "max", &sample_provider())
            .unwrap();
    let doc: toml::Table = toml::from_str(&out).unwrap();
    assert_eq!(doc.get("foo").and_then(|v| v.as_str()), Some("bar"));
}

#[test]
fn parse_provider_models_extracts_names_and_falls_back_to_id() {
    let provider = serde_json::json!({
        "models": {
            "deepseek-flash": { "name": "DeepSeek V4.1 Flash", "reasoning": true },
            "deepseek-v4-pro": {}
        }
    });
    let models = parse_provider_models(&provider);
    let mut ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(ids, vec!["deepseek-flash", "deepseek-v4-pro"]);
    let flash = models
        .iter()
        .find(|model| model.id == "deepseek-flash")
        .expect("flash present");
    assert_eq!(flash.name, "DeepSeek V4.1 Flash");
    assert!(flash.reasoning);
    let pro = models
        .iter()
        .find(|model| model.id == "deepseek-v4-pro")
        .expect("pro present");
    assert_eq!(pro.name, "deepseek-v4-pro");
    assert!(!pro.reasoning);
}

#[test]
fn merge_provider_models_keeps_catalog_names_and_appends_live_only_ids() {
    let catalog = vec![ProviderModel {
        id: "deepseek-flash".to_string(),
        name: "DeepSeek V4.1 Flash".to_string(),
        reasoning: true,
    }];
    let merged = merge_provider_models(
        catalog,
        vec!["deepseek-flash".to_string(), "deepseek-v4-pro".to_string()],
    );
    assert_eq!(
        merged,
        vec![
            ProviderModel {
                id: "deepseek-flash".to_string(),
                name: "DeepSeek V4.1 Flash".to_string(),
                reasoning: true,
            },
            ProviderModel {
                id: "deepseek-v4-pro".to_string(),
                name: "deepseek-v4-pro".to_string(),
                reasoning: false,
            },
        ]
    );
}
