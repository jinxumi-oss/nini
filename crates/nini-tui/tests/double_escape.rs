//! Oracle: the double-Escape opens the tree selector — Pi's rule, exactly.
//!
//! # The gap (B5)
//!
//! Pi's escape handler (`interactive-mode.ts#setupKeyHandlers`):
//!
//! ```text
//! } else if (!this.editor.getText().trim()) {
//!     // Double-escape with empty editor triggers /tree, /fork, or nothing based on setting
//!     const action = this.settingsManager.getDoubleEscapeAction();
//!     if (action !== "none") {
//!         const now = Date.now();
//!         if (now - this.lastEscapeTime < 500) {
//!             if (action === "tree") { this.showTreeSelector(); }
//!             else { this.showUserMessageSelector(); }
//!             this.lastEscapeTime = 0;
//!         } else { this.lastEscapeTime = now; }
//!     }
//! }
//! ```
//!
//! Three facts, all of which the oracle pins:
//!
//! 1. it only counts when the **editor is empty**;
//! 2. the window is **500 ms of wall clock**;
//! 3. the action comes from the settings (`"none" | "tree" | "fork"`,
//!    default `"tree"`).
//!
//! # Why the timestamp is an argument
//!
//! The window is wall-clock (Pi uses `Date.now()`), but the *decision*
//! takes the timestamp as a parameter. A test can then say "two presses
//! 100 ms apart" without sleeping — and CI does not get a 500 ms sleep
//! that is flaky under load.

use nini_tui::runtime::{double_escape_action, DoubleEscapeOutcome, DOUBLE_ESCAPE_WINDOW_MS};
use nini_tui::state::AppState;

fn state_with_action(action: &str) -> AppState {
    let mut s = AppState::new("test-model");
    s.model_state.settings_snapshot.double_escape_action = action.to_string();
    s
}

#[test]
fn two_escapes_within_the_window_open_the_tree_selector() {
    let mut s = state_with_action("tree");
    // First press: nothing yet, only the timestamp is recorded.
    assert_eq!(
        double_escape_action(&mut s, 1_000),
        DoubleEscapeOutcome::None
    );
    // Second press 100 ms later: the tree opens.
    assert_eq!(
        double_escape_action(&mut s, 1_100),
        DoubleEscapeOutcome::Tree
    );
    assert_eq!(
        s.run_state.status, "open_selector:tree",
        "the runtime opens a selector by setting this flag — that is the \
         mechanism it watches (see runtime.rs's `open_selector:` arm)"
    );
}

#[test]
fn the_window_is_five_hundred_milliseconds() {
    assert_eq!(DOUBLE_ESCAPE_WINDOW_MS, 500, "Pi's window");
    let mut s = state_with_action("tree");
    assert_eq!(double_escape_action(&mut s, 0), DoubleEscapeOutcome::None);
    // 499 ms later: still inside.
    assert_eq!(
        double_escape_action(&mut s, DOUBLE_ESCAPE_WINDOW_MS - 1),
        DoubleEscapeOutcome::Tree
    );

    let mut s = state_with_action("tree");
    assert_eq!(double_escape_action(&mut s, 0), DoubleEscapeOutcome::None);
    // Exactly 500 ms later: outside (Pi's check is `< 500`).
    assert_eq!(
        double_escape_action(&mut s, DOUBLE_ESCAPE_WINDOW_MS),
        DoubleEscapeOutcome::None
    );
}

#[test]
fn a_later_press_only_records_the_timestamp_again() {
    let mut s = state_with_action("tree");
    assert_eq!(double_escape_action(&mut s, 0), DoubleEscapeOutcome::None);
    // Too late: this press starts a new window.
    assert_eq!(
        double_escape_action(&mut s, 10_000),
        DoubleEscapeOutcome::None
    );
    // And the *next* press inside the new window fires.
    assert_eq!(
        double_escape_action(&mut s, 10_100),
        DoubleEscapeOutcome::Tree
    );
}

#[test]
fn the_action_none_does_nothing_at_all() {
    let mut s = state_with_action("none");
    assert_eq!(double_escape_action(&mut s, 0), DoubleEscapeOutcome::None);
    assert_eq!(double_escape_action(&mut s, 100), DoubleEscapeOutcome::None);
    assert_ne!(
        s.run_state.status, "open_selector:tree",
        "\"none\" must not open anything"
    );
}

#[test]
fn the_fork_action_says_it_is_not_implemented() {
    // Pi's "fork" opens `showUserMessageSelector()` — a picker of previous
    // user messages. nini has no counterpart, so it says so rather than
    // opening the tree selector and pretending.
    let mut s = state_with_action("fork");
    assert_eq!(double_escape_action(&mut s, 0), DoubleEscapeOutcome::None);
    assert_eq!(
        double_escape_action(&mut s, 100),
        DoubleEscapeOutcome::ForkUnsupported
    );
    assert_ne!(
        s.run_state.status, "open_selector:tree",
        "the fork action must not open the tree selector"
    );
}

#[test]
fn the_editor_must_be_empty_for_the_press_to_count() {
    // Pi counts only when the editor is empty. The decision function is
    // called from the key handler under exactly that condition; this test
    // pins the condition itself (the handler's job): a **non-empty**
    // editor clears the input and the tree must not open.
    use nini_tui::keys::Key;
    let mut s = state_with_action("tree");
    s.input.insert_str("draft");
    nini_tui::runtime::apply_action(&mut s, Key::esc());
    nini_tui::runtime::apply_action(&mut s, Key::esc());
    assert_eq!(s.input.text, "", "a non-empty editor is cleared by Esc");
    assert_ne!(
        s.run_state.status, "open_selector:tree",
        "a non-empty editor must not trigger the double-Escape"
    );
    // The **second** press now sees an empty editor (the first one cleared
    // it), so *that* press is the one that starts the window — the tree
    // still does not open (it takes two empty-editor presses).
    assert!(
        s.run_state.last_escape_ms.is_some(),
        "the press that found an empty editor starts the window"
    );
    assert_ne!(s.run_state.status, "open_selector:tree");
}

#[test]
fn two_escapes_with_an_empty_editor_open_the_tree_from_the_key_path() {
    // The same thing through the real dispatch point (not just the
    // decision function).
    use nini_tui::keys::Key;
    let mut s = state_with_action("tree");
    nini_tui::runtime::apply_action(&mut s, Key::esc());
    assert_ne!(
        s.run_state.status, "open_selector:tree",
        "the first press must not open anything"
    );
    nini_tui::runtime::apply_action(&mut s, Key::esc());
    assert_eq!(
        s.run_state.status, "open_selector:tree",
        "the second press (well inside 500 ms) opens the tree selector"
    );
}
