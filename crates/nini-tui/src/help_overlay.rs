//! Full-screen `/help` overlay.
//!
//! v0.5 had no help overlay; `/help` typed into the agent got the Chinese
//! "你好" greeting. v0.6 ships a real full-screen panel listing every
//! registered slash command with its description and argument hint.
//!
//! Implemented as a thin wrapper over the existing `SelectorPanel`
//! widget so we get fuzzy-filter, scroll, and theme integration for
//! free. The user can fuzzy-filter by typing, then press Esc to close
//! without picking anything.

use std::any::Any;

use crate::commands::{REGISTRY, by_name};
use crate::selector::{SelectorItem, SelectorOutcome, SelectorState};
use crate::theme::Theme;

/// State backing the `/help` overlay.
#[derive(Debug)]
pub struct HelpSelector {
    /// Pre-built snapshot of the registered commands (set on `new`).
    items: Vec<SelectorItem>,
    /// Currently selected index in `items`.
    selected: usize,
    /// Fuzzy-filter prefix typed by the user; filtering is recomputed
    /// by the runtime before each render.
    query: String,
}

impl HelpSelector {
    pub fn new() -> Self {
        let items = REGISTRY
            .iter()
            .map(|def| SelectorItem {
                id: def.name.to_string(),
                label: format!("/{}", def.name),
                description: Some(def.description.to_string()),
                is_current: false,
            })
            .collect();
        Self {
            items,
            selected: 0,
            query: String::new(),
        }
    }

    /// Look up the long-form help text for the currently-selected command.
    /// Returns the command name, its description, and (if any) the
    /// argument hint. Used by the renderer's right-pane view.
    pub fn current_detail(&self) -> Option<(String, String, Option<&'static str>)> {
        let item = self.items.get(self.selected)?;
        by_name(&item.id).map(|def| {
            (
                format!("/{}", def.name),
                def.description.to_string(),
                def.argument_hint,
            )
        })
    }
}

impl Default for HelpSelector {
    fn default() -> Self {
        Self::new()
    }
}

impl SelectorState for HelpSelector {
    fn state_items(&self) -> Vec<SelectorItem> {
        self.items.clone()
    }

    fn state_title(&self) -> String {
        "Help — type to filter slash commands".to_string()
    }

    fn state_selected(&self) -> usize {
        self.selected
    }

    fn state_set_selected(&mut self, idx: usize) {
        if idx < self.items.len() {
            self.selected = idx;
        }
    }

    fn state_query(&self) -> String {
        self.query.clone()
    }

    fn state_set_query(&mut self, q: String) {
        self.query = q;
    }

    fn state_on_select(&mut self) -> SelectorOutcome {
        // /help is informational — pressing Enter on a command just
        // keeps the panel open. Esc closes it (handled by the runtime
        // via SelectorOutcome::Cancelled).
        SelectorOutcome::Back
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Render the right-pane detail for the currently-highlighted command.
/// Pulled out so the runtime can render this alongside the selector list
/// if there's room on wide terminals.
pub fn render_detail_lines(theme: &Theme, detail: &(String, String, Option<&str>)) -> Vec<ratatui::text::Line<'static>> {
    use ratatui::style::Modifier;
    use ratatui::text::{Line, Span};
    let (name, description, hint) = detail;
    let mut out = Vec::new();
    out.push(Line::from(Span::styled(
        name.clone(),
        theme
            .fg_style("accent")
            .add_modifier(Modifier::BOLD),
    )));
    out.push(Line::from(Span::styled(
        description.clone(),
        theme.fg_style("text"),
    )));
    if let Some(h) = hint {
        out.push(Line::from(Span::styled(
            format!("arguments: {h}"),
            theme.fg_style("dim"),
        )));
    }
    out.push(Line::from(""));
    out.push(Line::from(Span::styled(
        "Esc to close, ↑↓ to navigate, type to filter.".to_string(),
        theme.fg_style("muted"),
    )));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_lists_all_28_commands() {
        let sel = HelpSelector::new();
        assert_eq!(
            sel.items.len(),
            REGISTRY.len(),
            "expected {} commands in help, got {}",
            REGISTRY.len(),
            sel.items.len()
        );
        // First command in registry is /settings.
        assert_eq!(sel.items[0].label, "/settings");
    }

    #[test]
    fn help_detail_returns_full_command_info() {
        let mut sel = HelpSelector::new();
        sel.state_set_selected(1); // /model
        let (name, _desc, hint) = sel.current_detail().expect("must have detail");
        assert_eq!(name, "/model");
        assert_eq!(hint, Some("<provider/model>"));
    }

    #[test]
    fn help_select_returns_back() {
        let mut sel = HelpSelector::new();
        // /help is informational; Enter should not "pick" anything.
        assert_eq!(sel.state_on_select(), SelectorOutcome::Back);
    }
}