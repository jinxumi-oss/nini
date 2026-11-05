//! v0.16 fix: wheel scrolling in completion popup and selector overlays.
//!
//! Before this fix, `handle_mouse` dropped every mouse event when an
//! overlay was active. The completion popup is keyboard-driven (Up/Down)
//! with no mouse input path, so a wheel scroll inside the popup was a
//! dead no-op. Selectors are routed via `handle_selector_key` in the
//! run_loop event loop — wheel events there are translated to Up/Down.

use crossterm::event::{MouseEvent, MouseEventKind};
use nini_tui::runtime::{handle_mouse, shared_state, SharedState};
use nini_tui::state::{AppState, CompletionItem, CompletionPopup, RunMode};
use std::sync::Arc;

fn mouse(kind: MouseEventKind) -> MouseEvent {
    MouseEvent {
        kind,
        column: 40,
        row: 10,
        modifiers: crossterm::event::KeyModifiers::NONE,
    }
}

fn popup_with_items() -> CompletionPopup {
    CompletionPopup::with_items(vec![
        CompletionItem {
            name: "cmd1".to_string(),
            description: "first".to_string(),
            argument_hint: None,
        },
        CompletionItem {
            name: "cmd2".to_string(),
            description: "second".to_string(),
            argument_hint: None,
        },
        CompletionItem {
            name: "cmd3".to_string(),
            description: "third".to_string(),
            argument_hint: None,
        },
    ])
}

fn state_with_popup(popup: CompletionPopup) -> SharedState {
    let mut state = AppState::default();
    state.run_state.mode = RunMode::Editing;
    state.ui_state.completion = Some(popup);
    shared_state(state)
}

#[test]
fn wheel_scrolls_completion_popup_down() {
    let popup = popup_with_items();
    let shared = state_with_popup(popup);

    // Start at index 0.
    let selected_before = {
        let g = nini_tui::runtime::lock_state(&shared);
        g.ui_state.completion.as_ref().unwrap().selected
    };
    assert_eq!(selected_before, 0);

    // Scroll down: should move to index 1.
    handle_mouse(mouse(MouseEventKind::ScrollDown), &shared);
    let selected_after = {
        let g = nini_tui::runtime::lock_state(&shared);
        g.ui_state.completion.as_ref().unwrap().selected
    };
    assert_eq!(selected_after, 1, "wheel down should move selection down");
}

#[test]
fn wheel_scrolls_completion_popup_up() {
    let mut popup = popup_with_items();
    popup.selected = 2; // Start at index 2.
    let shared = state_with_popup(popup);

    // Scroll up: should move to index 1.
    handle_mouse(mouse(MouseEventKind::ScrollUp), &shared);
    let selected_after = {
        let g = nini_tui::runtime::lock_state(&shared);
        g.ui_state.completion.as_ref().unwrap().selected
    };
    assert_eq!(selected_after, 1, "wheel up should move selection up");
}

#[test]
fn wheel_wraps_completion_popup() {
    let popup = popup_with_items(); // index 0.
    let shared = state_with_popup(popup);

    // Scroll up from index 0 should wrap to index 2.
    handle_mouse(mouse(MouseEventKind::ScrollUp), &shared);
    let selected_after = {
        let g = nini_tui::runtime::lock_state(&shared);
        g.ui_state.completion.as_ref().unwrap().selected
    };
    assert_eq!(selected_after, 2, "wheel up from top should wrap to bottom");

    // Scroll down from index 2 should wrap to index 0.
    handle_mouse(mouse(MouseEventKind::ScrollDown), &shared);
    let selected_after = {
        let g = nini_tui::runtime::lock_state(&shared);
        g.ui_state.completion.as_ref().unwrap().selected
    };
    assert_eq!(
        selected_after, 0,
        "wheel down from bottom should wrap to top"
    );
}

#[test]
fn wheel_does_not_scroll_transcript_when_popup_open() {
    let mut state = AppState::default();
    state.run_state.mode = RunMode::Editing;
    state.ui_state.completion = Some(popup_with_items());
    // Add a transcript so scrolling would be possible.
    Arc::make_mut(&mut state.transcript_state).lines.push(
        nini_tui::state::TranscriptLine::AssistantText("test".to_string()),
    );
    let shared = shared_state(state);

    // Scroll should affect the popup, not the transcript.
    let scroll_offset_before = {
        let g = nini_tui::runtime::lock_state(&shared);
        g.transcript_state.scroll_offset
    };
    handle_mouse(mouse(MouseEventKind::ScrollDown), &shared);
    let scroll_offset_after = {
        let g = nini_tui::runtime::lock_state(&shared);
        g.transcript_state.scroll_offset
    };
    assert_eq!(
        scroll_offset_before, scroll_offset_after,
        "wheel should not scroll the transcript when popup is open"
    );
}
