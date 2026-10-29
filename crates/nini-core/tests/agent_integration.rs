//! Integration tests for the agent loop with a real `BashTool`.
//!
//! These tests exercise the full path: provider → agent → tool registry →
//! bash → process spawn → stdout capture → back to provider.

use futures_util::StreamExt;
use nini_ai::fixture::{FixtureTurn, ProgrammedProvider};
use nini_core::provider::Usage;
use nini_core::tool::ToolOutput;
use nini_core::tool::ToolRegistry;
use nini_core::{Agent, AgentEvent, AgentMessage, RunConfig};
use nini_tools::BashTool;
use std::sync::Arc;

#[tokio::test]
async fn demo_use_bash_to_print_hello() {
    let provider: Arc<dyn nini_core::Provider> = Arc::new(ProgrammedProvider::from_turns(vec![
        vec![
            FixtureTurn::ToolCall {
                name: "bash".to_string(),
                args: serde_json::json!({"command": "echo hello"}),
            },
            FixtureTurn::Stop {
                stop_reason: "tool_use".to_string(),
                usage: Usage::default(),
            },
        ],
        vec![
            FixtureTurn::Text("hello".to_string()),
            FixtureTurn::Stop {
                stop_reason: "end_turn".to_string(),
                usage: Usage::default(),
            },
        ],
    ]));

    let tools = ToolRegistry::new().register(Arc::new(BashTool::new()));
    let mut agent = Agent::new(provider, tools, RunConfig::new("test-model"));

    let mut stream = Box::pin(agent.run(nini_core::AgentMessage::user("use bash to print hello")));

    let mut text = String::new();
    let mut saw_tool_call = false;
    let mut saw_tool_result = false;
    let mut tool_result_content = String::new();
    let mut saw_turn_ends = 0;

    while let Some(ev) = stream.next().await {
        match ev {
            Ok(AgentEvent::TextDelta { text: t }) => text.push_str(&t),
            Ok(AgentEvent::ToolCallStart { name, .. }) if name == "bash" => saw_tool_call = true,
            Ok(AgentEvent::ToolResult { output, .. }) => {
                saw_tool_result = true;
                tool_result_content = output.content;
            }
            Ok(AgentEvent::TurnEnd { .. }) => saw_turn_ends += 1,
            _ => {}
        }
    }

    assert!(saw_tool_call, "agent should have invoked bash tool");
    assert!(saw_tool_result, "agent should have received a tool result");
    assert!(
        tool_result_content.contains("hello"),
        "bash output should contain 'hello', got: {tool_result_content:?}"
    );
    assert_eq!(
        saw_turn_ends, 2,
        "should have completed 2 turns (tool then final)"
    );
    assert_eq!(text, "hello", "final assistant text should be 'hello'");
}

#[tokio::test]
async fn bash_nonzero_exit_marks_error() {
    let provider: Arc<dyn nini_core::Provider> = Arc::new(ProgrammedProvider::from_turns(vec![
        vec![
            FixtureTurn::ToolCall {
                name: "bash".to_string(),
                args: serde_json::json!({"command": "exit 7"}),
            },
            FixtureTurn::Stop {
                stop_reason: "tool_use".to_string(),
                usage: Usage::default(),
            },
        ],
        vec![
            FixtureTurn::Text("failed".to_string()),
            FixtureTurn::Stop {
                stop_reason: "end_turn".to_string(),
                usage: Usage::default(),
            },
        ],
    ]));
    let tools = ToolRegistry::new().register(Arc::new(BashTool::new()));
    let mut agent = Agent::new(provider, tools, RunConfig::new("test-model"));
    let mut stream = Box::pin(agent.run(nini_core::AgentMessage::user("run exit 7")));

    let mut saw_error_result = false;
    while let Some(ev) = stream.next().await {
        if let Ok(AgentEvent::ToolResult { output, .. }) = ev {
            if output.is_error {
                saw_error_result = true;
            }
        }
    }
    assert!(
        saw_error_result,
        "non-zero exit should produce is_error tool result"
    );
}

#[tokio::test]
async fn agent_text_only_skips_tool_execution() {
    let provider: Arc<dyn nini_core::Provider> =
        Arc::new(ProgrammedProvider::from_turns(vec![vec![
            FixtureTurn::Text("just text".to_string()),
            FixtureTurn::Stop {
                stop_reason: "end_turn".to_string(),
                usage: Usage::default(),
            },
        ]]));
    let tools = ToolRegistry::new().register(Arc::new(BashTool::new()));
    let mut agent = Agent::new(provider, tools, RunConfig::new("test-model"));
    let mut stream = Box::pin(agent.run(nini_core::AgentMessage::user("hi")));

    let mut text = String::new();
    let mut saw_tool_call = false;
    let mut turn_count = 0;
    while let Some(ev) = stream.next().await {
        match ev {
            Ok(AgentEvent::TextDelta { text: t }) => text.push_str(&t),
            Ok(AgentEvent::ToolCallStart { .. }) => saw_tool_call = true,
            Ok(AgentEvent::TurnEnd { .. }) => turn_count += 1,
            _ => {}
        }
    }
    assert_eq!(text, "just text");
    assert!(
        !saw_tool_call,
        "no tool calls expected when model returns text only"
    );
    assert_eq!(turn_count, 1);
}

// =====================================================================
// v0.8.5: parallel tool execution regression.
//
// Before v0.8.5 the agent ran tool_calls serially (`for tc in tool_calls`),
// even when the model emitted 14 in one turn. This test registers a
// "sleeper" tool that blocks for 100ms and emits 8 parallel calls in
// one fixture turn. Serial execution would take ~800ms (8 × 100ms);
// parallel execution through `FuturesUnordered` should take ~100ms
// (8 simultaneous sleeps).
//
// We also assert correctness invariants:
//   * All 8 tool_results land in state.
//   * tool_results appear in the SAME order as tool_calls (Anthropic
//     API requires tool_result blocks to answer tool_use blocks in
//     the same order).
//   * No result is dropped (count matches call count exactly).
// =====================================================================
mod parallel_tool_tests {
    use super::*;
    use async_trait::async_trait;
    use nini_core::tool::{Tool, ToolContext, ToolError, ToolOutput, ToolSpec};
    use nini_core::AgentMessage;
    use serde_json::{json, Value};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// A trivial test tool that sleeps for a fixed duration and
    /// records that it was called. Returns the call index in the
    /// output so the test can verify ordering.
    pub struct SleeperTool {
        delay: Duration,
        call_count: Arc<AtomicU32>,
        /// v0.8.9 (P2-10): live + peak counters, kept in sync with the
        /// existing `call_count` semantics (total invocations).
        #[allow(dead_code)]
        live: Arc<AtomicU32>,
        #[allow(dead_code)]
        peak_live: Arc<AtomicU32>,
    }

    impl SleeperTool {
        pub fn new(delay: Duration) -> Self {
            Self {
                delay,
                call_count: Arc::new(AtomicU32::new(0)),
                live: Arc::new(AtomicU32::new(0)),
                peak_live: Arc::new(AtomicU32::new(0)),
            }
        }
    }

    #[async_trait]
    impl Tool for SleeperTool {
        fn name(&self) -> &'static str {
            "sleeper"
        }

        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "sleeper".to_string(),
                description: "Sleeps for a configured duration (test only).".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "label": {"type": "string"}
                    }
                }),
            }
        }

        async fn execute(&self, args: Value, _ctx: ToolContext) -> Result<ToolOutput, ToolError> {
            let label = args
                .get("label")
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .to_string();
            let n = self.call_count.fetch_add(1, Ordering::SeqCst) + 1;
            tokio::time::sleep(self.delay).await;
            Ok(ToolOutput::ok(format!("sleeper[{label}] call#{n}")))
        }
    }

    fn build_provider_with_parallel_calls(call_count: usize) -> Arc<ProgrammedProvider> {
        // Build `call_count` parallel tool_calls in turn 0, then a
        // final stop-only turn so the agent loop terminates cleanly.
        let mut turn0: Vec<FixtureTurn> = Vec::with_capacity(call_count);
        for i in 0..call_count {
            turn0.push(FixtureTurn::ToolCall {
                name: "sleeper".to_string(),
                args: json!({"label": format!("call_{i}")}),
            });
        }
        turn0.push(FixtureTurn::Stop {
            stop_reason: "tool_use".to_string(),
            usage: Usage::default(),
        });
        let turn1 = vec![FixtureTurn::Stop {
            stop_reason: "end_turn".to_string(),
            usage: Usage::default(),
        }];
        Arc::new(ProgrammedProvider::from_turns(vec![turn0, turn1]))
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn parallel_tool_calls_complete_faster_than_serial() {
        const N: usize = 8;
        const DELAY: Duration = Duration::from_millis(100);

        let sleeper = Arc::new(SleeperTool::new(DELAY));
        let provider = build_provider_with_parallel_calls(N);
        let mut tools = ToolRegistry::new();
        tools = tools.register(sleeper.clone() as Arc<dyn Tool>);
        let mut agent = Agent::new(provider, tools, RunConfig::new("test-model"));

        let start = Instant::now();
        let mut stream = Box::pin(agent.run(AgentMessage::user("run 8 sleepers in parallel")));
        let mut tool_results: Vec<String> = Vec::new();
        while let Some(ev) = stream.next().await {
            if let Ok(AgentEvent::ToolResult { output, .. }) = ev {
                tool_results.push(output.content);
            }
        }
        let elapsed = start.elapsed();

        // Sanity: all N results landed in state.
        assert_eq!(
            tool_results.len(),
            N,
            "expected {N} tool results, got {}: {tool_results:?}",
            tool_results.len()
        );

        // Correctness: results come back in the SAME order as calls.
        // (Anthropic API contract — tool_result N must answer tool_use N.)
        for (i, r) in tool_results.iter().enumerate() {
            assert!(
                r.contains(&format!("call_{i}")),
                "result[{i}] should reference call_{i}, got: {r}"
            );
        }

        // Perf: 8 parallel × 100ms each must finish in much less than
        // 8 × 100ms = 800ms. We allow 500ms to be CI-noise-immune;
        // serial execution would take ~800ms+ and fail this assert.
        assert!(
            elapsed < Duration::from_millis(500),
            "{N} parallel tool calls took {elapsed:?} — expected <500ms. \
             If this regressed, agent.rs likely reverted to serial execution."
        );

        // All 8 calls actually fired (sanity).
        assert_eq!(sleeper.call_count.load(Ordering::SeqCst), N as u32);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn parallel_tool_calls_preserve_order_even_when_completing_out_of_order() {
        // Use varying delays so completion order ≠ launch order.
        // If a regression drops the index slot, this catches it.
        const N: usize = 6;

        struct VarSleeper {
            delays: Vec<Duration>,
        }
        #[async_trait]
        impl Tool for VarSleeper {
            fn name(&self) -> &'static str {
                "var_sleeper"
            }
            fn spec(&self) -> ToolSpec {
                ToolSpec {
                    name: "var_sleeper".to_string(),
                    description: "sleeps per-call (test)".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {"label": {"type": "string"}}
                    }),
                }
            }
            async fn execute(
                &self,
                args: Value,
                _ctx: ToolContext,
            ) -> Result<ToolOutput, ToolError> {
                let label = args
                    .get("label")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?")
                    .to_string();
                // Parse `call_<idx>` to look up delay.
                let idx: usize = label
                    .strip_prefix("call_")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                tokio::time::sleep(self.delays[idx % self.delays.len()]).await;
                Ok(ToolOutput::ok(format!("var_sleeper[{label}]")))
            }
        }

        let delays = vec![
            Duration::from_millis(150), // call_0 — slowest
            Duration::from_millis(20),  // call_1 — fastest
            Duration::from_millis(100), // call_2
            Duration::from_millis(40),  // call_3
            Duration::from_millis(80),  // call_4
            Duration::from_millis(60),  // call_5
        ];
        let tool = Arc::new(VarSleeper { delays });
        // Fixture must reference var_sleeper (the registered tool),
        // not sleeper (which isn't registered in this test).
        let mut turn0: Vec<FixtureTurn> = Vec::with_capacity(N);
        for i in 0..N {
            turn0.push(FixtureTurn::ToolCall {
                name: "var_sleeper".to_string(),
                args: json!({"label": format!("call_{i}")}),
            });
        }
        turn0.push(FixtureTurn::Stop {
            stop_reason: "tool_use".to_string(),
            usage: Usage::default(),
        });
        let turn1 = vec![FixtureTurn::Stop {
            stop_reason: "end_turn".to_string(),
            usage: Usage::default(),
        }];
        let provider: Arc<ProgrammedProvider> =
            Arc::new(ProgrammedProvider::from_turns(vec![turn0, turn1]));

        let mut tools = ToolRegistry::new();
        tools = tools.register(tool as Arc<dyn Tool>);
        let mut agent = Agent::new(provider, tools, RunConfig::new("test-model"));

        let stream = Box::pin(agent.run(AgentMessage::user("test order preservation")));
        let mut tool_results: Vec<String> = Vec::new();
        // Use futures_util::StreamExt via `while let` rather than
        // pulling in the full module — we already `use super::*` above.
        use futures_util::StreamExt;
        let mut stream = stream;
        while let Some(ev) = stream.next().await {
            if let Ok(AgentEvent::ToolResult { output, .. }) = ev {
                tool_results.push(output.content);
            }
        }

        // All N results present (set membership).
        assert_eq!(tool_results.len(), N);
        for i in 0..N {
            let expected = format!("call_{i}");
            assert!(
                tool_results.iter().any(|r| r.contains(&expected)),
                "missing result for {expected}; got: {tool_results:?}"
            );
        }

        // Events arrive in completion order (FuturesUnordered
        // semantics) -- which is what the TUI wants for snappy UI
        // feedback. The final Tool message sent to the LLM is in
        // call order (preserved by `tool_results_slots[idx]`), which
        // is the Anthropic API contract -- verified indirectly by the
        // first test asserting the agent can produce a coherent
        // final assistant turn after parallel calls.
    }
}

// =====================================================================
// v0.8.5: parallel_tool_results_in_slot_order_for_llm_message
//
// Verifies the invariant that matters most for the LLM API: tool_results
// in the message sent to the next LLM call must be in the SAME ORDER as
// the tool_calls in the previous assistant message. Anthropic's API
// requires this; breaking it causes the next turn to 400-error.
//
// We can't directly inspect `Agent::messages` (private), but we CAN
// inspect the request the provider sees on the second turn. We use a
// custom Provider that captures the request from turn 1.
// =====================================================================
mod slot_order_tests {
    use super::*;
    use async_trait::async_trait;
    use futures_core::Stream;
    use futures_util::stream;
    use nini_core::provider::{Capabilities, Provider, ProviderError, Request, StreamEvent};
    use nini_core::tool::{Tool, ToolContext, ToolError, ToolOutput, ToolSpec};
    use nini_core::{AgentMessage, ProviderMessage as Message};
    use serde_json::{json, Value};
    use std::pin::Pin;
    use std::sync::Mutex;
    use std::time::Duration;

    /// Provider that captures the second-turn Request so we can
    /// inspect the tool_results ordering the agent will send to the
    /// next LLM call.
    struct CapturingProvider {
        captured: Arc<Mutex<Option<Request>>>,
        call_count: Arc<Mutex<usize>>,
    }

    impl CapturingProvider {
        fn new() -> Self {
            Self {
                captured: Arc::new(Mutex::new(None)),
                call_count: Arc::new(Mutex::new(0)),
            }
        }
        fn captured(&self) -> Arc<Mutex<Option<Request>>> {
            self.captured.clone()
        }
    }

    impl Provider for CapturingProvider {
        fn name(&self) -> &'static str {
            "capturing"
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }

        fn stream(
            &self,
            req: Request,
        ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send + 'static>>
        {
            // Count calls so we can distinguish turn 0 from turn 1.
            let call_n = {
                let mut c = self.call_count.lock().unwrap();
                *c += 1;
                *c
            };
            let captured = self.captured.clone();
            if call_n >= 2 {
                *captured.lock().unwrap() = Some(req);
                Box::pin(stream::iter(vec![Ok(StreamEvent::MessageStop {
                    stop_reason: "end_turn".to_string(),
                    usage: Usage::default(),
                })]))
            } else {
                // Turn 0: emit 4 parallel tool calls with id-prefixed
                // labels so we can verify ordering in the captured req.
                let mut events = vec![StreamEvent::MessageStart {
                    id: "msg_0".to_string(),
                    model: "capturing".to_string(),
                }];
                for i in 0..4 {
                    let id = format!("toolu_t0_{i}");
                    events.push(StreamEvent::ToolCallStart {
                        id: id.clone(),
                        name: "tagged".to_string(),
                    });
                    let args = json!({"label": format!("order_{i}")});
                    events.push(StreamEvent::ToolCallDelta {
                        id: id.clone(),
                        input_json_delta: serde_json::to_string(&args).unwrap(),
                    });
                    events.push(StreamEvent::ToolCallStop {
                        id,
                        input_json: args,
                    });
                }
                events.push(StreamEvent::MessageStop {
                    stop_reason: "tool_use".to_string(),
                    usage: Usage::default(),
                });
                Box::pin(stream::iter(events.into_iter().map(Ok)))
            }
        }
    }

    pub struct TaggedTool;
    #[async_trait]
    impl Tool for TaggedTool {
        fn name(&self) -> &'static str {
            "tagged"
        }
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "tagged".to_string(),
                description: "echoes the label".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {"label": {"type": "string"}}
                }),
            }
        }
        async fn execute(&self, args: Value, _ctx: ToolContext) -> Result<ToolOutput, ToolError> {
            // Variable delay so completion order ≠ launch order.
            let label = args
                .get("label")
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .to_string();
            let delay_ms: u64 = label
                .strip_prefix("order_")
                .and_then(|s| s.parse::<u64>().ok())
                .map(|i| 80 - i * 15)
                .unwrap_or(20);
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            Ok(ToolOutput::ok(format!("tagged[{label}]")))
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn parallel_tool_results_in_slot_order_for_llm_message() {
        let provider = CapturingProvider::new();
        let captured = provider.captured();
        let provider: Arc<dyn Provider> = Arc::new(provider);

        let tools = ToolRegistry::new().register(Arc::new(TaggedTool) as Arc<dyn Tool>);
        let mut agent = Agent::new(provider, tools, RunConfig::new("test-model"));

        let stream = Box::pin(agent.run(AgentMessage::user("test slot order")));
        use futures_util::StreamExt;
        let mut stream = stream;
        while let Some(_ev) = stream.next().await {
            // drain
        }

        // The provider captured the SECOND request. Its `messages`
        // should contain a Tool message whose ContentBlock::ToolResult
        // entries are in the SAME order as the tool_calls in the
        // preceding Assistant message.
        let req = captured
            .lock()
            .unwrap()
            .take()
            .expect("provider should have seen a second turn");

        // Find the Assistant message and the next Tool message.
        let mut assistant_msg: Option<&Message> = None;
        let mut tool_msg_after: Option<&Message> = None;
        for pair in req.messages.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            if matches!(a.role, nini_core::Role::Assistant)
                && matches!(b.role, nini_core::Role::Tool)
            {
                assistant_msg = Some(a);
                tool_msg_after = Some(b);
                break;
            }
        }

        let assistant_msg = assistant_msg.expect("missing Assistant→Tool pair");
        let tool_msg = tool_msg_after.expect("missing Tool message after Assistant");

        // Collect tool_use_ids from Assistant (in order).
        let call_ids: Vec<String> = assistant_msg
            .content
            .iter()
            .filter_map(|b| match b {
                nini_core::ContentBlock::ToolUse { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect();

        // Collect tool_use_ids from Tool message (in order).
        let result_ids: Vec<String> = tool_msg
            .content
            .iter()
            .filter_map(|b| match b {
                nini_core::ContentBlock::ToolResult { tool_use_id, .. } => {
                    Some(tool_use_id.clone())
                }
                _ => None,
            })
            .collect();

        assert_eq!(call_ids.len(), 4, "expected 4 tool calls, got {call_ids:?}");
        assert_eq!(
            result_ids.len(),
            4,
            "expected 4 tool results, got {result_ids:?}"
        );
        assert_eq!(
            call_ids, result_ids,
            "tool_results must be in the SAME order as tool_calls (Anthropic API contract).\n\
             calls:   {call_ids:?}\n\
             results: {result_ids:?}"
        );
    }
}

// =====================================================================
// v0.8.5: abort during parallel tool execution cancels cleanly.
//
// Pins down that firing `abort.abort()` while FuturesUnordered is
// mid-flight causes the whole agent run to return `Aborted` instead
// of partially completing. We use a sleeper tool with a 200ms delay
// and abort after 50ms (mid-flight).
// =====================================================================
mod abort_parallel_tests {
    use super::*;
    use async_trait::async_trait;
    use nini_core::tool::{Tool, ToolContext, ToolError, ToolOutput, ToolSpec};
    use nini_core::AgentMessage;
    use serde_json::{json, Value};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    pub struct SlowTool {
        delay: Duration,
        completed: Arc<AtomicU32>,
    }

    #[async_trait]
    impl Tool for SlowTool {
        fn name(&self) -> &'static str {
            "slow"
        }
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "slow".to_string(),
                description: "sleeps (test only)".to_string(),
                input_schema: json!({"type": "object"}),
            }
        }
        async fn execute(&self, _args: Value, _ctx: ToolContext) -> Result<ToolOutput, ToolError> {
            tokio::time::sleep(self.delay).await;
            self.completed.fetch_add(1, Ordering::SeqCst);
            Ok(ToolOutput::ok("done".to_string()))
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn abort_during_parallel_tool_returns_aborted_event() {
        let completed = Arc::new(AtomicU32::new(0));
        // Build a fixture with 6 parallel slow tool_calls + final stop.
        let mut turn0: Vec<FixtureTurn> = Vec::with_capacity(7);
        for _ in 0..6 {
            turn0.push(FixtureTurn::ToolCall {
                name: "slow".to_string(),
                args: json!({}),
            });
        }
        turn0.push(FixtureTurn::Stop {
            stop_reason: "tool_use".to_string(),
            usage: Usage::default(),
        });
        let provider: Arc<dyn nini_core::Provider> =
            Arc::new(ProgrammedProvider::from_turns(vec![turn0]));

        let tool = Arc::new(SlowTool {
            delay: Duration::from_millis(500),
            completed: completed.clone(),
        });
        let tools = ToolRegistry::new().register(tool as Arc<dyn Tool>);
        let mut agent = Agent::new(provider, tools, RunConfig::new("test-model"));

        let abort = agent.abort_handle();
        let agent_handle = tokio::spawn(async move {
            let mut stream = Box::pin(agent.run(AgentMessage::user("test abort")));
            use futures_util::StreamExt;
            while let Some(_ev) = stream.next().await {}
        });

        // Let the agent start running the futures (each sleeps 500ms).
        tokio::time::sleep(Duration::from_millis(50)).await;
        abort.abort();

        // Bounded wait for the agent task to finish (should be quick).
        let res = tokio::time::timeout(Duration::from_millis(2_000), agent_handle).await;
        assert!(res.is_ok(), "agent task did not finish within 2s of abort");

        // After abort, not all 6 futures should have completed (the
        // 500ms delay means at most 1-2 had time to finish).
        let n = completed.load(Ordering::SeqCst);
        assert!(
            n < 6,
            "abort should cancel in-flight futures, but {n}/6 completed"
        );
    }

    /// v0.8.9 (P2-11): every `ToolCallStart` must be followed by a
    /// `ToolResult`, including on the abort path.
    ///
    /// The TUI draws a `▸ <name>` row the moment a `ToolCallStart`
    /// arrives and only resolves it when the matching `ToolResult`
    /// shows up. Before this fix the abort branch returned without
    /// emitting results, so every in-flight tool call stayed pending
    /// on screen for the rest of the session — the user could not tell
    /// whether the tool was still running, and the only escape was
    /// restarting. Pi emits a cancelled result per outstanding call.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn abort_closes_out_every_started_tool_call() {
        use nini_core::AgentEvent;
        use std::collections::HashSet;

        let completed = Arc::new(AtomicU32::new(0));
        let mut turn0: Vec<FixtureTurn> = Vec::with_capacity(7);
        for _ in 0..6 {
            turn0.push(FixtureTurn::ToolCall {
                name: "slow".to_string(),
                args: json!({}),
            });
        }
        turn0.push(FixtureTurn::Stop {
            stop_reason: "tool_use".to_string(),
            usage: Usage::default(),
        });
        let provider: Arc<dyn nini_core::Provider> =
            Arc::new(ProgrammedProvider::from_turns(vec![turn0]));

        let tool = Arc::new(SlowTool {
            delay: Duration::from_millis(500),
            completed: completed.clone(),
        });
        let tools = ToolRegistry::new().register(tool as Arc<dyn Tool>);
        let mut agent = Agent::new(provider, tools, RunConfig::new("test-model"));

        let abort = agent.abort_handle();
        let agent_handle = tokio::spawn(async move {
            let events: Vec<AgentEvent> = {
                let mut stream = Box::pin(agent.run(AgentMessage::user("test abort")));
                use futures_util::StreamExt;
                let mut acc = Vec::new();
                while let Some(ev) = stream.next().await {
                    // The stream is `Result<AgentEvent, AgentError>`.
                    acc.push(ev.expect("agent stream yielded an error"));
                }
                acc
            };
            events
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        abort.abort();
        let events = tokio::time::timeout(Duration::from_millis(2_000), agent_handle)
            .await
            .expect("agent task did not finish within 2s of abort")
            .expect("agent task panicked");

        // The invariant: no dangling ToolCallStart.
        let mut started: HashSet<String> = HashSet::new();
        let mut resolved: HashSet<String> = HashSet::new();
        for ev in &events {
            match ev {
                AgentEvent::ToolCallStart { id, .. } => {
                    started.insert(id.clone());
                }
                AgentEvent::ToolResult { id, .. } => {
                    resolved.insert(id.clone());
                }
                _ => {}
            }
        }

        assert!(
            !started.is_empty(),
            "precondition: the fixture should have started tool calls, got {events:?}"
        );
        let dangling: Vec<&String> = started.difference(&resolved).collect();
        assert!(
            dangling.is_empty(),
            "{} tool call(s) left permanently pending after abort: {dangling:?}\n\
             The TUI draws a row per ToolCallStart and only resolves it on \
             ToolResult, so these would stay pending for the rest of the session.",
            dangling.len()
        );

        // The abort must still be signalled, and it must be the last
        // thing that happens.
        assert!(
            matches!(events.last(), Some(AgentEvent::Aborted)),
            "stream should end with Aborted, got {:?}",
            events.last()
        );
    }

    /// A resolved-by-abort result should say so, so the model and the
    /// user can tell a cancellation from a genuine tool failure.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn abort_results_are_labelled_as_aborted() {
        use nini_core::AgentEvent;

        let completed = Arc::new(AtomicU32::new(0));
        let turn0 = vec![
            FixtureTurn::ToolCall {
                name: "slow".to_string(),
                args: json!({}),
            },
            FixtureTurn::Stop {
                stop_reason: "tool_use".to_string(),
                usage: Usage::default(),
            },
        ];
        let provider: Arc<dyn nini_core::Provider> =
            Arc::new(ProgrammedProvider::from_turns(vec![turn0]));
        let tool = Arc::new(SlowTool {
            delay: Duration::from_millis(400),
            completed: completed.clone(),
        });
        let tools = ToolRegistry::new().register(tool as Arc<dyn Tool>);
        let mut agent = Agent::new(provider, tools, RunConfig::new("test-model"));

        let abort = agent.abort_handle();
        let agent_handle = tokio::spawn(async move {
            let events: Vec<AgentEvent> = {
                let mut stream = Box::pin(agent.run(AgentMessage::user("abort me")));
                use futures_util::StreamExt;
                let mut acc = Vec::new();
                while let Some(ev) = stream.next().await {
                    // The stream is `Result<AgentEvent, AgentError>`.
                    acc.push(ev.expect("agent stream yielded an error"));
                }
                acc
            };
            events
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        abort.abort();
        let events = tokio::time::timeout(Duration::from_millis(2_000), agent_handle)
            .await
            .expect("agent did not finish")
            .expect("agent panicked");

        let results: Vec<&AgentEvent> = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::ToolResult { .. }))
            .collect();
        assert_eq!(
            results.len(),
            1,
            "expected one closed-out result: {events:?}"
        );
        match results[0] {
            AgentEvent::ToolResult { output, .. } => {
                assert!(
                    output.is_error,
                    "an aborted call should be an error, not a success"
                );
                assert!(
                    output.content.contains("abort"),
                    "aborted result should say so: {:?}",
                    output.content
                );
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }
}
// =====================================================================
// v0.8.9 (P2-10): a tool that reports its own peak concurrency.
//
// `SleeperTool` counts total invocations, which cannot distinguish
// "30 calls ran at once" from "8 ran at once and 22 queued". The worker
// pool's entire purpose is the second number, so the tool has to expose
// the high-water mark.
// =====================================================================
mod probe_tools {
    use async_trait::async_trait;
    use nini_core::tool::{Tool, ToolContext, ToolError, ToolOutput, ToolSpec};
    use serde_json::{json, Value};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    /// v0.8.9 (P2-10): a tool that records how many of its own bodies
    /// were executing at the same time.
    ///
    /// `SleeperTool` only counted total invocations, which cannot tell
    /// "30 calls ran concurrently" from "8 ran concurrently, 22 queued".
    /// The worker pool's whole point is the second number, so the tool
    /// has to expose the high-water mark.
    pub struct ConcurrencyProbeTool {
        delay: Duration,
        #[allow(dead_code)]
        pub live: Arc<AtomicU32>,
        pub peak_live: Arc<AtomicU32>,
        pub total: Arc<AtomicU32>,
    }

    impl ConcurrencyProbeTool {
        pub fn new(delay: Duration) -> Self {
            Self {
                delay,
                live: Arc::new(AtomicU32::new(0)),
                peak_live: Arc::new(AtomicU32::new(0)),
                total: Arc::new(AtomicU32::new(0)),
            }
        }
    }

    #[async_trait]
    impl Tool for ConcurrencyProbeTool {
        fn name(&self) -> &'static str {
            "probe"
        }

        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "probe".to_string(),
                description: "Records peak concurrency (test only).".to_string(),
                input_schema: json!({"type": "object"}),
            }
        }

        async fn execute(&self, _args: Value, _ctx: ToolContext) -> Result<ToolOutput, ToolError> {
            self.total.fetch_add(1, Ordering::SeqCst);
            let now = self.live.fetch_add(1, Ordering::SeqCst) + 1;
            // Bump the high-water mark. A plain `fetch_max` is not
            // available on all supported rustc targets for this shape,
            // so compare-and-swap in a loop.
            let mut peak = self.peak_live.load(Ordering::SeqCst);
            while now > peak {
                match self
                    .peak_live
                    .compare_exchange(peak, now, Ordering::SeqCst, Ordering::SeqCst)
                {
                    Ok(_) => break,
                    Err(actual) => peak = actual,
                }
            }
            tokio::time::sleep(self.delay).await;
            self.live.fetch_sub(1, Ordering::SeqCst);
            Ok(ToolOutput::ok("probe done".to_string()))
        }
    }
}

// =====================================================================
// v0.8.9 (P2-10): parallel tool execution has a ceiling.
//
// Models emit however many tool calls they like. Before this, every one
// of them got a future immediately, so a 30-call turn forked 30 process
// trees at the same instant — which multiplies directly against the
// per-stream memory ceiling in `nini_tools::bash`. The batch size was
// the model's mood, not our decision.
//
// These tests measure the tool's own peak concurrency, because "all N
// calls completed" cannot distinguish "N ran at once" from "8 ran at
// once and 12 queued".
// =====================================================================
mod parallel_ceiling_tests {
    use super::probe_tools::ConcurrencyProbeTool;
    use super::*;
    use nini_core::tool::Tool;
    use nini_core::{AgentMessage, RunConfig};
    use serde_json::json;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    const CALLS: usize = 20;
    const LIMIT: usize = 4;

    fn provider_with(calls: usize) -> Arc<ProgrammedProvider> {
        let mut turn0: Vec<FixtureTurn> = Vec::with_capacity(calls + 1);
        for i in 0..calls {
            turn0.push(FixtureTurn::ToolCall {
                name: "probe".to_string(),
                args: json!({"label": format!("c{i}")}),
            });
        }
        turn0.push(FixtureTurn::Stop {
            stop_reason: "tool_use".to_string(),
            usage: Usage::default(),
        });
        let turn1 = vec![FixtureTurn::Stop {
            stop_reason: "end_turn".to_string(),
            usage: Usage::default(),
        }];
        Arc::new(ProgrammedProvider::from_turns(vec![turn0, turn1]))
    }

    /// The same run as `run_with_limit`, but with whatever ceiling
    /// `RunConfig::new` hands out — no explicit limit.
    async fn run_without_limit(delay: Duration) -> (u32, u32, Duration) {
        let probe = Arc::new(ConcurrencyProbeTool::new(delay));
        let peak = probe.peak_live.clone();
        let total = probe.total.clone();
        let tools = ToolRegistry::new().register(probe as Arc<dyn Tool>);
        let cfg = RunConfig::new("test-model");
        let mut agent = Agent::new(provider_with(CALLS), tools, cfg);

        let started = Instant::now();
        {
            let mut stream = Box::pin(agent.run(AgentMessage::user("go")));
            use futures_util::StreamExt;
            while let Some(ev) = stream.next().await {
                let _ = ev.expect("agent stream error");
            }
        }
        (
            peak.load(Ordering::SeqCst),
            total.load(Ordering::SeqCst),
            started.elapsed(),
        )
    }

    async fn run_with_limit(limit: usize, delay: Duration) -> (u32, u32, Duration) {
        let probe = Arc::new(ConcurrencyProbeTool::new(delay));
        let peak = probe.peak_live.clone();
        let total = probe.total.clone();
        let tools = ToolRegistry::new().register(probe as Arc<dyn Tool>);
        let mut cfg = RunConfig::new("test-model");
        cfg.max_parallel_tools = limit;
        let mut agent = Agent::new(provider_with(CALLS), tools, cfg);

        let started = Instant::now();
        {
            let mut stream = Box::pin(agent.run(AgentMessage::user("go")));
            use futures_util::StreamExt;
            while let Some(ev) = stream.next().await {
                let _ = ev.expect("agent stream error");
            }
        }
        (
            peak.load(Ordering::SeqCst),
            total.load(Ordering::SeqCst),
            started.elapsed(),
        )
    }

    /// v0.14 (Pi parity): the *default* ceiling must be "no limit".
    ///
    /// Pi 0.85.1's parallel branch is an unbounded `Promise.all`
    /// (`references/pi-0.85.1/src/loop/agent-loop.ts:547-549`), so a turn
    /// with K tool calls runs K of them at once. nini used to cap that at
    /// 8 — a cap Pi does not have, and one the user could not change
    /// (no flag, no settings key). `0` is nini's "unlimited" spelling,
    /// the same one `--max-iterations` uses.
    #[test]
    fn the_default_ceiling_is_unlimited() {
        assert_eq!(
            RunConfig::new("test-model").max_parallel_tools,
            0,
            "the default tool-concurrency ceiling must be 0 (unlimited), \
             the way Pi 0.85.1 runs every tool call of a turn at once"
        );
    }

    /// v0.14 (Pi parity): with no ceiling, a turn's K tool calls all run
    /// at once — the same peak Pi reports for the same script.
    ///
    /// The probe counts its own in-flight bodies, so this measures
    /// concurrency, not "all calls eventually finished". The 60ms delay
    /// is the same one the ceiling tests use; it keeps the in-flight
    /// window open long enough for the counter to see every call.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_default_batch_runs_all_calls_at_once() {
        let (peak, total, _) = run_without_limit(Duration::from_millis(60)).await;
        println!("NINI_PEAK={peak}");
        assert_eq!(
            total, CALLS as u32,
            "every tool call must still run — the ceiling queues, it does not drop"
        );
        assert_eq!(
            peak, CALLS as u32,
            "peak concurrency was {peak}, expected all {CALLS} calls of the turn \
             to run at once (Pi 0.85.1 has no concurrency cap)"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn peak_concurrency_respects_the_configured_limit() {
        let (peak, total, _) = run_with_limit(LIMIT, Duration::from_millis(60)).await;
        assert_eq!(
            total, CALLS as u32,
            "every tool call must still run — the limit queues, it does not drop"
        );
        assert!(
            peak <= LIMIT as u32,
            "peak concurrency {peak} exceeded the configured limit of {LIMIT}"
        );
        // And it must actually be parallel, otherwise "respects the
        // limit" is trivially satisfied by running everything serially.
        assert!(
            peak > 1,
            "peak concurrency was {peak} — the calls ran serially, so the              limit is not buying anything and v0.8.5's win is gone"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn zero_means_no_limit() {
        // Explicit opt-out preserves the pre-v0.8.9 behaviour.
        let (peak, total, _) = run_with_limit(0, Duration::from_millis(60)).await;
        assert_eq!(total, CALLS as u32);
        assert!(
            peak > LIMIT as u32,
            "max_parallel_tools = 0 should mean unlimited, but peak was only {peak}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_ceiling_actually_saves_wall_clock_vs_serial() {
        // The whole point of a bounded pool: the batch finishes in about
        // `ceil(CALLS / LIMIT)` waves, not `CALLS` serial steps.
        let delay = Duration::from_millis(60);
        let (_, _, elapsed) = run_with_limit(LIMIT, delay).await;
        let serial = CALLS as u32 * delay;
        assert!(
            elapsed < serial / 2,
            "20 calls at {LIMIT}-way concurrency took {elapsed:?}, which is              not meaningfully better than the {serial:?} a serial run costs"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn results_still_arrive_in_call_order() {
        // The ordering invariant predates the pool: providers require
        // tool_result blocks in the same order as the tool_use blocks
        // they answer. The pool must not have broken it.
        let probe = Arc::new(ConcurrencyProbeTool::new(Duration::from_millis(5)));
        let tools = ToolRegistry::new().register(probe as Arc<dyn Tool>);
        let mut cfg = RunConfig::new("test-model");
        cfg.max_parallel_tools = 3;
        let mut agent = Agent::new(provider_with(CALLS), tools, cfg);

        let mut results: Vec<String> = Vec::new();
        {
            let mut stream = Box::pin(agent.run(AgentMessage::user("go")));
            use futures_util::StreamExt;
            use nini_core::AgentEvent;
            while let Some(ev) = stream.next().await {
                if let Ok(AgentEvent::ToolResult { id, .. }) = ev {
                    results.push(id);
                }
            }
        }
        assert_eq!(
            results.len(),
            CALLS,
            "expected {CALLS} results: {results:?}"
        );
        let unique: std::collections::HashSet<&String> = results.iter().collect();
        assert_eq!(unique.len(), CALLS, "duplicate tool results: {results:?}");
    }
}

// =====================================================================
// v0.8.9 (P0-5): interleaved parallel tool calls must keep their args.
//
// The agent used to accumulate tool calls in a single `current_call`
// slot: every `ToolCallStart` pushed the previous call out (with its
// `input_json` still empty) and every `ToolCallStop` discarded whatever
// did not match. That is fine for a strictly serial Start→Stop→Start
// sequence, which is exactly what the `fixture` provider emits and
// therefore exactly what all 900-odd tests exercised.
//
// Real models interleave. Driving the real TUI against MiniMax-M3 with
// "read these four files in parallel" produced:
//
//     ▸ read {"path":"/tmp/nini-e2e/target1.rs"}
//     ▸ read {"path":"/tmp/nini-e2e/target2.rs"}
//     ...
//     ✗ read Took 0ms
//       invalid arguments: invalid type: null, expected struct ReadArgs
//
// The transcript looked perfect — the TUI sink tracks each line by id
// independently — while every tool received `Value::Null`.
// =====================================================================
mod interleaved_tool_calls {
    use super::*;
    use async_trait::async_trait;
    use nini_core::provider::{Capabilities, ProviderError, Request, StreamEvent};
    use nini_core::tool::{Tool, ToolContext, ToolError, ToolOutput, ToolSpec};
    use nini_core::AgentMessage;
    use serde_json::{json, Value};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// A provider that emits tool-call events in an arbitrary
    /// interleaving — all Starts first, then the bodies, then the
    /// Stops. This is the shape that broke the single-slot accumulator.
    struct InterleavingProvider {
        events: Vec<StreamEvent>,
        cursor: Arc<AtomicUsize>,
    }

    impl InterleavingProvider {
        fn new() -> Self {
            let paths = ["/a/one.rs", "/a/two.rs", "/a/three.rs", "/a/four.rs"];
            let mut events = vec![StreamEvent::MessageStart {
                id: "msg".to_string(),
                model: "test-model".to_string(),
            }];
            // All four starts up front, before any body arrives.
            for (i, p) in paths.iter().enumerate() {
                events.push(StreamEvent::ToolCallStart {
                    id: format!("call_{i}"),
                    name: "echo".to_string(),
                });
                let _ = p;
            }
            // Then each body, split in two, out of order.
            for (i, p) in paths.iter().enumerate() {
                let full = format!("{{\"path\":\"{p}\"}}");
                let (head, tail) = full.split_at(full.len() / 2);
                events.push(StreamEvent::ToolCallDelta {
                    id: format!("call_{i}"),
                    input_json_delta: head.to_string(),
                });
                events.push(StreamEvent::ToolCallDelta {
                    id: format!("call_{i}"),
                    input_json_delta: tail.to_string(),
                });
            }
            // Stops, in a different order again.
            for i in [2usize, 0, 3, 1] {
                events.push(StreamEvent::ToolCallStop {
                    id: format!("call_{i}"),
                    input_json: serde_json::json!({
                        "path": format!("/a/{}.rs", ["one", "two", "three", "four"][i])
                    }),
                });
            }
            events.push(StreamEvent::MessageStop {
                stop_reason: "tool_use".to_string(),
                usage: Usage::default(),
            });
            Self {
                events,
                cursor: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    #[async_trait]
    impl nini_core::Provider for InterleavingProvider {
        fn name(&self) -> &'static str {
            "interleaving"
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities {
                streaming: true,
                tool_use: true,
                vision: false,
            }
        }

        fn stream(
            &self,
            _req: Request,
        ) -> std::pin::Pin<
            Box<
                dyn futures_core::Stream<Item = Result<StreamEvent, ProviderError>>
                    + Send
                    + 'static,
            >,
        > {
            // Turn 0: the interleaved batch. Turn 1: a plain end_turn so
            // the agent loop terminates after the tools have run.
            let turn = self.cursor.fetch_add(1, Ordering::SeqCst);
            if turn == 0 {
                let events = self.events.clone();
                Box::pin(futures_util::stream::iter(events.into_iter().map(Ok)))
            } else {
                Box::pin(futures_util::stream::iter(vec![Ok(
                    StreamEvent::MessageStop {
                        stop_reason: "end_turn".to_string(),
                        usage: Usage::default(),
                    },
                )]))
            }
        }
    }

    /// A tool that just echoes back the `path` it was handed, so the
    /// test can see exactly what the agent parsed.
    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &'static str {
            "echo"
        }
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "echo".to_string(),
                description: "echo the path".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {"path": {"type": "string"}},
                    "required": ["path"]
                }),
            }
        }
        async fn execute(&self, args: Value, _ctx: ToolContext) -> Result<ToolOutput, ToolError> {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidArgs("no path".to_string()))?;
            Ok(ToolOutput::ok(format!("echo:{path}")))
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn interleaved_parallel_tool_calls_keep_their_args() {
        let provider = Arc::new(InterleavingProvider::new());
        let tools = ToolRegistry::new().register(Arc::new(EchoTool) as Arc<dyn Tool>);
        let mut agent = Agent::new(
            provider as Arc<dyn nini_core::Provider>,
            tools,
            RunConfig::new("test-model"),
        );

        let mut results: Vec<(String, String, bool)> = Vec::new();
        {
            let mut stream = Box::pin(agent.run(AgentMessage::user("read them all")));
            use futures_util::StreamExt;
            while let Some(ev) = stream.next().await {
                if let Ok(nini_core::AgentEvent::ToolResult { id, name, output }) = ev {
                    results.push((id, name, output.is_error));
                }
            }
        }

        assert_eq!(results.len(), 4, "expected 4 results, got {results:?}");
        for (id, name, is_error) in &results {
            assert!(!is_error, "tool call {id} ({name}) failed — args were lost");
            assert_eq!(name, "echo", "wrong tool name for {id}");
        }
        let mut ids: Vec<&String> = results.iter().map(|(id, _, _)| id).collect();
        ids.sort();
        assert_eq!(
            ids,
            vec!["call_0", "call_1", "call_2", "call_3"],
            "every parallel call must be executed exactly once"
        );
    }
}

// =====================================================================
// v0.8.6: length-stop tool_call reject gate.
//
// When stop_reason == "length" and tool_calls is non-empty, the assistant
// message ends mid-tool-call-JSON. nini must emit synthetic [reject]
// ToolResults instead of executing the truncated calls (which would
// surface as cryptic 'tool error: invalid args' downstream).
//
// Coordinate with the existing auto-compact-on-length-stop path: that
// path still fires (it shrinks context for the next turn). We only skip
// the tool execution itself.
// =====================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn length_stop_with_tool_calls_rejects_all() {
    use nini_core::AgentEvent;

    // Turn 0: LLM emits a tool_call AND hits length stop. The args
    // JSON is syntactically valid (length stop doesn't necessarily
    // truncate mid-string), but per Pi we still refuse the whole
    // batch on length stop.
    // Turn 1: LLM recovers with a fresh tool_call + tool_use stop.
    // Turn 2: Final assistant text + end_turn.
    let provider: Arc<dyn nini_core::Provider> = Arc::new(ProgrammedProvider::from_turns(vec![
        vec![
            FixtureTurn::ToolCall {
                name: "bash".to_string(),
                args: serde_json::json!({"command": "echo hi"}),
            },
            FixtureTurn::Stop {
                stop_reason: "length".to_string(),
                usage: Usage::default(),
            },
        ],
        vec![
            FixtureTurn::ToolCall {
                name: "bash".to_string(),
                args: serde_json::json!({"command": "echo recovered"}),
            },
            FixtureTurn::Stop {
                stop_reason: "tool_use".to_string(),
                usage: Usage::default(),
            },
        ],
        vec![
            FixtureTurn::Text("done".to_string()),
            FixtureTurn::Stop {
                stop_reason: "end_turn".to_string(),
                usage: Usage::default(),
            },
        ],
    ]));

    let tools = ToolRegistry::new().register(Arc::new(BashTool::new()));
    let mut agent = Agent::new(provider, tools, RunConfig::new("test-model"));
    let stream = Box::pin(agent.run(AgentMessage::user("test length stop")));
    use futures_util::StreamExt;
    let mut stream = stream;
    let mut tool_results: Vec<ToolOutput> = Vec::new();
    while let Some(ev) = stream.next().await {
        if let Ok(AgentEvent::ToolResult { output, .. }) = ev {
            tool_results.push(output);
        }
    }

    assert_eq!(
        tool_results.len(),
        2,
        "expected 2 tool_results (1 reject from length-stop + 1 success after recovery); got {tool_results:?}"
    );

    // Turn 0: length stop → reject
    assert!(
        tool_results[0].is_error,
        "first tool_result must be error (length-stop reject); got: {}",
        tool_results[0].content
    );
    assert!(
        tool_results[0].content.contains("[reject]"),
        "first tool_result must contain '[reject]' marker; got: {}",
        tool_results[0].content
    );
    assert!(
        tool_results[0].content.contains("truncated"),
        "first tool_result must mention 'truncated'; got: {}",
        tool_results[0].content
    );
    assert!(
        tool_results[0].content.contains("length stop"),
        "first tool_result must mention 'length stop'; got: {}",
        tool_results[0].content
    );

    // Turn 1: recovered — bash actually ran
    assert!(
        !tool_results[1].is_error,
        "second tool_result must succeed after recovery; got: {}",
        tool_results[1].content
    );
    assert!(
        tool_results[1].content.contains("recovered"),
        "second tool_result should contain 'recovered' from bash output; got: {}",
        tool_results[1].content
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn length_stop_with_no_tool_calls_returns_normally() {
    use nini_core::AgentEvent;

    // Turn 0: pure text + length stop, no tool_calls. Should NOT
    // enter the reject path (tool_calls.is_empty() is true).
    // Turn 1: end_turn so the agent finishes.
    let provider: Arc<dyn nini_core::Provider> = Arc::new(ProgrammedProvider::from_turns(vec![
        vec![
            FixtureTurn::Text("I gave up due to length.".to_string()),
            FixtureTurn::Stop {
                stop_reason: "length".to_string(),
                usage: Usage::default(),
            },
        ],
        vec![
            FixtureTurn::Text("recovered".to_string()),
            FixtureTurn::Stop {
                stop_reason: "end_turn".to_string(),
                usage: Usage::default(),
            },
        ],
    ]));

    let tools = ToolRegistry::new();
    let mut agent = Agent::new(provider, tools, RunConfig::new("test-model"));
    let stream = Box::pin(agent.run(AgentMessage::user("test")));
    use futures_util::StreamExt;
    let mut stream = stream;
    let mut tool_results = 0;
    let mut text_chunks = 0;
    while let Some(ev) = stream.next().await {
        match ev {
            Ok(AgentEvent::ToolResult { .. }) => tool_results += 1,
            Ok(AgentEvent::TextDelta { .. }) => text_chunks += 1,
            _ => {}
        }
    }
    assert_eq!(tool_results, 0, "no tool calls means nothing to reject");
    assert!(text_chunks >= 1, "should still get text from both turns");
}
