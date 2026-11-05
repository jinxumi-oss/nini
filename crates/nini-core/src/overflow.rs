//! Overflow detection — Pi's `isContextOverflow` / `isRecoverableLength`.
//!
//! Verified against Pi 0.84.2 (`pi-ai/src/utils/overflow.ts:134-162`). The
//! three cases below are Pi's, including the one that matters most in
//! practice and the one this file previously did not have at all: **Case 1**,
//! the error-message pattern, is how every real HTTP 400 arrives.
//!
//! v0.10 (fix): the only production call site passed `None` for the error
//! message (`agent.rs:1146`, "error message is in MessageStop"), so the
//! string branch below could never fire — while the provider reports the
//! overflow on `StreamEvent::Error`, a different branch of the agent loop.
//! The detector was, in the product, dead code.
//!
//! v0.12 (fix): two more gaps, both about *how a real overflow arrives*.
//!
//! 1. **The HTTP error path never consulted the detector at all.** Every
//!    provider turns a non-2xx response into
//!    `ProviderError::Api { status, message }`, and the agent's
//!    `Some(Err(e))` arm used to re-raise it without a single look at the
//!    text. A real HTTP 400 ("prompt is too long") therefore never reached
//!    the compaction-and-retry path. The agent now calls the detector on
//!    that arm as well (`agent.rs`).
//! 2. **Some vendors say nothing in prose.** iFlytek (讯飞星火) answers an
//!    over-long prompt with `{"error":{"code":10907,"message":"Bad
//!    Request"}}` — not one overflow word in the body, and the code is the
//!    only thing that distinguishes it from `{"error":{"code":10005,
//!    "message":"invalid request"}}` (bad parameters) or
//!    `{"error":{"code":10404,"message":"No Available Source"}}` (model
//!    unavailable). Natural-language patterns can never separate those
//!    three, so the vendor's own **error code** is a second, structured
//!    channel: [`OverflowSignals`] carries it, and
//!    [`VENDOR_ERROR_CODES`] is the table that decodes it.
//!
//! Pi's pattern list below is untouched — it is Pi parity. The vendor
//! table is an *additional* channel, never a replacement.

use crate::provider::Usage;

/// Error text that means the context is too big.
const OVERFLOW_PATTERNS: &[&str] = &[
    "context_length",
    "context length",
    "too large",
    "maximum context",
    "context window",
    "prompt is too long",
    "input too long",
    "reduce the length",
    "string too long",
];

/// Error text that looks alarming but is **not** an overflow.
///
/// Pi excludes these first (`overflow.ts:138`) so a throttled request never
/// triggers a compaction. Compacting on a rate limit discards real history
/// to fix a problem that compaction cannot fix.
const NON_OVERFLOW_PATTERNS: &[&str] = &[
    "rate limit",
    "rate_limit",
    "too many requests",
    "429",
    "quota",
    "overloaded",
    "529",
    "service unavailable",
    "503",
];

fn matches_any(haystack: &str, patterns: &[&str]) -> bool {
    let lower = haystack.to_lowercase();
    patterns.iter().any(|p| lower.contains(p))
}

/// What a vendor's error code means for the context window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VendorCodeVerdict {
    /// The code means the request did not fit the model's window.
    ///
    /// Compacting the history and retrying is the right response.
    Overflow,
    /// The code means something else entirely — bad parameters, an unknown
    /// model, an exhausted quota, a throttled request.
    ///
    /// Compacting would throw real history away to fix a problem that
    /// compaction cannot fix, so this verdict never triggers a compaction.
    NotOverflow,
}

/// One row of the vendor error-code table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VendorCodeRule {
    /// The vendor that defines the code.
    pub vendor: &'static str,
    /// The code exactly as the vendor writes it (numeric or symbolic).
    pub code: &'static str,
    /// What the code means for the context window.
    pub verdict: VendorCodeVerdict,
    /// Where the code comes from, and when it was checked.
    pub note: &'static str,
}

/// The vendor error-code table.
///
/// A vendor's own code is the only reliable signal for some providers:
/// iFlytek, for one, answers an over-long prompt with
/// `{"error":{"code":10907,"message":"Bad Request"}}` — the word
/// "Request" is all the prose there is, and the *same* prose arrives for
/// an invalid request (code `10005`) and for an unavailable model (code
/// `10404`). No natural-language pattern can tell those three apart.
///
/// Rules of the table:
///
/// * Only codes that were **observed or documented** are listed. A code
///   that is merely plausible is left out — a wrong "overflow" verdict
///   costs the user their history, so silence is cheaper than a guess.
/// * Numeric codes are **vendor-specific**. `10907` means "too long" for
///   iFlytek and nothing at all for anybody else; the lookup is therefore
///   conservative: a code that more than one vendor defines with
///   conflicting verdicts resolves to [`VendorCodeVerdict::NotOverflow`].
/// * A `NotOverflow` row does **not** veto the text-based detection. It
///   only means "this code does not, by itself, ask for a compaction" —
///   the server's prose still gets its say.
pub static VENDOR_ERROR_CODES: &[VendorCodeRule] = &[
    // --- iFlytek (讯飞星火 / spark) ------------------------------------
    // Measured against the live API on 2026-11: the same endpoint answers
    // with three different codes and three different messages, and only
    // the code says which one happened.
    VendorCodeRule {
        vendor: "iFlytek (spark)",
        code: "10907",
        verdict: VendorCodeVerdict::Overflow,
        note: "上下文超限 (context length exceeded). Measured 2026-11: the body is \
               {\"error\":{\"code\":10907,\"message\":\"Bad Request\"}} — the message \
               alone is indistinguishable from a parameter error.",
    },
    VendorCodeRule {
        vendor: "iFlytek (spark)",
        code: "10005",
        verdict: VendorCodeVerdict::NotOverflow,
        note: "invalid request (参数非法). Measured 2026-11. Must never trigger a \
               compaction: the history would be lost for nothing.",
    },
    VendorCodeRule {
        vendor: "iFlytek (spark)",
        code: "10404",
        verdict: VendorCodeVerdict::NotOverflow,
        note: "No Available Source (模型不可用). Measured 2026-11. Switching model \
               fixes it; compacting does not.",
    },
    // --- OpenAI --------------------------------------------------------
    VendorCodeRule {
        vendor: "OpenAI",
        code: "context_length_exceeded",
        verdict: VendorCodeVerdict::Overflow,
        note: "Documented by OpenAI: the request exceeded the model's context window.",
    },
    VendorCodeRule {
        vendor: "OpenAI",
        code: "invalid_api_key",
        verdict: VendorCodeVerdict::NotOverflow,
        note: "Documented by OpenAI. A credential problem, not a context problem.",
    },
    VendorCodeRule {
        vendor: "OpenAI",
        code: "model_not_found",
        verdict: VendorCodeVerdict::NotOverflow,
        note: "Documented by OpenAI. The model name is wrong; the context is fine.",
    },
    VendorCodeRule {
        vendor: "OpenAI",
        code: "insufficient_quota",
        verdict: VendorCodeVerdict::NotOverflow,
        note: "Documented by OpenAI. The account is out of quota — compaction cannot \
               help, and the request must not be retried in a loop.",
    },
    // --- Anthropic -----------------------------------------------------
    VendorCodeRule {
        vendor: "Anthropic",
        code: "request_too_large",
        verdict: VendorCodeVerdict::Overflow,
        note: "Anthropic's documented code for a request that does not fit the window. \
               Note that the *type* `invalid_request_error` on its own is NOT an \
               overflow — it covers every malformed request — so only this code is \
               listed.",
    },
    VendorCodeRule {
        vendor: "Anthropic",
        code: "invalid_api_key",
        verdict: VendorCodeVerdict::NotOverflow,
        note: "Anthropic's documented authentication error.",
    },
    VendorCodeRule {
        vendor: "Anthropic",
        code: "overloaded_error",
        verdict: VendorCodeVerdict::NotOverflow,
        note: "Anthropic's documented capacity error — transient, like a 529. \
               Compacting on it would drop history for nothing.",
    },
    // --- Google --------------------------------------------------------
    VendorCodeRule {
        vendor: "Google",
        code: "RESOURCE_EXHAUSTED",
        verdict: VendorCodeVerdict::NotOverflow,
        note: "Gemini's documented status for an exhausted quota / rate limit (HTTP \
               429). Gemini reports a context overflow as INVALID_ARGUMENT with a \
               token-count message, which the text patterns catch. Compacting on a \
               quota error would destroy history for nothing.",
    },
];

/// Structured signals about a provider error, beyond its prose.
///
/// The prose of an error message is the primary channel (Pi parity). The
/// vendor's own error code is the secondary one: some vendors say nothing
/// in prose at all (see [`VENDOR_ERROR_CODES`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OverflowSignals {
    /// The vendor's error code, when the provider surfaced one.
    ///
    /// Numeric (`"10907"`) and symbolic (`"context_length_exceeded"`)
    /// codes are both fine; the lookup is case-insensitive.
    pub error_code: Option<String>,
}

/// Decode a vendor error code.
///
/// Returns `None` when the code is not in the table — an unknown code
/// says nothing either way.
pub fn lookup_vendor_code(code: &str) -> Option<VendorCodeVerdict> {
    let needle = code.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return None;
    }
    let mut verdict: Option<VendorCodeVerdict> = None;
    for rule in VENDOR_ERROR_CODES {
        if rule.code.to_ascii_lowercase() != needle {
            continue;
        }
        match rule.verdict {
            // Conservative: when two vendors give the same code conflicting
            // verdicts, do not compact.
            VendorCodeVerdict::NotOverflow => return Some(VendorCodeVerdict::NotOverflow),
            VendorCodeVerdict::Overflow => verdict = Some(VendorCodeVerdict::Overflow),
        }
    }
    verdict
}

/// True when the vendor's code means "the input did not fit the window".
pub fn vendor_code_is_overflow(code: &str) -> bool {
    lookup_vendor_code(code) == Some(VendorCodeVerdict::Overflow)
}

/// Pull a vendor error code out of an error message.
///
/// Providers that surface a code do it in a handful of shapes, and the
/// code has to survive the trip through a one-line error string:
///
/// ```text
///   request failed: 400 Bad Request: Bad Request (code 10907)
///   request failed: 400 Bad Request: {"error":{"code":10907,...}}
///   request failed: 400 Bad Request: code=10907
/// ```
///
/// **Only a *labelled* code counts.** The code must be introduced by the
/// word `code` (`code 10907`, `code: 10907`, `"code":10907`).
///
/// The bare-number fallbacks this function used to have — `(10907)` and
/// a naked five-digit number — were a false-positive machine, and the
/// false positive was expensive. A five-digit number is *not* rare in an
/// error message: it is a request id, a token count, a byte offset, a
/// model version. And `10907` is a **number in the table**: so
///
/// ```text
///   request failed: 400 Bad Request: invalid request id 10907
///   request failed: 400 Bad Request: max tokens 10907 exceeded
///   request failed: 400 Bad Request: unknown model gpt-4 (10907)
/// ```
///
/// all used to decode as iFlytek's context-overflow code. The agent then
/// *compressed the user's history* and re-sent the request — and the
/// server refused it again, for the same reason, with the real cause
/// ("invalid request id 10907") nowhere in the message the user saw.
///
/// The provider layer renders the code as `(code 10907)`, so the labelled
/// form is the one that actually arrives; the fallbacks only ever added
/// false positives.
pub fn extract_vendor_error_code(text: &str) -> Option<String> {
    // Labelled codes: `code: 10907`, `code=10907`, `error code 10907`,
    // `"code":"10907"`, `(code 10907)`.
    code_label_re()
        .captures(text)
        .map(|caps| caps[1].to_string())
}

fn code_label_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        // The key may be quoted (JSON: `"code":10907`) and the value may
        // be quoted (`"code":"10907"`); the label itself is what makes a
        // number a *code* rather than an incidental number.
        regex::Regex::new(
            r#"(?i)["']?\b(?:error[_\s-]?)?code\b["']?\s*[:=]?\s*\(?\s*"?([A-Za-z0-9_.\-]+)"#,
        )
        .expect("the vendor-code label pattern must compile")
    })
}

/// Returns true if the model returned a context-overflow error (input too
/// long).
///
/// Pi's three cases, in its order:
///
/// 1. **Error message** — `stopReason === "error"` and the message matches an
///    overflow pattern, excluding the known non-overflow ones.
/// 2. **Silent overflow** — a *successful* response whose input already
///    exceeded the window (z.ai-style: the provider truncates instead of
///    erroring). Only the input counts: the output fits by definition.
/// 3. **Length-stop overflow** — `stopReason === "length"` with zero output
///    and input filling the window (Xiaomi MiMo-style: the server truncates
///    the oversized input, leaving no room to answer).
///
/// `stop_reason` is compared against both Pi's names (`stop`) and the
/// provider spellings nini actually stores (`end_turn`, `tool_use`), because
/// nini's loop keeps its own value rather than Pi's enum.
///
/// This is the four-argument form kept for backwards compatibility; it is
/// exactly [`is_context_overflow_with_signals`] with no vendor signals.
pub fn is_context_overflow(
    stop_reason: &str,
    error_message: Option<&str>,
    usage: Option<&Usage>,
    context_window: Option<u32>,
) -> bool {
    is_context_overflow_with_signals(
        stop_reason,
        error_message,
        usage,
        context_window,
        &OverflowSignals::default(),
    )
}

/// The detector, with the vendor-code channel added.
///
/// Same three Pi cases as [`is_context_overflow`], plus:
///
/// 4. **Vendor error code** — the provider's own code says the request did
///    not fit the window, even though the prose does not (iFlytek's
///    `10907` is the canonical example; see [`VENDOR_ERROR_CODES`]).
///
/// The code channel never overrides the non-overflow exclusions: a
/// throttled request (rate limit / 429 / 529 / 503) is not an overflow,
/// whatever code the vendor attached to it. Compacting on a rate limit
/// discards real history to fix a problem that compaction cannot fix.
pub fn is_context_overflow_with_signals(
    stop_reason: &str,
    error_message: Option<&str>,
    usage: Option<&Usage>,
    context_window: Option<u32>,
    signals: &OverflowSignals,
) -> bool {
    // Case 1 — the message pattern. Checked for any stop reason: providers
    // disagree about whether a 400 is an "error" turn or a plain stop, and
    // the text is unambiguous when it is present.
    if let Some(msg) = error_message {
        if matches_any(msg, OVERFLOW_PATTERNS) && !matches_any(msg, NON_OVERFLOW_PATTERNS) {
            return true;
        }
    }

    // Case 4 — the vendor's own error code. Some vendors say nothing in
    // prose ("Bad Request"), and the code is the only thing that says the
    // request was too long.
    if let Some(code) = signals.error_code.as_deref() {
        let throttled = error_message
            .map(|m| matches_any(m, NON_OVERFLOW_PATTERNS))
            .unwrap_or(false);
        if !throttled && vendor_code_is_overflow(code) {
            return true;
        }
    }

    let Some(usage) = usage else { return false };
    let Some(cw) = context_window.filter(|w| *w > 0) else {
        return false;
    };
    let input = usage.input_tokens.saturating_add(usage.cache_read_tokens);

    // Case 2 — silent overflow on a successful response.
    if matches!(stop_reason, "stop" | "end_turn" | "tool_use") && input > cw {
        return true;
    }

    // Case 3 — length stop, no output, window filled.
    if stop_reason == "length" && usage.output_tokens == 0 && input as u64 >= (cw as u64 * 99) / 100
    {
        return true;
    }

    false
}

/// Returns true if the model hit `length` stop with output_tokens
/// below `desired_max` — caller can retry after compaction.
///
/// v0.10 (fix): Pi's `isRecoverableLength` requires `desiredMaxOutput > 0`
/// (`overflow.ts:172`). Without it, an unset `max_tokens` (which resolves to
/// 0 here) compared as `0 < 0` is false — but any *negative* drift, and any
/// provider that reports a tiny non-zero limit, could classify an ordinary
/// truncation as recoverable.
pub fn is_recoverable_length(stop_reason: &str, output_tokens: u32, desired_max: u32) -> bool {
    stop_reason == "length" && desired_max > 0 && output_tokens < desired_max
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u32, output: u32) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        }
    }

    #[test]
    fn case1_error_message() {
        for m in [
            "This model's maximum context length is 200000 tokens",
            "context_length_exceeded",
            "prompt is too long: 300000 tokens > 200000",
        ] {
            assert!(
                is_context_overflow("error", Some(m), None, Some(1000)),
                "{m}"
            );
        }
    }

    #[test]
    fn case1_excludes_non_overflow() {
        for m in [
            "rate limit exceeded, please retry",
            "429 Too Many Requests",
            "upstream overloaded (529)",
        ] {
            assert!(
                !is_context_overflow("error", Some(m), None, Some(1000)),
                "{m}"
            );
        }
    }

    #[test]
    fn case2_silent_overflow() {
        assert!(is_context_overflow(
            "stop",
            None,
            Some(&usage(1200, 4)),
            Some(1000)
        ));
        // Output does not count — a long answer in a full window is normal.
        assert!(!is_context_overflow(
            "stop",
            None,
            Some(&usage(900, 900)),
            Some(1000)
        ));
    }

    #[test]
    fn case3_length_stop_with_no_output() {
        assert!(is_context_overflow(
            "length",
            None,
            Some(&usage(1000, 0)),
            Some(1000)
        ));
        // Output present → the model did answer; not an overflow.
        assert!(!is_context_overflow(
            "length",
            None,
            Some(&usage(1000, 50)),
            Some(1000)
        ));
    }

    #[test]
    fn no_window_means_no_verdict() {
        assert!(!is_context_overflow(
            "stop",
            None,
            Some(&usage(99_999, 0)),
            None
        ));
        assert!(!is_context_overflow(
            "stop",
            None,
            Some(&usage(99_999, 0)),
            Some(0)
        ));
    }

    // --- the vendor-code channel -----------------------------------------

    fn signals(code: &str) -> OverflowSignals {
        OverflowSignals {
            error_code: Some(code.to_string()),
        }
    }

    #[test]
    fn the_vendor_code_channel_finds_the_xunfei_overflow() {
        // Measured 2026-11 against the live iFlytek endpoint: the body of a
        // context overflow is `{"error":{"code":10907,"message":"Bad
        // Request"}}` — no overflow word anywhere in it.
        assert!(is_context_overflow_with_signals(
            "error",
            Some("request failed: 400 Bad Request: Bad Request"),
            None,
            Some(1000),
            &signals("10907"),
        ));
    }

    #[test]
    fn the_vendor_code_channel_ignores_the_other_xunfei_codes() {
        // 10005 = invalid request, 10404 = model unavailable. Compacting on
        // either would throw the history away for nothing.
        for code in ["10005", "10404"] {
            assert!(
                !is_context_overflow_with_signals(
                    "error",
                    Some("request failed: 400 Bad Request: Bad Request"),
                    None,
                    Some(1000),
                    &signals(code),
                ),
                "{code} must not be read as an overflow"
            );
        }
    }

    #[test]
    fn an_unknown_vendor_code_says_nothing() {
        assert!(!is_context_overflow_with_signals(
            "error",
            Some("request failed: 400 Bad Request"),
            None,
            Some(1000),
            &signals("99999"),
        ));
    }

    #[test]
    fn a_throttled_request_is_never_an_overflow() {
        // The non-overflow exclusions keep their power over the code
        // channel: a rate limit is not a context problem.
        assert!(!is_context_overflow_with_signals(
            "error",
            Some("request failed: 429 Too Many Requests"),
            None,
            Some(1000),
            &signals("10907"),
        ));
    }

    #[test]
    fn the_old_signature_still_works() {
        // Backwards compatibility: the four-argument form is the same
        // detector without the code channel.
        assert!(is_context_overflow(
            "error",
            Some("prompt is too long: 300000 tokens > 200000"),
            None,
            Some(1000)
        ));
        assert!(!is_context_overflow(
            "error",
            Some("rate limit exceeded"),
            None,
            Some(1000)
        ));
    }

    #[test]
    fn a_vendor_code_is_extracted_from_the_error_text() {
        for (text, want) in [
            (
                "request failed: 400 Bad Request: Bad Request (code 10907)",
                Some("10907"),
            ),
            (
                "request failed: 400 Bad Request: {\"error\":{\"code\":10907,\"message\":\"Bad Request\"}}",
                Some("10907"),
            ),
            ("request failed: 400 Bad Request: code=10907", Some("10907")),
            ("request failed: 400 Bad Request: error code: 10907", Some("10907")),
            ("request failed: 400 Bad Request: Bad Request", None),
            (
                "request failed: 400 Bad Request: prompt is too long: 300000 tokens > 200000",
                None,
            ),
        ] {
            assert_eq!(extract_vendor_error_code(text).as_deref(), want, "{text}");
        }
    }

    /// v0.12 (review, C1): a five-digit number that merely *equals* a
    /// table value is not a code.
    ///
    /// The bare-number fallback this replaces decoded `invalid request id
    /// 10907` as iFlytek's context-overflow code — and the agent answered
    /// that by compressing the user's history and re-sending the request.
    /// The server's real reason was then nowhere to be seen.
    #[test]
    fn a_five_digit_number_is_not_a_vendor_code() {
        for text in [
            "request failed: 400 Bad Request: invalid request id 10907",
            "request failed: 400 Bad Request: max tokens 10907 exceeded",
            "request failed: 400 Bad Request: unknown model gpt-4 (10907)",
            "request failed: 400 Bad Request: Bad Request [10907]",
            "request failed: 400 Bad Request: request 10907 was rejected",
        ] {
            assert_eq!(
                extract_vendor_error_code(text),
                None,
                "{text} must not be read as a vendor code"
            );
        }
    }

    /// The same false positive, end to end: a message that is **not** an
    /// overflow must not compact the user's history, however many digits
    /// it happens to contain.
    #[test]
    fn a_five_digit_number_does_not_make_a_non_overflow_an_overflow() {
        for text in [
            "request failed: 400 Bad Request: invalid request id 10907",
            "request failed: 400 Bad Request: max tokens 10907 exceeded",
            "request failed: 400 Bad Request: unknown model gpt-4 (10907)",
        ] {
            let signals = OverflowSignals {
                error_code: extract_vendor_error_code(text),
            };
            assert!(
                !is_context_overflow_with_signals("error", Some(text), None, Some(1000), &signals),
                "{text} is not a context overflow and must not compact the history"
            );
        }
    }

    #[test]
    fn the_vendor_code_table_is_conservative() {
        assert_eq!(
            lookup_vendor_code("10907"),
            Some(VendorCodeVerdict::Overflow)
        );
        assert_eq!(
            lookup_vendor_code("10005"),
            Some(VendorCodeVerdict::NotOverflow)
        );
        assert_eq!(lookup_vendor_code("nope"), None);
        // Case-insensitive.
        assert!(vendor_code_is_overflow("CONTEXT_LENGTH_EXCEEDED"));
    }
}
