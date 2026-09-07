//! Provider connection popup for `ChatWidget`.
//!
//! Flow: Select provider → Enter API key → Fetch models → Select model
//! → Select variant (reasoning effort) → Close → Start prompting.
//!
//! Provider catalog comes from models.dev (cached). Credentials stored in
//! `~/.sofia/providers.json`.

use super::*;
use crate::bottom_pane::SelectionItem;
use crate::bottom_pane::SelectionViewParams;
use crate::bottom_pane::custom_prompt_view::CustomPromptView;
use crate::render::renderable::ColumnRenderable;

// ---------------------------------------------------------------------------
// Provider entry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct ProviderEntry {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub wire_api: String,
}

// ---------------------------------------------------------------------------
// Providers config file (~/.sofia/providers.json)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub(crate) struct ProvidersConfig {
    pub providers: std::collections::HashMap<String, ProviderConfig>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct ProviderConfig {
    pub api_key: String,
    pub base_url: String,
    pub wire_api: String,
    pub name: String,
}

fn providers_config_path() -> String {
    format!(
        "{}/providers.json",
        codex_utils_home_dir::codex_home_string()
    )
}

pub(crate) fn load_providers_config() -> ProvidersConfig {
    let path = providers_config_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub(crate) fn save_providers_config(config: &ProvidersConfig) -> Result<(), String> {
    let path = providers_config_path();
    let dir = std::path::Path::new(&path)
        .parent()
        .ok_or("cannot determine config dir")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())?;
    Ok(())
}

/// Path to the engine's credential store (`sofia-auth.json`), which is the file
/// the engine reads to resolve provider API keys (see `ModelProviderInfo::api_key`).
fn sofia_auth_path() -> String {
    format!(
        "{}/sofia-auth.json",
        codex_utils_home_dir::codex_home_string()
    )
}

/// Write (or merge) an API key into `sofia-auth.json` under the given key name
/// (typically the provider's `env_key`). The engine reads credentials from this
/// file, so this is what makes a connected provider actually able to authenticate.
pub(crate) fn save_auth_key(key_name: &str, api_key: &str) -> Result<(), String> {
    let path = sofia_auth_path();
    let dir = std::path::Path::new(&path)
        .parent()
        .ok_or("cannot determine config dir")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;

    let mut auth: serde_json::Map<String, serde_json::Value> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .and_then(|v: serde_json::Value| v.as_object().cloned())
        .unwrap_or_default();
    auth.insert(
        key_name.to_string(),
        serde_json::Value::String(api_key.trim().to_string()),
    );
    let json = serde_json::to_string_pretty(&auth).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())?;
    Ok(())
}

/// Build the updated `config.toml` content that selects `provider_id`/`model_id`/
/// `effort` at the top level and registers the provider under `[model_providers.X]`.
///
/// This must set `model_provider` at the top level (not nested inside the
/// `[model_providers.X]` table), otherwise the engine never sees the provider
/// override and keeps routing to its default (e.g. OpenAI).
pub(crate) fn build_provider_config_toml(
    existing: &str,
    provider_id: &str,
    model_id: &str,
    effort: &str,
    provider_info: &ProviderConfig,
) -> Result<String, String> {
    let mut doc: toml::Table =
        toml::from_str(existing).map_err(|e| format!("failed to parse config.toml: {e}"))?;

    // Top-level model selection keys.
    doc.insert(
        "model".to_string(),
        toml::Value::String(model_id.to_string()),
    );
    doc.insert(
        "model_provider".to_string(),
        toml::Value::String(provider_id.to_string()),
    );
    doc.insert(
        "model_reasoning_effort".to_string(),
        toml::Value::String(effort.to_string()),
    );

    // Build the [model_providers.X] table.
    let env_key_name = format!("{}_API_KEY", provider_id.to_uppercase().replace('-', "_"));
    let providers = doc
        .entry("model_providers".to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let provider_table = providers
        .as_table_mut()
        .ok_or("`model_providers` is not a table")?;
    let entry = provider_table
        .entry(provider_id.to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let entry = entry
        .as_table_mut()
        .ok_or("`model_providers.<id>` is not a table")?;
    entry.insert(
        "name".to_string(),
        toml::Value::String(provider_info.name.clone()),
    );
    entry.insert(
        "base_url".to_string(),
        toml::Value::String(provider_info.base_url.clone()),
    );
    entry.insert(
        "env_key".to_string(),
        toml::Value::String(env_key_name.clone()),
    );
    entry.insert(
        "wire_api".to_string(),
        toml::Value::String(provider_info.wire_api.clone()),
    );

    toml::to_string_pretty(&doc).map_err(|e| format!("failed to serialize config.toml: {e}"))
}

// ---------------------------------------------------------------------------
// Provider catalog (models.dev cache + well-known fallback)
// ---------------------------------------------------------------------------

/// Maximum age of the models.dev cache before re-fetching (24 hours).
const MODELS_DEV_CACHE_MAX_AGE_SECS: u64 = 24 * 60 * 60;

pub(crate) fn load_providers() -> Vec<ProviderEntry> {
    let cache_path = format!(
        "{}/models_dev_cache.json",
        codex_utils_home_dir::codex_home_string()
    );

    // Try cached catalog first (only if not expired).
    let cache_is_fresh = std::fs::metadata(&cache_path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|mtime| mtime.elapsed().ok())
        .map(|age| age.as_secs() < MODELS_DEV_CACHE_MAX_AGE_SECS)
        .unwrap_or(false);

    if cache_is_fresh {
        if let Ok(catalog) = std::fs::read_to_string(&cache_path) {
            if let Some(providers) = parse_models_dev_catalog(&catalog) {
                return providers;
            }
        }
    }

    // Cache miss or expired — fetch from models.dev and cache.
    if let Ok(catalog) = fetch_models_dev_catalog() {
        let _ = std::fs::create_dir_all(std::path::Path::new(&cache_path).parent().unwrap());
        let _ = std::fs::write(&cache_path, &catalog);
        if let Some(providers) = parse_models_dev_catalog(&catalog) {
            return providers;
        }
    }

    // If the cache expired but the fetch failed, serve stale data as fallback.
    if !cache_is_fresh {
        if let Ok(catalog) = std::fs::read_to_string(&cache_path) {
            if let Some(providers) = parse_models_dev_catalog(&catalog) {
                return providers;
            }
        }
    }

    well_known_providers()
}

/// Parse the models.dev catalog JSON into a list of provider entries.
fn parse_models_dev_catalog(json: &str) -> Option<Vec<ProviderEntry>> {
    let data: serde_json::Value = serde_json::from_str(json).ok()?;
    let obj = data.as_object()?;
    let mut providers: Vec<ProviderEntry> = obj
        .iter()
        .filter_map(|(id, val)| {
            let name = val.get("name")?.as_str()?.to_string();
            let api = val.get("api").and_then(|v| v.as_str()).unwrap_or("");
            if api.is_empty() {
                return None;
            }
            Some(ProviderEntry {
                id: id.clone(),
                name,
                base_url: api.to_string(),
                wire_api: "chat_completions".to_string(),
            })
        })
        .collect();
    if providers.is_empty() {
        return None;
    }
    providers.sort_by(|a, b| a.name.cmp(&b.name));
    Some(providers)
}

/// Fetch the models.dev catalog JSON.
fn fetch_models_dev_catalog() -> Result<String, String> {
    let output = std::process::Command::new("curl")
        .arg("-s")
        .arg("--max-time")
        .arg("15")
        .arg("https://models.dev/api.json")
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        String::from_utf8(output.stdout).map_err(|e| e.to_string())
    } else {
        Err(format!("curl failed with status: {}", output.status))
    }
}

fn well_known_providers() -> Vec<ProviderEntry> {
    vec![
        ProviderEntry {
            id: "xiaomi".into(),
            name: "Xiaomi (MiMo)".into(),
            base_url: "https://api.xiaomimimo.com/v1".into(),
            wire_api: "chat_completions".into(),
        },
        ProviderEntry {
            id: "anthropic".into(),
            name: "Anthropic".into(),
            base_url: "https://api.anthropic.com/v1".into(),
            wire_api: "chat_completions".into(),
        },
        ProviderEntry {
            id: "openrouter".into(),
            name: "OpenRouter".into(),
            base_url: "https://openrouter.ai/api/v1".into(),
            wire_api: "chat_completions".into(),
        },
        ProviderEntry {
            id: "groq".into(),
            name: "Groq".into(),
            base_url: "https://api.groq.com/openai/v1".into(),
            wire_api: "chat_completions".into(),
        },
        ProviderEntry {
            id: "deepseek".into(),
            name: "DeepSeek".into(),
            base_url: "https://api.deepseek.com/v1".into(),
            wire_api: "chat_completions".into(),
        },
        ProviderEntry {
            id: "mistral".into(),
            name: "Mistral AI".into(),
            base_url: "https://api.mistral.ai/v1".into(),
            wire_api: "chat_completions".into(),
        },
        ProviderEntry {
            id: "together".into(),
            name: "Together AI".into(),
            base_url: "https://api.together.xyz/v1".into(),
            wire_api: "chat_completions".into(),
        },
        ProviderEntry {
            id: "xai".into(),
            name: "xAI (Grok)".into(),
            base_url: "https://api.x.ai/v1".into(),
            wire_api: "chat_completions".into(),
        },
        ProviderEntry {
            id: "google".into(),
            name: "Google Gemini".into(),
            base_url: "https://generativelanguage.googleapis.com/v1beta".into(),
            wire_api: "chat_completions".into(),
        },
        ProviderEntry {
            id: "fireworks".into(),
            name: "Fireworks AI".into(),
            base_url: "https://api.fireworks.ai/inference/v1".into(),
            wire_api: "chat_completions".into(),
        },
        ProviderEntry {
            id: "cerebras".into(),
            name: "Cerebras".into(),
            base_url: "https://api.cerebras.ai/v1".into(),
            wire_api: "chat_completions".into(),
        },
        ProviderEntry {
            id: "perplexity".into(),
            name: "Perplexity".into(),
            base_url: "https://api.perplexity.ai".into(),
            wire_api: "chat_completions".into(),
        },
        ProviderEntry {
            id: "cohere".into(),
            name: "Cohere".into(),
            base_url: "https://api.cohere.com/v2".into(),
            wire_api: "chat_completions".into(),
        },
    ]
}

// ---------------------------------------------------------------------------
// ChatWidget methods
// ---------------------------------------------------------------------------

impl ChatWidget {
    /// Step 1: Show provider picker.
    pub(crate) fn open_connect_provider_popup(&mut self) {
        let entries = load_providers();
        let config = load_providers_config();

        let items: Vec<SelectionItem> = entries
            .iter()
            .map(|entry| {
                let is_configured = config.providers.contains_key(&entry.id);
                let status = if is_configured { " [configured]" } else { "" };
                let name = format!("{}{}", entry.name, status);
                let description = Some(entry.base_url.clone());

                let entry_clone = entry.clone();
                let tx = self.app_event_tx.clone();

                let action: crate::bottom_pane::SelectionAction =
                    Box::new(move |_: &AppEventSender| {
                        let _ = tx.send(AppEvent::ConnectProvider {
                            provider_id: entry_clone.id.clone(),
                            provider_name: entry_clone.name.clone(),
                            base_url: entry_clone.base_url.clone(),
                            wire_api: entry_clone.wire_api.clone(),
                        });
                    });
                let search_val = format!("{} {}", entry.name, entry.id);
                SelectionItem {
                    name,
                    description,
                    search_value: Some(search_val),
                    actions: vec![action],
                    ..Default::default()
                }
            })
            .collect();

        let mut header = ColumnRenderable::new();
        header.push(Line::from("Connect a Model Provider".bold()));
        header.push(Line::from("Select a provider to configure.".dim()));

        self.bottom_pane.show_selection_view(SelectionViewParams {
            title: None,
            subtitle: None,
            header: Box::new(header),
            footer_hint: Some(Line::from(
                "↑↓ navigate · Enter select · Esc cancel · type to search".dim(),
            )),
            is_searchable: true,
            search_placeholder: Some("Search providers...".to_string()),
            items,
            ..Default::default()
        });
    }

    /// Step 2: Prompt for API key.
    ///
    /// `existing_key` pre-fills the input when the provider is already
    /// configured, so the user can review or change it.
    pub(crate) fn prompt_for_provider_api_key(
        &mut self,
        provider_id: String,
        provider_name: String,
        base_url: String,
        wire_api: String,
        existing_key: String,
    ) {
        let tx = self.app_event_tx.clone();
        let prompt = CustomPromptView::new(
            format!("API Key for {provider_name}"),
            format!("Paste your API key for {provider_name}"),
            existing_key,
            Some(format!("Base URL: {base_url}")),
            Box::new(move |input: String| {
                let key = input.trim().to_string();
                if key.is_empty() {
                    return;
                }
                let _ = tx.send(AppEvent::SaveProviderApiKey {
                    provider_id: provider_id.clone(),
                    provider_name: provider_name.clone(),
                    base_url: base_url.clone(),
                    wire_api: wire_api.clone(),
                    api_key: key,
                });
            }),
        );
        self.bottom_pane.show_text_prompt(prompt);
    }

    /// Step 4: Show model picker for a configured provider.
    /// Kick off an async model-list fetch for a provider.  The result arrives
    /// as `AppEvent::ModelsFetched` — no blocking on the TUI thread.
    pub(crate) fn fetch_models_for_provider(&mut self, provider_id: String, provider_name: String) {
        let config = load_providers_config();
        let Some(provider_config) = config.providers.get(&provider_id).cloned() else {
            self.add_error_message(format!("No config found for provider '{provider_id}'"));
            return;
        };
        let tx = self.app_event_tx.clone();
        tokio::task::spawn_blocking(move || {
            let result = fetch_models(&provider_config.base_url, &provider_config.api_key);
            let _ = tx.send(AppEvent::ModelsFetched {
                provider_id,
                provider_name,
                result,
            });
        });
    }

    /// Step 4: Show model picker from a pre-fetched model list (called from the
    /// `ModelsFetched` event handler — no I/O on the TUI thread).
    pub(crate) fn show_model_picker(&mut self, provider_id: String, models: Vec<String>) {
        if models.is_empty() {
            self.add_info_message(
                format!("No models found for {provider_id}. Check your API key and base URL."),
                None,
            );
            return;
        }

        let items: Vec<SelectionItem> = models
            .iter()
            .map(|model| {
                let mid = model.clone();
                let mid_name = model.clone();
                let pid = provider_id.clone();
                let tx = self.app_event_tx.clone();
                let action: crate::bottom_pane::SelectionAction =
                    Box::new(move |_: &AppEventSender| {
                        let _ = tx.send(AppEvent::SelectModel {
                            provider_id: pid.clone(),
                            model_id: mid.clone(),
                        });
                    });
                SelectionItem {
                    name: mid_name,
                    description: None,
                    search_value: Some(model.clone()),
                    actions: vec![action],
                    ..Default::default()
                }
            })
            .collect();

        let mut header = ColumnRenderable::new();
        header.push(Line::from(format!("Models for {provider_id}").bold()));
        header.push(Line::from("Select a model.".dim()));

        self.bottom_pane.show_selection_view(SelectionViewParams {
            title: None,
            subtitle: None,
            header: Box::new(header),
            footer_hint: Some(Line::from(
                "↑↓ navigate · Enter select · Esc cancel · type to search".dim(),
            )),
            is_searchable: true,
            search_placeholder: Some("Search models...".to_string()),
            items,
            ..Default::default()
        });
    }

    /// Step 5: Show effort picker.
    pub(crate) fn open_effort_picker_for_model(&mut self, provider_id: String, model_id: String) {
        let efforts = [
            ("low", "Low — fast, lighter reasoning"),
            ("medium", "Medium — balanced (default)"),
            ("high", "High — greater reasoning depth"),
            ("max", "Max — maximum reasoning depth"),
        ];

        let items: Vec<SelectionItem> = efforts
            .iter()
            .map(|(effort, desc)| {
                let effort_str = effort.to_string();
                let pid = provider_id.clone();
                let mid = model_id.clone();
                let tx = self.app_event_tx.clone();
                let action: crate::bottom_pane::SelectionAction =
                    Box::new(move |_: &AppEventSender| {
                        let _ = tx.send(AppEvent::FinalizeProviderSetup {
                            provider_id: pid.clone(),
                            model_id: mid.clone(),
                            effort: effort_str.clone(),
                        });
                    });
                SelectionItem {
                    name: effort.to_string(),
                    description: Some(desc.to_string()),
                    actions: vec![action],
                    dismiss_on_select: true,
                    ..Default::default()
                }
            })
            .collect();

        let mut header = ColumnRenderable::new();
        header.push(Line::from(
            format!("Reasoning Effort for {model_id}").bold(),
        ));
        header.push(Line::from("Select reasoning depth.".dim()));

        self.bottom_pane.show_selection_view(SelectionViewParams {
            title: None,
            subtitle: None,
            header: Box::new(header),
            footer_hint: Some(Line::from("↑↓ navigate · Enter select · Esc cancel".dim())),
            items,
            ..Default::default()
        });
    }
}

// ---------------------------------------------------------------------------
// Model fetching (provider /models endpoint)
// ---------------------------------------------------------------------------

fn fetch_models(base_url: &str, api_key: &str) -> Result<Vec<String>, String> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let result = std::process::Command::new("curl")
        .arg("-s")
        .arg("--max-time")
        .arg("10")
        .arg("-H")
        .arg(format!("Authorization: Bearer {api_key}"))
        .arg(&url)
        .output()
        .map_err(|e| format!("Failed to run curl: {e}"))?;

    if !result.status.success() {
        let _stderr = String::from_utf8_lossy(&result.stderr);
        let stdout = String::from_utf8_lossy(&result.stdout);
        // HTTP 401/403 → likely bad API key.
        if stdout.contains(r#""error""#)
            && (stdout.contains("401")
                || stdout.contains("403")
                || stdout.contains("unauthorized")
                || stdout.contains("Invalid API key"))
        {
            return Err(format!(
                "Authentication failed. Check your API key for this provider."
            ));
        }
        return Err(format!(
            "HTTP error from /models endpoint (exit {})",
            result.status,
        ));
    }

    let body = String::from_utf8_lossy(&result.stdout).to_string();
    let data: serde_json::Value = serde_json::from_str(&body)
        .map_err(|_| "Invalid JSON response from provider".to_string())?;

    // Check for error responses.
    if let Some(err) = data.get("error") {
        let msg = err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown");
        return Err(format!("Provider error: {msg}"));
    }

    // OpenAI-compatible: {"data": [{"id":"..."}]}
    if let Some(arr) = data.get("data").and_then(|v| v.as_array()) {
        let models: Vec<String> = arr
            .iter()
            .filter_map(|m| m.get("id")?.as_str().map(String::from))
            .collect();
        if !models.is_empty() {
            return Ok(models);
        }
    }
    // Internal: {"models": [...]}
    if let Some(arr) = data.get("models").and_then(|v| v.as_array()) {
        let models: Vec<String> = arr
            .iter()
            .filter_map(|m| {
                m.get("slug")
                    .or_else(|| m.get("id"))?
                    .as_str()
                    .map(String::from)
            })
            .collect();
        if !models.is_empty() {
            return Ok(models);
        }
    }
    Err(
        "No models found in provider response. The provider may not expose a /models endpoint."
            .to_string(),
    )
}

#[cfg(test)]
#[path = "connect_provider_popup_tests.rs"]
mod tests;
