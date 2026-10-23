//! Oracle: nini's tool batch must honour Pi's `executionMode` and
//! `terminate` semantics.
//!
//! Verified against Pi 0.84.2 `pi-agent-core/src/agent-loop.ts` (restored
//! from `dist/*.js.map` `sourcesContent`):
//!
//! ```ts
//! // types.ts:402-408 — per-tool override
//! /** "sequential": this tool must execute one at a time with other tool calls.
//!  *  "parallel": this tool can execute concurrently with other tool calls. */
//! executionMode?: ToolExecutionMode
//!
//! // agent-loop.ts:416-426 — ONE sequential tool downgrades the WHOLE batch
//! const hasSequentialToolCall = toolCalls.some(
//!     (tc) => currentContext.tools?.find((t) => t.name === tc.name)?.executionMode === "sequential");
//! if (config.toolExecution === "sequential" || hasSequentialToolCall) {
//!     return executeToolCallsSequential(...);
//! }
//! return executeToolCallsParallel(...);
//!
//! // agent-loop.ts:589-584 — ALL results must terminate, and not be empty
//! function shouldTerminateToolBatch(finalizedCalls) {
//!     return finalizedCalls.length > 0 && finalizedCalls.every((f) => f.result.terminate === true);
//! }
//!
//! // agent-loop.ts:235 — the batch's verdict drives the inner loop
//! hasMoreToolCalls = !executedToolBatch.terminate;
//! ```
//!
//! nini has neither: `ToolOutput` carries no `terminate`, `Tool` has no
//! `execution_mode`, and every batch runs through the parallel worker pool.
//!
//! The `every` in `shouldTerminateToolBatch` is the load-bearing detail and
//! is easy to get wrong: "any result terminated" would let one finished tool
//! cut off a sibling that still has work to report.

use nini_core::tool::{Tool, ToolContext, ToolOutput, ToolSpec, ToolSystemPrompt};
use nini_core::RunConfig;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

// =====================================================================
// executionMode — one sequential tool downgrades the whole batch
// =====================================================================

/// Records how many of its calls are in flight at once.
struct ModeProbe {
    execution_mode: nini_core::tool::ToolExecutionMode,
    name: &'static str,
    live: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    hold_ms: u64,
}

#[async_trait::async_trait]
impl Tool for ModeProbe {
    fn name(&self) -> &'static str {
        self.name
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.to_string(),
            description: "probe".to_string(),
            input_schema: serde_json::json!({ "type": "object" }),
        }
    }
    fn execution_mode(&self) -> nini_core::tool::ToolExecutionMode {
        self.execution_mode
    }
    fn system_prompt_contribution(&self) -> Option<ToolSystemPrompt> {
        None
    }
    async fn execute(
        &self,
        _args: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolOutput, nini_core::tool::ToolError> {
        let (live, peak, hold) = (self.live.clone(), self.peak.clone(), self.hold_ms);
        {
            let now = live.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(hold)).await;
            live.fetch_sub(1, Ordering::SeqCst);
            Ok(ToolOutput::ok("done"))
        }
    }
}

/// A provider that emits N tool calls to the given tools in one turn.
struct BatchProvider {
    calls: Vec<(String, String)>,
}

impl nini_core::provider::Provider for BatchProvider {
    fn name(&self) -> &'static str {
        "batch"
    }
    fn capabilities(&self) -> nini_core::provider::Capabilities {
        Default::default()
    }
    fn stream(
        &self,
        _req: nini_core::provider::Request,
    ) -> Pin<
        Box<
            dyn futures_util::Stream<
                    Item = Result<
                        nini_core::provider::StreamEvent,
                        nini_core::provider::ProviderError,
                    >,
                > + Send,
        >,
    > {
        use nini_core::provider::StreamEvent as E;
        let mut evs: Vec<Result<E, nini_core::provider::ProviderError>> =
            vec![Ok(E::MessageStart {
                id: "m".into(),
                model: "test".into(),
            })];
        for (i, (id, name)) in self.calls.iter().enumerate() {
            evs.push(Ok(E::ToolCallStart {
                id: id.clone(),
                name: name.clone(),
            }));
            evs.push(Ok(E::ToolCallDelta {
                id: id.clone(),
                input_json_delta: format!("{{\"n\":{i}}}"),
            }));
            evs.push(Ok(E::ToolCallStop {
                id: id.clone(),
                input_json: serde_json::json!({ "n": i }),
            }));
        }
        evs.push(Ok(E::MessageStop {
            stop_reason: "toolUse".into(),
            usage: Default::default(),
        }));
        Box::pin(futures_util::stream::iter(evs))
    }
}

async fn run_batch(tools: Vec<Arc<dyn Tool>>) -> usize {
    let calls: Vec<(String, String)> = (0..8)
        .map(|i| (format!("c{i}"), format!("t{}", i % tools.len())))
        .collect();
    let mut registry = nini_core::tool::ToolRegistry::new();
    for t in tools {
        registry.register_mut(t);
    }
    let mut agent = nini_core::agent::Agent::new(
        Arc::new(BatchProvider { calls }),
        registry,
        RunConfig::new("test-model"),
    );
    use futures_util::StreamExt;
    let s = agent.run(nini_core::AgentMessage::user("go"));
    tokio::pin!(s);
    while let Some(_e) = s.next().await {}
    0
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_sequential_tool_downgrades_the_whole_batch() {
    use nini_core::tool::ToolExecutionMode as M;
    let live = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    // 7 parallel tools plus 1 sequential one. Pi: the presence of the
    // sequential tool makes ALL eight run one at a time.
    let mut tools: Vec<Arc<dyn Tool>> = (0..7)
        .map(|i| {
            Arc::new(ModeProbe {
                execution_mode: M::Parallel,
                name: Box::leak(format!("t{i}").into_boxed_str()),
                live: live.clone(),
                peak: peak.clone(),
                hold_ms: 20,
            }) as Arc<dyn Tool>
        })
        .collect();
    tools.push(Arc::new(ModeProbe {
        execution_mode: M::Sequential,
        name: "t7",
        live: live.clone(),
        peak: peak.clone(),
        hold_ms: 20,
    }));

    run_batch(tools).await;

    assert_eq!(
        peak.load(Ordering::SeqCst),
        1,
        "Pi agent-loop.ts:416-426 downgrades the WHOLE batch when any tool is \
         sequential; peak concurrency was {}",
        peak.load(Ordering::SeqCst)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_all_parallel_batch_still_runs_concurrently() {
    use nini_core::tool::ToolExecutionMode as M;
    let live = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let tools: Vec<Arc<dyn Tool>> = (0..8)
        .map(|i| {
            Arc::new(ModeProbe {
                execution_mode: M::Parallel,
                name: Box::leak(format!("t{i}").into_boxed_str()),
                live: live.clone(),
                peak: peak.clone(),
                hold_ms: 20,
            }) as Arc<dyn Tool>
        })
        .collect();

    run_batch(tools).await;

    assert!(
        peak.load(Ordering::SeqCst) > 1,
        "an all-parallel batch must keep the concurrency v0.8.5 added; peak was {}",
        peak.load(Ordering::SeqCst)
    );
}

// =====================================================================
// shouldTerminateToolBatch — `every`, not `some`
// =====================================================================

/// Asserts Pi's predicate directly, independent of the agent.
#[test]
fn terminate_requires_every_result_and_a_non_empty_batch() {
    use nini_core::tool::should_terminate_tool_batch;
    assert!(
        !should_terminate_tool_batch(&[]),
        "an empty batch must not terminate (agent-loop.ts:590 requires length > 0)"
    );
    assert!(
        should_terminate_tool_batch(&[true, true, true]),
        "all results terminating ends the turn"
    );
    assert!(
        !should_terminate_tool_batch(&[true, true, false]),
        "ONE non-terminating result must keep the loop alive — this is the `every`, not `some`"
    );
    assert!(
        !should_terminate_tool_batch(&[false]),
        "a single non-terminating result keeps the loop alive"
    );
    assert!(
        should_terminate_tool_batch(&[true]),
        "a lone terminating result does end the turn"
    );
}

#[test]
fn tool_output_terminate_defaults_to_false_and_is_constructible() {
    // `ToolOutput::ok` is the constructor every tool uses; the new field
    // must not break it, and must be opt-in.
    let o = ToolOutput::ok("x");
    assert!(!o.terminate, "terminate must default to false");
    let o2 = ToolOutput {
        terminate: true,
        ..ToolOutput::ok("x")
    };
    assert!(o2.terminate);
}

// =====================================================================
// terminate — behavioural: the turn actually ends
// =====================================================================

/// A provider that counts how many times it was asked for a response.
struct CountingProvider {
    calls: Arc<AtomicUsize>,
    n_tool_calls: usize,
}

impl nini_core::provider::Provider for CountingProvider {
    fn name(&self) -> &'static str {
        "counting"
    }
    fn capabilities(&self) -> nini_core::provider::Capabilities {
        Default::default()
    }
    fn stream(
        &self,
        _req: nini_core::provider::Request,
    ) -> Pin<
        Box<
            dyn futures_util::Stream<
                    Item = Result<
                        nini_core::provider::StreamEvent,
                        nini_core::provider::ProviderError,
                    >,
                > + Send,
        >,
    > {
        use nini_core::provider::StreamEvent as E;
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        let mut evs: Vec<Result<E, nini_core::provider::ProviderError>> =
            vec![Ok(E::MessageStart {
                id: format!("m{n}"),
                model: "test".into(),
            })];
        for i in 0..self.n_tool_calls {
            evs.push(Ok(E::ToolCallStart {
                id: format!("c{n}-{i}"),
                name: "probe".into(),
            }));
            evs.push(Ok(E::ToolCallDelta {
                id: format!("c{n}-{i}"),
                input_json_delta: "{}".into(),
            }));
            evs.push(Ok(E::ToolCallStop {
                id: format!("c{n}-{i}"),
                input_json: serde_json::json!({}),
            }));
        }
        evs.push(Ok(E::MessageStop {
            stop_reason: if self.n_tool_calls > 0 {
                "toolUse".into()
            } else {
                "stop".into()
            },
            usage: Default::default(),
        }));
        Box::pin(futures_util::stream::iter(evs))
    }
}

struct TerminatingTool {
    name: &'static str,
    /// Which of this tool's calls terminate. `-1` = all.
    terminate_every: i32,
    n: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl Tool for TerminatingTool {
    fn name(&self) -> &'static str {
        self.name
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.to_string(),
            description: "probe".into(),
            input_schema: serde_json::json!({ "type": "object" }),
        }
    }
    fn system_prompt_contribution(&self) -> Option<ToolSystemPrompt> {
        None
    }
    async fn execute(
        &self,
        _args: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolOutput, nini_core::tool::ToolError> {
        let i = self.n.fetch_add(1, Ordering::SeqCst);
        // 0 = never, -1 = always, n>0 = every nth call.
        // Written as a match rather than `i % n` so that 0 cannot divide:
        // the first version panicked with `i % 0` and this test caught it.
        let terminate = match self.terminate_every {
            n if n < 0 => true,
            0 => false,
            n => i % (n as usize) == 0,
        };
        Ok(ToolOutput {
            terminate,
            ..ToolOutput::ok("x")
        })
    }
}

async fn calls_with(tool: Arc<dyn Tool>, n_tool_calls: usize) -> usize {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = nini_core::tool::ToolRegistry::new();
    registry.register_mut(tool);
    let mut agent = nini_core::agent::Agent::new(
        Arc::new(CountingProvider {
            calls: calls.clone(),
            n_tool_calls,
        }),
        registry,
        RunConfig::new("test-model"),
    );
    use futures_util::StreamExt;
    let s = agent.run(nini_core::AgentMessage::user("go"));
    tokio::pin!(s);
    while let Some(_e) = s.next().await {}
    calls.load(Ordering::SeqCst)
}

#[tokio::test(flavor = "current_thread")]
async fn all_results_terminating_ends_the_turn_after_one_model_call() {
    // Pi: `hasMoreToolCalls = !batch.terminate` (`agent-loop.js:214`). With
    // every result terminating, the model is not asked a second time.
    let tool = Arc::new(TerminatingTool {
        name: "probe",
        terminate_every: -1,
        n: Arc::new(AtomicUsize::new(0)),
    });
    let calls = calls_with(tool, 3).await;
    assert_eq!(
        calls, 1,
        "every result terminated, so the turn must end without another model call"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn one_non_terminating_result_keeps_the_loop_alive() {
    // The `every` in `agent-loop.ts:590`. With `some` instead, the first
    // call would end the turn and the sibling's work would be discarded.
    let tool = Arc::new(TerminatingTool {
        name: "probe",
        terminate_every: 2, // calls 0 and 2 terminate, 1 and 3 do not
        n: Arc::new(AtomicUsize::new(0)),
    });
    let calls = calls_with(tool, 4).await;
    assert!(
        calls >= 2,
        "a batch containing a non-terminating result must not end the turn; \
         model was called {calls}×"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_tool_that_never_terminates_behaves_exactly_as_before() {
    let tool = Arc::new(TerminatingTool {
        name: "probe",
        terminate_every: 0, // never
        n: Arc::new(AtomicUsize::new(0)),
    });
    let calls = calls_with(tool, 1).await;
    assert!(
        calls >= 2,
        "tools that do not terminate still drive the loop"
    );
}
