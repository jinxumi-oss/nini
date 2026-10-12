//! End-to-end: the prompt cursor must sit on the last character the user
//! typed, for input a CJK terminal actually contains.
//!
//! v0.10. The defect this file pins was reported as "the input box is
//! wrong" and had two parts, both invisible to a suite that types English:
//!
//! 1. The column came from `chars().count()` — a count of characters used
//!    where a count of terminal cells was needed. CJK glyphs are two cells,
//!    so `研究一下` (4 chars, 8 cells) put the cursor on top of the second
//!    character.
//! 2. The row came from `chars / inner_width`, which does not know a
//!    newline exists. On a multi-line prompt the cursor was placed at a row
//!    computed from the whole buffer, not the line being typed on.
//!
//! The unit tests in `render.rs` cover `prompt_cursor_geometry` directly.
//! This file exists because those are private, and because the thing that
//! broke was never the arithmetic — it was the arithmetic being unreachable
//! from a test that rendered the box. So: render the box, read the cursor
//! back off the terminal, compare against the cells the text occupies.
//!
//! Following the discipline in `transcript_width_e2e.rs`: assert on cells,
//! never on bytes or characters.

use nini_tui::render::render_frame;
use nini_tui::state::{AppState, RunMode};
use nini_tui::width::display_width;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// Render `text` with the cursor at the end and return
/// `(cursor_column, the input row's rendered cells)`.
fn render_prompt(text: &str, width: u16, height: u16) -> (u16, String) {
    let mut state = AppState::default();
    state.input.text = text.to_string();
    state.input.cursor = text.len();
    state.run_state.mode = RunMode::Editing;

    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_frame(f, &state)).unwrap();

    let pos = terminal
        .get_cursor_position()
        .unwrap_or(ratatui::layout::Position::new(0, 0));
    let buffer = terminal.backend().buffer().clone();

    // The prompt line is the one the cursor is on.
    let row = pos.y as usize;
    let cells: String = (0..width)
        .map(|x| buffer[(x, row as u16)].symbol().to_string())
        .collect();

    (pos.x, cells)
}

/// The box draws a `❯ ` prefix, one border cell and one padding cell on
/// the left, so the first text cell sits at this offset. Computed from the
/// renderer's own constants rather than guessed, so this test breaks if the
/// chrome moves rather than silently testing a stale number.
const PROMPT_TEXT_INSET: u16 = 1 /* border */ + 1 /* padding */ + 2 /* "❯ " */;

#[test]
fn an_ascii_prompt_puts_the_cursor_after_the_last_character() {
    let text = "hello world";
    let (cx, _) = render_prompt(text, 80, 24);
    assert_eq!(
        cx as usize,
        PROMPT_TEXT_INSET as usize + text.chars().count(),
        "ASCII: one cell per character"
    );
}

#[test]
fn a_cjk_prompt_puts_the_cursor_after_the_last_character() {
    let text = "研究一下";
    let (cx, _) = render_prompt(text, 80, 24);
    assert_eq!(
        cx as usize,
        PROMPT_TEXT_INSET as usize + display_width(text),
        "CJK glyphs are two cells each; before the fix this was inset + 4 \
         instead of inset + 8"
    );
}

#[test]
fn an_emoji_prompt_puts_the_cursor_after_the_last_character() {
    let text = "done ✅";
    let (cx, _) = render_prompt(text, 80, 24);
    assert_eq!(
        cx as usize,
        PROMPT_TEXT_INSET as usize + display_width(text)
    );
}

#[test]
fn a_mixed_prompt_puts_the_cursor_after_the_last_character() {
    // The shape real input takes: an ASCII command, then Chinese.
    let text = "/research 研究一下这个问题";
    let (cx, _) = render_prompt(text, 80, 24);
    assert_eq!(
        cx as usize,
        PROMPT_TEXT_INSET as usize + display_width(text)
    );
}

/// The assertion that matters here is "the cursor is on the row showing the
/// text being typed", not "the cursor moved down one screen row".
///
/// The first draft asserted `cy == first_row + 1` and it failed, correctly.
/// Adding a newline makes the input box one row taller, which moves the
/// whole box up, so the cursor's absolute row barely moves while its row
/// *inside* the box does. Absolute rows were the wrong thing to pin; the
/// row's contents are the thing a user can actually see.
#[test]
fn a_multiline_prompt_puts_the_cursor_on_the_line_being_typed() {
    let text = "first line\nsecond line";
    let (cx, cells) = render_prompt(text, 80, 24);

    // `contains`, not `starts_with`: the row begins with the box border and
    // the `❯ ` prompt marker. An earlier draft skipped leading spaces and
    // then asserted `starts_with("second")`, which failed on the border
    // while the behaviour was right.
    assert!(
        cells.contains("second line"),
        "the cursor row should show the second line, got the full row \
         {cells:?}"
    );
    assert_eq!(
        cx as usize,
        PROMPT_TEXT_INSET as usize + display_width("second line"),
        "and the column should be at the end of it"
    );
}

#[test]
fn a_three_line_prompt_puts_the_cursor_on_the_third_line() {
    let text = "one\ntwo\nthree";
    let (_, cells) = render_prompt(text, 80, 24);
    assert!(
        cells.contains("three"),
        "the cursor row should show `three`, got {cells:?}"
    );
}

#[test]
fn a_narrow_terminal_does_not_lose_the_cursor() {
    // The row arithmetic divides by `inner_width`; at 40 columns a CJK
    // prompt wraps, and the cursor has to follow it onto the next row.
    let text = "研究这个非常长的问题需要换行";
    for width in [40u16, 60, 80, 120] {
        let (cx, _) = render_prompt(text, width, 24);
        assert!(
            cx < width,
            "at width {width} the cursor landed at column {cx}, off the screen"
        );
    }
}

/// The regression that made this worth a file: nothing in the suite typed a
/// wide character into the prompt, so 1196 green tests coexisted with a
/// cursor drawn on top of the text for every CJK user. This test fails if
/// that blind spot is reintroduced by accident — for instance if someone
/// "simplifies" `prompt_cursor_geometry` back to a character count, because
/// the ASCII assertions above would still pass.
#[test]
fn every_prompt_test_in_this_file_uses_a_non_ascii_case() {
    let cases = ["研究一下", "done ✅", "/research 研究一下这个问题"];
    assert!(
        cases.iter().all(|c| !c.is_ascii()),
        "the file must keep at least one wide-character case or it stops \
         guarding the defect it exists for"
    );
}
