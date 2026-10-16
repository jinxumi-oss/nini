//! M0 — the input/interrupt gaps, pinned as assertions that fail today.
//!
//! Source: `docs/plans/input_interrupt_parity.md`. Pi's editor is a separate
//! component and `isStreaming` never decides whether it accepts typing
//! (`interactive-mode.js:2491`); nini gates every input path on
//! `mode == RunMode::Editing`, in thirteen places.
//!
//! Measured in tmux against a provider with a ten-second delay:
//!
//! ```text
//!   typing during a turn -> the input line stays empty
//!   Enter during a turn  -> nothing reaches the transcript
//! ```
//!
//! # Why this file names nothing new
//!
//! It writes only `RunState::pending_next_turn_messages`, `input.text`,
//! `transcript_state.lines` and `RunMode` — all of which exist today. An
//! earlier version of this file reached for a `pending_steering_messages`
//! field that the fix would introduce, and therefore **failed to compile**
//! against the parent commit. That is the third time this session a test
//! was written against an API the fix would add, after the same mistake in
//! §33 and §34. A test that cannot build is not an oracle.
//!
//! It is also why M1 reuses `pending_next_turn_messages` rather than
//! splitting the queue the way Pi does into `steering` and `followUp`:
//! one queue is what the current struct can express, and a test that can
//! only be written against fields that already exist is worth more than a
//! prettier queue.

use nini_tui::keys::Key;
use nini_tui::runtime::apply_action;
use nini_tui::state::{AppState, RunMode};

fn state_in(mode: RunMode) -> AppState {
    let mut state = AppState::new("test-model");
    state.run_state.mode = mode;
    state
}

fn type_text(state: &mut AppState, text: &str) {
    for c in text.chars() {
        apply_action(state, Key::char(c));
    }
}

fn transcript_text(state: &AppState) -> String {
    state
        .transcript_state
        .lines
        .iter()
        .map(|l| format!("{l:?}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Gap #1: a running turn must not lock the editor.
///
/// Pi lets you type while the model works; what happens to the text is
/// decided at submit time, not at keystroke time.
#[test]
fn typing_works_while_a_turn_is_running() {
    let mut state = state_in(RunMode::Running);
    type_text(&mut state, "hello");

    assert!(
        state.input.text.contains("hello"),
        "the editor rejected typing during a turn: {:?}",
        state.input.text
    );
}

/// Gap #2: submitting during a turn must queue, not vanish.
///
/// Pi: `await this.session.prompt(text, { streamingBehavior: "steer" })`
/// (`interactive-mode.js:2491`). nini had no path at all — the submit was
/// refused by a mode check and the text disappeared with the keystroke.
#[test]
fn submitting_while_running_queues_the_message() {
    let mut state = state_in(RunMode::Running);
    type_text(&mut state, "second question");
    apply_action(&mut state, Key::enter());

    let queued = state.run_state.pending_next_turn_messages.join(" ");
    assert!(
        queued.contains("second question"),
        "a message submitted during a turn must be queued, not dropped: {queued:?}"
    );
}

/// The queued message must be visible where the user will look for it, or
/// they cannot tell that something is pending.
#[test]
fn a_queued_message_is_visible_in_the_transcript() {
    let mut state = state_in(RunMode::Running);
    type_text(&mut state, "queued while running");
    apply_action(&mut state, Key::enter());

    let transcript = transcript_text(&state);
    assert!(
        transcript.contains("queued while running"),
        "the queued message left no trace in the transcript:\n{transcript}"
    );
}

/// Gap #3: an interrupt must hand the queue back, not discard it.
///
/// Pi: `restoreQueuedMessagesToEditor({ abort: true })` merges the queued
/// messages into the editor text, aborts, and returns — the text survives
/// the interrupt (`interactive-mode.js:3505`).
#[test]
fn an_interrupt_returns_queued_text_to_the_editor() {
    let mut state = state_in(RunMode::Editing);
    state
        .run_state
        .pending_next_turn_messages
        .push("first queued".into());
    state
        .run_state
        .pending_next_turn_messages
        .push("second queued".into());
    state.input.text.push_str("being typed");

    state.run_state.mode = RunMode::Running;
    apply_action(&mut state, Key::esc());

    let restored = &state.input.text;
    assert!(
        restored.contains("first queued") && restored.contains("second queued"),
        "an interrupt discarded what the user had queued: {restored:?}"
    );
    assert!(
        restored.contains("being typed"),
        "and it discarded what was half-typed in the editor: {restored:?}"
    );
    assert!(
        state.run_state.pending_next_turn_messages.is_empty(),
        "the queue is drained when it is handed back: {:?}",
        state.run_state.pending_next_turn_messages
    );
}

/// Aborting a turn must not become a clear — that is the other half of Pi's
/// `onEscape`, where clearing only happens when nothing is running.
#[test]
fn an_interrupt_with_an_empty_queue_keeps_the_draft() {
    let mut state = state_in(RunMode::Running);
    state.input.text.push_str("draft");
    apply_action(&mut state, Key::esc());

    assert_eq!(
        state.input.text, "draft",
        "Esc during a turn aborts the turn; it is not a clear"
    );
    assert!(state.run_state.pending_next_turn_messages.is_empty());
}
