//! v0.12: a transient HTTP failure must be **retried**.
//!
//! ## The defect this pins down
//!
//! The agent loop has three error exits, and only one of them had a
//! retry:
//!
//! | exit | overflow check | transient retry |
//! |---|---|---|
//! | `Ok(StreamEvent::MessageStop { .. })` | yes | — |
//! | `Ok(StreamEvent::Error { message })` | yes | yes |
//! | `Err(ProviderError::Api { .. })` | yes | **no** |
//!
//! Every provider turns a non-2xx response into
//! `ProviderError::Api { status, message }`, so the **third** row is where
//! every real HTTP failure arrives. A 429/500/502/503/504/529 therefore
//! died on the very first attempt, even though this very file documents
//! that a 529 is "exactly as transient as a 503".
//!
//! ## The judgement (status × text)
//!
//! | status | text | retried? |
//! |---|---|---|
//! | 5xx (incl. 529), 429, 408 | anything, **including an empty body** | yes |
//! | 4xx other than 429/408 | anything, **including "timeout"** | no |
//! | unknown (`0`) | `is_retryable_error` decides | depends |
//!
//! The status is the server's own verdict and it wins: a 503 with an
//! empty body is still a 503, and a 400 that happens to mention an
//! upstream timeout is still a request the client has to fix.
//!
//! ## What is asserted here
//!
//! * a 503 with an **empty body** is retried (the text alone would say
//!   "no");
//! * a 429 and a 529 are retried;
//! * a 400 is **not** retried, and neither is a 400 whose body says
//!   "timeout" — the text must not be running on its own;
//! * the retries are capped by `retry.max_retries`;
//! * a successful retry resets the counter.

use nini_core::agent::AgentPhase;
use nini_core::agent::{is_transient_provider_error, is_transient_status, Agent, RunConfig};
use nini_core::provider::{Capabilities, Provider, ProviderError, Request, StreamEvent, Usage};
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
    /// failure looks like from the agent's side.
    Fail(ProviderError),
    /// The request succeeds.
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

fn config() -> RunConfig {
    let mut cfg = RunConfig::new("test-model");
    // A window big enough that the *threshold* compaction never fires —
    // nothing here may compact.
    cfg.compaction.context_window = 200_000;
    cfg.compaction.reserve_tokens = 16_384;
    cfg.compaction.keep_recent_tokens = 20_000;
    cfg.max_iterations = 10;
    // The backoff is real but must not make the test slow.
    cfg.retry.base_delay_ms = 1;
    cfg
}

/// Run the agent to the end, collecting the events and the final error.
async fn drain(agent: &mut Agent) -> (Vec<AgentEvent>, Option<String>) {
    use futures_util::StreamExt;
    let stream = agent.run(nini_core::AgentMessage::user("do the task"));
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

/// The retry attempts announced by the agent.
fn retrying_attempts(events: &[AgentEvent]) -> Vec<u32> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::PhaseChanged(AgentPhase::Retrying { attempt }) => Some(*attempt),
            _ => None,
        })
        .collect()
}

fn assistant_text(events: &[AgentEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TextDelta { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 1. A 503 with an empty body is retried
// ---------------------------------------------------------------------------

/// The body is **empty**: nothing in the text says "retry me", so this
/// can only pass if the *status* is what decides. This is the shape most
/// gateways produce when they are saturated.
#[tokio::test(flavor = "current_thread")]
async fn an_http_503_with_an_empty_body_is_retried() {
    let (provider, calls) = ScriptProvider::new(vec![
        Step::Fail(ProviderError::Api {
            status: 503,
            message: String::new(),
        }),
        Step::Ok("done".into()),
    ]);
    let mut agent = Agent::new(provider, ToolRegistry::new(), config());

    let (events, error) = drain(&mut agent).await;

    assert!(
        calls.load(Ordering::SeqCst) >= 2,
        "a 503 was not retried — the provider was called {} time(s): {events:?}",
        calls.load(Ordering::SeqCst)
    );
    assert_eq!(
        retrying_attempts(&events),
        vec![1],
        "the Retrying phase was never announced: {events:?}"
    );
    assert!(error.is_none(), "the run ended with {error:?}");
    assert_eq!(assistant_text(&events), "done");
}

/// The body is in Chinese and says nothing the text heuristics know. The
/// status still decides.
#[tokio::test(flavor = "current_thread")]
async fn an_http_503_with_a_foreign_body_is_retried() {
    let (provider, calls) = ScriptProvider::new(vec![
        Step::Fail(ProviderError::Api {
            status: 503,
            message: "网关暂时不可用".into(),
        }),
        Step::Ok("done".into()),
    ]);
    let mut agent = Agent::new(provider, ToolRegistry::new(), config());

    let (events, error) = drain(&mut agent).await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "a 503 was not retried: {events:?}"
    );
    assert_eq!(retrying_attempts(&events), vec![1]);
    assert!(error.is_none(), "the run ended with {error:?}");
}

// ---------------------------------------------------------------------------
// 2 + 3. A 429 and a 529 are retried
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn an_http_429_is_retried() {
    let (provider, calls) = ScriptProvider::new(vec![
        Step::Fail(ProviderError::Api {
            status: 429,
            message: "request failed: 429 Too Many Requests: rate limit exceeded".into(),
        }),
        Step::Ok("done".into()),
    ]);
    let mut agent = Agent::new(provider, ToolRegistry::new(), config());

    let (events, error) = drain(&mut agent).await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "a 429 was not retried: {events:?}"
    );
    assert_eq!(retrying_attempts(&events), vec![1]);
    assert!(error.is_none(), "the run ended with {error:?}");
}

/// 529 is what MiniMax's gateway returns while a model is saturated —
/// exactly as transient as a 503.
#[tokio::test(flavor = "current_thread")]
async fn an_http_529_is_retried() {
    let (provider, calls) = ScriptProvider::new(vec![
        Step::Fail(ProviderError::Api {
            status: 529,
            message: "request failed: 529".into(),
        }),
        Step::Ok("done".into()),
    ]);
    let mut agent = Agent::new(provider, ToolRegistry::new(), config());

    let (events, error) = drain(&mut agent).await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "a 529 was not retried: {events:?}"
    );
    assert_eq!(retrying_attempts(&events), vec![1]);
    assert!(error.is_none(), "the run ended with {error:?}");
}

// ---------------------------------------------------------------------------
// 4 + 5. A 400 is NOT retried
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn an_http_400_is_not_retried() {
    let (provider, calls) = ScriptProvider::new(vec![Step::Fail(ProviderError::Api {
        status: 400,
        message: "request failed: 400 Bad Request: invalid request: unknown field `foo`".into(),
    })]);
    let mut agent = Agent::new(provider, ToolRegistry::new(), config());

    let (events, error) = drain(&mut agent).await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "a 400 was retried: {events:?}"
    );
    assert!(
        retrying_attempts(&events).is_empty(),
        "a 400 entered the Retrying phase: {events:?}"
    );
    let error = error.expect("the request must fail");
    assert!(
        error.contains("unknown field `foo`"),
        "the server's own words were lost: {error}"
    );
}

/// The body says "timeout" — the text heuristics would say "retry" — and
/// the status says "your request is wrong". The status wins.
#[tokio::test(flavor = "current_thread")]
async fn an_http_400_that_mentions_a_timeout_is_not_retried() {
    let (provider, calls) = ScriptProvider::new(vec![Step::Fail(ProviderError::Api {
        status: 400,
        message: "request failed: 400 Bad Request: upstream timeout while parsing the request"
            .into(),
    })]);
    let mut agent = Agent::new(provider, ToolRegistry::new(), config());

    let (events, error) = drain(&mut agent).await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "a 400 was retried because its body said \"timeout\": {events:?}"
    );
    assert!(
        retrying_attempts(&events).is_empty(),
        "a 400 entered the Retrying phase: {events:?}"
    );
    assert!(error.is_some(), "the request must fail");
}

// ---------------------------------------------------------------------------
// 6. The retries are capped
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn the_retries_are_capped_by_max_retries() {
    let mut cfg = config();
    cfg.retry.max_retries = 2;
    let fail = || {
        Step::Fail(ProviderError::Api {
            status: 503,
            message: "request failed: 503 Service Unavailable".into(),
        })
    };
    let (provider, calls) = ScriptProvider::new(vec![fail(), fail(), fail(), fail(), fail()]);
    let mut agent = Agent::new(provider, ToolRegistry::new(), cfg);

    let (events, error) = drain(&mut agent).await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "the retries were not capped at max_retries (1 attempt + 2 retries \
         expected): {events:?}"
    );
    assert_eq!(
        retrying_attempts(&events),
        vec![1, 2],
        "the announced attempts do not match the cap: {events:?}"
    );
    assert!(error.is_some(), "the run must end with an error");
}

// ---------------------------------------------------------------------------
// 7. A successful retry resets the counter
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn a_successful_retry_resets_the_retry_counter() {
    let (provider, calls) = ScriptProvider::new(vec![
        Step::Fail(ProviderError::Api {
            status: 503,
            message: "request failed: 503 Service Unavailable".into(),
        }),
        Step::Ok("done".into()),
    ]);
    let mut agent = Agent::new(provider, ToolRegistry::new(), config());

    let (events, error) = drain(&mut agent).await;

    assert_eq!(calls.load(Ordering::SeqCst), 2, "{events:?}");
    assert!(error.is_none(), "the run ended with {error:?}");
    assert_eq!(assistant_text(&events), "done");
    assert_eq!(
        agent.retry_attempt(),
        0,
        "the retry counter was not reset by the successful turn — a later \
         error would never be retried"
    );
}

// ---------------------------------------------------------------------------
// 8. The table itself
// ---------------------------------------------------------------------------

/// The judgement table, so a wrong row is caught here and not in
/// production.
#[test]
fn the_transient_table_is_what_it_says() {
    // Transient statuses — retried whatever the body says.
    for status in [408u16, 429, 500, 502, 503, 504, 529] {
        assert!(
            is_transient_status(status),
            "{status} is transient and must be retried"
        );
        assert!(
            is_transient_provider_error(status, ""),
            "{status} with an empty body must still be retried"
        );
        assert!(
            is_transient_provider_error(status, "服务暂时不可用"),
            "{status} with a foreign body must still be retried"
        );
    }

    // Client errors — never retried, not even when the body says
    // "timeout".
    for status in [400u16, 401, 403, 404, 422] {
        assert!(
            !is_transient_status(status),
            "{status} is a client error and must not be retried"
        );
        assert!(
            !is_transient_provider_error(status, "upstream timeout"),
            "{status} must not be retried because its body says \"timeout\""
        );
    }

    // No status at all: the text decides (a network-level failure, or an
    // SSE error that never carried an HTTP code).
    assert!(is_transient_provider_error(0, "connection reset by peer"));
    assert!(!is_transient_provider_error(0, "invalid api key"));
}
