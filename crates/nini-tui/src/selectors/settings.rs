//! SettingsSelector — pick a setting category to inspect (model / theme /
//! thinking level).
//!
//! The module doc used to say that selecting a category makes "the runtime
//! open the corresponding sub-selector (ModelSelector / ThinkingSelector)
//! afterwards". It does not: `apply_selector_result` has no route from a
//! picked category to a sub-selector, and no `ThemeSelector` exists at
//! all. What picking a category does today is nothing. That is recorded
//! here rather than papered over, because the alternative - a comment
//! promising a flow the code does not have - is how `/settings` came to
//! look finished while being inert.

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
            thinking: manager
                .settings
                .default_thinking_level
                .clone()
                .unwrap_or_default(),
        };
        Self::with_snapshot(snapshot)
    }

    pub fn with_snapshot(snapshot: SettingsSnapshot) -> Self {
        // An unset value used to render as an empty string, which in a
        // selector list is indistinguishable from a row that failed to
        // load. It says so instead.
        // A nested `fn`, not a closure: a closure would have to infer a
        // single lifetime for both arms, and the return type differs.
        fn show(v: &str) -> &str {
            if v.is_empty() {
                "(unset)"
            } else {
                v
            }
        }
        let items = vec![
            SelectorItem {
                value: "model".to_string(),
                label: "model".to_string(),
                description: Some(show(&snapshot.model).to_string()),
                is_current: false,
            },
            SelectorItem {
                value: "theme".to_string(),
                label: "theme".to_string(),
                description: Some(show(&snapshot.theme).to_string()),
                is_current: false,
            },
            SelectorItem {
                value: "thinking".to_string(),
                label: "thinking".to_string(),
                description: Some(show(&snapshot.thinking).to_string()),
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

    /// Which category the user picked: `"model"`, `"theme"` or
    /// `"thinking"`.
    ///
    /// This used to be
    ///
    /// ```text
    /// /// Persist the picked setting back to a SettingsManager. Used by
    /// /// runtime when the user picks e.g. "model" (sub-selector opens).
    /// pub fn apply(&mut self, _idx: usize) -> Option<String> {
    ///     self.result.clone()
    /// }
    /// ```
    ///
    /// Three things wrong with it. It persisted nothing and the selector
    /// holds no manager - only a `SettingsSnapshot` - so the comment
    /// described an object that is not there. It ignored its index, so
    /// every category returned the same answer. And its caller treated
    /// that answer as a new model name, so picking "theme" or "thinking"
    /// reported a model change that had not happened.
    ///
    /// The category at `idx`, or `None` if that row does not exist.
    ///
    /// Any in-range index answers. The first version of this had an
    /// `idx == self.selected` guard on it, which meant a caller asking
    /// about a row the user was not currently on got nothing - the same
    /// index-blindness as the function it replaced, just pointing the
    /// other way.
    pub fn picked_category(&self, idx: usize) -> Option<&str> {
        self.items.get(idx).map(|i| i.value.as_str())
    }
}

impl SelectorState for SettingsSelector {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn state_items(&self) -> Vec<SelectorItem> {
        self.items.clone()
    }
    fn state_title(&self) -> String {
        "Settings".to_string()
    }
    fn state_selected(&self) -> usize {
        self.selected
    }
    fn state_set_selected(&mut self, idx: usize) {
        if idx < self.items.len() {
            self.selected = idx;
        }
    }
    fn state_on_select(&mut self) -> SelectorOutcome {
        let item = self.items.get(self.selected).cloned();
        match item {
            Some(it) => {
                self.result = Some(it.value.clone());
                SelectorOutcome::Picked(it)
            }
            None => SelectorOutcome::Cancelled,
        }
    }
}
