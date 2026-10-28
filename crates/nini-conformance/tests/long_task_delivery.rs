//! Oracle: a long task must reach its **delivery**, not be truncated at a
//! turn cap.
//!
//! # What Pi actually does
//!
//! Verified against the frozen snapshot
//! `references/pi-0.85.1/src/loop/agent-loop.ts`:
//!
//! ```text
//!  175   while (hasMoreToolCalls || pendingMessages.length > 0) {   // inner
//!  264     …
//!  272   while (true) {                                             // outer
//! ```
//!
//! and there is **no turn counter anywhere** in the snapshot:
//! `maxIterations` / `max_turns` / `maxTurns` / `turnCount` /
//! `iterationCount` — zero hits under `references/pi-0.85.1/src/`.
//! Pi stops when the model stops, when the user aborts, or when the
//! context overflows and cannot be compacted. Nothing else.
//!
//! # The defect this pins down
//!
//! nini capped one run at 50 turns (`MAX_TOOL_ITERATIONS = 50`,
//! `crates/nini-core/src/agent.rs`) and ended the run with
//! `AgentError::TooManyIterations(50)` when the counter tripped.
//!
//! Fifty turns is not "a lot of work" — it is *halfway*. A 60-step task
//! therefore died with `too many tool iterations (50)` **before
//! delivering anything**: the transcript showed 49 tool results and no
//! answer, and the session went back to idle. Pi, given the identical
//! transcript and the identical model, finished the task.
//!
//! A stop that lands between "still working" and "done" is not a safety
//! net, it is a truncation. The counter was nini's own invention — it is
//! not in the ledger (`scripts/parity-gaps.json`), so it was never a
//! decided product behaviour either.
//!
//! # What these tests assert
//!
//! 1. A 60-step task **delivers**, and the provider is called exactly
//!    61 times (60 tool turns + 1 final turn) — the same number Pi
//!    makes on the same transcript.
//! 2. The cap still exists as an **opt-in** guard: an explicit
//!    `max_iterations` stops the run, and the history survives so the
//!    user can continue.
//! 3. `max_follow_up_rounds = 0` means *no ceiling* (Pi's outer loop is
//!    `while (true)`), not *stop immediately* — the reading that would
//!    silently truncate a queued follow-up.
//!
//! invariant: 2297600

use futures_util::Stream;
use nini_core::agent::Agent;
use nini_core::agent_hooks::AgentLoopHooks;
use nini_core::provider::{
    Capabilities, Provider, ProviderError, Request, StreamEvent, Usage as PUsage,
};
use nini_core::tool::{Tool, ToolContext, ToolError, ToolOutput, ToolRegistry, ToolSpec};
use nini_core::RunConfig;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// The model: `tool_steps` tool calls, then one final text turn.
struct LongTaskProvider {
    calls: Arc<AtomicUsize>,
    tool_steps: usize,
}

impl Provider for LongTaskProvider {
    fn name(&self) -> &'static str {
        "long-task"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
    fn stream(
        &self,
        _req: Request,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send + 'static>> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        let id = format!("msg-{n}");
        if n <= self.tool_steps {
            let tool_id = format!("tc-{n}");
            Box::pin(futures_util::stream::iter(vec![
                Ok(StreamEvent::MessageStart {
                    id,
                    model: "test".into(),
                }),
                Ok(StreamEvent::ToolCallStart {
                    id: tool_id.clone(),
                    name: "bash".into(),
                }),
                Ok(StreamEvent::ToolCallDelta {
                    id: tool_id.clone(),
                    input_json_delta: "{}".into(),
                }),
                Ok(StreamEvent::ToolCallStop {
                    id: tool_id,
                    input_json: serde_json::json!({"command": format!("step-{n}")}),
                }),
                Ok(StreamEvent::MessageStop {
                    stop_reason: "tool_use".into(),
                    usage: PUsage::default(),
                }),
            ]))
        } else {
            Box::pin(futures_util::stream::iter(vec![
                Ok(StreamEvent::MessageStart {
                    id,
                    model: "test".into(),
                }),
                Ok(StreamEvent::TextDelta {
                    text: "TASK DELIVERED: all steps done".to_string(),
                }),
                Ok(StreamEvent::MessageStop {
                    stop_reason: "stop".into(),
                    usage: PUsage::default(),
                }),
            ]))
        }
    }
}

/// A tool that always succeeds, so the only thing that can end the run
/// early is the agent itself.
struct Echo;

#[async_trait::async_trait]
impl Tool for Echo {
    fn name(&self) -> &'static str {
        "bash"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "bash".to_string(),
            description: "test tool".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
        }
    }
    async fn execute(
        &self,
        _args: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput {
            content: "ok".to_string(),
            is_error: false,
            details: None,
            duration_ms: 1,
            terminate: false,
        })
    }
}

/// Drain a run, collecting the final text and the error, if any.
async fn drain(agent: &mut Agent) -> (String, Option<String>) {
    use futures_util::StreamExt;
    let mut text = String::new();
    let mut error = None;
    let s = agent.run(nini_core::AgentMessage::user("do the task"));
    tokio::pin!(s);
    while let Some(ev) = s.next().await {
        match ev {
            Ok(nini_core::AgentEvent::TextDelta { text: t }) => text.push_str(&t),
            Ok(_) => {}
            Err(e) => {
                error = Some(e.to_string());
                break;
            }
        }
    }
    (text, error)
}

/// The headline test: 60 steps, and the task **delivers**.
#[tokio::test(flavor = "current_thread")]
async fn a_sixty_step_task_is_delivered() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut agent = Agent::new(
        Arc::new(LongTaskProvider {
            calls: calls.clone(),
            tool_steps: 60,
        }),
        ToolRegistry::new().register(Arc::new(Echo)),
        // The **default** config: no cap. This is the shape of the bug —
        // the old default of 50 truncated this very run.
        RunConfig::new("test-model"),
    );

    let (text, error) = drain(&mut agent).await;

    assert!(
        error.is_none(),
        "a 60-step task must not end in an error: {error:?}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        61,
        "60 tool turns + 1 final turn — the same number Pi makes on the \
         same transcript"
    );
    assert!(
        text.contains("TASK DELIVERED"),
        "the delivery text never arrived: {text:?}"
    );
}

/// The cap is still available — as an opt-in, not as a default.
#[tokio::test(flavor = "current_thread")]
async fn an_explicit_cap_still_stops_the_run() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut agent = Agent::new(
        Arc::new(LongTaskProvider {
            calls: calls.clone(),
            tool_steps: 60,
        }),
        ToolRegistry::new().register(Arc::new(Echo)),
        RunConfig {
            max_iterations: 3,
            ..RunConfig::new("test-model")
        },
    );

    let (text, error) = drain(&mut agent).await;

    assert!(error.is_some(), "an explicit cap must still stop the run");
    let err = error.unwrap();
    assert!(
        err.contains("iteration limit reached (3)"),
        "the error must name the limit that was hit: {err}"
    );
    assert!(
        err.contains("send another message to continue"),
        "the error must say what the user can do next: {err}"
    );
    assert!(
        !text.contains("TASK DELIVERED"),
        "a capped run must not claim delivery: {text:?}"
    );
    assert!(
        agent.messages().len() > 1,
        "the history must survive the cap, otherwise the user cannot continue"
    );
}

/// A follow-up producer that never stops on its own.
struct FollowupProducer {
    remaining: AtomicUsize,
}

impl AgentLoopHooks for FollowupProducer {
    fn get_followup_messages(
        &self,
        _messages: &[nini_core::provider::Message],
    ) -> Vec<nini_core::provider::Message> {
        if self.remaining.fetch_sub(1, Ordering::SeqCst) == 0 {
            return Vec::new();
        }
        vec![nini_core::provider::Message::user("keep going")]
    }
}

/// `max_follow_up_rounds = 0` means *no ceiling*.
#[tokio::test(flavor = "current_thread")]
async fn a_zero_follow_up_ceiling_means_unlimited() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut agent = Agent::with_hooks(
        Arc::new(LongTaskProvider {
            calls: calls.clone(),
            tool_steps: 0, // every turn is a text turn, so the loop keeps polling
        }),
        ToolRegistry::new().register(Arc::new(Echo)),
        RunConfig {
            max_follow_up_rounds: 0,
            ..RunConfig::new("test-model")
        },
        Arc::new(FollowupProducer {
            remaining: AtomicUsize::new(5),
        }),
    );

    let (_text, error) = drain(&mut agent).await;

    assert!(error.is_none(), "the run must not fail: {error:?}");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        6,
        "1 initial turn + 5 follow-ups — a ceiling of 0 must not mean \
         'stop after the first turn'"
    );
}
