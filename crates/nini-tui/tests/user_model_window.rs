//! Oracle: a model the user's `models.json` declares keeps **its own**
//! context window.
//!
//! # The gap
//!
//! `refresh_context_window` had exactly two sources: the shipped catalog
//! and `settings.context_window` (200 000, Pi's
//! `DEFAULT_COMPACTION_SETTINGS`). A model reached through the user's own
//! gateway is in neither — the catalog covers the providers nini
//! implements natively — so the window was a default nini had made up, and
//! automatic compaction fired at the wrong moment.
//!
//! # What is asserted here
//!
//! 1. the user's declared window wins for their model;
//! 2. the catalog still wins for a model the catalog knows (it is
//!    measured; the user's file may not mention it at all);
//! 3. a model nobody declared keeps the settings fallback — a window is
//!    never invented.

use nini_core::model_runtime::ModelSpec;
use nini_tui::state::AppState;

fn spec(id: &str, window: u32) -> ModelSpec {
    ModelSpec {
        id: id.to_string(),
        name: id.to_string(),
        api: "openai-completions".to_string(),
        provider: id.split('/').next().unwrap_or("gw").to_string(),
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

#[test]
fn the_users_own_window_is_used_for_their_model() {
    let mut state = AppState::new("MiniMax-M2.7-highspeed");
    state.model_state.user_models = vec![spec("aiio-failover/MiniMax-M2.7-highspeed", 204_800)];
    state.refresh_context_window();
    assert_eq!(
        state.run_state.context_window, 204_800,
        "the window the user wrote down, not nini's fallback"
    );
}

#[test]
fn a_model_nobody_declared_keeps_the_settings_fallback() {
    let mut state = AppState::new("some-unknown-model");
    state.refresh_context_window();
    assert_eq!(
        state.run_state.context_window, state.model_state.settings_snapshot.context_window,
        "an unknown model keeps Pi's default window; nothing is invented"
    );
}

#[test]
fn the_catalog_still_wins_for_a_model_it_knows() {
    let mut state = AppState::new("openai/gpt-5");
    // The user also declared a window for a *different* model; it must not
    // be applied to this one.
    state.model_state.user_models = vec![spec("aiio-failover/MiniMax-M2.7-highspeed", 204_800)];
    state.refresh_context_window();
    let catalog_window = nini_core::model_lookup::context_window_for("openai/gpt-5")
        .expect("the catalog knows openai/gpt-5");
    assert_eq!(state.run_state.context_window, catalog_window);
}
