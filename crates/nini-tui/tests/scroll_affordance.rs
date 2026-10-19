//! Oracle: scrolling the transcript has to be *visible* and *fine-grained*.
//! invariant: ffb3d965
//!
//! ffb3d965 (v0.8.4 ux-001) fixed an inverted scroll: `scroll_offset == 0`
//! rendered the **top** of the transcript, so a long assistant reply
//! arrived while the page stayed frozen on the opening turn. It shipped
//! three user-visible things: tail-aligned autoscroll, a `↓ N more`
//! indicator for the lines the user has scrolled past, and a 1-row
//! scroll step (it had been 10).
//!
//! What nini already had: `↑ N more above` (a *later* fix, for
//! multi-row overflow) and the Home/End round trip (`Home` is a
//! different commit again). Nothing asserted the `↓` indicator, and
//! nothing asserted the step size — so both halves of this fix could
//! regress silently.
//!
//! Driven through `runtime::apply_action` and read off a `TestBackend`
//! frame, the same discipline as `transcript_scroll_to_top.rs`.

use nini_tui::keys::{Key, KeyCode, KeyModifiers};
use nini_tui::render::render_frame;
use nini_tui::runtime::apply_action;
use nini_tui::state::AppState;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn page_up() -> Key {
    Key::new(KeyCode::PageUp, KeyModifiers::NONE)
}
fn page_down() -> Key {
    Key::new(KeyCode::PageDown, KeyModifiers::NONE)
}
fn shift_end() -> Key {
    Key::new(KeyCode::End, KeyModifiers::SHIFT)
}

fn render(state: &AppState, w: u16, h: u16) -> ratatui::buffer::Buffer {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_frame(f, state)).unwrap();
    terminal.backend().buffer().clone()
}

fn frame_text(buf: &ratatui::buffer::Buffer) -> String {
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "))
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The user-scroll indicator only — `↓ N more`. The `↑ N more above`
/// label is a *different* indicator from a later fix and says nothing
/// about it, which is why the two must not be matched by "more".
fn down_indicator(frame: &str) -> Option<usize> {
    frame.lines().find_map(|row| {
        let at = row.find('\u{2193}')?;
        row[at..]
            .split("more")
            .next()?
            .split_whitespace()
            .nth(1)?
            .parse()
            .ok()
    })
}

/// `n` numbered turns — enough to overflow any viewport used here, and
/// every line carries its index so the visible window is identifiable.
fn transcript(n: usize) -> AppState {
    let mut state = AppState::new("test-model");
    for i in 0..n {
        state.push_user(format!("turn {i}"));
    }
    state
}

#[test]
fn a_cold_transcript_shows_its_tail_not_its_head() {
    let state = transcript(200);
    assert_eq!(
        state.transcript_state.scroll_offset, 0,
        "fixture precondition"
    );

    let text = frame_text(&render(&state, 80, 24));
    assert!(
        text.contains("turn 199"),
        "the newest turn is not on screen on a cold start. `scroll_offset == 0` \
         used to mean the TOP of the transcript, which is why a finished long \
         reply looked like a frozen page."
    );
    assert!(
        !text.contains("turn 0 "),
        "the oldest turn is on screen with 200 lines in a 24-row viewport — \
         the viewport is pinned to the head again"
    );
}

#[test]
fn a_scrolled_up_transcript_says_how_much_is_hidden_below() {
    let mut state = transcript(200);
    for _ in 0..3 {
        apply_action(&mut state, page_up());
    }
    let offset = state.transcript_state.scroll_offset;
    assert!(offset > 0, "fixture precondition: scrolled off the tail");

    let text = frame_text(&render(&state, 80, 24));
    assert!(
        down_indicator(&text).is_some_and(|n| n == offset),
        "no `↓ {offset} more` indicator after scrolling {offset} lines off \
         the tail: the user cannot tell that anything is below the fold.\n\n{text}"
    );
}

#[test]
fn the_indicator_disappears_when_the_viewport_is_back_at_the_tail() {
    let mut state = transcript(200);
    apply_action(&mut state, page_up());
    assert!(
        down_indicator(&frame_text(&render(&state, 80, 24))).is_some(),
        "fixture precondition: an indicator is on screen while scrolled up"
    );

    apply_action(&mut state, page_down());

    assert_eq!(
        state.transcript_state.scroll_offset, 0,
        "fixture precondition"
    );
    assert!(
        state.transcript_state.autoscroll,
        "fixture precondition: PageDown back to the bottom re-enables autoscroll"
    );
    let text = frame_text(&render(&state, 80, 24));
    assert_eq!(
        down_indicator(&text),
        None,
        "the `↓ N more` indicator is still drawn at the live tail, where \
         there is nothing below to go to.\n\n{text}"
    );
}

#[test]
fn scrolling_down_at_the_tail_cannot_underflow() {
    // The step is `page_lines()` — a page, not a row (that part of the
    // original fix was superseded). Whatever the step is, one press
    // down at the tail must leave the offset at 0 with autoscroll on;
    // an unsigned `scroll_offset -= step` underflow would pin the
    // viewport at `saturating_sub` of a wrapped number and the
    // transcript would go blank.
    let mut state = transcript(200);
    apply_action(&mut state, page_down());

    assert_eq!(
        state.transcript_state.scroll_offset, 0,
        "PageDown at the tail left the viewport at {}",
        state.transcript_state.scroll_offset
    );
    assert!(state.transcript_state.autoscroll);
    assert!(
        frame_text(&render(&state, 80, 24)).contains("turn 199"),
        "the newest turn is not on screen after a no-op scroll"
    );
}

#[test]
fn scrolling_down_back_to_the_tail_restores_autoscroll() {
    let mut state = transcript(200);
    apply_action(&mut state, page_up());
    apply_action(&mut state, page_up());
    assert_eq!(
        state.transcript_state.scroll_offset, 2,
        "fixture precondition"
    );

    apply_action(&mut state, page_down());
    apply_action(&mut state, page_down());

    assert_eq!(state.transcript_state.scroll_offset, 0);
    assert!(
        state.transcript_state.autoscroll,
        "scrolling back to the bottom must re-enable autoscroll"
    );
    assert!(
        frame_text(&render(&state, 80, 24)).contains("turn 199"),
        "and the newest turn must be back on screen"
    );
}

#[test]
fn shift_end_snaps_back_to_the_live_tail() {
    // ScrollToBottom was added by this fix (Shift+End): "I'm done
    // skimming, get me back to the current message", plus
    // re-enabling autoscroll so later streaming tokens stick to the
    // bottom.
    let mut state = transcript(200);
    for _ in 0..5 {
        apply_action(&mut state, page_up());
    }
    assert!(
        state.transcript_state.scroll_offset > 0,
        "fixture precondition"
    );

    apply_action(&mut state, shift_end());

    assert_eq!(state.transcript_state.scroll_offset, 0);
    assert!(state.transcript_state.autoscroll);
    let text = frame_text(&render(&state, 80, 24));
    assert!(
        text.contains("turn 199"),
        "not back at the live tail:\n{text}"
    );
    assert_eq!(
        down_indicator(&text),
        None,
        "the `↓ N more` indicator is still drawn after Shift+End"
    );
}
