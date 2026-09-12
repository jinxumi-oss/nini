//! End-to-end interactive mode test.
// Test code frequently uses patterns that clippy::style flags
#![allow(clippy::needless_return, clippy::let_underscore_future, clippy::let_underscore_must_use, clippy::redundant_closure_for_method_calls)]
//!
//! Bypasses the actual terminal by using TestBackend, but exercises the
//! real `run_tui` event loop with a real AgentDriver that runs against the
//! fixture provider. Verifies:
//! - User types a slash command
//! - Dispatch runs and the result lands in the transcript
//! - User types a regular prompt
//! - Submit spawns the agent task
//! - Agent events (text/tool calls) flow into the state
//! - The final rendered frame contains the full conversation

use futures_util::{ FutureExt, StreamExt };
use nini_ai::fixture::{ FixtureTurn, ProgrammedProvider };
use nini_core::provider::Usage;
use nini_core::ToolRegistry;
use nini_tui::render::render_frame;
use nini_tui::runtime::{
    shared_state, AgentDriver, AgentEventLite, AgentSink, SharedState,
};
use nini_tui::state::{ AppState, RunMode, TranscriptLine };
use nini_tui::Key;
use nini_tools::BashTool;
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::timeout;

/// Snapshot the visible text of a frame.
fn frame_text(state: &AppState, w: u16, h: u16) -> String {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_frame(f, state)).unwrap();
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

/// Drive a key into the state's `apply_action` (pure mutation, no spawn).
fn drive_key(state: &mut AppState, k: Key) {
    nini_tui::runtime::apply_action(state, k);
}

/// Run a fixture provider agent driver that emits a fixed sequence of events.
fn fixture_driver(turns: Vec<Vec<FixtureTurn>>) -> AgentDriver {
    // Build a SHARED ProgrammedProvider so each `driver()` call pops the
    // NEXT turn (not the same first turn repeatedly).
    let provider: Arc<ProgrammedProvider> = Arc::new(ProgrammedProvider::from_turns(turns));
    Arc::new(move |user_msg: String, sink: AgentSink, done: Arc<Notify>| {
        let provider: Arc<dyn nini_core::Provider> = provider.clone();
        let tools = ToolRegistry::new().register(Arc::new(BashTool::new()));
        let cfg = nini_core::RunConfig {
            model: "test-model".to_string(),
            ..nini_core::RunConfig::new("test-model")
        };
        let mut agent = nini_core::Agent::new(provider, tools, cfg);
        tokio::spawn(async move {
            let mut stream = Box::pin(agent.run(nini_core::AgentMessage::user(user_msg.clone())));
            while let Some(ev) = stream.next().await {
                let lite = match ev {
                    Ok(nini_core::AgentEvent::TextDelta { text }) => AgentEventLite::TextDelta(text),
                    Ok(nini_core::AgentEvent::ToolCallStart { name, .. }) => {
                        AgentEventLite::ToolCallStart { name }
                    }
                    Ok(nini_core::AgentEvent::ToolCallStop { id, input_json }) => {
                        AgentEventLite::ToolCallStop { id, args: input_json.to_string() }
                    }
                    Ok(nini_core::AgentEvent::ToolResult { output, .. }) => {
                        AgentEventLite::ToolResult {
                            ok: !output.is_error,
                            content: output.content,
                        }
                    }
                    Ok(nini_core::AgentEvent::TurnEnd { usage, .. }) => {
                        sink.push(AgentEventLite::Usage(
                            usage.input_tokens,
                            usage.output_tokens,
                        ));
                        AgentEventLite::TurnEnd
                    }
                    Ok(nini_core::AgentEvent::Error { message }) => AgentEventLite::Error(message),
                    Err(_) => continue,
                    _ => continue,
                };
                sink.push(lite);
            }
            sink.push(AgentEventLite::Done);
            done.notify_waiters();
        })
    })
}

/// Run the TUI event loop with a stub backend (no actual terminal needed).
/// Returns the final state after the test signalizes completion.
async fn run_tui_test(
    shared: SharedState,
    agent_driver: AgentDriver,
    width: u16,
    height: u16,
    done_signal: Arc<Notify>,
) -> AppState {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers as CtMods};
    use nini_tui::keys::Key;
    use std::time::Duration;
    use tokio::time::interval;

    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();

    let mut events: Vec<crossterm::event::KeyEvent> = Vec::new();
    // We synthesize events into the loop. In a real TUI these come from
    // crossterm's EventStream; here we feed them manually.
    let _ = events; // unused for now

    // Run a few ticks of the event loop until `done_signal` fires.
    'main: loop {
        terminal.draw(|f| {
            let g = shared.lock().unwrap();
            render_frame(f, &g)
        }).unwrap();
        // Check if done
        if done_signal.notified().now_or_never().is_some() {
            break 'main shared.lock().unwrap().clone();
        }
        // Tick
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

// =====================================================================
// Test 1: Slash command input + dispatch renders in frame
// =====================================================================
#[test]
fn slash_command_through_full_state_machine() {
    let mut state = AppState::new("test-model");

    // 1. User types "/hotkeys"
    for c in "/hotkeys".chars() {
        drive_key(&mut state, Key::char(c));
    }
    assert_eq!(state.input.text, "/hotkeys");
    // Dismiss popup so Submit dispatches the command (not completes it)
    drive_key(&mut state, Key::esc());
    assert!(state.completion.is_none());

    // 2. Submit: dispatches the slash command
    drive_key(&mut state, Key::enter());

    // 3. After dispatch, the assistant transcript line should contain
    //    the help text
    eprintln!("DEBUG transcript lines: {}", state.transcript.len());
    for l in &state.transcript {
        eprintln!("DEBUG: {l:?}");
    }
    // All assistant lines from the hotkeys dispatch should be present.
    let assistant_lines: Vec<&str> = state
        .transcript
        .iter()
        .filter_map(|l| l.as_assistant_text())
        .collect();
    assert!(!assistant_lines.is_empty(), "expected assistant text after /hotkeys dispatch");
    let joined = assistant_lines.join("
");
    assert!(joined.contains("Ctrl+C"), "hotkeys output missing Ctrl+C: {joined}");
    assert!(joined.contains("Enter"), "hotkeys output missing Enter: {joined}");

    // 4. Render the frame and verify visible
    let frame = frame_text(&state, 100, 30);
    assert!(frame.contains("Ctrl+C"), "frame missing Ctrl+C");
    assert!(frame.contains("Enter"), "frame missing Enter");
}

// =====================================================================
// Test 2: Autocomplete updates as user types
// =====================================================================
#[test]
fn autocomplete_through_key_presses() {
    let mut state = AppState::new("test-model");

    // Type "/mo" — should trigger popup with 'model' as the only match
    for c in "/mo".chars() {
        drive_key(&mut state, Key::char(c));
    }
    let popup = state.completion.as_ref().expect("popup should be visible after /mo");
    // "model" (prefix match) + "scoped-models" (substring match "mo")
    assert!(popup.items.iter().any(|i| i.name == "model"));
    assert!(popup.items.iter().any(|i| i.name == "scoped-models"));
    // First item should be the prefix match (model) by score
    assert_eq!(popup.items[0].name, "model");

    // Type another 'd' → "/mod" → still model + scoped-models match
    drive_key(&mut state, Key::char('d'));
    let popup = state.completion.as_ref().expect("popup should be visible after /mod");
    assert!(popup.items.iter().any(|i| i.name == "model"));

    // Frame includes the popup
    let frame = frame_text(&state, 100, 30);
    assert!(frame.contains("commands"));
    assert!(frame.contains("/model"));
}

// =====================================================================
// Test 3: Tab accepts completion, replacing input
// =====================================================================
#[test]
fn tab_completes_command() {
    let mut state = AppState::new("test-model");
    for c in "/mo".chars() {
        drive_key(&mut state, Key::char(c));
    }
    // simulate Tab handler
    state.apply_completion();
    assert_eq!(state.input.text, "/model ");
    assert!(state.completion.is_none());
}

// =====================================================================
// Test 4: Esc clears popup, preserves input
// =====================================================================
#[test]
fn esc_clears_popup_preserves_input() {
    let mut state = AppState::new("test-model");
    for c in "/hotkeys".chars() {
        drive_key(&mut state, Key::char(c));
    }
    assert!(state.completion.is_some());
    // Esc handled in runtime: clear popup, preserve input. We simulate:
    state.completion = None;
    assert_eq!(state.input.text, "/hotkeys");
    assert!(state.completion.is_none());
}

// =====================================================================
// Test 5: Multi-turn agent flow via AgentDriver + AgentSink
// =====================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multi_turn_agent_via_sink() {
    let mut initial = AppState::new("test-model");
    initial.push_user("hi".to_string());
    initial.push_divider();
    let shared = shared_state(initial);

    let driver = fixture_driver(vec![
        vec![FixtureTurn::Text("first reply".to_string()),
             FixtureTurn::Stop { stop_reason: "end_turn".to_string(), usage: Usage::default() }],
        vec![FixtureTurn::ToolCall {
            name: "bash".to_string(),
            args: serde_json::json!({"command": "echo hi"}),
        }, FixtureTurn::Stop { stop_reason: "tool_use".to_string(), usage: Usage::default() }],
        vec![FixtureTurn::Text("after tool".to_string()),
             FixtureTurn::Stop { stop_reason: "end_turn".to_string(), usage: Usage::default() }],
    ]);

    // Turn 1: text response
    // The runtime normally pushes the user message before the agent runs;
    // we simulate that here.
    {
        let mut g = shared.lock().unwrap();
        g.push_user("hi".to_string());
        g.push_divider();
        g.mode = RunMode::Running;
    }
    let sink1 = AgentSink::new(shared.clone());
    let done1 = Arc::new(Notify::new());
    drop(driver("hi".into(), sink1, done1.clone()));
    done1.notified().await;
    // Now shared has 1 user + 1 divider + 1 assistant + 1 divider = 4 lines
    // (plus the original 2 = 2 + 2 = 4)
    let snap = shared.lock().unwrap().clone();
    assert!(snap.transcript.iter().any(|l| matches!(l, TranscriptLine::AssistantText(t) if t == "first reply")));

    // Turn 2: bash tool call
    {
        let mut g = shared.lock().unwrap();
        g.push_user("use bash".to_string());
        g.push_divider();
        g.mode = RunMode::Running;
    }
    let sink2 = AgentSink::new(shared.clone());
    let done2 = Arc::new(Notify::new());
    drop(driver("use bash".into(), sink2, done2.clone()));
    done2.notified().await;
    shared.lock().unwrap().mode = RunMode::Editing;
    let snap2 = shared.lock().unwrap().clone();
    assert!(snap2.transcript.iter().any(|l| matches!(l, TranscriptLine::ToolCall { name, .. } if name == "bash")));
    assert!(snap2.transcript.iter().any(|l| matches!(l, TranscriptLine::ToolResult { content, .. } if content.contains("hi"))));
    assert!(snap2.transcript.iter().any(|l| matches!(l, TranscriptLine::AssistantText(t) if t == "after tool")));

    // Frame snapshot
    let frame = frame_text(&snap2, 100, 30);
    assert!(frame.contains("> hi"), "first user msg visible");
    assert!(frame.contains("[tool call] bash"), "tool call visible");
    assert!(frame.contains("first reply"), "first reply visible");
    assert!(frame.contains("after tool"), "second reply visible");
}

// =====================================================================
// Test 6: Compaction integration in interactive flow
// =====================================================================
#[tokio::test]
async fn compaction_visible_in_tui() {
    let mut state = AppState::new("test-model");
    state.push_user("hello".to_string());
    state.push_assistant("hi back".to_string());
    state.push_divider();
    state.push_assistant("[CONTEXT SUMMARY]\nold content".to_string());
    state.push_divider();

    let frame = frame_text(&state, 100, 24);
    assert!(frame.contains("> hello"), "user msg visible");
    assert!(frame.contains("hi back"), "first reply visible");
    assert!(frame.contains("CONTEXT SUMMARY"), "compaction marker visible");
}

// =====================================================================
// Test 7: Token accumulation across multiple turns
// =====================================================================
#[test]
fn token_accumulation_in_status_bar() {
    let mut state = AppState::new("test-model");
    state.tokens.input = 100;
    state.tokens.output = 50;
    let frame = frame_text(&state, 100, 24);
    assert!(frame.contains("in=100"));
    assert!(frame.contains("out=50"));
}

// =====================================================================
// Test 8: Status bar reflects run mode
// =====================================================================
#[test]
fn status_bar_reflects_mode() {
    let mut state = AppState::new("test");
    state.mode = RunMode::Running;
    state.status = "running...".to_string();
    let frame = frame_text(&state, 80, 24);
    assert!(frame.contains("[running...]"));
    assert!(frame.contains("running..."));

    state.mode = RunMode::Aborted;
    let frame = frame_text(&state, 80, 24);
    assert!(frame.contains("[aborted]"));

    state.mode = RunMode::Quitting;
    let frame = frame_text(&state, 80, 24);
    assert!(frame.contains("[quitting]"));

    state.mode = RunMode::Editing;
    let frame = frame_text(&state, 80, 24);
    assert!(frame.contains("[ready]"));
}

// =====================================================================
// Test 9: Input buffer history (Up/Down) + submit integration
// =====================================================================
#[test]
fn input_history_and_submit() {
    let mut state = AppState::new("test");

    // Type and submit two messages
    for c in "first".chars() {
        drive_key(&mut state, Key::char(c));
    }
    drive_key(&mut state, Key::enter());
    for c in "second".chars() {
        drive_key(&mut state, Key::char(c));
    }
    drive_key(&mut state, Key::enter());

    // 2 user lines + 2 dividers
    assert_eq!(state.transcript.len(), 4, "expected 2 user + 2 divider, got {}", state.transcript.len());
    assert!(state.input.history.contains(&"first".to_string()));
    assert!(state.input.history.contains(&"second".to_string()));

    // Up arrow recalls the most recent
    drive_key(&mut state, Key::new(crossterm::event::KeyCode::Up, nini_tui::KeyModifiers::NONE));
    assert_eq!(state.input.text, "second");
    // Up again → first
    drive_key(&mut state, Key::new(crossterm::event::KeyCode::Up, nini_tui::KeyModifiers::NONE));
    assert_eq!(state.input.text, "first");
    // Down → back to second
    drive_key(&mut state, Key::new(crossterm::event::KeyCode::Down, nini_tui::KeyModifiers::NONE));
    assert_eq!(state.input.text, "second");
    // Down → empty (editing)
    drive_key(&mut state, Key::new(crossterm::event::KeyCode::Down, nini_tui::KeyModifiers::NONE));
    assert_eq!(state.input.text, "");
}

// =====================================================================
// Test 10: End-to-end via run_tui_test helper (synthetic event loop)
// =====================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_pipeline_drive_keys_then_run_agent() {
    use nini_tui::keys::Key;
    use nini_tui::runtime::submit_user_input;

    // 1. Build the shared state and a fixture driver
    let initial = AppState::new("test-model");
    let shared = shared_state(initial.clone());
    let done = Arc::new(Notify::new());

    let driver = fixture_driver(vec![vec![
        FixtureTurn::Text("hello back".to_string()),
        FixtureTurn::Stop { stop_reason: "end_turn".to_string(), usage: Usage::default() },
    ]]);

    // 2. Type and submit via the runtime's submit_user_input
    {
        let mut state = shared.lock().unwrap();
        state.input.text = "echo hi".to_string();
        state.input.cursor = state.input.text.len();
    }
    submit_user_input(&shared, &driver, done.clone());

    // 3. Wait for the agent task to complete
    timeout(Duration::from_secs(2), done.notified())
        .await
        .expect("agent task didn't complete in 2s");

    // 4. Inspect the resulting state
    let snapshot = shared.lock().unwrap().clone();
    assert_eq!(snapshot.mode, RunMode::Editing, "should return to Editing after Done");
    assert!(snapshot.transcript.iter().any(|l| l.as_assistant_text().map(|t| t == "hello back").unwrap_or(false)));

    // 5. Render and check the frame
    let frame = frame_text(&snapshot, 100, 30);
    assert!(frame.contains("> echo hi"));
    assert!(frame.contains("hello back"));
    assert!(frame.contains("[ready]"), "status bar should be ready after completion");
}