//! v0.12: a context overflow that arrives as an **HTTP error** must compact
//! and retry — and the retry must carry a **different** request.
//!
//! ## The defect this pins down
//!
//! The agent loop has three error exits, and only two of them ever looked
//! at the overflow detector:
//!
//! | exit | overflow check? |
//! |---|---|
//! | `Ok(StreamEvent::MessageStop { .. })` | yes (usage only) |
//! | `Ok(StreamEvent::Error { message })` | yes (the text) |
//! | `Err(ProviderError::Api { .. })` | **no** — it was re-raised as-is |
//!
//! Every provider turns a non-2xx response into
//! `ProviderError::Api { status, message }`, so the **third** row is where
//! every real HTTP 400 arrives — including the 400 a long task produces
//! when it outgrows the model's window. The run died there: no
//! compaction, no retry, the history intact but the session already lost.
//!
//! ## What is asserted here
//!
//! * the overflow path **compacts** (the hook is called, `Compacted` and
//!   `PhaseChanged(Compacting)` are emitted) and then **retries**;
//! * the retry carries a **different history** — a compaction that
//!   changed nothing is not a compaction, and re-sending the very same
//!   request is what the whole path exists to avoid;
//! * the retry happens **whatever the size of the history was**: a
//!   summary of a tiny history is *longer* than the history it replaces,
//!   and refusing to retry there threw the recovery away entirely
//!   (the regression this file now pins down);
//! * a vendor that says nothing in prose (iFlytek's `10907`) is still
//!   recognised, through the structured error-code channel;
//! * a vendor code that means something else (`10005` — bad parameters,
//!   `10404` — model unavailable) is **not** an overflow: compacting there
//!   would throw the history away for nothing;
//! * a five-digit number that merely *equals* a table value (`invalid
//!   request id 10907`) is **not** a code — see the C1 regression below;
//! * a throttled request is still not an overflow, and a **5xx** is not
//!   one either: a broken server is retried, it does not cost the user
//!   their history;
//! * a request the server rejected for another reason keeps the server's
//!   own words **and** gains a hint about what to do next;
//! * the error is reported **once**, not twice;
//! * the recovery is still attempted **once** — the second overflow stops
//!   the run instead of looping, and it says what the server said.

use nini_core::agent::AgentPhase;
use nini_core::agent::{Agent, CompactionHook, CompactionResult, RunConfig};
use nini_core::overflow::{
    extract_vendor_error_code, is_context_overflow, is_context_overflow_with_signals,
    lookup_vendor_code, OverflowSignals, VendorCodeVerdict,
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

/// One scripted request.
#[derive(Clone, Debug)]
enum Step {
    /// The request fails with a provider error — this is what a real HTTP
    /// 400 looks like from the agent's side.
    Fail(ProviderError),
    /// The request succeeds.
    Ok(String),
}

struct ScriptProvider {
    steps: Mutex<VecDeque<Step>>,
    calls: Arc<AtomicUsize>,
    /// The **content** of every request, in call order.
    ///
    /// This is the probe that tells "the run retried" apart from "the run
    /// retried **with a different conversation**". Content, not size: the
    /// size of a request is not what decides whether a compaction
    /// happened (see `the_retry_carries_a_different_history`).
    seen: Arc<Mutex<Vec<String>>>,
}

impl ScriptProvider {
    fn new(steps: Vec<Step>) -> (Arc<Self>, Arc<AtomicUsize>, Arc<Mutex<Vec<String>>>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let provider = Arc::new(Self {
            steps: Mutex::new(steps.into()),
            calls: calls.clone(),
            seen: seen.clone(),
        });
        (provider, calls, seen)
    }
}

/// The content of a request, rendered as one string per request.
///
/// Two requests with the same rendering are the *same* history; two with
/// different renderings are different histories, whatever their sizes.
fn request_content(req: &Request) -> String {
    req.messages
        .iter()
        .map(|m| {
            let role = match m.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
            };
            let blocks = m
                .content
                .iter()
                .map(|b| match b {
                    ContentBlock::Text { text } => format!("text:{text}"),
                    ContentBlock::ToolUse { id, name, input } => {
                        format!("tool_use:{id}:{name}:{input}")
                    }
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => format!("tool_result:{tool_use_id}:{content}:{is_error}"),
                })
                .collect::<Vec<_>>()
                .join("|");
            format!("{role}[{blocks}]")
        })
        .collect::<Vec<_>>()
        .join("||")
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
    ) -> Pin<Box<dyn futures_core::Stream<Item = Result<StreamEvent, ProviderError>> + Send>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen.lock().unwrap().push(request_content(&req));
        let step = self.steps.lock().unwrap().pop_front();
        let events: Vec<Result<StreamEvent, ProviderError>> = match step {
            Some(Step::Fail(e)) => vec![Err(e)],
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
// Harness
// ---------------------------------------------------------------------------

/// The summary a compaction leaves behind. It is **different** from the
/// conversation it replaces — which is the whole point of a compaction,
/// and what the agent now checks before it retries.
fn summary_history() -> Vec<Message> {
    vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "[summary]".into(),
        }],
        timestamp: 0,
    }]
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
            messages: summary_history(),
        })
    })
}

/// A hook that compacts **without changing anything**: it hands back
/// exactly the history it was given.
fn no_change_hook(calls: Arc<AtomicUsize>) -> CompactionHook {
    CompactionHook::new(move |msgs: &[Message]| {
        calls.fetch_add(1, Ordering::SeqCst);
        Some(CompactionResult {
            summary: "[summary]".into(),
            tokens_before: 999_999,
            tokens_after: 10,
            messages: msgs.to_vec(),
        })
    })
}

fn config() -> RunConfig {
    let mut cfg = RunConfig::new("test-model");
    // A window big enough that the *threshold* compaction never fires —
    // every compaction in these tests must come from the overflow path.
    cfg.compaction.context_window = 200_000;
    cfg.compaction.reserve_tokens = 16_384;
    cfg.compaction.keep_recent_tokens = 20_000;
    cfg.max_iterations = 10;
    // The backoff is real but must not make the test slow.
    cfg.retry.base_delay_ms = 1;
    cfg
}

/// The turn the user submits.
///
/// It is deliberately longer than the summary a compaction leaves behind:
/// a compaction that replaces it with something *different* is what lets
/// the agent retry.
fn user_turn() -> nini_core::AgentMessage {
    nini_core::AgentMessage::user(
        "do the long task — the conversation that does not fit into the window",
    )
}

/// Run the agent to the end with a **short** turn, collecting the events
/// and the final error.
async fn drain_with(
    agent: &mut Agent,
    turn: nini_core::AgentMessage,
) -> (Vec<AgentEvent>, Option<String>) {
    use futures_util::StreamExt;
    let stream = agent.run(turn);
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

/// Run the agent to the end, collecting the events and the final error.
async fn drain(agent: &mut Agent) -> (Vec<AgentEvent>, Option<String>) {
    use futures_util::StreamExt;
    let stream = agent.run(user_turn());
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

fn compacted(events: &[AgentEvent]) -> bool {
    events
        .iter()
        .any(|e| matches!(e, AgentEvent::Compacted { .. }))
}

fn entered_compacting(events: &[AgentEvent]) -> bool {
    events.iter().any(|e| {
        matches!(
            e,
            AgentEvent::PhaseChanged(AgentPhase::Compacting { reason, .. }) if reason == "overflow"
        )
    })
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

/// The retry attempts the agent announced.
fn retrying_attempts(events: &[AgentEvent]) -> Vec<u32> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::PhaseChanged(AgentPhase::Retrying { attempt }) => Some(*attempt),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 1. A 400 that says "prompt is too long" must compact and retry
// ---------------------------------------------------------------------------

/// The exact shape a provider produces today: the server's own words are
/// in the message, and the message says the prompt was too long.
#[tokio::test(flavor = "current_thread")]
async fn an_http_400_overflow_compacts_and_retries() {
    let (provider, calls, _seen) = ScriptProvider::new(vec![
        Step::Fail(ProviderError::Api {
            status: 400,
            message: "request failed: 400 Bad Request: prompt is too long: 300000 tokens > 200000"
                .into(),
        }),
        Step::Ok("done".into()),
    ]);
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let mut agent = Agent::new(provider, ToolRegistry::new(), config());
    agent.set_compaction_hook(compacting_hook(hook_calls.clone()));

    let (events, error) = drain(&mut agent).await;

    assert!(
        hook_calls.load(Ordering::SeqCst) == 1,
        "the HTTP 400 never reached the compaction hook — the run died with \
         the history intact ({error:?})"
    );
    assert!(compacted(&events), "no Compacted event: {events:?}");
    assert!(
        entered_compacting(&events),
        "the Compacting phase was never announced: {events:?}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "the request was not retried against the compacted history"
    );
    assert!(
        error.is_none(),
        "the overflow was surfaced as a terminal error instead of recovered: {error:?}"
    );
    assert!(
        !error_messages(&events)
            .iter()
            .any(|m| m.contains("prompt is too long")),
        "the recovered overflow was also reported as an error: {events:?}"
    );
}

// ---------------------------------------------------------------------------
// 2. C2 — the retry must carry a DIFFERENT history
// ---------------------------------------------------------------------------

/// The claim this pins down: the retry is not "the same request, one more
/// time" — it is the **compacted** conversation.
///
/// The test used to assert only that the provider was called twice, and
/// the provider threw the request away. So the run could compact, keep the
/// old history and re-send it, and the test stayed green: with
/// `self.messages = result.messages.clone()` broken, the second request
/// was byte-for-byte the first one.
///
/// The measure is **content**, not size. The summary a compaction leaves
/// behind may be longer than the history it replaces (it is, for a tiny
/// history), and the compaction still worked — so "smaller" is the wrong
/// question, and asserting on it is what broke the small-history case.
#[tokio::test(flavor = "current_thread")]
async fn the_retry_carries_a_different_history() {
    let (provider, calls, seen) = ScriptProvider::new(vec![
        Step::Fail(ProviderError::Api {
            status: 400,
            message: "request failed: 400 Bad Request: prompt is too long: 300000 tokens > 200000"
                .into(),
        }),
        Step::Ok("done".into()),
    ]);
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let mut agent = Agent::new(provider, ToolRegistry::new(), config());
    agent.set_compaction_hook(compacting_hook(hook_calls.clone()));

    let (events, error) = drain(&mut agent).await;

    assert_eq!(calls.load(Ordering::SeqCst), 2, "{events:?}");
    assert!(error.is_none(), "the run ended with {error:?}");

    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2, "the provider was called {seen:?}");
    assert!(
        seen[1] != seen[0],
        "the retry carried the *pre-compaction* history: {seen:?} — the \
         compaction ran but its result never reached the request"
    );
}

/// The other half of the same claim: a compaction that changes **nothing**
/// must not be retried.
///
/// Re-sending an identical oversized request is worse than giving up: it
/// burns another round trip to learn the same thing, and the server may
/// charge for it.
#[tokio::test(flavor = "current_thread")]
async fn a_compaction_that_changes_nothing_ends_the_run() {
    let (provider, calls, seen) = ScriptProvider::new(vec![
        Step::Fail(ProviderError::Api {
            status: 400,
            message: "request failed: 400 Bad Request: prompt is too long: 300000 tokens > 200000"
                .into(),
        }),
        Step::Ok("done".into()),
    ]);
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let mut agent = Agent::new(provider, ToolRegistry::new(), config());
    agent.set_compaction_hook(no_change_hook(hook_calls.clone()));

    let (events, _error) = drain(&mut agent).await;

    assert_eq!(
        hook_calls.load(Ordering::SeqCst),
        1,
        "the compaction hook never ran: {events:?}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the run re-sent the very same request after a compaction that \
         changed nothing: {events:?}"
    );
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "{seen:?}");
    let messages = error_messages(&events);
    assert!(
        messages.iter().any(|m| m.contains("did not change")),
        "the run ended without saying why: {messages:?}"
    );
    assert!(
        messages.iter().any(|m| m.contains("prompt is too long")),
        "the server's own words were lost: {messages:?}"
    );
}

// ---------------------------------------------------------------------------
// 2b. The regression oracle: a SMALL history must still be retried
// ---------------------------------------------------------------------------

/// The bug this pins down: the recovery used to veto the retry when the
/// compaction did not make the history **smaller**.
///
/// That is the wrong criterion. A history of one short turn is five bytes;
/// the summary that replaces it is *longer*; the compaction worked
/// perfectly — and the agent refused to retry, so the whole
/// compact-and-recover path was dead for every small conversation that
/// hit an overflow.
///
/// The criterion that is correct is **"the history changed"**, not "the
/// history shrank".
#[tokio::test(flavor = "current_thread")]
async fn a_small_history_is_still_compacted_and_retried() {
    let (provider, calls, seen) = ScriptProvider::new(vec![
        Step::Fail(ProviderError::Api {
            status: 400,
            message: "request failed: 400 Bad Request: Bad Request (code 10907)".into(),
        }),
        Step::Ok("done".into()),
    ]);
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let mut agent = Agent::new(provider, ToolRegistry::new(), config());
    agent.set_compaction_hook(compacting_hook(hook_calls.clone()));

    // The shortest history imaginable: one five-byte turn.
    let (events, error) = drain_with(&mut agent, nini_core::AgentMessage::user("hello")).await;

    assert_eq!(
        hook_calls.load(Ordering::SeqCst),
        1,
        "the overflow never reached the compaction hook: {events:?}"
    );
    assert!(compacted(&events), "no Compacted event: {events:?}");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "the run refused to retry a small history: {events:?}"
    );
    assert!(error.is_none(), "the run ended with {error:?}: {events:?}");

    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert!(
        seen[1] != seen[0],
        "the retry carried the *pre-compaction* history: {seen:?}"
    );
    // The very shape of the regression: the compacted history is *longer*
    // than the one it replaces, and that must not veto the retry.
    assert!(
        seen[1].len() > seen[0].len(),
        "this test is supposed to cover the 'the compaction grew the \
         history' case: {seen:?}"
    );
    assert!(
        !error_messages(&events)
            .iter()
            .any(|m| m.contains("did not change")),
        "the run refused the retry: {events:?}"
    );
}

// ---------------------------------------------------------------------------
// 3. A vendor that says nothing in prose: iFlytek 10907
// ---------------------------------------------------------------------------

/// Measured 2026-11 against the live endpoint: an over-long prompt comes
/// back as `{"error":{"code":10907,"message":"Bad Request"}}` — "Bad
/// Request" is the whole prose, and it is the same prose a *parameter*
/// error arrives with. Only the code separates them.
#[tokio::test(flavor = "current_thread")]
async fn the_xunfei_overflow_code_compacts_and_retries() {
    let (provider, calls, seen) = ScriptProvider::new(vec![
        Step::Fail(ProviderError::Api {
            status: 400,
            message: "request failed: 400 Bad Request: Bad Request (code 10907)".into(),
        }),
        Step::Ok("done".into()),
    ]);
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let mut agent = Agent::new(provider, ToolRegistry::new(), config());
    agent.set_compaction_hook(compacting_hook(hook_calls.clone()));

    let (events, error) = drain(&mut agent).await;

    assert_eq!(
        hook_calls.load(Ordering::SeqCst),
        1,
        "the vendor's overflow code was not recognised: {events:?}"
    );
    assert!(compacted(&events), "no Compacted event: {events:?}");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "the request was not retried"
    );
    assert!(error.is_none(), "the run ended with {error:?}");
    let seen = seen.lock().unwrap().clone();
    assert!(
        seen.len() == 2 && seen[1] != seen[0],
        "the retry carried the pre-compaction history: {seen:?}"
    );
}

// ---------------------------------------------------------------------------
// 4 + 5. The other iFlytek codes are NOT overflows
// ---------------------------------------------------------------------------

/// `10005` is "invalid request" (参数非法). Compacting there would drop
/// the history for nothing — and the user would never learn why.
#[tokio::test(flavor = "current_thread")]
async fn the_xunfei_parameter_error_is_not_an_overflow() {
    let (provider, calls, _seen) = ScriptProvider::new(vec![Step::Fail(ProviderError::Api {
        status: 400,
        message: "request failed: 400 Bad Request: invalid request (code 10005)".into(),
    })]);
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let mut agent = Agent::new(provider, ToolRegistry::new(), config());
    agent.set_compaction_hook(compacting_hook(hook_calls.clone()));

    let (events, error) = drain(&mut agent).await;

    assert_eq!(
        hook_calls.load(Ordering::SeqCst),
        0,
        "a bad request must not compact the history: {events:?}"
    );
    assert!(!compacted(&events), "a bad request compacted: {events:?}");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "the request was retried");
    let error = error.expect("the request must fail");
    assert!(
        error.contains("invalid request"),
        "the server's own words were lost: {error}"
    );
}

/// `10404` is "No Available Source" (模型不可用) — a model problem, not a
/// context problem.
#[tokio::test(flavor = "current_thread")]
async fn the_xunfei_unavailable_model_is_not_an_overflow() {
    let (provider, _calls, _seen) = ScriptProvider::new(vec![Step::Fail(ProviderError::Api {
        status: 400,
        message: "request failed: 400 Bad Request: No Available Source (code 10404)".into(),
    })]);
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let mut agent = Agent::new(provider, ToolRegistry::new(), config());
    agent.set_compaction_hook(compacting_hook(hook_calls.clone()));

    let (events, _error) = drain(&mut agent).await;

    assert_eq!(
        hook_calls.load(Ordering::SeqCst),
        0,
        "an unavailable model must not compact the history: {events:?}"
    );
    assert!(
        !compacted(&events),
        "an unavailable model compacted: {events:?}"
    );
}

// ---------------------------------------------------------------------------
// 6. C1 — a five-digit number is not a vendor code
// ---------------------------------------------------------------------------

/// `10907` is a number **in the table** (iFlytek's context-overflow code),
/// and a five-digit number is not rare in an error message: it is a
/// request id, a token count, a byte offset.
///
/// The bare-number fallback this replaces read `invalid request id 10907`
/// as the overflow code — so the agent compacted the user's history and
/// re-sent the request, and the server refused it again, and the reason
/// the user finally saw was "recovery failed after one attempt" instead
/// of "invalid request id 10907".
#[tokio::test(flavor = "current_thread")]
async fn a_five_digit_number_in_a_rejection_is_not_an_overflow() {
    for message in [
        "request failed: 400 Bad Request: invalid request id 10907",
        "request failed: 400 Bad Request: max tokens 10907 exceeded",
        "request failed: 400 Bad Request: unknown model gpt-4 (10907)",
    ] {
        let (provider, calls, _seen) = ScriptProvider::new(vec![Step::Fail(ProviderError::Api {
            status: 400,
            message: message.into(),
        })]);
        let hook_calls = Arc::new(AtomicUsize::new(0));
        let mut agent = Agent::new(provider, ToolRegistry::new(), config());
        agent.set_compaction_hook(compacting_hook(hook_calls.clone()));

        let (events, error) = drain(&mut agent).await;

        assert_eq!(
            hook_calls.load(Ordering::SeqCst),
            0,
            "{message:?} is not an overflow and must not compact the history: {events:?}"
        );
        assert!(!compacted(&events), "{message:?} compacted: {events:?}");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "{message:?} was retried: {events:?}"
        );
        let error = error.expect("the request must fail");
        assert!(
            error.contains("10907"),
            "the server's own words were lost: {error}"
        );
    }
}

/// The same claim, at the level of the extractor itself.
#[test]
fn a_five_digit_number_is_not_read_as_a_code() {
    for text in [
        "request failed: 400 Bad Request: invalid request id 10907",
        "request failed: 400 Bad Request: max tokens 10907 exceeded",
        "request failed: 400 Bad Request: unknown model gpt-4 (10907)",
    ] {
        assert_eq!(
            extract_vendor_error_code(text),
            None,
            "{text:?} must not be read as a vendor code"
        );
        let signals = OverflowSignals {
            error_code: extract_vendor_error_code(text),
        };
        assert!(
            !is_context_overflow_with_signals("error", Some(text), None, Some(1000), &signals,),
            "{text:?} is not a context overflow"
        );
    }
}

// ---------------------------------------------------------------------------
// 7. A throttled request is still not an overflow
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn a_rate_limited_request_does_not_compact() {
    let (provider, _calls, _seen) = ScriptProvider::new(vec![Step::Fail(ProviderError::Api {
        status: 429,
        message: "request failed: 429 Too Many Requests: rate limit exceeded".into(),
    })]);
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let mut agent = Agent::new(provider, ToolRegistry::new(), config());
    agent.set_compaction_hook(compacting_hook(hook_calls.clone()));

    let (events, _error) = drain(&mut agent).await;

    assert_eq!(
        hook_calls.load(Ordering::SeqCst),
        0,
        "a rate limit must never compact the history: {events:?}"
    );
    assert!(!compacted(&events), "a rate limit compacted: {events:?}");
}

// ---------------------------------------------------------------------------
// 8. W1 — a 5xx is a broken server, not an oversized request
// ---------------------------------------------------------------------------

/// A 500 whose body says "maximum context length exceeded" is a gateway
/// that quoted another provider's error (or a gateway that is confused).
/// Either way the server is broken, not the conversation: the right
/// response is to try again, and the **wrong** one is to throw the user's
/// history away.
#[tokio::test(flavor = "current_thread")]
async fn a_server_error_is_retried_but_never_compacted() {
    for status in [500u16, 502, 504] {
        let (provider, calls, _seen) = ScriptProvider::new(vec![
            Step::Fail(ProviderError::Api {
                status,
                message: format!(
                    "request failed: {status} Internal Server Error: maximum context length exceeded"
                ),
            }),
            Step::Ok("done".into()),
        ]);
        let hook_calls = Arc::new(AtomicUsize::new(0));
        let mut agent = Agent::new(provider, ToolRegistry::new(), config());
        agent.set_compaction_hook(compacting_hook(hook_calls.clone()));

        let (events, error) = drain(&mut agent).await;

        assert_eq!(
            hook_calls.load(Ordering::SeqCst),
            0,
            "a {status} compacted the history: {events:?}"
        );
        assert!(!compacted(&events), "a {status} compacted: {events:?}");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "a {status} was not retried: {events:?}"
        );
        assert_eq!(
            retrying_attempts(&events),
            vec![1],
            "a {status}: {events:?}"
        );
        assert!(error.is_none(), "a {status} ended the run: {error:?}");
    }
}

/// The same, with a **vendor code** in the body: the code says "overflow"
/// in the table, but a 500 that carries it is still a broken server.
#[tokio::test(flavor = "current_thread")]
async fn a_server_error_with_a_vendor_code_is_never_compacted() {
    let (provider, calls, _seen) = ScriptProvider::new(vec![
        Step::Fail(ProviderError::Api {
            status: 500,
            message: "request failed: 500 Internal Server Error: Bad Request (code 10907)".into(),
        }),
        Step::Ok("done".into()),
    ]);
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let mut agent = Agent::new(provider, ToolRegistry::new(), config());
    agent.set_compaction_hook(compacting_hook(hook_calls.clone()));

    let (events, error) = drain(&mut agent).await;

    assert_eq!(
        hook_calls.load(Ordering::SeqCst),
        0,
        "a 500 compacted the history: {events:?}"
    );
    assert!(!compacted(&events), "a 500 compacted: {events:?}");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "a 500 was not retried: {events:?}"
    );
    assert!(error.is_none(), "the run ended with {error:?}");
}

/// A 4xx that is neither 400 nor 413 is the client's own fault: it is
/// reported, not compacted, not retried.
#[tokio::test(flavor = "current_thread")]
async fn a_403_is_not_an_overflow_and_is_not_retried() {
    let (provider, calls, _seen) = ScriptProvider::new(vec![Step::Fail(ProviderError::Api {
        status: 403,
        message: "request failed: 403 Forbidden: context window access denied".into(),
    })]);
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let mut agent = Agent::new(provider, ToolRegistry::new(), config());
    agent.set_compaction_hook(compacting_hook(hook_calls.clone()));

    let (events, error) = drain(&mut agent).await;

    assert_eq!(
        hook_calls.load(Ordering::SeqCst),
        0,
        "a 403 compacted the history: {events:?}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "a 403 was retried: {events:?}"
    );
    assert!(error.is_some(), "the request must fail");
}

// ---------------------------------------------------------------------------
// 9. A rejected request tells the user what to do
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn a_rejected_request_keeps_the_server_words_and_gains_a_hint() {
    let (provider, _calls, _seen) = ScriptProvider::new(vec![Step::Fail(ProviderError::Api {
        status: 400,
        message: "request failed: 400 Bad Request: invalid request: unknown field `foo`".into(),
    })]);
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let mut agent = Agent::new(provider, ToolRegistry::new(), config());
    agent.set_compaction_hook(compacting_hook(hook_calls.clone()));

    let (events, error) = drain(&mut agent).await;

    let error = error.expect("the request must fail");
    assert!(
        error.contains("unknown field `foo`"),
        "the server's own words were lost: {error}"
    );
    assert!(
        error.contains("/compact"),
        "the error says nothing about what to do next: {error}"
    );
    assert!(
        error.to_lowercase().contains("may"),
        "the hint asserts a cause instead of naming a possibility: {error}"
    );
    // The hint must not turn the next failure into an overflow: it may not
    // contain any of the words the detector matches on.
    let hint_tail = error.split("/compact").next().unwrap_or("");
    for pattern in [
        "context length",
        "context_length",
        "context window",
        "too large",
        "maximum context",
        "prompt is too long",
        "input too long",
        "string too long",
        "reduce the length",
    ] {
        assert!(
            !hint_tail.to_lowercase().contains(pattern),
            "the hint contains the detector's pattern {pattern:?}: {error}"
        );
    }
    // W4: the rejection is reported **once**. The CLI pushes both the
    // `AgentEvent::Error` and the terminal error into the same sink, so
    // reporting it twice showed the identical sentence twice.
    assert!(
        error_messages(&events).is_empty(),
        "the rejection was reported twice: {events:?}"
    );
}

// ---------------------------------------------------------------------------
// 10. The recovery is still attempted exactly once
// ---------------------------------------------------------------------------

/// Pi bounds the recovery at one compact-and-retry
/// (`agent-session.ts:2172`). Without the bound, a compaction that cannot
/// shrink the context re-sends the identical oversized request until the
/// iteration cap trips.
#[tokio::test(flavor = "current_thread")]
async fn the_recovery_is_attempted_once() {
    let overflow = || {
        Step::Fail(ProviderError::Api {
            status: 400,
            message: "request failed: 400 Bad Request: prompt is too long: 300000 tokens > 200000"
                .into(),
        })
    };
    let (provider, calls, _seen) = ScriptProvider::new(vec![overflow(), overflow(), overflow()]);
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let mut agent = Agent::new(provider, ToolRegistry::new(), config());
    agent.set_compaction_hook(compacting_hook(hook_calls.clone()));

    let (events, _error) = drain(&mut agent).await;

    assert_eq!(
        hook_calls.load(Ordering::SeqCst),
        1,
        "the recovery ran more than once: {events:?}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "the run kept retrying the oversized request"
    );
    let messages = error_messages(&events);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("recovery failed after one compact-and-retry attempt")),
        "the second overflow produced no explanation: {messages:?}"
    );
}

/// W3: the failure of the recovery must not throw away the server's own
/// words.
///
/// The fixed sentence ("…try a larger-context model") says nothing about
/// *why* the request was refused. For iFlytek the difference between
/// "your context is too long" and "your request was malformed" is a
/// number — and the number was the only thing in the message.
#[tokio::test(flavor = "current_thread")]
async fn the_failed_recovery_keeps_the_servers_words() {
    let overflow = || {
        Step::Fail(ProviderError::Api {
            status: 400,
            message: "request failed: 400 Bad Request: Bad Request (code 10907)".into(),
        })
    };
    let (provider, _calls, _seen) = ScriptProvider::new(vec![overflow(), overflow(), overflow()]);
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let mut agent = Agent::new(provider, ToolRegistry::new(), config());
    agent.set_compaction_hook(compacting_hook(hook_calls.clone()));

    let (events, _error) = drain(&mut agent).await;

    let messages = error_messages(&events);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("recovery failed after one compact-and-retry attempt")),
        "the second overflow produced no explanation: {messages:?}"
    );
    assert!(
        messages.iter().any(|m| m.contains("10907")),
        "the server's own words were thrown away: {messages:?}"
    );
}

// ---------------------------------------------------------------------------
// 11. The detector itself
// ---------------------------------------------------------------------------

/// The vendor-code channel, on its own.
#[test]
fn the_vendor_code_channel_reads_the_xunfei_codes() {
    let win = Some(1000u32);
    let overflow = OverflowSignals {
        error_code: Some("10907".into()),
    };
    assert!(
        is_context_overflow_with_signals(
            "error",
            Some("request failed: 400 Bad Request: Bad Request"),
            None,
            win,
            &overflow,
        ),
        "10907 is the measured context-overflow code and must be one"
    );
    for code in ["10005", "10404"] {
        let signals = OverflowSignals {
            error_code: Some(code.into()),
        };
        assert!(
            !is_context_overflow_with_signals(
                "error",
                Some("request failed: 400 Bad Request: Bad Request"),
                None,
                win,
                &signals,
            ),
            "{code} is not an overflow and must not compact the history"
        );
    }
}

/// The four-argument signature every existing call site uses keeps its
/// behaviour: no vendor code, no code channel.
#[test]
fn the_old_detector_signature_is_unchanged() {
    let win = Some(1000u32);
    assert!(
        is_context_overflow(
            "error",
            Some("prompt is too long: 300000 tokens > 200000"),
            None,
            win
        ),
        "the text case must still fire"
    );
    assert!(
        !is_context_overflow("error", Some("rate limit exceeded"), None, win),
        "the non-overflow exclusions must still hold"
    );
    assert!(
        !is_context_overflow("error", Some("invalid request"), None, win),
        "an ordinary error must not be an overflow"
    );
    // Pi's other two cases are untouched.
    let usage = nini_core::provider::Usage {
        input_tokens: 1200,
        output_tokens: 4,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
    };
    assert!(
        is_context_overflow("end_turn", None, Some(&usage), win),
        "the silent-overflow case must still fire"
    );
}

/// The table itself, so a wrong row is caught here and not in production.
#[test]
fn the_vendor_code_table_says_what_it_means() {
    assert_eq!(
        lookup_vendor_code("10907"),
        Some(VendorCodeVerdict::Overflow)
    );
    assert_eq!(
        lookup_vendor_code("10005"),
        Some(VendorCodeVerdict::NotOverflow)
    );
    assert_eq!(
        lookup_vendor_code("10404"),
        Some(VendorCodeVerdict::NotOverflow)
    );
    assert_eq!(lookup_vendor_code("123456"), None);
}
