//! B20 — the `tool_execution_start` event, and **when** it fires.
//!
//! Pi has two distinct moments for a tool call
//! (`references/pi-0.85.1/src/loop/agent-loop.ts`):
//!
//!   * the model **says** it wants to call a tool → the transcript row
//!     appears immediately;
//!   * the runtime is **about to execute** it → `tool_execution_start`.
//!
//! The two execution branches differ, and the difference is observable:
//!
//!   * `executeToolCallsSequential` (`:431`) awaits the start *inside*
//!     the per-call loop → `start(A), end(A), start(B), end(B)`;
//!   * `executeToolCallsParallel` (`:487`) awaits the start for every
//!     call first, then runs them together → every start precedes
//!     every result.
//!
//! These tests pin the **order** of the events, not merely their
//! presence: an implementation that emitted all the starts up front
//! would still "contain" the right events and would still be wrong.

use async_trait::async_trait;
use futures_util::StreamExt;
use nini_ai::fixture::{FixtureTurn, ProgrammedProvider};
use nini_core::tool::{
    Tool, ToolContext, ToolError, ToolExecutionMode, ToolOutput, ToolRegistry, ToolSpec,
};
use nini_core::{Agent, AgentEvent, AgentMessage, RunConfig};
use serde_json::Value;
use std::sync::Arc;

/// A tool that does nothing but answer, so the only thing that varies
/// between the tests is the *order of the events*.
struct EchoTool {
    name: &'static str,
}

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &'static str {
        self.name
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.to_string(),
            description: "test tool".to_string(),
            input_schema: serde_json::json!({"type": "object", "properties": {}}),
        }
    }

    async fn execute(&self, _args: Value, _ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::ok(format!("{} ran", self.name)))
    }
}

/// One entry of the collected event stream.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ev {
    /// The model announced the call (model stream).
    ModelStart(String),
    /// The runtime is about to execute the call.
    ExecStart(String),
    /// The call finished.
    Result(String),
}

/// Collect the whole run as an ordered list.
///
/// The ORDER is the whole point of this test file, so the events are
/// flattened into a `Vec` rather than inspected as they arrive.
async fn collect(seq_mode: ToolExecutionMode) -> Vec<Ev> {
    let provider: Arc<dyn nini_core::Provider> = Arc::new(ProgrammedProvider::from_turns(vec![
        vec![
            FixtureTurn::ToolCall {
                name: "alpha".to_string(),
                args: serde_json::json!({}),
            },
            FixtureTurn::ToolCall {
                name: "beta".to_string(),
                args: serde_json::json!({}),
            },
            FixtureTurn::Stop {
                stop_reason: "tool_use".to_string(),
                usage: Default::default(),
            },
        ],
        vec![
            FixtureTurn::Text("done".to_string()),
            FixtureTurn::Stop {
                stop_reason: "end_turn".to_string(),
                usage: Default::default(),
            },
        ],
    ]));

    let tools = ToolRegistry::new()
        .register(Arc::new(EchoTool { name: "alpha" }))
        .register(Arc::new(EchoTool { name: "beta" }));

    let mut config = RunConfig::new("test-model");
    config.tool_execution = seq_mode;

    let mut agent = Agent::new(provider, tools, config);
    let mut stream = Box::pin(agent.run(AgentMessage::user("run both tools")));

    let mut events = Vec::new();
    while let Some(ev) = stream.next().await {
        match ev {
            Ok(AgentEvent::ToolCallStart { id, .. }) => events.push(Ev::ModelStart(id)),
            Ok(AgentEvent::ToolExecutionStart { id, .. }) => events.push(Ev::ExecStart(id)),
            Ok(AgentEvent::ToolResult { id, .. }) => events.push(Ev::Result(id)),
            Ok(_) => {}
            Err(e) => panic!("agent run failed: {e}"),
        }
    }
    events
}

/// The index of the first event of the given shape carrying `id`.
fn idx(events: &[Ev], want: &Ev) -> usize {
    events
        .iter()
        .position(|e| e == want)
        .unwrap_or_else(|| panic!("event {want:?} never happened; sequence: {events:?}"))
}

/// The id of the n-th model-announced call (the model stream order).
fn nth_model_start(events: &[Ev], n: usize) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            Ev::ModelStart(id) => Some(id.clone()),
            _ => None,
        })
        .nth(n)
        .unwrap_or_else(|| panic!("the model announced fewer than {} calls", n + 1))
}

/// The id of the n-th execution start.
fn nth_exec_start(events: &[Ev], n: usize) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            Ev::ExecStart(id) => Some(id.clone()),
            _ => None,
        })
        .nth(n)
        .unwrap_or_else(|| panic!("fewer than {} execution starts happened", n + 1))
}

/// A **sequential** batch interleaves start and end, exactly like Pi's
/// `executeToolCallsSequential`:
///
/// ```text
/// start(A), end(A), start(B), end(B)
/// ```
#[tokio::test]
async fn a_sequential_batch_interleaves_start_and_result() {
    let events = collect(ToolExecutionMode::Sequential).await;

    let a = nth_model_start(&events, 0);
    let b = nth_model_start(&events, 1);
    assert_ne!(a, b, "the two calls must have different ids");

    let idx_start_a = idx(&events, &Ev::ExecStart(a.clone()));
    let idx_result_a = idx(&events, &Ev::Result(a.clone()));
    let idx_start_b = idx(&events, &Ev::ExecStart(b.clone()));
    let idx_result_b = idx(&events, &Ev::Result(b.clone()));

    assert!(
        idx_start_a < idx_result_a,
        "A must start before it finishes (Pi: `await emit(start)` then the call)"
    );
    assert!(
        idx_result_a < idx_start_b,
        "in a sequential batch the second call starts only AFTER the first finished \
         (Pi `executeToolCallsSequential`, agent-loop.ts:431)"
    );
    assert!(
        idx_start_b < idx_result_b,
        "B must start before it finishes"
    );
}

/// A **parallel** batch emits every start before any result, exactly
/// like Pi's `executeToolCallsParallel` (agent-loop.ts:487): the loop
/// emits the start of each call first, and only then are they run
/// together.
#[tokio::test]
async fn a_parallel_batch_starts_everything_before_any_result() {
    let events = collect(ToolExecutionMode::Parallel).await;

    let a = nth_model_start(&events, 0);
    let b = nth_model_start(&events, 1);

    let idx_start_a = idx(&events, &Ev::ExecStart(a.clone()));
    let idx_start_b = idx(&events, &Ev::ExecStart(b.clone()));
    let idx_result_a = idx(&events, &Ev::Result(a.clone()));
    let idx_result_b = idx(&events, &Ev::Result(b.clone()));

    assert!(
        idx_start_a < idx_result_a,
        "A must start before it finishes"
    );
    assert!(
        idx_start_b < idx_result_b,
        "B must start before it finishes"
    );
    assert!(
        idx_start_b < idx_result_a,
        "in a parallel batch every start precedes every result \
         (Pi `executeToolCallsParallel`, agent-loop.ts:487); got {events:?}"
    );
    assert!(
        idx_start_a < idx_result_b,
        "in a parallel batch every start precedes every result; got {events:?}"
    );
}

/// The model-stream announcement is a *different* moment and stays
/// where it was: the transcript row exists long before the tool runs.
#[tokio::test]
async fn the_model_announcement_precedes_every_execution_start() {
    for mode in [ToolExecutionMode::Sequential, ToolExecutionMode::Parallel] {
        let events = collect(mode).await;

        let last_model_start = events
            .iter()
            .rposition(|e| matches!(e, Ev::ModelStart(_)))
            .expect("the model announced at least one call");
        let first_exec_start = events
            .iter()
            .position(|e| matches!(e, Ev::ExecStart(_)))
            .expect("at least one call executed");

        assert!(
            last_model_start < first_exec_start,
            "the model must finish announcing the batch before the first call runs \
             (mode {mode:?}); got {events:?}"
        );
    }
}

/// The wire name is Pi's: `tool_execution_start`.
#[test]
fn the_event_carries_pis_wire_name() {
    let ev = AgentEvent::ToolExecutionStart {
        id: "t1".to_string(),
        name: "alpha".to_string(),
    };
    let json = serde_json::to_value(&ev).expect("the event serializes");
    assert_eq!(json["type"], "tool_execution_start");
    assert_eq!(json["id"], "t1");
    assert_eq!(json["name"], "alpha");
}

/// The two execution starts carry the SAME ids as the model's
/// announcements — the event says "this call is running", not "this is
/// a different call".
#[tokio::test]
async fn an_execution_start_names_the_call_the_model_announced() {
    let events = collect(ToolExecutionMode::Sequential).await;
    let a = nth_model_start(&events, 0);
    let b = nth_model_start(&events, 1);
    assert_eq!(
        nth_exec_start(&events, 0),
        a,
        "the first execution start belongs to the first announced call"
    );
    assert_eq!(
        nth_exec_start(&events, 1),
        b,
        "the second execution start belongs to the second announced call"
    );
}
