//! v1.0.3: a **transport-level** provider failure (DNS / connect / TLS /
//! timeout) must be retried, and the error must say what actually broke.
//!
//! ## The defect this pins down
//!
//! One network blip against
//! `https://maas-api.cn-huabei-1.xf-yun.com/v2/chat/completions` killed a
//! whole turn with:
//!
//! ```text
//! [error] provider error: transport error: error sending request for url (https://…)
//! ```
//!
//! Two things are wrong with that:
//!
//! 1. **It was not retried.** `is_retryable_error` only knows the
//!    *server's* words ("rate limit", "503", …). A reqwest transport
//!    failure says "error sending request for url", which matches none of
//!    them, so the agent gave up on the first attempt — even though a
//!    transport failure is the most transient failure there is. Pi retries
//!    network errors
//!    (`references/pi-0.85.1/src/api/openai-codex-responses.ts:441` —
//!    "Network errors are retryable"), and nini's own fallback chain
//!    already treats `ProviderError::Http` as retryable
//!    (`crates/nini-ai/src/fallback.rs:122`). The agent loop was the only
//!    place that disagreed.
//!
//! 2. **The cause was thrown away.** `From<reqwest::Error>` keeps only
//!    `e.to_string()`, and reqwest's own `Display` for a request error is
//!    "error sending request for url (…)" — the `source` chain (dns error
//!    / connection refused / operation timed out) never reaches the user.
//!    The sentence names no cause at all.
//!
//! ## The rules this pins down
//!
//! * a transport failure is classified as transient (so the agent's
//!   retry path picks it up);
//! * a transport failure is **retried**, and the turn completes when the
//!   network comes back;
//! * a retry happens only **before the first output** — a stream that
//!   already produced text is not replayed, because the text that
//!   already reached the user cannot be taken back;
//! * the error the user finally sees carries the **cause**, and the
//!   cause is never what the length cap cuts;
//! * a transport failure says "transport error", not "http error" —
//!   there was no HTTP response at all;
//! * a *permanent* failure (an illegal URL, a redirect loop) is not
//!   retried — the request never left, so sending it again would only
//!   produce the same failure;
//! * a request the server refused (401) is still *not* retried — the
//!   retry must not become "retry everything";
//! * the fallback chain's own "gave up" verdict has its **own**
//!   variant (`ChainExhausted`): it is transient only when *every*
//!   failure along the chain was transient — the numbers quoted inside
//!   the summary decide nothing.

use nini_core::agent::{
    is_transient_provider_error, is_transient_provider_failure, Agent, RunConfig,
};
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

#[derive(Clone, Debug)]
enum Step {
    /// Fail this call (and only this call).
    Fail(ProviderError),
    /// Fail **every** call with the same error — the "the network never
    /// comes back" case.
    FailForever(ProviderError),
    /// Emit a piece of text, then fail: the mid-stream break.
    TextThenFail(String, ProviderError),
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
        let mut queue = self.steps.lock().unwrap();
        let step = queue.pop_front();
        // `FailForever` is put back: every call fails the same way.
        if let Some(Step::FailForever(e)) = &step {
            queue.push_back(Step::FailForever(e.clone()));
        }
        let events: Vec<Result<StreamEvent, ProviderError>> = match step {
            Some(Step::Fail(e)) => vec![Err(e)],
            Some(Step::FailForever(e)) => vec![Err(e)],
            Some(Step::TextThenFail(text, e)) => {
                vec![Ok(StreamEvent::TextDelta { text }), Err(e)]
            }
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
    cfg.compaction.context_window = 200_000;
    cfg.compaction.reserve_tokens = 16_384;
    cfg.compaction.keep_recent_tokens = 20_000;
    cfg.max_iterations = 10;
    cfg.retry.base_delay_ms = 1;
    cfg
}

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

/// The transport failure reqwest produces when the request never got an
/// HTTP response — the exact shape of the failure in the screenshot.
fn transport_failure() -> ProviderError {
    ProviderError::Http(
        "error sending request for url (https://maas-api.cn-huabei-1.xf-yun.com/v2/chat/completions): dns error: failed to lookup address information: Name or service not known"
            .into(),
    )
}

// ---------------------------------------------------------------------------
// 1. A transport failure is transient
// ---------------------------------------------------------------------------

#[test]
fn a_transport_failure_is_classified_as_transient() {
    // The path that actually runs: the error *object* decides.
    assert!(
        is_transient_provider_failure(&transport_failure()),
        "a transport failure must be transient — it is the most transient \
         failure there is; got: {}",
        transport_failure()
    );

    // The text path (no HTTP status available) must agree.
    assert!(
        is_transient_provider_error(
            0,
            "transport error: error sending request for url (https://x)"
        ),
        "a transport failure that arrived as text must be transient too"
    );
}

// ---------------------------------------------------------------------------
// 2. A transport failure is retried
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn a_transport_failure_is_retried() {
    let (provider, calls) = ScriptProvider::new(vec![
        Step::Fail(transport_failure()),
        Step::Ok("done".into()),
    ]);
    let mut agent = Agent::new(provider, ToolRegistry::new(), config());

    let (events, error) = drain(&mut agent).await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "a transport failure was not retried: {events:?}"
    );
    assert!(error.is_none(), "the run ended with {error:?}");
}

// ---------------------------------------------------------------------------
// 3. The error keeps the cause
// ---------------------------------------------------------------------------

/// A real reqwest error — a connection to a closed port on localhost.
/// The cause ("connection refused") must survive into the message the
/// user reads; "error sending request for url" alone names no cause.
///
/// The timeout is not decoration: on a host that DROPs packets to
/// 127.0.0.1:1 the connection would hang forever.
#[tokio::test(flavor = "current_thread")]
async fn a_transport_error_keeps_the_cause() {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap();
    let err = client
        .get("http://127.0.0.1:1/never")
        .send()
        .await
        .expect_err("connecting to a closed port must fail");
    let message = ProviderError::from(err).to_string();

    assert!(
        message.contains("connection refused")
            || message.contains("refused")
            || message.contains("timed out"),
        "the cause was dropped from the transport error: {message}"
    );
    assert!(
        message.contains("error sending request for url"),
        "the request context was dropped from the transport error: {message}"
    );
    // A connection failure is a *transport* failure: the variant must
    // stay `Http`, and the prefix must not claim there was an HTTP
    // response — there was none.
    let err = client
        .get("http://127.0.0.1:1/never")
        .send()
        .await
        .expect_err("connecting to a closed port must fail");
    let err = ProviderError::from(err);
    assert!(
        matches!(err, ProviderError::Http(_)),
        "a connection failure must stay `Http`: {err:?}"
    );
    assert!(
        err.to_string().starts_with("transport error:"),
        "a transport failure must not be called an http error: {err}"
    );
    assert!(
        is_transient_provider_failure(&err),
        "a connection failure must be transient: {err}"
    );
}

// ---------------------------------------------------------------------------
// 3b. A request that never left is permanent
// ---------------------------------------------------------------------------

/// An illegal URL never reaches the network: the failure is a
/// configuration error, so sending it again would only produce the
/// same error after the same 2 + 4 + 8 seconds of waiting (Pi's
/// `baseDelayMs: 2000`). It must
/// be classified as `InvalidRequest` (not retried) — and it must say
/// "invalid request", not "transport error", because no transport was
/// ever involved.
#[tokio::test(flavor = "current_thread")]
async fn an_illegal_url_is_permanent_and_not_retried() {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap();
    let err = client
        .get("not a url")
        .send()
        .await
        .expect_err("an illegal URL must fail");
    let err = ProviderError::from(err);

    assert!(
        matches!(err, ProviderError::InvalidRequest(_)),
        "an illegal URL must be `InvalidRequest`, not a retryable failure: {err:?}"
    );
    assert!(
        !is_transient_provider_failure(&err),
        "an illegal URL must not be transient: {err}"
    );
    assert!(
        err.to_string().starts_with("invalid request:"),
        "the permanent failure must say so: {err}"
    );

    // The classification alone is not the claim: the **agent** must not
    // replay the request either. One call, no Retrying phase, and the
    // error the user finally reads says what broke.
    let (provider, calls) = ScriptProvider::new(vec![Step::FailForever(err.clone())]);
    let mut agent = Agent::new(provider, ToolRegistry::new(), config());

    let (events, error) = drain(&mut agent).await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "an illegal URL was retried: {events:?}"
    );
    assert!(
        !events.iter().any(|ev| matches!(
            ev,
            AgentEvent::PhaseChanged(nini_core::agent::AgentPhase::Retrying { .. })
        )),
        "the run entered the Retrying phase: {events:?}"
    );
    let error = error.expect("the turn must fail");
    assert!(
        error.contains("invalid request"),
        "the permanent failure must say so: {error}"
    );
    assert!(
        error.contains("builder error"),
        "the reason must survive: {error}"
    );
}

/// A permanent failure whose **text** looks transient must not be
/// retried either.
///
/// The text heuristic matches "500"/"503" anywhere in the message, and
/// a request that never left carries such a number in its own text —
/// a redirect loop to "…/deploy-503" is the shape that used to be
/// mistaken for a transient failure, and the agent then sat out the
/// whole backoff (2 s + 4 s + 8 s) before the same permanent failure
/// came back. The **kind** decides, not the text.
#[tokio::test(flavor = "current_thread")]
async fn an_invalid_request_that_looks_transient_in_its_text_is_not_retried() {
    let err = ProviderError::InvalidRequest(
        "error following redirect for url (https://api.example.com/deploy-503): \
         too many redirects"
            .into(),
    );
    assert!(
        !is_transient_provider_failure(&err),
        "a request that never left must not be transient: {err}"
    );

    let (provider, calls) = ScriptProvider::new(vec![Step::FailForever(err.clone())]);
    let mut agent = Agent::new(provider, ToolRegistry::new(), config());

    let (events, error) = drain(&mut agent).await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "a permanent failure was retried: {events:?}"
    );
    assert!(
        !events.iter().any(|ev| matches!(
            ev,
            AgentEvent::PhaseChanged(nini_core::agent::AgentPhase::Retrying { .. })
        )),
        "the run entered the Retrying phase: {events:?}"
    );
    let error = error.expect("the turn must fail");
    assert!(
        error.contains("invalid request"),
        "the permanent failure must say so: {error}"
    );
}

// ---------------------------------------------------------------------------
// 4. A refused request is still not retried
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn a_refused_request_is_not_retried() {
    let (provider, calls) = ScriptProvider::new(vec![Step::Fail(ProviderError::Api {
        status: 401,
        message: "request failed: 401 Unauthorized: invalid api key".into(),
    })]);
    let mut agent = Agent::new(provider, ToolRegistry::new(), config());

    let (events, error) = drain(&mut agent).await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "a 401 was retried: {events:?}"
    );
    let error = error.expect("the request must fail");
    assert!(
        error.contains("invalid api key"),
        "the server's own words were lost: {error}"
    );
}

// ---------------------------------------------------------------------------
// 5. A long URL may be cut — the cause may not
// ---------------------------------------------------------------------------

/// The 512-character cap must never eat the reason. The head (the URL)
/// is the part that may be shortened; the cause is what the reader
/// needs.
#[tokio::test(flavor = "current_thread")]
async fn a_long_url_does_not_cut_the_cause() {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap();
    let url = format!("http://127.0.0.1:1/{}", "a".repeat(600));
    let err = client
        .get(&url)
        .send()
        .await
        .expect_err("connecting to a closed port must fail");
    let message = ProviderError::from(err).to_string();

    assert!(
        message.chars().count() <= 512,
        "the message is longer than the cap: {} chars",
        message.chars().count()
    );
    assert!(
        message.contains("connection refused")
            || message.contains("refused")
            || message.contains("timed out"),
        "the cause was cut off by the cap: {message}"
    );
}

// ---------------------------------------------------------------------------
// 6. A mid-stream break is NOT retried
// ---------------------------------------------------------------------------

/// The stream already produced text — that text is out in the world and
/// in the session file. Replaying the request would show it twice, so
/// the turn must fail instead of retrying.
#[tokio::test(flavor = "current_thread")]
async fn a_mid_stream_failure_is_not_retried() {
    let (provider, calls) = ScriptProvider::new(vec![Step::TextThenFail(
        "half a sentence".into(),
        transport_failure(),
    )]);
    let mut agent = Agent::new(provider, ToolRegistry::new(), config());

    let (events, error) = drain(&mut agent).await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "a stream that already produced output was replayed: {events:?}"
    );
    assert!(
        !events.iter().any(|ev| matches!(
            ev,
            AgentEvent::PhaseChanged(nini_core::agent::AgentPhase::Retrying { .. })
        )),
        "the run entered the Retrying phase after producing output: {events:?}"
    );
    let error = error.expect("the turn must fail");
    assert!(
        error.contains("error sending request for url"),
        "the transport error was lost: {error}"
    );
}

// ---------------------------------------------------------------------------
// 7. An exhausted retry budget still names the cause
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn an_exhausted_retry_budget_still_names_the_cause() {
    let mut cfg = config();
    cfg.retry.max_retries = 2;
    let (provider, calls) = ScriptProvider::new(vec![Step::FailForever(transport_failure())]);
    let mut agent = Agent::new(provider, ToolRegistry::new(), cfg);

    let (events, error) = drain(&mut agent).await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "the retry budget was not honoured (max_retries + 1 calls): {events:?}"
    );
    let error = error.expect("the run must end with an error");
    assert!(
        error.contains("error sending request for url"),
        "the request context was lost: {error}"
    );
    assert!(error.contains("dns error"), "the cause was lost: {error}");
}

// ---------------------------------------------------------------------------
// 8. The truth table of the classifier
// ---------------------------------------------------------------------------

#[test]
fn the_transient_classifier_truth_table() {
    // Transport failures: no HTTP response at all → transient.
    // (`Http` keeps this meaning to itself — the fallback chain's "gave
    // up" verdict has its own variant, see below.)
    assert!(is_transient_provider_failure(&ProviderError::Http(
        "error sending request for url (https://x)".into()
    )));
    assert!(is_transient_provider_failure(&ProviderError::Io(
        "connection reset by peer".into()
    )));

    // The server's own verdict decides for `Api`.
    assert!(is_transient_provider_failure(&ProviderError::Api {
        status: 503,
        message: "service unavailable".into(),
    }));
    assert!(is_transient_provider_failure(&ProviderError::Api {
        status: 429,
        message: "too many requests".into(),
    }));
    assert!(!is_transient_provider_failure(&ProviderError::Api {
        status: 400,
        message: "upstream timeout while reading the request".into(),
    }));
    assert!(!is_transient_provider_failure(&ProviderError::Api {
        status: 401,
        message: "unauthorized".into(),
    }));

    // The other variants keep the text heuristic (no HTTP status).
    assert!(is_transient_provider_failure(&ProviderError::Sse(
        "503 service unavailable".into()
    )));
    assert!(!is_transient_provider_failure(&ProviderError::Sse(
        "the stream ended early".into()
    )));
}

// ---------------------------------------------------------------------------
// 9. The fallback chain's own verdict
// ---------------------------------------------------------------------------

/// The chain's verdict is a variant of its own.
///
/// `Http` says "the request never produced a response"; "the whole chain
/// gave up" is a different fact with a different consequence — replay
/// the chain or do not. The chain carries that consequence in its
/// `transient` flag, and the message only carries the diagnostics.
#[test]
fn a_chain_exhaustion_is_transient_only_when_every_failure_was() {
    let transient = ProviderError::ChainExhausted {
        message: "all 2 fallback providers exhausted: openai: HTTP 500: internal; \
                  backup: HTTP 503: service unavailable"
            .into(),
        transient: true,
    };
    assert!(
        is_transient_provider_failure(&transient),
        "a chain whose every failure was transient must be replayed: {transient}"
    );

    // The message quotes a "503" on purpose: the **flag** decides, not
    // the text — a chain that hit a permanent failure must not be
    // replayed just because some quoted error looks transient.
    let permanent_message = "all 2 fallback providers exhausted: openai: HTTP 400: Bad Request \
                             (upstream said 503); backup: HTTP 401: unauthorized"
        .to_string();
    let permanent = ProviderError::ChainExhausted {
        message: permanent_message.clone(),
        transient: false,
    };
    assert!(
        !is_transient_provider_failure(&permanent),
        "a chain that hit a permanent failure must not be replayed: {permanent}"
    );

    // The summary is the whole message — no prefix hides it.
    assert_eq!(permanent.to_string(), permanent_message);
}
