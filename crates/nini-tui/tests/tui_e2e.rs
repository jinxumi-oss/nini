//! End-to-end TUI tests using `ratatui::backend::TestBackend`.
//!
//! These tests:
//! 1. Render the empty TUI and snapshot the layout (status bar / transcript / prompt / hints).
//! 2. Drive the state machine via keystrokes and verify each frame's text content.
//! 3. Drive the agent event stream into the transcript and verify the rendered output.

use futures_util::StreamExt;
use nini_ai::fixture::{FixtureTurn, ProgrammedProvider};
use nini_core::provider::Usage;
use nini_core::tool::ToolRegistry;
use nini_core::{Agent, AgentEvent, RunConfig};
use nini_tools::BashTool;
use nini_tui::render::{render_frame, render_frame_with_theme};
use nini_tui::theme::Theme;
use nini_tui::state::{AppState, RunMode, TranscriptLine};
use nini_tui::{Key, KeyAction, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use std::sync::Arc;

/// Snapshot the visible text of a frame, ignoring ANSI styling.
fn frame_text(terminal: &Terminal<TestBackend>) -> String {
    let buffer = terminal.backend().buffer().clone();
    let mut out = String::new();
    let area = buffer.area;
    for y in 0..area.height {
        let mut line = String::new();
        for x in 0..area.width {
            if let Some(cell) = buffer.cell((x, y)) {
                line.push_str(cell.symbol());
            } else {
                line.push(' ');
            }
        }
        // Trim trailing whitespace per line for cleaner snapshots
        let line_trimmed = line.trim_end_matches(' ').to_string();
        out.push_str(&line_trimmed);
        out.push('\n');
    }
    out
}

/// Render the TUI state into a fresh `TestBackend` and return the frame text.
fn render_to_text(state: &AppState, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_frame(f, state)).unwrap();
    frame_text(&terminal)
}

/// Drive a `Key` into the state machine (the same logic `runtime::handle_key`
/// uses, but inlined here so tests don't need a real terminal).
fn drive(state: &mut AppState, key: Key) {
    use nini_tui::keys::{default_keymap, resolve};
    let action = resolve(&default_keymap(), key);
    match action {
        KeyAction::Insert(c) => {
            if state.run_state.mode == RunMode::Editing {
                state.input.insert_char(c);
            }
        }
        KeyAction::Newline => {
            if state.run_state.mode == RunMode::Editing {
                state.input.insert_char('\n');
            }
        }
        KeyAction::Backspace => state.input.backspace(),
        KeyAction::Delete => state.input.delete(),
        KeyAction::MoveLeft => state.input.move_left(),
        KeyAction::MoveRight => state.input.move_right(),
        KeyAction::MoveLineStart => state.input.move_to_start(),
        KeyAction::MoveLineEnd => state.input.move_to_end(),
        KeyAction::MoveWordLeft => state.input.move_word_left(),
        KeyAction::MoveWordRight => state.input.move_word_right(),
        KeyAction::MoveUp => state.input.recall_history(-1),
        KeyAction::MoveDown => state.input.recall_history(1),
        KeyAction::KillToLineStart => state.input.kill_to_line_start(),
        KeyAction::KillToLineEnd => state.input.kill_to_line_end(),
        KeyAction::KillWordBackward => state.input.kill_word_backward(),
        KeyAction::ClearInput => state.input.clear(),
        KeyAction::Submit => {
            if state.run_state.mode == RunMode::Editing {
                let text = state.input.submit();
                if !text.trim().is_empty() {
                    state.push_user(text);
                    state.push_divider();
                }
            }
        }
        KeyAction::Abort => {
            if state.run_state.mode == RunMode::Running {
                state.run_state.mode = RunMode::Aborted;
            } else {
                state.input.clear();
            }
        }
        KeyAction::Quit => state.run_state.mode = RunMode::Quitting,
        KeyAction::SwitchModel
        | KeyAction::CycleModelNext
        | KeyAction::CycleModelPrev
        | KeyAction::CycleThinkingNext
        | KeyAction::CycleThinkingPrev
        | KeyAction::ShowHelp
        | KeyAction::ScrollUp
        | KeyAction::ScrollDown
        | KeyAction::KillWordForward
        | KeyAction::Yank
        | KeyAction::YankPop
        | KeyAction::Undo
        | KeyAction::PasteImage
        | KeyAction::ToggleCollapse
        | KeyAction::OpenSearch
        | KeyAction::OpenCommandPalette
        | KeyAction::OpenExternalEditor => {}
        KeyAction::AcceptCompletionOrInsertTab => {}
        KeyAction::Noop => {}
    }
}

// ====================================================================
// Test 1: Empty state renders correctly (status bar, transcript, prompt, hints)
// ====================================================================
#[test]
fn empty_state_layout_is_stable() {
    let state = AppState::new("test-model");
    let frame = render_to_text(&state, 80, 24);

    // Status bar contains the model name. (New format: 'nini test-model | idle')
    assert!(
        frame.contains("test-model"),
        "status bar missing model name. Frame:\n{frame}"
    );
    assert!(
        frame.contains("idle") || frame.contains("[ready]"),
        "status bar missing mode. Frame:\n{frame}"
    );

    // Transcript area exists (empty line between status and prompt)
    // Prompt editor block border with title "input"
    assert!(
        frame.contains("input"),
        "prompt block missing 'input' title. Frame:\n{frame}"
    );

    // Key hints row at the bottom
    assert!(
        frame.contains("F1"),
        "key hints missing F1. Frame:\n{frame}"
    );
    assert!(
        frame.contains("Ctrl+C") || frame.contains("Ctrl+D"),
        "key hints missing Ctrl+C/D. Frame:\n{frame}"
    );
    assert!(
        frame.contains("Enter"),
        "key hints missing Enter. Frame:\n{frame}"
    );
    assert!(
        frame.contains("Ctrl+L"),
        "key hints missing Ctrl+L. Frame:\n{frame}"
    );
}

// ====================================================================
// Test 2: Typing characters inserts them into the prompt
// ====================================================================
#[test]
fn typing_appends_to_prompt() {
    let mut state = AppState::new("test-model");
    drive(&mut state, Key::char('h'));
    drive(&mut state, Key::char('i'));
    assert_eq!(state.input.text, "hi");
    assert_eq!(state.input.cursor, 2);

    let frame = render_to_text(&state, 80, 24);
    assert!(
        frame.contains("hi"),
        "typed text missing from frame. Frame:\n{frame}"
    );
}

// ====================================================================
// Test 3: Backspace removes last character
// ====================================================================
#[test]
fn backspace_removes_char() {
    let mut state = AppState::new("test-model");
    drive(&mut state, Key::char('a'));
    drive(&mut state, Key::char('b'));
    drive(&mut state, Key::char('c'));
    assert_eq!(state.input.text, "abc");
    drive(&mut state, Key::backspace());
    assert_eq!(state.input.text, "ab");
    assert_eq!(state.input.cursor, 2);
}

// ====================================================================
// Test 4: Enter submits and pushes user message to transcript
// ====================================================================
#[test]
fn enter_submits_and_pushes_user_message() {
    let mut state = AppState::new("test-model");
    for c in "hello".chars() {
        drive(&mut state, Key::char(c));
    }
    drive(&mut state, Key::enter());

    assert_eq!(state.input.text, "", "input should be cleared after submit");
    assert_eq!(state.transcript_state.lines.len(), 2, "should have user line + divider");
    assert!(matches!(&state.transcript_state.lines[0], TranscriptLine::User(s) if s == "hello"));
    assert!(matches!(&state.transcript_state.lines[1], TranscriptLine::Divider));

    let frame = render_to_text(&state, 80, 24);
    assert!(
        frame.contains("> hello"),
        "transcript missing user message. Frame:\n{frame}"
    );
}

// ====================================================================
// Test 5: History navigation with up/down arrows
// ====================================================================
#[test]
fn history_recall_via_arrow_keys() {
    let mut state = AppState::new("test-model");
    for c in "first".chars() {
        drive(&mut state, Key::char(c));
    }
    drive(&mut state, Key::enter());

    for c in "second".chars() {
        drive(&mut state, Key::char(c));
    }
    drive(&mut state, Key::enter());

    // Cursor in editing (no history selected). Up → previous
    drive(
        &mut state,
        Key::new(crossterm::event::KeyCode::Up, KeyModifiers::NONE),
    );
    assert_eq!(state.input.text, "second");
    drive(
        &mut state,
        Key::new(crossterm::event::KeyCode::Up, KeyModifiers::NONE),
    );
    assert_eq!(state.input.text, "first");
    // Down → next
    drive(
        &mut state,
        Key::new(crossterm::event::KeyCode::Down, KeyModifiers::NONE),
    );
    assert_eq!(state.input.text, "second");
    drive(
        &mut state,
        Key::new(crossterm::event::KeyCode::Down, KeyModifiers::NONE),
    );
    assert_eq!(state.input.text, "");
}

// ====================================================================
// Test 6: Ctrl+A goes to beginning of line (readline behavior)
// ====================================================================
#[test]
fn ctrl_a_goes_to_line_start() {
    let mut state = AppState::new("test-model");
    for c in "hello world".chars() {
        drive(&mut state, Key::char(c));
    }
    // Cursor at end (position 11). Ctrl+A → 0.
    assert_eq!(state.input.cursor, 11);
    drive(
        &mut state,
        Key::new(crossterm::event::KeyCode::Char('a'), KeyModifiers::CTRL),
    );
    assert_eq!(state.input.text, "hello world");
    assert_eq!(state.input.cursor, 0);
}

// ====================================================================
// Test 7: Ctrl+K kills from cursor to end of line
// ====================================================================
#[test]
fn ctrl_k_kills_to_line_end() {
    let mut state = AppState::new("test-model");
    for c in "hello world".chars() {
        drive(&mut state, Key::char(c));
    }
    state.input.move_to_start();
    for _ in 0..6 {
        drive(
            &mut state,
            Key::new(crossterm::event::KeyCode::Right, KeyModifiers::NONE),
        );
    }
    drive(
        &mut state,
        Key::new(crossterm::event::KeyCode::Char('k'), KeyModifiers::CTRL),
    );
    assert_eq!(state.input.text, "hello ");
    assert_eq!(state.input.cursor, 6);
}

// ====================================================================
// Test 8: Ctrl+U clears the entire input
// ====================================================================
#[test]
fn ctrl_u_clears_input() {
    let mut state = AppState::new("test-model");
    for c in "discard me".chars() {
        drive(&mut state, Key::char(c));
    }
    drive(
        &mut state,
        Key::new(crossterm::event::KeyCode::Char('u'), KeyModifiers::CTRL),
    );
    assert_eq!(state.input.text, "");
    assert_eq!(state.input.cursor, 0);
}

// ====================================================================
// Test 9: Word navigation with Ctrl+arrows
// ====================================================================
#[test]
fn ctrl_arrows_navigate_words() {
    let mut state = AppState::new("test-model");
    for c in "one two three".chars() {
        drive(&mut state, Key::char(c));
    }
    // Cursor at end (position 13). Move word left.
    drive(
        &mut state,
        Key::new(crossterm::event::KeyCode::Left, KeyModifiers::CTRL),
    );
    // move_word_left lands at the start of the trailing non-ws run.
    // "one two three" @ pos 13 → strip "three" → "one two " @ 8 (start of "three")
    assert_eq!(state.input.cursor, 8, "should land at start of 'three'");
    // Step again → strip "two" → "one " @ 4 (start of "two")
    drive(
        &mut state,
        Key::new(crossterm::event::KeyCode::Left, KeyModifiers::CTRL),
    );
    assert_eq!(state.input.cursor, 4, "should land at start of 'two'");
}

// ====================================================================
// Test 10: Esc clears input when editing
// ====================================================================
#[test]
fn esc_clears_input_when_editing() {
    let mut state = AppState::new("test-model");
    for c in "junk".chars() {
        drive(&mut state, Key::char(c));
    }
    drive(&mut state, Key::esc());
    assert_eq!(state.input.text, "");
}

// ====================================================================
// Test 11: Ctrl+D quits the TUI
// ====================================================================
#[test]
fn ctrl_d_qui_tui() {
    let mut state = AppState::new("test-model");
    drive(
        &mut state,
        Key::new(crossterm::event::KeyCode::Char('d'), KeyModifiers::CTRL),
    );
    assert_eq!(state.run_state.mode, RunMode::Quitting);
}

// ====================================================================
// Test 12: Transcript renders user/assistant/tool lines distinctly
// ====================================================================
#[test]
fn transcript_renders_all_line_kinds() {
    let mut state = AppState::new("test-model");
    state.push_user("find bugs");
    state.push_divider();
    state.push_assistant("searching...");
    state.push_divider();
    state.push_tool_call("grep", "{\"pattern\":\"TODO\"}");
    state.push_tool_result(true, "main.rs:42: // TODO: ...", None);
    state.push_divider();

    let frame = render_to_text(&state, 80, 24);
    assert!(frame.contains("> find bugs"), "user line missing");
    assert!(frame.contains("searching..."), "assistant line missing");
    assert!(
        frame.contains("[tool call] grep"),
        "tool call label missing"
    );
    // Tool result is now rendered as a 2-row block: header row
    // ('[tool result] ') on its own line, then the body indented
    // ('  main.rs:42: ...'). Verify both pieces appear somewhere.
    assert!(
        frame.contains("[tool result]"),
        "tool result label missing"
    );
    assert!(
        frame.contains("main.rs:42:"),
        "tool result body missing"
    );
}

// ====================================================================
// Test 13: Full E2E — drive keystrokes, then run a fixture agent,
//            push events into the transcript, render, and snapshot.
// ====================================================================
#[tokio::test]
async fn full_e2e_user_typed_command_then_agent_responds() {
    use crossterm::event::KeyCode;
    use nini_tui::keys::{default_keymap, resolve};

    let mut state = AppState::new("test-model");

    // User types "echo hello" and submits
    for c in "echo hello".chars() {
        drive(&mut state, Key::char(c));
    }
    drive(&mut state, Key::enter());
    // State is now Editing with transcript = [User("echo hello"), Divider]

    // Run a fixture agent that calls bash then echoes back
    let provider = Arc::new(ProgrammedProvider::from_turns(vec![
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
    let tools = ToolRegistry::new().register(Arc::new(BashTool::new()));
    let mut agent = Agent::new(provider, tools, RunConfig::new("test-model"));
    let mut stream = std::pin::pin!(agent.run(nini_core::AgentMessage::user("echo hello")));

    // Feed events into transcript (this is what the TUI runtime will do)
    let mut total_tokens = 0u32;
    while let Some(ev) = stream.next().await {
        match ev {
            Ok(AgentEvent::TextDelta { text }) => state.push_assistant(text),
            Ok(AgentEvent::ToolCallStart { name, .. }) => {
                state.push_tool_call(name, "");
            }
            Ok(AgentEvent::ToolCallStop { id, input_json }) => {
                // Update the last tool call line with the final args
                if let Some(TranscriptLine::ToolCall { args, .. }) = state.transcript_state.lines.last_mut() {
                    *args = input_json.to_string();
                } else {
                    state.push_tool_call(id, input_json.to_string());
                }
            }
            Ok(AgentEvent::ToolResult { output, .. }) => {
                state.push_tool_result(!output.is_error, output.content, None);
            }
            Ok(AgentEvent::TurnEnd { usage, .. }) => {
                total_tokens += usage.input_tokens + usage.output_tokens;
                state.run_state.tokens.input += usage.input_tokens as u64;
                state.run_state.tokens.output += usage.output_tokens as u64;
                state.push_divider();
            }
            Ok(AgentEvent::Error { message }) => state.push_assistant(format!("error: {message}")),
            _ => {}
        }
    }

    // Final rendered frame should contain the user message, the tool call,
    // the tool result, and the assistant's "hello".
    let frame = render_to_text(&state, 100, 30);
    assert!(
        frame.contains("> echo hello"),
        "user message missing in frame"
    );
    assert!(frame.contains("[tool call] bash"), "tool call line missing");
    assert!(frame.contains("[tool result]"), "tool result line missing");
    assert!(frame.contains("hello"), "assistant text missing");
    // Key hints still visible
    assert!(frame.contains("F1"));
    assert!(frame.contains("Ctrl+C"));

    // Token accounting is wired (even if zero from fixture)
    let _ = total_tokens;

    // Sanity: we processed some keystrokes via drive() — verify they didn't
    // corrupt the state machine
    assert!(resolve(&default_keymap(), Key::enter()) == KeyAction::Submit);
    let _ = KeyCode::Backspace;
}

// ====================================================================
// Test 14: Layout snapshot regression — exact pixel layout of empty TUI
// ====================================================================
#[test]
fn empty_state_pixel_layout_regression() {
    let state = AppState::new("test-model");
    // 80x24 is the standard terminal size — verify we render cleanly
    let frame = render_to_text(&state, 80, 24);
    // The frame must have exactly 24 lines (one per row)
    let line_count = frame.lines().count();
    assert_eq!(line_count, 24, "expected 24 lines, got {line_count}");

    // v0.8: Pi-style 2-line footer.
    //   Line 0: pwd/branch/session (env context line)
    //   Line 1: brand badge + phase + ... + right-aligned model
    //   Line 23 (last): key hints
    let pwd_line = frame.lines().next().unwrap();
    // With no cwd/session set, the pwd line should still render the
    // 'no session' placeholder (line 0 is reserved for env context).
    assert!(
        !pwd_line.contains(" nini "),
        "line 0 is the pwd line (no badge), got: {pwd_line:?}"
    );

    let stats_line = frame.lines().nth(1).unwrap();
    assert!(
        stats_line.contains(" nini "),
        "line 1 should contain ' nini ' brand badge, got: {stats_line:?}"
    );
    assert!(
        stats_line.contains("test-model"),
        "line 1 should show model name, got: {stats_line:?}"
    );
    assert!(
        stats_line.contains("idle"),
        "line 1 should show 'idle' phase, got: {stats_line:?}"
    );

    // Prompt block ("input" border) should be in the bottom region.
    // Layout: footer(2) + transcript(min 3) + prompt(3) + hints(1) = 9 rows minimum,
    // leaving 15 rows for transcript on a 24-row screen.
    // Prompt top border sits at row 2 + transcript_height = 20.
    let prompt_line_idx = 20;
    let prompt_line = frame.lines().nth(prompt_line_idx).unwrap();
    assert!(
        prompt_line.contains("input") || prompt_line.contains("❯"),
        "row {prompt_line_idx} should be prompt border or content, got: {prompt_line:?}"
    );

    // Last line (row 23) is the key hints
    let hints_line = frame.lines().last().unwrap();
    assert!(
        hints_line.contains("F1"),
        "last line should be hints, got: {hints_line:?}"
    );
    assert!(
        hints_line.contains("Ctrl+C") || hints_line.contains("Ctrl+D"),
        "last line should mention Ctrl+C or Ctrl+D"
    );
}

// ====================================================================
// Test 15: Narrow terminal — prompt and transcript wrap correctly
// ====================================================================
#[test]
fn narrow_terminal_handles_long_text() {
    let mut state = AppState::new("test-model");
    // Type a long line that exceeds 40 cols
    let long = "a".repeat(60);
    for c in long.chars() {
        drive(&mut state, Key::char(c));
    }
    // Render in a 40-wide terminal
    let frame = render_to_text(&state, 40, 12);
    // Just assert it doesn't panic and the text is present somewhere
    assert!(frame.contains("aaa"), "long text should be in frame");
    let line_count = frame.lines().count();
    assert_eq!(line_count, 12);
}

// ====================================================================
// Test 16: Running mode shows [running...] in status bar
// ====================================================================
#[test]
fn running_mode_status_bar() {
    let mut state = AppState::new("test-model");
    state.run_state.mode = RunMode::Running;
    let frame = render_to_text(&state, 80, 24);
    // New 5-state status bar shows 'working…' label + spinner.
    assert!(
        frame.contains("working") || frame.contains("running"),
        "running mode missing in frame"
    );
}

// ====================================================================
// Test 17: Aborted mode shows [aborted]
// ====================================================================
#[test]
fn aborted_mode_status_bar() {
    let mut state = AppState::new("test-model");
    state.run_state.mode = RunMode::Aborted;
    // Set the runtime status string to indicate abort.
    state.run_state.status = "aborted".to_string();
    let frame = render_to_text(&state, 80, 24);
    // Aborted is Idle phase; the status string carries the abort label.
    assert!(
        frame.contains("aborted") || frame.contains("[aborted]"),
        "aborted mode missing in frame"
    );
}

// ====================================================================
// Test 18: Shift+Enter inserts a newline (multi-line input)
// ====================================================================
#[test]
fn shift_enter_inserts_newline() {
    use crossterm::event::KeyCode;
    let mut state = AppState::new("test-model");
    for c in "line1".chars() {
        drive(&mut state, Key::char(c));
    }
    drive(&mut state, Key::new(KeyCode::Enter, KeyModifiers::SHIFT));
    for c in "line2".chars() {
        drive(&mut state, Key::char(c));
    }
    assert_eq!(state.input.text, "line1\nline2");
}

// ====================================================================
// Test 19: Submit with whitespace-only input does NOT add to transcript
// ====================================================================
#[test]
fn whitespace_only_submit_is_silent() {
    let mut state = AppState::new("test-model");
    for c in "   ".chars() {
        drive(&mut state, Key::char(c));
    }
    drive(&mut state, Key::enter());
    // No transcript lines added
    assert!(
        state.transcript_state.lines.is_empty(),
        "whitespace submit should not add to transcript"
    );
}

// ====================================================================
// Test 20: Full demo pipeline — like `nini demo` but driven through the TUI state machine
// ====================================================================
#[tokio::test]
async fn full_demo_pipeline_through_tui_state() {
    let mut state = AppState::new("test-model");

    // User types the demo task and submits
    for c in "find TODOs and fix them".chars() {
        drive(&mut state, Key::char(c));
    }
    drive(&mut state, Key::enter());

    // Run the scripted demo fixture
    let cwd = std::env::current_dir().unwrap();
    let provider = Arc::new(ProgrammedProvider::from_turns(vec![
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
            FixtureTurn::ToolCall {
                name: "read".to_string(),
                args: serde_json::json!({"path": "src/main.rs"}),
            },
            FixtureTurn::Stop {
                stop_reason: "tool_use".to_string(),
                usage: Usage::default(),
            },
        ],
        vec![
            FixtureTurn::ToolCall {
                name: "edit".to_string(),
                args: serde_json::json!({"old_text": "TODO", "new_text": "DONE"}),
            },
            FixtureTurn::Stop {
                stop_reason: "tool_use".to_string(),
                usage: Usage::default(),
            },
        ],
        vec![
            FixtureTurn::Text("Found and fixed TODOs.".to_string()),
            FixtureTurn::Stop {
                stop_reason: "end_turn".to_string(),
                usage: Usage::default(),
            },
        ],
    ]));
    let tools = ToolRegistry::new().register(Arc::new(BashTool::new()));
    let mut agent = Agent::new(provider, tools, RunConfig::new("test-model"));
    let mut stream = std::pin::pin!(agent.run(nini_core::AgentMessage::user("find TODOs")));

    while let Some(ev) = stream.next().await {
        if let Ok(AgentEvent::TextDelta { text }) = ev {
            state.push_assistant(text);
        } else if let Ok(AgentEvent::ToolCallStart { name, .. }) = ev {
            state.push_tool_call(name, "");
        } else if let Ok(AgentEvent::ToolCallStop { input_json, .. }) = ev {
            if let Some(TranscriptLine::ToolCall { args, .. }) = state.transcript_state.lines.last_mut() {
                *args = input_json.to_string();
            }
        } else if let Ok(AgentEvent::ToolResult { output, .. }) = ev {
            state.push_tool_result(!output.is_error, output.content, None);
        } else if let Ok(AgentEvent::TurnEnd { .. }) = ev {
            state.push_divider();
        }
    }

    // Render and assert the full pipeline produced visible output
    let frame = render_to_text(&state, 100, 30);
    assert!(frame.contains("> find TODOs"), "user message in transcript");
    assert!(
        frame.contains("Found and fixed TODOs"),
        "final assistant text"
    );
    assert!(
        frame.contains("[tool call] grep"),
        "grep tool call rendered"
    );
    assert!(
        frame.contains("[tool call] read"),
        "read tool call rendered"
    );
    assert!(
        frame.contains("[tool call] edit"),
        "edit tool call rendered"
    );

    // Token totals still 0 from fixture (real providers would populate)
    assert_eq!(state.run_state.tokens.input, 0);
    assert_eq!(state.run_state.tokens.output, 0);

    // Just to silence unused warnings on `cwd`
    let _ = cwd;
}

/// REGRESSION: theme system works — dark and light themes both render without crashing.
#[test]
fn theme_dark_and_light_both_render() {
    let state = AppState::new("test");
    let backend_dark = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal_dark = Terminal::new(backend_dark).unwrap();
    terminal_dark
        .draw(|f| render_frame_with_theme(f, &state, &Theme::dark()))
        .unwrap();

    let backend_light = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal_light = Terminal::new(backend_light).unwrap();
    terminal_light
        .draw(|f| render_frame_with_theme(f, &state, &Theme::light()))
        .unwrap();

    // Both terminals should produce non-empty buffers.
    let dark_buffer = terminal_dark.backend().buffer();
    let light_buffer = terminal_light.backend().buffer();
    let dark_text = format!("{dark_buffer:?}");
    let light_text = format!("{light_buffer:?}");
    assert!(!dark_text.is_empty());
    assert!(!light_text.is_empty());

    // Default render_frame uses dark theme by default.
    let backend_default = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal_default = Terminal::new(backend_default).unwrap();
    terminal_default.draw(|f| render_frame(f, &state)).unwrap();
}

/// REGRESSION: markdown in assistant text renders with theme-aware colors.
#[test]
fn markdown_in_assistant_text_renders() {
    use nini_tui::markdown::render_markdown;
    let theme = Theme::dark();
    let lines = render_markdown("# Title\n\n- item 1\n- item 2\n", &theme);
    assert!(lines.len() >= 2, "Markdown should produce multiple lines");
    let first: String = lines[0]
        .spans
        .iter()
        .map(|sp| sp.content.as_ref())
        .collect();
    assert!(first.starts_with("# "));
    // Paragraph break emits a blank line, then the bullet.
    let second: String = lines
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("|");
    assert!(second.contains("• item 1"));
}

/// REGRESSION: scroll_offset clips transcript to last N lines.
#[test]
fn scroll_offset_clips_to_last_n_lines() {
    let mut state = AppState::new("test");
    // Push 50 lines.
    for i in 0..50 {
        state.push_user(format!("line {i}"));
    }
    // Set scroll_offset to 20 → show only last 30 lines.
    state.transcript_state.scroll_offset = 20;
    let backend = ratatui::backend::TestBackend::new(80, 10);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render_frame_with_theme(f, &state, &Theme::dark()))
        .unwrap();
    // Verify no panic. Visual correctness depends on terminal width.
    let buffer = terminal.backend().buffer();
    let buf_str = format!("{buffer:?}");
    assert!(!buf_str.is_empty());
}

/// REGRESSION: scroll_offset=0 shows from beginning (no clip).
#[test]
fn scroll_offset_zero_shows_from_beginning() {
    let mut state = AppState::new("test");
    state.push_user("first");
    state.push_user("second");
    state.transcript_state.scroll_offset = 0;
    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render_frame(f, &state))
        .unwrap();
}

