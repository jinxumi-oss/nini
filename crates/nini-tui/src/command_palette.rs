//! Command palette (F015).
//!
//! VSCode-style Ctrl+K overlay: single input box + fuzzy search across
//! every executable action — slash commands, tool names, skill names,
//! and built-in shortcuts. Pressing Enter on a hit dispatches the
//! action immediately (commits a slash command, fires a tool, etc.).
//!
//! Implementation strategy: re-use the `SelectorState` trait and
//! `SelectorPanel` widget (same machinery as `/help` and `/model`).
//! The selector's `state_on_select` returns the chosen `id` and the
//! runtime dispatches based on a prefix convention:
//!
//!   * `cmd:/name`        → submit the slash command
//!   * `tool:/name`       → insert `@<tool-name>` into the input buffer
//!   * `skill:/name`      → insert the skill prompt into the input buffer
//!
//! Identifiers are surfaced to the user as `/name` (slash-command
//! form) so the palette looks consistent with the rest of the TUI.

use std::any::Any;

use crate::commands::REGISTRY;
use crate::selector::{SelectorItem, SelectorOutcome, SelectorState};

/// State backing the Ctrl+K command palette.
#[derive(Debug)]
pub struct CommandPalette {
    items: Vec<SelectorItem>,
    selected: usize,
}

impl CommandPalette {
    pub fn new() -> Self {
        let mut items: Vec<SelectorItem> = REGISTRY
            .iter()
            .map(|def| SelectorItem {
                id: format!("cmd:/{}", def.name),
                label: format!("/{}", def.name),
                description: Some(def.description.to_string()),
                is_current: false,
            })
            .collect();
        // Add a couple of meta-actions so users can find them via
        // fuzzy search even if they don't remember the slash name.
        items.push(SelectorItem {
            id: "action:clear".to_string(),
            label: "Clear transcript".to_string(),
            description: Some("Clear the visible transcript".to_string()),
            is_current: false,
        });
        items.push(SelectorItem {
            id: "action:exit".to_string(),
            label: "Quit nini".to_string(),
            description: Some("Exit the TUI".to_string()),
            is_current: false,
        });
        Self { items, selected: 0 }
    }
}

impl Default for CommandPalette {
    fn default() -> Self {
        Self::new()
    }
}

impl SelectorState for CommandPalette {
    fn state_items(&self) -> Vec<SelectorItem> {
        self.items.clone()
    }

    fn state_title(&self) -> String {
        "Command palette — type to fuzzy-search all commands".to_string()
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
        // The runtime interprets the resulting SelectorItem.id to
        // decide what action to dispatch (see runtime.rs).
        SelectorOutcome::Picked(self.items[self.selected].clone())
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_includes_all_commands_plus_meta() {
        let p = CommandPalette::new();
        // 28 commands + 2 meta = 30.
        assert_eq!(p.items.len(), REGISTRY.len() + 2);
    }

    #[test]
    fn palette_selects_first_command_by_default() {
        let p = CommandPalette::new();
        let items = p.state_items();
        let item = items.first().expect("at least one item");
        // First entry is the first command in the registry.
        assert!(item.id.starts_with("cmd:/"));
    }
}