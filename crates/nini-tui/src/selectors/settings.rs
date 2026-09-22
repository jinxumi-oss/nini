//! SettingsSelector — pick a setting category to edit (model / theme /
//! thinking level). Selecting one doesn't change anything directly; the
//! runtime opens the corresponding sub-selector (ModelSelector /
//! ThinkingSelector) afterwards.

use std::any::Any;

use crate::selector::{SelectorItem, SelectorOutcome, SelectorState};
use crate::settings::SettingsManager;

#[derive(Debug, Clone)]
pub struct SettingsSelector {
    pub result: Option<String>,
    pub settings: SettingsSnapshot,
    pub items: Vec<SelectorItem>,
    pub selected: usize,
}

/// Snapshot of the current settings values, shown in the description.
#[derive(Debug, Clone, Default)]
pub struct SettingsSnapshot {
    pub model: String,
    pub theme: String,
    pub thinking: String,
}

impl SettingsSnapshot {
    pub fn model_name(&self) -> String {
        self.model.clone()
    }
}

impl SettingsSelector {
    pub fn new(manager: SettingsManager) -> Self {
        let snapshot = SettingsSnapshot {
            model: manager.settings.default_model.clone().unwrap_or_default(),
            theme: manager.theme.clone(),
            thinking: manager.settings.default_thinking_level.clone().unwrap_or_default(),
        };
        Self::with_snapshot(snapshot)
    }

    pub fn with_snapshot(snapshot: SettingsSnapshot) -> Self {
        let items = vec![
            SelectorItem {
                id: "model".to_string(),
                label: "model".to_string(),
                description: Some(snapshot.model.clone()),
                is_current: false,
            },
            SelectorItem {
                id: "theme".to_string(),
                label: "theme".to_string(),
                description: Some(snapshot.theme.clone()),
                is_current: false,
            },
            SelectorItem {
                id: "thinking".to_string(),
                label: "thinking".to_string(),
                description: Some(snapshot.thinking.clone()),
                is_current: false,
            },
        ];
        Self {
            result: None,
            settings: snapshot,
            items,
            selected: 0,
        }
    }

    /// Persist the picked setting back to a SettingsManager. Used by
    /// runtime when the user picks e.g. "model" (sub-selector opens).
    pub fn apply(&mut self, _idx: usize) -> Option<String> {
        self.result.clone()
    }
}

impl SelectorState for SettingsSelector {
    fn as_any_mut(&mut self) -> &mut dyn Any { self }
    fn state_items(&self) -> Vec<SelectorItem> { self.items.clone() }
    fn state_title(&self) -> String { "Settings".to_string() }
    fn state_selected(&self) -> usize { self.selected }
    fn state_set_selected(&mut self, idx: usize) {
        if idx < self.items.len() {
            self.selected = idx;
        }
    }
    fn state_on_select(&mut self) -> SelectorOutcome {
        let item = self.items.get(self.selected).cloned();
        match item {
            Some(it) => {
                self.result = Some(it.id.clone());
                SelectorOutcome::Picked(it)
            }
            None => SelectorOutcome::Cancelled,
        }
    }
}
