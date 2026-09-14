//! TUI model and collaboration inventories; refreshing models preserves the server mode catalog.

use codex_protocol::config_types::CollaborationModeMask;
use codex_protocol::openai_models::ModelPreset;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::Mutex;

pub(crate) const LUNA_RESERVE_MODEL: &str = "gpt-reserve";
pub(crate) const LUNA_MODEL: &str = "gpt-5.6-luna";

pub(crate) fn model_display_name(model: &str) -> &str {
    if model.eq_ignore_ascii_case(LUNA_RESERVE_MODEL) {
        "Luna Reserve"
    } else {
        model
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ModelCatalog {
    pub(crate) models: Arc<Mutex<Vec<ModelPreset>>>,
    pub(crate) collaboration_modes: Vec<CollaborationModeMask>,
}

impl ModelCatalog {
    pub(crate) fn new(models: Vec<ModelPreset>) -> Self {
        Self {
            models: Arc::new(Mutex::new(models)),
            collaboration_modes: Vec::new(),
        }
    }

    pub(crate) fn with_collaboration_modes(mut self, modes: Vec<CollaborationModeMask>) -> Self {
        self.collaboration_modes = modes;
        self
    }

    pub(crate) fn try_list_models(&self) -> Result<Vec<ModelPreset>, Infallible> {
        Ok(self.models.lock().unwrap().clone())
    }

    /// Add a model preset to the catalog. If the slug already exists, merge
    /// the richer catalog metadata (display name, reasoning flags) into the
    /// existing entry so the picker shows proper names instead of blank/engine
    /// defaults.
    pub(crate) fn add_model(&self, preset: ModelPreset) {
        let mut models = self.models.lock().unwrap();
        if let Some(existing) = models.iter_mut().find(|m| m.model == preset.model) {
            if existing.display_name.is_empty() && !preset.display_name.is_empty() {
                existing.display_name = preset.display_name;
            }
            if existing.supported_reasoning_efforts.is_empty()
                && !preset.supported_reasoning_efforts.is_empty()
            {
                existing.supported_reasoning_efforts = preset.supported_reasoning_efforts;
            }
            if existing.description.is_empty() && !preset.description.is_empty() {
                existing.description = preset.description;
            }
        } else {
            models.push(preset);
        }
    }

    /// Add every configured provider's discovered models to the catalog,
    /// models.dev-style. The active provider's models are added unprefixed so
    /// selecting them keeps the current provider; every other connected
    /// provider's models are added namespaced `provider/model` so selecting one
    /// switches providers. Providers with no live-fetched list fall back to the
    /// cached models.dev catalog.
    pub(crate) fn add_connected_provider_models(
        &self,
        active_provider_id: &str,
        codex_home: &std::path::Path,
    ) {
        use codex_protocol::openai_models::InputModality;
        use codex_protocol::openai_models::ReasoningEffort;
        use codex_protocol::openai_models::ReasoningEffortPreset;

        let config =
            crate::chatwidget::connect_provider_popup::load_providers_config_in(codex_home);
        let cached = crate::chatwidget::connect_provider_popup::cached_catalog_models(codex_home);
        for (provider_id, provider) in config.providers {
            // Always union the models.dev catalog (authoritative names and
            // reasoning flags) with whatever the live endpoint returned. A
            // stored live-only list (e.g. DeepSeek's v4 ids) must not hide
            // newer catalog models such as DeepSeek V4.1 Flash.
            let stored_ids = provider.models.into_iter().map(|model| model.id).collect();
            let catalog = cached.get(&provider_id).cloned().unwrap_or_default();
            let models = crate::chatwidget::connect_provider_popup::merge_provider_models(
                catalog, stored_ids,
            );
            let is_active = provider_id == active_provider_id;
            for model in models {
                let (slug, description) = if is_active {
                    (model.id, provider.name.clone())
                } else {
                    (
                        format!("{provider_id}/{}", model.id),
                        format!("{} via {provider_id}", provider.name),
                    )
                };
                // Catalog-only models still get an effort picker when models.dev
                // marks them reasoning-capable; otherwise the model is applied
                // directly (and the picker must close — handled by the popup).
                let supported_reasoning_efforts = if model.reasoning {
                    vec![
                        ReasoningEffortPreset {
                            effort: ReasoningEffort::Low,
                            description: "Low — fast, lighter reasoning".to_string(),
                        },
                        ReasoningEffortPreset {
                            effort: ReasoningEffort::Medium,
                            description: "Medium — balanced".to_string(),
                        },
                        ReasoningEffortPreset {
                            effort: ReasoningEffort::High,
                            description: "High — greater reasoning depth".to_string(),
                        },
                    ]
                } else {
                    Vec::new()
                };
                self.add_model(ModelPreset {
                    id: slug.clone(),
                    model: slug,
                    display_name: model.name,
                    description,
                    model_specialty: None,
                    default_reasoning_effort: ReasoningEffort::Medium,
                    supported_reasoning_efforts,
                    supports_personality: false,
                    additional_speed_tiers: Vec::new(),
                    service_tiers: Vec::new(),
                    default_service_tier: None,
                    is_default: false,
                    upgrade: None,
                    show_in_picker: true,
                    multi_agent_version: None,
                    availability_nux: None,
                    supported_in_api: true,
                    input_modalities: vec![InputModality::Text],
                });
            }
        }
    }
}
