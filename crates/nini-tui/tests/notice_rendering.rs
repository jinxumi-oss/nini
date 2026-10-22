//! A notice is a line the user reads. It has to be readable, and it has
//! to be safe to export.
//!
//! invariant: cc2e35ff —— 声明本文件守住了哪几条历史不变量。
//! 该标注让 scripts/invariants.py 认出这些断言；词法重叠看不出它们，
//! 真正的证据是 docs/invariant-verification.md 里的注入-观察记录。
//!
//! Two defects, both found by the second-round review:
//!
//! * a notice was drawn as **one** string in a `List`, which clips
//!   rather than wraps — so the tail of a long diagnostic was never on
//!   screen. A notice that ends mid-word looks exactly like the
//!   corruption the terminal-guard work was about.
//! * a notice was written into the HTML export **unescaped**, and a
//!   notice can carry model output (the `/copy` fallback puts the
//!   assistant's text in one) — so a `<script>` in a model answer
//!   became markup in the exported file.

//! invariant: 655db2d
//! invariant: a87797f1
use nini_tui::render::render_frame;
use nini_tui::state::AppState;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

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

#[test]
fn a_long_notice_is_wrapped_not_clipped() {
    let mut state = AppState::new("test-model");
    let tail = "THE-TAIL-OF-THE-DIAGNOSTIC";
    let body = format!("the head of the diagnostic, and then {tail}");
    state.push_notice(body);

    let rows = frame_rows(&state, 60, 24);
    let screen = rows.join("\n");

    assert!(
        screen.contains(tail),
        "the end of a long notice must be on screen — a notice that stops \
         mid-word reads as garbage:\n{screen}"
    );
}

#[test]
fn a_notice_in_the_export_is_escaped() {
    let mut state = AppState::new("test-model");
    state.push_notice("model said: use Vec<String> here <script>alert('xss')</script>");

    let html =
        nini_tui::commands::html_export::render_transcript_html(&state.transcript_state.lines);

    assert!(
        !html.contains("<script>"),
        "model output must not become markup in the export:\n{html}"
    );
    assert!(
        html.contains("&lt;script&gt;"),
        "the angle brackets must be escaped, not dropped:\n{html}"
    );
}

#[test]
fn a_notice_keeps_its_own_text() {
    // The notice is the user's evidence that something happened; it has
    // to say what.
    let mut state = AppState::new("test-model");
    state.push_notice("[nini] trust: cwd '/x' is at Ask level");

    let screen = frame_rows(&state, 100, 24).join("\n");
    assert!(
        screen.contains("trust: cwd '/x' is at Ask level"),
        "the notice must be on screen verbatim:\n{screen}"
    );
}

/// A notice whose body is full of ambiguous-width characters must not
/// be wider than the screen either.
///
/// `→`, `…`, `✓` and `•` are East-Asian-Ambiguous: one column in a
/// western terminal, two in a CJK one. A library diagnostic is full of
/// them (`error: expected one of ..., found →`), and the wrapping
/// measures with the western width — so the row came out one column
/// per ambiguous character too wide in a CJK terminal, and the
/// transcript's `List` cut the tail off.
///
/// The fix budgets the extra columns the body will take in a CJK
/// terminal. The test measures the row the way a CJK terminal would.
#[test]
fn a_notice_with_ambiguous_characters_fits_a_cjk_terminal() {
    let mut state = AppState::new("test-model");
    let tail = "THE-END-OF-THE-NOTE";
    // A body made of ambiguous characters: every one of them takes two
    // columns in a CJK terminal.
    let body = format!("{} {tail}", "→…✓•·".repeat(12));
    state.push_notice(body);

    let width = 60u16;
    let rows = frame_rows(&state, width, 24);

    let mut measured = 0;
    for (n, row) in rows.iter().enumerate() {
        if !row.contains('→') && !row.contains(tail) {
            continue;
        }
        measured += 1;
        let text = row.trim_end();
        let columns: usize = text.chars().map(cjk_columns).sum();
        assert!(
            columns <= width as usize,
            "row {n} is {columns} columns wide in a CJK terminal, the screen \
             is {width}:\n{row:?}"
        );
    }
    assert!(
        measured >= 2,
        "the notice should occupy more than one row — the test did not \
         measure what it meant to measure"
    );

    let screen = rows.join("\n");
    assert!(
        screen.contains(tail),
        "the tail of the notice must survive the CJK budget:\n{screen}"
    );
}

/// How many columns a character takes in a CJK terminal: two for the
/// East-Asian-Ambiguous ones, the usual width otherwise.
fn cjk_columns(c: char) -> usize {
    use unicode_width::UnicodeWidthChar;
    match UnicodeWidthChar::width(c) {
        Some(1) => {
            // `unicode-width` says one column. Is the character one of
            // the ambiguous ones? `width_cjk` says two.
            if unicode_width::UnicodeWidthChar::width_cjk(c) == Some(2) {
                2
            } else {
                1
            }
        }
        Some(w) => w,
        None => 0,
    }
}

/// A notice row must not be wider than the screen it is drawn on.
///
/// The warning sign is East-Asian-Ambiguous: one cell in a western
/// terminal, two in a CJK one. The wrapping budget assumed four
/// columns for the prefix, so in a CJK terminal the row was one column
/// too wide — and the transcript's `List` clips rather than wraps, so
/// the tail of a long diagnostic disappeared again.
///
/// The test measures the rows the way a CJK terminal would: the
/// ambiguous sign counts as two columns.
#[test]
fn a_notice_row_is_never_wider_than_the_screen() {
    let mut state = AppState::new("test-model");
    let tail = "THE-END-OF-THE-NOTE";
    // The body is chosen so that the **first** wrapped line is exactly
    // as long as the budget allows: that is the line that carries the
    // warning sign, and it is the line that overflows when the budget
    // is one column too generous. A body that wraps earlier would
    // leave slack and hide the off-by-one.
    state.push_notice(format!("{} {} {tail}", "A".repeat(56), "B".repeat(40)));

    let width = 60u16;
    let rows = frame_rows(&state, width, 24);

    // Only the rows that carry the notice are measured: the rest of
    // the screen (status bar, input box) has its own layout, and its
    // own ambiguous-width characters.
    let mut measured = 0;
    for (n, row) in rows.iter().enumerate() {
        if !row.contains('A') && !row.contains('B') && !row.contains(tail) {
            continue;
        }
        measured += 1;
        // The trailing padding is not content: it is the rest of the
        // row. What matters is how wide the *text* is.
        let text = row.trim_end();
        let columns: usize = text.chars().map(cjk_columns).sum();
        assert!(
            columns <= width as usize,
            "row {n} is {columns} columns wide, the screen is {width}:\n{row:?}"
        );
    }
    assert!(
        measured >= 2,
        "the notice should occupy more than one row — the test did not \
         measure what it meant to measure"
    );

    // And nothing was cut: the point of the budget is to fit, not to
    // lose a character. A row that is one column too wide is clipped
    // by the `List`, and the clipped character is gone for good.
    let notice_rows: Vec<&String> = rows
        .iter()
        .filter(|r| r.contains('A') || r.contains('B') || r.contains(tail))
        .collect();
    let visible_x: usize = notice_rows
        .iter()
        .map(|r| r.chars().filter(|c| *c == 'A' || *c == 'B').count())
        .sum();
    let screen = notice_rows
        .iter()
        .map(|r| r.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        visible_x, 96,
        "the notice lost characters: {visible_x} of 96 are on screen:\n{screen}"
    );
    assert!(
        screen.contains(tail),
        "the tail of the notice must survive the narrower budget:\n{screen}"
    );
}
