//! A3b: an overflow compaction must be recorded.
//!
//! This was the last place a compaction could change the model's context
//! without leaving a trace. The Agent detected a recoverable length stop,
//! compacted its own memory, emitted a `Compacted` event that nothing in
//! the tree consumed, and continued. The session was never told.
//!
//! The Agent cannot reach the session, so a caller that can installs a
//! compaction hook and the Agent delegates. Pi's sequence, at
//! `agent-session.ts:2137-2161`:
//!
//! ```text
//!   compact -> sessionManager.appendCompaction(...)
//!          -> buildSessionContext() -> agent.state.messages = ctx
//! ```
//!
//! These tests drive a real Agent with a provider that reports a
//! recoverable length stop, and check that the session ends up with a
//! `CompactionEntry` rather than a changed memory and nothing else.

use nini_ai::fixture::{FixtureTurn, ProgrammedProvider};
use nini_core::provider::{ContentBlock, Message, Provider, Role, Usage};
use nini_core::tool::ToolRegistry;
use nini_core::{Agent, AgentEvent, CompactionHook, CompactionResult, RunConfig};
use std::sync::{Arc, Mutex};

/// A provider whose turn ends in a length stop, which is what makes the
/// overflow path reachable.
fn length_stop_provider() -> Arc<dyn Provider> {
    Arc::new(ProgrammedProvider::from_turns(vec![
        vec![
            FixtureTurn::Text("this is a long answer that got cut off".into()),
            FixtureTurn::Stop {
                stop_reason: "length".into(),
                usage: Usage::default(),
            },
        ],
        vec![
            FixtureTurn::Text("and here is the answer after compaction".into()),
            FixtureTurn::Stop {
                stop_reason: "end_turn".into(),
                usage: Usage::default(),
            },
        ],
    ]))
}

fn big_history(n: usize) -> Vec<Message> {
    (0..n)
        .map(|i| Message {
            role: if i % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            },
            content: vec![ContentBlock::Text {
                text: format!("message {i} {}", "x".repeat(50)),
            }],
            timestamp: 0,
        })
        .collect()
}

fn config() -> RunConfig {
    let mut cfg = RunConfig::new("test-model");
    cfg.compaction.context_window = 200;
    cfg.compaction.reserve_tokens = 50;
    cfg.compaction.keep_recent_tokens = 40;
    cfg
}

#[tokio::test(flavor = "current_thread")]
async fn the_hook_is_called_on_an_overflow_stop() {
    let called = Arc::new(Mutex::new(Vec::<u32>::new()));
    let c2 = Arc::clone(&called);
    let mut agent = Agent::new(length_stop_provider(), ToolRegistry::new(), config());
    agent.seed(big_history(20));
    agent.set_compaction_hook(CompactionHook::new(move |msgs| {
        c2.lock().unwrap().push(msgs.len() as u32);
        Some(CompactionResult {
            summary: "SUMMARY FROM THE SESSION".into(),
            tokens_before: 999,
            tokens_after: 10,
            messages: msgs[..2].to_vec(),
        })
    }));

    let mut stream = Box::pin(agent.run(Message::user("go")));
    while let Some(_ev) = futures_util::StreamExt::next(&mut stream).await {}

    assert!(
        !called.lock().unwrap().is_empty(),
        "a recoverable length stop must reach the caller's hook, or the \\
         Agent compacts its own memory and nothing is recorded"
    );
}

/// The hook's answer must become the Agent's history, not just a number.
#[tokio::test(flavor = "current_thread")]
async fn the_hook_decides_what_history_continues() {
    let marker = Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "ONLY THIS".into(),
        }],
        timestamp: 0,
    };
    let m2 = marker.clone();
    let mut agent = Agent::new(length_stop_provider(), ToolRegistry::new(), config());
    agent.seed(big_history(20));
    agent.set_compaction_hook(CompactionHook::new(move |_msgs| {
        Some(CompactionResult {
            summary: "S".into(),
            tokens_before: 1,
            tokens_after: 1,
            messages: vec![m2.clone()],
        })
    }));

    {
        let mut stream = Box::pin(agent.run(Message::user("go")));
        while let Some(_ev) = futures_util::StreamExt::next(&mut stream).await {}
    }

    let texts: Vec<String> = agent
        .messages()
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(
        texts.iter().any(|t| t == "ONLY THIS"),
        "the history the hook returned must be the history the run \\
         continues with. The Agent kept its own: {texts:?}"
    );
}

/// `None` from the hook means "nothing to compact" and the run continues
/// with the history it had. Treating it as a failure would drop a turn.
#[tokio::test(flavor = "current_thread")]
async fn a_hook_that_declines_leaves_the_history_alone() {
    let before = big_history(20).len();
    let mut agent = Agent::new(length_stop_provider(), ToolRegistry::new(), config());
    agent.seed(big_history(20));
    agent.set_compaction_hook(CompactionHook::new(|_msgs| None));

    {
        let mut stream = Box::pin(agent.run(Message::user("go")));
        while let Some(_ev) = futures_util::StreamExt::next(&mut stream).await {}
    }

    assert!(
        agent.messages().len() >= before,
        "declining compaction must not discard history: {} < {before}",
        agent.messages().len()
    );
}

/// With no hook installed the fallback stands: the Agent compacts its own
/// memory and emits `Compacted` with a locally-generated summary. Nothing
/// is recorded — the Agent has no session to record it in — and that is
/// precisely the gap `A3b` closes for callers that can reach one.
///
/// This test states the fallback rather than treating it as correct.
#[tokio::test(flavor = "current_thread")]
async fn without_a_hook_the_agent_falls_back_to_compacting_itself() {
    let mut agent = Agent::new(length_stop_provider(), ToolRegistry::new(), config());
    agent.seed(big_history(20));
    let seeded = agent.messages().len();

    let mut summary = None;
    {
        let mut stream = Box::pin(agent.run(Message::user("go")));
        while let Some(ev) = futures_util::StreamExt::next(&mut stream).await {
            if let Ok(AgentEvent::Compacted { summary: s, .. }) = &ev {
                summary = Some(s.clone());
            }
        }
    }

    assert!(
        summary.is_some(),
        "the fallback must still compact, or a caller that installs no \
         hook loses overflow recovery entirely"
    );
    assert!(
        !summary.unwrap().contains("SUMMARY FROM THE SESSION"),
        "the fallback uses the local summariser, not a caller's — which is \
         how a caller finds out it did not install a hook"
    );
    assert!(
        agent.messages().len() < seeded,
        "and the Agent's own history must have been spliced: {} vs {seeded}",
        agent.messages().len()
    );
}

/// The event the Agent emits must still be produced, so a UI can show it.
#[tokio::test(flavor = "current_thread")]
async fn the_compacted_event_is_still_emitted() {
    let mut agent = Agent::new(length_stop_provider(), ToolRegistry::new(), config());
    agent.seed(big_history(20));
    agent.set_compaction_hook(CompactionHook::new(|msgs| {
        Some(CompactionResult {
            summary: "SUMMARY FROM THE SESSION".into(),
            tokens_before: 4242,
            tokens_after: 10,
            messages: msgs[..1].to_vec(),
        })
    }));

    let mut seen = None;
    let mut stream = Box::pin(agent.run(Message::user("go")));
    while let Some(ev) = futures_util::StreamExt::next(&mut stream).await {
        if let Ok(AgentEvent::Compacted {
            summary,
            tokens_before,
        }) = &ev
        {
            seen = Some((summary.clone(), *tokens_before));
        }
    }
    assert_eq!(
        seen,
        Some(("SUMMARY FROM THE SESSION".to_string(), 4242)),
        "the event must carry what the hook reported, so a UI can report it"
    );
}
