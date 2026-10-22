//! A frame must not be able to break the terminal it is drawn on.
//!
//! # What this checks
//!
//! A TUI writes a *grid* of cells, but the terminal it draws on is a
//! stream of bytes with its own idea of how wide a character is. Two
//! things can go wrong that a cell-grid test never sees:
//!
//! * **Overflow.** If the cells of a row add up to more columns than
//!   the terminal has, the terminal wraps the row — and every row below
//!   it moves down by one. The screen is then shifted by one line for
//!   the rest of the session: that is the "the screen went wrong"
//!   report, and it is invisible in a grid test (the grid is always
//!   exactly the terminal's width).
//! * **A wide character in the last column.** The character needs two
//!   columns and only one is left: the terminal wraps *inside* the
//!   character.
//!
//! So the assertion is about the *rendered frame*, measured the way a
//! terminal measures it, and compared against the width nini was told
//! the terminal has.

use nini_tui::render::render_frame;
use nini_tui::state::AppState;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// The width of a row, the way a terminal counts it.
///
/// Two subtleties, and both of them have bitten a measurement before:
///
/// * A wide character (CJK, emoji) takes **two** columns. The grid
///   stores it in one cell and resets the next one to a space — that
///   space is the *second half* of the character, not a column of its
///   own, so it must not be counted again.
/// * `unicode-width` is what the renderer used to lay the frame out; a
///   terminal that treats East-Asian *ambiguous* characters as double
///   width is the harshest case, so the check assumes that.
fn terminal_width(s: &str) -> usize {
    use unicode_segmentation::UnicodeSegmentation;
    use unicode_width::UnicodeWidthStr;
    let mut width = 0usize;
    let mut previous_wide = false;
    // Graphemes, not characters: a ZWJ emoji sequence is **one** glyph
    // that takes two columns, and a terminal counts it as two.
    for g in s.graphemes(true) {
        if previous_wide && g == " " {
            // The second half of the wide character before it.
            previous_wide = false;
            continue;
        }
        let w = UnicodeWidthStr::width(g);
        width += w;
        previous_wide = w == 2;
    }
    width
}

/// Every cell of the frame, row by row.
fn frame_cells(state: &AppState, w: u16, h: u16) -> Vec<Vec<String>> {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| render_frame(f, state)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| {
                    buffer
                        .cell((x, y))
                        .map(|c| c.symbol().to_string())
                        .unwrap_or_else(|| " ".to_string())
                })
                .collect()
        })
        .collect()
}

/// The frame must not be wider than the terminal, on any row.
///
/// A row that is too wide wraps in the terminal, and every row below it
/// is pushed down by one — the screen is then wrong in a way the grid
/// cannot show.
fn assert_no_overflow(state: &AppState, w: u16, h: u16, what: &str) {
    let rows = frame_cells(state, w, h);
    for (y, row) in rows.iter().enumerate() {
        let text: String = row.concat();
        let width = terminal_width(&text);
        assert!(
            width <= w as usize,
            "{what}: row {y} is {width} cells wide, the terminal has {w} — it would \
             wrap, and every row below it would shift down by one:\n{text:?}"
        );
    }
}

/// The input box's right border must be the last column.
///
/// The input box is the one thing the user looks at while a task runs,
/// and its border is what shows the frame is aligned.
fn assert_input_box_aligned(state: &AppState, w: u16, h: u16, what: &str) {
    let rows = frame_cells(state, w, h);
    // The input box is the one bordered block with a title: find the row
    // that carries "─ input ─" (a table's top border has no title, and
    // picking that one would measure the wrong thing).
    let top = rows
        .iter()
        .position(|r| r.concat().contains("input"))
        .unwrap_or_else(|| panic!("{what}: no input box on screen"));
    let row: String = rows[top].concat();
    let last = row.chars().last().unwrap_or(' ');
    assert_eq!(
        last,
        '┐',
        "{what}: the input box's right border is at column {} of {w}, not the last — \
         the frame is not aligned:\n{row:?}",
        row.chars().count() - 1
    );
}

/// Content that a model can plausibly produce, and that a terminal
/// handles in a way the renderer has to respect.
fn hostile_bodies() -> Vec<(&'static str, String)> {
    vec![
        ("plain ascii", "hello world".to_string()),
        ("cjk", "统计结果：二百一十三个源文件。".to_string()),
        (
            "cjk + ascii mix",
            "crates/nini-tui: 106 个文件，nini-core: 31。".to_string(),
        ),
        ("emoji", "done ✅ 🚀 🎉".to_string()),
        ("family emoji (ZWJ)", "👨‍👩‍👧‍👦 family".to_string()),
        ("combining marks", "e\u{0301}clair cafe\u{0301}".to_string()),
        ("ambiguous width", "→ ↑ ↓ • ░ ▓ │ ─ ┌ ┐".to_string()),
        ("fullwidth", "ＦＵＬＬＷＩＤＴＨ　テキスト".to_string()),
        ("rtl", "שלום עולם mixed".to_string()),
        ("tab", "col1\tcol2\tcol3".to_string()),
        ("ansi escape", "\u{1b}[31mred\u{1b}[0m plain".to_string()),
        ("long unbroken", "x".repeat(400)),
        ("cjk long unbroken", "字".repeat(200)),
        (
            "mixed table",
            "| 项目 | 数量 | 说明 |\n|---|---|---|\n| crates | 213 | 说明 |".to_string(),
        ),
    ]
}

#[test]
fn no_content_can_push_the_frame_past_the_terminal_width() {
    for width in [60u16, 80, 100, 120] {
        for (name, body) in hostile_bodies() {
            let mut state = AppState::new("test-model");
            state.push_user(body.clone());
            state.push_assistant(body.clone());
            assert_no_overflow(&state, width, 24, &format!("{name} @ {width}"));
            assert_input_box_aligned(&state, width, 24, &format!("{name} @ {width}"));
        }
    }
}

#[test]
fn no_content_can_put_a_replacement_character_on_screen() {
    for (name, body) in hostile_bodies() {
        let mut state = AppState::new("test-model");
        state.push_user(body.clone());
        state.push_assistant(body.clone());
        for row in frame_cells(&state, 100, 24) {
            let text: String = row.concat();
            assert!(
                !text.contains('\u{fffd}'),
                "{name}: the frame carries a replacement character (U+FFFD) — a byte \
                 sequence that was cut in the middle of a character:\n{text:?}"
            );
        }
    }
}
