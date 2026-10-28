//! P0.1: the agent's side of a conversation must reach the session file.
//!
//! Driven through the REAL `AgentEventLite` path, not by calling the state
//! helpers directly: calling `push_user` by hand proves the helper works and
//! proves nothing about production.
//!
//! Before this, a complete turn left exactly one entry in the session — the
//! user message. Assistant prose arrived as `TextDelta` ->
//! `push_assistant_raw` (transcript only). `push_tool_call` and
//! `push_tool_result` never touched the session, and `state.rs` carried the
//! comment "Tool results are part of the turn — session_append is handled on
//! TurnEnd" while nothing handled them. `AgentSink` does not even hold a
//! session handle.
//!
//! Pi's shape (pi-coding-agent, session entries):
//!   assistant message  content: [ {type:text}, {type:toolCall,...} ]
//!   toolResult message  { type:"toolResult", toolCallId, toolName, content,
//!                        isError, ... }   — one message per result
//!
//! So the oracle pins the shape, not merely the presence of the word.

use nini_tui::runtime::{AgentEventLite, AgentSink};
use nini_tui::state::AppState;
use std::sync::Arc;

fn drain(sink: &AgentSink, _state: &Arc<std::sync::Mutex<AppState>>, evs: Vec<AgentEventLite>) {
    for e in evs {
        sink.push(e);
    }
}

fn entries(state: &Arc<std::sync::Mutex<AppState>>) -> Vec<serde_json::Value> {
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
        .map(|e| serde_json::from_str(&serde_json::to_string(e).unwrap()).unwrap())
        .collect()
}

fn new_state(
    tag: &str,
) -> (
    Arc<std::sync::Mutex<AppState>>,
    AgentSink,
    std::path::PathBuf,
) {
    let dir = std::env::temp_dir().join(format!("nini-p01-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let state = Arc::new(std::sync::Mutex::new(AppState::new("test-model")));
    state.lock().unwrap().session_init(dir.join("s.jsonl"));
    let (tx, _rx) = tokio::sync::watch::channel(false);
    let sink = AgentSink::new(Arc::clone(&state), tx);
    (state, sink, dir)
}

#[tokio::test]
async fn a_turn_with_a_tool_call_round_trips_in_pis_shape() {
    let (state, sink, dir) = new_state("shape");
    state.lock().unwrap().push_user("read the config");

    drain(
        &sink,
        &state,
        vec![
            AgentEventLite::TextDelta("I will read it.".into()),
            AgentEventLite::ToolCallStart {
                id: "call-1".into(),
                name: "read".into(),
            },
            AgentEventLite::ToolCallStop {
                id: "call-1".into(),
                args: "{\"path\":\"a.txt\"}".into(),
            },
            AgentEventLite::ToolResult {
                id: "call-1".into(),
                name: "read".into(),
                ok: true,
                content: "file body".into(),
                details: None,
                duration_ms: 4,
            },
            AgentEventLite::TurnEnd,
        ],
    );

    let e = entries(&state);
    println!("{}", serde_json::to_string_pretty(&e).unwrap());

    assert_eq!(e.len(), 3, "user + assistant(with call) + result: {e:#?}");

    // 1. the assistant message holds the prose AND the call together
    let asst = &e[1]["message"];
    assert_eq!(asst["role"], "assistant", "{asst}");
    let blocks = asst["content"].as_array().unwrap();
    assert_eq!(blocks[0]["type"], "text", "{blocks:?}");
    assert_eq!(blocks[0]["text"], "I will read it.", "{blocks:?}");
    assert_eq!(blocks[1]["type"], "toolCall", "{blocks:?}");
    assert_eq!(blocks[1]["id"], "call-1", "{blocks:?}");
    assert_eq!(blocks[1]["name"], "read", "{blocks:?}");
    assert_eq!(
        blocks[1]["arguments"]["path"], "a.txt",
        "the arguments must be parsed JSON, not a string blob: {blocks:?}"
    );

    // 2. the result is its own message, bound back by toolCallId
    let res = &e[2]["message"];
    assert_eq!(res["role"], "toolResult", "{res}");
    assert_eq!(res["toolCallId"], "call-1", "{res}");
    assert_eq!(res["toolName"], "read", "{res}");
    assert_eq!(res["isError"], false, "{res}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_failed_tool_is_marked_as_an_error() {
    let (state, sink, dir) = new_state("err");
    state.lock().unwrap().push_user("break it");

    drain(
        &sink,
        &state,
        vec![
            AgentEventLite::ToolCallStart {
                id: "c".into(),
                name: "edit".into(),
            },
            AgentEventLite::ToolCallStop {
                id: "c".into(),
                args: "{}".into(),
            },
            AgentEventLite::ToolResult {
                id: "c".into(),
                name: "edit".into(),
                ok: false,
                content: "boom".into(),
                details: None,
                duration_ms: 1,
            },
            AgentEventLite::TurnEnd,
        ],
    );

    let e = entries(&state);
    let res = &e[e.len() - 1]["message"];
    assert_eq!(res["role"], "toolResult", "{e:#?}");
    assert_eq!(
        res["isError"], true,
        "a failed tool must be recorded as an error or the model is told \
         it succeeded: {e:#?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn parallel_tool_calls_keep_their_own_arguments() {
    // The v0.8.8 bug this file's sibling fixed for the transcript: parallel
    // calls overwrote one another's arguments. Persistence must not
    // reintroduce it.
    let (state, sink, dir) = new_state("parallel");
    state.lock().unwrap().push_user("read three files");

    let mut evs = vec![AgentEventLite::TextDelta("reading all three".into())];
    for (i, path) in ["a.txt", "b.txt", "c.txt"].iter().enumerate() {
        let id = format!("call-{i}");
        evs.push(AgentEventLite::ToolCallStart {
            id: id.clone(),
            name: "read".into(),
        });
        evs.push(AgentEventLite::ToolCallStop {
            id,
            args: format!("{{\"path\":\"{path}\"}}"),
        });
    }
    for i in 0..3 {
        evs.push(AgentEventLite::ToolResult {
            id: format!("call-{i}"),
            name: "read".into(),
            ok: true,
            content: format!("body {i}"),
            details: None,
            duration_ms: 1,
        });
    }
    evs.push(AgentEventLite::TurnEnd);
    drain(&sink, &state, evs);

    let e = entries(&state);
    let blocks = e[1]["message"]["content"].as_array().unwrap().clone();
    let calls: Vec<&serde_json::Value> =
        blocks.iter().filter(|b| b["type"] == "toolCall").collect();
    assert_eq!(
        calls.len(),
        3,
        "all three calls must be recorded: {blocks:?}"
    );

    let mut seen: Vec<String> = calls
        .iter()
        .map(|c| c["arguments"]["path"].as_str().unwrap().to_string())
        .collect();
    seen.sort();
    assert_eq!(
        seen,
        vec!["a.txt", "b.txt", "c.txt"],
        "each call must keep its own arguments: {calls:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn prose_with_no_tool_call_still_reaches_the_session() {
    let (state, sink, dir) = new_state("prose");
    state.lock().unwrap().push_user("hello");
    drain(
        &sink,
        &state,
        vec![
            AgentEventLite::TextDelta("hi ".into()),
            AgentEventLite::TextDelta("there".into()),
            AgentEventLite::TurnEnd,
        ],
    );

    let e = entries(&state);
    assert_eq!(e.len(), 2, "user + assistant: {e:#?}");
    let text = e[1]["message"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(
        text, "hi there",
        "streamed deltas must join into one message"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_next_user_message_cannot_overtake_an_unflushed_turn() {
    let (state, sink, dir) = new_state("order");
    state.lock().unwrap().push_user("first question");
    drain(
        &sink,
        &state,
        vec![
            AgentEventLite::TextDelta("the answer".into()),
            AgentEventLite::ToolCallStart {
                id: "c".into(),
                name: "read".into(),
            },
            AgentEventLite::ToolCallStop {
                id: "c".into(),
                args: "{}".into(),
            },
        ],
    );
    // No TurnEnd, no ToolResult: the assistant turn is still pending.
    state.lock().unwrap().push_user("second question");

    let e = entries(&state);
    let roles: Vec<String> = e
        .iter()
        .map(|x| x["message"]["role"].as_str().unwrap_or("?").to_string())
        .collect();
    assert_eq!(
        roles,
        vec!["user", "assistant", "user"],
        "the pending turn must land before the next user message: {e:#?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
/// The assertions above read the in-memory session. This one checks the
/// bytes on disk, because "it is in the struct" and "it survives a restart"
/// are different claims and only the second one matters to a resumed session.
///
/// `session_flush` writes on a background thread, so this polls rather than
/// assuming the write has landed.
#[tokio::test]
async fn the_persisted_turn_survives_a_restart() {
    let (state, sink, dir) = new_state("restart");
    state.lock().unwrap().push_user("read the config");
    drain(
        &sink,
        &state,
        vec![
            AgentEventLite::TextDelta("I will read it.".into()),
            AgentEventLite::ToolCallStart {
                id: "call-1".into(),
                name: "read".into(),
            },
            AgentEventLite::ToolCallStop {
                id: "call-1".into(),
                args: "{\"path\":\"a.txt\"}".into(),
            },
            AgentEventLite::ToolResult {
                id: "call-1".into(),
                name: "read".into(),
                ok: true,
                content: "file body".into(),
                details: None,
                duration_ms: 4,
            },
            AgentEventLite::TurnEnd,
        ],
    );

    let path = dir.join("s.jsonl");
    state.lock().unwrap().session_flush();

    let mut text = String::new();
    for _ in 0..100 {
        text = std::fs::read_to_string(&path).unwrap_or_default();
        if text.contains("toolResult") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    for needle in ["toolCall", "toolResult", "I will read it", "file body"] {
        assert!(
            text.contains(needle),
            "`{needle}` must be in the session file, not only in the \
             transcript. File was:\n{text}"
        );
    }

    // And the file must parse back through the normal reader.
    let reloaded = nini_session::Session::read_from_file(&path).expect("the file must reload");
    assert!(
        reloaded.entries.iter().any(|e| matches!(
            e,
            nini_core::SessionEntry::Message(m)
                if matches!(m.message, nini_core::entries::AgentMessage::ToolResult(_))
        )),
        "the tool result must be readable as a real session entry, not \
         merely present as text"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
