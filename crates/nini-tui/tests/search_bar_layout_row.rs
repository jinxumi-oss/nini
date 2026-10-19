//! Oracle: the F019 search bar must have a row of its own.
//! invariant: d4070d0b
//!
//! The defect, verbatim from the fix: the search bar used to be an
//! **overlay** painted on top of the first transcript row, "which
//! produced a mixed display (search hint text bleeding into the bash
//! transcript line below it)". The invariant: **when search is
//! active, the layout reserves a row for the bar and the transcript
//! starts below it — the two never share a row.**
//!
//! Everything here is read off a `TestBackend` frame: row indices and
//! the glyphs in each row. No layout constant from `render.rs` is
//! restated, so a later re-layout cannot silently satisfy these
//! assertions by coincidence.

use nini_tui::render::render_frame;
use nini_tui::state::AppState;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// Tokens that only ever appear in transcript content, never in the
/// search bar or the chrome around it.
const TRANSCRIPT_TOKENS: [&str; 2] = ["ALPHA_MARKER", "BETA_MARKER"];

fn render(state: &AppState, w: u16, h: u16) -> ratatui::buffer::Buffer {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_frame(f, state)).unwrap();
    terminal.backend().buffer().clone()
}

fn row_text(buf: &ratatui::buffer::Buffer, y: u16) -> String {
    (0..buf.area.width)
        .map(|x| buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "))
        .collect()
}

fn bar_row(buf: &ratatui::buffer::Buffer) -> Option<u16> {
    (0..buf.area.height).find(|&y| row_text(buf, y).contains("n=next"))
}

fn first_transcript_row(buf: &ratatui::buffer::Buffer) -> Option<u16> {
    (0..buf.area.height).find(|&y| row_text(buf, y).contains("ALPHA_MARKER"))
}

/// Fixture: two transcript lines whose first characters are the markers.
/// `push_user` starts a new turn, so this is genuinely two rows.
fn transcript_with_search() -> AppState {
    let mut state = AppState::new("test-model");
    state.push_user("ALPHA_MARKER first transcript line");
    state.push_user("BETA_MARKER second transcript line");
    state.begin_search();
    state.update_search_query("MARKER".to_string());
    assert_eq!(
        state
            .ui_state
            .search
            .as_ref()
            .expect("search is active")
            .matches
            .len(),
        2,
        "fixture precondition: both lines match"
    );
    state
}

#[test]
fn the_search_bar_row_carries_no_transcript_text() {
    let state = transcript_with_search();
    let buf = render(&state, 60, 12);

    let bar = bar_row(&buf).expect("the search bar must be on screen");
    let text = row_text(&buf, bar);
    for token in TRANSCRIPT_TOKENS {
        assert!(
            !text.contains(token),
            "row {bar} carries both the search bar and transcript text \
             ({token}) — this is the bleed d4070d0b fixed:\n{text}"
        );
    }
}

#[test]
fn the_transcript_starts_below_the_search_bar() {
    let state = transcript_with_search();
    let buf = render(&state, 60, 12);

    let bar = bar_row(&buf).expect("the search bar must be on screen");
    let first = first_transcript_row(&buf).expect("the transcript must be on screen");

    assert_ne!(
        bar, first,
        "the search bar is painted on top of the first transcript row \
         (row {bar}) instead of getting a row of its own"
    );
    assert!(
        first > bar,
        "the transcript starts at row {first}, above the search bar at row \
         {bar}: the layout did not shift the transcript down to make room"
    );
    // And the row directly below the bar is a transcript row, i.e. the
    // reserved row really is the first transcript row.
    assert_eq!(
        first,
        bar + 1,
        "row {bar} is the bar and row {first} is the first transcript row; \
         they are not adjacent, so the reserved row went somewhere else"
    );
}

#[test]
fn opening_search_costs_exactly_one_transcript_row() {
    let mut open = transcript_with_search();

    let mut closed = AppState::new("test-model");
    closed.push_user("ALPHA_MARKER first transcript line");
    closed.push_user("BETA_MARKER second transcript line");

    let with_search = render(&open, 60, 12);
    let without_search = render(&closed, 60, 12);

    bar_row(&with_search).expect("the search bar must be on screen");
    assert!(
        bar_row(&without_search).is_none(),
        "a search bar is on screen with no search active"
    );
    assert_eq!(
        first_transcript_row(&with_search),
        first_transcript_row(&without_search).map(|y| y + 1),
        "the transcript did not move down by exactly the one row the \
         search bar occupies"
    );

    // Cancelling search must give the row back, not leave a blank one.
    open.end_search();
    let after = render(&open, 60, 12);
    assert!(
        bar_row(&after).is_none(),
        "the search bar is still painted after search was cancelled"
    );
    assert_eq!(
        first_transcript_row(&after),
        first_transcript_row(&without_search),
        "after cancelling, the transcript did not return to its unshifted row"
    );
}
