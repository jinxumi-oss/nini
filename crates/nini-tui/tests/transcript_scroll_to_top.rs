//! Oracle: `Home` jumps to the top of the transcript, like Pi.
//!
//! **Reported class of bug (found by audit, not by a user report).**
//!
//! Pi binds `home` to `tui.altScreen.top` — "Scroll viewport to top"
//! (`pi-tui/src/keybindings.ts:208`) — and `end` to
//! `tui.altScreen.bottom` (`:209`). The binding matters more than it
//! looks, because `tui-alt-screen.ts:224` registers the viewport's
//! `handleViewportInput` as an input listener at construction, and
//! `tui.ts:336` dispatches through a **`Set`**, which iterates in
//! insertion order. The viewport is therefore asked first, and its
//! `scrollToTop()` returns `{ consume: true }` (`:634`) — so the
//! editor's own `cursorLineStart` binding, which lists `"home"` first
//! too, is **unreachable in the full-screen layout**.
//!
//! nini has the opposite wiring: `Home` was `MoveLineStart`, and
//! nothing anywhere in the workspace scrolled to the top —
//! `grep -rn ScrollToTop` returned nothing. A user who had scrolled back
//! through 400 lines to find something had exactly one way forward again,
//! `PageUp` repeated once per screenful, with `PageUp` counting
//! *TranscriptLines* against a viewport that may render each of them
//! across fifty rows.
//!
//! The fix is the Pi wiring, plus the binding nini was missing all
//! along: `Ctrl+E` for end-of-line (Pi's second key for
//! `cursorLineEnd`, `keybindings.ts:186`), since `Home` and `End` now
//! belong to the transcript.
//!
//! **Test discipline (v0.10 §35.6)**: driven through
//! `runtime::apply_action`, whose signature is identical on both sides,
//! and through the `Key` constructors that already exist. No API was
//! added to make this compile.

use nini_tui::keys::{Key, KeyCode, KeyModifiers};
use nini_tui::render::render_frame;
use nini_tui::runtime::apply_action;
use nini_tui::state::AppState;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn home() -> Key {
    Key::new(KeyCode::Home, KeyModifiers::NONE)
}
fn end() -> Key {
    Key::new(KeyCode::End, KeyModifiers::NONE)
}
fn ctrl_a() -> Key {
    Key::new(KeyCode::Char('a'), KeyModifiers::CTRL)
}
fn ctrl_e() -> Key {
    Key::new(KeyCode::Char('e'), KeyModifiers::CTRL)
}
fn page_up() -> Key {
    Key::new(KeyCode::PageUp, KeyModifiers::NONE)
}

/// A transcript of `n` numbered turns, rendered once so the renderer
/// publishes `page_lines` and the item cache the scroll maths uses.
fn transcript(n: usize, rows: u16) -> AppState {
    let mut state = AppState::new("test-model");
    for i in 0..n {
        state.push_user(format!("turn {i}"));
    }
    let mut terminal = Terminal::new(TestBackend::new(80, rows)).unwrap();
    terminal.draw(|f| render_frame(f, &state)).unwrap();
    state
}

#[test]
fn home_scrolls_to_the_top_of_the_transcript() {
    let mut state = transcript(200, 24);
    let total = state.transcript_state.lines.len();
    assert_eq!(total, 200, "fixture precondition");

    apply_action(&mut state, page_up()); // leave the tail
    assert!(
        state.transcript_state.scroll_offset > 0,
        "PageUp should have moved the viewport off the tail"
    );

    apply_action(&mut state, home());

    assert_eq!(
        state.transcript_state.scroll_offset,
        total - 1,
        "Home must put the first line at the top of the viewport. \
         `scroll_offset` counts TranscriptLines skipped from the bottom, so \
         the top is `total - 1`."
    );
    assert!(
        !state.transcript_state.autoscroll,
        "and autoscroll must be off, or the renderer takes the tail-aligned \
         branch and ignores scroll_offset entirely"
    );
}

#[test]
fn end_returns_to_the_live_tail() {
    let mut state = transcript(200, 24);
    apply_action(&mut state, home());
    assert!(state.transcript_state.scroll_offset > 0);

    apply_action(&mut state, end());

    assert_eq!(
        state.transcript_state.scroll_offset, 0,
        "End must snap back to the newest message"
    );
    assert!(
        state.transcript_state.autoscroll,
        "and re-enable autoscroll, so streaming keeps following the tail"
    );
}

#[test]
fn home_then_end_round_trips() {
    let mut state = transcript(120, 24);
    apply_action(&mut state, page_up());
    apply_action(&mut state, home());
    let at_top = state.transcript_state.scroll_offset;
    apply_action(&mut state, end());
    let at_bottom = state.transcript_state.scroll_offset;

    assert!(at_top > 0 && at_bottom == 0);
}

#[test]
fn ctrl_a_still_reaches_the_start_of_the_line() {
    // Pi's `cursorLineStart` is `["home", "ctrl+home", "ctrl+a"]`. Home
    // now belongs to the viewport, so `Ctrl+A` is the binding a user has
    // for the editor, and it has to keep working — otherwise the change
    // trades one broken key for a missing one.
    let mut state = AppState::new("test-model");
    state.input.insert_str("hello world");
    state.input.cursor = state.input.text.len();

    apply_action(&mut state, ctrl_a());

    assert_eq!(
        state.input.cursor, 0,
        "Ctrl+A must still move the caret to the start of the line"
    );
    assert_eq!(state.input.text, "hello world");
}

#[test]
fn ctrl_e_reaches_the_end_of_the_line() {
    // Pi's `cursorLineEnd` is `["end", "ctrl+end", "ctrl+e"]`; `end` now
    // belongs to the viewport and nini never had `Ctrl+E`. This is the
    // key that was missing, not merely moved.
    let mut state = AppState::new("test-model");
    state.input.insert_str("hello world");
    state.input.cursor = 0;

    apply_action(&mut state, ctrl_e());

    assert_eq!(
        state.input.cursor,
        state.input.text.len(),
        "Ctrl+E must move the caret to the end of the line"
    );
}

#[test]
fn home_scrolls_even_with_text_in_the_prompt() {
    // The point of the change: Home is a transcript action, so it must not
    // be swallowed by the editor just because there is something to edit.
    let mut state = transcript(200, 24);
    state.input.insert_str("half-typed prompt");
    state.input.cursor = 5;

    apply_action(&mut state, home());

    assert_eq!(
        state.input.cursor, 5,
        "the caret must not move — Home is not an editor key any more"
    );
    assert_eq!(
        state.transcript_state.scroll_offset,
        state.transcript_state.lines.len() - 1,
        "and the transcript must be at the top"
    );
}
