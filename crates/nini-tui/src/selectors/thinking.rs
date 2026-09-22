//! ThinkingSelector — picks a thinking level (off/minimal/low/medium/high).
//!
//! Levels match Pi's ThinkingLevel enum. The active level is passed to
//! `new()` and tagged `is_current = true`.

use std::any::Any;

use crate::selector::{SelectorItem, SelectorOutcome, SelectorState};

#[derive(Debug, Clone)]
pub struct ThinkingSelector {
    pub result: Option<String>,
    items: Vec<SelectorItem>,
    selected: usize,
}

impl ThinkingSelector {
    pub fn new(current: Option<&str>) -> Self {
        let levels = [
            ("off", "No extended thinking"),
            ("minimal", "Brief thinking budget"),
            ("low", "Light reasoning overhead"),
            ("medium", "Moderate reasoning"),
            ("high", "Maximum reasoning"),
        ];
        let current_id = current.unwrap_or("off").to_string();
        let items: Vec<SelectorItem> = levels
            .iter()
            .map(|(id, desc)| SelectorItem {
                id: id.to_string(),
                label: id.to_string(),
                description: Some(desc.to_string()),
                is_current: *id == current_id,
            })
            .collect();
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

impl SelectorState for ThinkingSelector {
    fn as_any_mut(&mut self) -> &mut dyn Any { self }
    fn state_items(&self) -> Vec<SelectorItem> { self.items.clone() }
    fn state_title(&self) -> String { "Thinking level".to_string() }
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
