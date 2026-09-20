//! Overflow detection — heuristic detection of context-overflow and
//! recoverable-length errors from upstream LLM providers. Placeholder
//! implementations: real patterns added in next commit.

use crate::provider::Usage;

/// Returns true if the model returned a context-overflow error
/// (input too long). Heuristic: looks for "context_length" or
/// "too large" in the error message, or a `StopReason::Length` with
/// input tokens near `context_window`.
pub fn is_context_overflow(
    stop_reason: &str,
    error_message: Option<&str>,
    usage: Option<&Usage>,
    context_window: Option<u32>,
) -> bool {
    if let Some(msg) = error_message {
        let lower = msg.to_lowercase();
        if lower.contains("context_length")
            || lower.contains("context length")
            || lower.contains("too large")
            || lower.contains("maximum context")
        {
            return true;
        }
    }
    if stop_reason == "length" {
        if let (Some(u), Some(cw)) = (usage, context_window) {
            let total = u.input_tokens.saturating_add(u.output_tokens);
            if total as u32 >= cw {
                return true;
            }
        }
    }
    false
}

/// Returns true if the model hit `length` stop with output_tokens
/// below `desired_max` — caller can retry after compaction.
pub fn is_recoverable_length(
    stop_reason: &str,
    output_tokens: u32,
    desired_max: u32,
) -> bool {
    stop_reason == "length" && output_tokens < desired_max
}
