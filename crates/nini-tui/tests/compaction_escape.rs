//! Oracle: Esc during a compaction cancels **the compaction**, not the turn.
//!
//! # The gap (A2b)
//!
//! Pi replaces the editor's escape handler while a compaction is running
//! (`interactive-mode.ts#autoCompactionEscapeHandler`):
//!
//! ```text
//! case "compaction_start":
//!     this.autoCompactionEscapeHandler = this.defaultEditor.onEscape;
//!     this.defaultEditor.onEscape = () => { this.session.abortCompaction(); };
//! ```
//!
//! and puts the old one back on `compaction_end`. So Esc during a
//! compaction cancels **the compaction** — the turn itself is not cut
//! short, and the user does not lose the work of the round.
//!
//! nini mapped `EscapeTarget::Compacting` onto `AbortTurn` — the whole
//! round was aborted for a keypress that was only meant to stop a
//! compaction.
//!
//! # And the bigger thing underneath (N5)
//!
//! While writing this oracle, the abort path turned out to have **no
//! consumer at all**: the TUI notified an `Arc<Notify>` and the driver
//! never read it — so "aborted" was a status string and the model kept
//! streaming. `Notify::notify_waiters()` also stores no permit, so a
//! signal triggered before anybody waits is lost for good.
//!
//! The oracle therefore pins **both** halves:
//!
//! 1. Esc during a compaction yields `AbortCompaction`, not `AbortTurn`;
//! 2. the compaction-abort signal is a *different* signal from the
//!    turn-abort one, and only the former is triggered;
//! 3. the turn-abort signal is triggered by a plain Esc in the `Agent`
//!    context — and it is an `AbortSignal` (flag first, then notify), so
//!    a waiter that arrives late still sees it;
//! 4. the other contexts (`Retrying`, `BranchSummary`) keep aborting the
//!    turn — A3's semantics must not regress.

use nini_core::abort_signal::AbortSignal;
use nini_tui::runtime::{escape_action, EscapeAction};
use nini_tui::state::{AppState, EscapeTarget, RunMode};
use std::sync::Arc;

fn state_in(target: EscapeTarget) -> AppState {
    let mut s = AppState::new("test-model");
    s.run_state.escape_target = target;
    s
}

#[test]
fn esc_during_compaction_cancels_the_compaction_not_the_turn() {
    let state = state_in(EscapeTarget::Compacting);
    assert_eq!(
        escape_action(&state),
        EscapeAction::AbortCompaction,
        "Pi: Esc during a compaction aborts the compaction, not the round"
    );
}

#[test]
fn the_other_contexts_still_abort_the_turn() {
    // A3's semantics must not regress: these three still cut the round
    // short.
    for target in [
        EscapeTarget::Agent,
        EscapeTarget::Retrying,
        EscapeTarget::BranchSummary,
    ] {
        let state = state_in(target);
        assert_eq!(
            escape_action(&state),
            EscapeAction::AbortTurn,
            "{target:?} must still abort the turn"
        );
    }
}

#[test]
fn idle_still_clears_the_input() {
    let mut state = state_in(EscapeTarget::Idle);
    state.run_state.mode = RunMode::Editing;
    assert_eq!(escape_action(&state), EscapeAction::ClearInput);
}

/// The two signals are separate objects: triggering one must not trigger
/// the other. That is the whole point of A2b — cancelling a compaction
/// must not abort the round.
#[test]
fn the_two_abort_signals_are_independent() {
    let turn = AbortSignal::new();
    let compaction = AbortSignal::new();
    compaction.trigger();
    assert!(
        compaction.is_triggered(),
        "the compaction signal must be triggered"
    );
    assert!(
        !turn.is_triggered(),
        "the turn signal must NOT be triggered by a compaction cancel — \
         that is the whole difference between A2b and the old behaviour"
    );
}

#[tokio::test]
async fn a_trigger_that_lands_before_the_wait_is_not_lost() {
    // `Notify::notify_waiters()` loses this race; `AbortSignal` does not.
    let sig = AbortSignal::new();
    sig.trigger();
    // Would hang forever with a bare `Notify`.
    sig.wait().await;
    assert!(sig.is_triggered());
}

#[tokio::test]
async fn a_waiter_that_arrives_late_still_sees_the_trigger() {
    let sig = AbortSignal::new();
    let s2 = sig.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        s2.trigger();
    });
    sig.wait().await;
    assert!(sig.is_triggered());
}

/// The state carries both signals, and they are per-turn.
#[test]
fn the_state_carries_both_signals() {
    let mut state = AppState::new("test-model");
    assert!(state.run_state.abort_signal.is_none());
    assert!(state.run_state.abort_compaction_signal.is_none());
    state.run_state.abort_signal = Some(Arc::new(AbortSignal::new()));
    state.run_state.abort_compaction_signal = Some(Arc::new(AbortSignal::new()));
    assert!(state.run_state.abort_signal.is_some());
    assert!(state.run_state.abort_compaction_signal.is_some());
}

/// The key handler must **trigger** the signal — that is the only thing
/// that ever reached the agent, and for a while it reached nothing at
/// all.
#[test]
fn the_key_handler_triggers_the_turn_abort_signal() {
    use nini_tui::keys::Key;
    let mut state = AppState::new("test-model");
    state.run_state.escape_target = EscapeTarget::Agent;
    state.run_state.mode = RunMode::Running;
    state.run_state.abort_signal = Some(Arc::new(AbortSignal::new()));
    state.run_state.abort_compaction_signal = Some(Arc::new(AbortSignal::new()));

    nini_tui::runtime::apply_action(&mut state, Key::esc());

    assert!(
        state
            .run_state
            .abort_signal
            .as_ref()
            .unwrap()
            .is_triggered(),
        "Esc in the Agent context must trigger the turn-abort signal"
    );
    assert!(
        !state
            .run_state
            .abort_compaction_signal
            .as_ref()
            .unwrap()
            .is_triggered(),
        "a plain Esc must not touch the compaction signal"
    );
}

/// And in the compaction context it must trigger the **other** one.
#[test]
fn the_key_handler_triggers_only_the_compaction_signal_while_compacting() {
    use nini_tui::keys::Key;
    let mut state = AppState::new("test-model");
    state.run_state.escape_target = EscapeTarget::Compacting;
    state.run_state.abort_signal = Some(Arc::new(AbortSignal::new()));
    state.run_state.abort_compaction_signal = Some(Arc::new(AbortSignal::new()));

    nini_tui::runtime::apply_action(&mut state, Key::esc());

    assert!(
        state
            .run_state
            .abort_compaction_signal
            .as_ref()
            .unwrap()
            .is_triggered(),
        "Esc while compacting must trigger the compaction signal"
    );
    assert!(
        !state
            .run_state
            .abort_signal
            .as_ref()
            .unwrap()
            .is_triggered(),
        "Esc while compacting must NOT abort the round — that was A2b"
    );
}
