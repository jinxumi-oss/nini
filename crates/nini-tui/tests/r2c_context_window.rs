//! R2c: the automatic compaction path must be reachable in a real session,
//! with the right window.
//!
//! It was not reachable. `runtime::run` builds `AppState::new(initial_model)`
//! (`runtime.rs:364`), which left `run_state.context_window` at 0;
//! `set_status_bar_usage` had no caller; and `should_auto_compact` opens
//! with `if self.run_state.context_window == 0 { return false }`. Every
//! test of the automatic path set the window by hand, which is how the gap
//! survived seven commits that assumed the path worked.
//!
//! **The trap these tests exist to avoid:** writing them the obvious way -
//! `with_context_window(...)`, or setting the field - would pass against
//! the broken code. They build state through `runtime::initial_state`,
//! which `run` itself calls, so the production sequence cannot drift.
//!
//! Updated in R2d: the window now comes from the catalog's measured
//! `contextWindow` for the selected model, falling back to settings. The
//! three assertions that changed are marked; the reasoning is in each.

use nini_tui::settings::Settings;
use nini_tui::state::AppState;

/// Build state the way `runtime::run` does.
fn production_shaped_state() -> AppState {
    nini_tui::runtime::initial_state("anthropic/claude-sonnet-4-5", |s| {
        s.model_state.settings_snapshot = Settings::default();
    })
}

/// State for a model with a small window, so a test-sized transcript can
/// actually cross the budget. `claude-sonnet-4-5` has a 1,000,000-token
/// window - correct, and useless for this purpose.
fn state_for_small_window_model() -> AppState {
    let small = nini_core::model_runtime::ModelRuntime::with_defaults()
        .all()
        .into_iter()
        .filter(|m| m.context_window <= 32_000 && m.context_window > 0)
        .map(|m| m.id)
        .next()
        .expect("the catalog has at least one small-window model");
    nini_tui::runtime::initial_state(&small, |s| {
        s.model_state.settings_snapshot = Settings::default();
    })
}

#[test]
fn a_production_shaped_state_has_a_usable_window() {
    let s = production_shaped_state();
    assert!(
        s.run_state.context_window > 0,
        "the window was 0, which is why auto-compaction never fired"
    );
}

#[test]
fn the_window_comes_from_the_catalog_for_a_model_it_knows() {
    // Changed in R2d. Previously this asserted the settings default
    // (200_000), because the catalog held a flat 128_000 and was deliberately
    // not consulted. The catalog is generated from Pi's own data now, so
    // its measured value wins: `claude-sonnet-4-5` is 1M in Pi, and a
    // settings default would have compacted it an order of magnitude early.
    let s = production_shaped_state();
    assert_eq!(
        s.run_state.context_window, 1_000_000,
        "Pi's own contextWindow for this model"
    );
    assert_ne!(
        s.run_state.context_window,
        Settings::default().context_window,
        "and it is deliberately not the settings default"
    );
}

#[test]
fn auto_compaction_can_now_fire() {
    let mut s = state_for_small_window_model();
    assert!(
        !s.should_auto_compact(&Settings::default()),
        "precondition: a short transcript is under budget"
    );

    let big = "x".repeat(4_000);
    for i in 0..200 {
        s.push_user(format!("message {i} {big}"));
    }

    assert!(
        s.should_auto_compact(&Settings::default()),
        "an over-budget transcript must trigger. Returning false here means \
         the window is not reaching the trigger, and the whole automatic \
         compaction path is dead again."
    );
}

#[test]
fn an_unknown_window_stays_unknown() {
    let mut s = AppState::new("some-model-nobody-has-heard-of");
    let mut settings = Settings::default();
    settings.context_window = 0;
    s.model_state.settings_snapshot = settings;
    s.refresh_context_window();

    assert_eq!(
        s.run_state.context_window, 0,
        "an unknown model must fall back to settings, and zero settings \
         must not be replaced with a guess"
    );
    assert!(!s.should_auto_compact(&Settings::default()));
}

/// Settings remain the fallback for a model the catalog does not have -
/// which is the case a user hits the moment they pass `--model` something
/// nini has never heard of.
#[test]
fn settings_still_answer_for_an_unknown_model() {
    let mut s = AppState::new("acme/brand-new-model");
    let mut settings = Settings::default();
    settings.context_window = 111_000;
    s.model_state.settings_snapshot = settings;
    s.refresh_context_window();

    assert_eq!(
        s.run_state.context_window, 111_000,
        "a model the catalog does not have must still get a window"
    );
}

#[test]
fn changing_the_model_re_derives_the_window() {
    let mut s = production_shaped_state();
    let before = s.run_state.context_window;

    // A model with a genuinely different window, so the assertion cannot
    // pass by the value happening to be unchanged.
    let other = nini_core::model_runtime::ModelRuntime::with_defaults()
        .all()
        .into_iter()
        .find(|m| m.context_window != before)
        .expect("the catalog has models with different windows");
    s.set_model(other.id.clone());

    assert_eq!(
        s.run_state.context_window, other.context_window,
        "the window must follow the model"
    );
    assert_ne!(s.run_state.context_window, before);
}

/// R2d replaced this. It asserted that the catalog was *not* consulted,
/// because every entry shared one number. It now checks the reason that
/// decision was made no longer applies.
#[test]
fn the_catalog_is_no_longer_one_number() {
    let rt = nini_core::model_runtime::ModelRuntime::with_defaults();
    let windows: std::collections::HashSet<u32> =
        rt.all().iter().map(|m| m.context_window).collect();
    assert!(
        windows.len() > 5,
        "the old flat 128_000 is gone; there are now {:?}",
        windows
    );
}
