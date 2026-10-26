//! Oracle: the `/model` selector searches and ranks the way Pi does.
//!
//! # The gap (B6)
//!
//! nini's selector filtered on the **label** (the model id) with a
//! generic substring-then-fuzzy rule, and it never saw the provider at
//! all. Pi does both differently:
//!
//! * it searches `provider provider/id provider id name`
//!   (`model-search.js#getModelSelectorSearchText`), and
//! * it ranks the survivors by fuzzy score (`fuzzy.js#fuzzyFilter`).
//!
//! The consequence was not a cosmetic ordering difference. A user typing
//! `openai/gpt-5` in nini found `openrouter/openai/gpt-5` — the proxy
//! whose id merely *contains* the query — and could not find
//! `openai/gpt-5` at all, because that id was not a prefix of the proxy's
//! id. Pi's provider-first search text is precisely the rule that puts
//! the model the user meant on top.
//!
//! # What is asserted here
//!
//! 1. the exact search text Pi builds;
//! 2. the ranking: the provider's own model beats a proxy whose id
//!    contains the query;
//! 3. the selector's filter is the one that does this — the generic
//!    filter is not reachable from the model selector any more;
//! 4. a model the user's `models.json` declares is in the catalogue and
//!    carries the user's own context window.

use nini_core::model_runtime::ModelSpec;
use nini_tui::selector::SelectorState;
use nini_tui::selectors::model::{model_selector_search_text, ModelSelector};

fn spec(id: &str, provider: &str, name: &str, window: u32) -> ModelSpec {
    ModelSpec {
        id: id.to_string(),
        name: name.to_string(),
        api: "openai-completions".to_string(),
        provider: provider.to_string(),
        reasoning: false,
        input: vec!["text".to_string()],
        context_window: window,
        max_tokens: 32_768,
        cost: nini_core::model_runtime::ModelCost::default(),
        supports_temperature: true,
        cost_tiers: Vec::new(),
        thinking_levels: Vec::new(),
    }
}

/// The catalogue the test searches: the real model, a proxy that contains
/// it, and a gateway model the user declared.
fn catalogue() -> Vec<ModelSpec> {
    vec![
        spec("openai/gpt-5", "openai", "GPT-5", 400_000),
        spec(
            "openrouter/openai/gpt-5",
            "openrouter",
            "GPT-5 (OpenRouter)",
            128_000,
        ),
        spec(
            "aiio-failover/MiniMax-M2.7-highspeed",
            "aiio-failover",
            "MiniMax M2.7 Highspeed",
            204_800,
        ),
    ]
}

#[test]
fn the_search_text_is_pis() {
    // Pi: `${provider} ${provider}/${id} ${provider} ${id}${name}` —
    // the provider-prefixed form leads, so an exact provider/model query
    // outranks a proxy whose id merely contains it.
    assert_eq!(
        model_selector_search_text("gpt-5", "openai", "GPT-5"),
        "openai openai/gpt-5 openai gpt-5 GPT-5"
    );
    // No name: the trailing space must not appear.
    assert_eq!(
        model_selector_search_text("gpt-5", "openai", ""),
        "openai openai/gpt-5 openai gpt-5"
    );
}

#[test]
fn the_catalogue_builds_pis_text_for_a_qualified_id() {
    // nini's ids are qualified (`openai/gpt-5`); Pi's `item.id` is bare
    // (`gpt-5`) with the provider beside it. The catalogue must hand Pi's
    // template a bare id, or the text is `openai openai/openai/gpt-5 …` —
    // a text Pi never builds, whose scores are not Pi's.
    let sel = ModelSelector::from_specs(catalogue(), None);
    let texts = sel.search_texts();
    assert_eq!(
        texts[0], "openai openai/gpt-5 openai gpt-5 GPT-5",
        "the search text of the first catalogue entry: {texts:?}"
    );
    assert_eq!(
        texts[1], "openrouter openrouter/openai/gpt-5 openrouter openai/gpt-5 GPT-5 (OpenRouter)",
        "the proxy's search text: {texts:?}"
    );
}

#[test]
fn the_real_model_outranks_the_proxy_that_contains_it() {
    let sel = ModelSelector::from_specs(catalogue(), Some("openai/gpt-5"));
    let ranked = sel.rank("openai/gpt-5");
    assert!(
        !ranked.is_empty(),
        "the query must find something: {ranked:?}"
    );
    let first = sel.state_items()[ranked[0]].value.clone();
    assert_eq!(
        first, "openai/gpt-5",
        "Pi's provider-first search text puts the real model first; got {ranked:?}"
    );
}

#[test]
fn a_model_is_findable_by_its_name_which_the_generic_filter_cannot_see() {
    let sel = ModelSelector::from_specs(catalogue(), None);
    // Pi's search text contains the model's **name**; the generic filter
    // only ever looks at label / value / description, so a model whose
    // display name is what the user remembers is invisible to it.
    let generic = nini_tui::selector::default_filter("minimax m2.7", &sel.state_items());
    assert!(
        generic.is_empty(),
        "precondition: the generic filter cannot see the name — {generic:?}"
    );

    // Pi's rule finds it.
    let filtered = sel.state_filter("minimax m2.7");
    assert!(!filtered.is_empty(), "Pi's rule must find it by name");
    let first = sel.state_items()[filtered[0]].value.clone();
    assert_eq!(first, "aiio-failover/MiniMax-M2.7-highspeed");
}

#[test]
fn a_user_declared_model_is_in_the_catalogue_with_its_own_window() {
    let sel = ModelSelector::from_specs(catalogue(), Some("MiniMax-M2.7-highspeed"));
    let items = sel.state_items();
    let row = items
        .iter()
        .find(|i| i.value == "aiio-failover/MiniMax-M2.7-highspeed")
        .expect("the model the user declared is in the catalogue");
    // The window is the user's own number (204 800 tokens = 200k), not a
    // default nini made up.
    let spec = sel
        .models()
        .iter()
        .find(|m| m.id == "aiio-failover/MiniMax-M2.7-highspeed")
        .expect("spec");
    assert_eq!(spec.context_window, 204_800);
    assert!(
        row.description.as_deref().unwrap_or("").contains("200k"),
        "the user's own window, not the fallback: {:?}",
        row.description
    );
    // The model in use is marked, even though it is named bare.
    assert!(
        row.is_current,
        "the bare id the provider is asked for must mark its catalogue row"
    );
}
