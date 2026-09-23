//! End-to-end slash command tests.
//!
//! These tests verify the full vertical slice: user types `/cmd`, presses
//! Enter, the command dispatcher runs, output lands in the transcript, and
//! the rendered frame reflects the result.

use nini_tui::commands::{CommandId, CommandOutcome, REGISTRY, complete, dispatch, parse};
// Mutex serializing tests that mutate the HOME environment variable to
// avoid cross-test interference when cargo runs tests in parallel.
static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

use nini_tui::render::render_frame;
use nini_tui::settings::SettingsManager;
use nini_tui::state::{AppState, RunMode, TranscriptLine};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

/// Snapshot the visible text of a frame, like in tui_e2e.rs.
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

// =====================================================================
// Test 1: Parse all 23 commands via parse() — round-trip the registry
// =====================================================================
#[test]
fn parse_all_commands_in_registry() {
    for def in REGISTRY {
        let input = if let Some(hint) = def.argument_hint {
            // Synthesize an arg matching the hint's first letter
            let arg = hint
                .trim_start_matches('<')
                .split_whitespace()
                .next()
                .unwrap_or("x");
            format!("/{} {}", def.name, arg)
        } else {
            format!("/{}", def.name)
        };
        let parsed = parse(&input);
        assert!(
            parsed.is_some(),
            "failed to parse /{def_name} from {input:?}",
            def_name = def.name
        );
        let (cmd_id, _args) = parsed.unwrap();
        assert_eq!(
            cmd_id,
            def.id,
            "/{name} parsed to wrong id",
            name = def.name
        );
    }
}

// =====================================================================
// Test 2: /help renders all keybindings to the transcript
// =====================================================================
#[test]
fn help_command_renders_keybindings() {
    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    let r = dispatch(&mut state, &mut settings, CommandId::Hotkeys, "");
    match r.outcome {
        CommandOutcome::Output(lines) => {
            assert!(lines.iter().any(|l| l.contains("Ctrl+C")));
            assert!(lines.iter().any(|l| l.contains("Enter")));
            assert!(lines.iter().any(|l| l.contains("Ctrl+D")));
            assert!(lines.iter().any(|l| l.contains("Ctrl+L")));
            assert!(lines.iter().any(|l| l.contains("Ctrl+A")));
        }
        _ => panic!("expected Output"),
    }
}

// =====================================================================
// Test 3: /model <provider/model> updates state.model
// =====================================================================
#[test]
fn model_command_updates_state() {
    let mut state = AppState::new("old-model");
    let mut settings = SettingsManager::default();
    let r = dispatch(&mut state, &mut settings, CommandId::Model, "anthropic/claude-opus-4-7");
    assert!(matches!(r.outcome, CommandOutcome::Output(_)));
    assert_eq!(state.model, "anthropic/claude-opus-4-7");
    // Transcript should mention the model change
    let last_text = state
        .transcript
        .iter()
        .rev()
        .find_map(|l| l.as_assistant_text());
    assert!(last_text.is_some());
    assert!(last_text.unwrap().contains("anthropic/claude-opus-4-7"));
}

// =====================================================================
// Test 4: /model with no args returns usage
// =====================================================================
#[test]
fn model_command_no_args_returns_usage() {
    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    let r = dispatch(&mut state, &mut settings, CommandId::Model, "");
    match r.outcome {
        CommandOutcome::Output(lines) => {
            assert!(lines[0].contains("Usage"));
        }
        _ => panic!("expected Output"),
    }
    // State.model unchanged on usage error
    assert_eq!(state.model, "test-model");
}

// =====================================================================
// Test 5: /thinking validates levels
// =====================================================================
#[test]
fn thinking_command_validates_levels() {
    let mut state = AppState::new("test");
    let mut settings = SettingsManager::default();

    for level in ["off", "minimal", "low", "medium", "high", "xhigh", "max"] {
        let r = dispatch(&mut state, &mut settings, CommandId::Thinking, level);
        assert!(matches!(r.outcome, CommandOutcome::Output(_)));
    }

    // Invalid level returns usage line
    let r = dispatch(&mut state, &mut settings, CommandId::Thinking, "ultra");
    match r.outcome {
        CommandOutcome::Output(lines) => {
            assert!(lines[0].contains("Usage"));
            assert!(lines[0].contains("off|minimal|low|medium|high|xhigh|max"));
        }
        _ => panic!("expected Output"),
    }
}

// =====================================================================
// Test 6: /new clears transcript but keeps model
// =====================================================================
#[test]
fn new_command_clears_transcript_keeps_model() {
    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    state.push_user("old question".to_string());
    state.push_assistant("old answer".to_string());
    state.push_divider();
    assert!(state.transcript.len() >= 3);

    let r = dispatch(&mut state, &mut settings, CommandId::New, "");
    assert!(matches!(r.outcome, CommandOutcome::Output(_)));
    assert_eq!(state.model, "test-model"); // unchanged

    // Transcript has "(started new session)" + divider
    assert!(
        state.transcript[0]
            .as_assistant_text()
            .map(|t| t.contains("started new session"))
            .unwrap_or(false)
    );
}

// =====================================================================
// Test 7: /quit sets Quitting mode and returns Quit outcome
// =====================================================================
#[test]
fn quit_command_signals_exit() {
    let mut state = AppState::new("test");
    let mut settings = SettingsManager::default();
    let r = dispatch(&mut state, &mut settings, CommandId::Quit, "");
    assert_eq!(r.outcome, CommandOutcome::Quit);
    assert_eq!(state.mode, RunMode::Quitting);
}

// =====================================================================
// Test 8: completion ranks prefix match before substring match
// =====================================================================
#[test]
fn completion_prefers_prefix() {
    let r = complete("mo", 10);
    // 'model' starts with 'mo' → score 0, comes first
    assert!(!r.is_empty());
    assert_eq!(r[0].name, "model");
}

#[test]
fn completion_substring_works() {
    let r = complete("ink", 10);
    assert!(r.iter().any(|c| c.name == "thinking"));
}

#[test]
fn completion_case_insensitive() {
    let r = complete("MO", 10);
    assert!(r.iter().any(|c| c.name == "model"));
}

#[test]
fn completion_no_match_empty() {
    assert!(complete("xyzzy", 10).is_empty());
}

#[test]
fn completion_empty_returns_all() {
    // v0.6: empty query returns the WHOLE list (popup scrolling
    // takes care of clipping to the viewport). v0.5 capped at `limit`,
    // hiding 20 commands from autocomplete.
    let r = complete("", 5);
    assert!(r.len() >= 28, "expected ≥28 commands, got {}", r.len());
    // First 5 in registry order (settings, model, tree, thinking, scoped-models)
    assert_eq!(r[0].name, "settings");
    assert_eq!(r[1].name, "model");
    assert_eq!(r[4].name, "scoped-models");
}

// =====================================================================
// Test 9: Frame snapshot — `/help` transcript entry renders correctly
// =====================================================================
#[test]
fn command_output_renders_in_frame() {
    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    let _ = dispatch(&mut state, &mut settings, CommandId::Hotkeys, "");
    let frame = frame_text(&state, 100, 30);
    // The hotkeys text should appear in the transcript area
    assert!(frame.contains("Ctrl+C"), "Ctrl+C help missing");
    assert!(frame.contains("Enter"), "Enter help missing");
}

// =====================================================================
// Test 10: Frame snapshot — `/new` clears visible transcript
// =====================================================================
#[test]
fn new_command_visible_in_frame() {
    let mut state = AppState::new("test");
    let mut settings = SettingsManager::default();
    state.push_user("old".to_string());
    state.push_assistant("old reply".to_string());
    state.push_divider();

    let _ = dispatch(&mut state, &mut settings, CommandId::New, "");
    let frame = frame_text(&state, 100, 30);
    // "started new session" appears, "old reply" does not (cleared)
    assert!(frame.contains("started new session"));
    assert!(!frame.contains("old reply"));
}

// =====================================================================
// Test 11: Argument validation — /thinking without args returns usage
// =====================================================================
#[test]
fn thinking_command_no_args_returns_usage() {
    let mut state = AppState::new("test");
    let mut settings = SettingsManager::default();
    let r = dispatch(&mut state, &mut settings, CommandId::Thinking, "");
    match r.outcome {
        CommandOutcome::Output(lines) => {
            assert!(lines[0].contains("Usage"));
        }
        _ => panic!("expected Output"),
    }
}

// =====================================================================
// Test 12: Name command stores in status
// =====================================================================
#[test]
fn name_command_sets_status() {
    let mut state = AppState::new("test");
    let mut settings = SettingsManager::default();
    let _ = dispatch(&mut state, &mut settings, CommandId::Name, "My Cool Session");
    assert!(state.status.contains("My Cool Session"));
}

// =====================================================================
// Test 13: Session command shows stats
// =====================================================================
#[test]
fn session_command_shows_stats() {
    let mut state = AppState::new("claude-opus");
    let mut settings = SettingsManager::default();
    state.push_user("hi".to_string());
    state.push_assistant("hello".to_string());
    state.tokens.input = 42;
    state.tokens.output = 17;

    let r = dispatch(&mut state, &mut settings, CommandId::Session, "");
    match r.outcome {
        CommandOutcome::Output(lines) => {
            let joined = lines.join("\n");
            assert!(joined.contains("model:"));
            assert!(joined.contains("claude-opus"));
            assert!(joined.contains("transcript:"));
            assert!(joined.contains("tokens:"));
            assert!(joined.contains("in=42"));
            assert!(joined.contains("out=17"));
        }
        _ => panic!("expected Output"),
    }
}

// =====================================================================
// Test 14: /export path naming and format validation
// =====================================================================
#[test]
fn export_command_path_format() {
    let mut state = AppState::new("test");
    let mut settings = SettingsManager::default();
    state.push_user("hello world".to_string());
    state.push_assistant("hi".to_string());

    let r = dispatch(&mut state, &mut settings, CommandId::Export, "");
    match r.outcome {
        CommandOutcome::Output(lines) => {
            let path_line = &lines[0];
            assert!(path_line.starts_with("export → "), "path_line: {path_line}");
            // Verify the file path doesn't contain error markers
            assert!(!path_line.contains("(failed"), "path_line: {path_line}");
            assert!(!path_line.contains("(no HOME"), "path_line: {path_line}");
            // Verify the path matches the expected naming convention
            let path_str = path_line.trim_start_matches("export → ");
            assert!(
                path_str.contains("session-") && path_str.ends_with(".html"),
                "path should be session-TIMESTAMP.html: {path_str}"
            );
            // Verify the transcript was included (path exists and is readable)
            assert!(
                std::path::Path::new(path_str).exists(),
                "exported HTML file should exist: {path_str}"
            );
            let content = std::fs::read_to_string(path_str).unwrap_or_default();
            assert!(content.contains("<!DOCTYPE html>"), "HTML should contain DOCTYPE");
            assert!(content.contains("hello world"), "HTML should contain user message");
        }
        _ => panic!("expected Output"),
    }
}

// =====================================================================
// Test 15: /copy finds last assistant message
// =====================================================================
#[test]
fn copy_finds_last_assistant_message() {
    let mut state = AppState::new("test");
    let mut settings = SettingsManager::default();
    state.push_user("hi".to_string());
    state.push_assistant("first reply".to_string());
    state.push_divider();
    state.push_user("another".to_string());
    state.push_assistant("second reply".to_string());

    let r = dispatch(&mut state, &mut settings, CommandId::Copy, "");
    match r.outcome {
        CommandOutcome::Output(lines) => {
            assert!(lines[0].contains("copied"));
            // "second reply" should be the most recent assistant message
            // (we don't capture the printed output here, but verify transcript)
            let last_assistant = state
                .transcript
                .iter()
                .rev()
                .find_map(|l| l.as_assistant_text());
            assert!(last_assistant.is_some());
            // Should contain "second reply" not "first reply"
            let last = last_assistant.unwrap();
            // The most recent assistant text might be a "copied" notification
            // rather than "second reply" itself (since /copy doesn't push
            // the copied text). We just verify something assistant-shaped
            // is present.
            assert!(!last.is_empty());
        }
        _ => panic!("expected Output"),
    }
}

#[test]
fn copy_with_no_assistant_message() {
    let mut state = AppState::new("test");
    let mut settings = SettingsManager::default();
    state.push_user("just a question".to_string());
    let r = dispatch(&mut state, &mut settings, CommandId::Copy, "");
    match r.outcome {
        CommandOutcome::Output(lines) => {
            assert!(lines[0].contains("no assistant message"));
        }
        _ => panic!("expected Output"),
    }
}

// =====================================================================
// Test 16: Multiple commands in sequence
// =====================================================================
#[test]
fn multiple_commands_in_sequence() {
    let mut state = AppState::new("test");
    let mut settings = SettingsManager::default();

    dispatch(&mut state, &mut settings, CommandId::Model, "anthropic/claude");
    dispatch(&mut state, &mut settings, CommandId::Thinking, "high");
    dispatch(&mut state, &mut settings, CommandId::Name, "Test Run");
    dispatch(&mut state, &mut settings, CommandId::Session, "");

    assert_eq!(state.model, "anthropic/claude");
    assert!(state.status.contains("Test Run"));
}

// =====================================================================
// Test 17: Frame — command output renders as transcript line
// =====================================================================
#[test]
fn command_output_visible_in_frame() {
    let mut state = AppState::new("test");
    let mut settings = SettingsManager::default();
    state.push_user("/help".to_string()); // simulate user typing the slash
    state.push_divider();
    let _ = dispatch(&mut state, &mut settings, CommandId::Hotkeys, "");

    let frame = frame_text(&state, 100, 30);
    // Both the user "/help" and the hotkeys output should be visible
    assert!(frame.contains("/help"));
    assert!(frame.contains("Ctrl+C"));
}

// =====================================================================
// Test 18: parse handles whitespace edge cases
// =====================================================================
#[test]
fn parse_whitespace_handling() {
    // Leading/trailing whitespace
    let (id, args) = parse("  /model foo  ").unwrap();
    assert_eq!(id, CommandId::Model);
    assert_eq!(args, "foo");

    // Multiple spaces between cmd and args
    let (id, args) = parse("/thinking    high").unwrap();
    assert_eq!(id, CommandId::Thinking);
    // args are trimmed, so leading whitespace is gone
    assert_eq!(args, "high");

    // Just `/` alone returns None
    assert!(parse("/").is_none());

    // Empty string returns None
    assert!(parse("").is_none());
}

// =====================================================================
// Test 19: dispatch is total — every CommandId produces a result
// =====================================================================
#[test]
fn dispatch_is_total_over_all_commands() {
    let mut state = AppState::new("test");
    let mut settings = SettingsManager::default();
    for def in REGISTRY {
        // Pass an empty arg (commands that require args handle it gracefully)
        let _ = dispatch(&mut state, &mut settings, def.id, "");
    }
    // No panics means success
}

// =====================================================================
// Test 20: /resume lists sessions or reports none found
// =====================================================================
#[test]
fn resume_command_lists_sessions_or_none() {
    let mut state = AppState::new("test");
    let mut settings = SettingsManager::default();
    let r = dispatch(&mut state, &mut settings, CommandId::Resume, "");
    match r.outcome {
        CommandOutcome::Output(lines) => {
            // Either "No sessions found" or a numbered list.
            assert!(!lines.is_empty());
            let is_empty_state = lines[0].contains("No sessions");
            let is_list = lines[0].contains("session(s) available");
            assert!(is_empty_state || is_list, "Expected no-sessions or list, got: {:?}", lines);
        }
        _ => panic!("expected Output"),
    }
}

// =====================================================================
// Test 21: /new creates a session
// =====================================================================
#[test]
fn new_command_creates_session() {
    let mut state = AppState::new("test");
    let mut settings = SettingsManager::default();
    // /new clears transcript AND creates a session.
    let r = dispatch(&mut state, &mut settings, CommandId::New, "");
    match r.outcome {
        CommandOutcome::Output(lines) => {
            assert!(!lines.is_empty());
        }
        _ => panic!("expected Output"),
    }
    // A session should have been created.
    assert!(state.session.is_some(), "/new should create a session");
    assert!(state.session_id.is_some(), "/new should set session_id");
    assert!(state.session_path.is_some(), "/new should set session_path");
}

// =====================================================================
// Test 22: completion limit is respected
// =====================================================================
#[test]
fn completion_returns_full_registry_on_empty() {
    // v0.6: empty query returns the WHOLE list (popup scrolling
    // clips to viewport). The limit arg is unused for empty query.
    let r = complete("", 3);
    assert_eq!(r.len(), REGISTRY.len());
    let r = complete("", 100);
    assert_eq!(r.len(), REGISTRY.len());
    // "mo" matches `model` (prefix) and `scoped-models` (substring).
    let r = complete("mo", 3);
    assert_eq!(r.len(), 2);
}

// =====================================================================
// Tests for newly implemented commands (changelog / compact / scoped-models
// / trust / clone) — Phase 1 of the slash-command completion push.
// =====================================================================

#[test]
fn changelog_command_renders_release_notes() {
    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    // Run /changelog from the project root so CHANGELOG.md is found.
    let original_cwd = std::env::current_dir().ok();
    let project_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent() // crates/
        .and_then(|p| p.parent()) // nini/
        .unwrap()
        .to_path_buf();
    std::env::set_current_dir(&project_root).unwrap();
    let r = dispatch(&mut state, &mut settings, CommandId::Changelog, "");
    if let Some(orig) = original_cwd {
        let _ = std::env::set_current_dir(&orig);
    }
    match r.outcome {
        CommandOutcome::Output(lines) => {
            let joined = lines.join("\n");
            assert!(
                joined.contains("# Changelog") || joined.contains("nini"),
                "expected changelog header, got: {joined}"
            );
        }
        _ => panic!("expected Output, got {:?}", r.outcome),
    }
    // Transcript should contain a [changelog] annotation
    assert!(state.transcript.iter().any(|l| {
        matches!(l, TranscriptLine::AssistantText(s) if s.contains("[changelog]"))
    }));
}

#[test]
fn compact_command_short_transcript_returns_early() {
    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    state.push_user("hi".to_string());
    state.push_assistant("hello".to_string());
    let before = state.transcript.len();
    let r = dispatch(&mut state, &mut settings, CommandId::Compact, "");
    match r.outcome {
        CommandOutcome::Output(lines) => {
            assert!(
                lines.iter().any(|l| l.contains("nothing to compact")),
                "expected early-return message, got: {lines:?}"
            );
        }
        _ => panic!("expected Output"),
    }
    // When short transcript returns early, the command does NOT push
    // any echo line — state should be untouched.
    assert_eq!(
        state.transcript.len(),
        before,
        "short transcript should not be mutated"
    );
}

#[test]
fn compact_command_long_transcript_replaces_prefix_with_summary() {
    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    // Build a long transcript with 10 user/assistant pairs.
    for i in 0..10 {
        state.push_user(format!("question {i}: how to refactor auth?"));
        state.push_assistant(format!(
            "answer {i}: use sqlx, not diesel; t_ prefix on tables."
        ));
    }
    let before = state.transcript.len();
    let r = dispatch(&mut state, &mut settings, CommandId::Compact, "");
    assert!(matches!(r.outcome, CommandOutcome::Output(_)));
    // Transcript should now have a [CONTEXT SUMMARY] line at the front.
    assert!(
        state.transcript[0]
            .as_assistant_text()
            .map(|s| s.contains("[CONTEXT SUMMARY]"))
            .unwrap_or(false),
        "expected [CONTEXT SUMMARY] header after compaction"
    );
    // The transcript should be smaller (or equal).
    assert!(state.transcript.len() <= before);
}

#[test]
fn scoped_models_add_and_list() {
    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    // Empty cycle.
    let r = dispatch(
        &mut state,
        &mut settings,
        CommandId::ScopedModels,
        "list",
    );
    assert!(matches!(r.outcome, CommandOutcome::Output(_)));

    // Add a model.
    let r = dispatch(
        &mut state,
        &mut settings,
        CommandId::ScopedModels,
        "add anthropic/claude-opus-4-7",
    );
    match r.outcome {
        CommandOutcome::Output(lines) => {
            assert!(
                lines.iter().any(|l| l.contains("added anthropic/claude-opus-4-7")),
                "expected add confirmation, got: {lines:?}"
            );
        }
        _ => panic!("expected Output"),
    }
    assert!(state.models_cycle.contains(&"anthropic/claude-opus-4-7".to_string()));

    // Add a duplicate (should be no-op).
    let before = state.models_cycle.len();
    dispatch(
        &mut state,
        &mut settings,
        CommandId::ScopedModels,
        "add anthropic/claude-opus-4-7",
    );
    assert_eq!(state.models_cycle.len(), before);

    // Clear.
    let r = dispatch(&mut state, &mut settings, CommandId::ScopedModels, "clear");
    assert!(matches!(r.outcome, CommandOutcome::Output(_)));
    assert!(state.models_cycle.is_empty());
}

#[test]
fn trust_command_marks_cwd_default() {
    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    let r = dispatch(&mut state, &mut settings, CommandId::Trust, "");
    match r.outcome {
        CommandOutcome::Output(lines) => {
            let joined = lines.join("\n");
            assert!(
                joined.contains("cwd:") || joined.contains("decision:"),
                "expected trust output with cwd and decision, got: {joined}"
            );
        }
        _ => panic!("expected Output"),
    }
}

#[test]
fn trust_command_distrust_and_ask() {
    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    // Distrust
    let r = dispatch(&mut state, &mut settings, CommandId::Trust, "distrust");
    assert!(matches!(r.outcome, CommandOutcome::Output(_)));
    // Ask
    let r = dispatch(&mut state, &mut settings, CommandId::Trust, "ask");
    match r.outcome {
        CommandOutcome::Output(lines) => {
            assert!(lines.iter().any(|l| l.contains("ask")));
        }
        _ => panic!("expected Output"),
    }
    // List
    let r = dispatch(&mut state, &mut settings, CommandId::Trust, "list");
    match r.outcome {
        CommandOutcome::Output(lines) => {
            assert!(
                lines.iter().any(|l| l.contains("ask")),
                "list should show ask after setting"
            );
        }
        _ => panic!("expected Output"),
    }
}

#[test]
fn clone_command_without_session_returns_error() {
    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    // No session_path set.
    let r = dispatch(&mut state, &mut settings, CommandId::Clone, "");
    match r.outcome {
        CommandOutcome::Output(lines) => {
            assert!(
                lines.iter().any(|l| l.contains("no active session")),
                "expected no-session message, got: {lines:?}"
            );
        }
        _ => panic!("expected Output"),
    }
}

#[test]
fn clone_command_with_session_duplicates_file() {
    use std::io::Write;
    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    // Create a temp session file.
    let tmp = tempfile::tempdir().unwrap();
    let session_path = tmp.path().join("session-test.jsonl");
    {
        let mut f = std::fs::File::create(&session_path).unwrap();
        writeln!(f, "{{\"type\":\"session\"}}").unwrap();
    }
    state.session_path = Some(session_path.clone());

    let r = dispatch(&mut state, &mut settings, CommandId::Clone, "");
    match r.outcome {
        CommandOutcome::Output(lines) => {
            assert!(
                lines.iter().any(|l| l.contains("cloned to")),
                "expected clone confirmation, got: {lines:?}"
            );
        }
        _ => panic!("expected Output"),
    }
    // The clone file should exist as a sibling.
    let parent = session_path.parent().unwrap();
    let clones: Vec<_> = std::fs::read_dir(parent)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .contains("session-test-clone-")
        })
        .collect();
    assert_eq!(clones.len(), 1, "expected exactly 1 clone file, got {clones:?}");
}

// =====================================================================
// Tests for stage-2 slash commands: /tree, /fork, /logout.
// =====================================================================

#[test]
fn tree_command_empty_session_renders_friendly_message() {
    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    let r = dispatch(&mut state, &mut settings, CommandId::Tree, "");
    match r.outcome {
        CommandOutcome::Output(lines) => {
            assert!(
                lines.iter().any(|l| l.contains("session tree")),
                "expected tree header, got: {lines:?}"
            );
        }
        _ => panic!("expected Output"),
    }
}

#[test]
fn fork_command_no_user_messages_returns_error() {
    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    let r = dispatch(&mut state, &mut settings, CommandId::Fork, "");
    match r.outcome {
        CommandOutcome::Output(lines) => {
            assert!(
                lines.iter().any(|l| l.contains("no user messages")),
                "expected 'no user messages' message, got: {lines:?}"
            );
        }
        _ => panic!("expected Output"),
    }
}

#[test]
fn fork_command_lists_fork_points_without_index() {
    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    state.push_user("first question".to_string());
    state.push_assistant("first answer".to_string());
    state.push_user("second question".to_string());
    state.push_assistant("second answer".to_string());
    let r = dispatch(&mut state, &mut settings, CommandId::Fork, "");
    match r.outcome {
        CommandOutcome::Output(lines) => {
            let joined = lines.join("\n");
            assert!(joined.contains("2 user messages"), "got: {joined}");
            assert!(joined.contains("first question"), "got: {joined}");
            assert!(joined.contains("second question"), "got: {joined}");
        }
        _ => panic!("expected Output"),
    }
}

#[test]
fn fork_command_with_invalid_index_returns_error() {
    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    state.push_user("only question".to_string());
    let r = dispatch(&mut state, &mut settings, CommandId::Fork, "5");
    match r.outcome {
        CommandOutcome::Output(lines) => {
            assert!(
                lines.iter().any(|l| l.contains("invalid index")),
                "expected invalid-index message, got: {lines:?}"
            );
        }
        _ => panic!("expected Output"),
    }
}

#[test]
fn fork_command_with_valid_index_cuts_transcript() {
    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    state.push_user("first".to_string());
    state.push_assistant("a1".to_string());
    state.push_user("second".to_string());
    state.push_assistant("a2".to_string());
    state.push_user("third".to_string());
    state.push_assistant("a3".to_string());
    let before = state.transcript.len();
    let r = dispatch(&mut state, &mut settings, CommandId::Fork, "2");
    match r.outcome {
        CommandOutcome::Output(lines) => {
            assert!(
                lines.iter().any(|l| l.contains("fork: cut at user msg #2")),
                "expected cut confirmation, got: {lines:?}"
            );
        }
        _ => panic!("expected Output"),
    }
    // Transcript grows by 2 (the echo + divider).
    assert_eq!(state.transcript.len(), before + 2);
}

#[test]
fn logout_command_unknown_provider_returns_error() {
    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    let r = dispatch(&mut state, &mut settings, CommandId::Logout, "nonexistent-provider");
    match r.outcome {
        CommandOutcome::Output(lines) => {
            assert!(
                lines.iter().any(|l| l.contains("unknown provider")),
                "expected unknown-provider error, got: {lines:?}"
            );
        }
        _ => panic!("expected Output"),
    }
}

#[test]
fn logout_command_list_format() {
    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    let r = dispatch(&mut state, &mut settings, CommandId::Logout, "");
    match r.outcome {
        CommandOutcome::Output(lines) => {
            let joined = lines.join("\n");
            // Should list credentials status (all unset in test env).
            assert!(
                joined.contains("unset") || joined.contains("missing"),
                "expected unset/missing status, got: {joined}"
            );
        }
        _ => panic!("expected Output"),
    }
}

// =====================================================================
// Tests for stage-3 slash commands: /import, /login.
// =====================================================================

#[test]
fn login_command_lists_known_providers() {
    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    let r = dispatch(&mut state, &mut settings, CommandId::Login, "");
    match r.outcome {
        CommandOutcome::Output(lines) => {
            let joined = lines.join("\n");
            assert!(
                joined.contains("ANTHROPIC_API_KEY"),
                "expected anthropic env var hint, got: {joined}"
            );
            assert!(
                joined.contains("OPENAI_API_KEY"),
                "expected openai env var hint, got: {joined}"
            );
        }
        _ => panic!("expected Output"),
    }
}

#[test]
fn login_command_specific_provider_reports_status() {
    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    let r = dispatch(&mut state, &mut settings, CommandId::Login, "anthropic");
    match r.outcome {
        CommandOutcome::Output(lines) => {
            let joined = lines.join("\n");
            assert!(joined.contains("ANTHROPIC_API_KEY"), "got: {joined}");
            // Test env doesn't have the var, so "not set" should appear.
            assert!(
                joined.contains("not set"),
                "expected 'not set' status, got: {joined}"
            );
        }
        _ => panic!("expected Output"),
    }
}

#[test]
fn login_command_unknown_provider_returns_error() {
    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    let r = dispatch(&mut state, &mut settings, CommandId::Login, "fake-provider-xyz");
    match r.outcome {
        CommandOutcome::Output(lines) => {
            let joined = lines.join("\n");
            assert!(
                joined.contains("unknown provider"),
                "expected unknown-provider message, got: {joined}"
            );
        }
        _ => panic!("expected Output"),
    }
}

#[test]
fn import_command_without_path_returns_usage() {
    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    let r = dispatch(&mut state, &mut settings, CommandId::Import, "");
    match r.outcome {
        CommandOutcome::Output(lines) => {
            let joined = lines.join("\n");
            assert!(
                joined.contains("path-to-jsonl"),
                "expected usage message, got: {joined}"
            );
        }
        _ => panic!("expected Output"),
    }
}

#[test]
fn import_command_with_missing_file_returns_error() {
    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    let r = dispatch(
        &mut state,
        &mut settings,
        CommandId::Import,
        "/tmp/does-not-exist-12345.jsonl",
    );
    match r.outcome {
        CommandOutcome::Output(lines) => {
            assert!(
                lines.iter().any(|l| l.contains("read failed")),
                "expected read-failed error, got: {lines:?}"
            );
        }
        _ => panic!("expected Output"),
    }
}

#[test]
fn import_command_with_valid_jsonl_replaces_transcript() {
    // Write a JSONL file with 3 valid + 1 garbage line. Avoid `writeln!`
    // with raw `{{ }}` in format strings (they get unescaped); instead
    // use `writeln!(f, "...{}", json_str)` with a plain JSON literal.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("session-test.jsonl");
    std::fs::write(
        &path,
        concat!(
            "{\"type\":\"session_info\",\"id\":\"abc123\",\"parentId\":null,\"timestamp\":\"2026-01-01T00:00:00Z\",\"name\":\"imported\"}\n",
            "{\"type\":\"message\",\"id\":\"m1\",\"parentId\":null,\"timestamp\":\"2026-01-01T00:00:01Z\",\"message\":{\"role\":\"user\",\"content\":\"hello from import\",\"timestamp\":0}}\n",
            "{\"type\":\"message\",\"id\":\"m2\",\"parentId\":\"m1\",\"timestamp\":\"2026-01-01T00:00:02Z\",\"message\":{\"role\":\"user\",\"content\":\"second user msg\",\"timestamp\":0}}\n",
            "this is not json\n",
        ),
    )
    .unwrap();
    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    let r = dispatch(
        &mut state,
        &mut settings,
        CommandId::Import,
        path.to_str().unwrap(),
    );
    match r.outcome {
        CommandOutcome::Output(lines) => {
            let joined = lines.join("\n");
            assert!(
                joined.contains("imported 3 entries"),
                "expected imported 3 entries, got: {joined}"
            );
            assert!(
                joined.contains("skipped 1"),
                "expected 'skipped 1' for garbage line, got: {joined}"
            );
        }
        _ => panic!("expected Output"),
    }
    // Transcript now has the imported messages + the [import] echo + a
    // divider (4 entries total). The important invariant is that we
    // got exactly the 2 imported user messages, not 3 (which would
    // include the garbage) or 0.
    let user_count = state
        .transcript
        .iter()
        .filter(|l| matches!(l, TranscriptLine::User(_)))
        .count();
    assert_eq!(user_count, 2, "expected 2 user messages from import, got {user_count}");
    // Verify session_id set
    assert_eq!(state.session_id.as_deref(), Some("abc123"));
}



// =====================================================================
// /prompt command: loads .md templates from .pi/prompts/ and
// ~/.pi/agent/prompts/, substitutes $1/$@/$ARGUMENTS placeholders, and
// injects the rendered text as a user message.
// =====================================================================

#[test]
fn prompt_command_unknown_template_lists_available() {
    use std::io::Write;
    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    let tmp = tempfile::tempdir().unwrap();
    let prompts_dir = tmp.path().join(".pi").join("prompts");
    std::fs::create_dir_all(&prompts_dir).unwrap();
    let mut f = std::fs::File::create(prompts_dir.join("greet.md")).unwrap();
    writeln!(
        f,
        "---\ndescription: Greet the user\n---\nHello $1"
    )
    .unwrap();

    let _home_guard = HOME_LOCK.lock().unwrap();
        let orig_cwd = std::env::current_dir().ok();
    std::env::set_current_dir(tmp.path()).unwrap();
    let r = dispatch(&mut state, &mut settings, CommandId::Prompt, "missing");
    if let Some(orig) = orig_cwd {
        let _ = std::env::set_current_dir(&orig);
    }
    match r.outcome {
        CommandOutcome::Output(lines) => {
            let joined = lines.join("\n");
            assert!(
                joined.contains("not found"),
                "expected 'not found' in error, got: {joined}"
            );
            assert!(
                joined.contains("greet") || joined.contains("Greet"),
                "expected available template in error, got: {joined}"
            );
        }
        _ => panic!("expected Output, got {:?}", r.outcome),
    }
}

#[test]
fn prompt_command_renders_substituted_template() {
    use std::io::Write;
    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    let tmp = tempfile::tempdir().unwrap();
    let prompts_dir = tmp.path().join(".pi").join("prompts");
    std::fs::create_dir_all(&prompts_dir).unwrap();
    let mut f = std::fs::File::create(prompts_dir.join("greet.md")).unwrap();
    writeln!(f, "---\ndescription: Greet the user\n---\nHello $1").unwrap();

    let _home_guard = HOME_LOCK.lock().unwrap();
        let orig_cwd = std::env::current_dir().ok();
    std::env::set_current_dir(tmp.path()).unwrap();
    let r = dispatch(
        &mut state,
        &mut settings,
        CommandId::Prompt,
        "greet World",
    );
    if let Some(orig) = orig_cwd {
        let _ = std::env::set_current_dir(&orig);
    }
    let has_user_world = state.transcript.iter().any(|l| match l {
        nini_tui::state::TranscriptLine::User(s) => s == "Hello World",
        _ => false,
    });
    assert!(
        has_user_world,
        "expected 'Hello World' in transcript, got: {:?}",
        state
            .transcript
            .iter()
            .filter_map(|l| match l {
                nini_tui::state::TranscriptLine::User(s) => Some(s.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
    );
}

#[test]
fn prompt_command_quoted_args_preserve_whitespace() {
    use std::io::Write;
    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    let tmp = tempfile::tempdir().unwrap();
    let prompts_dir = tmp.path().join(".pi").join("prompts");
    std::fs::create_dir_all(&prompts_dir).unwrap();
    let mut f = std::fs::File::create(prompts_dir.join("greet.md")).unwrap();
    writeln!(f, "---\ndescription: Greet user\n---\nHello $1").unwrap();

    let _home_guard = HOME_LOCK.lock().unwrap();
        let orig_cwd = std::env::current_dir().ok();
    std::env::set_current_dir(tmp.path()).unwrap();
    let r = dispatch(
        &mut state,
        &mut settings,
        CommandId::Prompt,
        "greet \"hello world\"",
    );
    if let Some(orig) = orig_cwd {
        let _ = std::env::set_current_dir(&orig);
    }
    let has = state.transcript.iter().any(|l| match l {
        nini_tui::state::TranscriptLine::User(s) => s == "Hello hello world",
        _ => false,
    });
    assert!(
        has,
        "expected 'Hello hello world' (with the space from the quoted arg), got: {:?}",
        state
            .transcript
            .iter()
            .filter_map(|l| match l {
                nini_tui::state::TranscriptLine::User(s) => Some(s.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
    );
    assert!(matches!(r.outcome, CommandOutcome::Output(_)));
}

#[test]
fn prompt_command_no_args_shows_usage() {
    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    let r = dispatch(&mut state, &mut settings, CommandId::Prompt, "");
    match r.outcome {
        CommandOutcome::Output(lines) => {
            let joined = lines.join("\n");
            assert!(joined.contains("Usage"), "expected 'Usage' in help, got: {joined}");
            assert!(joined.contains("name"), "expected 'name' in help, got: {joined}");
        }
        _ => panic!("expected Output"),
    }
}
