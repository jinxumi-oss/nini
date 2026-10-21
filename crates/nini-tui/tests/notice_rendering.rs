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
