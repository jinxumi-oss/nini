//! Oracle: `prepareNextTurn` — a live model change reaches the next turn.
//!
//! This is the last unimplemented field of Pi's `AgentLoopConfig`
//! (`pi-agent-core/src/types.ts`). It is not an abstract extension point:
//! Pi's own session layer uses it for one concrete job
//! (`pi-coding-agent/src/core/agent-session.ts#_installAgentNextTurnRefresh-554`,
//! `_installAgentNextTurnRefresh`):
//!
//! ```ts
//! this.agent.prepareNextTurnWithContext = async (turn, signal) => ({
//!     context: { ...previousContext, systemPrompt: …, tools: … },
//!     model: this.agent.state.model,
//!     thinkingLevel: this.agent.state.thinkingLevel,
//! });
//! ```
//!
//! Re-syncing live session state into the loop is what makes a change made
//! *while the agent is running* take effect. Concretely: the user presses
//! Ctrl+L, or runs `/model`, mid-task. Pi's next provider request uses the
//! new model.
//!
//! nini could not do this. The TUI updates `state.model_state.model`
//! (`runtime.rs:583`, `:1099`) and the status bar reflects it, but
//! `Agent::run` clones `RunConfig` once at entry (`agent.rs:748`) and the
//! loop reuses that clone, so the running agent never saw the change. The
//! user saw the status bar say one model and the model was called on
//! another.
//!
//! ## Scope: model only, deliberately
//!
//! Pi's `AgentLoopTurnUpdate` also carries `context` and `thinkingLevel`.
//! Neither is implemented here, and neither *should* be, because nini has
//! no consumer for them:
//!
//!   * `thinkingLevel` — nini's `Request` (`provider.rs:120-128`) has no
//!     thinking/reasoning field at all. `thinking_level` is stored in TUI
//!     state and written to settings.json, but no provider has ever
//!     received it. Wiring a `TurnUpdate` field for it would be a
//!     field with no reader — the `branch_summary` mistake.
//!   * `context` — Pi re-syncs from a long-lived session object. nini's
//!     `Agent` is constructed fresh per `run` by the driver closure, so
//!     there is no session to re-read.
//!
//! The loop ORDER is also pinned, because it is easy to get backwards:
//! `agent-loop.ts#prepareNextTurn-250` runs `prepareNextTurn` first, then
//! `shouldStopAfterTurn`.

use nini_core::agent::Agent;
use nini_core::agent_hooks::{AgentLoopHooks, TurnUpdate};
use nini_core::provider::Message;
use nini_core::provider::{
    Capabilities, Provider, ProviderError, Request, StreamEvent, Usage as PUsage,
};
use nini_core::tool::{Tool, ToolContext, ToolOutput, ToolRegistry};
use nini_core::RunConfig;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

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

/// Records the model on every request, and emits a tool call for the first
/// `turns` requests so the loop keeps going.
struct RecordingProvider {
    seen: Arc<Mutex<Vec<String>>>,
    calls: Arc<AtomicUsize>,
    turns: usize,
}

impl Provider for RecordingProvider {
    fn name(&self) -> &'static str {
        "recording"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
    fn stream(
        &self,
        req: Request,
    ) -> Pin<Box<dyn futures_util::Stream<Item = Result<StreamEvent, ProviderError>> + Send>> {
        use nini_core::provider::StreamEvent as E;
        self.seen.lock().unwrap().push(req.model.clone());
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        let mut evs: Vec<Result<StreamEvent, ProviderError>> = vec![Ok(E::MessageStart {
            id: format!("m{n}"),
            model: "x".into(),
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

/// Always reports a new model — the user pressed Ctrl+L while turn 1 was
/// running. The next request must use it.
///
/// An earlier version of this test switched only from the *second* hook
/// call and then asserted `seen[1] == "model-B"`. That was off by one in
/// the fixture rather than the code: a decision made after turn 2 cannot
/// affect turn 2. Always-on is both simpler and exactly the scenario —
/// the change is made during turn 1, so turn 2 is the first that can
/// possibly see it.
struct AlwaysSwitch {
    fired: AtomicUsize,
}

impl AgentLoopHooks for AlwaysSwitch {
    fn prepare_next_turn(&self, _messages: &[Message]) -> TurnUpdate {
        self.fired.fetch_add(1, Ordering::SeqCst);
        TurnUpdate {
            model: Some("model-B".into()),
            ..Default::default()
        }
    }
}

async fn drive(hooks: Arc<dyn AgentLoopHooks>, turns: usize) -> Vec<String> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut registry = ToolRegistry::new();
    registry.register_mut(Arc::new(Noop));
    let mut agent = Agent::with_hooks(
        Arc::new(RecordingProvider {
            seen: seen.clone(),
            calls: Arc::new(AtomicUsize::new(0)),
            turns,
        }),
        registry,
        RunConfig::new("model-A"),
        hooks,
    );
    use futures_util::StreamExt;
    let s = agent.run(nini_core::AgentMessage::user("go"));
    tokio::pin!(s);
    while let Some(_e) = s.next().await {}
    let out = seen.lock().unwrap().clone();
    out
}

/// The whole point: a model change made mid-run reaches the next request.
#[tokio::test(flavor = "current_thread")]
async fn a_model_change_reaches_the_next_provider_request() {
    let h = Arc::new(AlwaysSwitch {
        fired: AtomicUsize::new(0),
    });
    let seen = drive(h, 3).await;
    assert_eq!(
        seen.first().map(String::as_str),
        Some("model-A"),
        "the first turn must use the model the run started with"
    );
    assert!(
        seen.iter().any(|m| m == "model-B"),
        "the model switch never reached a provider request; saw {seen:?}"
    );
    assert_eq!(
        &seen[1..],
        ["model-B", "model-B", "model-B"],
        "the switch must take effect on the very next turn and stick; saw {seen:?}"
    );
}

/// Control: a hook that changes nothing leaves the model alone.
#[tokio::test(flavor = "current_thread")]
async fn a_hook_that_returns_nothing_changes_nothing() {
    let seen = drive(Arc::new(nini_core::agent_hooks::NoopHooks), 3).await;
    assert!(
        seen.iter().all(|m| m == "model-A"),
        "the default hook must not alter the model; saw {seen:?}"
    );
}

/// Pi runs `prepareNextTurn` BEFORE `shouldStopAfterTurn`
/// (`agent-loop.ts:181-250`). Getting this backwards would let a stop
/// verdict be computed against a stale model.
#[tokio::test(flavor = "current_thread")]
async fn prepare_next_turn_runs_before_should_stop_after_turn() {
    let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    struct Order {
        order: Arc<Mutex<Vec<&'static str>>>,
    }
    impl AgentLoopHooks for Order {
        fn prepare_next_turn(&self, _m: &[Message]) -> TurnUpdate {
            self.order.lock().unwrap().push("prepare");
            TurnUpdate {
                model: Some("model-B".into()),
                ..Default::default()
            }
        }
        fn should_stop_after_turn(&self, _m: &[Message]) -> bool {
            self.order.lock().unwrap().push("stop");
            true
        }
    }
    let _ = &order;
    let o = Arc::new(Mutex::new(Vec::new()));
    drive(Arc::new(Order { order: o.clone() }), 3).await;
    let seen = o.lock().unwrap().clone();
    assert_eq!(
        seen,
        vec!["prepare", "stop"],
        "Pi evaluates prepareNextTurn (:236) before shouldStopAfterTurn (:250)"
    );
}
