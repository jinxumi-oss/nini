//! ModelSelector — picks a model from the catalog of builtin + custom models.
//!
//! The catalog comes from `nini_core::model_runtime::ModelRuntime::with_defaults`
//! which gives 10 builtin models. Items are tagged `is_current = true` for
//! the model name passed to `new()` so the selector can highlight the
//! active row.
//!
//! # Search order (Pi parity)
//!
//! Pi's `/model` selector does **not** filter on the model id alone. It
//! filters on `getModelSelectorSearchText`
//! (`model-search.js#getModelSelectorSearchText`):
//!
//! ```text
//! `${provider} ${provider}/${id} ${provider} ${id}${name}`
//! ```
//!
//! and ranks the survivors with `fuzzyFilter` (`fuzzy.js#fuzzyFilter`),
//! whose score rewards word-boundary matches and consecutive characters
//! (see [`crate::selectors::pi_fuzzy`]).
//!
//! The provider-prefixed form comes **first** on purpose. Pi says so in
//! the function's own comment: the bare model id must not sit in the
//! leading position, so that an exact `provider/model` query outranks a
//! proxy provider whose id merely *contains* it — `openrouter/openai/gpt-5`
//! must not win a search for `openai/gpt-5`.
//!
//! nini's selector filtered on the raw id, so a search for
//! `openai/gpt-5` could only ever find the proxy id and never the model
//! the user actually meant. This module implements Pi's rule.

use std::any::Any;

use nini_core::model_runtime::{ModelRuntime, ModelSpec};

use crate::selector::{SelectorItem, SelectorOutcome, SelectorState};
use crate::selectors::pi_fuzzy;

/// Pi's search text for one model (`model-search.js#getModelSelectorSearchText`).
///
/// The provider-prefixed form leads; the bare id comes last.
pub fn model_selector_search_text(id: &str, provider: &str, name: &str) -> String {
    let name_part = if name.is_empty() {
        String::new()
    } else {
        format!(" {name}")
    };
    format!("{provider} {provider}/{id} {provider} {id}{name_part}")
}

#[derive(Debug, Clone)]
pub struct ModelSelector {
    pub result: Option<String>,
    /// The catalogue this selector was built from. Kept so the search can
    /// use Pi's search text (provider + id + name), not just the label.
    models: Vec<ModelSpec>,
    items: Vec<SelectorItem>,
    selected: usize,
}

impl ModelSelector {
    pub fn new(current: Option<&str>) -> Self {
        Self::from_specs(ModelRuntime::with_defaults().list(), current)
    }

    /// Build a selector over an explicit catalogue.
    pub fn from_specs(models: Vec<ModelSpec>, current: Option<&str>) -> Self {
        let current_id = current.unwrap_or("").to_string();
        let items: Vec<SelectorItem> = models
            .iter()
            .map(|m| SelectorItem {
                value: m.id.clone(),
                label: m.id.clone(),
                description: Some(format!(
                    "{} • context: {}k",
                    m.provider,
                    m.context_window / 1024
                )),
                // The current model may be named bare (`MiniMax-M2.7`)
                // while the catalogue qualifies it (`gateway/MiniMax-M2.7`).
                // The bare id is what the provider is asked for, so the
                // row that *is* the model in use is the one whose id ends
                // with it — otherwise the selector opens on a row that is
                // not what is running.
                is_current: m.id == current_id
                    || (!current_id.is_empty() && m.id.ends_with(&format!("/{current_id}"))),
            })
            .collect();
        // Select the current model by default.
        let selected = items.iter().position(|i| i.is_current).unwrap_or(0);
        Self {
            result: None,
            models,
            items,
            selected,
        }
    }

    /// The catalogue this selector offers.
    pub fn models(&self) -> &[ModelSpec] {
        &self.models
    }

    /// Pi's search text for every model in the catalogue, in catalogue order.
    ///
    /// Pi's `item.id` is the **bare** model id (`gpt-5`) with the provider
    /// beside it; nini's `ModelSpec::id` is qualified (`openai/gpt-5`).
    /// Handing the qualified id to Pi's template produced
    /// `openai openai/openai/gpt-5 …` — a text Pi never builds, and one
    /// whose scores are therefore not Pi's. The provider prefix is
    /// stripped here so the text is Pi's.
    pub fn search_texts(&self) -> Vec<String> {
        self.models
            .iter()
            .map(|m| {
                let prefix = format!("{}/", m.provider);
                let bare = m.id.strip_prefix(&prefix).unwrap_or(&m.id);
                model_selector_search_text(bare, &m.provider, &m.name)
            })
            .collect()
    }

    /// Rank the catalogue against `query`, best first.
    ///
    /// The returned indices point into [`ModelSelector::state_items`] (the
    /// catalogue order), and are ordered by Pi's fuzzy score.
    pub fn rank(&self, query: &str) -> Vec<usize> {
        pi_fuzzy::fuzzy_filter_indices(query, &self.search_texts())
    }
}

impl SelectorState for ModelSelector {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn state_items(&self) -> Vec<SelectorItem> {
        self.items.clone()
    }
    fn state_title(&self) -> String {
        "Switch model".to_string()
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

    /// Pi's search order, not nini's generic one.
    ///
    /// The generic filter (substring on the label, then a fuzzy fallback)
    /// cannot see the provider at all: it matches on `label`/`value`/
    /// `description`, and for a proxy model the id *is* the proxy id. So
    /// searching `openai/gpt-5` in nini found `openrouter/openai/gpt-5` and
    /// could not find `openai/gpt-5` — the exact inverse of what the user
    /// asked for. This override applies Pi's search text and Pi's scoring.
    fn state_filter(&self, query: &str) -> Vec<usize> {
        self.rank(query)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nini_core::model_runtime::ModelCost;

    fn spec(id: &str, provider: &str, name: &str) -> ModelSpec {
        ModelSpec {
            id: id.to_string(),
            name: name.to_string(),
            api: "openai-completions".to_string(),
            provider: provider.to_string(),
            reasoning: false,
            input: vec!["text".to_string()],
            context_window: 200_000,
            max_tokens: 8_000,
            cost: ModelCost::default(),
            supports_temperature: true,
            cost_tiers: Vec::new(),
            thinking_levels: Vec::new(),
        }
    }

    #[test]
    fn the_search_text_leads_with_the_provider() {
        let t = model_selector_search_text("gpt-5", "openai", "GPT-5");
        assert_eq!(t, "openai openai/gpt-5 openai gpt-5 GPT-5");
    }

    #[test]
    fn an_exact_provider_model_outranks_a_proxy_id() {
        let models = vec![
            spec("openai/gpt-5", "openrouter", "GPT-5 (OpenRouter)"),
            spec("gpt-5", "openai", "GPT-5"),
        ];
        let sel = ModelSelector::from_specs(models, None);
        let ranked = sel.rank("openai/gpt-5");
        assert_eq!(
            ranked.first().copied(),
            Some(1),
            "the model whose provider/id *is* the query must come first, got {ranked:?}"
        );
    }
}
