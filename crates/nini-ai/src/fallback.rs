//! v0.7.5 (UX fix) — `FallbackProvider` actually implements fallback.
//!
//! Previously this was a stub that just yielded `NotImplemented`. The
//! bug surfaced in production when a user's `.bashrc` wrapper set
//! `--provider anthropic --model ...` with `NINI_FALLBACK_KEYS` set:
//! the wrapper code in `nini-cli` correctly constructed a
//! `FallbackProvider`, but its `stream()` method did nothing useful.
//!
//! ## Behavior
//!
//! `FallbackProvider::stream()` tries each provider in order. As soon
//! as one returns a stream that successfully produces a
//! `StreamEvent::MessageStop`, that stream becomes the output. If a
//! provider returns a *retryable* error (5xx, 429, network/transport)
//! before any events have been produced, the next provider in the
//! chain is tried.
//!
//! The key semantic: **fallback only triggers before any events are
//! produced**. Once a provider has started streaming events (TextDelta,
//! ToolCallStart, etc.), a later retryable error does NOT trigger
//! fallback — the partial response would be lost. This matches what
//! most users expect: "if the request hasn't started producing output
//! yet, try the next key; otherwise, return what we have".
//!
//! A non-retryable error (4xx other than 429, auth, parse, etc.)
//! immediately fails the stream. So does any error from a provider
//! after it has produced at least one event.
//!
//! A provider whose stream ends **without** a `MessageStop` after it
//! has already produced events is a failure too: the chain stops there
//! (the next provider's answer would be spliced onto the half-answer
//! already emitted) and the failure is **reported** as an error. A run
//! that ends without a conclusion and without a complaint is the worst
//! of both outcomes.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Arc;

use async_stream::stream;
use futures_core::Stream;
use futures_util::StreamExt;
use nini_core::provider::{Capabilities, Provider, ProviderError, Request, StreamEvent};

/// Provider that tries each child in sequence, falling back on
/// retryable errors BEFORE any event has been emitted.
pub struct FallbackProvider {
    /// `fallback_keys` env: secondary keys for the same provider.
    /// Order matters: index 0 is the primary; 1..N are the
    /// fallbacks. We try them in order.
    providers: Vec<Arc<dyn Provider>>,
}

impl FallbackProvider {
    pub fn new(providers: Vec<Arc<dyn Provider>>) -> Self {
        Self { providers }
    }
}

/// One-line, user-facing description of *how* a provider failed.
///
/// Lives outside `stream()` because every failure path needs it: the
/// retryable one (recorded, chain continues) and the terminal one
/// (recorded, error yielded) — see `3bf4815d`, whose diagnostics were
/// unreachable until the retryable branch recorded too.
fn kind_of(err: &ProviderError) -> String {
    match err {
        ProviderError::Api { status, message } => {
            // v0.10 (fix): a bare "HTTP 400" threw away the reason a
            // second time. The providers now carry the server's own
            // explanation ("request failed: 400 Bad Request: prompt is
            // too long"), and the fallback summary is the *last* place
            // the user sees it — so the reason comes along.
            let reason = brief_reason(message);
            if reason.is_empty() {
                format!("HTTP {status}")
            } else {
                format!("HTTP {status}: {reason}")
            }
        }
        // v0.13: the transport label used to be bare ("network/transport",
        // "I/O") — the *reason* was thrown away a second time, right
        // where the user reads it. The provider layer already carries
        // the real cause ("dns error: failed to lookup address
        // information: Name or service not known"), so the label quotes
        // it too.
        ProviderError::Http(message) => {
            with_reason("network/transport", &transport_reason(message))
        }
        ProviderError::Io(message) => with_reason("I/O", &transport_reason(message)),
        ProviderError::Sse(_) => "SSE parse".to_string(),
        ProviderError::Auth(_) => "auth".to_string(),
        // The request never left (an illegal URL, a redirect loop): the
        // reason says **which** one it was, and this summary is the last
        // place the user sees it.
        ProviderError::InvalidRequest(message) => with_reason(
            "invalid-request",
            &cut_reason(reason_without_url(message), 200),
        ),
        ProviderError::Json(_) => "json-parse".to_string(),
        ProviderError::NotImplemented(_) => "not-implemented".to_string(),
        // The chain gave up: every provider was tried and none produced
        // a result. This is the fallback chain's *terminal* error — it
        // never comes from a single provider — so the label is here to
        // keep the match exhaustive.
        ProviderError::ChainExhausted { .. } => "chain exhausted".to_string(),
    }
}

/// A short label plus the reason behind it, or the label alone when
/// there is no reason to quote.
fn with_reason(label: &str, reason: &str) -> String {
    if reason.is_empty() {
        label.to_string()
    } else {
        format!("{label}: {reason}")
    }
}

/// The transport failure's cause, without reqwest's redundant URL.
///
/// reqwest's own `Display` opens with "error sending request for url
/// (https://…)". The URL is redundant here: the label already says
/// network/transport, and the user knows which provider they asked.
/// The cause is what the user cannot guess — it follows the URL.
///
/// Messages without that prefix are taken as they are.
fn transport_reason(message: &str) -> String {
    const MAX: usize = 160;
    cut_reason(reason_without_url(message), MAX)
}

/// The cause behind a failure, with reqwest's URL taken off.
///
/// reqwest puts the request context in parentheses — "error sending
/// request for url (https://…)" for a transport failure, "error
/// following redirect for url (https://…)" for a redirect loop — and
/// the cause follows the closing parenthesis.
///
/// The URL may contain parentheses of its own (a query value like
/// `?tag=(beta)`), so the closing parenthesis is found by
/// **balancing** the brackets, not by taking the first one.
///
/// Messages without such a prefix are taken as they are.
fn reason_without_url(message: &str) -> &str {
    const PREFIXES: [&str; 2] = [
        "error sending request for url (",
        "error following redirect for url (",
    ];
    for prefix in PREFIXES {
        let Some(rest) = message.strip_prefix(prefix) else {
            continue;
        };
        return match closing_paren(rest) {
            // Everything after the closing paren is the cause; the
            // separating ": " goes with it.
            Some(i) => rest[i + 1..].trim_start().trim_start_matches(':').trim(),
            None => rest.trim(),
        };
    }
    message.trim()
}

/// The index of the `)` that closes the `(` a prefix left open.
///
/// `rest` starts right after that `(`, so the depth starts at 1 and
/// every `(` inside the URL opens another level.
fn closing_paren(rest: &str) -> Option<usize> {
    let mut depth = 1usize;
    for (i, c) in rest.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// A reason cut to `max` characters, with the cut marked by "…".
///
/// The same rule as [`brief_reason`]: the cut is on a character
/// boundary, so a multi-byte character is never split in half.
fn cut_reason(reason: &str, max: usize) -> String {
    if reason.is_empty() {
        return String::new();
    }
    if reason.chars().count() <= max {
        return reason.to_string();
    }
    let mut out: String = reason.chars().take(max).collect();
    out.push('…');
    out
}

/// The server's explanation, without the `request failed: <status>` prefix.
///
/// Returns an empty string when the message carries no reason (a stub
/// provider's own wording, or a body-less failure), so the caller can
/// keep the short form.
fn brief_reason(message: &str) -> String {
    const MAX: usize = 200;
    let Some(rest) = message.strip_prefix("request failed: ") else {
        return String::new();
    };
    // `rest` is "<status>: <reason>"; the reason is everything after the
    // first ": ".
    let Some((_, reason)) = rest.split_once(": ") else {
        return String::new();
    };
    let reason = reason.trim();
    if reason.is_empty() {
        return String::new();
    }
    if reason.chars().count() <= MAX {
        return reason.to_string();
    }
    let mut out: String = reason.chars().take(MAX).collect();
    out.push('…');
    out
}

/// v0.7.5 — classify a ProviderError as retryable. 5xx, 408 and 429
/// from the upstream are always retryable. Network/transport errors
/// (`Http`, `Io`) are too, and so is a stream that broke (`Sse`): the
/// request was worth sending once and may well be worth sending
/// again. JSON parse errors and auth errors are NOT retryable (a
/// different key won't help).
fn is_retryable(err: &ProviderError) -> bool {
    match err {
        ProviderError::Api { status, .. } => {
            // 5xx: server-side — try the next key.
            // 408 (request timeout) and 429 (rate limit) too.
            *status >= 500 || *status == 408 || *status == 429
        }
        ProviderError::Http(_) | ProviderError::Io(_) => true,
        ProviderError::Sse(_) => true, // stream broke — try again from scratch
        _ => false,
    }
}

impl Provider for FallbackProvider {
    fn name(&self) -> &'static str {
        // Use the primary's name so the TUI status bar / logs show
        // the "real" provider, not "fallback".
        self.providers
            .first()
            .map(|p| p.name())
            .unwrap_or("fallback")
    }

    fn capabilities(&self) -> Capabilities {
        // Union of capabilities across all fallbacks. Conservative:
        // empty capabilities — extensions can opt-in via a real
        // provider's `capabilities()`.
        Capabilities::default()
    }

    fn stream(
        &self,
        req: Request,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send + 'static>> {
        let providers = self.providers.clone();

        Box::pin(stream! {
            if providers.is_empty() {
                yield Err(ProviderError::NotImplemented(
                    "FallbackProvider constructed with no providers".into(),
                ));
                return;
            }

            // We track whether ANY provider has emitted an event.
            // If yes, we don't fall back — partial responses are
            // sacred. This matches the documented behavior.
            let any_event_emitted = Arc::new(AtomicBool::new(false));

            // v0.8.5: collect a per-provider failure summary so the
            // final "all exhausted" error tells the user WHICH keys
            // failed and HOW (auth, 5xx, 429, network, SSE parse,
            // empty stream). Without this, debugging required enabling
            // RUST_LOG and reading anthropic.rs traces.
            let mut failures: Vec<String> = Vec::new();

            // v0.13: was EVERY failure in this chain a **transient** one
            // — judged by this module's own `is_retryable` (5xx / 408 /
            // 429, a transport failure, a stream that broke)? If yes,
            // nothing in the chain was refused on its merits: no key is
            // bad, no request is malformed, no gateway is misconfigured.
            // The terminal error carries that verdict in its `transient`
            // flag, so the agent layer replays the whole chain (a user
            // with `--fallback-keys` used to die on the first network
            // hiccup — and, before this rule, on the very common "every
            // key answered 500" too).
            //
            // An **empty stream** ("empty stream (0 events)") is NOT
            // transient: the gateway answered and had nothing to say
            // (a misconfigured gateway that buffers the whole response,
            // or a model that emits nothing) — two seconds later it
            // answers the same way.
            let mut all_failures_transient = true;

            for provider in providers.iter() {
                let any_event_emitted = any_event_emitted.clone();
                let mut had_event = false;
                let mut result_stream = Box::pin(provider.stream(req.clone()));

                let mut provider_failed_retryably = false;
                let mut last_err: Option<ProviderError> = None;

                while let Some(ev) = result_stream.next().await {
                    match ev {
                        Ok(StreamEvent::MessageStop { .. }) => {
                            if had_event {
                                any_event_emitted.store(true, AtomicOrdering::SeqCst);
                            }
                            yield Ok(StreamEvent::MessageStop {
                                stop_reason: "end_turn".into(),
                                usage: Default::default(),
                            });
                            return;
                        }
                        Ok(other) => {
                            had_event = true;
                            any_event_emitted.store(true, AtomicOrdering::SeqCst);
                            yield Ok(other);
                        }
                        Err(e) => {
                            let retryable = is_retryable(&e);
                            last_err = Some(e);
                            if retryable && !had_event {
                                provider_failed_retryably = true;
                            }
                            break;
                        }
                    }
                }

                // Provider finished without ever emitting MessageStop.
                // Decide what to record, and whether the chain may go on.
                //
                // v0.8.5 audit fix: the retryable branch used to `continue`
                // **before** `failures.push(...)`, so by the time control
                // reached the "all exhausted" error, `failures` was
                // provably always empty and the per-provider diagnostics
                // the same commit added were unreachable dead code. The
                // user got the bare `all 2 fallback providers exhausted`
                // message that commit set out to replace.
                //
                // Nothing has been produced on this provider, so walking on
                // to the next key loses nothing — the same condition
                // `is_retryable` already treats as "this key is no good",
                // and the module's stated rule is "fallback only triggers
                // before any events have been produced". It is also the
                // only way the "empty stream" diagnosis can reach the user:
                // previously a zero-event provider ended the whole stream
                // silently, with no event and no error at all.
                if let Some(e) = last_err {
                    // Transient or not is this module's own question, and
                    // `is_retryable` is where it is answered: 5xx / 408 /
                    // 429, a transport failure (`Http` / `Io`) and a
                    // stream that broke (`Sse`) are worth another try; a
                    // verdict (`Api`: 401 / 400 / …), a request that never
                    // left (`InvalidRequest`) and a client-side fault
                    // (`Auth` / `Json`) are not.
                    //
                    // A permanent verdict never reaches the "exhausted"
                    // branch below — it is yielded unchanged — so the flag
                    // only ever decides the fate of the transient
                    // failures.
                    if !is_retryable(&e) {
                        all_failures_transient = false;
                    }
                    failures.push(format!("{}: {}", provider.name(), kind_of(&e)));
                    if provider_failed_retryably {
                        // Try the next provider.
                        continue;
                    }
                    yield Err(e);
                } else if !had_event {
                    // Empty stream — the provider returned 200 but
                    // yielded zero events. This happens with a
                    // misconfigured gateway that buffers the whole
                    // response into one chunk instead of streaming,
                    // or with a model that emits nothing. Record it and
                    // keep walking the chain.
                    //
                    // It is **not** transient: the gateway answered and
                    // had nothing to say — two seconds later it answers
                    // the same way.
                    all_failures_transient = false;
                    failures.push(format!("{}: empty stream (0 events)", provider.name()));
                    continue;
                } else {
                    // Partial response: events were produced but the
                    // stream ended without MessageStop. Falling back now
                    // would splice two responses together, so the chain
                    // cannot go on — but the failure must be **reported**:
                    // this branch used to `return` with no error at all,
                    // so the run ended in silence (no conclusion and no
                    // complaint), which is the worst of both worlds.
                    //
                    // The error is raised here and now, in place:
                    // `Sse` says exactly what happened — the response
                    // came, the stream died — and it carries no transient
                    // keyword, so the agent layer's text heuristic does
                    // not replay it. A half answer must not be replayed.
                    failures.push(format!(
                        "{}: stream ended without MessageStop",
                        provider.name()
                    ));
                    yield Err(ProviderError::Sse(format!(
                        "{}: the stream ended without MessageStop",
                        provider.name()
                    )));
                }
                return;
            }

            // All providers exhausted. Emit a fallback error with
            // per-provider diagnostics so the user can see which key
            // failed (auth / 5xx / 429 / network / empty stream) and
            // which was tried. Without this the bare "all fallback
            // providers exhausted" message gave no actionable info.
            let mut summary = format!(
                "all {} fallback providers exhausted without producing a result",
                providers.len()
            );
            if !failures.is_empty() {
                summary.push_str(": ");
                for (i, f) in failures.iter().enumerate() {
                    if i > 0 { summary.push_str("; "); }
                    summary.push_str(&f);
                }
            }
            // The terminal error is always a `ChainExhausted`: it carries
            // the per-provider diagnostics in `message` and the verdict
            // in `transient`, and the agent layer reads that flag to
            // decide whether the chain is worth another try.
            //
            // * `transient == true` — **every** recorded failure was
            //   transient (5xx / 408 / 429 / a transport failure / a
            //   stream that broke): nothing in the chain was refused on
            //   its merits — no key is bad, no request is malformed, no
            //   gateway is misconfigured. The whole chain is replayed.
            // * `transient == false` — at least one failure was not
            //   transient (an empty stream: the gateway answered and had
            //   nothing to say): the chain is not replayed.
            //
            // Two facts that are easy to get wrong:
            //
            // (a) The agent layer judges that **flag**, not the text: the
            //     summary quotes the inner errors verbatim — including
            //     their status codes ("primary: HTTP 500: internal") —
            //     and a text heuristic would read the "500" in there as
            //     transient and replay a chain that has already had its
            //     verdict.
            // (b) A **permanent** failure never reaches this branch: as
            //     soon as a provider gives a verdict of its own (401 /
            //     400 / auth / invalid request) the chain yields that
            //     error unchanged (`yield Err(e)`) and stops. The
            //     "exhausted" summary is for chains that ran out of
            //     providers without a verdict.
            // Nothing in the chain was refused on its merits → the chain
            // is worth one more try; otherwise it is not. The verdict
            // travels as a flag, next to the per-provider diagnostics.
            yield Err(ProviderError::ChainExhausted {
                message: summary,
                transient: all_failures_transient,
            });
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_stream::stream;

    fn mk_text(text: &str) -> Result<StreamEvent, ProviderError> {
        Ok(StreamEvent::TextDelta {
            text: text.to_string(),
        })
    }

    fn mk_stop() -> Result<StreamEvent, ProviderError> {
        Ok(StreamEvent::MessageStop {
            stop_reason: "end_turn".into(),
            usage: Default::default(),
        })
    }

    fn mk_5xx() -> Result<StreamEvent, ProviderError> {
        Err(ProviderError::Api {
            status: 500,
            message: "internal".into(),
        })
    }

    fn mk_429() -> Result<StreamEvent, ProviderError> {
        Err(ProviderError::Api {
            status: 429,
            message: "rate limit".into(),
        })
    }

    fn mk_401() -> Result<StreamEvent, ProviderError> {
        Err(ProviderError::Api {
            status: 401,
            message: "unauthorized".into(),
        })
    }

    /// A refusal whose wording carries **no** transient keyword — the
    /// status alone must decide.
    fn mk_400() -> Result<StreamEvent, ProviderError> {
        Err(ProviderError::Api {
            status: 400,
            message: "the request was rejected".into(),
        })
    }

    fn mk_http() -> Result<StreamEvent, ProviderError> {
        Err(ProviderError::Http("connection reset".into()))
    }

    /// Build a stub provider that emits a single fixed sequence.
    /// Events are stored in `Arc<[...]>` so the stub is Clone even though
    /// `Result<StreamEvent, ProviderError>` is not.
    fn stub(
        name: &'static str,
        events: Vec<Result<StreamEvent, ProviderError>>,
    ) -> Arc<dyn Provider> {
        struct StubProvider {
            name: &'static str,
            events: Arc<[Result<StreamEvent, ProviderError>]>,
        }
        impl Provider for StubProvider {
            fn name(&self) -> &'static str {
                self.name
            }
            fn capabilities(&self) -> Capabilities {
                Capabilities::default()
            }
            fn stream(
                &self,
                _req: Request,
            ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send + 'static>>
            {
                let events = self.events.clone();
                Box::pin(stream! {
                    // events is Arc<[Result<...>]> which is Clone.
                    // Iterate by reference and clone the owned value out.
                    for ev in events.iter() {
                        yield ev.clone();
                    }
                })
            }
        }
        Arc::new(StubProvider {
            name,
            events: Arc::from(events.into_boxed_slice()),
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

    fn collect(provider: &dyn Provider) -> Vec<Result<StreamEvent, ProviderError>> {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut s = Box::pin(provider.stream(req()));
            let mut out = Vec::new();
            while let Some(ev) = s.next().await {
                out.push(ev);
            }
            out
        })
    }

    #[test]
    fn is_retryable_classification() {
        assert!(is_retryable(&mk_5xx().unwrap_err()));
        assert!(is_retryable(&mk_429().unwrap_err()));
        assert!(is_retryable(&mk_http().unwrap_err()));
        assert!(!is_retryable(&mk_401().unwrap_err()));
        assert!(!is_retryable(&ProviderError::Auth("bad key".into())));
        assert!(!is_retryable(&ProviderError::Json("garbage".into())));
    }

    /// The fallback summary must not throw the server's reason away a
    /// second time.
    ///
    /// The providers now carry it ("request failed: 400 Bad Request:
    /// prompt is too long"), but the summary used to print "HTTP 400"
    /// and stop there — so the user of a *fallback* chain still learned
    /// nothing about the actual cause.
    #[test]
    fn the_summary_keeps_the_servers_reason() {
        let err = ProviderError::Api {
            status: 400,
            message: "request failed: 400 Bad Request: prompt is too long".to_string(),
        };
        assert_eq!(kind_of(&err), "HTTP 400: prompt is too long");
    }

    /// The summary repeats what the provider said — and only that.
    ///
    /// The provider layer states facts (status, the server's words, the
    /// vendor's error code) and the agent layer states the advice. The
    /// summary must not carry a third copy of either, and above all no
    /// provider-side hint: the hint is gone from the message, so the
    /// summary is clean by construction.
    #[test]
    fn the_summary_carries_the_facts_and_no_hint() {
        let err = ProviderError::Api {
            status: 400,
            message: "request failed: 400 Bad Request: Bad Request (code 10907)".to_string(),
        };
        let summary = kind_of(&err);
        assert_eq!(summary, "HTTP 400: Bad Request (code 10907)");
        assert!(
            !summary.contains("hint") && !summary.contains("the server rejected the request"),
            "the summary must not resurrect a provider-side hint: {summary:?}"
        );

        let err = ProviderError::Api {
            status: 400,
            message: "request failed: 400 Bad Request".to_string(),
        };
        assert_eq!(kind_of(&err), "HTTP 400");
    }

    /// A failure with no reason (an empty body, or a provider that
    /// speaks for itself) keeps the short form.
    #[test]
    fn the_summary_stays_short_without_a_reason() {
        let err = ProviderError::Api {
            status: 500,
            message: "request failed: 500 Internal Server Error".to_string(),
        };
        assert_eq!(kind_of(&err), "HTTP 500");

        let err = ProviderError::Api {
            status: 500,
            message: "internal".to_string(),
        };
        assert_eq!(kind_of(&err), "HTTP 500");
    }

    /// A transport failure must say **why** it failed, and must not
    /// waste the reader's time on the URL they already know.
    ///
    /// reqwest's own wording is "error sending request for url
    /// (https://…): dns error: …". The URL is redundant (the label
    /// already says "network/transport", and the user knows which
    /// provider they asked); the cause is the part nobody can guess.
    #[test]
    fn the_transport_label_carries_the_reason_and_not_the_url() {
        let err = ProviderError::Http(
            "error sending request for url (https://api.openai.com/v1/chat/completions): \
             dns error: failed to lookup address information: Name or service not known"
                .into(),
        );
        let label = kind_of(&err);
        assert_eq!(
            label,
            "network/transport: dns error: failed to lookup address information: \
             Name or service not known",
            "the transport label must quote the cause"
        );
        assert!(
            !label.contains("https://"),
            "the URL is redundant — the label already says network/transport: {label:?}"
        );
    }

    /// An `I/O` failure has the same problem and the same fix.
    #[test]
    fn the_io_label_carries_the_reason() {
        let err = ProviderError::Io("connection reset by peer".into());
        assert_eq!(kind_of(&err), "I/O: connection reset by peer");
    }

    /// A request that never left has a reason too, and it says **which**
    /// one it was — "builder error" (an illegal URL) or a redirect
    /// loop.
    #[test]
    fn the_invalid_request_label_carries_the_reason() {
        let err =
            ProviderError::InvalidRequest("builder error: relative URL without a base".into());
        assert_eq!(
            kind_of(&err),
            "invalid-request: builder error: relative URL without a base",
            "the reason must survive into the summary"
        );

        let err = ProviderError::InvalidRequest(
            "error following redirect for url (https://api.example.com/deploy-503): \
             too many redirects"
                .into(),
        );
        assert_eq!(
            kind_of(&err),
            "invalid-request: too many redirects",
            "the URL must be taken off, the cause must stay"
        );
    }

    /// A URL that contains parentheses of its own must not cut the cause
    /// short: the brackets are **balanced**, not guessed.
    #[test]
    fn the_transport_label_survives_a_url_with_parentheses() {
        let err = ProviderError::Http(
            "error sending request for url (https://api.example.com/v1/models?tag=(beta)): \
             dns error: failed to lookup address information"
                .into(),
        );
        let label = kind_of(&err);
        assert_eq!(
            label, "network/transport: dns error: failed to lookup address information",
            "the URL must be cut off at its **closing** parenthesis: {label:?}"
        );
    }

    /// A chain where **every** failure is a transport failure never got
    /// a verdict from any server: this is the network, not the request,
    /// so the terminal error must be transient — the agent layer then
    /// retries the whole chain instead of dying on the first hiccup.
    /// The per-provider diagnostics must survive.
    #[test]
    fn a_chain_of_transport_failures_ends_chain_exhausted_and_transient() {
        let fb = FallbackProvider::new(vec![
            stub("primary", vec![mk_http()]),
            stub("secondary", vec![mk_http()]),
        ]);
        let out = collect(&fb);
        let err = out
            .into_iter()
            .find_map(|e| e.err())
            .unwrap_or_else(|| panic!("expected an error"));
        match err {
            ProviderError::ChainExhausted { message, transient } => {
                assert!(
                    transient,
                    "a chain that never got a verdict must be worth another try"
                );
                assert!(
                    message.contains("primary: network/transport")
                        && message.contains("secondary: network/transport"),
                    "the diagnostics must survive in the exhausted error: {message}"
                );
            }
            other => panic!("expected ProviderError::ChainExhausted, got {other:?}"),
        }
    }

    /// A **permanent** verdict is yielded unchanged: the chain stops
    /// there, and the "exhausted" summary is never built.
    ///
    /// A 401 is not retryable, so the original error is what the user
    /// sees — and the agent layer does not replay it either.
    #[test]
    fn a_chain_with_a_permanent_verdict_yields_that_verdict_unchanged() {
        let fb = FallbackProvider::new(vec![
            stub("primary", vec![mk_401()]),
            stub("secondary", vec![mk_http()]),
        ]);
        let out = collect(&fb);
        let err = out
            .into_iter()
            .find_map(|e| e.err())
            .unwrap_or_else(|| panic!("expected an error"));
        assert!(
            matches!(err, ProviderError::Api { status: 401, .. }),
            "a 401 must be yielded unchanged, never as a transport failure: got {err:?}"
        );
        assert!(
            !nini_core::agent::is_transient_provider_failure(&err),
            "a refused request must not be replayed by the agent layer: {err:?}"
        );
    }

    /// A chain whose every failure was **transient** is worth another
    /// try — and that includes the very common shape: every key answered
    /// 500.
    ///
    /// The old rule only counted *transport* failures, so a chain of 500s
    /// ended as `NotImplemented` — nini's own "the chain gave up"
    /// diagnostic — and the agent layer, which reads the kind and not the
    /// text, did not replay it. That was worse than before: the summary
    /// quotes "HTTP 500" verbatim, and the old text heuristic used to
    /// replay it.
    #[test]
    fn a_chain_of_transient_failures_ends_replayable() {
        let fb = FallbackProvider::new(vec![
            stub("primary", vec![mk_5xx()]),
            stub("secondary", vec![mk_5xx()]),
        ]);
        let out = collect(&fb);
        let err = out
            .into_iter()
            .find_map(|e| e.err())
            .unwrap_or_else(|| panic!("expected an error"));
        assert!(
            matches!(
                err,
                ProviderError::ChainExhausted {
                    transient: true,
                    ..
                }
            ),
            "a chain of transient failures must end in the replayable kind: got {err:?}"
        );
        assert!(
            nini_core::agent::is_transient_provider_failure(&err),
            "a chain of transient failures must be replayed by the agent layer: {err:?}"
        );
    }

    /// An empty stream is **not** transient: the gateway answered and
    /// had nothing to say (a misconfigured gateway that buffers the whole
    /// response, or a model that emits nothing) — two seconds later it
    /// answers the same way. The chain must not be replayed.
    #[test]
    fn a_chain_with_an_empty_stream_is_not_replayed() {
        let fb = FallbackProvider::new(vec![stub("primary", vec![]), stub("secondary", vec![])]);
        let out = collect(&fb);
        let err = out
            .into_iter()
            .find_map(|e| e.err())
            .unwrap_or_else(|| panic!("expected an error"));
        assert!(
            matches!(
                err,
                ProviderError::ChainExhausted {
                    transient: false,
                    ..
                }
            ),
            "an empty stream must not look transient: got {err:?}"
        );
        assert!(
            !nini_core::agent::is_transient_provider_failure(&err),
            "an empty stream must not be replayed by the agent layer: {err:?}"
        );
    }

    /// The terminal error's *kind* is what the agent layer reads to
    /// decide whether a retry could help — so the behaviour worth
    /// pinning down is not "which variant" but "will the agent replay
    /// the request".
    ///
    /// A chain whose every failure was a transport failure never got a
    /// verdict from any server: this is the network, and the agent must
    /// replay the whole chain.
    #[test]
    fn a_transport_only_chain_is_replayed_by_the_agent_layer() {
        let fb = FallbackProvider::new(vec![
            stub("primary", vec![mk_http()]),
            stub("secondary", vec![mk_http()]),
        ]);
        let out = collect(&fb);
        let err = out
            .into_iter()
            .find_map(|e| e.err())
            .unwrap_or_else(|| panic!("expected an error"));
        assert!(
            nini_core::agent::is_transient_provider_failure(&err),
            "a chain that never got a verdict from any server must be \
             replayed by the agent layer: {err:?}"
        );
    }

    /// A chain that got a **verdict** is not replayed: the request was
    /// refused, and sending it again would only produce the same
    /// refusal.
    ///
    /// A 4xx is not retryable, so the chain stops at the refusal and
    /// that refusal is the terminal error. Its wording carries no
    /// transient keyword ("the request was rejected") — the status
    /// alone has to decide.
    #[test]
    fn a_chain_with_a_server_verdict_is_not_replayed_by_the_agent_layer() {
        let fb = FallbackProvider::new(vec![
            stub("primary", vec![mk_400()]),
            stub("secondary", vec![mk_http()]),
        ]);
        let out = collect(&fb);
        let err = out
            .into_iter()
            .find_map(|e| e.err())
            .unwrap_or_else(|| panic!("expected an error"));
        assert!(
            !nini_core::agent::is_transient_provider_failure(&err),
            "a refused request must not be replayed by the agent layer: {err:?}"
        );
    }

    #[test]
    fn primary_succeeds_immediately_no_fallback() {
        let fb = FallbackProvider::new(vec![stub("primary", vec![mk_text("hi"), mk_stop()])]);
        let out = collect(&fb);
        assert_eq!(out.len(), 2);
        assert!(matches!(&out[0], Ok(StreamEvent::TextDelta { text }) if text == "hi"));
    }

    #[test]
    fn primary_5xx_falls_back_to_secondary() {
        let fb = FallbackProvider::new(vec![
            stub("primary", vec![mk_5xx()]),
            stub("secondary", vec![mk_text("recovered"), mk_stop()]),
        ]);
        let out = collect(&fb);
        let has_recovered = out
            .iter()
            .any(|e| matches!(e, Ok(StreamEvent::TextDelta { text }) if text == "recovered"));
        assert!(
            has_recovered,
            "expected fallback to secondary; got: {out:?}"
        );
    }

    #[test]
    fn primary_429_falls_back() {
        let fb = FallbackProvider::new(vec![
            stub("primary", vec![mk_429()]),
            stub("secondary", vec![mk_text("ok"), mk_stop()]),
        ]);
        let out = collect(&fb);
        let has_ok = out
            .iter()
            .any(|e| matches!(e, Ok(StreamEvent::TextDelta { text }) if text == "ok"));
        assert!(has_ok);
    }

    #[test]
    fn primary_http_error_falls_back() {
        let fb = FallbackProvider::new(vec![
            stub("primary", vec![mk_http()]),
            stub("secondary", vec![mk_text("ok"), mk_stop()]),
        ]);
        let out = collect(&fb);
        let has_ok = out
            .iter()
            .any(|e| matches!(e, Ok(StreamEvent::TextDelta { text }) if text == "ok"));
        assert!(has_ok);
    }

    #[test]
    fn primary_4xx_does_not_fall_back() {
        let fb = FallbackProvider::new(vec![
            stub("primary", vec![mk_401()]),
            stub("secondary", vec![mk_text("should-not-appear"), mk_stop()]),
        ]);
        let out = collect(&fb);
        assert!(out.iter().any(|e| matches!(e, Err(_))));
        let no_secondary = !out.iter().any(
            |e| matches!(e, Ok(StreamEvent::TextDelta { text }) if text == "should-not-appear"),
        );
        assert!(no_secondary, "401 should not fall back to secondary");
    }

    #[test]
    fn partial_response_does_not_fall_back() {
        let fb = FallbackProvider::new(vec![
            stub("primary", vec![mk_text("partial"), mk_5xx()]),
            stub("secondary", vec![mk_text("should-not-appear"), mk_stop()]),
        ]);
        let out = collect(&fb);
        let has_partial = out
            .iter()
            .any(|e| matches!(e, Ok(StreamEvent::TextDelta { text }) if text == "partial"));
        assert!(has_partial, "partial content must be preserved");
        let no_secondary = !out.iter().any(
            |e| matches!(e, Ok(StreamEvent::TextDelta { text }) if text == "should-not-appear"),
        );
        assert!(no_secondary, "no fallback after partial response");
    }

    /// A provider that produced events but **no** `MessageStop` used to
    /// end the whole chain in silence: no error, no conclusion — the
    /// user could not tell a broken stream from a hang.
    ///
    /// The chain cannot go on (the next provider's answer would be
    /// spliced onto the half-answer already emitted), so the failure is
    /// reported in place, naming the provider that died.
    #[test]
    fn a_stream_that_ends_without_message_stop_is_reported() {
        let fb = FallbackProvider::new(vec![
            stub("primary", vec![mk_text("half an answer")]),
            stub("secondary", vec![mk_text("must-not-appear"), mk_stop()]),
        ]);
        let out = collect(&fb);
        let err = out
            .iter()
            .find_map(|e| match e {
                Err(e) => Some(e.clone()),
                Ok(_) => None,
            })
            .unwrap_or_else(|| panic!("a silent end is the worst outcome: {out:?}"));
        assert!(
            matches!(&err, ProviderError::Sse(m) if m.contains("primary")),
            "the failure must be reported and name the provider: got {err:?}"
        );
        assert!(
            !out.iter().any(
                |e| matches!(e, Ok(StreamEvent::TextDelta { text }) if text == "must-not-appear")
            ),
            "the chain must not go on: the next answer would be spliced onto the half-answer"
        );
        // Whether the **agent layer** replays this is decided by the
        // "only before the first output" rule, not by the classification:
        // the half answer was already streamed, so the `produced_output`
        // guard in `crates/nini-core/src/agent.rs` blocks the replay —
        // see `crates/nini-core/tests/provider_transport_retry.rs`
        // (`a_broken_sse_stream_after_output_is_not_replayed`).
        //
        // What matters *here* is that the chain stops instead of splicing
        // two answers together (asserted above) and that the failure is
        // reported at all (asserted above).
    }

    #[test]
    fn empty_provider_list_errors() {
        let fb = FallbackProvider::new(vec![]);
        let out = collect(&fb);
        assert!(out
            .iter()
            .any(|e| matches!(e, Err(ProviderError::NotImplemented(_)))));
    }

    #[test]
    fn name_is_primarys_name() {
        let fb = FallbackProvider::new(vec![
            stub("anthropic-primary", vec![]),
            stub("anthropic-fallback", vec![]),
        ]);
        assert_eq!(fb.name(), "anthropic-primary");
    }
}
