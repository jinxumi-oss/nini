//! The scrollbar's unit is **rendered rows**, and the scroll offset is
//! expressed in the same unit.
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
//! **Unit change (worker C, this round).** The scroll offset itself counts
//! **rendered rows** now, so the scrollbar's unit and the scroll state's
//! unit are the same number and no conversion is needed anywhere.
//! 钉的是旧的转录行语义，本次因滚动单位改为渲染行而变。
//! The old guard ("the two units must differ here, or the test means
//! nothing") no longer holds — the units are identical by construction —
//! so it is replaced by the fixture guard that still matters: the
//! transcript must contain multi-row lines, or "rows" and "lines" would
//! coincide and every assertion would pass for the wrong reason.

//! invariant: 266bc74
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

/// How many **rendered rows** the transcript is scrolled up by.
///
/// 钉的是旧的转录行语义，本次因滚动单位改为渲染行而变：
/// 旧语义下这是 20 条转录行，新语义下是 20 个渲染行。
const SCROLL_ROWS: usize = 20;

/// A transcript whose lines are several rows tall, scrolled up by
/// [`SCROLL_ROWS`] rendered rows — and therefore by a *different* number
/// of transcript lines.
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
        transcript.scroll_offset = SCROLL_ROWS;
    }
    let shared = shared_state(state);
    // One render publishes the viewport and the per-line row counts.
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
    /// The scroll offset, in rendered rows.
    scroll_rows: usize,
    /// Per-line rendered row counts.
    heights: Vec<usize>,
}

fn facts(shared: &SharedState) -> Facts {
    let state = lock_state(shared);
    let viewport = *state.transcript_viewport.lock().unwrap();
    let heights = state.line_heights.lock().unwrap().clone();
    let facts = Facts {
        viewport,
        scroll_rows: state.transcript_state.scroll_offset,
        heights,
    };
    facts
}

#[test]
fn the_units_really_do_differ_in_this_transcript() {
    // A guard on the test itself: if every line were one row tall, the
    // row count and the line count would coincide and every assertion
    // below would pass for the wrong reason.
    //
    // 钉的是旧的转录行语义，本次因滚动单位改为渲染行而变：
    // 旧断言是「渲染行数必须与转录行数不同」（两个单位不同）；新语义下
    // 滚动位置与滚动条本来就是同一个单位（渲染行），该断言失去意义，
    // 改为守住夹具前提——转录里必须有多行行。
    let shared = primed();
    let f = facts(&shared);
    assert!(
        f.heights.iter().any(|h| *h > 1),
        "the transcript must contain multi-row lines, got {:?}",
        f.heights
    );
    assert!(
        f.scroll_rows > 0,
        "fixture precondition: the transcript is scrolled up by {} rendered \
         rows",
        f.scroll_rows
    );
}

/// The thumb the user can see and the thumb the mouse hits must be the same
/// thumb.
#[test]
fn a_press_on_the_visible_thumb_grabs_the_thumb() {
    // 钉的是旧的转录行语义，本次因滚动单位改为渲染行而变：
    // 旧语义下这里对比的是「按渲染行算的滑块」与「按转录行算的滑块」，
    // 新语义下二者是同一个数（滚动位置就是渲染行数），对比项消失，
    // 改为断言「画出来的滑块就是滚动位置算出来的滑块」。
    let shared = primed();
    let f = facts(&shared);

    // The geometry the renderer used — and, now, the geometry the mouse
    // hit-tests with, because both speak rendered rows.
    let rendered = f.viewport.geometry(f.scroll_rows).unwrap();

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
    // 钉的是旧的转录行语义，本次因滚动单位改为渲染行而变：
    // 旧语义下的落点由 `line_offset_for_thumb_offset`（渲染行 → 转录行）
    // 换算而来，新语义下拖拽的落点本身就是渲染行
    // （`scroll_rows_for_thumb_offset`），夹紧上限也从转录行数改为渲染行总数。
    let shared = primed();
    let f = facts(&shared);
    let rendered = f.viewport.geometry(f.scroll_rows).unwrap();
    let col = f.viewport.x + f.viewport.width - 1;
    let thumb_before = thumb_rows(&shared).first().copied();

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
    let expected = scrollbar::scroll_rows_for_thumb_offset(0, &rendered)
        .unwrap()
        .min(f.viewport.content_height.saturating_sub(1));
    assert_eq!(
        offset(&shared),
        expected,
        "the drag must land on the row-derived offset"
    );

    // And the thumb moved up with it: the drag went to the top of the
    // track, so the thumb must be above where it was. (The exact row is
    // not asserted: the `↑ N more above` indicator is painted over the
    // scrollbar's topmost cell, so the first painted thumb row is not
    // necessarily the geometry's `thumb_top`.)
    let thumb_after = thumb_rows(&shared).first().copied();
    assert!(
        thumb_after < thumb_before,
        "the thumb must move up when the viewport scrolls up: {thumb_before:?} \
         -> {thumb_after:?}"
    );
}
