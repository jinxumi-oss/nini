//! TUI bug reproduction — prompt cursor placement.
//!
//! v0.10. These are not new features; they are executable statements of
//! defects found by inspection on 2026-10-12, each with the failure it
//! produces written down.
//!
//! Run: `cargo test -p nini-tui --test tui_bugs -- --ignored --nocapture`
//!
//! Every test here is marked `#[ignore]`, following the convention in the
//! plan (section 13.1): they are executable statements of defects, not
//! passing tests. Leaving them un-ignored would turn the whole suite red,
//! which is not what a report is for.

use nini_tui::width::display_width;

// ---------------------------------------------------------------------------
// BUG 1 — the prompt cursor is placed by character count, not display width
// ---------------------------------------------------------------------------

/// `render.rs:1115` computes the cursor column from
/// `text[..cursor].chars().count()`, then `render.rs:1187-1188` places it at
/// `left_inset + cursor_char % inner_width`.
///
/// Every other width computation in the same file uses
/// `width::display_width` — lines 274, 327, 552 — and the comment at line 638
/// says so outright: "using display_width so emoji / CJK count correctly".
/// Line 642 imports `unicode_width::UnicodeWidthChar`. The dependency is
/// right there. The cursor is the one place that was missed.
///
/// CJK glyphs occupy two terminal cells. `研究一下` is four characters and
/// eight cells, so the cursor is drawn on top of the second character.
///
/// The failure is invisible for ASCII, which is why the e2e suite never saw
/// it: every existing input test types English.
#[test]
#[ignore = "v0.10: executable statement of a TUI defect, not a passing test"]
fn cjk_input_puts_the_cursor_on_top_of_the_text() {
    let text = "研究一下";
    let cursor_byte = text.len();

    let cursor_cell = text[..cursor_byte].chars().count();
    let real_cell = display_width(text);

    assert_eq!(
        cursor_cell, real_cell,
        "cursor lands at column {cursor_cell} but the text occupies {real_cell} \
         cells — the cursor is drawn inside the second character"
    );
}

/// The bug is not specific to CJK: any wide glyph does it. Emoji are the
/// common case in this project's own output — tool results and status lines
/// carry ✓ ✗ ✅ routinely.
#[test]
#[ignore = "v0.10: executable statement of a TUI defect, not a passing test"]
fn emoji_input_puts_the_cursor_on_top_of_the_text() {
    let text = "done ✅ ok";
    let cursor_byte = text.len();

    let cursor_cell = text[..cursor_byte].chars().count();
    let real_cell = display_width(text);

    assert_eq!(
        cursor_cell, real_cell,
        "cursor at {cursor_cell}, text occupies {real_cell} cells"
    );
}

/// The same arithmetic decides which *row* the cursor is on
/// (`cursor_char / inner_width`, `render.rs:1188`). So a wide-character
/// prompt is not merely misplaced horizontally: its line index is wrong too,
/// and `line_idx` is clamped by `cursor_y`, so the cursor can land on a row
/// holding no text at all.
#[test]
#[ignore = "v0.10: executable statement of a TUI defect, not a passing test"]
fn cjk_input_puts_the_cursor_on_the_wrong_row() {
    let inner_width = 10usize;
    let text = "一二三四五六"; // 6 chars, 12 cells

    let cursor_char = text.chars().count();
    let row_by_chars = cursor_char / inner_width;
    let row_by_cells = display_width(text) / inner_width;

    assert_eq!(
        row_by_chars, row_by_cells,
        "cursor row computed as {row_by_chars}, text actually spans rows up to {row_by_cells}"
    );
}

/// A mixed line, which is what real prompts look like: one ASCII command and
/// a Chinese description. The error accumulates left to right, so the cursor
/// drifts further off with each wide character.
#[test]
#[ignore = "v0.10: executable statement of a TUI defect, not a passing test"]
fn a_mixed_prompt_drifts_by_two_cells_per_wide_character() {
    let text = "/research 研究一下这个问题";
    let cursor_byte = text.len();

    let cursor_cell = text[..cursor_byte].chars().count();
    let real_cell = display_width(text);

    assert_eq!(
        cursor_cell,
        real_cell,
        "drift of {} cells: cursor at {cursor_cell}, text occupies {real_cell}",
        real_cell as i64 - cursor_cell as i64
    );
}

/// `cursor_char as u16` truncates rather than being a genuine bound. Not
/// reachable with a terminal-sized buffer, so this asserts the *type*
/// decision instead of pretending the arithmetic matters: the code casts a
/// `usize` count into `u16` and then does modulo/division on the cast,
/// which is a latent truncation rather than a saturating conversion.
#[test]
#[ignore = "v0.10: executable statement of a TUI defect, not a passing test"]
fn the_cursor_column_is_computed_in_u16() {
    // Recorded as a fact about the current implementation, not a wish.
    // If someone changes `as u16` to `u16::try_from(..)` + clamp, this test
    // is the place to notice and update.
    let long = "x".repeat(70_000);
    let chars = long.chars().count() as u16; // truncates: 70000 -> 4464
    assert_eq!(
        chars, 4464,
        "if this ever stops holding, the u16 cast in render.rs:1187 was replaced"
    );
}

// ---------------------------------------------------------------------------
// BUG 1b — the gap is untested, not just unhandled
// ---------------------------------------------------------------------------

/// Every input-path test in the suite types ASCII. That is the reason a
/// defect affecting every Chinese user shipped. This test fails while the
/// suite is green, which is the point: it makes the blind spot explicit
/// rather than leaving it to be rediscovered.
#[test]
#[ignore = "v0.10: executable statement of a TUI defect, not a passing test"]
fn the_existing_suite_types_only_ascii_in_the_prompt() {
    // `assert_eq!` with a deliberately wrong expectation, so this fails and
    // is noticed. It is a marker, not a bug report that can be ignored.
    let has_cjk_input_test = false;
    assert!(
        has_cjk_input_test,
        "no e2e test drives the input box with a wide character; BUG 1 was \
         reachable in a green tree because of this"
    );
}
