//! Pi's overflow口径: **a completed response is compacted, never replayed.**
//!
//! ## The rule (Pi, verbatim)
//!
//! `references/pi-0.85.1/src/session/agent-session.ts:2164-2170`:
//!
//! ```text
//! const willRetry = assistantMessage.stopReason !== "stop";
//! // Case 2: the response completed successfully. Compact, but do not retry because
//! // agent.continue() cannot continue from a completed assistant response.
//! if (!willRetry) {
//!     return await this._runAutoCompaction("overflow", false);
//! }
//! ```
//!
//! * the response **completed** (Pi's `stopReason === "stop"`; nini's
//!   `"stop"` / `"end_turn"`) → **compact, but do not replay**;
//! * the response did **not** complete (`length`, `error`, …) → compact
//!   **and** replay.
//!
//! Pi's one-shot bound (`_overflowRecoveryAttempted`) is checked *after*
//! the `!willRetry` early return (`:2167-2172`), so a completed response
//! that merely gets compacted neither consumes the budget nor is refused
//! by it.
//!
//! ## nini's one deliberate deviation
//!
//! nini additionally refuses to replay when the attempt already
//! **produced output** (`produced_output`): content that reached the user
//! cannot be taken back — Pi deletes the failed assistant message from its
//! state and replays, nini cannot un-print the terminal, so a replay would
//! show the same passage twice (terminal *and* session file).
//!
//! ## What is asserted here
//!
//! * a **silent overflow** (a successful response whose `usage.input_tokens`
//!   already exceeds the window) **after** the model produced text is
//!   **compacted** (the hook runs, a `Compacted` event is emitted) and
//!   **not replayed** (the provider is called once); the answer survives;
//! * the same, **before** anything was produced: also compacted, also
//!   **not** replayed — Pi's Case 2 does not care whether there was
//!   output;
//! * a **length stop** that overflows (Pi's `overflow.ts` Case 3) is
//!   compacted **and the request goes out again** — but through the
//!   truncation-continuation path (`consecutive_truncations`), not
//!   through `recover_from_overflow`'s `will_retry` branch: the
//!   length-stop branch in `agent.rs` is reached first and never hands
//!   the turn to the overflow-recovery routine. The `will_retry ==
//!   true` road (the recovery of a **failed** request) is covered by
//!   `crates/nini-core/tests/overflow_http_400.rs`.
//! * the retry backoff starts at Pi's 2 s (`baseDelayMs: 2000`,
//!   `references/pi-0.85.1/src/core/settings-manager.ts:882-886`) and
//!   doubles: 2000 / 4000 / 8000.

use nini_core::agent::Agent;
use nini_core::agent::{
    backoff_ms, CompactionHook, CompactionResult, RetrySettings, RunConfig, DEFAULT_RETRY_BASE_MS,
};
use nini_core::provider::{
    Capabilities, ContentBlock, Message, Provider, ProviderError, Request, Role, StreamEvent, Usage,
};
use nini_core::tool::ToolRegistry;
use nini_core::AgentEvent;
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Provider script
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Step {
    /// A *successful* response whose `usage.input_tokens` already exceeds
    /// the window — Pi's "silent overflow" (`overflow.ts` Case 2). The
    /// text is optional: without it, nothing was produced.
    SilentOverflow {
        text: Option<String>,
        input_tokens: u32,
    },
    /// A **length stop** with no output whose input already filled the
    /// window — Pi's `overflow.ts` Case 3. The response did **not**
    /// complete, so Pi's `willRetry` is `true`.
    LengthOverflow { input_tokens: u32 },
    /// An ordinary turn.
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
        let step = self.steps.lock().unwrap().pop_front();
        let start = Ok(StreamEvent::MessageStart {
            id: "msg-1".into(),
            model: "test".into(),
        });
        let events: Vec<Result<StreamEvent, ProviderError>> = match step {
            Some(Step::SilentOverflow { text, input_tokens }) => {
                let mut evs = vec![start];
                if let Some(text) = text {
                    evs.push(Ok(StreamEvent::TextDelta { text }));
                }
                evs.push(Ok(StreamEvent::MessageStop {
                    stop_reason: "end_turn".into(),
                    usage: Usage {
                        input_tokens,
                        output_tokens: 4,
                        cache_read_tokens: 0,
                        cache_write_tokens: 0,
                    },
                }));
                evs
            }
            Some(Step::LengthOverflow { input_tokens }) => vec![
                start,
                Ok(StreamEvent::MessageStop {
                    stop_reason: "length".into(),
                    usage: Usage {
                        input_tokens,
                        output_tokens: 0,
                        cache_read_tokens: 0,
                        cache_write_tokens: 0,
                    },
                }),
            ],
            Some(Step::Ok(text)) => vec![
                start,
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
// Harness
// ---------------------------------------------------------------------------

/// A window small enough that a reported `input_tokens` of 5_000 is over
/// it — the silent-overflow trigger — while the *estimated* size of the
/// conversation stays far below the compaction budget, so the threshold
/// compaction never fires and every compaction in these tests comes from
/// the overflow path.
fn config() -> RunConfig {
    let mut cfg = RunConfig::new("test-model");
    cfg.compaction.context_window = 100;
    cfg.compaction.reserve_tokens = 10;
    cfg.compaction.keep_recent_tokens = 20;
    cfg.max_iterations = 10;
    // The backoff is real but must not make the test slow.
    cfg.retry.base_delay_ms = 1;
    cfg
}

/// A hook that compacts: counts the calls and replaces the history with a
/// short summary.
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

/// A hook that compacts **without changing the history**: it returns no
/// messages, so `compact_history_via_hook` leaves the agent's own history
/// exactly as it was — a compaction that changed nothing.
fn no_change_hook(calls: Arc<AtomicUsize>) -> CompactionHook {
    CompactionHook::new(move |_msgs: &[Message]| {
        calls.fetch_add(1, Ordering::SeqCst);
        Some(CompactionResult {
            summary: "[summary]".into(),
            tokens_before: 999_999,
            tokens_after: 10,
            messages: vec![],
        })
    })
}

async fn drain(agent: &mut Agent) -> (Vec<AgentEvent>, Option<String>) {
    use futures_util::StreamExt;
    let stream = agent.run(nini_core::AgentMessage::user("go"));
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

fn text_deltas(events: &[AgentEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TextDelta { text } => Some(text.clone()),
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

fn error_messages(events: &[AgentEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Error { message } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 1. A silent overflow *after* the model answered: compacted, not replayed
// ---------------------------------------------------------------------------

/// Pi's Case 2: the response **completed** (`stop_reason == "end_turn"`,
/// Pi's `"stop"`), so it is compacted but never replayed — "agent.continue()
/// cannot continue from a completed assistant response".
///
/// The turn ends normally; the answer survives; the oversized context is
/// the next round's problem.
#[tokio::test(flavor = "current_thread")]
async fn a_silent_overflow_after_output_is_compacted_but_not_replayed() {
    let (provider, calls) = ScriptProvider::new(vec![Step::SilentOverflow {
        text: Some("the answer".into()),
        input_tokens: 5_000,
    }]);
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let mut agent = Agent::new(provider, ToolRegistry::new(), config());
    agent.set_compaction_hook(compacting_hook(hook_calls.clone()));

    let (events, error) = drain(&mut agent).await;

    assert!(
        hook_calls.load(Ordering::SeqCst) >= 1,
        "the completed response was not compacted — Pi's Case 2 compacts \
         even when it does not retry: {events:?}"
    );
    assert!(
        compacted_count(&events) >= 1,
        "no Compacted event: {events:?}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the completed response was replayed — Pi's Case 2 compacts but does \
         not retry: {events:?}"
    );
    assert!(
        text_deltas(&events).iter().any(|t| t == "the answer"),
        "the answer was lost: {events:?}"
    );
    assert!(error.is_none(), "the run ended with {error:?}: {events:?}");
}

// ---------------------------------------------------------------------------
// 2. A silent overflow *before* anything was produced: also not replayed
// ---------------------------------------------------------------------------

/// Pi's Case 2 does not ask whether there was output: a **completed**
/// response is compacted and never replayed, whatever it contained.
///
/// (The earlier version of this file asserted the opposite — "still
/// compacts and replays" — and was wrong: it made nini's rule narrower
/// than Pi's.)
#[tokio::test(flavor = "current_thread")]
async fn a_silent_overflow_without_output_is_compacted_but_not_replayed() {
    let (provider, calls) = ScriptProvider::new(vec![Step::SilentOverflow {
        text: None,
        input_tokens: 5_000,
    }]);
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let mut agent = Agent::new(provider, ToolRegistry::new(), config());
    agent.set_compaction_hook(compacting_hook(hook_calls.clone()));

    let (events, error) = drain(&mut agent).await;

    assert!(
        hook_calls.load(Ordering::SeqCst) >= 1,
        "the completed response was not compacted: {events:?}"
    );
    assert!(
        compacted_count(&events) >= 1,
        "no Compacted event: {events:?}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the completed response was replayed — a response that finished is \
         never replayed: {events:?}"
    );
    assert!(error.is_none(), "the run ended with {error:?}: {events:?}");
}

// ---------------------------------------------------------------------------
// 3. A length stop that overflows: compacted, and the request goes out again
// ---------------------------------------------------------------------------

/// The other branch of Pi's rule: the response did **not** complete
/// (`stop_reason == "length"`), so the turn is not over — the history is
/// compacted and the request is sent again.
///
/// **This is the truncation-continuation path, not the overflow-recovery
/// routine.** In nini the length stop is carried by `agent.rs`'s
/// `length_truncated` branch (v0.17: the half-written answer goes to the
/// transcript, a "continue" notice goes into the model's context, the
/// request goes out again) — and that branch is reached *before* the
/// overflow check, so `recover_from_overflow` never sees this turn at
/// all. What this test locks is therefore the **observable** of Pi's
/// length branch: the history is compacted and the provider is called
/// again (≥ 2 calls).
///
/// The `will_retry == true` road of `recover_from_overflow` — the
/// recovery of a **failed** request (a 400 that means "too long") — is
/// covered by `crates/nini-core/tests/overflow_http_400.rs`.
#[tokio::test(flavor = "current_thread")]
async fn a_length_stop_overflow_is_compacted_and_the_request_goes_out_again() {
    let (provider, calls) = ScriptProvider::new(vec![
        Step::LengthOverflow {
            input_tokens: 5_000,
        },
        Step::Ok("done".into()),
    ]);
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let mut agent = Agent::new(provider, ToolRegistry::new(), config());
    agent.set_compaction_hook(compacting_hook(hook_calls.clone()));

    let (events, error) = drain(&mut agent).await;

    assert!(
        hook_calls.load(Ordering::SeqCst) >= 1,
        "the overflow never reached the compaction hook: {events:?}"
    );
    assert!(
        compacted_count(&events) >= 1,
        "no Compacted event: {events:?}"
    );
    assert!(
        calls.load(Ordering::SeqCst) >= 2,
        "the request did not go out again against the compacted history — a \
         response that did not complete is continued (Pi's length branch): {events:?}"
    );
    assert!(error.is_none(), "the run ended with {error:?}: {events:?}");
}

// ---------------------------------------------------------------------------
// 4. The retry backoff starts at Pi's 2 s and doubles
// ---------------------------------------------------------------------------
/// Pi: `maxRetries: 3`, `baseDelayMs: 2000`
/// (`references/pi-0.85.1/src/core/settings-manager.ts:882-886`), delay
/// `baseDelayMs * 2 ** (retryAttempt - 1)`
/// (`references/pi-0.85.1/src/session/agent-session.ts:2894-2921`) →
/// 2000 / 4000 / 8000.
///
/// nini used 500 ms (500 / 1000 / 2000, 3.5 s in total): a one-second
/// network blip was enough for nini to give up while Pi carried on.
#[test]
fn the_retry_backoff_matches_pi() {
    assert_eq!(DEFAULT_RETRY_BASE_MS, 2000);
    assert_eq!(RetrySettings::default().base_delay_ms, 2000);
    assert_eq!(RetrySettings::default().max_retries, 3);
    assert_eq!(backoff_ms(1, 2000), 2000);
    assert_eq!(backoff_ms(2, 2000), 4000);
    assert_eq!(backoff_ms(3, 2000), 8000);
}

// ---------------------------------------------------------------------------
// 5. A compaction that changed nothing is reported — even without a replay
// ---------------------------------------------------------------------------

/// A compaction that **changed nothing** must be reported on every path.
///
/// The report used to hang off the *veto* — the guard that refuses a
/// replay carrying the very same history — and the veto only fires when
/// the caller demands a different history (`must_have_changed`). The
/// MessageStop arm does not demand one, so on the "compact, do not
/// replay" road the run said **nothing**: the user read the `Compacted`
/// event, believed the context had been folded, and the next round hit
/// the same window.
///
/// The hook below returns no messages, so `compact_history_via_hook`
/// leaves the agent's own history exactly as it was.
#[tokio::test(flavor = "current_thread")]
async fn a_compaction_that_changes_nothing_is_reported_even_without_a_replay() {
    let (provider, calls) = ScriptProvider::new(vec![Step::SilentOverflow {
        text: Some("the answer".into()),
        input_tokens: 5_000,
    }]);
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let mut agent = Agent::new(provider, ToolRegistry::new(), config());
    agent.set_compaction_hook(no_change_hook(hook_calls.clone()));

    let (events, error) = drain(&mut agent).await;

    assert!(
        hook_calls.load(Ordering::SeqCst) >= 1,
        "the completed response was not compacted: {events:?}"
    );
    assert!(
        compacted_count(&events) >= 1,
        "no Compacted event: {events:?}"
    );
    let messages = error_messages(&events);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("history did not change")),
        "a compaction that changed nothing was swallowed — the user is told \
         the context was compacted, and the next round hits the same wall: \
         {events:?}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the request was replayed against an unchanged history: {events:?}"
    );
    assert!(
        text_deltas(&events).iter().any(|t| t == "the answer"),
        "the answer was lost: {events:?}"
    );
    assert!(error.is_none(), "the run ended with {error:?}: {events:?}");
}
