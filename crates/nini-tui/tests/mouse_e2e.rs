//! End-to-end: wheel scrolling and dragging the transcript scrollbar.
//!
//! v0.10. Both drive `scroll_offset` through the same path the keyboard uses,
//! so this asserts the shared invariant rather than a private one: whatever
//! moves the viewport, the rendered thumb follows it.
//!
//! Pi's equivalents are `tui-alt-screen.ts:741-760` (drag) and the
//! `wheelScrollLines` option (default 3).

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use nini_tui::render::render_frame;
use nini_tui::runtime::{handle_mouse, shared_state, SharedState};
use nini_tui::state::{AppState, RunMode, TranscriptLine};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::sync::Arc;

const THUMB: char = '█';

/// A state with a long transcript, already rendered once so the mouse has a
/// viewport to hit — the render pass is what publishes it.
fn primed(lines: usize, mode: nini_tui::scrollbar::ScrollbarMode) -> SharedState {
    let mut state = AppState::default();
    state.run_state.mode = RunMode::Editing;
    state.ui_state.fullscreen_scrollbar = mode;
    Arc::make_mut(&mut state.transcript_state)
        .lines
        .extend((0..lines).map(|i| TranscriptLine::AssistantText(format!("line {i}"))));
    let shared = shared_state(state);
    draw(&shared);
    shared
}

fn draw(shared: &SharedState) {
    let state = nini_tui::runtime::lock_state(shared);
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|f| render_frame(f, &state)).unwrap();
}

fn offset(shared: &SharedState) -> usize {
    nini_tui::runtime::lock_state(shared)
        .transcript_state
        .scroll_offset
}

fn thumb_rows(shared: &SharedState) -> Vec<usize> {
    let state = nini_tui::runtime::lock_state(shared);
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|f| render_frame(f, &state)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    (0..24u16)
        .filter(|y| buffer[(79, *y)].symbol().starts_with(THUMB))
        .map(|y| y as usize)
        .collect()
}

fn viewport_of(shared: &SharedState) -> nini_tui::scrollbar::Viewport {
    nini_tui::runtime::lock_state(shared)
        .transcript_viewport
        .lock()
        .map(|v| *v)
        .unwrap_or_default()
}

fn mouse(kind: MouseEventKind, col: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column: col,
        row,
        modifiers: crossterm::event::KeyModifiers::NONE,
    }
}

// ---------------------------------------------------------------------------
// wheel
// ---------------------------------------------------------------------------

#[test]
fn the_wheel_scrolls_up_and_back_down() {
    let shared = primed(80, nini_tui::scrollbar::ScrollbarMode::Always);

    handle_mouse(mouse(MouseEventKind::ScrollUp, 40, 10), &shared);
    let up = offset(&shared);
    assert_eq!(up, 3, "Pi's wheelScrollLines default");

    handle_mouse(mouse(MouseEventKind::ScrollDown, 40, 10), &shared);
    assert_eq!(offset(&shared), 0);
    assert_eq!(up, 3, "and the scroll really happened");
}

#[test]
fn the_wheel_stops_at_the_ends() {
    let shared = primed(80, nini_tui::scrollbar::ScrollbarMode::Always);
    for _ in 0..200 {
        handle_mouse(mouse(MouseEventKind::ScrollUp, 40, 10), &shared);
    }
    let top = offset(&shared);
    assert_eq!(top, 79, "clamped to lines.len() - 1");

    for _ in 0..200 {
        handle_mouse(mouse(MouseEventKind::ScrollDown, 40, 10), &shared);
    }
    assert_eq!(offset(&shared), 0, "and back to the tail");
}

#[test]
fn scrolling_up_disables_autoscroll_and_returns_to_the_tail_restores_it() {
    let shared = primed(80, nini_tui::scrollbar::ScrollbarMode::Always);
    handle_mouse(mouse(MouseEventKind::ScrollUp, 40, 10), &shared);
    assert!(
        !nini_tui::runtime::lock_state(&shared)
            .transcript_state
            .autoscroll
    );

    for _ in 0..5 {
        handle_mouse(mouse(MouseEventKind::ScrollDown, 40, 10), &shared);
    }
    assert!(
        nini_tui::runtime::lock_state(&shared)
            .transcript_state
            .autoscroll
    );
}

#[test]
fn the_wheel_moves_the_thumb() {
    let shared = primed(80, nini_tui::scrollbar::ScrollbarMode::Always);
    let before = thumb_rows(&shared).first().copied().unwrap_or(0);

    handle_mouse(mouse(MouseEventKind::ScrollUp, 40, 10), &shared);
    let after = thumb_rows(&shared).first().copied().unwrap_or(0);
    assert!(
        after < before,
        "scrolling up must move the thumb up: {before} -> {after}"
    );
}

// ---------------------------------------------------------------------------
// scrollbar drag
// ---------------------------------------------------------------------------

#[test]
fn a_press_on_the_bar_starts_a_drag() {
    let shared = primed(80, nini_tui::scrollbar::ScrollbarMode::Always);
    let vp = viewport_of(&shared);
    let col = vp.x + vp.width - 1;
    let row = vp.y + vp.height - 1; // on the thumb, at the tail

    handle_mouse(
        mouse(MouseEventKind::Down(MouseButton::Left), col, row),
        &shared,
    );
    assert!(
        nini_tui::runtime::lock_state(&shared)
            .ui_state
            .scrollbar_drag
            .is_some(),
        "a press on the scrollbar column must begin a drag"
    );
}

#[test]
fn a_press_away_from_the_bar_starts_nothing() {
    let shared = primed(80, nini_tui::scrollbar::ScrollbarMode::Always);
    let vp = viewport_of(&shared);
    handle_mouse(
        mouse(MouseEventKind::Down(MouseButton::Left), vp.x + 10, vp.y + 5),
        &shared,
    );
    assert!(nini_tui::runtime::lock_state(&shared)
        .ui_state
        .scrollbar_drag
        .is_none());
}

/// Grabbing the rail *above* the thumb and dragging scrolls to the grab
/// point, which is how a reader covers a long transcript quickly.
///
/// A press alone does not move anything, and that is Pi's behaviour, not an
/// omission: `handleScrollbarMouseEvent` (`tui-alt-screen.ts:741`) sets
/// `scrollbarDrag` on the press and only recomputes on a subsequent drag.
/// The first draft of this test sent press-then-release and asserted the
/// viewport had moved; it had not, and the implementation was right.
#[test]
fn dragging_from_the_rail_scrolls_to_the_grab_point() {
    let shared = primed(80, nini_tui::scrollbar::ScrollbarMode::Always);
    let vp = viewport_of(&shared);
    let col = vp.x + vp.width - 1;

    handle_mouse(
        mouse(MouseEventKind::Down(MouseButton::Left), col, vp.y + 2),
        &shared,
    );
    assert_eq!(offset(&shared), 0, "a press alone moves nothing, as in Pi");

    handle_mouse(
        mouse(MouseEventKind::Drag(MouseButton::Left), col, vp.y),
        &shared,
    );
    let after = offset(&shared);
    assert!(
        after > 0,
        "dragging the pointer to the top of the track must scroll back; got {after}"
    );
    // The pointer went from near the bottom of the rail to the top of it,
    // so this must be a real jump rather than a one-line nudge.
    assert!(
        after > 5,
        "a drag across the rail covers most of the history; got {after}"
    );
}

#[test]
fn dragging_the_thumb_to_the_top_reaches_the_beginning() {
    let shared = primed(80, nini_tui::scrollbar::ScrollbarMode::Always);
    let vp = viewport_of(&shared);
    let col = vp.x + vp.width - 1;

    // Grab wherever the thumb is, then walk it to the top of the track.
    let thumb_top = thumb_rows(&shared).first().copied().unwrap_or(0) as u16;
    handle_mouse(
        mouse(MouseEventKind::Down(MouseButton::Left), col, thumb_top),
        &shared,
    );
    handle_mouse(
        mouse(MouseEventKind::Drag(MouseButton::Left), col, vp.y),
        &shared,
    );
    handle_mouse(
        mouse(MouseEventKind::Up(MouseButton::Left), col, vp.y),
        &shared,
    );

    assert!(
        offset(&shared) > 0,
        "the thumb at the top must hide history, got {}",
        offset(&shared)
    );
    assert!(nini_tui::runtime::lock_state(&shared)
        .ui_state
        .scrollbar_drag
        .is_none());
}

/// Pi's `grabOffset` (`tui-alt-screen.ts:777`): the thumb keeps the same
/// point under the cursor. Grab it low, and dragging should not jump by the
/// whole distance.
#[test]
fn the_thumb_keeps_its_grab_point() {
    let shared = primed(80, nini_tui::scrollbar::ScrollbarMode::Always);
    let vp = viewport_of(&shared);
    let col = vp.x + vp.width - 1;
    // Must stay inside the track: `thumb_top + 3` was the first draft and it
    // landed past the bottom of the transcript area, so the press missed the
    // bar entirely and there was no drag to inspect.
    let thumb_top = thumb_rows(&shared).first().copied().unwrap_or(0) as u16;
    let press_row = (thumb_top + 1).min(vp.y + vp.height - 1);
    assert!(
        press_row >= vp.y && press_row < vp.y + vp.height,
        "the press must be inside the track: {press_row} vs {}..{}",
        vp.y,
        vp.y + vp.height
    );

    handle_mouse(
        mouse(MouseEventKind::Down(MouseButton::Left), col, press_row),
        &shared,
    );
    let grab_offset = nini_tui::runtime::lock_state(&shared)
        .ui_state
        .scrollbar_drag
        .unwrap()
        .grab_offset;
    assert_eq!(
        grab_offset,
        press_row as i32 - thumb_top as i32,
        "the grab offset is how far below the thumb's top the pointer was"
    );
}

/// A drag that leaves the track clamps, and a release ends it. Without the
/// release the next press anywhere would still be treated as a drag.
#[test]
fn releasing_ends_the_drag() {
    let shared = primed(80, nini_tui::scrollbar::ScrollbarMode::Always);
    let vp = viewport_of(&shared);
    let col = vp.x + vp.width - 1;

    handle_mouse(
        mouse(MouseEventKind::Down(MouseButton::Left), col, vp.y + 2),
        &shared,
    );
    // Drag far above the track. It clamps to the top, which is *not* zero.
    handle_mouse(
        mouse(
            MouseEventKind::Drag(MouseButton::Left),
            col,
            vp.y.saturating_sub(99),
        ),
        &shared,
    );
    let top = offset(&shared);
    assert!(
        top > 0,
        "a drag above the track clamps to the top, got {top}"
    );

    // And the reverse: far below clamps to the live tail.
    handle_mouse(
        mouse(MouseEventKind::Drag(MouseButton::Left), col, vp.y + 99),
        &shared,
    );
    assert_eq!(
        offset(&shared),
        0,
        "a drag below the track clamps to the tail, not wrapping to the top"
    );

    handle_mouse(
        mouse(MouseEventKind::Up(MouseButton::Left), col, vp.y + 99),
        &shared,
    );
    assert!(nini_tui::runtime::lock_state(&shared)
        .ui_state
        .scrollbar_drag
        .is_none());
}

/// crossterm enables 1003, so every pointer motion arrives. Nothing acts on
/// motion, and acting on it would repaint on every jiggle.
#[test]
fn motion_alone_is_ignored() {
    let shared = primed(80, nini_tui::scrollbar::ScrollbarMode::Always);
    let before = offset(&shared);
    for row in 0..24u16 {
        handle_mouse(mouse(MouseEventKind::Moved, 40, row), &shared);
    }
    assert_eq!(offset(&shared), before);
    assert!(nini_tui::runtime::lock_state(&shared)
        .ui_state
        .scrollbar_drag
        .is_none());
}

/// With no frame drawn there is no viewport, so a press on the bar must be
/// refused rather than guessing a scroll position.
///
/// The wheel is different and the first draft got this wrong: it needs no
/// viewport at all, only the transcript length, because Pi's handler calls
/// `scrollBy` on the primary scroll view without consulting the pointer
/// Found by falsification: an injection that drops `grab_offset` from the
/// drag arithmetic left every test green, because the earlier test asserts
/// the *field* rather than its effect.
///
/// Two independent runs from the same starting state, differing only in
/// where on the thumb the pointer went down. The first draft reused one
/// state, which does not work: the first drag moves the thumb, so the
/// second press computes its offset against a thumb that has moved.
#[test]
fn the_grab_point_changes_where_a_drag_lands() {
    let vp_of = |shared: &SharedState| viewport_of(shared);
    let target_row = 6;

    // Grab the thumb's very top, then drag the pointer to a fixed row.
    let a = primed(80, nini_tui::scrollbar::ScrollbarMode::Always);
    let (col_a, thumb_a) = {
        let vp = vp_of(&a);
        (
            vp.x + vp.width - 1,
            thumb_rows(&a).first().copied().unwrap_or(0) as u16,
        )
    };
    handle_mouse(
        mouse(MouseEventKind::Down(MouseButton::Left), col_a, thumb_a),
        &a,
    );
    handle_mouse(
        mouse(MouseEventKind::Drag(MouseButton::Left), col_a, target_row),
        &a,
    );
    let from_top = offset(&a);

    // Same starting state, but grab two rows lower.
    let b = primed(80, nini_tui::scrollbar::ScrollbarMode::Always);
    let (col_b, thumb_b) = {
        let vp = vp_of(&b);
        (
            vp.x + vp.width - 1,
            thumb_rows(&b).first().copied().unwrap_or(0) as u16,
        )
    };
    assert_eq!(thumb_a, thumb_b, "both runs start from the same thumb");
    handle_mouse(
        mouse(MouseEventKind::Down(MouseButton::Left), col_b, thumb_b + 2),
        &b,
    );
    handle_mouse(
        mouse(MouseEventKind::Drag(MouseButton::Left), col_b, target_row),
        &b,
    );
    let from_lower = offset(&b);

    assert_ne!(
        from_top, from_lower,
        "grabbing two rows lower must change where the same pointer \
         position lands: {from_top} vs {from_lower}"
    );
}

/// Pressing *above* the thumb gives a negative grab offset, exactly as Pi
/// does (`event.y - geometry.thumbTop`). A `u16` with a saturating
/// subtraction turned that into zero, which made the thumb jump up to the
/// pointer the instant the drag began.
#[test]
fn grabbing_above_the_thumb_keeps_the_pointer_where_it_landed() {
    let shared = primed(80, nini_tui::scrollbar::ScrollbarMode::Always);
    let vp = viewport_of(&shared);
    let col = vp.x + vp.width - 1;
    let thumb_top = thumb_rows(&shared).first().copied().unwrap_or(0) as u16;
    let above = thumb_top - 3;
    assert!(above >= vp.y, "must stay on the track");

    handle_mouse(
        mouse(MouseEventKind::Down(MouseButton::Left), col, above),
        &shared,
    );
    let drag = nini_tui::runtime::lock_state(&shared)
        .ui_state
        .scrollbar_drag
        .unwrap();
    assert_eq!(
        drag.grab_offset, -3,
        "grabbing three rows above the thumb is a negative offset, as in Pi"
    );

    handle_mouse(
        mouse(MouseEventKind::Drag(MouseButton::Left), col, above),
        &shared,
    );
    assert_eq!(
        offset(&shared),
        0,
        "dragging without moving leaves the viewport where it was"
    );
}

/// Also found by falsification: dropping the wheel's `mark_scroll_activity`
/// left the suite green, which means nothing checked that a wheel gesture
/// wakes the `auto`-mode scrollbar. It must.
#[test]
fn the_wheel_wakes_the_auto_scrollbar() {
    let shared = primed(80, nini_tui::scrollbar::ScrollbarMode::Auto);
    assert!(
        thumb_rows(&shared).is_empty(),
        "`auto` starts dark with no recent scrolling"
    );

    handle_mouse(mouse(MouseEventKind::ScrollUp, 40, 10), &shared);
    assert!(
        !thumb_rows(&shared).is_empty(),
        "a wheel gesture counts as scrolling and must light the bar"
    );
}

/// The drag must keep it lit too, for as long as the reader is moving.
#[test]
fn dragging_wakes_the_auto_scrollbar() {
    let shared = primed(80, nini_tui::scrollbar::ScrollbarMode::Auto);
    let vp = viewport_of(&shared);
    let col = vp.x + vp.width - 1;

    // Not `thumb_rows(...).first()`: `auto` draws nothing until something
    // scrolls, so there is no thumb to find and the first draft read a
    // thumb_top of 0 - which is above the track, so the press missed and the
    // drag never began. The thumb at rest is at the bottom of the track.
    let rest = vp.y + vp.height - 1;

    handle_mouse(
        mouse(MouseEventKind::Down(MouseButton::Left), col, rest),
        &shared,
    );
    handle_mouse(
        mouse(MouseEventKind::Drag(MouseButton::Left), col, vp.y),
        &shared,
    );

    assert!(
        nini_tui::runtime::lock_state(&shared)
            .ui_state
            .scrollbar_drag
            .is_some(),
        "the drag must have started"
    );
    assert!(
        !thumb_rows(&shared).is_empty(),
        "an in-progress drag keeps the bar visible"
    );
}
/// position. A wheel event before the first frame scrolls.
#[test]
fn a_press_before_the_first_frame_is_refused() {
    let mut state = AppState::default();
    state.run_state.mode = RunMode::Editing;
    Arc::make_mut(&mut state.transcript_state)
        .lines
        .extend((0..80).map(|i| TranscriptLine::AssistantText(format!("line {i}"))));
    let shared = shared_state(state); // deliberately never drawn

    assert!(!handle_mouse(
        mouse(MouseEventKind::Down(MouseButton::Left), 79, 5),
        &shared
    ));
    assert!(nini_tui::runtime::lock_state(&shared)
        .ui_state
        .scrollbar_drag
        .is_none());
    assert_eq!(offset(&shared), 0);
}

#[test]
fn the_wheel_needs_no_viewport_only_a_transcript() {
    let mut state = AppState::default();
    state.run_state.mode = RunMode::Editing;
    Arc::make_mut(&mut state.transcript_state)
        .lines
        .extend((0..80).map(|i| TranscriptLine::AssistantText(format!("line {i}"))));
    let shared = shared_state(state); // never drawn

    handle_mouse(mouse(MouseEventKind::ScrollUp, 40, 10), &shared);
    assert_eq!(offset(&shared), 3, "the wheel acts on line counts alone");
}
