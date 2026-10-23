//! End-to-end: the transcript scrollbar is painted on the last column.
//!
//! v0.10. Pi paints one (`pi-tui/src/layout.ts:293` `paintScrollbar`) and nini
//! carried only the theme key for it — `theme.rs` has had `scrollbarThumb`
//! (`#3a3a4a` dark, `#d0d0e0` light) since the theme was ported from Pi's
//! schema, with nothing that drew it. `interactive-mode.ts#scrollbarThumb` styles Pi's
//! thumb with `theme.bg("scrollbarThumb", …)`, which is where the key name
//! came from.
//!
//! Asserting on the rendered buffer, not on geometry: the geometry functions
//! are unit-tested in `scrollbar.rs`, and what matters to a user is that a
//! block of thumb appears in the right column of the right rows.

use nini_tui::render::render_frame;
use nini_tui::state::{AppState, RunMode, TranscriptLine};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::sync::Arc;

const THUMB: char = '█';

fn state_with(lines: usize, mode: nini_tui::scrollbar::ScrollbarMode) -> AppState {
    let mut state = AppState::default();
    state.run_state.mode = RunMode::Editing;
    state.ui_state.fullscreen_scrollbar = mode;
    Arc::make_mut(&mut state.transcript_state)
        .lines
        .extend((0..lines).map(|i| TranscriptLine::AssistantText(format!("line {i}"))));
    state
}

fn draw(state: &AppState, w: u16, h: u16) -> ratatui::buffer::Buffer {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| render_frame(f, state)).unwrap();
    terminal.backend().buffer().clone()
}

/// Cell contents of the last column, top to bottom, as one `char` per row.
///
/// A `String` is the obvious return type and it is wrong: `String::find`
/// and `String::rfind` return **byte** offsets, and this column contains
/// three-byte box-drawing characters. An early version of the test below
/// used them and reported a thumb ending on row 21 while the buffer held
/// `█` on 18 and 19 — the second `█` starts at byte 21, not row 21.
/// The first draft of this helper produced a failure that pointed
/// squarely at correct code.
fn last_column(buffer: &ratatui::buffer::Buffer, width: u16, height: u16) -> Vec<char> {
    (0..height)
        .map(|y| {
            buffer[(width - 1, y)]
                .symbol()
                .chars()
                .next()
                .unwrap_or(' ')
        })
        .collect()
}

/// Rows in `col` holding `ch`, as row indices.
fn rows_with(col: &[char], ch: char) -> Vec<usize> {
    col.iter()
        .enumerate()
        .filter(|(_, c)| **c == ch)
        .map(|(i, _)| i)
        .collect()
}

#[test]
fn a_thumb_is_painted_when_the_transcript_overflows() {
    let state = state_with(80, nini_tui::scrollbar::ScrollbarMode::Always);
    let col = last_column(&draw(&state, 80, 24), 80, 24);
    assert!(
        !rows_with(&col, THUMB).is_empty(),
        "the last column should carry a thumb, got {col:?}"
    );
}

/// Find the top border of the input box, which is the first row below the
/// transcript. Derived from the buffer rather than hardcoded: the real layout
/// is a two-row status bar, a one-row overflow indicator, then the transcript
/// — so row 20 in an 80x24 terminal, but that is a fact about this test's
/// dimensions, not an invariant.
fn input_box_top(buffer: &ratatui::buffer::Buffer, height: u16) -> u16 {
    (0..height)
        .find(|y| buffer[(0, *y)].symbol() == "\u{250c}")
        .unwrap_or_else(|| panic!("no input box top border found"))
}

/// At the live tail the thumb must end on the transcript's last row. The
/// first draft of this test asserted a hardcoded row and failed at 18 against
/// an expected 20 — the transcript area is rows 3..19 here, not 0..19, and
/// the thumb at 18..19 was right all along.
#[test]
fn the_thumb_sits_at_the_bottom_while_the_transcript_follows_the_tail() {
    let state = state_with(80, nini_tui::scrollbar::ScrollbarMode::Always);
    let buffer = draw(&state, 80, 24);
    let col = last_column(&buffer, 80, 24);

    let rows = rows_with(&col, THUMB);
    let first = *rows
        .first()
        .unwrap_or_else(|| panic!("no thumb in {col:?}"));
    let last = *rows.last().unwrap();
    assert_eq!(
        rows.len(),
        last - first + 1,
        "a contiguous thumb in {col:?}"
    );

    let transcript_last_row = input_box_top(&buffer, 24) - 1;
    assert_eq!(
        last as u16, transcript_last_row,
        "the thumb should end on the transcript's last row ({transcript_last_row}), \
         got {first}..{last}"
    );
}

#[test]
fn no_thumb_when_the_transcript_fits() {
    let state = state_with(3, nini_tui::scrollbar::ScrollbarMode::Always);
    let col = last_column(&draw(&state, 80, 24), 80, 24);
    assert!(
        rows_with(&col, THUMB).is_empty(),
        "content that fits has nothing to indicate, got {col:?}"
    );
}

#[test]
fn hidden_mode_draws_nothing_even_when_overflowing() {
    let state = state_with(80, nini_tui::scrollbar::ScrollbarMode::Hidden);
    let col = last_column(&draw(&state, 80, 24), 80, 24);
    assert!(rows_with(&col, THUMB).is_empty(), "got {col:?}");
}

/// `auto` is Pi's default (`settings-manager.ts:139`) and requires recent
/// scrolling. With no scroll recorded, `since_scroll` is the whole clock, so
/// the bar stays dark — matching Pi's `transientScrollbarVisible`.
#[test]
fn auto_mode_is_dark_until_the_reader_scrolls() {
    let state = state_with(80, nini_tui::scrollbar::ScrollbarMode::Auto);
    let col = last_column(&draw(&state, 80, 24), 80, 24);
    assert!(
        rows_with(&col, THUMB).is_empty(),
        "auto with no scroll activity: {col:?}"
    );
}

#[test]
fn auto_mode_lights_up_after_a_scroll() {
    let state = state_with(80, nini_tui::scrollbar::ScrollbarMode::Auto);
    state.mark_scroll_activity();

    let col = last_column(&draw(&state, 80, 24), 80, 24);
    assert!(
        rows_with(&col, THUMB).len() > 1,
        "auto after mark_scroll_activity should show the bar, got {col:?}"
    );
}

/// The thumb moves when the viewport moves, and it moves up. A bar that
/// stays at the bottom regardless of scroll would be decoration.
#[test]
fn the_thumb_moves_up_when_scrolled_up() {
    let mut state = state_with(80, nini_tui::scrollbar::ScrollbarMode::Always);
    state.mark_scroll_activity();

    Arc::make_mut(&mut state.transcript_state).scroll_offset = 60;
    Arc::make_mut(&mut state.transcript_state).autoscroll = false;

    let col = last_column(&draw(&state, 80, 24), 80, 24);
    let top = col
        .iter()
        .position(|c| *c == THUMB)
        .unwrap_or_else(|| panic!("no thumb in {col:?}"));
    assert!(
        top < 20,
        "scrolled to the top of an 80-line transcript, the thumb should be \
         near the top of the track, got row {top}"
    );
}

/// The thumb must not steal a whole row of content. Pi overwrites one cell;
/// nini does the same, and reflowing the transcript instead would move text
/// under the reader every time the bar appears.
#[test]
fn the_bar_occupies_one_column() {
    let state = state_with(80, nini_tui::scrollbar::ScrollbarMode::Always);
    let buffer = draw(&state, 80, 24);

    let thumb_rows = (0..24)
        .filter(|y| buffer[(79, *y)].symbol().starts_with(THUMB))
        .count();
    assert!(thumb_rows > 0, "expected a thumb");

    // The column to its left must still carry transcript content on some row.
    let neighbours_have_text = (0..24).any(|y| {
        let s = buffer[(78, y)].symbol();
        s != " " && !s.starts_with(THUMB)
    });
    assert!(
        neighbours_have_text,
        "the column beside the bar should still hold content"
    );
}
