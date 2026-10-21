//! The overflow indicators must not be painted over the transcript.
//!
//! What was broken: `↑ N more above` was drawn on the transcript area's
//! **first** row and `↓ N more` on its **last** row — the same rows the
//! text was in. The indicator was painted *after* the list, so it landed
//! on top of the content:
//!
//! ```text
//!  ↑ 30 more above r: turn `AppState` into ratatui `Frame` drawing calls.
//! ```
//!
//! (the original line was `//! Render layer: turn `AppState` into …` —
//! its beginning was eaten).
//!
//! The indicators now get a row of their own, and the transcript gets
//! one row less. Two properties are asserted here, because either one
//! alone can be faked:
//!
//! * the indicator's row carries **no** transcript text (the indicator is
//!   not painted over content), and
//! * the indicator is still there (the fix is not "delete the hint and
//!   call it a day").
//!
//! Every message carries a marker at its end. The indicator label is 17
//! cells wide, so a short message would be covered completely and the
//! overlap would be invisible; the marker is the part that survives, and
//! finding it on the indicator's row is the bug.

use std::sync::Arc;

use nini_tui::render::render_frame;
use nini_tui::state::AppState;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// The end-of-line marker. Long enough that the indicator label cannot
/// cover the whole line.
const MARKER: &str = "#tail#";

/// The rendered frame, one string per row.
fn frame_rows(state: &AppState, w: u16, h: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| render_frame(f, state)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "))
                .collect::<String>()
        })
        .collect()
}

/// `n` turns — enough to overflow any viewport used here. Every line
/// carries its index, so a truncated row is recognisable.
fn transcript(n: usize) -> AppState {
    let mut state = AppState::new("test-model");
    for i in 0..n {
        state.push_user(format!(
            "turn {i} — a line long enough to outlast the indicator label {MARKER}"
        ));
    }
    state
}

/// The row that carries the indicator, and its index.
fn indicator_row<'a>(rows: &'a [String], needle: &str) -> (usize, &'a str) {
    rows.iter()
        .enumerate()
        .find(|(_, row)| row.contains(needle))
        .map(|(i, row)| (i, row.as_str()))
        .unwrap_or_else(|| panic!("no `{needle}` indicator on screen:\n{}", rows.join("\n")))
}

/// The turn numbers the frame shows, in order.
fn visible_turns(rows: &[String]) -> Vec<u32> {
    rows.iter()
        .filter_map(|row| {
            let at = row.find("turn ")?;
            row[at + 5..].split_whitespace().next()?.parse::<u32>().ok()
        })
        .collect()
}

#[test]
fn the_overflow_indicator_does_not_eat_the_first_row_of_the_transcript() {
    let state = transcript(200);
    let rows = frame_rows(&state, 80, 24);

    let (at, row) = indicator_row(&rows, "more above");
    assert!(
        !row.contains(MARKER),
        "the `↑ N more above` indicator is painted over the transcript's first \
         row, so the first line the reader sees is cut in half:\n{row:?}"
    );

    // The first visible turn must be complete, and the visible turns
    // must be a contiguous run: the indicator used to eat the head of the
    // first one, which left a gap in the numbering.
    let turns = visible_turns(&rows);
    assert!(
        !turns.is_empty(),
        "no turn is on screen:\n{}",
        rows.join("\n")
    );
    assert!(
        turns.windows(2).all(|w| w[1] == w[0] + 1),
        "the visible turns are not contiguous — the first one was cut:\n{turns:?}"
    );
    assert!(
        rows[at + 1].contains(&format!("> turn {}", turns[0])),
        "the row under the indicator does not start the visible run:\n{:?}",
        rows[at + 1]
    );
}

#[test]
fn the_user_scroll_indicator_does_not_eat_the_last_row_of_the_transcript() {
    let mut state = transcript(200);
    {
        let ts = Arc::make_mut(&mut state.transcript_state);
        ts.autoscroll = false;
        ts.scroll_offset = 1;
    }
    let rows = frame_rows(&state, 80, 24);

    let (_at, row) = indicator_row(&rows, "\u{2193}");
    assert!(
        !row.contains(MARKER),
        "the `↓ N more` indicator is painted over the transcript's last row, \
         so the last line the reader sees is cut in half:\n{row:?}"
    );

    // The hint itself must survive: the fix is a row of its own, not the
    // removal of the hint.
    assert!(
        rows.iter().any(|r| r.contains("more")),
        "the `↓ N more` indicator disappeared:\n{}",
        rows.join("\n")
    );
}
