//! Regression: the context bar must reflect the model's own usage numbers.
//!
//! **Reported symptom**: the status bar never showed a context percentage.
//!
//! **What was there**: every piece except the connection.
//!
//! - `AppState::set_status_bar_usage(cost, window, used)` — the writer
//! - `run_state.context_window` / `run_state.context_used` — the fields
//! - `render.rs:390-405` — `ctx 42% [████░░░░]`, Pi-style
//! - `AgentEventLite::Usage` — the per-turn numbers, arriving from
//!   `app.rs` on every `TurnEnd`
//!
//! **What was missing**: nothing ever called the writer. `context_used`
//! stayed at its initialiser for the life of the process, so the renderer
//! either drew `ctx 0%` or — because the segment is gated on
//! `context_window > 0` and that one *was* derived (`refresh_context_window`)
//! — drew a permanently empty bar next to a correct window. A user could
//! watch the window number and never learn anything from the bar.
//!
//! The other half: `Usage` carried `(input, output)` and dropped the cache
//! counters. Pi budgets with
//! `calculateContextTokens(usage) = totalTokens || input + output + cacheRead + cacheWrite`
//! (`core/compaction/compaction.ts:146`). On any provider with prompt
//! caching — which is every provider nini ships a catalog entry for — the
//! cached read is the majority of the context, so the number could not have
//! been right even once the wiring existed.
//!
//! These tests drive the sink the way the agent driver does, then read the
//! state the renderer reads.

use nini_tui::runtime::{AgentEventLite, AgentSink};
use std::sync::Arc;

fn sink_with_window(window: u32) -> (AgentSink, Arc<std::sync::Mutex<nini_tui::state::AppState>>) {
    // A model the catalog prices, so the cost assertion below is about the
    // usage-event change rather than about a free model.
    let state = Arc::new(std::sync::Mutex::new(nini_tui::state::AppState::new(
        "anthropic/claude-sonnet-4-5",
    )));
    {
        let mut g = state.lock().unwrap();
        g.run_state.context_window = window;
    }
    let (tx, _rx) = tokio::sync::watch::channel(false);
    (AgentSink::new(state.clone(), tx), state)
}

#[test]
fn a_turn_records_the_context_it_consumed() {
    let (sink, state) = sink_with_window(200_000);
    sink.push(AgentEventLite::Usage(12_000, 800, 40_000, 2_000));

    let g = state.lock().unwrap();
    assert_eq!(
        g.run_state.context_used,
        12_000 + 800 + 40_000 + 2_000,
        "context_used did not pick up the turn's usage — the bar is reading a \
         field nothing writes"
    );
}

/// A cached read counts toward the window. Without this the bar would report
/// the non-cached remainder of a long conversation and stay green right up
/// to the point where the next request is refused.
#[test]
fn cache_read_counts_toward_the_window() {
    let (sink, state) = sink_with_window(200_000);
    sink.push(AgentEventLite::Usage(1_000, 100, 150_000, 0));

    let g = state.lock().unwrap();
    assert_eq!(g.run_state.context_used, 151_100);
    let pct = g.run_state.context_used as f64 / g.run_state.context_window as f64 * 100.0;
    assert!(
        pct > 70.0,
        "a conversation that is 75% full reported {pct:.0}%"
    );
}

/// Context used is the *latest* turn, not a running total: the next request
/// carries the last context plus the answer to it. Summing turns would make
/// the bar hit 100% after a handful of exchanges.
#[test]
fn context_used_is_the_latest_turn_not_a_running_total() {
    let (sink, state) = sink_with_window(200_000);
    for _ in 0..10 {
        sink.push(AgentEventLite::Usage(10_000, 500, 0, 0));
    }

    let g = state.lock().unwrap();
    assert_eq!(
        g.run_state.context_used, 10_500,
        "ten turns of 10.5k each reported {} — the bar would saturate on a \
         short conversation",
        g.run_state.context_used
    );
    // The cumulative counters are a different thing and must still add up.
    assert_eq!(g.run_state.tokens.input, 100_000);
}

/// The bar is only rendered when a window is known; a zero window stays
/// silent rather than dividing by zero.
#[test]
fn a_zero_window_keeps_the_bar_dark() {
    let (sink, state) = sink_with_window(0);
    sink.push(AgentEventLite::Usage(10_000, 500, 0, 0));
    let g = state.lock().unwrap();
    assert_eq!(g.run_state.context_used, 10_500);
    assert_eq!(g.run_state.context_window, 0);
}

/// The cost path must survive the widened event.
#[test]
fn cost_still_accumulates_with_cache_fields_present() {
    let (sink, state) = sink_with_window(200_000);
    sink.push(AgentEventLite::Usage(1_000, 100, 50_000, 0));
    sink.push(AgentEventLite::Usage(1_000, 100, 50_000, 0));
    let g = state.lock().unwrap();
    assert!(
        g.run_state.cost_usd > 0.0,
        "cost stopped accruing when the usage event grew fields"
    );
}
