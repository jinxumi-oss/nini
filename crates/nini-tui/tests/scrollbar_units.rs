//! The scrollbar's unit is **rendered rows**, and the mouse must use the
//! same one the renderer used.
//!
//! v0.10.x. The renderer draws the scrollbar thumb from
//! `items_skipped_from_bottom` — how many *rendered rows* the viewport sits
//! above the live tail. The mouse hit-tested the same bar with
//! `transcript_state.scroll_offset`, which counts `TranscriptLine`s. The
//! two numbers coincide only when every line happens to be one row tall;
//! with a wrapped assistant message or a tall tool result they do not, and
//! a press on the visible thumb landed on a different thumb — so the bar
//! jumped under the pointer.
//!
//! The render path now publishes the number it used, and the mouse reads
//! it. These tests assert that the visible thumb and the hittable thumb are
//! the same thing, and that a drag lands where the row-based geometry says
//! it should.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use nini_tui::render::render_frame;
use nini_tui::runtime::{handle_mouse, lock_state, shared_state, SharedState};
use nini_tui::scrollbar::{self, ScrollbarMode};
use nini_tui::state::{AppState, RunMode, TranscriptLine};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::sync::Arc;

const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;
const THUMB: char = '█';

/// How many lines the transcript is scrolled up by.
const SCROLL_LINES: usize = 20;

/// A transcript whose lines are several rows tall, scrolled up by
/// [`SCROLL_LINES`] lines — and therefore by a *different* number of rows.
///
/// The messages are long enough to wrap, which is what makes the row count
/// and the line count differ. Without that, the two units coincide and
/// every assertion below would pass for the wrong reason.
fn primed() -> SharedState {
    let mut state = AppState::default();
    state.run_state.mode = RunMode::Editing;
    state.ui_state.fullscreen_scrollbar = ScrollbarMode::Always;
    let filler = "lorem ipsum dolor sit amet consectetur ".repeat(8);
    Arc::make_mut(&mut state.transcript_state)
        .lines
        .extend((0..40).map(|i| TranscriptLine::AssistantText(format!("{i}: {filler}"))));
    {
        let transcript = Arc::make_mut(&mut state.transcript_state);
        transcript.autoscroll = false;
        transcript.scroll_offset = SCROLL_LINES;
    }
    let shared = shared_state(state);
    // One render publishes the viewport, the per-line row counts, and the
    // row-based scroll offset the mouse now uses.
    draw(&shared);
    shared
}

fn draw(shared: &SharedState) {
    let state = lock_state(shared);
    let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).unwrap();
    terminal.draw(|f| render_frame(f, &state)).unwrap();
}

/// The rows of the last column that carry the thumb, top to bottom.
fn thumb_rows(shared: &SharedState) -> Vec<u16> {
    let state = lock_state(shared);
    let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).unwrap();
    terminal.draw(|f| render_frame(f, &state)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    (0..HEIGHT)
        .filter(|y| buffer[(WIDTH - 1, *y)].symbol().starts_with(THUMB))
        .collect()
}

fn mouse(kind: MouseEventKind, col: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column: col,
        row,
        modifiers: crossterm::event::KeyModifiers::NONE,
    }
}

fn offset(shared: &SharedState) -> usize {
    lock_state(shared).transcript_state.scroll_offset
}

/// Everything the two paths need, gathered under one lock.
struct Facts {
    viewport: scrollbar::Viewport,
    /// The row count the renderer published.
    skipped_items: usize,
    /// The line count the transcript is scrolled by.
    scroll_lines: usize,
    /// Per-line rendered row counts.
    heights: Vec<usize>,
    lines: usize,
}

fn facts(shared: &SharedState) -> Facts {
    let state = lock_state(shared);
    let viewport = *state.transcript_viewport.lock().unwrap();
    let heights = state.line_heights.lock().unwrap().clone();
    let facts = Facts {
        viewport,
        skipped_items: state.items_skipped_from_bottom(),
        scroll_lines: state.transcript_state.scroll_offset,
        heights,
        lines: state.transcript_state.lines.len(),
    };
    facts
}

#[test]
fn the_units_really_do_differ_in_this_transcript() {
    // A guard on the test itself: if the row count and the line count
    // coincided, every assertion below would pass for the wrong reason and
    // the unit mismatch would be invisible.
    let shared = primed();
    let f = facts(&shared);
    assert!(
        f.heights.iter().any(|h| *h > 1),
        "the transcript must contain multi-row lines, got {:?}",
        f.heights
    );
    assert_ne!(
        f.skipped_items, f.scroll_lines,
        "the renderer works in rows ({}) and the transcript scrolls in lines \
         ({}) — they must differ for this test to mean anything",
        f.skipped_items, f.scroll_lines
    );
}

/// The thumb the user can see and the thumb the mouse hits must be the same
/// thumb.
#[test]
fn a_press_on_the_visible_thumb_grabs_the_thumb() {
    let shared = primed();
    let f = facts(&shared);

    // The geometry the renderer used, and the one the old code used
    // (line count). They must be different, or the assertion below is
    // vacuous.
    let rendered = f.viewport.geometry(f.skipped_items).unwrap();
    let by_lines = f.viewport.geometry(f.scroll_lines).unwrap();
    assert_ne!(
        rendered.thumb_top, by_lines.thumb_top,
        "the row-based and line-based thumbs must differ here, or this test \
         cannot tell the two apart"
    );

    // And the rendered thumb really is where the geometry says it is.
    let rows = thumb_rows(&shared);
    assert_eq!(
        rows.first().copied(),
        Some(rendered.thumb_top),
        "the painted thumb must sit at the geometry's thumb_top: {rows:?} vs {rendered:?}"
    );

    // Pressing exactly on the visible thumb must grab it at its top row.
    let col = f.viewport.x + f.viewport.width - 1;
    handle_mouse(
        mouse(
            MouseEventKind::Down(MouseButton::Left),
            col,
            rendered.thumb_top,
        ),
        &shared,
    );
    let grab = lock_state(&shared)
        .ui_state
        .scrollbar_drag
        .expect("a press on the scrollbar column must start a drag")
        .grab_offset;
    assert_eq!(
        grab, 0,
        "the press was on the thumb's top row, so the grab offset must be 0; \
         got {grab} (the mouse hit-tested a different thumb)"
    );
}

/// A drag must land where the row-based geometry says it lands.
#[test]
fn a_drag_lands_where_the_row_geometry_says() {
    let shared = primed();
    let f = facts(&shared);
    let rendered = f.viewport.geometry(f.skipped_items).unwrap();
    let col = f.viewport.x + f.viewport.width - 1;

    // Grab the visible thumb, then drag it to the top of the track.
    handle_mouse(
        mouse(
            MouseEventKind::Down(MouseButton::Left),
            col,
            rendered.thumb_top,
        ),
        &shared,
    );
    let drag_row = f.viewport.y;
    handle_mouse(
        mouse(MouseEventKind::Drag(MouseButton::Left), col, drag_row),
        &shared,
    );

    // The expected offset, computed from the row-based geometry alone: the
    // press was on the thumb's top row (grab offset 0), the pointer is now
    // at the top of the track (thumb offset 0).
    let expected = scrollbar::line_offset_for_thumb_offset(&f.heights, 0, &rendered)
        .unwrap()
        .min(f.lines - 1);
    assert_eq!(
        offset(&shared),
        expected,
        "the drag must land on the row-derived offset"
    );

    // And the line-based reading would have landed somewhere else — that is
    // the defect this test exists for.
    let by_lines = f.viewport.geometry(f.scroll_lines).unwrap();
    let wrong_grab = rendered.thumb_top as i32 - by_lines.thumb_top as i32;
    let wrong_thumb_offset =
        (drag_row as i64 - f.viewport.y as i64 - wrong_grab as i64).max(0) as usize;
    let wrong = scrollbar::line_offset_for_thumb_offset(&f.heights, wrong_thumb_offset, &by_lines)
        .unwrap()
        .min(f.lines - 1);
    assert_ne!(
        expected, wrong,
        "the row-based and line-based readings must disagree here, or this \
         test cannot tell them apart"
    );
}
