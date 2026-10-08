//! A4: `/resume` must show the session it loaded.
//!
//! `session_load` read the file into `AppState` and stopped. Its comment
//! claimed - twice - that "the caller rebuilds the transcript from
//! `session.entries` right after this", and no caller did: the `/resume`
//! command and the session picker both returned at the `?`.
//!
//! Since A3 the next Agent is seeded from the session, so `/resume` swapped
//! the history the *model* would see while the transcript the *user* looks
//! at kept whatever it had. The two disagreed about the session.
//!
//! These tests write a session to disk, load it, and look at the transcript.

use nini_tui::state::{AppState, TranscriptLine};
use std::sync::Arc;

fn scratch(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("nini-a4-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn texts(s: &AppState) -> Vec<String> {
    s.transcript_state
        .lines
        .iter()
        .filter_map(|l| match l {
            TranscriptLine::User(t) | TranscriptLine::AssistantText(t) => Some(t.clone()),
            _ => None,
        })
        .collect()
}

/// The headline: loading a session must put its content on screen.
#[test]
fn loading_a_session_shows_its_messages() {
    let dir = scratch("messages");

    // Write a session with a conversation in it.
    {
        let mut st = AppState::new("test-model");
        st.session_init(dir.join("s.jsonl"));
        st.push_user("what did we decide about auth?");
        st.push_assistant("we chose sqlx, not diesel");
        st.push_user("and the table prefix?");
        st.push_assistant("t_ on every table");
        st.session_flush();
        std::thread::sleep(std::time::Duration::from_millis(120));
    }

    let mut fresh = AppState::new("test-model");
    fresh.session_load(dir.join("s.jsonl")).expect("must load");

    let got = texts(&fresh);
    assert!(
        got.iter()
            .any(|t| t.contains("what did we decide about auth?")),
        "`/resume` must show the loaded history. The transcript used to \
         keep whatever it had, so the user saw the old conversation while \
         the model was given this one: {got:?}"
    );
    assert!(
        got.iter().any(|t| t.contains("t_ on every table")),
        "including the most recent exchange: {got:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Tool traffic has to appear too. It is in the session since P0.1, but a
/// transcript that shows only prose would misrepresent a coding session.
#[test]
fn loading_a_session_shows_its_tool_traffic() {
    use nini_tui::runtime::{AgentEventLite, AgentSink};

    let dir = scratch("tools");
    let path = dir.join("s.jsonl");

    let state = Arc::new(std::sync::Mutex::new(AppState::new("test-model")));
    state.lock().unwrap().session_init(path.clone());
    let (tx, _rx) = tokio::sync::watch::channel(false);
    let sink = AgentSink::new(Arc::clone(&state), tx);

    state.lock().unwrap().push_user("read the config");
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
    drop(state);
    std::thread::sleep(std::time::Duration::from_millis(120));

    let mut fresh = AppState::new("test-model");
    fresh.session_load(path.clone()).expect("must load");

    let has_call = fresh
        .transcript_state
        .lines
        .iter()
        .any(|l| matches!(l, TranscriptLine::ToolCall { name, .. } if name == "read"));
    let has_result = fresh
        .transcript_state
        .lines
        .iter()
        .any(|l| matches!(l, TranscriptLine::ToolResult { content, .. } if content.contains("the file body")));

    assert!(
        has_call,
        "a tool call is in the session and must be in the transcript"
    );
    assert!(has_result, "and so must its result");

    let _ = std::fs::remove_dir_all(&dir);
}

/// The transcript must show the session's *live context*, not its raw
/// entry list. A compaction that summarised away half the history should
/// appear as a summary; the half it replaced should not be on screen.
#[test]
fn loading_shows_the_live_context_not_the_raw_entries() {
    let dir = scratch("compacted");
    let path = dir.join("s.jsonl");

    {
        let mut st = AppState::new("test-model");
        st.session_init(path.clone());
        st.push_user("an early question about caching");
        st.push_assistant("an early answer about eviction policy");
        st.push_user("a later question about the TTL");
        let arc = st.session_state.session.as_ref().unwrap().clone();
        let mut g = arc.try_lock().unwrap();
        let legacy: Vec<nini_core::Entry> =
            g.get_path().into_iter().cloned().map(Into::into).collect();
        let cut = nini_core::compaction::find_cut_point(&legacy, 1);
        assert!(cut.keep_from > 0, "precondition: the cut must land inside");
        g.append_compaction(
            "SUMMARY: caching uses an LRU eviction policy".into(),
            legacy[cut.keep_from].id.clone(),
            40_000,
        );
        drop(g);
        st.session_flush();
        std::thread::sleep(std::time::Duration::from_millis(120));
    }

    let mut fresh = AppState::new("test-model");
    fresh.session_load(path.clone()).expect("must load");

    let got = texts(&fresh);
    assert!(
        got.iter()
            .any(|t| t.contains("SUMMARY: caching uses an LRU")),
        "the compaction must appear as a summary: {got:?}"
    );
    assert!(
        !got.iter()
            .any(|t| t.contains("an early question about caching")),
        "and what it summarised away must not be on screen. Showing the \
         raw entries would tell the user a history the agent does not \
         have: {got:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A fresh AppState has an empty transcript, so a load has to change it.
/// Without this, a test that loaded into an already-populated state could
/// pass by accident.
#[test]
fn a_fresh_state_starts_with_an_empty_transcript() {
    let mut st = AppState::new("test-model");
    st.push_user("this is in the transcript");
    let dir = scratch("precondition");
    st.session_init(dir.join("s.jsonl"));
    st.push_user("a question that will be on disk");
    st.session_flush();
    std::thread::sleep(std::time::Duration::from_millis(120));

    let fresh = AppState::new("test-model");
    assert!(
        fresh.transcript_state.lines.is_empty(),
        "precondition: a fresh AppState shows nothing, so a passing load \
         test cannot be explained by leftover lines"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
