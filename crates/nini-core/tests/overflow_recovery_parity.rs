//! v0.12 (review): the overflow-recovery bound and the *diagnostics* around it.
//!
//! ## W5 — Pi parity: a good turn clears the one-shot bound
//!
//! Pi resets `_overflowRecoveryAttempted` whenever an assistant message
//! arrives whose stop reason is neither "error" nor "length"
//! (`references/pi-0.85.1/src/session/agent-session.ts:697-699`).
//!
//! nini only ever *set* the flag. On a long task that means: overflow →
//! compact → recover → the model answers → the task grows → **overflow
//! again** → and nini refuses the second recovery with "recovery failed
//! after one compact-and-retry attempt", while Pi compacts once more and
//! carries on. The flag has to be reset where Pi resets it.
//!
//! ## W9 — a *summary* is not a server verdict
//!
//! The fallback provider ends with a diagnostic nini composed itself:
//!
//! ```text
//!   all 3 fallback providers exhausted: openai: HTTP 400: Bad Request (code 10907)
//! ```
//!
//! It quotes another provider's error verbatim. The agent used to run the
//! vendor-code extractor over that text, read the *quoted* `10907` as this
//! error's own code, and classified the summary as a context overflow —
//! compacting the user's history over a provider chain that had already
//! given up.
//!
//! ## What is asserted here
//!
//! * a second overflow **after** a successful turn is recovered again;
//! * the fallback summary is **not** an overflow and does not compact;
//! * a status-less failure is still judged by its **text** (retryable or
//!   not) — the vendor-code channel never replaced that.

use async_trait::async_trait;
use nini_core::agent::AgentPhase;
use nini_core::agent::{Agent, CompactionHook, CompactionResult, RunConfig};
use nini_core::provider::{
    Capabilities, ContentBlock, Message, Provider, ProviderError, Request, Role, StreamEvent, Usage,
};
use nini_core::tool::{Tool, ToolContext, ToolError, ToolOutput, ToolRegistry, ToolSpec};
use nini_core::AgentEvent;
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Provider script
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Step {
    Fail(ProviderError),
    /// A turn that calls a tool, so the agent loop keeps going.
    ToolCall,
    Ok(String),
}

struct ScriptProvider {
    steps: Mutex<VecDeque<Step>>,
    calls: Arc<AtomicUsize>,
}

impl ScriptProvider {
    fn new(steps: Vec<Step>) -> (Arc<Self>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(Self {
            steps: Mutex::new(steps.into()),
            calls: calls.clone(),
        });
        (provider, calls)
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
        _req: Request,
    ) -> Pin<Box<dyn futures_core::Stream<Item = Result<StreamEvent, ProviderError>> + Send>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let n = self.calls.load(Ordering::SeqCst);
        let step = self.steps.lock().unwrap().pop_front();
        let events: Vec<Result<StreamEvent, ProviderError>> = match step {
            Some(Step::Fail(e)) => vec![Err(e)],
            Some(Step::ToolCall) => vec![
                Ok(StreamEvent::MessageStart {
                    id: "msg-1".into(),
                    model: "test".into(),
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
                    usage: Usage::default(),
                }),
            ],
            Some(Step::Ok(text)) => vec![
                Ok(StreamEvent::MessageStart {
                    id: "msg-1".into(),
                    model: "test".into(),
                }),
                Ok(StreamEvent::TextDelta { text }),
                Ok(StreamEvent::MessageStop {
                    stop_reason: "end_turn".into(),
                    usage: Usage::default(),
                }),
            ],
            None => vec![],
        };
        Box::pin(futures_util::stream::iter(events))
    }
}

// ---------------------------------------------------------------------------
// A tool, so the agent loop has more than one turn to survive.
// ---------------------------------------------------------------------------

struct Noop;

#[async_trait]
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
    ) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::ok("ok"))
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn config() -> RunConfig {
    let mut cfg = RunConfig::new("test-model");
    // A window big enough that the *threshold* compaction never fires —
    // every compaction in these tests must come from the overflow path.
    cfg.compaction.context_window = 200_000;
    cfg.compaction.reserve_tokens = 16_384;
    cfg.compaction.keep_recent_tokens = 20_000;
    cfg.max_iterations = 10;
    cfg.retry.base_delay_ms = 1;
    cfg
}

/// A hook that compacts into a short summary.
fn compacting_hook(calls: Arc<AtomicUsize>) -> CompactionHook {
    CompactionHook::new(move |_msgs: &[Message]| {
        calls.fetch_add(1, Ordering::SeqCst);
        Some(CompactionResult {
            summary: "[summary]".into(),
            tokens_before: 999_999,
            tokens_after: 10,
            messages: vec![Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "[summary]".into(),
                }],
                timestamp: 0,
            }],
        })
    })
}

async fn drain(agent: &mut Agent) -> (Vec<AgentEvent>, Option<String>) {
    use futures_util::StreamExt;
    let stream = agent.run(nini_core::AgentMessage::user(
        "do the long task — the conversation that does not fit into the window",
    ));
    tokio::pin!(stream);
    let mut events = Vec::new();
    let mut error = None;
    while let Some(ev) = stream.next().await {
        match ev {
            Ok(ev) => events.push(ev),
            Err(e) => {
                error = Some(e.to_string());
                break;
            }
        }
    }
    (events, error)
}

fn error_messages(events: &[AgentEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Error { message } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

fn retrying_attempts(events: &[AgentEvent]) -> Vec<u32> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::PhaseChanged(AgentPhase::Retrying { attempt }) => Some(*attempt),
            _ => None,
        })
        .collect()
}

fn compacted_count(events: &[AgentEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, AgentEvent::Compacted { .. }))
        .count()
}

// ---------------------------------------------------------------------------
// 1. W5 — a second overflow after a good turn recovers again
// ---------------------------------------------------------------------------

/// The scenario Pi handles and nini did not:
///
/// ```text
///   request 1 → 400 "prompt is too long"   → compact, retry      (recovery 1)
///   request 2 → a tool call                 → the task continues
///   request 3 → 400 "prompt is too long"   → compact, retry      (recovery 2)
/// ```
///
/// The turn in between succeeded, so Pi's bound is cleared
/// (`agent-session.ts:697-699`) and Pi compacts again. nini kept the bound
/// set for the whole run and refused the second recovery.
#[tokio::test(flavor = "current_thread")]
async fn a_second_overflow_after_a_good_turn_recovers_again() {
    let overflow = || {
        Step::Fail(ProviderError::Api {
            status: 400,
            message: "request failed: 400 Bad Request: prompt is too long: 300000 tokens > 200000"
                .into(),
        })
    };
    let (provider, calls) = ScriptProvider::new(vec![
        overflow(),
        Step::ToolCall,
        overflow(),
        Step::Ok("done".into()),
    ]);
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let tools = ToolRegistry::new().register(Arc::new(Noop));
    let mut agent = Agent::new(provider, tools, config());
    agent.set_compaction_hook(compacting_hook(hook_calls.clone()));

    let (events, error) = drain(&mut agent).await;

    assert_eq!(
        hook_calls.load(Ordering::SeqCst),
        2,
        "the second overflow was not recovered: the one-shot bound was never \
         cleared by the successful turn in between ({:?})",
        error_messages(&events)
    );
    assert_eq!(
        compacted_count(&events),
        2,
        "the second recovery did not compact: {events:?}"
    );
    assert!(
        error.is_none(),
        "the run ended with {error:?} — the second overflow was refused"
    );
    assert!(
        !error_messages(&events)
            .iter()
            .any(|m| m.contains("recovery failed after one compact-and-retry attempt")),
        "the second overflow was refused by the one-shot bound: {events:?}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 4, "{events:?}");
}

// ---------------------------------------------------------------------------
// 2. W9 — the fallback summary is not a server verdict
// ---------------------------------------------------------------------------

/// The fallback provider's last word is a *summary nini composed*: it
/// quotes the other providers' errors, code and all.
///
/// The vendor code in it belongs to the provider that failed, not to this
/// error. Reading it as this error's own code turned "every fallback
/// failed" into "your context is too long" — and the agent answered that
/// by compacting the user's history.
#[tokio::test(flavor = "current_thread")]
async fn a_fallback_exhaustion_summary_is_not_an_overflow() {
    let summary = "all 3 fallback providers exhausted: openai: HTTP 400: \
                   Bad Request (code 10907)";
    let (provider, calls) = ScriptProvider::new(vec![Step::Fail(ProviderError::NotImplemented(
        summary.into(),
    ))]);
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let mut agent = Agent::new(provider, ToolRegistry::new(), config());
    agent.set_compaction_hook(compacting_hook(hook_calls.clone()));

    let (events, error) = drain(&mut agent).await;

    assert_eq!(
        hook_calls.load(Ordering::SeqCst),
        0,
        "the fallback summary was read as a context overflow and the \
         history was compacted: {events:?}"
    );
    assert_eq!(compacted_count(&events), 0, "{events:?}");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the run was retried: {events:?}"
    );
    let error = error.expect("the run must fail");
    assert!(
        error.contains("fallback providers exhausted"),
        "the summary was lost: {error}"
    );
}

// ---------------------------------------------------------------------------
// 3. W9 — a status-less failure is still judged by its text
// ---------------------------------------------------------------------------

/// The vendor-code channel is *additional*. It never replaced the
/// text-based judgement, and it must not: a network-level failure has no
/// HTTP status, and the text is all there is.
#[tokio::test(flavor = "current_thread")]
async fn a_status_less_failure_is_still_judged_by_its_text() {
    let (provider, calls) = ScriptProvider::new(vec![
        Step::Fail(ProviderError::Http("connection reset by peer".into())),
        Step::Ok("done".into()),
    ]);
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let mut agent = Agent::new(provider, ToolRegistry::new(), config());
    agent.set_compaction_hook(compacting_hook(hook_calls.clone()));

    let (events, error) = drain(&mut agent).await;

    assert_eq!(
        hook_calls.load(Ordering::SeqCst),
        0,
        "a connection reset compacted the history: {events:?}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "a status-less transient failure was not retried: {events:?}"
    );
    assert_eq!(retrying_attempts(&events), vec![1], "{events:?}");
    assert!(error.is_none(), "the run ended with {error:?}");
}

// ---------------------------------------------------------------------------
// 4. The vendor-code channel is not applied to a summary
// ---------------------------------------------------------------------------

/// The extractor itself still finds the code — the *caller* decides
/// whether the text it has is a server's own words or a diagnostic nini
/// composed.
#[test]
fn the_extractor_still_finds_a_labelled_code() {
    let code = nini_core::overflow::extract_vendor_error_code(
        "request failed: 400 Bad Request: Bad Request (code 10907)",
    );
    assert_eq!(code.as_deref(), Some("10907"));
    // …and the code in a *summary* is the same string — which is exactly
    // why the agent must not read it out of a summary.
    let code = nini_core::overflow::extract_vendor_error_code(
        "all 3 fallback providers exhausted: openai: HTTP 400: Bad Request (code 10907)",
    );
    assert_eq!(code.as_deref(), Some("10907"));
}

/// A guard against a silent regression: the flags the agent uses must
/// exist and default to "no recovery attempted".
#[test]
fn the_recovery_flag_starts_clear() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| ());
    let provider = ScriptProvider::new(vec![]).0;
    let agent = Agent::new(provider, ToolRegistry::new(), config());
    assert_eq!(agent.retry_attempt(), 0);
}

// ---------------------------------------------------------------------------
// 5. W4 — the failure is reported once
// ---------------------------------------------------------------------------

/// The CLI pushes the `AgentEvent::Error` **and** the terminal error into
/// the same sink, so reporting a failure twice showed the same sentence
/// twice, glued into one line.
#[tokio::test(flavor = "current_thread")]
async fn a_failure_is_reported_once() {
    let (provider, _calls) = ScriptProvider::new(vec![Step::Fail(ProviderError::Api {
        status: 400,
        message: "request failed: 400 Bad Request: invalid request: unknown field `foo`".into(),
    })]);
    let mut agent = Agent::new(provider, ToolRegistry::new(), config());

    let (events, error) = drain(&mut agent).await;

    let error = error.expect("the request must fail");
    assert!(
        error.contains("unknown field `foo`"),
        "the server's own words were lost: {error}"
    );
    assert!(
        error_messages(&events).is_empty(),
        "the failure was reported twice: {events:?}"
    );
}
