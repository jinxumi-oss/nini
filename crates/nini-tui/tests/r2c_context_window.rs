//! R2c: the automatic compaction path must be reachable in a real session.
//!
//! It was not. `runtime::run` builds `AppState::new(initial_model)`
//! (`runtime.rs:364`), which leaves `run_state.context_window` at 0;
//! `set_status_bar_usage` has no caller in the crate; and
//! `should_auto_compact` opens with
//!
//! ```text
//! if self.run_state.context_window == 0 { return false }
//! ```
//!
//! So the whole automatic path that R2a and A2 built - compaction recorded
//! through the session, context reconstructed, transcript rebuilt - never
//! fired once in a real session. Every test of it set the window by hand,
//! which is precisely how the gap stayed hidden.
//!
//! **The trap these tests exist to avoid:** writing them the obvious way -
//! `AppState::with_context_window(...)` or setting the field directly -
//! would pass against the broken code and prove nothing. They build state
//! the way the runtime does and then assert the derived value.

use nini_tui::settings::Settings;
use nini_tui::state::AppState;

/// Build state the way `runtime::run` does.
///
/// This calls `runtime::initial_state` - the same function `run` calls -
/// rather than reproducing the steps. An earlier version spelled the steps
/// out here, which meant an injection that deleted the call to
/// `refresh_context_window` from `run` still passed 5/5: the test was
/// verifying a method nobody was calling.
fn production_shaped_state() -> AppState {
    nini_tui::runtime::initial_state("anthropic/claude-sonnet-4-5", |s| {
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
    assert_eq!(
        s.run_state.context_window,
        Settings::default().context_window,
        "it should come from settings, which default to Pi's 200_000"
    );
}

/// The headline. With the window derived, the trigger can fire - and the
/// rest of the pipeline is what R2a/A2 built, so this is the link that was
/// missing.
#[test]
fn auto_compaction_can_now_fire() {
    let mut s = production_shaped_state();
    assert!(
        !s.should_auto_compact(&Settings::default()),
        "precondition: a short transcript is under budget"
    );

    // Grow the transcript past the budget.
    let big = "x".repeat(4_000);
    for i in 0..200 {
        s.push_user(format!("message {i} {big}"));
    }

    assert!(
        s.should_auto_compact(&Settings::default()),
        "an over-budget transcript must trigger. Returning false here means \\
         the window is still not reaching the trigger, and the whole \\
         automatic compaction path is dead again."
    );
}

/// A zero window means "unknown", and must stay zero rather than becoming a
/// plausible-looking number. Inventing one triggers compaction at the wrong
/// time, which is worse than not triggering.
#[test]
fn an_unknown_window_stays_unknown() {
    let mut s = AppState::new("some/unknown-model");
    let mut settings = Settings::default();
    settings.context_window = 0;
    s.model_state.settings_snapshot = settings;
    s.refresh_context_window();

    assert_eq!(
        s.run_state.context_window, 0,
        "an unknown window must not be guessed at"
    );
    assert!(
        !s.should_auto_compact(&Settings::default()),
        "and an unknown window must not fire the trigger"
    );
}

/// Changing the model must not leave a stale window behind. Both model
/// changes in `runtime.rs` assigned `model_state.model` directly, so
/// anything derived from it went stale.
#[test]
fn changing_the_model_re_derives_the_window() {
    let mut s = production_shaped_state();

    let mut other = Settings::default();
    other.context_window = 32_000;
    s.model_state.settings_snapshot = other;
    s.set_model("some/other-model");

    assert_eq!(
        s.run_state.context_window, 32_000,
        "the window must follow the settings, not lag behind the previous \\
         model"
    );
    assert_eq!(s.model_state.model, "some/other-model");
}

/// The catalog's own `context_window` is deliberately not consulted: every
/// entry in it is 128_000, which is a guess rather than a measurement.
/// This pins the choice, so a later change to "prefer the catalog" is a
/// deliberate act rather than an accident.
#[test]
fn the_hardcoded_catalog_window_is_not_the_source() {
    let mut s = production_shaped_state();
    let mut settings = Settings::default();
    settings.context_window = 200_000;
    s.model_state.settings_snapshot = settings;
    s.refresh_context_window();

    assert_eq!(
        s.run_state.context_window, 200_000,
        "not the catalog's flat 128_000: a wrong number fires the trigger \
         at the wrong time"
    );

    let catalog = nini_core::model_runtime::ModelRuntime::with_defaults();
    let windows: std::collections::HashSet<u32> =
        catalog.all().iter().map(|m| m.context_window).collect();
    assert_eq!(
        windows.len(),
        1,
        "precondition for this decision: every catalog entry currently \
         shares one window ({:?}), so it carries no information",
        windows
    );
}
