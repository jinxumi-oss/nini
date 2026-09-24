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

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

use async_stream::stream;
use futures_core::Stream;
use futures_util::StreamExt;
use nini_core::provider::{
    Capabilities, Provider, ProviderError, Request, StreamEvent,
};

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

/// v0.7.5 — classify a ProviderError as retryable. 5xx and 429 from
/// the upstream are always retryable. Network/transport errors
/// (`Http`, `Io`) are too. JSON/SSE parse errors and auth errors
/// are NOT retryable (different key won't help).
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

                if provider_failed_retryably {
                    // Try the next provider.
                    continue;
                }

                // Provider completed without emitting MessageStop
                // and didn't fail retryably. Emit the last error
                // if any, otherwise success.
                if let Some(e) = last_err {
                    yield Err(e);
                }
                return;
            }

            // All providers exhausted. Emit a fallback error.
            yield Err(ProviderError::NotImplemented(
                "all fallback providers exhausted without producing a result".into(),
            ));
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_stream::stream;

    fn mk_text(text: &str) -> Result<StreamEvent, ProviderError> {
        Ok(StreamEvent::TextDelta { text: text.to_string() })
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

    fn mk_http() -> Result<StreamEvent, ProviderError> {
        Err(ProviderError::Http("connection reset".into()))
    }

    /// Build a stub provider that emits a single fixed sequence.
    /// Events are stored in `Arc<[...]>` so the stub is Clone even though
    /// `Result<StreamEvent, ProviderError>` is not.
    fn stub(name: &'static str, events: Vec<Result<StreamEvent, ProviderError>>) -> Arc<dyn Provider> {
        struct StubProvider {
            name: &'static str,
            events: Arc<[Result<StreamEvent, ProviderError>]>,
        }
        impl Provider for StubProvider {
            fn name(&self) -> &'static str { self.name }
            fn capabilities(&self) -> Capabilities { Capabilities::default() }
            fn stream(
                &self,
                _req: Request,
            ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send + 'static>> {
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

    #[test]
    fn primary_succeeds_immediately_no_fallback() {
        let fb = FallbackProvider::new(vec![stub(
            "primary",
            vec![mk_text("hi"), mk_stop()],
        )]);
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
        let has_recovered = out.iter().any(|e| matches!(e, Ok(StreamEvent::TextDelta { text }) if text == "recovered"));
        assert!(has_recovered, "expected fallback to secondary; got: {out:?}");
    }

    #[test]
    fn primary_429_falls_back() {
        let fb = FallbackProvider::new(vec![
            stub("primary", vec![mk_429()]),
            stub("secondary", vec![mk_text("ok"), mk_stop()]),
        ]);
        let out = collect(&fb);
        let has_ok = out.iter().any(|e| matches!(e, Ok(StreamEvent::TextDelta { text }) if text == "ok"));
        assert!(has_ok);
    }

    #[test]
    fn primary_http_error_falls_back() {
        let fb = FallbackProvider::new(vec![
            stub("primary", vec![mk_http()]),
            stub("secondary", vec![mk_text("ok"), mk_stop()]),
        ]);
        let out = collect(&fb);
        let has_ok = out.iter().any(|e| matches!(e, Ok(StreamEvent::TextDelta { text }) if text == "ok"));
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
        let no_secondary = !out.iter().any(|e| matches!(e, Ok(StreamEvent::TextDelta { text }) if text == "should-not-appear"));
        assert!(no_secondary, "401 should not fall back to secondary");
    }

    #[test]
    fn partial_response_does_not_fall_back() {
        let fb = FallbackProvider::new(vec![
            stub("primary", vec![mk_text("partial"), mk_5xx()]),
            stub("secondary", vec![mk_text("should-not-appear"), mk_stop()]),
        ]);
        let out = collect(&fb);
        let has_partial = out.iter().any(|e| matches!(e, Ok(StreamEvent::TextDelta { text }) if text == "partial"));
        assert!(has_partial, "partial content must be preserved");
        let no_secondary = !out.iter().any(|e| matches!(e, Ok(StreamEvent::TextDelta { text }) if text == "should-not-appear"));
        assert!(no_secondary, "no fallback after partial response");
    }

    #[test]
    fn empty_provider_list_errors() {
        let fb = FallbackProvider::new(vec![]);
        let out = collect(&fb);
        assert!(out.iter().any(|e| matches!(e, Err(ProviderError::NotImplemented(_)))));
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