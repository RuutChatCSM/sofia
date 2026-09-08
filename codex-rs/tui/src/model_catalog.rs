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

    /// Add a model preset to the catalog (for newly configured providers).
    pub(crate) fn add_model(&self, preset: ModelPreset) {
        let mut models = self.models.lock().unwrap();
        if !models.iter().any(|m| m.model == preset.model) {
            models.push(preset);
        }
    }
}
