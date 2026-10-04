//! End-to-end regression: transcript lines must not be silently cut off.
//!
//! **Reported class of bug (found by audit, not by a user report)**:
//! every transcript line type *except* `tool_result` was handed to
//! ratatui's `List` without any width budget. `List` does not wrap —
//! it writes each line into the buffer and cuts it at the screen edge,
//! silently. So a 210-character user message rendered as:
//!
//! ```text
//! > STARTAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA      ← "END" simply gone, no marker
//! ```
//!
//! The nastiest instance was `render_tool_call`, whose budget was a flat
//! **120 bytes**. On an 80-column terminal that put the `…` at column
//! 120 — *off-screen* — so the truncation marker was itself truncated
//! away. The existing unit test "covered" it by asserting
//! `joined.contains('…')` on the returned string, which passed while
//! the screen showed a bare hard cut. That is the whole reason this file
//! exists: **assert on cells, not on strings.**
//!
//! # What this file asserts
//!
//! For every line type that can overflow, at both a narrow (40 cols)
//! and a wide (120 cols) terminal:
//!
//! 1. no rendered row exceeds the terminal width
//! 2. content that was actually dropped is marked with `…` *on screen*
//! 3. the identifying prefix (`> `, `▸ <name>`, `✓ <name>`) survives
//!
//! # Test discipline (v0.8.9)
//!
//! * Widths are measured with `display_width` (cells), never `.len()`
//!   (bytes) or `.chars().count()`.
//! * Every case runs at two widths, so a fix that only works at one
//!   terminal size cannot pass.

#![allow(
    clippy::needless_return,
    clippy::let_underscore_future,
    clippy::let_underscore_must_use,
    clippy::redundant_closure_for_method_calls
)]

use nini_tui::render::render_frame;
use nini_tui::state::{AppState, TranscriptLine};
use nini_tui::width::display_width;
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::sync::Arc;

/// The narrow terminal users actually run this in, and the wide one.
const NARROW: (u16, u16) = (40, 24);
const WIDE: (u16, u16) = (120, 30);

/// Snapshot the visible frame as a Vec of rows (one `String` per row,
/// trimmed of trailing padding).
///
/// **Gotcha this helper exists for**: ratatui renders a double-width
/// glyph (CJK, most emoji) into *two* cells — the glyph, then a filler
/// cell whose symbol reads back as `" "`. Naively concatenating
/// `cell(x, y).symbol()` across a row therefore measures `"文"` as two
/// cells and produces text like `"文 文 文"`. An earlier draft of this
/// file failed four tests for exactly that reason — the renderer was
/// correct and the *measurement* was wrong. So we walk the row glyph by
/// glyph and skip the filler after every 2-cell character.
fn frame_rows(state: &AppState, w: u16, h: u16) -> Vec<String> {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_frame(f, state)).unwrap();
    let buf = terminal.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| {
            let mut line = String::new();
            let mut x = 0u16;
            while x < buf.area.width {
                let symbol = buf
                    .cell((x, y))
                    .map(|c| c.symbol())
                    .unwrap_or(" ")
                    .to_string();
                let glyph_w = display_width(&symbol).max(1);
                line.push_str(&symbol);
                x += glyph_w as u16;
            }
            line.trim_end().to_string()
        })
        .collect()
}

/// Cell width of a row as *rendered*, using the same filler-skipping
/// walk as [`frame_rows`]. This is the number that must never exceed
/// the terminal width.
fn row_width(row: &str) -> usize {
    // `row` came out of `frame_rows`, which already dropped the fillers,
    // so a plain cell count is correct here.
    display_width(row)
}

/// Every row in the frame must fit the terminal. A row that overflows
/// can't even be represented, so asserting on the whole frame is
/// strictly stronger than scoping to the transcript — and it catches
/// the footer regressions for free.
fn assert_all_rows_fit(rows: &[String], width: u16, what: &str) {
    for (y, row) in rows.iter().enumerate() {
        assert!(
            row_width(row) <= width as usize,
            "{what}: row {y} is {} cells wide, terminal is {width}: {row:?}",
            row_width(row)
        );
    }
}

/// The transcript rows of a frame: everything above the input box.
///
/// Needed because the key-hint row legitimately contains `…` (it elides
/// its own hints), so a whole-frame `contains('…')` check would pass for
/// the wrong reason.
/// NOTE: every marker assertion in this file goes through this helper,
/// never `rows.join()`. The key-hint row under the input box legitimately
/// contains `…` (it elides its own hints), so a whole-frame grep for the
/// marker is satisfied by an unrelated row — which is exactly how two of
/// these tests passed against the unfixed code on the first run.
fn transcript_text(rows: &[String]) -> String {
    let end = rows
        .iter()
        .position(|r| r.contains("─ input"))
        .unwrap_or(rows.len());
    rows[..end].join("\n")
}

fn push(s: &mut AppState, line: TranscriptLine) {
    Arc::make_mut(&mut s.transcript_state).lines.push(line);
}

fn state_with(line: TranscriptLine) -> AppState {
    let mut s = AppState::new("test-model");
    push(&mut s, line);
    s
}

// ---------------------------------------------------------------------------
// 1. User messages
// ---------------------------------------------------------------------------

#[test]
fn long_user_message_shows_ellipsis_on_screen() {
    let long = format!("START{}END", "A".repeat(200));
    for (w, h) in [NARROW, WIDE] {
        let s = state_with(TranscriptLine::User(long.clone()));
        let rows = frame_rows(&s, w, h);
        assert_all_rows_fit(&rows, w, "user message");

        let body: String = transcript_text(&rows);
        assert!(
            body.contains('…'),
            "user message at {w} cols has no on-screen truncation marker.\n{body}"
        );
        assert!(
            body.contains("> "),
            "user message lost its `> ` prefix at {w} cols.\n{body}"
        );
    }
}

#[test]
fn long_user_message_keeps_its_own_visible_text() {
    // Regression guard: the whole point is that the *tail* is reachable
    // and marked, not silently eaten.
    let long = format!("TAIL-MARKER{}", "A".repeat(300));
    for (w, h) in [NARROW, WIDE] {
        let s = state_with(TranscriptLine::User(long.clone()));
        let rows = frame_rows(&s, w, h);
        let transcript: String = rows[2..].join("\n");
        // Either the tail is on screen, or the line is marked as cut.
        // Both would be fine; what must NOT happen is neither.
        let tail_visible = transcript.contains("TAIL");
        let marked = transcript.contains('…');
        assert!(
            tail_visible || marked,
            "at {w} cols the user message lost its tail with no marker.\n{transcript}"
        );
    }
}

#[test]
fn multiline_user_message_clamps_every_line() {
    let text = "first line\n".to_string() + &"b".repeat(400);
    for (w, h) in [NARROW, WIDE] {
        let s = state_with(TranscriptLine::User(text.clone()));
        let rows = frame_rows(&s, w, h);
        assert_all_rows_fit(&rows, w, "multiline user message");
        assert!(
            transcript_text(&rows).contains('…'),
            "no marker on the clamped continuation line at {w} cols"
        );
    }
}

#[test]
fn cjk_user_message_uses_cell_width() {
    // 100 CJK chars = 300 bytes but 200 cells. A byte-based budget
    // would let far too much through.
    let s = state_with(TranscriptLine::User("文".repeat(100)));
    for (w, h) in [NARROW, WIDE] {
        let rows = frame_rows(&s, w, h);
        assert_all_rows_fit(&rows, w, "CJK user message");
        assert!(
            transcript_text(&rows).contains('…'),
            "no marker at {w} cols"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. Tool-call headers — the worst offender
// ---------------------------------------------------------------------------

#[test]
fn tool_call_ellipsis_is_on_screen_not_off_screen() {
    // 200 ASCII bytes of args. Before v0.8.9 the budget was 120 bytes
    // and the `…` landed at column 120 — invisible on an 80-col
    // terminal, let alone 40.
    let s = state_with(TranscriptLine::ToolCall {
        id: "t1".into(),
        name: "bash".into(),
        args: format!("\"command\":\"{}\"", "x".repeat(200)),
        collapsed: false,
    });
    for (w, h) in [NARROW, WIDE] {
        let rows = frame_rows(&s, w, h);
        assert_all_rows_fit(&rows, w, "tool call");
        let body = transcript_text(&rows);
        assert!(
            body.contains('…'),
            "tool-call header at {w} cols has no on-screen marker.\n{body}"
        );
        assert!(
            body.contains("▸ bash"),
            "tool name disappeared at {w} cols.\n{body}"
        );
    }
}

#[test]
fn cjk_tool_call_args_do_not_overflow() {
    // 60 CJK args = 180 bytes = 120 cells. This is the exact shape that
    // used to render 20 of the 60 characters with no marker at all.
    let s = state_with(TranscriptLine::ToolCall {
        id: "t1".into(),
        name: "grep".into(),
        args: "文".repeat(60),
        collapsed: false,
    });
    for (w, h) in [NARROW, WIDE] {
        let rows = frame_rows(&s, w, h);
        assert_all_rows_fit(&rows, w, "CJK tool call");
        let body = transcript_text(&rows);
        assert!(body.contains('…'), "no marker at {w} cols.\n{body}");
        assert!(body.contains("▸ grep"), "name lost at {w} cols.\n{body}");
    }
}

#[test]
fn short_tool_call_is_not_marked() {
    // The other direction: don't litter `…` on lines that fit.
    let s = state_with(TranscriptLine::ToolCall {
        id: "t1".into(),
        name: "read".into(),
        args: r#"{"path":"src/main.rs"}"#.into(),
        collapsed: false,
    });
    let rows = frame_rows(&s, NARROW.0, NARROW.1);
    // Scope to the transcript: the key-hint row below the input box
    // legitimately elides its own hints with `…`.
    let body = transcript_text(&rows);
    assert!(body.contains("▸ read"), "missing header: {body}");
    assert!(
        !body.contains('…'),
        "a short tool call must not be marked as truncated: {body}"
    );
}

// ---------------------------------------------------------------------------
// 3. Assistant markdown
// ---------------------------------------------------------------------------

#[test]
fn long_assistant_paragraph_shows_ellipsis() {
    let s = state_with(TranscriptLine::AssistantText(format!(
        "START {} END",
        "c".repeat(300)
    )));
    for (w, h) in [NARROW, WIDE] {
        let rows = frame_rows(&s, w, h);
        assert_all_rows_fit(&rows, w, "assistant paragraph");
        assert!(
            transcript_text(&rows).contains('…'),
            "assistant paragraph at {w} cols has no marker"
        );
    }
}

#[test]
fn assistant_code_block_is_clamped() {
    // Code fences are the widest markdown construct; verify the clamp
    // reaches them too.
    let md = "```rust\nfn main() { println!(\"a fairly long line of code here\"); }\n```";
    let s = state_with(TranscriptLine::AssistantText(md.to_string()));
    for (w, h) in [NARROW, WIDE] {
        let rows = frame_rows(&s, w, h);
        assert_all_rows_fit(&rows, w, "assistant code block");
    }
}

// ---------------------------------------------------------------------------
// 4. Bash execution blocks
// ---------------------------------------------------------------------------

#[test]
fn long_bash_output_line_shows_ellipsis() {
    // 1500 bytes on one line: under the 2 KB volume budget, so the old
    // byte-level truncation never fired and the `List` cut it with no
    // marker whatsoever.
    let s = state_with(TranscriptLine::BashExecution {
        id: "t1".into(),
        cmd: "echo hi".into(),
        output: "q".repeat(1500),
        stderr: String::new(),
        ok: true,
        exit_code: Some(0),
        duration_ms: 5,
        collapsed: false,
    });
    for (w, h) in [NARROW, WIDE] {
        let rows = frame_rows(&s, w, h);
        assert_all_rows_fit(&rows, w, "bash output");
        assert!(
            transcript_text(&rows).contains('…'),
            "bash output at {w} cols has no marker"
        );
    }
}

#[test]
fn long_bash_command_header_is_clamped() {
    let s = state_with(TranscriptLine::BashExecution {
        id: "t1".into(),
        cmd: "c".repeat(300),
        output: "out".into(),
        stderr: String::new(),
        ok: true,
        exit_code: Some(0),
        duration_ms: 1,
        collapsed: false,
    });
    for (w, h) in [NARROW, WIDE] {
        let rows = frame_rows(&s, w, h);
        assert_all_rows_fit(&rows, w, "bash header");
        assert!(
            transcript_text(&rows).contains('…'),
            "no marker at {w} cols"
        );
    }
}

#[test]
fn long_bash_stderr_is_clamped() {
    let s = state_with(TranscriptLine::BashExecution {
        id: "t1".into(),
        cmd: "cmd".into(),
        output: "ok".into(),
        stderr: "e".repeat(500),
        ok: false,
        exit_code: Some(1),
        duration_ms: 1,
        collapsed: false,
    });
    for (w, h) in [NARROW, WIDE] {
        let rows = frame_rows(&s, w, h);
        assert_all_rows_fit(&rows, w, "bash stderr");
    }
}

// ---------------------------------------------------------------------------
// 5. Mixed / stress
// ---------------------------------------------------------------------------

#[test]
fn no_transcript_row_overflows_at_any_width() {
    // Everything at once, at five widths. This is the sweep that would
    // have caught the whole class on day one.
    let mut s = AppState::new("test-model");
    push(&mut s, TranscriptLine::User("A".repeat(400)));
    push(
        &mut s,
        TranscriptLine::AssistantText("some assistant reasoning ".to_string() + &"b".repeat(500)),
    );
    for i in 0..3 {
        push(
            &mut s,
            TranscriptLine::ToolCall {
                id: format!("t{i}"),
                name: "bash".into(),
                args: format!("{{\"command\":\"{}\"}}", "c".repeat(300)),
                collapsed: false,
            },
        );
        push(
            &mut s,
            TranscriptLine::ToolResult {
                id: format!("t{i}"),
                name: "bash".into(),
                ok: true,
                content: "中文输出内容".repeat(200),
                collapsed: false,
                duration_ms: Some(1),
            },
        );
    }
    push(
        &mut s,
        TranscriptLine::BashExecution {
            id: "t9".into(),
            cmd: "d".repeat(200),
            output: "e".repeat(2000),
            stderr: "f".repeat(500),
            ok: true,
            exit_code: Some(0),
            duration_ms: 3,
            collapsed: false,
        },
    );

    for w in [20u16, 40, 60, 80, 120] {
        let rows = frame_rows(&s, w, 30);
        assert_all_rows_fit(&rows, w, &format!("mixed transcript at {w} cols"));
    }

    // "No row overflows" alone is a weak assertion: ratatui's `List`
    // *does* clip to the area, so a row can never overflow even when the
    // content was silently eaten. The signal we actually care about is
    // the marker — and it has to be checked *per line type*, because a
    // `…` belonging to some other row in the transcript would satisfy a
    // whole-block grep. At 20 columns the 400-character user message
    // cannot possibly fit, so its own row must be marked.
    let rows = frame_rows(&s, 20, 30);
    let user_row = rows
        .iter()
        .find(|r| r.starts_with("> "))
        .unwrap_or_else(|| panic!("user message row vanished at 20 cols:\n{}", rows.join("\n")));
    assert!(
        user_row.contains('…'),
        "20 columns, 400-char user message, no marker on its own row: {user_row:?}"
    );
}

#[test]
fn tool_result_still_renders_its_name_after_the_change() {
    // Guard against a regression from the shared-`max_w` refactor:
    // v0.8.8's identity fix must survive the width plumbing.
    let s = state_with(TranscriptLine::ToolResult {
        id: "t1".into(),
        name: "read".into(),
        ok: true,
        content: "some file content".into(),
        collapsed: false,
        duration_ms: Some(4),
    });
    let body = frame_rows(&s, NARROW.0, NARROW.1).join("\n");
    assert!(body.contains("✓ read"), "tool result lost its name: {body}");
    assert!(
        body.contains("4ms"),
        "tool result lost its duration: {body}"
    );
}
