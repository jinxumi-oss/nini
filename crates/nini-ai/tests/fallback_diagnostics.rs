//! `3bf4815d` — the "all fallback providers exhausted" error must say **which**
//! key failed and **how**, and a provider that answers 200 with zero events
//! must be named rather than swallowed.
//!
//! # Why this file exists
//!
//! `3bf4815d` added exactly this: a per-provider `failures` summary rendered
//! into the final error ("anthropic: HTTP 401; anthropic: empty stream
//! (0 events)"). It shipped with **no test at all** — its own message admits
//! it: "823 → 823 (no regression)". Nothing checked the string, so nothing
//! noticed that the summary was unreachable.
//!
//! It was unreachable, not merely untested. Control flow in `stream()`:
//!
//! ```text
//! for provider in providers {
//!     ... on Err(e): if is_retryable(e) && !had_event { provider_failed_retryably = true }
//!     if provider_failed_retryably { continue }        // ← leaves the loop
//!     if let Some(e) = last_err { failures.push(...); yield Err(e) }  // ← records, then returns
//!     else { failures.push(...); return }
//! }
//! yield Err(ChainExhausted { message, transient })  // the chain gave up
//! ```
//!
//! The exhausted error is reachable **only** when every iteration took the
//! `continue`, and every `continue` happens *before* the only two
//! `failures.push` calls. So `failures` was provably always empty there, and
//! the diagnostics this commit added were dead code. The user got the bare
//! string the commit set out to replace.
//!
//! The same control flow had a second, louder consequence: a provider that
//! returns HTTP 200 and yields **zero events** took the `else` branch and
//! `return`ed — the whole fallback stream ended with no event and no error at
//! all. Not a bad message. No message.
//!
//! # Reproduction (the pre-fix behaviour, for the record)
//!
//! ```text
//! EXHAUSTED  out = [Err(NotImplemented("all 2 fallback providers exhausted without producing a result"))]
//! EMPTY-TAIL out = []
//! ```
//!
//! (The exhausted error is a `ChainExhausted` today: the message carries
//! the per-provider diagnostics and the `transient` flag carries the
//! verdict — **every** failure was transient (5xx/408/429/transport/broken
//! stream) means the chain never got a verdict from any server, so the
//! agent layer must replay it; at least one permanent failure (or an
//! empty stream) means it must not. See `crates/nini-ai/src/fallback.rs`.)

//! invariant: 3bf4815d —— 声明本文件守住了哪几条历史不变量。
//! 该标注让 scripts/invariants.py 认出这些断言；词法重叠看不出它们，

use std::pin::Pin;
use std::sync::Arc;

use async_stream::stream;
use futures_core::Stream;
use futures_util::StreamExt;
use nini_ai::fallback::FallbackProvider;
use nini_core::provider::{Capabilities, Provider, ProviderError, Request, StreamEvent};

/// A provider whose stream is a fixed list of events, named so the diagnostic
/// can be attributed to it.
struct Stub {
    name: &'static str,
    events: Vec<Result<StreamEvent, ProviderError>>,
}

impl Provider for Stub {
    fn name(&self) -> &'static str {
        self.name
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
    fn stream(
        &self,
        _req: Request,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send + 'static>> {
        let events = self.events.clone();
        Box::pin(stream! {
            for ev in events.into_iter() {
                yield ev;
            }
        })
    }
}

fn stub(name: &'static str, events: Vec<Result<StreamEvent, ProviderError>>) -> Arc<dyn Provider> {
    Arc::new(Stub { name, events })
}

/// A key that got HTTP `status` before producing a single event.
fn http(status: u16) -> Result<StreamEvent, ProviderError> {
    Err(ProviderError::Api {
        status,
        message: "upstream said no".into(),
    })
}

/// A transport-level failure (no status code to report).
fn network() -> Result<StreamEvent, ProviderError> {
    Err(ProviderError::Http("connection reset".into()))
}

fn text_delta(t: &str) -> Result<StreamEvent, ProviderError> {
    Ok(StreamEvent::TextDelta { text: t.into() })
}

fn stop() -> Result<StreamEvent, ProviderError> {
    Ok(StreamEvent::MessageStop {
        stop_reason: "end_turn".into(),
        usage: Default::default(),
    })
}

fn req() -> Request {
    Request {
        model: String::new(),
        messages: vec![],
        tools: vec![],
        max_tokens: None,
        temperature: None,
        system: None,
        reasoning: None,
    }
}

/// Drain a provider's stream into a vector.
fn collect(provider: &dyn Provider) -> Vec<Result<StreamEvent, ProviderError>> {
    tokio::runtime::Runtime::new()
        .expect("a runtime")
        .block_on(async {
            let mut s = Box::pin(provider.stream(req()));
            let mut out = Vec::new();
            while let Some(ev) = s.next().await {
                out.push(ev);
            }
            out
        })
}

/// The invariant in one line: **when the chain gives up, the error names every
/// key it tried and how each one failed.**
#[test]
fn an_exhausted_chain_names_every_key_it_tried_and_why() {
    let fb = FallbackProvider::new(vec![
        stub("key-ratelimited", vec![http(429)]),
        stub("key-network", vec![network()]),
        stub("key-gateway", vec![http(503)]),
    ]);

    let out = collect(&fb);
    // The terminal error is always a `ChainExhausted`: the message
    // carries the per-provider diagnostics, the `transient` flag carries
    // the verdict (this chain is all transient — the agent replays the
    // whole chain; a chain with a permanent failure in it is not).
    let (msg, transient) = out
        .iter()
        .filter_map(|e| match e {
            Err(ProviderError::ChainExhausted { message, transient }) => {
                Some((message.as_str(), *transient))
            }
            _ => None,
        })
        .next()
        .unwrap_or_else(|| panic!("no exhausted-chain error; got {out:?}"));

    assert!(
        transient,
        "every failure in this chain was transient — the chain is worth another try: {out:?}"
    );

    assert!(
        msg.contains("3"),
        "the message must say how many providers were tried: {msg}"
    );
    for expected in [
        "key-ratelimited: HTTP 429",
        "key-network: network/transport: connection reset",
        "key-gateway: HTTP 503",
    ] {
        assert!(
            msg.contains(expected),
            "the exhausted error omits `{expected}` — the user cannot tell which key to \
             rotate or which gateway to fix. Full message: {msg}"
        );
    }

    let ratelimited = msg.find("key-ratelimited").expect("key named");
    let network = msg.find("key-network").expect("key named");
    let gateway = msg.find("key-gateway").expect("key named");
    assert!(
        ratelimited < network && network < gateway,
        "keys must be reported in the order they were tried, or the report misleads \
         about which one to drop: {msg}"
    );
}

/// The louder half: a provider that answers **200 with zero events** used to end
/// the entire stream — no event, no error. The user saw a run that produced
/// nothing at all, which is indistinguishable from a hang.
#[test]
fn a_provider_that_returns_200_with_zero_events_is_named_not_dropped() {
    let fb = FallbackProvider::new(vec![
        stub("key-broken-gateway", vec![]),
        stub("key-also-empty", vec![]),
    ]);

    let out = collect(&fb);

    assert!(
        !out.is_empty(),
        "a chain where every provider yielded zero events produced no events AND no \
         error — the user gets silence, which is the failure this file exists to forbid"
    );
    let (msg, transient) = out
        .iter()
        .filter_map(|e| match e {
            Err(ProviderError::ChainExhausted { message, transient }) => {
                Some((message.as_str(), *transient))
            }
            _ => None,
        })
        .next()
        .unwrap_or_else(|| panic!("no exhausted-chain error; got {out:?}"));

    assert!(
        !transient,
        "a gateway that answered and had nothing to say is not transient — the chain must \
         not be replayed: {out:?}"
    );

    assert!(
        msg.contains("key-broken-gateway: empty stream (0 events)")
            && msg.contains("key-also-empty: empty stream (0 events)"),
        "a 200-with-zero-events provider must be named as such (it is the usual symptom \
         of a gateway buffering the whole response instead of streaming SSE). \
         Full message: {msg}"
    );
}

/// A zero-event provider is a failed attempt, not a reason to abandon the chain:
/// nothing was produced, so the next key is still worth trying. This is the
/// module's own rule ("fallback only triggers before any events have been
/// produced"), and the reason the empty-stream branch continues.
#[test]
fn an_empty_stream_does_not_stop_the_chain_from_recovering() {
    let fb = FallbackProvider::new(vec![
        stub("key-empty", vec![]),
        stub("key-good", vec![text_delta("recovered"), stop()]),
    ]);

    let out = collect(&fb);
    assert!(
        out.iter()
            .any(|e| matches!(e, Ok(StreamEvent::TextDelta { text }) if text == "recovered")),
        "a key that produced no events must not prevent a later key from answering; got {out:?}"
    );
    assert!(
        out.iter()
            .any(|e| matches!(e, Ok(StreamEvent::MessageStop { .. }))),
        "the recovered response must reach MessageStop; got {out:?}"
    );
}

/// The terminal, non-retryable branch still yields the original error unchanged —
/// the diagnostics are an addition, not a replacement. (A 401 does not fall back:
/// a second key will not fix a malformed request.)
#[test]
fn a_non_retryable_failure_is_yielded_unchanged() {
    let fb = FallbackProvider::new(vec![
        stub(
            "key-bad-auth",
            vec![Err(ProviderError::Auth("bad key".into()))],
        ),
        stub("key-other", vec![text_delta("must not appear"), stop()]),
    ]);

    let out = collect(&fb);
    assert!(
        out.iter()
            .any(|e| matches!(e, Err(ProviderError::Auth(m)) if m == "bad key")),
        "the original error kind must survive to the caller; got {out:?}"
    );
    assert!(
        !out.iter()
            .any(|e| matches!(e, Ok(StreamEvent::TextDelta { text }) if text == "must not appear")),
        "auth failures are not retryable: got {out:?}"
    );
}

/// A **verdict of the server's own** (401) is not a chain failure either: it is
/// yielded unchanged, so the chain never reaches the "exhausted" branch at all.
///
/// This is the other half of the exhausted-chain contract: the `ChainExhausted`
/// error (and its `transient` flag) is only ever built for chains that ran out
/// of providers **without** a verdict.
#[test]
fn a_server_verdict_never_becomes_a_chain_exhaustion() {
    let fb = FallbackProvider::new(vec![
        stub("key-unauthorized", vec![http(401)]),
        stub("key-other", vec![text_delta("must not appear"), stop()]),
    ]);

    let out = collect(&fb);
    assert!(
        out.iter()
            .any(|e| matches!(e, Err(ProviderError::Api { status: 401, .. }))),
        "the 401 must be yielded unchanged; got {out:?}"
    );
    assert!(
        !out.iter()
            .any(|e| matches!(e, Err(ProviderError::ChainExhausted { .. }))),
        "a permanent verdict stops the chain there — the exhausted error must not be \
         built at all: got {out:?}"
    );
    assert!(
        !out.iter()
            .any(|e| matches!(e, Ok(StreamEvent::TextDelta { text }) if text == "must not appear")),
        "a 401 is not retryable: got {out:?}"
    );
}
