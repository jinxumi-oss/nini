//! Oracle: `Up` / `Down` follow Pi's ladder, and never destroy the input.
//!
//! **Reported class of bug (found by audit, not by a user report).**
//!
//! `apply_action` mapped both keys straight to `recall_history(±1)`:
//!
//! ```rust
//! KeyAction::MoveUp => {
//!     if state.ui_state.completion.is_some() { …select_up… }
//!     else { state.input.recall_history(-1); }
//! }
//! ```
//!
//! Pi (`editor.ts:833`) runs a three-step ladder instead:
//!
//! ```js
//! if (isOnFirstVisualLine() && (isEditorEmpty() || historyIndex > -1 || cursorCol === 0))
//!     navigateHistory(-1);          // only here is history reachable
//! else if (isOnFirstVisualLine())
//!     moveToLineStart();
//! else
//!     moveCursor(-1, 0);            // keep the column, move up a row
//! ```
//!
//! The asymmetry is the point: **history is the fallback, not the action.**
//! In nini it was the only action.
//!
//! It is a P0 rather than a papercut because nini's renderer draws the
//! input as several rows — `render_prompt` splits on `\n` and paints a
//! `❯` gutter per line — while `InputBuffer` has no row concept at all:
//! one byte offset, `move_to_start` / `move_to_end` that mean "whole
//! buffer", no way to ask which row the caret is on. The user *sees*
//! three rows, presses Up expecting the caret to rise, and the entire
//! half-written prompt is replaced by a previous message.
//!
//! **Test discipline (v0.10 §35.6)** — this file is the fourth to hit
//! this. An earlier draft called `InputBuffer::move_visual_up` and set
//! `ui_state.prompt_inner_width`, neither of which exists yet, so it could
//! not build against the unfixed code: a compile error is not an oracle.
//! Everything here goes through `runtime::apply_action`, whose signature
//! is identical on both sides, and the width is established the way the
//! product establishes it — by rendering a frame first.

use nini_tui::keys::{Key, KeyCode, KeyModifiers};
use nini_tui::render::render_frame;
use nini_tui::runtime::apply_action;
use nini_tui::state::AppState;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn up() -> Key {
    Key::new(KeyCode::Up, KeyModifiers::NONE)
}
fn down() -> Key {
    Key::new(KeyCode::Down, KeyModifiers::NONE)
}
fn ctrl_a() -> Key {
    Key::new(KeyCode::Char('a'), KeyModifiers::CTRL)
}

/// Render one frame so the runtime knows the prompt's width, then hand
/// the state back. `prompt_cursor_geometry` — the same visual-row maths
/// Pi calls `buildVisualLineMap` for — needs the width, and the only place
/// it exists is the renderer, exactly as in Pi (`this.lastWidth`).
fn sized(width: u16) -> AppState {
    let state = AppState::new("test-model");
    let backend = TestBackend::new(width, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_frame(f, &state)).unwrap();
    state
}

fn press(state: &mut AppState, key: Key) {
    apply_action(state, key);
}

/// A state with one submitted message in history and a three-row draft.
fn draft() -> AppState {
    let mut state = sized(100);
    state.input.insert_str("an earlier message");
    state.input.submit();
    state.input.insert_str("line one\nline two\nline three");
    state.input.cursor = state.input.text.len();
    state
}

#[test]
fn up_in_the_middle_of_a_multi_line_draft_moves_the_caret() {
    let mut state = draft();
    let before = state.input.text.clone();

    press(&mut state, up());

    assert_eq!(
        state.input.text, before,
        "Up must never edit the prompt text"
    );
    assert_ne!(
        state.input.cursor,
        state.input.text.len(),
        "the caret should have risen off the last row"
    );
    assert_eq!(
        &state.input.text[state.input.cursor..],
        "\nline three",
        "keeping the column, the caret lands on the row above"
    );
}

#[test]
fn up_twice_reaches_the_first_row_without_losing_the_text() {
    let mut state = draft();
    let before = state.input.text.clone();

    press(&mut state, up());
    press(&mut state, up());

    assert_eq!(state.input.text, before, "two rows up, still no edit");
}

#[test]
fn up_at_the_first_row_but_not_at_column_zero_does_not_browse_history() {
    let mut state = draft();
    // Walk to the first row with a non-zero column.
    press(&mut state, up());
    press(&mut state, up());
    press(&mut state, ctrl_a()); // line start → cursor 0
    assert_eq!(state.input.cursor, 0);

    state.input.cursor = 3; // column 3 of row 0
    let before = state.input.text.clone();

    press(&mut state, up());

    assert_eq!(
        state.input.text, before,
        "first row + column > 0 is Pi's `moveToLineStart()` branch, not \
         `navigateHistory(-1)`; the draft must survive."
    );
    assert_eq!(state.input.cursor, 0, "it should have gone to column 0");
}

#[test]
fn up_at_the_first_row_and_column_zero_still_recalls_history() {
    let mut state = draft();
    press(&mut state, up());
    press(&mut state, up());
    press(&mut state, ctrl_a());
    assert_eq!(state.input.cursor, 0);
    assert_eq!(
        &state.input.text[state.input.cursor..state.input.cursor + 4],
        "line"
    );

    press(&mut state, up());

    assert_eq!(
        state.input.text, "an earlier message",
        "first row + column 0 is exactly Pi's `cursorCol === 0` branch, and \
         it is the one place Up is *supposed* to replace the buffer."
    );
}

#[test]
fn down_from_a_middle_row_moves_the_caret_not_history() {
    let mut state = draft();
    press(&mut state, up()); // row 1
    press(&mut state, up()); // row 0
    press(&mut state, ctrl_a());
    state.input.cursor = 1; // row 0, column 1
    let before = state.input.text.clone();

    press(&mut state, down());

    assert_eq!(
        state.input.text, before,
        "Down must not pull history in when the caret is not browsing it; \
         that is Pi's `historyIndex > -1` guard."
    );
    assert_eq!(
        state.input.cursor, 10,
        "row 1 column 1 — offset 9 is the `\\n`, 10 is `l`"
    );
}

#[test]
fn down_onto_a_short_row_clamps_to_its_end() {
    // Pi's `moveCursor(1, 0)` clamps. Moving down from a long row into a
    // two-character row must land at its end, not past it.
    let mut state = sized(100);
    state.input.insert_str("long row here\nab");
    state.input.cursor = 2;

    press(&mut state, down());

    assert_eq!(
        state.input.cursor,
        state.input.text.len(),
        "clamped to the end of \"ab\""
    );
    assert_eq!(state.input.text, "long row here\nab");
}

#[test]
fn a_walk_down_and_back_up_through_every_row_keeps_the_text() {
    let mut state = draft();
    let before = state.input.text.clone();

    press(&mut state, up());
    press(&mut state, up());
    press(&mut state, down());
    press(&mut state, down());

    assert_eq!(
        state.input.text, before,
        "a round trip through the rows must never edit the prompt"
    );
    // The caret ends on the last row again — but **not** at its original
    // column. Column 10 cannot survive the trip up: row 0 is "line one",
    // eight columns wide, so the caret clamps to 8 there, and coming back
    // down it moves by the column it actually holds. That is Pi's
    // behaviour too — `moveCursor(-1, 0)` carries the current column and
    // clamps, and nothing remembers a "desired column" across rows.
    //
    // Asserted as (row, column) rather than a byte offset: two earlier
    // drafts of this assertion hand-computed the offset and got it wrong
    // twice, which is the same mistake the plan logs twice already
    // (§19.6, §20.7: "又一条我写错的期望值"). A test whose expected value
    // is derived by hand from a fixture is a test that will be "fixed"
    // to agree with whatever the code happens to do.
    let (row, col) = state.input.visual_position(
        state
            .prompt_inner_width
            .load(std::sync::atomic::Ordering::Relaxed),
    );
    assert_eq!(row, 2, "back on the last row");
    assert_eq!(
        col, 8,
        "carrying the column it was clamped to on the shortest row, not \
         the 10 it started with"
    );
}
