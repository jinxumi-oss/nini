//! Oracle: `shouldStopAfterTurn` is consulted at Pi's position in the turn.
//!
//! Pi's per-turn order (`agent-loop.ts:225-250`):
//!
//! ```text
//! LLM call
//!   → tool execution          (hasMoreToolCalls = !batch.terminate)
//!   → emit(turn_end)
//!   → prepareNextTurn          (may swap context / model / thinking level)
//!   → shouldStopAfterTurn      ← if true: agent_end + return
//!   → poll steering            (agent-loop.js:160)
//! ```
//!
//! The hook therefore sees the turn that just **finished**, including its
//! tool results, and returning true ends the run *before* the queues are
//! polled — so any queued steering and follow-up are discarded.
//!
//! nini consulted it at the top of the loop, before the LLM call for the
//! turn it was about to make. That inverts both properties: the hook never
//! saw a completed turn, and its verdict was about a turn that had not
//! happened yet. An extension implementing "stop once the context is
//! nearly full" stopped one turn earlier than Pi does.
//!
//! The existing test `should_stop_after_turn_sees_messages_at_iteration_start`
//! asserted the inverted behaviour by name, so it had to be rewritten too.

use nini_core::agent::Agent;
use nini_core::agent_hooks::AgentLoopHooks;
use nini_core::provider::Message;
use nini_core::provider::{
    Capabilities, Provider, ProviderError, Request, StreamEvent, Usage as PUsage,
};
use nini_core::tool::{Tool, ToolContext, ToolOutput, ToolRegistry};
use nini_core::{Role, RunConfig};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Emits one tool call per turn, so the loop keeps going.
struct LoopingProvider {
    calls: Arc<AtomicUsize>,
    turns: usize,
}

impl Provider for LoopingProvider {
    fn name(&self) -> &'static str {
        "looping"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
    fn stream(
        &self,
        _req: Request,
    ) -> Pin<Box<dyn futures_util::Stream<Item = Result<StreamEvent, ProviderError>> + Send>> {
        use nini_core::provider::StreamEvent as E;
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        let mut evs: Vec<Result<StreamEvent, ProviderError>> = vec![Ok(E::MessageStart {
            id: format!("m{n}"),
            model: "t".into(),
        })];
        if n <= self.turns {
            evs.push(Ok(E::ToolCallStart {
                id: format!("c{n}"),
                name: "noop".into(),
            }));
            evs.push(Ok(E::ToolCallDelta {
                id: format!("c{n}"),
                input_json_delta: "{}".into(),
            }));
            evs.push(Ok(E::ToolCallStop {
                id: format!("c{n}"),
                input_json: serde_json::json!({}),
            }));
        }
        evs.push(Ok(E::MessageStop {
            stop_reason: if n <= self.turns {
                "toolUse".into()
            } else {
                "stop".into()
            },
            usage: PUsage::default(),
        }));
        Box::pin(futures_util::stream::iter(evs))
    }
}

struct Noop;
#[async_trait::async_trait]
impl Tool for Noop {
    fn name(&self) -> &'static str {
        "noop"
    }
    fn spec(&self) -> nini_core::tool::ToolSpec {
        nini_core::tool::ToolSpec {
            name: "noop".into(),
            description: "noop".into(),
            input_schema: serde_json::json!({ "type": "object" }),
        }
    }
    fn system_prompt_contribution(&self) -> Option<nini_core::tool::ToolSystemPrompt> {
        None
    }
    async fn execute(
        &self,
        _a: serde_json::Value,
        _c: ToolContext,
    ) -> Result<ToolOutput, nini_core::tool::ToolError> {
        Ok(ToolOutput::ok("ok"))
    }
}

/// Records, for each call, how many assistant messages existed at the time.
struct Observer {
    observed: Mutex<Vec<usize>>,
    stop_after: Option<usize>,
    steering: Mutex<Option<Vec<Message>>>,
    followup: Mutex<Option<Vec<Message>>>,
    steering_polls: AtomicUsize,
    followup_polls: AtomicUsize,
}

impl AgentLoopHooks for Observer {
    fn should_stop_after_turn(&self, msgs: &[Message]) -> bool {
        let n = msgs.iter().filter(|m| m.role == Role::Assistant).count();
        self.observed.lock().unwrap().push(n);
        self.stop_after.is_some_and(|k| n >= k)
    }
    fn get_steering_messages(&self, _r: &[Message]) -> Vec<Message> {
        self.steering_polls.fetch_add(1, Ordering::SeqCst);
        self.steering.lock().unwrap().take().unwrap_or_default()
    }
    fn get_followup_messages(&self, _m: &[Message]) -> Vec<Message> {
        self.followup_polls.fetch_add(1, Ordering::SeqCst);
        self.followup.lock().unwrap().take().unwrap_or_default()
    }
}

fn user(t: &str) -> Message {
    Message {
        role: Role::User,
        content: vec![nini_core::provider::ContentBlock::Text { text: t.into() }],
        timestamp: 0,
    }
}

async fn drive(hooks: Arc<Observer>, turns: usize) -> usize {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register_mut(Arc::new(Noop));
    let mut agent = Agent::with_hooks(
        Arc::new(LoopingProvider {
            calls: calls.clone(),
            turns,
        }),
        registry,
        RunConfig::new("test-model"),
        hooks,
    );
    use futures_util::StreamExt;
    let s = agent.run(nini_core::AgentMessage::user("go"));
    tokio::pin!(s);
    while let Some(_e) = s.next().await {}
    calls.load(Ordering::SeqCst)
}

fn observer() -> Arc<Observer> {
    Arc::new(Observer {
        observed: Mutex::new(Vec::new()),
        stop_after: None,
        steering: Mutex::new(None),
        followup: Mutex::new(None),
        steering_polls: AtomicUsize::new(0),
        followup_polls: AtomicUsize::new(0),
    })
}

/// The core assertion: Pi consults the hook **after** the turn completes,
/// so the very first observation already contains an assistant message.
#[tokio::test(flavor = "current_thread")]
async fn hook_is_consulted_after_the_turn_it_reports_on() {
    let h = observer();
    drive(h.clone(), 4).await;
    let seen = h.observed.lock().unwrap().clone();
    assert!(!seen.is_empty(), "hook was never consulted");
    assert!(
        seen[0] >= 1,
        "Pi calls shouldStopAfterTurn after turn_end (agent-loop.js:151), so the \
         first consultation must already see the assistant message of the turn \
         that just finished. Observations: {seen:?}"
    );
}

/// One consultation per completed turn, seeing 1, 2, 3, … assistant
/// messages — never 0, never a repeat, never a skip.
///
/// Stated as the invariant rather than a magic number: `LoopingProvider`
/// with `turns: 3` emits 3 tool-calling turns *plus* a final text-only
/// turn, so 4 assistant messages and therefore 4 consultations. Pinning
/// `[1, 2, 3]` would have encoded a miscount of the fixture.
#[tokio::test(flavor = "current_thread")]
async fn one_consultation_per_completed_turn_in_order() {
    let h = observer();
    let calls = drive(h.clone(), 3).await;
    let seen = h.observed.lock().unwrap().clone();
    let expected: Vec<usize> = (1..=calls).collect();
    assert_eq!(
        seen, expected,
        "each consultation must report exactly one more assistant message \
         than the last; the model was called {calls}×"
    );
}

/// Returning true ends the run and — per Pi — **discards the queues**, since
/// `agent_end` is emitted before `:160` and `:163` are reached.
#[tokio::test(flavor = "current_thread")]
async fn a_true_verdict_ends_the_run_without_reviving_the_queues() {
    let h = observer();
    *h.followup.lock().unwrap() = Some(vec![user("queued follow-up")]);
    *h.steering.lock().unwrap() = Some(vec![user("queued steering")]);
    // An `Observer` with a stop verdict needs its own instance because
    // `stop_after` is fixed at construction.
    let h2 = Arc::new(Observer {
        observed: Mutex::new(Vec::new()),
        stop_after: Some(1),
        steering: Mutex::new(Some(vec![user("queued steering")])),
        followup: Mutex::new(Some(vec![user("queued follow-up")])),
        steering_polls: AtomicUsize::new(0),
        followup_polls: AtomicUsize::new(0),
    });
    let calls = drive(h2.clone(), 4).await;
    assert_eq!(calls, 1, "the run must end on the first completed turn");
    assert_eq!(
        h2.followup_polls.load(Ordering::SeqCst),
        0,
        "Pi emits agent_end before the follow-up poll (:163), so the queued \
         follow-up is discarded, not executed"
    );
}
