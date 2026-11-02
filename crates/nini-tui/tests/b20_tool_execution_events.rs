//! B20 — the TUI must show **which** tool call is running.
//!
//! Pi publishes two moments for a tool call
//! (`references/pi-0.85.1/src/loop/agent-loop.ts`):
//!
//!   * the model announces it → the transcript row appears;
//!   * the runtime is about to execute it → `tool_execution_start`.
//!
//! Only the second one says "this is the call the agent is sitting in
//! right now". In a sequential batch the two are far apart in time, so
//! without the second event the user cannot tell a queued call from
//! the running one — the rows all look identical.
//!
//! These tests drive the real sink → state → frame pipeline and assert
//! the **text** of the row, because a colour difference is invisible
//! in a captured frame (and in a terminal with no colour support).

#![allow(
    clippy::needless_return,
    clippy::let_underscore_future,
    clippy::let_underscore_must_use,
    clippy::redundant_closure_for_method_calls
)]

use nini_tui::render::render_frame;
use nini_tui::runtime::{shared_state, AgentEventLite, AgentSink};
use nini_tui::state::AppState;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// Render the state into a `TestBackend` frame and return its rows.
fn frame_rows(state: &AppState, w: u16, h: u16) -> Vec<String> {
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

/// The row that shows the tool call (the one carrying the tool name
/// and the `▸` / `»` marker).
fn call_row(rows: &[String]) -> String {
    rows.iter()
        .find(|r| r.contains("alpha"))
        .unwrap_or_else(|| panic!("no tool-call row in the frame: {rows:?}"))
        .trim_end()
        .to_string()
}

fn sink_and_state() -> (AgentSink, nini_tui::runtime::SharedState) {
    let shared = shared_state(AppState::new("test-model"));
    let sink = AgentSink::new(shared.clone(), tokio::sync::watch::channel(false).0);
    (sink, shared)
}

/// The queued call and the running call must be drawn differently.
#[test]
fn a_running_tool_call_is_drawn_differently_from_a_queued_one() {
    let (sink, shared) = sink_and_state();

    // 1. The model announces the call: the row exists, the call is NOT
    //    running yet.
    sink.push(AgentEventLite::ToolCallStart {
        id: "t1".to_string(),
        name: "alpha".to_string(),
    });
    sink.push(AgentEventLite::ToolCallStop {
        id: "t1".to_string(),
        args: r#"{"path":"x"}"#.to_string(),
    });
    let state = shared.lock().unwrap().clone();
    let queued = call_row(&frame_rows(&state, 100, 24));
    assert_eq!(
        queued, "▸ alpha {\"path\":\"x\"}",
        "a queued call keeps the plain marker"
    );

    // 2. The runtime starts executing it: the row must change.
    sink.push(AgentEventLite::ToolExecutionStart {
        id: "t1".to_string(),
        name: "alpha".to_string(),
    });
    let state = shared.lock().unwrap().clone();
    let running = call_row(&frame_rows(&state, 100, 24));
    assert_eq!(
        running, "» alpha {\"path\":\"x\"}",
        "the running call gets its own marker"
    );
    assert_ne!(
        queued, running,
        "the queued row and the running row must not look the same"
    );

    // 3. The call finishes: the running marker goes away again.
    sink.push(AgentEventLite::ToolResult {
        id: "t1".to_string(),
        name: "alpha".to_string(),
        ok: true,
        content: "alpha ran".to_string(),
        details: None,
        duration_ms: 1,
    });
    let state = shared.lock().unwrap().clone();
    let finished = call_row(&frame_rows(&state, 100, 24));
    assert_eq!(
        finished, "▸ alpha {\"path\":\"x\"}",
        "a finished call is no longer drawn as running"
    );
    assert_ne!(finished, running);
}

/// The running marker must follow the call, not the position: with two
/// calls, the SECOND one is the running one.
#[test]
fn the_running_marker_follows_the_call_and_not_the_row() {
    let (sink, shared) = sink_and_state();

    sink.push(AgentEventLite::ToolCallStart {
        id: "t1".to_string(),
        name: "alpha".to_string(),
    });
    sink.push(AgentEventLite::ToolCallStop {
        id: "t1".to_string(),
        args: r#"{"path":"x"}"#.to_string(),
    });
    sink.push(AgentEventLite::ToolCallStart {
        id: "t2".to_string(),
        name: "beta".to_string(),
    });
    sink.push(AgentEventLite::ToolCallStop {
        id: "t2".to_string(),
        args: r#"{"path":"y"}"#.to_string(),
    });

    // Only the second call is running.
    sink.push(AgentEventLite::ToolExecutionStart {
        id: "t2".to_string(),
        name: "beta".to_string(),
    });

    let state = shared.lock().unwrap().clone();
    let rows = frame_rows(&state, 100, 24);
    let alpha = rows
        .iter()
        .find(|r| r.contains("alpha"))
        .expect("the alpha row")
        .trim_end()
        .to_string();
    let beta = rows
        .iter()
        .find(|r| r.contains("beta"))
        .expect("the beta row")
        .trim_end()
        .to_string();

    assert_eq!(alpha, "▸ alpha {\"path\":\"x\"}", "alpha is only queued");
    assert_eq!(beta, "» beta {\"path\":\"y\"}", "beta is the running one");
}
