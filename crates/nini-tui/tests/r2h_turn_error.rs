//! R2h: a turn that failed must be in the session, not just on screen.
//!
//! `AgentEvent::TurnEnd` has no error payload, and `run()` returns `Err`
//! outright on a non-retryable provider error. The `[error]` line the user
//! saw came from `push_assistant_raw`, which is transcript-only. So a failed
//! turn left **no trace in the session at all** - reopening it showed a
//! conversation quietly missing its last turn.
//!
//! Pi's schema has a place for exactly this: `stopReason: "error"` with
//! `errorMessage` set (`pi-protocol/src/schemas.ts:148-154`), rendered at
//! `assistant-message.ts:190-193` as `Error: <message>` with `"Unknown
//! error"` as the fallback. R1f added both fields to nini so real Pi
//! sessions would parse; this is the producer that was missing.

use nini_tui::runtime::{AgentEventLite, AgentSink};
use nini_tui::state::{AppState, TranscriptLine};
use std::sync::Arc;

fn sink_for(
    tag: &str,
) -> (
    Arc<std::sync::Mutex<AppState>>,
    AgentSink,
    std::path::PathBuf,
) {
    let dir = std::env::temp_dir().join(format!("nini-r2h-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let state = Arc::new(std::sync::Mutex::new(AppState::new("test-model")));
    state.lock().unwrap().session_init(dir.join("s.jsonl"));
    let (tx, _rx) = tokio::sync::watch::channel(false);
    let sink = AgentSink::new(Arc::clone(&state), tx);
    (state, sink, dir)
}

/// The headline: the error text reaches the file.
#[tokio::test]
async fn a_failed_turn_is_recorded_with_its_message() {
    let (state, sink, dir) = sink_for("record");
    state.lock().unwrap().push_user("do the thing");
    sink.push(AgentEventLite::TurnFailed("503 service unavailable".into()));

    let json: Vec<String> = {
        let arc = state
            .lock()
            .unwrap()
            .session_state
            .session
            .as_ref()
            .unwrap()
            .clone();
        let g = nini_tui::state::lock_session(&arc);
        g.entries
            .iter()
            .map(|e| serde_json::to_string(e).unwrap())
            .collect()
    };
    let joined = json.join("\n");

    assert!(
        joined.contains("\"errorMessage\":\"503 service unavailable\""),
        "Pi's wire field must carry the text: {joined}"
    );
    assert!(
        joined.contains("\"stopReason\":\"error\""),
        "and the stop reason must say the turn failed: {joined}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A failed turn must survive a reload. Before this it was transcript-only,
/// so the resumed conversation simply had a missing turn.
#[tokio::test]
async fn a_failed_turn_survives_a_reload() {
    let (state, sink, dir) = sink_for("reload");
    let path = dir.join("s.jsonl");
    {
        let mut g = state.lock().unwrap();
        g.session_init(path.clone());
        g.push_user("do the thing");
    }
    sink.push(AgentEventLite::TurnFailed(
        "connection reset by peer".into(),
    ));
    state.lock().unwrap().session_flush();
    std::thread::sleep(std::time::Duration::from_millis(120));

    let mut fresh = AppState::new("test-model");
    fresh.session_load(path.clone()).expect("must load");

    let shown = fresh.transcript_state.lines.iter().find_map(|l| match l {
        TranscriptLine::StopNotice(s) => Some(s.clone()),
        _ => None,
    });
    assert!(
        shown
            .as_deref()
            .map(|s| s.contains("connection reset by peer"))
            .unwrap_or(false),
        "a resumed session must show that the turn failed and why. \
         Got {shown:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The failure must not swallow what came before it.
#[tokio::test]
async fn a_failure_does_not_hide_the_earlier_turn() {
    let (state, sink, dir) = sink_for("order");
    let path = dir.join("s.jsonl");
    state.lock().unwrap().push_user("first question");
    sink.push(AgentEventLite::TurnFailed("boom".into()));
    state.lock().unwrap().session_flush();
    std::thread::sleep(std::time::Duration::from_millis(120));

    let mut fresh = AppState::new("test-model");
    fresh.session_load(path).unwrap();
    let has_user = fresh
        .transcript_state
        .lines
        .iter()
        .any(|l| matches!(l, TranscriptLine::User(u) if u.contains("first question")));
    assert!(has_user, "the earlier turn must still be there");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `Error` and `TurnFailed` are different things. The Agent also emits
/// `Error` for transient retry chatter, which is not a failed turn and must
/// not end up in the session as one.
#[tokio::test]
async fn a_transient_notice_is_not_recorded_as_a_failed_turn() {
    let (state, sink, dir) = sink_for("transient");
    state.lock().unwrap().push_user("do the thing");
    sink.push(AgentEventLite::Error(
        "retrying after transient error (attempt 1, delay 0ms)".into(),
    ));

    let arc = state
        .lock()
        .unwrap()
        .session_state
        .session
        .as_ref()
        .unwrap()
        .clone();
    let g = nini_tui::state::lock_session(&arc);
    let recorded = g.entries.iter().any(|e| {
        serde_json::to_string(e)
            .map(|s| s.contains("retrying after transient"))
            .unwrap_or(false)
    });
    assert!(
        !recorded,
        "a retry notice is not a failed turn and must not be recorded as one"
    );
    drop(g);

    let _ = std::fs::remove_dir_all(&dir);
}

/// A failed turn must not be replayed to the model as a conversation turn.
/// It has no content, and the error is a fact about the run, not something
/// the model said.
#[tokio::test]
async fn a_failure_is_not_replayed_as_model_context() {
    let (state, _sink, dir) = sink_for("context");
    {
        let mut g = state.lock().unwrap();
        g.session_init(dir.join("s.jsonl"));
        g.push_user("do the thing");
        g.session_append_turn_error("503");
    }

    let seed = state.lock().unwrap().session_context_messages();
    let texts: Vec<String> = seed
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            nini_core::provider::ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(
        !texts.iter().any(|t| t.contains("503")),
        "the error must not be replayed to the model: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("do the thing")),
        "while the real turn still is: {texts:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
