//! Real-world hostile content must not break the frame.
//!
//! The existing `frame_never_breaks_the_terminal.rs` proves that a
//! synthetic body (`"x".repeat(400)`) cannot push a row past the
//! terminal's edge. This file covers the three shapes that a **real**
//! agent session produces and that the synthetic body does not:
//!
//! * **a** — a JS stack trace: one long, unbroken `file://` path with a
//!   `:line:col` suffix, inside parentheses, repeated for every frame;
//! * **b** — a raw JSON object on a single line, with escaped quotes and
//!   a nested array of strings;
//! * **c** — a multi-line tool result where **every** line is long.
//!
//! The measurement is the same as in `frame_never_breaks_the_terminal`:
//! the rendered frame, measured the way a terminal measures it, compared
//! against the width nini was told the terminal has.

use nini_tui::render::render_frame;
use nini_tui::state::{AppState, TranscriptLine};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// The width of a row, the way a terminal counts it.
fn terminal_width(s: &str) -> usize {
    use unicode_segmentation::UnicodeSegmentation;
    use unicode_width::UnicodeWidthStr;
    let mut width = 0usize;
    let mut previous_wide = false;
    for g in s.graphemes(true) {
        if previous_wide && g == " " {
            previous_wide = false;
            continue;
        }
        let w = UnicodeWidthStr::width(g);
        width += w;
        previous_wide = w == 2;
    }
    width
}

fn frame_rows(state: &AppState, w: u16, h: u16) -> Vec<String> {
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
                .collect::<String>()
        })
        .collect()
}

/// Every row of the frame must fit the terminal.
fn assert_frame_fits(state: &AppState, w: u16, h: u16, what: &str) {
    for (y, row) in frame_rows(state, w, h).iter().enumerate() {
        let width = terminal_width(row);
        assert!(
            width <= w as usize,
            "{what}: row {y} is {width} cells wide, the terminal has {w} — it would \
             wrap, and every row below it would shift down by one:\n{row:?}"
        );
    }
}

fn state_with(line: TranscriptLine) -> AppState {
    let mut state = AppState::new("test-model");
    {
        let ts = std::sync::Arc::make_mut(&mut state.transcript_state);
        ts.lines.push(line);
    }
    state
}

/// **a** — a JS stack trace: `at fn (file:///…/run.js:277:37):`
///
/// The path is long, unbroken, and carries `(`/`:`/`://`; every frame of
/// the trace is a line like that.
fn js_stack_trace() -> String {
    let base = "/home/jin/.nvm/versions/node/v25.9.0/lib/node_modules/ncp/dist";
    (0..12)
        .map(|i| {
            format!(
                "    at processSessionCache.connect (file://{base}/run.js:{}:37):",
                270 + i
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// **b** — a raw JSON object on one line, with escaped quotes.
fn raw_json_object() -> String {
    let messages = (0..40)
        .map(|i| format!("{{\"role\":\"user\",\"content\":\"message number {i} with some text\"}}"))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"status\":\"ok\",\"name\":\"GitHub 仓库和代码\",\"messages\":[{messages}],\
         \"backends\":[\"gh\",\"ch\"],\"output\":\"26 more lines\"}}"
    )
}

/// **c** — a multi-line tool result where every line is long.
fn long_lines_stack() -> String {
    (0..20)
        .map(|i| {
            format!(
                "  at /home/jin/.nvm/versions/node/v25.9.0/lib/node_modules/ncp/dist/run.js:{}:37",
                100 + i
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn js_stack_trace_does_not_break_the_frame() {
    let body = js_stack_trace();
    for line in [
        TranscriptLine::ToolResult {
            id: "t1".into(),
            name: "bash".into(),
            ok: false,
            content: body.clone(),
            collapsed: false,
            duration_ms: Some(12),
        },
        TranscriptLine::BashExecution {
            id: "t1".into(),
            cmd: "node run.js".into(),
            output: body.clone(),
            stderr: body.clone(),
            ok: false,
            exit_code: Some(1),
            duration_ms: 12,
            collapsed: false,
        },
        TranscriptLine::AssistantText(body.clone()),
        TranscriptLine::ThinkingText(body.clone()),
    ] {
        for width in [60u16, 80, 100, 120] {
            let state = state_with(line.clone());
            assert_frame_fits(&state, width, 24, "js stack trace");
        }
    }
}

#[test]
fn raw_json_object_does_not_break_the_frame() {
    let body = raw_json_object();
    for line in [
        TranscriptLine::ToolResult {
            id: "t1".into(),
            name: "list".into(),
            ok: true,
            content: body.clone(),
            collapsed: false,
            duration_ms: None,
        },
        TranscriptLine::AssistantText(body.clone()),
        TranscriptLine::User(body.clone()),
    ] {
        for width in [60u16, 80, 100, 120] {
            let state = state_with(line.clone());
            assert_frame_fits(&state, width, 24, "raw json object");
        }
    }
}

#[test]
fn multi_line_long_tool_result_does_not_break_the_frame() {
    let body = long_lines_stack();
    for line in [
        TranscriptLine::ToolResult {
            id: "t1".into(),
            name: "bash".into(),
            ok: true,
            content: body.clone(),
            collapsed: false,
            duration_ms: Some(4),
        },
        TranscriptLine::BashExecution {
            id: "t1".into(),
            cmd: "node run.js".into(),
            output: body.clone(),
            stderr: body.clone(),
            ok: true,
            exit_code: Some(0),
            duration_ms: 4,
            collapsed: false,
        },
    ] {
        for width in [60u16, 80, 100, 120] {
            let state = state_with(line.clone());
            assert_frame_fits(&state, width, 24, "multi-line long tool result");
        }
    }
}
