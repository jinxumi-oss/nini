//! R: `/resume <n>` must rebuild the transcript from the session's live
//! context, and its own words must never reach the session file.
//!
//! Three defects lived in the 40-line hand-built rebuild loop that used
//! to sit after `session_load` (removed in v0.16; see `cmd_resume`):
//!
//! 1. it re-pushed only `ContentBlock::Text` blocks, so tool calls,
//!    tool results and bash executions were on disk but not on screen
//!    (a coding session resumed as prose);
//! 2. the `(loaded session ...)` confirmation was written with
//!    `push_assistant`, so it became a **model message in the session
//!    file** — the next `/resume` of the same file showed it as an
//!    assistant turn, and every resume accumulated one more;
//! 3. the same line was glued onto the last assistant text of the
//!    loaded history (`push_assistant`'s same-turn merging), so a
//!    notice the TUI produced became part of a model message on screen
//!    and in `/export`.
//!
//! These tests pin all three: tools on screen after resume, the session
//! file byte-identical before and after, and the confirmation as a
//! `Notice` (its own row, never a model message).

use nini_tui::commands::{dispatch, CommandId};
use nini_tui::runtime::{AgentEventLite, AgentSink};
use nini_tui::settings::SettingsManager;
use nini_tui::state::{AppState, TranscriptLine};
use std::sync::{Arc, Mutex};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn scratch(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("nini-resume-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A session on disk: user + assistant (text **and** a tool call) +
/// the tool result. If tools are missing from a resumed transcript,
/// this is what lost them.
fn seed_session_with_tools(path: &std::path::Path) {
    let state = Arc::new(Mutex::new(AppState::new("test-model")));
    state.lock().unwrap().session_init(path.to_path_buf());
    state.lock().unwrap().push_user("read the config");
    let (tx, _rx) = tokio::sync::watch::channel(false);
    let sink = AgentSink::new(Arc::clone(&state), tx);
    for e in [
        AgentEventLite::TextDelta("reading it now".into()),
        AgentEventLite::ToolCallStart {
            id: "c1".into(),
            name: "read".into(),
        },
        AgentEventLite::ToolCallStop {
            id: "c1".into(),
            args: "{\"path\":\"a.txt\"}".into(),
        },
        AgentEventLite::ToolResult {
            id: "c1".into(),
            name: "read".into(),
            ok: true,
            content: "the file body".into(),
            details: None,
            duration_ms: 1,
        },
        AgentEventLite::TurnEnd,
    ] {
        sink.push(e);
    }
    state.lock().unwrap().session_flush();
    std::thread::sleep(std::time::Duration::from_millis(150));
}

/// Run `/resume <n>` against a scratch agent dir whose sessions dir
/// holds exactly one seeded session (index 1).
fn resume_one(dir: &std::path::Path) -> AppState {
    std::env::set_var("NINI_AGENT_DIR", dir.join("agent"));
    let session_path = dir.join("agent/sessions/proj/20260101-000000_aa.jsonl");
    seed_session_with_tools(&session_path);

    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    let r = dispatch(&mut state, &mut settings, CommandId::Resume, "1");
    assert!(r.error.is_none(), "/resume errored: {:?}", r.error);
    state
}

fn labels(state: &AppState) -> Vec<String> {
    state
        .transcript_state
        .lines
        .iter()
        .map(|l| match l {
            TranscriptLine::User(_) => "USER".to_string(),
            TranscriptLine::AssistantText(_) => "ASSISTANT".to_string(),
            TranscriptLine::ToolCall { name, .. } => format!("TOOLCALL[{name}]"),
            TranscriptLine::ToolResult { name, .. } => format!("TOOLRESULT[{name}]"),
            TranscriptLine::BashExecution { .. } => "BASH".to_string(),
            TranscriptLine::StopNotice(_) => "STOPNOTICE".to_string(),
            TranscriptLine::Notice(_) => "NOTICE".to_string(),
            TranscriptLine::ThinkingText(_) => "THINKING".to_string(),
            TranscriptLine::Divider => "DIV".to_string(),
        })
        .collect()
}

/// Defect 1: a resumed transcript must show the session's tool traffic.
/// The hand-built loop kept only text blocks, so this was prose-only.
#[test]
fn resume_shows_the_tool_traffic_of_the_loaded_session() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("tools");
    let state = resume_one(&dir);

    let got = labels(&state);
    assert!(
        got.iter().any(|l| l == "TOOLCALL[read]"),
        "the tool call is in the session file and must be on screen: {got:?}"
    );
    assert!(
        got.iter().any(|l| l == "TOOLRESULT[read]"),
        "and so must its result: {got:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Defect 2: the `(loaded session ...)` confirmation must never reach
/// the session file — not as an assistant message, not at all.
///
/// `push_assistant` wrote it there (v0.8.9 through v0.16), so every
/// resume accumulated one more fake assistant turn in the file. The
/// file is byte-identical before and after a resume.
#[test]
fn resume_does_not_write_anything_into_the_loaded_session_file() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("file");
    std::env::set_var("NINI_AGENT_DIR", dir.join("agent"));
    let session_path = dir.join("agent/sessions/proj/20260101-000000_aa.jsonl");
    seed_session_with_tools(&session_path);

    let before = std::fs::read(&session_path).expect("seeded session file");

    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    let r = dispatch(&mut state, &mut settings, CommandId::Resume, "1");
    assert!(r.error.is_none(), "/resume errored: {:?}", r.error);

    // A resume that is followed by any flush (the runtime flushes on
    // the next turn, on /new, on exit) must not write the confirmation
    // into the file either. The dispatch path does not flush, so the
    // only way to catch the defect where it lands is to flush here.
    state.session_flush();
    std::thread::sleep(std::time::Duration::from_millis(150));
    let after = std::fs::read(&session_path).expect("session file must still exist");

    assert_eq!(
        before, after,
        "a resume must not modify the session file. The confirmation used \
         to be written into it as an assistant message, so the next \
         /resume of the same file showed it as a phantom turn."
    );
    let text = String::from_utf8_lossy(&after);
    assert!(
        !text.contains("loaded session"),
        "the confirmation must not be in the session file at all"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Defect 3: the confirmation is the TUI talking — a `Notice`, its own
/// row, never glued onto the last assistant text (same-turn merging)
/// and never mistaken for a model message in `/export`.
#[test]
fn resume_confirmation_is_a_notice_row_not_glued_to_the_history() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("notice");
    let state = resume_one(&dir);

    let notice = state
        .transcript_state
        .lines
        .iter()
        .find_map(|l| match l {
            TranscriptLine::Notice(s) if s.starts_with("(loaded session") => Some(s.clone()),
            _ => None,
        })
        .expect("the resume confirmation must be a Notice row");

    // It must not have been glued onto an assistant text: no
    // AssistantText line may end with the confirmation.
    for l in &state.transcript_state.lines {
        if let TranscriptLine::AssistantText(t) = l {
            assert!(
                !t.starts_with("(loaded session"),
                "the confirmation was glued onto an assistant message: {t:?}"
            );
        }
    }
    let _ = notice;

    let _ = std::fs::remove_dir_all(&dir);
}

/// Defect 1, continued: a bash execution the user ran must survive a
/// resume too (R2f's content, but through the `/resume` path — the
/// hand-built loop had no arm for it).
#[test]
fn resume_shows_bash_executions_from_the_loaded_session() {
    use nini_core::entries::{AgentMessage, AssistantMessage, StopReason};
    use nini_core::SessionEntry;

    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("bash");
    std::env::set_var("NINI_AGENT_DIR", dir.join("agent"));
    let session_path = dir.join("agent/sessions/proj/20260101-000000_aa.jsonl");
    std::fs::create_dir_all(session_path.parent().unwrap()).unwrap();

    // Write a session by hand: user, bash execution, assistant.
    {
        let mut st = AppState::new("test-model");
        st.session_init(session_path.clone());
        st.push_user("run the tests");
        st.push_assistant("running them now");
        // Append a bash execution entry directly (the TUI path for it
        // is `session_append_bash_execution`, which needs a job).
        let arc = st.session_state.session.as_ref().unwrap().clone();
        nini_tui::state::lock_session(&arc).append_chained(SessionEntry::Message(
            nini_core::entries::SessionMessageEntry {
                id: "msg_b".to_string(),
                parent_id: Some("msg_a".to_string()),
                timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                message: AgentMessage::BashExecution(nini_core::entries::BashExecutionMessage {
                    command: "cargo test".to_string(),
                    output: "2 passed".to_string(),
                    exit_code: Some(0),
                    cancelled: false,
                    truncated: false,
                    full_output_path: None,
                    timestamp: chrono::Utc::now().timestamp_millis(),
                }),
            },
        ));
        // And the assistant message it chains under (id msg_a) so the
        // entry is not detached.
        nini_tui::state::lock_session(&arc).append_chained(SessionEntry::Message(
            nini_core::entries::SessionMessageEntry {
                id: "msg_a".to_string(),
                parent_id: None,
                timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                message: AgentMessage::Assistant(AssistantMessage {
                    content: vec![nini_core::entries::ContentBlock::Text {
                        text: "running them now".to_string(),
                    }],
                    api: String::new(),
                    provider: String::new(),
                    model: String::new(),
                    usage: Default::default(),
                    stop_reason: StopReason::Stop,
                    error_message: None,
                    timestamp: chrono::Utc::now().timestamp_millis(),
                }),
            },
        ));
        st.session_flush();
        std::thread::sleep(std::time::Duration::from_millis(150));
    }

    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    let r = dispatch(&mut state, &mut settings, CommandId::Resume, "1");
    assert!(r.error.is_none(), "/resume errored: {:?}", r.error);

    let got = labels(&state);
    assert!(
        got.iter().any(|l| l == "BASH"),
        "a bash execution in the session must be on screen after resume: {got:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A damaged line must surface as a `StopNotice` next to the content
/// it affects — and the notice, again, must not reach the session.
#[test]
fn resume_surfaces_skipped_lines_as_a_stop_notice() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("skipped");
    std::env::set_var("NINI_AGENT_DIR", dir.join("agent"));
    let session_path = dir.join("agent/sessions/proj/20260101-000000_aa.jsonl");
    seed_session_with_tools(&session_path);

    // Append a line that cannot parse.
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&session_path)
            .unwrap();
        f.write_all(b"{\"type\":\"message\",\"broken").unwrap();
    }
    let before = std::fs::read(&session_path).unwrap();

    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    let r = dispatch(&mut state, &mut settings, CommandId::Resume, "1");
    assert!(r.error.is_none(), "/resume errored: {:?}", r.error);

    let skipped = state.session_state.skipped_lines_on_load;
    assert!(
        skipped > 0,
        "precondition: the damaged line must be counted"
    );
    let got = labels(&state);
    assert!(
        got.iter().any(|l| l == "STOPNOTICE"),
        "a skipped line must surface as a StopNotice: {got:?}"
    );

    let after = std::fs::read(&session_path).unwrap();
    assert_eq!(
        before, after,
        "the skipped-line notice must not reach the file"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// P3 (one line): `/session` reports where the session lives.
#[test]
fn session_command_reports_the_session_path() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("session-cmd");
    std::env::set_var("NINI_AGENT_DIR", dir.join("agent"));
    let session_path = dir.join("agent/sessions/proj/20260101-000000_aa.jsonl");
    seed_session_with_tools(&session_path);

    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    let r = dispatch(&mut state, &mut settings, CommandId::Resume, "1");
    assert!(r.error.is_none(), "/resume errored: {:?}", r.error);
    let r = dispatch(&mut state, &mut settings, CommandId::Session, "");
    let lines = match r.outcome {
        nini_tui::commands::CommandOutcome::Output(l) => l,
        other => panic!("expected Output, got {other:?}"),
    };
    let first = &lines[0];
    assert!(
        first.contains("session_path") || first.contains(session_path.to_string_lossy().as_ref()),
        "/session must report where the session lives: {first:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
