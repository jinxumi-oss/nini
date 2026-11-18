//! REGRESSION: the transcript scroll unit is the **rendered row**.
//!
//! **The bug this file exists for.** `scroll_offset` used to count
//! *transcript lines* (`TranscriptLine`), while the renderer and the
//! scrollbar worked in *rendered rows*. One assistant message of 200
//! lines is one transcript line and 200 rendered rows, so scrolling
//! could never stop inside it: the whole message was skipped as one
//! unit, and the renderer only ever painted the message's **tail**
//! (`render.rs`: `start = line_items.len() - take`). The middle of a
//! long message was therefore never on screen — the "翻页时中间内容消失"
//! report.
//!
//! The unit is the rendered row now (`state.transcript_state.scroll_offset`
//! counts rendered rows skipped from the bottom), and a page is one screen
//! of rendered rows minus Pi's four-row overlap
//! (`PAGE_SCROLL_OVERLAP = 4`, pi 0.85.1 `tui-alt-screen.ts:74`), so
//! consecutive pages overlap and a long message can be read screen by
//! screen.
//!
//! **Test discipline.** Everything here goes through public API only:
//! `runtime::apply_action` (the same entry point the keyboard uses),
//! `runtime::handle_mouse` (the same entry point the wheel uses) and
//! `render::render_frame`. The assertions read `scroll_offset` and the
//! rendered frame; nothing reaches into a private helper.
//!
//! **钉的是旧的转录行语义，本次因滚动单位改为渲染行而变。**

use crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};
use nini_tui::keys::{Key, KeyCode, KeyModifiers as KeyMods};
use nini_tui::render::render_frame;
use nini_tui::runtime::{apply_action, handle_mouse, lock_state, shared_state, SharedState};
use nini_tui::state::{AppState, TranscriptLine};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::sync::Arc;

const W: u16 = 80;
const H: u16 = 24;

/// Pi's page overlap: a page keeps four rows of the previous page on
/// screen (`references/pi-0.85.1/src/tui/tui-alt-screen.ts:74`).
const PAGE_OVERLAP: usize = 4;

/// How many paragraphs the long message has. Each paragraph is one
/// rendered row (they are separated by a blank line, so markdown does not
/// fold them into one paragraph).
const LONG_SEGMENTS: usize = 200;

/// How many one-row turns sit at the live tail, after the long message.
const TAIL_LINES: usize = 5;

/// The first and last row indices of the long message's *middle* — the
/// band that the old, line-based unit could never put on screen.
const MIDDLE_FROM: usize = 60;
const MIDDLE_TO: usize = 140;

fn page_up() -> Key {
    Key::new(KeyCode::PageUp, KeyMods::NONE)
}

fn wheel_up(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

/// Render one frame and return its text.
fn frame_text(state: &AppState) -> String {
    let backend = TestBackend::new(W, H);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_frame(f, state)).unwrap();
    let buf = terminal.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "))
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn draw_shared(shared: &SharedState) {
    let state = lock_state(shared);
    let mut terminal = Terminal::new(TestBackend::new(W, H)).unwrap();
    terminal.draw(|f| render_frame(f, &state)).unwrap();
}

/// One transcript line that renders as hundreds of rows, followed by
/// [`TAIL_LINES`] one-row turns at the live tail.
///
/// The long line is the *oldest* content, so scrolling up walks into it.
///
/// The paragraphs are separated by a blank line on purpose: without it
/// markdown folds them into one paragraph, the whole message becomes one
/// very long line, and it renders as ~20 wrapped rows instead of hundreds
/// — a fixture that looks long but is not.
fn long_line_transcript() -> AppState {
    let mut state = AppState::new("test-model");
    let body = (0..LONG_SEGMENTS)
        .map(|i| format!("seg-{i:03}"))
        .collect::<Vec<_>>()
        .join("\n\n");
    Arc::make_mut(&mut state.transcript_state)
        .lines
        .push(TranscriptLine::AssistantText(body));
    for i in 0..TAIL_LINES {
        state.push_user(format!("tail {i}"));
    }
    state
}

/// The number of rendered rows the whole transcript takes up, as published
/// by the render path.
fn total_rows(state: &AppState) -> usize {
    state.transcript_viewport.lock().unwrap().content_height
}

/// A page scroll must move a page of **rendered rows**, not a page of
/// transcript lines.
///
/// Under the old unit this fixture's page step was one *line* — and the
/// one line that filled the screen was the 200-row message, so a single
/// PageUp skipped the whole message at once.
#[test]
fn a_page_scroll_moves_rendered_rows_not_transcript_lines() {
    // 钉的是旧的转录行语义，本次因滚动单位改为渲染行而变：
    // 旧期望是「翻页步长 = 一屏转录行」，新语义下是「一屏渲染行 − 4 行重叠」。
    let mut state = long_line_transcript();

    // One frame publishes the viewport and the page size.
    frame_text(&state);
    assert_eq!(
        state.transcript_state.scroll_offset, 0,
        "fixture precondition: the transcript starts at the live tail"
    );
    let long_rows = total_rows(&state) - TAIL_LINES;
    assert!(
        long_rows > 100,
        "fixture precondition: the long message must be far taller than the \
         viewport, got {long_rows} rows"
    );

    let page_rows = state.page_rows();
    let step = page_rows.saturating_sub(PAGE_OVERLAP).max(1);

    // "A page" is a screen of rendered rows: the published page size and
    // the published viewport height must be the same thing.
    let viewport_height = state.transcript_viewport.lock().unwrap().viewport_height;
    assert!(
        (page_rows as i64 - viewport_height as i64).abs() <= 1,
        "a page is a screen of rendered rows: page_rows {page_rows} vs \
         viewport height {viewport_height}"
    );

    apply_action(&mut state, page_up());
    let offset = state.transcript_state.scroll_offset;

    assert_eq!(
        offset, step,
        "one PageUp must move one screen of rendered rows minus Pi's \
         {PAGE_OVERLAP}-row overlap ({step} rows), not a screen of transcript \
         lines"
    );
    assert!(
        step > 1,
        "the page step was {step} rows: with a 200-row message on screen a \
         line-based step would be 1, and the whole message would be skipped \
         in one press"
    );
    assert!(
        offset > TAIL_LINES,
        "the offset is {offset} rows and the tail is only {TAIL_LINES} rows: \
         the viewport must be *inside* the long message, not above it"
    );
    assert!(
        offset < long_rows,
        "the long message is {long_rows} rows and the offset is {offset}: the \
         whole message must not be skipped in one page"
    );
}

/// The middle of a long message must be reachable, screen by screen.
///
/// This is the report itself: "翻页时中间内容消失". With the old
/// line-based unit the renderer could only ever paint a message's head or
/// its tail, because a partially visible message was cut from its **end**
/// (`render.rs`: `start = line_items.len() - take`) — the middle was never
/// drawn at all.
#[test]
fn the_middle_of_a_long_line_can_be_scrolled_to() {
    // 钉的是旧的转录行语义，本次因滚动单位改为渲染行而变：
    // 旧语义下长消息只能整条跳过（头部或尾部可见），中间永远不渲染。
    let mut state = long_line_transcript();
    frame_text(&state);
    let long_rows = total_rows(&state) - TAIL_LINES;

    let mut reached_at = None;
    for press in 1..=120 {
        apply_action(&mut state, page_up());
        let text = frame_text(&state);
        if (MIDDLE_FROM..=MIDDLE_TO).any(|i| text.contains(&format!("seg-{i:03}"))) {
            // The middle must be the middle: neither the head nor the tail
            // of the message may be on screen at the same time.
            assert!(
                !text.contains("seg-000"),
                "the head of the long message is on screen together with its \
                 middle — the viewport is showing the wrong slice"
            );
            assert!(
                !text.contains(&format!("seg-{:03}", LONG_SEGMENTS - 1)),
                "the tail of the long message is on screen together with its \
                 middle — the viewport is showing the wrong slice"
            );
            reached_at = Some(press);
            break;
        }
    }

    assert!(
        reached_at.is_some(),
        "no screen ever showed rows {MIDDLE_FROM}..{MIDDLE_TO} of the long \
         message: the middle of a long message is still unreachable"
    );

    let offset = state.transcript_state.scroll_offset;
    assert!(
        offset > TAIL_LINES && offset < long_rows,
        "the middle was reached at scroll offset {offset} rows: it must be \
         *inside* the long message (between {TAIL_LINES} and {long_rows} \
         rows), not above it and not past it"
    );
}

/// The wheel moves one **rendered row**, not one transcript line.
///
/// Under the old unit one notch moved one transcript line, which in this
/// fixture was the whole 200-row message.
#[test]
fn the_wheel_moves_one_rendered_row() {
    // 钉的是旧的转录行语义，本次因滚动单位改为渲染行而变：
    // 旧语义下一格滚轮 = 一条转录行（此处即整条 200 行消息）。
    let state = long_line_transcript();
    let shared = shared_state(state);
    draw_shared(&shared);

    handle_mouse(wheel_up(40, 10), &shared);

    assert_eq!(
        lock_state(&shared).transcript_state.scroll_offset,
        1,
        "one wheel notch is one rendered row (Pi's `wheelScrollLines` default \
         of 1, `tui-alt-screen.ts:264`), so the offset must be 1 — not the \
         whole long message"
    );
}

/// Consecutive pages move by the same number of rows: the page size is a
/// property of the viewport, not of the content under the cursor.
#[test]
fn the_page_step_is_the_same_every_time() {
    // 钉的是旧的转录行语义，本次因滚动单位改为渲染行而变：
    // 旧语义下步长随内容行高浮动（长行一条就占满一屏），新语义下恒为一屏渲染行。
    let mut state = long_line_transcript();
    frame_text(&state);

    let mut last = state.transcript_state.scroll_offset;
    let mut deltas = Vec::new();
    for _ in 0..5 {
        apply_action(&mut state, page_up());
        let now = state.transcript_state.scroll_offset;
        deltas.push(now - last);
        last = now;
    }

    assert!(
        deltas.windows(2).all(|w| w[0] == w[1]),
        "the page step must not depend on the content under the cursor: \
         {deltas:?}"
    );
    assert!(
        deltas[0] > 1,
        "the step was {} rows: a line-based step would be 1 row here",
        deltas[0]
    );
}
