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
pub fn is_context_overflow(
    stop_reason: &str,
    error_message: Option<&str>,
    usage: Option<&Usage>,
    context_window: Option<u32>,
) -> bool {
    // Case 1 — the message pattern. Checked for any stop reason: providers
    // disagree about whether a 400 is an "error" turn or a plain stop, and
    // the text is unambiguous when it is present.
    if let Some(msg) = error_message {
        if matches_any(msg, OVERFLOW_PATTERNS) && !matches_any(msg, NON_OVERFLOW_PATTERNS) {
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
}
