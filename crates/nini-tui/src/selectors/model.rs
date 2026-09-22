//! ModelSelector — picks a model from the catalog of builtin + custom models.
//!
//! The catalog comes from `nini_core::model_runtime::ModelRuntime::with_defaults`
//! which gives 10 builtin models. Items are tagged `is_current = true` for
//! the model name passed to `new()` so the selector can highlight the
//! active row.

use std::any::Any;

use nini_core::model_runtime::ModelRuntime;

use crate::selector::{SelectorItem, SelectorOutcome, SelectorState};

#[derive(Debug, Clone)]
pub struct ModelSelector {
    pub result: Option<String>,
    items: Vec<SelectorItem>,
    selected: usize,
}

impl ModelSelector {
    pub fn new(current: Option<&str>) -> Self {
        let runtime = ModelRuntime::with_defaults();
        let current_id = current.unwrap_or("").to_string();
        let items: Vec<SelectorItem> = runtime
            .list()
            .into_iter()
            .map(|m| SelectorItem {
                id: m.id.clone(),
                label: m.id.clone(),
                description: Some(format!("{} • context: {}k", m.provider, m.context_window / 1024)),
                is_current: m.id == current_id,
            })
            .collect();
        // Select the current model by default.
        let selected = items
            .iter()
            .position(|i| i.is_current)
            .unwrap_or(0);
        Self {
            result: None,
            items,
            selected,
        }
    }
}

impl SelectorState for ModelSelector {
    fn as_any_mut(&mut self) -> &mut dyn Any { self }
    fn state_items(&self) -> Vec<SelectorItem> { self.items.clone() }
    fn state_title(&self) -> String { "Switch model".to_string() }
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
