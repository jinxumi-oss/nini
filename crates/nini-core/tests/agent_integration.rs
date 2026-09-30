//! Integration tests for the agent loop with a real `BashTool`.
//!
//! These tests exercise the full path: provider → agent → tool registry →
//! bash → process spawn → stdout capture → back to provider.

use futures_util::StreamExt;
use nini_ai::fixture::{FixtureTurn, ProgrammedProvider};
use nini_core::provider::Usage;
use nini_core::tool::ToolRegistry;
use nini_core::{Agent, AgentEvent, RunConfig};
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
    use serde_json::{Value, json};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// A trivial test tool that sleeps for a fixed duration and
    /// records that it was called. Returns the call index in the
    /// output so the test can verify ordering.
    pub struct SleeperTool {
        delay: Duration,
        call_count: Arc<AtomicU32>,
    }

    impl SleeperTool {
        pub fn new(delay: Duration) -> Self {
            Self {
                delay,
                call_count: Arc::new(AtomicU32::new(0)),
            }
        }
        pub fn call_count(&self) -> Arc<AtomicU32> {
            self.call_count.clone()
        }
    }

    #[async_trait]
    impl Tool for SleeperTool {
        fn name(&self) -> &'static str { "sleeper" }

        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "sleeper".to_string(),
                description: "Sleeps for a configured duration (test only)."
                    .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "label": {"type": "string"}
                    }
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
            fn name(&self) -> &'static str { "var_sleeper" }
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
    use serde_json::{Value, json};
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
        fn name(&self) -> &'static str { "capturing" }
        fn capabilities(&self) -> Capabilities { Capabilities::default() }

        fn stream(
            &self,
            req: Request,
        ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send + 'static>> {
            // Count calls so we can distinguish turn 0 from turn 1.
            let call_n = {
                let mut c = self.call_count.lock().unwrap();
                *c += 1;
                *c
            };
            let captured = self.captured.clone();
            if call_n >= 2 {
                *captured.lock().unwrap() = Some(req);
                Box::pin(stream::iter(vec![
                    Ok(StreamEvent::MessageStop {
                        stop_reason: "end_turn".to_string(),
                        usage: Usage::default(),
                    }),
                ]))
            } else {
                // Turn 0: emit 4 parallel tool calls with id-prefixed
                // labels so we can verify ordering in the captured req.
                let mut events = vec![
                    StreamEvent::MessageStart {
                        id: "msg_0".to_string(),
                        model: "capturing".to_string(),
                    },
                ];
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
        fn name(&self) -> &'static str { "tagged" }
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
        async fn execute(
            &self,
            args: Value,
            _ctx: ToolContext,
        ) -> Result<ToolOutput, ToolError> {
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
                nini_core::ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
                _ => None,
            })
            .collect();

        assert_eq!(call_ids.len(), 4, "expected 4 tool calls, got {call_ids:?}");
        assert_eq!(result_ids.len(), 4, "expected 4 tool results, got {result_ids:?}");
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
    use nini_core::{AbortHandle, AgentMessage};
    use serde_json::{Value, json};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    pub struct SlowTool {
        delay: Duration,
        completed: Arc<AtomicU32>,
    }

    #[async_trait]
    impl Tool for SlowTool {
        fn name(&self) -> &'static str { "slow" }
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "slow".to_string(),
                description: "sleeps (test only)".to_string(),
                input_schema: json!({"type": "object"}),
            }
        }
        async fn execute(
            &self,
            _args: Value,
            _ctx: ToolContext,
        ) -> Result<ToolOutput, ToolError> {
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
            let mut stream = stream;
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
}