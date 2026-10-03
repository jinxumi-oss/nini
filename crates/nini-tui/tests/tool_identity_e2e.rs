//! End-to-end regression: tool identity preservation across the TUI pipeline.
//!
//! **Reported bug class** (real LLM task driving v0.8.7): when a model
//! emits parallel tool calls (typical: 8× `read` in one turn), the TUI
//! showed scrambled `▸ name{args}` ↔ `✓ body` pairs:
//!
//!   1. The `ToolCallStop` sink handler updated the LAST ToolCall line
//!      unconditionally — for parallel calls this overwrote every earlier
//!      call's args with the last call's args (Bug A).
//!   2. The `ToolResult` line carried no tool name — the user couldn't
//!      tell which of N parallel results a `✓ Took Nms` block came from
//!      (Bug D).
//!   3. `AppState::new("test-model")` was hardcoded in `runtime.rs:214`,
//!      silently dropping the user's `--model MiniMax-M3` flag (Bug B).
//!
//! **These tests**: drive the `AgentSink → SharedState → render_frame`
//! pipeline with realistic parallel tool call sequences and assert that
//! each tool's identity (id + name + args) survives intact.

#![allow(
    clippy::needless_return,
    clippy::let_underscore_future,
    clippy::let_underscore_must_use,
    clippy::redundant_closure_for_method_calls
)]

use nini_tui::render::render_frame;
use nini_tui::runtime::{shared_state, AgentEventLite, AgentSink};
use nini_tui::state::{AppState, TranscriptLine};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// Snapshot the visible text of a frame as a Vec<String>.
fn frame_dump(state: &AppState, w: u16, h: u16) -> Vec<String> {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_frame(f, state)).unwrap();
    let buf = terminal.backend().buffer().clone();
    let mut out = Vec::with_capacity(buf.area.height as usize);
    for y in 0..buf.area.height {
        let mut line = String::new();
        for x in 0..buf.area.width {
            if let Some(cell) = buf.cell((x, y)) {
                line.push_str(cell.symbol());
            }
        }
        out.push(line);
    }
    out
}

// =====================================================================
// Test 1 — Parallel tool calls keep distinct args (Bug A regression)
// =====================================================================
#[test]
fn parallel_tool_calls_keep_distinct_args() {
    let shared = shared_state(AppState::new("test-model"));
    let sink = AgentSink::new(shared.clone(), tokio::sync::watch::channel(false).0);

    // Three parallel read tool calls with distinct paths.
    let paths = ["a.rs", "b.rs", "c.rs"];
    for (i, path) in paths.iter().enumerate() {
        let id = format!("toolu_{i}");
        sink.push(AgentEventLite::ToolCallStart {
            id: id.clone(),
            name: "read".to_string(),
        });
        sink.push(AgentEventLite::ToolCallStop {
            id,
            args: format!(r#"{{"path":"{path}"}}"#),
        });
    }

    let snap = shared.lock().unwrap().clone();

    // Walk the transcript and collect (id, name, args) for each ToolCall.
    let mut found: Vec<(String, String, String)> = Vec::new();
    for line in &snap.transcript_state.lines {
        if let TranscriptLine::ToolCall { id, name, args, .. } = line {
            found.push((id.clone(), name.clone(), args.clone()));
        }
    }

    assert_eq!(
        found.len(),
        3,
        "expected 3 distinct ToolCall lines, got {} (parallel args got overwritten)",
        found.len()
    );

    // Each call must carry its OWN path — the v0.8.7 sink bug caused
    // all three to share the LAST call's args.
    let args_joined: Vec<String> = found.iter().map(|(_, _, a)| a.clone()).collect();
    assert!(
        args_joined.iter().any(|a| a.contains("a.rs")),
        "a.rs args missing: {args_joined:?}"
    );
    assert!(
        args_joined.iter().any(|a| a.contains("b.rs")),
        "b.rs args missing: {args_joined:?}"
    );
    assert!(
        args_joined.iter().any(|a| a.contains("c.rs")),
        "c.rs args missing: {args_joined:?}"
    );
}

// =====================================================================
// Test 2 — ToolResult line shows tool name (Bug D regression)
// =====================================================================
#[test]
fn tool_result_renders_tool_name() {
    let shared = shared_state(AppState::new("test-model"));
    let sink = AgentSink::new(shared.clone(), tokio::sync::watch::channel(false).0);

    sink.push(AgentEventLite::ToolCallStart {
        id: "toolu_1".to_string(),
        name: "bash".to_string(),
    });
    sink.push(AgentEventLite::ToolCallStop {
        id: "toolu_1".to_string(),
        args: r#"{"command":"echo hi"}"#.to_string(),
    });
    sink.push(AgentEventLite::ToolResult {
        id: "toolu_1".to_string(),
        name: "bash".to_string(),
        ok: true,
        content: "hi\n".to_string(),
        details: None,
        duration_ms: 4,
    });

    let snap = shared.lock().unwrap().clone();
    let frame = frame_dump(&snap, 100, 24);

    // The result line should contain `✓ bash` (Pi parity) so the user
    // can tell which tool produced this result.
    let has_name_in_result = frame.iter().any(|r| r.contains("✓") && r.contains("bash"));
    assert!(
        has_name_in_result,
        "expected '✓ bash' in tool result header; frame was:\n{}",
        frame.join("\n")
    );
}

// =====================================================================
// Test 3 — ToolResult for failing tool shows `✗ <name>` (not just `✗`)
// =====================================================================
#[test]
fn failing_tool_result_renders_tool_name() {
    let shared = shared_state(AppState::new("test-model"));
    let sink = AgentSink::new(shared.clone(), tokio::sync::watch::channel(false).0);

    sink.push(AgentEventLite::ToolCallStart {
        id: "toolu_1".to_string(),
        name: "read".to_string(),
    });
    sink.push(AgentEventLite::ToolCallStop {
        id: "toolu_1".to_string(),
        args: r#"{"path":"/nope"}"#.to_string(),
    });
    sink.push(AgentEventLite::ToolResult {
        id: "toolu_1".to_string(),
        name: "read".to_string(),
        ok: false,
        content: "io error: file not found".to_string(),
        details: None,
        duration_ms: 0,
    });

    let snap = shared.lock().unwrap().clone();
    let frame = frame_dump(&snap, 100, 24);
    let has_x_name = frame.iter().any(|r| r.contains("✗") && r.contains("read"));
    assert!(
        has_x_name,
        "expected '✗ read' in failing tool result; frame was:\n{}",
        frame.join("\n")
    );
}

// =====================================================================
// Test 4 — Parallel tool RESULTS keep distinct names (no cross-over)
// =====================================================================
#[test]
fn parallel_tool_results_keep_distinct_names() {
    let shared = shared_state(AppState::new("test-model"));
    let sink = AgentSink::new(shared.clone(), tokio::sync::watch::channel(false).0);

    // Three different tool types in parallel.
    let tools = [("bash", "echo 1"), ("read", "{\"path\":\"/x\"}"), ("grep", "{\"pattern\":\"y\"}")];
    for (i, (name, args)) in tools.iter().enumerate() {
        let id = format!("toolu_{i}");
        sink.push(AgentEventLite::ToolCallStart {
            id: id.clone(),
            name: name.to_string(),
        });
        sink.push(AgentEventLite::ToolCallStop {
            id,
            args: args.to_string(),
        });
    }
    for (i, (name, _)) in tools.iter().enumerate() {
        let id = format!("toolu_{i}");
        sink.push(AgentEventLite::ToolResult {
            id,
            name: name.to_string(),
            ok: true,
            content: format!("{name} ok"),
            details: None,
            duration_ms: 1,
        });
    }

    let snap = shared.lock().unwrap().clone();
    let frame = frame_dump(&snap, 200, 30);

    // Each tool result should have its own name on the result header.
    let bash_ok = frame.iter().any(|r| r.contains("✓ bash"));
    let read_ok = frame.iter().any(|r| r.contains("✓ read"));
    let grep_ok = frame.iter().any(|r| r.contains("✓ grep"));
    assert!(bash_ok, "'✓ bash' missing from parallel tool results");
    assert!(read_ok, "'✓ read' missing from parallel tool results");
    assert!(grep_ok, "'✓ grep' missing from parallel tool results");
}

// =====================================================================
// Test 5 — ToolCallStop with unknown id falls back gracefully (no panic)
// =====================================================================
#[test]
fn tool_call_stop_unknown_id_falls_back() {
    let shared = shared_state(AppState::new("test-model"));
    let sink = AgentSink::new(shared.clone(), tokio::sync::watch::channel(false).0);

    // Push a Stop without a matching Start — out-of-order from a buggy
    // provider. Should NOT panic; should push a new line.
    sink.push(AgentEventLite::ToolCallStop {
        id: "orphan_id".to_string(),
        args: r#"{"command":"x"}"#.to_string(),
    });

    let snap = shared.lock().unwrap().clone();
    let last = snap.transcript_state.lines.last();
    if let Some(TranscriptLine::ToolCall { id, args, .. }) = last {
        assert_eq!(id, "orphan_id");
        assert!(args.contains("command"));
    } else {
        panic!("expected ToolCall line after orphan Stop, got {last:?}");
    }
}

// =====================================================================
// Test 6 — status bar model field reflects initial_model argument
// (Bug B regression). Render the frame and check the model name shows
// up in the status bar.
// =====================================================================
#[test]
fn initial_model_arg_propagates_to_status_bar() {
    let state = AppState::new("MiniMax-M3");
    // status bar reads from state.model_state.model — AppState::new
    // already seeds it.
    assert_eq!(state.model_state.model, "MiniMax-M3");

    // Render and check the frame contains the model name.
    let frame = frame_dump(&state, 200, 24);
    let status_row = frame.get(1).cloned().unwrap_or_default();
    assert!(
        status_row.contains("MiniMax-M3"),
        "status bar should show model name 'MiniMax-M3', got: {status_row:?}"
    );
}