//! TrustSelector — pick a trust level for the current working directory.
//!
//! The current decision is loaded from `~/.pi/agent/trust.json` (if it
//! exists) and tagged `is_current = true`. Levels: ask (always prompt),
//! trusted (auto-approve), never (block all).

use std::any::Any;

use nini_core::project_trust::{TrustDecision, TrustLevel};

use crate::selector::{SelectorItem, SelectorOutcome, SelectorState};

#[derive(Debug, Clone)]
pub struct TrustSelector {
    pub result: Option<TrustDecision>,
    pub cwd: String,
    items: Vec<SelectorItem>,
    selected: usize,
}

impl TrustSelector {
    pub fn new(cwd: String, current: Option<TrustDecision>) -> Self {
        let current_level = current.map(|d| d.level).unwrap_or(TrustLevel::Ask);
        let items = vec![
            ("ask", "Always prompt before tool calls", TrustLevel::Ask),
            (
                "trusted",
                "Auto-approve trusted tool calls",
                TrustLevel::Trusted,
            ),
            (
                "distrust",
                "Block all tool calls",
                TrustLevel::Distrusted,
            ),
        ];
        let items: Vec<SelectorItem> = items
            .into_iter()
            .map(|(id, desc, lvl)| SelectorItem {
                id: id.to_string(),
                label: id.to_string(),
                description: Some(desc.to_string()),
                is_current: lvl == current_level,
            })
            .collect();
        let selected = items
            .iter()
            .position(|i| i.is_current)
            .unwrap_or(0);
        Self {
            result: None,
            cwd,
            items,
            selected,
        }
    }
}

impl SelectorState for TrustSelector {
    fn as_any_mut(&mut self) -> &mut dyn Any { self }
    fn state_items(&self) -> Vec<SelectorItem> { self.items.clone() }
    fn state_title(&self) -> String { format!("Trust for {}", self.cwd) }
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
                let level = match it.id.as_str() {
                    "ask" => TrustLevel::Ask,
                    "trusted" => TrustLevel::Trusted,
                    "distrust" => TrustLevel::Distrusted,
                    _ => TrustLevel::Ask,
                };
                let decision = TrustDecision::new(level);
                self.result = Some(decision);
                SelectorOutcome::Picked(SelectorItem {
                    id: it.id,
                    label: it.label,
                    description: it.description,
                    is_current: it.is_current,
                })
            }
            None => SelectorOutcome::Cancelled,
        }
    }
}
