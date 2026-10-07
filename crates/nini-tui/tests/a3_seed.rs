//! A3: the session is the source of truth at the Agent boundary.
//!
//! The Agent used to start every run with an empty history. The driver
//! closure in `nini-cli/src/app.rs` builds a fresh `Agent` per turn and
//! never seeded it, so the model saw the current turn and nothing else
//! while the session held the whole conversation.
//!
//! That made the compaction work inert. A compaction recorded in the
//! session (A1, A2) had no effect on the next request, because the next
//! request was built from an empty history; and the Agent's own in-loop
//! compaction changed only its own memory and emitted a `Compacted` event
//! that nothing in the tree consumed. Two stores, one of which the model
//! never saw, and the authoritative one invisible to it.
//!
//! A3 closes that by handing the driver the session's live context -
//! Pi's `agent.state.messages = buildSessionContext(...)`, placed at the
//! boundary where the session is reachable. The in-loop threshold
//! compaction is removed; the overflow trigger stays as the safety net.

use nini_tui::runtime::{AgentEventLite, AgentSink};
use nini_tui::state::AppState;
use std::sync::Arc;

fn new_state(
    tag: &str,
) -> (
    Arc<std::sync::Mutex<AppState>>,
    AgentSink,
    std::path::PathBuf,
) {
    let dir = std::env::temp_dir().join(format!("nini-a3-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let state = Arc::new(std::sync::Mutex::new(AppState::new("test-model")));
    state.lock().unwrap().session_init(dir.join("s.jsonl"));
    let (tx, _rx) = tokio::sync::watch::channel(false);
    let sink = AgentSink::new(Arc::clone(&state), tx);
    (state, sink, dir)
}

/// The seed handed to the driver must be the session's context, so history
/// from previous turns reaches the model.
#[tokio::test]
async fn the_seed_carries_previous_turns() {
    let (state, _sink, dir) = new_state("history");
    state.lock().unwrap().push_user("first question");
    state.lock().unwrap().push_assistant("first answer");
    state.lock().unwrap().push_user("second question");

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
        texts.iter().any(|t| t.contains("first question")),
        "the seed must carry earlier turns, or the model sees only the \
         current one: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("second question")),
        "including the most recent turn: {texts:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The seed must reflect a compaction. This is the whole point: a
/// compaction that the next request cannot see is decoration.
#[tokio::test]
async fn the_seed_reflects_a_compaction() {
    let (state, _sink, dir) = new_state("compacted");
    state.lock().unwrap().push_user("an early question");
    state.lock().unwrap().push_assistant("an early answer");
    state.lock().unwrap().push_user("a later question");

    let compacted = {
        let mut g = state.lock().unwrap();
        g.run_state.context_window = 200_000;
        // Force the cut: the real one is a token budget.
        let arc = g.session_state.session.as_ref().unwrap().clone();
        let mut s = arc.try_lock().unwrap();
        let legacy: Vec<nini_core::Entry> =
            s.get_path().into_iter().cloned().map(Into::into).collect();
        let cut = nini_core::compaction::find_cut_point(&legacy, 1);
        assert!(cut.keep_from > 0, "precondition: the cut must land inside");
        s.append_compaction(
            "SUMMARY OF THE EARLY WORK".into(),
            legacy[cut.keep_from].id.clone(),
            40_000,
        );
        true
    };
    assert!(compacted);

    state
        .lock()
        .unwrap()
        .push_user("a question after the compaction");
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
        texts
            .iter()
            .any(|t| t.contains("CONTEXT SUMMARY") && t.contains("SUMMARY OF THE EARLY WORK")),
        "the seed must carry the summary, or a compaction changes nothing \
         the model sees: {texts:?}"
    );
    assert!(
        !texts.iter().any(|t| t.contains("an early question")),
        "and must not carry what the compaction summarised away: {texts:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// An empty session yields an empty seed, which is correct: there is no
/// history to replay. `--no-session` runs must not be handed phantom turns.
#[tokio::test]
async fn no_session_yields_an_empty_seed() {
    let state = Arc::new(std::sync::Mutex::new(AppState::new("test-model")));
    assert!(
        state.lock().unwrap().session_context_messages().is_empty(),
        "an AppState with no session must seed nothing"
    );
}

/// Tool traffic reaches the seed. P0.1 put it in the session; A3 is what
/// finally gets it in front of the model.
#[tokio::test]
async fn the_seed_carries_tool_calls_and_results() {
    let (state, sink, dir) = new_state("tools");
    state.lock().unwrap().push_user("read the config");
    for e in [
        AgentEventLite::TextDelta("reading it".into()),
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
            content: "file body".into(),
            details: None,
            duration_ms: 1,
        },
        AgentEventLite::TurnEnd,
    ] {
        sink.push(e);
    }
    state.lock().unwrap().push_user("what did you find?");

    let seed = state.lock().unwrap().session_context_messages();
    let json = serde_json::to_string(&seed).unwrap();
    assert!(
        json.contains("a.txt") && json.contains("file body"),
        "tool arguments and results must reach the model: {json}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The wiring itself, not just the accessor.
///
/// `a3_seed` above tests `AppState::session_context_messages` in isolation.
/// An earlier version of this file had nothing else, and so two injections
/// passed it clean: one that made the runtime hand the driver an empty
/// vector, and one that made the driver ignore the seed entirely. Both are
/// in different files from the accessor, and a test that never touches them
/// cannot see them.
///
/// So this drives `submit_user_input`, which is where the seed is computed
/// and passed, with a driver that records what it was given.
#[tokio::test]
async fn submit_hands_the_driver_the_session_context() {
    use nini_tui::runtime::{submit_user_input, AgentDriver, SharedState};
    use std::sync::mpsc;

    let dir = std::env::temp_dir().join(format!("nini-a3-wire-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let shared: SharedState = {
        let mut g = AppState::new("test-model");
        g.session_init(dir.join("s.jsonl"));
        g.push_user("an earlier question");
        g.push_assistant("an earlier answer");
        g.push_user("the latest question");
        Arc::new(std::sync::Mutex::new(g))
    };

    let (tx, _rx) = tokio::sync::watch::channel(false);
    let (send, recv) = mpsc::channel::<Vec<nini_core::provider::Message>>();
    let s2 = send.clone();
    let driver: AgentDriver = Arc::new(move |_msg, seed, _sink, done| {
        let _ = s2.send(seed);
        done.notify_waiters();
        tokio::spawn(async {})
    });
    drop(send);

    {
        let mut g = shared.lock().unwrap();
        for c in "a brand new question".chars() {
            g.input.insert_char(c);
        }
    }

    submit_user_input(&shared, &driver, Arc::new(tokio::sync::Notify::new()), tx);

    let seed = recv
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("the driver must be invoked");

    let texts: Vec<String> = seed
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            nini_core::provider::ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect();

    assert!(
        texts.iter().any(|t| t.contains("an earlier question")),
        "the driver must receive the session's history, not an empty \
         vector: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("the latest question")),
        "including the most recent turn: {texts:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
