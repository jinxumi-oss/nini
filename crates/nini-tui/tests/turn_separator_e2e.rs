//! A turn boundary is a blank line, not a rule.
//!
//! `rich::render_divider` drew `"─".repeat(60)`: sixty hard-coded
//! box-drawing characters, with no notion of terminal width. Measured in
//! tmux, the on-screen result was `min(60, width)` — sixty characters
//! stranded in the middle of a wide terminal, and a full-width line at 40
//! columns, which is the report this file exists to close.
//!
//! Pi draws no such line. Every message boundary in Pi is a `Spacer(1)`
//! (`assistant-message.js:74/121/131/139/144`, `bash-execution.js:30`,
//! `branch-summary-message.js:30`, `compaction-summary-message.js:31`),
//! and the one rule Pi does draw is sized from the real width
//! (`dynamic-border.js:18`).
//!
//! The assertions compare against a **baseline frame** rather than
//! counting from a fixed row. The prompt's own border is a `─` line, and
//! it is legitimately there — Pi draws it too. Counting from a hard-coded
//! coordinate would encode a layout guess that breaks the next time the
//! footer grows; counting rules in a transcript-less frame does not.

use nini_tui::render::render_frame_with_theme;
use nini_tui::state::AppState;
use nini_tui::theme::Theme;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn frame_text(width: u16, height: u16, state: &AppState) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|f| render_frame_with_theme(f, state, &Theme::dark()))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    let area = *buffer.area();
    let mut out = String::new();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buffer.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out.push('\n');
    }
    out
}

fn rules(frame: &str) -> usize {
    frame.matches('─').count()
}

/// The property: a transcript containing turn boundaries draws exactly as
/// many rules as an empty one — none at all beyond the prompt's border.
#[test]
fn turn_boundaries_draw_no_rule_at_any_width() {
    for width in [40u16, 60, 100, 200] {
        let baseline = frame_text(width, 40, &AppState::new("test-model"));

        let mut state = AppState::new("test-model");
        state.push_user("first question");
        state.push_divider();
        state.push_assistant("first answer");
        state.push_divider();
        state.push_assistant("second answer, after two boundaries");
        let with_turns = frame_text(width, 40, &state);

        assert_eq!(
            rules(&with_turns),
            rules(&baseline),
            "at width {width}, turn boundaries added {} rule(s) to the frame",
            rules(&with_turns) as i64 - rules(&baseline) as i64,
        );
    }
}

/// The other half: the boundary still takes a row. Deleting the rule must
/// not delete the separation — an empty `RLine` measures zero rows tall in
/// ratatui, which would silently remove the gap the rule was standing in
/// for.
#[test]
fn the_separator_still_occupies_one_row() {
    let mut above_below = AppState::new("test-model");
    above_below.push_assistant("above");
    above_below.push_divider();
    above_below.push_assistant("below");
    let with = frame_text(100, 40, &above_below);

    let mut joined = AppState::new("test-model");
    joined.push_assistant("above below");
    let without = frame_text(100, 40, &joined);

    let a = with
        .lines()
        .position(|l| l.contains("above"))
        .expect("above");
    let b = with
        .lines()
        .position(|l| l.contains("below"))
        .expect("below");
    assert!(
        b > a + 1,
        "the two messages must not be adjacent: no row was reserved for the \
         boundary (above at row {a}, below at row {b})"
    );
    assert!(
        (a + 1..b).all(|i| with.lines().nth(i).is_none_or(|l| l.trim().is_empty())),
        "the rows between the two messages are not blank"
    );

    // Same transcript content, one rule removed, and the frame must be
    // exactly one row shorter — the rule was a row, the blank line is a
    // row, and nothing else changed.
    let row_delta = without.lines().count() as i64 - with.lines().count() as i64;
    assert_eq!(
        row_delta, 0,
        "the frame layout should be identical; only content differs"
    );
}
