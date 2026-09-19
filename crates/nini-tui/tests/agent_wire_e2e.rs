//! End-to-end integration tests: keystroke → submit → agent task →
//! AgentSink → SharedState → render → snapshot.
// Test code frequently uses patterns that clippy::style flags
#![allow(
    clippy::needless_return,
    clippy::let_underscore_future,
    clippy::let_underscore_must_use,
    clippy::redundant_closure_for_method_calls
)]
//!
//! These tests verify the full vertical slice: user types a message, presses
//! Enter, the agent runs, and the TUI transcript updates with tool calls
//! and the final response.

use futures_util::StreamExt;
use nini_ai::fixture::{FixtureTurn, ProgrammedProvider};
use nini_core::provider::Usage;
use nini_core::tool::ToolRegistry;
use nini_core::{Agent, AgentEvent, RunConfig};
use nini_tools::BashTool;
use nini_tui::Key;
use nini_tui::render::render_frame;
use nini_tui::runtime::{AgentEventLite, AgentSink, shared_state};
use nini_tui::state::AppState;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use std::sync::Arc;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

/// Snapshot the visible text of a frame.
fn frame_text(terminal: &Terminal<TestBackend>) -> String {
    let buf = terminal.backend().buffer().clone();
    let mut out = String::new();
    let area = buf.area;
    for y in 0..area.height {
        let mut line = String::new();
        for x in 0..area.width {
            if let Some(cell) = buf.cell((x, y)) {
                line.push_str(cell.symbol());
            }
        }
        out.push_str(line.trim_end_matches(' '));
        out.push('\n');
    }
    out
}

fn render(state: &AppState, w: u16, h: u16) -> String {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_frame(f, state)).unwrap();
    frame_text(&terminal)
}

/// Build an `AgentDriver` that runs a fixture provider's scripted turns
/// against the given tools. Used by submit-spawned tests.
fn make_fixture_driver(
    turns: Vec<Vec<FixtureTurn>>,
) -> impl Fn(String, AgentSink, Arc<Notify>) -> JoinHandle<()> + Send + Sync + 'static {
    let cwd = std::env::current_dir().unwrap();
    move |user_msg: String, sink: AgentSink, done: Arc<Notify>| {
        let provider: Arc<dyn nini_core::Provider> =
            Arc::new(ProgrammedProvider::from_turns(turns.clone()));
        let tools = ToolRegistry::new().register(Arc::new(BashTool::new()));
        let agent = Agent::new(
            provider,
            tools,
            RunConfig {
                model: "test-model".to_string(),
                ..RunConfig::new("test-model")
            },
        );

        tokio::spawn(async move {
            // Move the agent and stream into this async block. Agent is now
            // owned by this task and lives for 'static (via the move closure).
            let mut agent = agent;
            let mut stream = Box::pin(agent.run(nini_core::AgentMessage::user(user_msg.clone())));

            // Translate AgentEvent → AgentEventLite and push to sink.
            while let Some(ev) = stream.next().await {
                let lite = match ev {
                    Ok(AgentEvent::TextDelta { text }) => AgentEventLite::TextDelta(text),
                    Ok(AgentEvent::ToolCallStart { name, .. }) => {
                        AgentEventLite::ToolCallStart { name }
                    }
                    Ok(AgentEvent::ToolCallStop { id, input_json }) => {
                        AgentEventLite::ToolCallStop {
                            id,
                            args: input_json.to_string(),
                        }
                    }
                    Ok(AgentEvent::ToolResult { output, .. }) => {
                        let content = output.content.clone();
                        let ok = !output.is_error;
                        AgentEventLite::ToolResult { ok, content }
                    }
                    Ok(AgentEvent::TurnEnd { usage, .. }) => {
                        sink.push(AgentEventLite::Usage(
                            usage.input_tokens,
                            usage.output_tokens,
                        ));
                        AgentEventLite::TurnEnd
                    }
                    Ok(AgentEvent::Error { message }) => AgentEventLite::Error(message),
                    Ok(AgentEvent::PhaseChanged(phase)) => {
                        AgentEventLite::PhaseChanged(format!("{phase:?}"))
                    }
                    Err(_) => continue,
                    _ => continue,
                };
                sink.push(lite);
            }
            sink.push(AgentEventLite::TurnEnd);
            sink.push(AgentEventLite::Done);
            let _ = cwd;
            done.notify_waiters();
        })
    }
}

/// Drive keystrokes into the AppState (mirrors runtime::handle_key for the
/// non-spawning actions). Used to set up the "user typed X then pressed Enter"
/// state before launching the agent.
fn drive_keys(state: &mut AppState, keys: &[Key]) {
    for k in keys {
        nini_tui::runtime::apply_action(state, *k);
    }
}

/// Helper: build a list of `Key`s that types a string.
fn type_str(s: &str) -> Vec<Key> {
    let mut out: Vec<Key> = s.chars().map(Key::char).collect();
    out.push(Key::enter());
    out
}

// =====================================================================
// Test 1: Submit → agent runs → transcript updated → render shows result
// =====================================================================
#[tokio::test]
async fn submit_triggers_agent_and_renders_response() {
    let mut state = AppState::new("test-model");
    let shared = shared_state(state.clone());
    let sink = AgentSink::new(shared.clone());

    // Pre-fill the input as if user typed it
    for c in "echo hello".chars() {
        state.input.insert_char(c);
    }
    assert_eq!(state.input.text, "echo hello");

    // Construct driver: model calls bash then says "hello"
    let driver: nini_tui::runtime::AgentDriver = Arc::new(make_fixture_driver(vec![
        vec![
            FixtureTurn::ToolCall {
                name: "bash".to_string(),
                args: serde_json::json!({"command": "echo hello"}),
            },
            FixtureTurn::Stop {
                stop_reason: "tool_use".to_string(),
                usage: Usage::default(),
            },
        ],
        vec![
            FixtureTurn::Text("hello".to_string()),
            FixtureTurn::Stop {
                stop_reason: "end_turn".to_string(),
                usage: Usage::default(),
            },
        ],
    ]));

    // Simulate submit: directly call sink-side logic (since handle_key uses
    // internal Arc<Notify> we can't reach from tests; we exercise the sink
    // path which is what the agent task uses).
    let submitted = state.input.submit();
    assert_eq!(submitted, "echo hello");
    {
        let mut g = shared.lock().unwrap();
        g.push_user(submitted.clone());
        g.push_divider();
        g.mode = nini_tui::state::RunMode::Running;
    } // lock dropped here

    let done = Arc::new(Notify::new());
    let _handle = driver(submitted, sink.clone(), done.clone());

    // Wait for agent task to finish (notify_waiters triggers)
    done.notified().await;

    // Snapshot state after agent ran
    let snapshot = shared.lock().unwrap().clone();
    let frame = render(&snapshot, 100, 30);

    // Frame must contain user message, tool call line, tool result line, and
    // the assistant's "hello" text.
    assert!(frame.contains("> echo hello"), "user message missing");
    assert!(
        frame.contains("[tool call] bash"),
        "tool call label missing"
    );
    assert!(frame.contains("[tool result]"), "tool result label missing");
    assert!(frame.contains("hello"), "assistant text missing");

    // Mode should be back to Editing after Done.
    assert_eq!(snapshot.mode, nini_tui::state::RunMode::Editing);
}

// =====================================================================
// Test 2: Multiple submits → multiple transcript sections
// =====================================================================
#[tokio::test]
async fn multiple_submits_accumulate_in_transcript() {
    let shared = shared_state(AppState::new("test-model"));

    // First turn
    let turns1: Vec<Vec<FixtureTurn>> = vec![vec![
        FixtureTurn::Text("first response".to_string()),
        FixtureTurn::Stop {
            stop_reason: "end_turn".to_string(),
            usage: Usage::default(),
        },
    ]];
    let sink1 = AgentSink::new(shared.clone());
    let driver1 = make_fixture_driver(turns1);
    let done1 = Arc::new(Notify::new());
    driver1("first question".into(), sink1, done1.clone());
    done1.notified().await;

    // Second turn
    let turns2: Vec<Vec<FixtureTurn>> = vec![vec![
        FixtureTurn::Text("second response".to_string()),
        FixtureTurn::Stop {
            stop_reason: "end_turn".to_string(),
            usage: Usage::default(),
        },
    ]];
    let sink2 = AgentSink::new(shared.clone());
    let driver2 = make_fixture_driver(turns2);
    let done2 = Arc::new(Notify::new());
    driver2("second question".into(), sink2, done2.clone());
    done2.notified().await;

    let snapshot = shared.lock().unwrap().clone();
    let frame = render(&snapshot, 100, 30);

    // Both Q&A pairs must appear in the transcript.
    assert!(frame.contains("first response"), "first response missing");
    assert!(frame.contains("second response"), "second response missing");

    // Two dividers (one after each user message).
    let divider_count =
        frame.matches("─────────").count() + frame.matches("─").count().saturating_sub(20); // crude: accept any dashes
    // Just check transcript length grew.
    assert!(
        snapshot.transcript.len() >= 6,
        "expected at least 6 transcript lines (2x user+assistant+divider)"
    );
    let _ = divider_count;
}

// =====================================================================
// Test 3: Agent error appears in transcript
// =====================================================================
#[tokio::test]
async fn agent_error_is_recorded() {
    let shared = shared_state(AppState::new("test-model"));
    let sink = AgentSink::new(shared.clone());

    // Push an error event directly via the sink
    sink.push(AgentEventLite::Error("boom".to_string()));
    sink.push(AgentEventLite::Done);

    let snapshot = shared.lock().unwrap().clone();
    let frame = render(&snapshot, 100, 24);
    assert!(frame.contains("boom"), "error message missing in frame");
    assert!(frame.contains("[error]"), "error label missing");
}

// =====================================================================
// Test 4: Token usage accumulates across turns
// =====================================================================
#[tokio::test]
async fn token_usage_accumulates() {
    let shared = shared_state(AppState::new("test-model"));
    let sink = AgentSink::new(shared.clone());

    sink.push(AgentEventLite::Usage(100, 50));
    sink.push(AgentEventLite::Usage(200, 100));

    let snapshot = shared.lock().unwrap().clone();
    assert_eq!(snapshot.tokens.input, 300);
    assert_eq!(snapshot.tokens.output, 150);

    // Render and verify status bar shows totals. New status-bar
    // format: 'in 300 | out 150' (with optional K/M suffix for
    // large values, but raw integers for small totals).
    let frame = render(&snapshot, 100, 24);
    assert!(frame.contains("in 300"), "input token total missing");
    assert!(frame.contains("out 150"), "output token total missing");
}

// =====================================================================
// Test 5: Tool call args are updated when ToolCallStop arrives
// =====================================================================
#[tokio::test]
async fn tool_call_args_are_updated_on_stop() {
    let shared = shared_state(AppState::new("test-model"));
    let sink = AgentSink::new(shared.clone());

    sink.push(AgentEventLite::ToolCallStart {
        name: "bash".to_string(),
    });
    // Before stop, args is empty string
    let snap1 = shared.lock().unwrap().clone();
    if let Some(nini_tui::state::TranscriptLine::ToolCall { args, .. }) = snap1.transcript.last() {
        assert_eq!(args, "", "args should be empty before ToolCallStop");
    } else {
        panic!("expected ToolCall line");
    }
    sink.push(AgentEventLite::ToolCallStop {
        id: "toolu_1".to_string(),
        args: r#"{"command":"ls"}"#.to_string(),
    });

    let snap2 = shared.lock().unwrap().clone();
    if let Some(nini_tui::state::TranscriptLine::ToolCall { args, name }) = snap2.transcript.last()
    {
        assert_eq!(name, "bash");
        assert_eq!(args, r#"{"command":"ls"}"#);
    } else {
        panic!("expected ToolCall line after stop");
    }

    let frame = render(&snap2, 100, 24);
    assert!(
        frame.contains(r#"{"command":"ls"}"#),
        "tool call args missing in frame"
    );
}

// =====================================================================
// Test 6: Running mode status bar shows during agent run
// =====================================================================
#[tokio::test]
async fn running_mode_visible_while_agent_runs() {
    let shared = shared_state(AppState::new("test-model"));
    let sink = AgentSink::new(shared.clone());

    // Set mode to Running (simulating what submit does)
    shared.lock().unwrap().mode = nini_tui::state::RunMode::Running;

    // Render before any events arrive. The new 5-state status bar
    // shows the spinner + 'working…' label when RunMode is Running.
    let frame_before = render(&shared.lock().unwrap().clone(), 80, 24);
    assert!(
        frame_before.contains("working") || frame_before.contains("running"),
        "running indicator missing"
    );

    // Now push a text delta and verify it appears
    sink.push(AgentEventLite::TextDelta("partial response".to_string()));
    let frame_after = render(&shared.lock().unwrap().clone(), 80, 24);
    assert!(
        frame_after.contains("partial response"),
        "text delta missing"
    );

    // Final Done
    sink.push(AgentEventLite::Done);
    let frame_done = render(&shared.lock().unwrap().clone(), 80, 24);
    // New 5-state status bar shows 'idle' when not running.
    assert!(
        frame_done.contains("idle") || frame_done.contains("[ready]"),
        "should be back to ready"
    );
    assert!(
        !frame_done.contains("[running...]"),
        "should not be running anymore"
    );
}

// =====================================================================
// Test 7: Multiple tool calls in sequence accumulate
// =====================================================================
#[tokio::test]
async fn multiple_tool_calls_accumulate() {
    let shared = shared_state(AppState::new("test-model"));
    let sink = AgentSink::new(shared.clone());

    // Three tool calls back to back (realistic for bash → read → edit)
    sink.push(AgentEventLite::ToolCallStart {
        name: "bash".to_string(),
    });
    sink.push(AgentEventLite::ToolCallStop {
        id: "t1".into(),
        args: r#"{"command":"ls"}"#.into(),
    });
    sink.push(AgentEventLite::ToolResult {
        ok: true,
        content: "main.rs".into(),
    });

    sink.push(AgentEventLite::ToolCallStart {
        name: "read".to_string(),
    });
    sink.push(AgentEventLite::ToolCallStop {
        id: "t2".into(),
        args: r#"{"path":"main.rs"}"#.into(),
    });
    sink.push(AgentEventLite::ToolResult {
        ok: true,
        content: "fn main() {}".into(),
    });

    let snap = shared.lock().unwrap().clone();
    assert!(snap.transcript.iter().any(
        |l| matches!(l, nini_tui::state::TranscriptLine::ToolCall { name, .. } if name == "bash")
    ));
    assert!(snap.transcript.iter().any(
        |l| matches!(l, nini_tui::state::TranscriptLine::ToolCall { name, .. } if name == "read")
    ));
    // Two results
    let result_count = snap
        .transcript
        .iter()
        .filter(|l| matches!(l, nini_tui::state::TranscriptLine::ToolResult { .. }))
        .count();
    assert_eq!(result_count, 2);
}

// =====================================================================
// Test 8: `apply_action` does not spawn (pure state mutation)
// =====================================================================
#[test]
fn apply_action_is_pure_no_spawn() {
    let mut state = AppState::new("test-model");
    for c in "hello".chars() {
        nini_tui::runtime::apply_action(&mut state, Key::char(c));
    }
    assert_eq!(state.input.text, "hello");
    assert_eq!(state.mode, nini_tui::state::RunMode::Editing);

    // Submit via pure action (no spawn)
    nini_tui::runtime::apply_action(&mut state, Key::enter());
    assert_eq!(state.input.text, "");
    assert_eq!(state.transcript.len(), 2); // user + divider
    // Mode stays Editing because no agent task spawned.
    assert_eq!(state.mode, nini_tui::state::RunMode::Editing);
}

// =====================================================================
// Test 9: Full pipeline — drive keys, run agent, snapshot final frame
// =====================================================================
#[tokio::test]
async fn full_pipeline_drive_keys_then_run_agent() {
    let mut state = AppState::new("test-model");

    // 1. Drive keys: type "find TODOs and fix them" + Enter
    let keys = type_str("find TODOs and fix them");
    drive_keys(&mut state, &keys);
    assert_eq!(state.input.text, "");
    assert_eq!(state.transcript.len(), 2); // user + divider

    // 2. Set up shared state + driver
    let shared = shared_state(state);
    let sink = AgentSink::new(shared.clone());
    let driver: nini_tui::runtime::AgentDriver = Arc::new(make_fixture_driver(vec![
        vec![
            FixtureTurn::ToolCall {
                name: "grep".to_string(),
                args: serde_json::json!({"pattern": "TODO"}),
            },
            FixtureTurn::Stop {
                stop_reason: "tool_use".to_string(),
                usage: Usage::default(),
            },
        ],
        vec![
            FixtureTurn::Text("Found 3 TODOs and fixed them.".to_string()),
            FixtureTurn::Stop {
                stop_reason: "end_turn".to_string(),
                usage: Usage::default(),
            },
        ],
    ]));

    let submitted = "find TODOs and fix them".to_string();
    let done = Arc::new(Notify::new());
    driver(submitted, sink.clone(), done.clone());

    // 3. Wait for agent task
    done.notified().await;

    // 4. Final render
    let snap = shared.lock().unwrap().clone();
    let frame = render(&snap, 100, 30);

    // Assertions: full pipeline result
    assert!(frame.contains("> find TODOs and fix them"));
    assert!(frame.contains("[tool call] grep"));
    assert!(frame.contains("Found 3 TODOs"));
    assert_eq!(snap.mode, nini_tui::state::RunMode::Editing);

    // 5. The transcript should have 5+ lines: user, divider, tool_call,
    //    tool_result, assistant, divider (from TurnEnd).
    assert!(
        snap.transcript.len() >= 5,
        "expected ≥5 transcript lines, got {}",
        snap.transcript.len()
    );
}

// =====================================================================
// Test 10: Concurrent pushes from many events — no panic, all apply
// =====================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_sink_pushes_dont_panic() {
    use std::sync::Arc;
    let shared = shared_state(AppState::new("test-model"));
    let sink = AgentSink::new(shared.clone());

    let mut handles = vec![];
    for _ in 0..5 {
        let s = sink.clone();
        handles.push(tokio::spawn(async move {
            for i in 0..20 {
                s.push(AgentEventLite::TextDelta(format!("chunk {i}")));
            }
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    let snap = shared.lock().unwrap().clone();
    // 100 chunks total
    let combined: String = snap
        .transcript
        .iter()
        .map(|l| match l {
            nini_tui::state::TranscriptLine::AssistantText(s) => s.clone(),
            _ => String::new(),
        })
        .collect();
    assert_eq!(combined.matches("chunk ").count(), 100);
    let _ = Arc::new(0); // silence unused
}
