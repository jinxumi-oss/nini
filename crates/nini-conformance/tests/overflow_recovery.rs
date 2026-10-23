//! Oracle: nini's context-overflow recovery must behave like Pi 0.84.2.
//!
//! # What Pi actually does
//!
//! Verified against the reconstructed source of the installed version,
//! `pi-coding-agent@0.84.2`:
//!
//! ```text
//!  overflow.ts:134   isContextOverflow(message, contextWindow)
//!      Case 1  stopReason === "error" && OVERFLOW_PATTERNS.test(errorMessage)
//!              (after excluding NON_OVERFLOW_PATTERNS — throttling etc.)
//!      Case 2  stopReason === "stop"   && usage.input + cacheRead > contextWindow
//!      Case 3  stopReason === "length" && usage.output === 0
//!              && usage.input + cacheRead >= contextWindow * 0.99
//!
//!  agent-session.ts:2132  _checkCompaction(assistantMessage)
//!      Case 1 (recoverable) → _runAutoCompaction("overflow", willRetry)
//!      Case 2 (threshold)   → _runAutoCompaction("threshold", willRetry=false)
//!      — called UNCONDITIONALLY after every assistant message (:1098)
//!
//!  agent-session.ts:2172  if (this._overflowRecoveryAttempted) → emit the error
//!                          "Context overflow recovery failed after one
//!                           compact-and-retry attempt…" and STOP
//!  agent-session.ts#messages.slice(0, -1)  messages.slice(0, -1)  — drop the failed assistant
//!                          from the state that gets compacted and retried
//! ```
//!
//! # The defect this pins down
//!
//! nini had all three pieces of plumbing and none of them connected:
//!
//! - `agent.rs:956-975` removed the in-run threshold check entirely, so a
//!   long run compacts **zero** times between submissions. A run of 50 tool
//!   turns must cross the window; only the per-submission check at
//!   `runtime.rs:2102` exists.
//! - `agent.rs:1146` calls `is_context_overflow(&stop_reason, None, …)`, so
//!   the error-message branch of the detector — the only one that matches a
//!   real HTTP 400 — never runs. The provider reports the overflow as
//!   `StreamEvent::Error` (`nini-ai/src/openai.rs:418`), a different branch,
//!   which classifies it as non-retryable (`agent.rs:216`) and aborts the run.
//! - There is no `_overflowRecoveryAttempted` equivalent: `agent.rs:1179`
//!   `continue`s on a failed compaction with the *same* oversized context,
//!   which overflows again, until `max_iterations` (50) trips.

use futures_util::Stream;
use nini_core::agent::{Agent, CompactionHook, CompactionResult, RunConfig};
use nini_core::provider::{
    Capabilities, Provider, ProviderError, Request, StreamEvent, Usage as PUsage,
};
use nini_core::tool::{Tool, ToolContext, ToolOutput, ToolRegistry, ToolSpec};
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Provider script
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum Step {
    /// A turn that calls a tool and keeps the loop going.
    ToolTurn { input_tokens: u32 },
    /// A turn that ends cleanly.
    Finish,
    /// A provider-reported context overflow, exactly as
    /// `nini-ai/src/openai.rs` maps HTTP 400.
    Overflow,
    /// A successful response whose input already exceeded the window
    /// (Pi's "silent overflow", overflow.ts Case 2).
    SilentOverflow { input_tokens: u32 },
}

struct ScriptProvider {
    steps: Mutex<VecDeque<Step>>,
    calls: Arc<AtomicUsize>,
    /// Message count of each request, in call order.
    request_sizes: Arc<Mutex<Vec<usize>>>,
}

impl ScriptProvider {
    fn new(steps: Vec<Step>) -> (Arc<Self>, Arc<AtomicUsize>, Arc<Mutex<Vec<usize>>>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let request_sizes = Arc::new(Mutex::new(Vec::new()));
        let p = Arc::new(Self {
            steps: Mutex::new(steps.into()),
            calls: calls.clone(),
            request_sizes: request_sizes.clone(),
        });
        (p, calls, request_sizes)
    }

    fn events(&self, step: Step) -> Vec<Result<StreamEvent, ProviderError>> {
        let n = self.calls.load(Ordering::SeqCst);
        let start = Ok(StreamEvent::MessageStart {
            id: format!("msg-{n}"),
            model: "test".into(),
        });
        match step {
            Step::ToolTurn { input_tokens } => vec![
                start,
                Ok(StreamEvent::TextDelta {
                    text: "working".into(),
                }),
                Ok(StreamEvent::ToolCallStart {
                    id: format!("t{n}"),
                    name: "noop".into(),
                }),
                Ok(StreamEvent::ToolCallStop {
                    id: format!("t{n}"),
                    input_json: serde_json::json!({}),
                }),
                Ok(StreamEvent::MessageStop {
                    stop_reason: "tool_use".into(),
                    usage: PUsage {
                        input_tokens,
                        output_tokens: 8,
                        cache_read_tokens: 0,
                        cache_write_tokens: 0,
                    },
                }),
            ],
            Step::Finish => vec![
                start,
                Ok(StreamEvent::TextDelta {
                    text: "done".into(),
                }),
                Ok(StreamEvent::MessageStop {
                    stop_reason: "end_turn".into(),
                    usage: PUsage::default(),
                }),
            ],
            Step::Overflow => vec![
                start,
                Ok(StreamEvent::Error {
                    message: "maximum context length exceeded (200000 tokens)".into(),
                }),
            ],
            Step::SilentOverflow { input_tokens } => vec![
                start,
                Ok(StreamEvent::TextDelta {
                    text: "partial".into(),
                }),
                Ok(StreamEvent::MessageStop {
                    stop_reason: "end_turn".into(),
                    usage: PUsage {
                        input_tokens,
                        output_tokens: 4,
                        cache_read_tokens: 0,
                        cache_write_tokens: 0,
                    },
                }),
            ],
            // `SilentOverflow` is exercised by the detector test at the end
            // of this file; reaching it through a run is what
            // `a_successful_response_over_the_window_compacts` does.
        }
    }
}

impl Provider for ScriptProvider {
    fn name(&self) -> &'static str {
        "script"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
    fn stream(
        &self,
        req: Request,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send + 'static>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.request_sizes.lock().unwrap().push(req.messages.len());
        let step = self
            .steps
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Step::Finish);
        Box::pin(futures_util::stream::iter(self.events(step)))
    }
}

// ---------------------------------------------------------------------------
// A tool, so the loop has more than one turn to survive.
// ---------------------------------------------------------------------------

struct Noop;

#[async_trait::async_trait]
impl Tool for Noop {
    fn name(&self) -> &'static str {
        "noop"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "noop".into(),
            description: "does nothing".into(),
            input_schema: serde_json::json!({ "type": "object", "properties": {} }),
        }
    }
    async fn execute(
        &self,
        _args: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolOutput, nini_core::tool::ToolError> {
        Ok(ToolOutput {
            content: "ok".into(),
            is_error: false,
            details: None,
            duration_ms: 0,
            terminate: false,
        })
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// What the hook observed, and how often it ran.
struct HookLog {
    calls: AtomicUsize,
    /// Last message of the history each invocation received.
    seen: Mutex<Vec<String>>,
}

fn build(
    provider: Arc<ScriptProvider>,
    hook: Option<CompactionHook>,
    window: u32,
    reserve: u32,
    max_iterations: usize,
) -> Agent {
    let tools = ToolRegistry::new().register(Arc::new(Noop));
    let mut cfg = RunConfig::new("test-model");
    cfg.compaction.context_window = window;
    cfg.compaction.reserve_tokens = reserve;
    cfg.compaction.enabled = true;
    cfg.max_iterations = max_iterations;
    cfg.compact = hook;
    let mut agent = Agent::new(provider, tools, cfg);
    agent.begin_auto_compaction();
    agent
}

async fn drain(agent: &mut Agent) -> Vec<nini_core::AgentEvent> {
    use futures_util::StreamExt;
    let s = agent.run(nini_core::AgentMessage::user("do the long task"));
    tokio::pin!(s);
    let mut out = Vec::new();
    while let Some(ev) = s.next().await {
        match ev {
            Ok(ev) => out.push(ev),
            Err(_) => break,
        }
    }
    out
}

/// A hook that compacts: counts calls, records what it saw, and returns a
/// short history — the "session already folded it" case.
fn compacting_hook(log: Arc<HookLog>) -> CompactionHook {
    CompactionHook::new(move |messages| {
        log.calls.fetch_add(1, Ordering::SeqCst);
        let last = messages
            .last()
            .map(|m| {
                m.content
                    .iter()
                    .map(|c| match c {
                        nini_core::ContentBlock::Text { text } => text.clone(),
                        _ => String::new(),
                    })
                    .collect::<String>()
            })
            .unwrap_or_default();
        log.seen.lock().unwrap().push(last);
        let keep: Vec<_> = messages.iter().rev().take(2).rev().cloned().collect();
        Some(CompactionResult {
            summary: "[summary]".into(),
            tokens_before: 999_999,
            tokens_after: 10,
            messages: keep,
        })
    })
}

// ---------------------------------------------------------------------------
// 1. The threshold check must exist *inside* a run
// ---------------------------------------------------------------------------

/// Pi compacts on threshold after every assistant message
/// (`agent-session.ts:1141`, Case 2) — so a run whose context crosses the
/// window mid-flight compacts without waiting for the API to complain.
///
/// nini deleted this check (`agent.rs:956-975`) and relies on a compaction
/// that only happens *between* submissions (`runtime.rs:2102`). A single run
/// of N tool turns therefore never compacts, which is precisely the "long
/// task breaks" shape.
#[tokio::test(flavor = "current_thread")]
async fn a_run_compacts_at_threshold_without_waiting_for_an_error() {
    let (provider, _calls, _sizes) = ScriptProvider::new(vec![
        Step::ToolTurn { input_tokens: 1500 },
        Step::ToolTurn { input_tokens: 1500 },
        Step::ToolTurn { input_tokens: 1500 },
        Step::Finish,
    ]);
    let log = Arc::new(HookLog {
        calls: AtomicUsize::new(0),
        seen: Mutex::new(Vec::new()),
    });
    // Budget = window - reserve = 1000; every turn reports 1500 input
    // tokens, so the threshold is crossed from the second turn on while the
    // window itself is never exceeded. That is Pi's Case 2, which exists
    // precisely so the API never has to refuse.
    let mut agent = build(provider, Some(compacting_hook(log.clone())), 2000, 1000, 50);

    drain(&mut agent).await;

    assert!(
        log.calls.load(Ordering::SeqCst) > 0,
        "the context crossed the compaction budget inside one run and \
         nothing compacted it — nini only checks between submissions"
    );
}

// ---------------------------------------------------------------------------
// 2. A provider-reported overflow must be recoverable, not fatal
// ---------------------------------------------------------------------------

/// Pi: an overflow arrives as an assistant message with
/// `stopReason === "error"` and an error message; `_checkCompaction` Case 1
/// catches it and compacts-and-retries.
///
/// nini: the error text arrives on `StreamEvent::Error`
/// (`nini-ai/src/openai.rs:418`), a different branch that treats it as a
/// fatal, non-retryable provider error (`agent.rs:1235` + `agent.rs:216`).
/// The run dies with the history untouched.
#[tokio::test(flavor = "current_thread")]
async fn a_provider_reported_overflow_compacts_and_retries() {
    let (provider, calls, _sizes) = ScriptProvider::new(vec![Step::Overflow, Step::Finish]);
    let log = Arc::new(HookLog {
        calls: AtomicUsize::new(0),
        seen: Mutex::new(Vec::new()),
    });
    let mut agent = build(
        provider,
        Some(compacting_hook(log.clone())),
        200_000,
        16_384,
        50,
    );

    let events = drain(&mut agent).await;

    assert!(
        log.calls.load(Ordering::SeqCst) > 0,
        "an HTTP 400 overflow never reached the compaction hook — the run \
         aborted instead of recovering"
    );
    assert!(
        calls.load(Ordering::SeqCst) >= 2,
        "the run was not retried after compacting (calls = {})",
        calls.load(Ordering::SeqCst)
    );
    assert!(
        !events.iter().any(|e| matches!(
            e,
            nini_core::AgentEvent::Error { message } if message.contains("maximum context length")
        )),
        "the overflow was surfaced as a terminal error rather than recovered"
    );
}

// ---------------------------------------------------------------------------
// 3. Recovery is attempted once, then it says so
// ---------------------------------------------------------------------------

/// Pi's `_overflowRecoveryAttempted` (`agent-session.ts:336/2001`) allows one
/// compact-and-retry. The second failure emits a specific, actionable error
/// and stops.
///
/// nini has no such bound: `agent.rs:1179` `continue`s with the identical
/// oversized context, which overflows again, until `max_iterations` trips
/// with `TooManyIterations` — a message that says nothing about context.
#[tokio::test(flavor = "current_thread")]
async fn a_second_overflow_reports_instead_of_looping() {
    let (provider, calls, _sizes) = ScriptProvider::new(vec![
        Step::Overflow,
        Step::Overflow,
        Step::Overflow,
        Step::Overflow,
        Step::Finish,
    ]);
    let log = Arc::new(HookLog {
        calls: AtomicUsize::new(0),
        seen: Mutex::new(Vec::new()),
    });
    let mut agent = build(
        provider,
        Some(compacting_hook(log.clone())),
        200_000,
        16_384,
        50,
    );

    let events = drain(&mut agent).await;

    let n = calls.load(Ordering::SeqCst);
    assert!(
        n <= 4,
        "recovery retried {n} times — Pi allows exactly one compact-and-retry \
         (agent-session.ts:2172)"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            nini_core::AgentEvent::Error { message }
                if message.to_lowercase().contains("overflow")
                    || message.to_lowercase().contains("compact")
        )),
        "the second overflow produced no explanation: {events:?}"
    );
}

// ---------------------------------------------------------------------------
// 4. The compacted context is the one the retry uses
// ---------------------------------------------------------------------------

/// Pi rebuilds state from the compaction and retries with it
/// (`agent-session.ts#sessionContext.messages-2170`). If nini compacts but re-sends the old
/// history, the retry overflows identically.
#[tokio::test(flavor = "current_thread")]
async fn the_retry_runs_against_the_compacted_history() {
    let (provider, _calls, sizes) = ScriptProvider::new(vec![
        Step::ToolTurn { input_tokens: 10 },
        Step::Overflow,
        Step::Finish,
    ]);
    let log = Arc::new(HookLog {
        calls: AtomicUsize::new(0),
        seen: Mutex::new(Vec::new()),
    });
    let mut agent = build(
        provider,
        Some(compacting_hook(log.clone())),
        200_000,
        16_384,
        50,
    );

    drain(&mut agent).await;

    let sizes = sizes.lock().unwrap().clone();
    assert!(
        sizes.len() >= 3,
        "no compact-and-retry happened; request sizes were {sizes:?}"
    );
    // The last request is the retry, the one before it carried the history
    // that overflowed.
    assert!(
        *sizes.last().unwrap() < sizes[sizes.len() - 2],
        "the retry carried the pre-compaction history ({sizes:?})"
    );
}

// ---------------------------------------------------------------------------
// 5. A successful compaction is not an error event
// ---------------------------------------------------------------------------

/// nini emitted `AgentEvent::Error { message: "auto-compaction (overflow):
/// tokens before=… after=…" }` (`agent.rs:1229`) — a status line in an
/// error channel. The TUI renders it red as a failure.
#[tokio::test(flavor = "current_thread")]
async fn a_successful_compaction_is_not_reported_as_an_error() {
    let (provider, _calls, _sizes) = ScriptProvider::new(vec![Step::Overflow, Step::Finish]);
    let log = Arc::new(HookLog {
        calls: AtomicUsize::new(0),
        seen: Mutex::new(Vec::new()),
    });
    let mut agent = build(
        provider,
        Some(compacting_hook(log.clone())),
        200_000,
        16_384,
        50,
    );

    let events = drain(&mut agent).await;

    assert!(
        !events
            .iter()
            .any(|e| matches!(e, nini_core::AgentEvent::Error { message }
                if message.starts_with("auto-compaction"))),
        "compaction bookkeeping was emitted on the error channel"
    );
}

// ---------------------------------------------------------------------------
// 6. The detector itself: Pi's three cases
// ---------------------------------------------------------------------------

/// `overflow.ts:134-162`. nini's `overflow.rs` has one string branch plus a
/// `length`-stop branch, and the production call site passes `None` for the
/// error message — so Case 1 never fires in the product.
#[test]
fn the_overflow_detector_covers_pis_three_cases() {
    use nini_core::overflow::is_context_overflow;

    let win = Some(1000u32);
    let usage = |i: u32, o: u32| PUsage {
        input_tokens: i,
        output_tokens: o,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
    };

    // Case 1 — the message pattern, which is how every real 400 arrives.
    assert!(
        is_context_overflow(
            "error",
            Some("This model's maximum context length is 200000 tokens"),
            None,
            win
        ),
        "Case 1 (error message pattern) not detected"
    );
    // …and throttling must NOT be mistaken for overflow (NON_OVERFLOW_PATTERNS).
    assert!(
        !is_context_overflow("error", Some("rate limit exceeded, retry later"), None, win),
        "a rate limit was classified as an overflow"
    );

    // Case 2 — silent overflow: a *successful* response whose input already
    // exceeded the window.
    assert!(
        is_context_overflow("end_turn", None, Some(&usage(1200, 4)), win),
        "Case 2 (silent overflow on a successful response) not detected"
    );

    // Case 3 — length stop with no output and input filling the window.
    assert!(
        is_context_overflow("length", None, Some(&usage(1000, 0)), win),
        "Case 3 (length stop, output 0, window filled) not detected"
    );
}

// ---------------------------------------------------------------------------
// 8. Silent overflow: a *successful* response that was already too big
// ---------------------------------------------------------------------------

/// Pi's Case 2 (`overflow.ts:145`): the provider truncates instead of
/// refusing, so the turn looks like a normal success — `stopReason === "stop"`
/// — while `usage.input + cacheRead` exceeds the window.
///
/// nini's old detector only looked at the error text and at `stop_reason ==
/// "length"`, so this case was invisible: the turn succeeded, the context
/// kept growing, and the next few requests died on a provider that may or
/// may not have started complaining.
#[tokio::test(flavor = "current_thread")]
async fn a_successful_response_over_the_window_compacts() {
    let (provider, calls, _sizes) = ScriptProvider::new(vec![
        Step::SilentOverflow {
            input_tokens: 2_400,
        },
        Step::Finish,
    ]);
    let log = Arc::new(HookLog {
        calls: AtomicUsize::new(0),
        seen: Mutex::new(Vec::new()),
    });
    let mut agent = build(provider, Some(compacting_hook(log.clone())), 2000, 0, 50);

    let events = drain(&mut agent).await;

    assert!(
        log.calls.load(Ordering::SeqCst) > 0,
        "a turn that succeeded with 2400 input tokens against a 2000-token \
         window did not trigger compaction; the old detector only matched \
         error text and `length` stops"
    );
    assert!(
        calls.load(Ordering::SeqCst) >= 2,
        "no retry after compacting"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, nini_core::AgentEvent::Error { .. })),
        "a recoverable overflow was reported as an error: {events:?}"
    );
}
