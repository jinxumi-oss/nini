//! R2e: the cost on the status bar is a real number.
//!
//! `run_state.cost_usd` was structurally always zero. `app.rs` pushed
//! `AgentEventLite::Usage(input, output)` - a hardcoded third field -
//! and `render.rs` shows the cost pill only when `cost_usd > 0.0`. So the
//! field existed, was accumulated into, and could never be anything else.
//!
//! A number that is always zero is worse than an absent feature: it looks
//! instrumented.
//!
//! These tests drive the sink, which is where the pricing now lives. The
//! first version of this work tested `calculate_cost` in isolation and
//! passed an injection that reverted `app.rs` to its hardcoded zero - the
//! arithmetic was pinned and the wiring was not.

use nini_tui::runtime::{AgentEventLite, AgentSink};
use nini_tui::state::AppState;
use std::sync::Arc;

fn sink_for(tag: &str, model: &str) -> (Arc<std::sync::Mutex<AppState>>, AgentSink) {
    let dir = std::env::temp_dir().join(format!("nini-cost-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let state = Arc::new(std::sync::Mutex::new(AppState::new(model)));
    state.lock().unwrap().session_init(dir.join("s.jsonl"));
    let (tx, _rx) = tokio::sync::watch::channel(false);
    let sink = AgentSink::new(Arc::clone(&state), tx);
    (state, sink)
}

/// The headline. A turn on a model whose rates are known must move the
/// number off zero.
#[tokio::test]
async fn a_turn_against_a_priced_model_adds_cost() {
    let (state, sink) = sink_for("priced", "anthropic/claude-sonnet-4-5");

    // 1M in + 1M out on Sonnet: 3 + 15 = 18 USD at Pi's rates.
    sink.push(AgentEventLite::Usage(1_000_000, 1_000_000));

    let cost = state.lock().unwrap().run_state.cost_usd;
    assert!(
        cost > 0.0,
        "a priced model must produce a cost; it was hardcoded to 0.0 and \
         the pill in render.rs only shows when cost_usd > 0.0"
    );
    assert!(
        (cost - 18.0).abs() < 1e-6,
        "1M in + 1M out on Sonnet is 3 + 15 USD, got {cost}"
    );
}

/// Token accounting is unaffected by the pricing change.
#[tokio::test]
async fn tokens_still_accumulate() {
    let (state, sink) = sink_for("tokens", "anthropic/claude-sonnet-4-5");
    sink.push(AgentEventLite::Usage(100, 250));
    sink.push(AgentEventLite::Usage(7, 9));

    let g = state.lock().unwrap();
    assert_eq!(g.run_state.tokens.input, 107);
    assert_eq!(g.run_state.tokens.output, 259);
}

/// Cost accumulates across turns rather than being overwritten.
#[tokio::test]
async fn cost_accumulates_across_turns() {
    let (state, sink) = sink_for("accumulate", "anthropic/claude-sonnet-4-5");
    sink.push(AgentEventLite::Usage(1_000_000, 0));
    let after_one = state.lock().unwrap().run_state.cost_usd;
    sink.push(AgentEventLite::Usage(1_000_000, 0));
    let after_two = state.lock().unwrap().run_state.cost_usd;

    assert!((after_one - 3.0).abs() < 1e-6, "{after_one}");
    assert!(
        (after_two - 6.0).abs() < 1e-6,
        "a second turn adds, it does not replace: {after_two}"
    );
}

/// A model the catalog does not have has no price. That must read as
/// absent, not as a fabricated number - and `render.rs` hides the pill at
/// zero, so zero is how "unknown" is shown.
#[tokio::test]
async fn an_unpriced_model_contributes_nothing_rather_than_a_guess() {
    let (state, sink) = sink_for("unknown", "acme/never-heard-of-it");
    sink.push(AgentEventLite::Usage(1_000_000, 1_000_000));

    assert_eq!(
        state.lock().unwrap().run_state.cost_usd,
        0.0,
        "an unknown model must not acquire a price"
    );
    let tokens = state.lock().unwrap().run_state.tokens.input;
    assert_eq!(tokens, 1_000_000, "but the tokens are still counted");
}

/// A bare model id prices the same as a qualified one, because nini names
/// models both ways.
#[tokio::test]
async fn a_bare_model_id_is_priced_too() {
    let (state, sink) = sink_for("bare", "claude-sonnet-4-5");
    sink.push(AgentEventLite::Usage(1_000_000, 0));
    assert!((state.lock().unwrap().run_state.cost_usd - 3.0).abs() < 1e-6);
}
