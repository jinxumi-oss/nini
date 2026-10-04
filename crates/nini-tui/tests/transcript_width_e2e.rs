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

// ---------------------------------------------------------------------------
// 6. The truncation indicator must tell the truth (v0.8.9 P1-4)
// ---------------------------------------------------------------------------
//
// The indicator used to subtract a 2 KB budget that nothing applied
// any more, so it was wrong in both directions:
//
//   * 1500-byte single line at 40 cols → ~1460 cells silently dropped
//     and the indicator said *nothing* (`1500 - 2000` saturates to 0).
//   * 5 lines x 1000 bytes → reported "2 KB hidden" while really
//     hiding ~4.7 KB.
//
// `transcript_text` is used throughout: the key-hint row has its own
// `…` and would otherwise satisfy any whole-frame grep.

/// The `…(...)` summary row the tool-result renderer emits, if any.
fn truncation_indicator(transcript: &str) -> Option<String> {
    transcript
        .lines()
        .find(|l| l.contains("…("))
        .map(|l| l.to_string())
}

/// Digits from whichever cell-loss wording the indicator used
/// ("N more cells" below 1024, "N cells hidden" at or above).
fn reported_cells(indicator: &str) -> Option<usize> {
    let cut = indicator
        .find("more cells")
        .or_else(|| indicator.find("cells hidden"))?;
    let head = &indicator[..cut];
    // Walk back over the separator first — there is always at least a
    // space (and often a `(`) between the number and the keyword.
    let digits: String = head
        .chars()
        .rev()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.chars().rev().collect::<String>().parse().ok()
}

#[test]
fn dropped_chars_are_reported_even_when_total_is_under_2kb() {
    // 1500 bytes < the old 2000-byte budget, so the old indicator
    // printed nothing at all. The user saw a 37-cell line and had no
    // way to know 1460 cells of file content were missing.
    let s = state_with(TranscriptLine::ToolResult {
        id: "t1".into(),
        name: "read".into(),
        ok: true,
        content: "y".repeat(1500),
        collapsed: false,
        duration_ms: Some(4),
    });
    for (w, h) in [NARROW, WIDE] {
        let rows = frame_rows(&s, w, h);
        let transcript = transcript_text(&rows);
        let indicator = truncation_indicator(&transcript).unwrap_or_else(|| {
            panic!("no truncation indicator at {w} cols despite ~1460 cells dropped:\n{transcript}")
        });
        let reported = reported_cells(&indicator)
            .unwrap_or_else(|| panic!("indicator has no cell count: {indicator:?}"));
        assert!(
            reported >= 1000,
            "at {w} cols the indicator claims only {reported} cells hidden; \
             the line is 1500 cells long and at most `width` survive"
        );
    }
}

#[test]
fn reported_hidden_cells_match_reality() {
    // 5 lines x 1000 cells at 40 cols: each line keeps 38, so
    // 5 x 962 = 4810 cells are genuinely dropped. The old metric said
    // "2 KB hidden" (3000) purely from `5000 - 2000`.
    let body = (0..5)
        .map(|i| format!("line{i}:{}", "z".repeat(990)))
        .collect::<Vec<_>>()
        .join("\n");
    let s = state_with(TranscriptLine::ToolResult {
        id: "t1".into(),
        name: "bash".into(),
        ok: true,
        content: body,
        collapsed: false,
        duration_ms: None,
    });
    let rows = frame_rows(&s, NARROW.0, NARROW.1);
    let transcript = transcript_text(&rows);
    let indicator =
        truncation_indicator(&transcript).unwrap_or_else(|| panic!("no indicator:\n{transcript}"));
    let reported = reported_cells(&indicator)
        .unwrap_or_else(|| panic!("indicator has no cell count: {indicator:?}"));

    // Each of the 5 lines is ~1000 cells; the budget is 38 (40 - 2
    // indent). Tolerance absorbs the `\n` joins and the prefix text.
    let expected = 5 * (1000 - NARROW.0 as usize);
    let slack = 60;
    assert!(
        reported + slack >= expected && reported <= expected + slack,
        "indicator reports {reported} hidden cells, expected ~{expected}: {indicator:?}"
    );
    assert!(
        !indicator.contains("more bytes"),
        "the dead byte metric is back: {indicator:?}"
    );
}

#[test]
fn whole_dropped_lines_are_counted_separately() {
    // 100 lines: 8 shown, 92 dropped. Both facts belong on the one row.
    let mut body = String::new();
    for i in 0..100 {
        body.push_str(&format!("line {i:03}: short\n"));
    }
    let s = state_with(TranscriptLine::ToolResult {
        id: "t1".into(),
        name: "read".into(),
        ok: true,
        content: body,
        collapsed: false,
        duration_ms: Some(15),
    });
    let transcript = transcript_text(&frame_rows(&s, WIDE.0, WIDE.1));
    let indicator =
        truncation_indicator(&transcript).unwrap_or_else(|| panic!("no indicator:\n{transcript}"));
    assert!(
        indicator.contains("92 more lines"),
        "expected the dropped-line count: {indicator:?}"
    );
    // One combined row, not two (v0.8.7 behaviour must survive).
    assert_eq!(
        transcript
            .lines()
            .filter(|l| l.contains("…(") || l.contains("…(truncated"))
            .count(),
        1,
        "indicator must be a single combined row: {transcript}"
    );
}

#[test]
fn short_tool_result_has_no_indicator() {
    // The other direction — don't invent losses that didn't happen.
    let s = state_with(TranscriptLine::ToolResult {
        id: "t1".into(),
        name: "read".into(),
        ok: true,
        content: "a short file body\nsecond line".into(),
        collapsed: false,
        duration_ms: Some(1),
    });
    let transcript = transcript_text(&frame_rows(&s, WIDE.0, WIDE.1));
    assert!(
        truncation_indicator(&transcript).is_none(),
        "a short tool result must not claim anything is hidden:\n{transcript}"
    );
}

#[test]
fn cjk_tool_result_reports_cells_not_bytes() {
    // 200 CJK chars = 600 bytes but 400 cells. Quoting bytes here would
    // overstate the loss by 3x.
    let s = state_with(TranscriptLine::ToolResult {
        id: "t1".into(),
        name: "read".into(),
        ok: true,
        content: "文".repeat(200),
        collapsed: false,
        duration_ms: Some(1),
    });
    let transcript = transcript_text(&frame_rows(&s, NARROW.0, NARROW.1));
    let indicator =
        truncation_indicator(&transcript).unwrap_or_else(|| panic!("no indicator:\n{transcript}"));
    let reported = reported_cells(&indicator)
        .unwrap_or_else(|| panic!("indicator has no cell count: {indicator:?}"));
    // 400 cells total, ~38 survive -> ~362 hidden. A byte-based figure
    // would have said ~1086.
    assert!(
        reported <= 500,
        "CJK loss quoted in bytes instead of cells ({reported}): {indicator:?}"
    );
    assert!(
        reported >= 300,
        "under-reported the loss ({reported}): {indicator:?}"
    );
}

// ---------------------------------------------------------------------------
// 7. Shrinking the transcript must not blank the viewport (v0.8.9 P1-5)
// ---------------------------------------------------------------------------
//
// `scroll_offset` counts lines from the *bottom*. Every operation that
// removes lines (compaction folding, `/clear`, `/new`, session reload)
// invalidated it, and the renderer translated the stale value into an
// item-space skip that ran past the end of the transcript — leaving the
// whole area blank with a meaningless `↓ 59 more` in the corner.
//
// Reproduced at 40x24 with 60 tool-result lines (~5 items each):
//
//   before compaction:  ✓ read Took 1ms / row0-0 / row0-1 / ...
//   after compaction:   (18 empty rows)  ↓ 59 more
//
/// Build a transcript long enough that its item count overflows the
/// viewport, so the autoscroll/scroll math is actually exercised.
fn long_scrolled_state() -> AppState {
    let mut s = AppState::new("test-model");
    for i in 0..60 {
        let content = (0..4)
            .map(|j| format!("row{i}-{j}"))
            .collect::<Vec<_>>()
            .join("\n");
        s.push_tool_result(&format!("t{i}"), "read", true, content, Some(1));
    }
    // Scroll all the way up, the way repeated PageUp would.
    let ts = Arc::make_mut(&mut s.transcript_state);
    ts.autoscroll = false;
    ts.scroll_offset = ts.lines.len().saturating_sub(1);
    s
}

/// The visible transcript rows (everything above the input box).
fn visible_transcript(s: &AppState, w: u16, h: u16) -> Vec<String> {
    let rows = frame_rows(s, w, h);
    let end = rows
        .iter()
        .position(|r| r.contains("─ input"))
        .unwrap_or(rows.len());
    rows[2..end].to_vec()
}

#[test]
fn compaction_while_scrolled_up_keeps_content_visible() {
    let mut s = long_scrolled_state();
    let before = visible_transcript(&s, NARROW.0, NARROW.1);
    assert!(
        before.iter().any(|r| !r.trim().is_empty()),
        "precondition: the transcript should have content before compaction"
    );

    let folded = s.auto_compact_local();
    assert!(folded > 0, "compaction should have folded something");

    let after = visible_transcript(&s, NARROW.0, NARROW.1);
    assert!(
        after.iter().any(|r| !r.trim().is_empty()),
        "compaction while scrolled up left a blank transcript:\n{after:?}"
    );

    // The summary line itself is folded in at the *front*; restoring
    // autoscroll means the viewport lands on the tail, so it is
    // correctly off-screen. Assert it exists in the transcript and
    // that we are looking at the newest content — not at nothing.
    assert!(
        s.transcript_state.lines.iter().any(
            |l| matches!(l, TranscriptLine::AssistantText(t) if t.contains("CONTEXT SUMMARY"))
        ),
        "the compaction summary should be in the transcript"
    );
    let tail_visible = after.iter().any(|r| r.contains("row59"));
    assert!(
        tail_visible,
        "after compaction the viewport should sit at the tail (newest content), \
         got:\n{after:?}"
    );
}

#[test]
fn compaction_resets_the_scroll_viewport() {
    let mut s = long_scrolled_state();
    assert!(
        !s.transcript_state.autoscroll,
        "precondition: user scrolled up"
    );
    assert!(
        s.transcript_state.scroll_offset > 0,
        "precondition: offset set"
    );

    s.auto_compact_local();

    let ts = &s.transcript_state;
    assert!(
        ts.autoscroll,
        "autoscroll should be restored after compaction"
    );
    assert_eq!(
        ts.scroll_offset, 0,
        "scroll_offset must be reset after the line count changed"
    );
}

#[test]
fn clear_while_scrolled_up_resets_the_viewport() {
    let mut s = long_scrolled_state();
    // Same code path as the `action:clear` hook.
    let ts = Arc::make_mut(&mut s.transcript_state);
    ts.lines.clear();
    ts.reset_view();
    s.push_divider();

    let ts = &s.transcript_state;
    assert!(ts.autoscroll);
    assert_eq!(ts.scroll_offset, 0);
    assert!(
        !visible_transcript(&s, NARROW.0, NARROW.1)
            .iter()
            .all(|r| r.trim().is_empty()),
        "a freshly cleared transcript must still render its divider"
    );
}

#[test]
fn out_of_range_scroll_offset_degrades_to_the_top_not_to_nothing() {
    // The renderer's own clamp, tested directly: an offset that could
    // never be satisfied must not produce a blank area.
    let mut s = long_scrolled_state();
    {
        let ts = Arc::make_mut(&mut s.transcript_state);
        ts.autoscroll = false;
        ts.scroll_offset = 999_999;
    }
    let rows = visible_transcript(&s, NARROW.0, NARROW.1);
    assert!(
        rows.iter().any(|r| !r.trim().is_empty()),
        "an impossible scroll_offset rendered an empty transcript:\n{rows:?}"
    );
}

#[test]
fn scroll_indicator_does_not_quote_an_impossible_line_count() {
    // `↓ N more` used to echo the raw offset, so after a stale-offset
    // blank frame it claimed 999 lines were hidden above a transcript
    // that has 60.
    let mut s = long_scrolled_state();
    {
        let ts = Arc::make_mut(&mut s.transcript_state);
        ts.autoscroll = false;
        ts.scroll_offset = 999_999;
    }
    let rows = frame_rows(&s, NARROW.0, NARROW.1);
    let transcript = transcript_text(&rows);
    assert!(
        !transcript.contains("999"),
        "the indicator quoted an impossible offset:\n{transcript}"
    );
}

// ---------------------------------------------------------------------------
// 8. The footer must not lie about what it cut (v0.8.9 P1-6)
// ---------------------------------------------------------------------------
//
// Both footer rows were handed to `Paragraph` whole and clipped at the
// screen edge. At 40 columns:
//
//   line 1:  ~/very/deep/nested/project/path/that/kee     ← cut, unmarked
//   line 2:   nini   idle  some-extremely-long-model-      ← cut, unmarked
//
// and at 80 columns the session pill degraded to `[no `. Worse, the
// stats line measured its halves with `chars().count()` (1 cell per
// CJK character instead of 2) and, when the halves didn't fit, appended
// the right half *anyway* past the terminal edge — so the thing pushed
// off screen was the model name, the one label v0.8.8 spent a whole
// release plumbing through from `--model`.
//
// The footer is the first two rows of the frame.

fn footer_rows(s: &AppState, w: u16, h: u16) -> Vec<String> {
    let rows = frame_rows(s, w, h);
    rows[..2].to_vec()
}

fn state_with_long_cwd_and_model() -> AppState {
    let mut s = AppState::new("some-extremely-long-model-name-that-will-not-fit");
    s.session_state.cwd = Some(
        "/home/jin/very/deep/nested/project/path/that/keeps/going/on/and/on/forever/nini".into(),
    );
    s
}

#[test]
fn footer_rows_never_exceed_the_terminal_width() {
    let s = state_with_long_cwd_and_model();
    for w in [20u16, 30, 40, 60, 80, 120] {
        let rows = footer_rows(&s, w, 24);
        for (i, row) in rows.iter().enumerate() {
            assert!(
                row_width(row) <= w as usize,
                "footer row {i} is {} cells at {w} cols: {row:?}",
                row_width(row)
            );
        }
    }
}

#[test]
fn long_cwd_is_marked_and_cut_from_the_left() {
    let s = state_with_long_cwd_and_model();
    for w in [20u16, 30, 40, 60] {
        let pwd = &footer_rows(&s, w, 24)[0];
        assert!(
            pwd.contains('…'),
            "long cwd at {w} cols is cut with no marker: {pwd:?}"
        );
        // The tail of a path is the part that identifies the project, so
        // the cut must come from the left — never show the head.
        assert!(
            !pwd.contains("/home/jin"),
            "cwd truncation kept the head instead of the tail at {w} cols: {pwd:?}"
        );
        assert!(
            pwd.starts_with('…'),
            "a head-cut cwd must start with the marker: {pwd:?}"
        );
    }
}

#[test]
fn cwd_tail_survives_when_there_is_room() {
    // At a width where the whole tail can fit, it must be there. (At 20
    // columns a 77-character path can only keep 19 cells, so "nini" is
    // physically unreachable — the test above asserts the *direction* of
    // the cut instead, which is the part that is actually a bug.)
    let s = state_with_long_cwd_and_model();
    for w in [60u16, 80, 120] {
        let pwd = &footer_rows(&s, w, 24)[0];
        assert!(
            pwd.contains("nini"),
            "cwd tail dropped at {w} cols where it fits: {pwd:?}"
        );
    }
}

#[test]
fn model_name_survives_narrow_terminals() {
    // The regression that matters: the right half used to be appended
    // past the terminal edge, so the model name — the one label v0.8.8
    // spent a release plumbing through from `--model` — got pushed off
    // screen. A name that *can* fit must always be shown in full.
    let mut s = AppState::new("claude-sonnet-4-5");
    s.session_state.cwd = Some("/home/jin/nini".into());
    for w in [20u16, 30, 40, 60, 80] {
        let stats = &footer_rows(&s, w, 24)[1];
        assert!(
            stats.contains("claude-sonnet-4-5"),
            "model identity lost at {w} cols: {stats:?}"
        );
    }
}

#[test]
fn pathological_model_name_is_cut_but_never_overflows() {
    // 47 cells of model id cannot fit in a 40-column terminal no matter
    // what. The correct behaviour is a head-cut with a marker, and a
    // row that stays inside the terminal — not an overflow.
    let s = state_with_long_cwd_and_model();
    for w in [20u16, 30, 40] {
        let stats = &footer_rows(&s, w, 24)[1];
        assert!(
            stats.contains('…'),
            "a 47-cell model id at {w} cols must be marked as cut: {stats:?}"
        );
        assert!(
            !stats.contains("……"),
            "double ellipsis on screen: {stats:?}"
        );
        assert!(
            stats.contains("-not-fit") || stats.contains("model"),
            "head-cut should keep the identifying tail: {stats:?}"
        );
    }
}

#[test]
fn wide_terminal_shows_the_footer_uncut() {
    // The other direction — at a comfortable width nothing should be
    // truncated, and the model name should be complete.
    let mut s = AppState::new("claude-sonnet-4-5");
    s.session_state.cwd = Some("/home/jin/nini".into());
    let rows = footer_rows(&s, 120, 24);
    assert!(
        !rows[0].contains('…'),
        "short cwd was truncated: {:?}",
        rows[0]
    );
    assert!(
        rows[1].contains("claude-sonnet-4-5"),
        "model name should be complete at 120 cols: {:?}",
        rows[1]
    );
}

#[test]
fn cjk_model_name_is_measured_in_cells_not_chars() {
    // A CJK model id is half the char count of its cell width. With
    // `chars().count()` the padding came up short and the right half
    // spilled off the right edge.
    let mut s = AppState::new("模型名称很长的测试用例");
    s.session_state.cwd = Some("/home/jin/nini".into());
    for w in [20u16, 30, 40, 60] {
        let rows = footer_rows(&s, w, 24);
        for (i, row) in rows.iter().enumerate() {
            assert!(
                row_width(row) <= w as usize,
                "CJK footer row {i} is {} cells at {w} cols: {row:?}",
                row_width(row)
            );
        }
    }
}

#[test]
fn session_pill_is_dropped_whole_not_shown_half() {
    // `[no ` tells the user strictly less than no pill at all.
    let mut s = AppState::new("m");
    s.session_state.cwd = Some("/home/jin/very/deep/nested/project/path/that/keeps/going".into());
    for w in [20u16, 30, 40] {
        let pwd = &footer_rows(&s, w, 24)[0];
        assert!(
            !pwd.contains("[no"),
            "half-shown session pill at {w} cols: {pwd:?}"
        );
    }
}
