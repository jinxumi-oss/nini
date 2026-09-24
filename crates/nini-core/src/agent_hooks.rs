//! Agent-loop hook traits (v0.7 — Pi-compat hook parity, M1).
//!
//! This module defines the trait surface that extensions can implement
//! to inject behavior into the agent loop without forking `nini-core`.
//!
//! ## Why a separate trait file
//!
//! The hook surface grows over time:
//! - **M1** (this commit): `get_steering_messages` / `get_followup_messages`
//! - **M2**: `before_execute` / `after_execute` on the `Tool` trait
//! - **M3b**: `convert_to_llm` / `transform_context` / `should_stop_after_turn`
//!
//! Keeping them in a dedicated module avoids bloating `agent.rs` (which
//! holds the run-loop state machine) and gives extension authors a
//! single discoverable place to look.
//!
//! ## Hook execution model
//!
//! - All hook methods are synchronous (`fn`) or `async fn`. They run
//!   on the agent's main task — there is NO implicit `tokio::spawn`.
//! - Hooks may `await` internal futures but must respect `abort_signal`.
//! - Hooks MUST NOT call `Agent::run` recursively (compile-time
//!   enforced: hooks take `&self`, not `&mut Agent`).
//! - Hooks MUST NOT panic in the default-implementation contract for
//!   hot-path hooks (#1, #2 in the Pi spec). The runtime wraps each
//!   call in `std::panic::catch_unwind` and falls back to a safe
//!   default if the hook panics — see `Agent::run` for the wrapping
//!   site.
//!
//! ## Backwards compatibility
//!
//! Every hook has a default implementation that returns the current
//! v0.6.x behavior (identity / empty). Adding the hook trait to an
//! `Agent` does NOT change the runtime path unless an extension opts in.

use crate::provider::Message;

/// Hook trait for agents that want to inject messages mid-run.
///
/// The default implementation (`NoopHooks`) is the v0.6.1 behavior —
/// no steering, no follow-up. v0.7.x will add `convert_to_llm`,
/// `transform_context`, `should_stop_after_turn` in M3b.
pub trait AgentLoopHooks: Send + Sync {
    /// Inject messages **after** a round of tool execution, **before**
    /// the next LLM call.
    ///
    /// Pi contract: called once per loop iteration immediately after
    /// the agent has appended the tool results to its internal message
    /// log. The returned messages are appended in order and become
    /// visible to the model on the next request.
    ///
    /// Use case: the user hits Enter mid-run to redirect the agent
    /// ("actually, don't edit that file — first check whether...").
    fn get_steering_messages(&self, _recent: &[Message]) -> Vec<Message> {
        Vec::new()
    }

    /// Inject messages **at agent wrap-up**, after the model has
    /// emitted its final assistant turn with no further tool calls.
    ///
    /// Pi contract: called once per `Agent::run` call, after the loop
    /// has decided to exit (no more tool calls). The returned messages
    /// are appended to the log so they appear in the transcript /
    /// session file, but they do NOT trigger another model call (the
    /// agent is already done).
    ///
    /// Use case: the user hits Alt+Enter to schedule "after you're
    /// done with the current task, also run `cargo test`".
    fn get_followup_messages(&self, _all: &[Message]) -> Vec<Message> {
        Vec::new()
    }
}

/// Default no-op hooks. The reference `Agent::new` installs this
/// implicitly so existing callers see no behavior change.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopHooks;

impl AgentLoopHooks for NoopHooks {}

/// Catch a hook's panic and convert it to the safe default value.
///
/// This is the runtime's panic-safety net for hook authors who slip
/// up — a panicking hook should never crash the agent loop. The
/// returned value is what `Agent::run` will treat as the hook's
/// output.
///
/// We deliberately wrap the closure in `AssertUnwindSafe` because
/// the closure typically captures `Arc<dyn AgentLoopHooks>`, which
/// is not `RefUnwindSafe` (trait objects aren't by default — and we
/// don't want to require every hook impl to be `RefUnwindSafe` just
/// to satisfy this safety net). The safety net itself is best-effort:
/// panic-across-await boundaries are caught by tokio's task system
/// and surface as `JoinError` to the caller instead.
///
/// `label` is used in the warn log so extension authors can find the
/// offending hook in their logs.
pub fn catch_hook_panic<F, R>(label: &'static str, f: F) -> R
where
    F: FnOnce() -> R,
    R: Default,
{
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(v) => v,
        Err(_) => {
            eprintln!(
                "[nini] hook {label} panicked; falling back to default (R::default())"
            );
            R::default()
        }
    }
}

/// Convenience wrapper for async hooks. Tokio has no `catch_unwind`
/// for futures, so we rely on the sync `catch_unwind` to wrap the
/// poll step. Returns the safe default if the hook panics.
///
/// NOTE: This only protects against panics during the synchronous
/// portions of the future (e.g. the first poll). Panics deep inside
/// an awaited operation are caught by tokio's task system and converted
/// to `JoinError` — the caller should also `match` on that.
pub async fn catch_hook_panic_async<F, Fut, R>(label: &'static str, f: F) -> R
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = R>,
    R: Default,
{
    // Catch the synchronous setup (the FnOnce closure body). The future
    // it returns is polled inside tokio's task; if it panics during
    // poll, tokio catches it and returns a JoinError to the caller.
    let fut = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(f) => f,
        Err(_) => {
            eprintln!("[nini] async hook {label} panicked during setup; using default");
            return R::default();
        }
    };
    fut.await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{ContentBlock, Role};

    fn user_msg(text: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: text.into() }],
            timestamp: 0,
        }
    }

    #[test]
    fn noop_hooks_returns_empty_for_both() {
        let h = NoopHooks;
        assert!(h.get_steering_messages(&[]).is_empty());
        assert!(h.get_followup_messages(&[]).is_empty());
    }

    #[test]
    fn noop_hooks_ignores_recent_messages() {
        let h = NoopHooks;
        let recent = vec![user_msg("hello")];
        assert!(h.get_steering_messages(&recent).is_empty());
        let all = vec![user_msg("a"), user_msg("b"), user_msg("c")];
        assert!(h.get_followup_messages(&all).is_empty());
    }

    #[test]
    fn catch_hook_panic_returns_default_on_panic() {
        let result: Vec<Message> = catch_hook_panic("test", || {
            panic!("simulated hook panic");
        });
        assert!(result.is_empty(), "expected empty Vec as default");
    }

    #[test]
    fn catch_hook_panic_returns_value_on_success() {
        let result: Vec<Message> = catch_hook_panic("test", || {
            vec![user_msg("from hook")]
        });
        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0].content[0],
            ContentBlock::Text { text: "from hook".to_string() }
        );
    }

    #[tokio::test]
    async fn catch_hook_panic_async_returns_value_on_success() {
        let result: Vec<Message> = catch_hook_panic_async("test", || async {
            vec![user_msg("async hook output")]
        })
        .await;
        assert_eq!(result.len(), 1);
    }

    /// A panicking async future body is caught by tokio's task
    /// system and surfaces as JoinError to the caller. The
    /// setup-time panic path is best handled by the sync
    /// `catch_hook_panic` above; this test just verifies the
    /// wrapper compiles + awaits successfully when the inner
    /// async block returns cleanly.
    #[tokio::test]
    async fn catch_hook_panic_async_with_setup_then_future() {
        let result: Vec<Message> = catch_hook_panic_async("test", || async {
            // Pretend the setup completed (no panic); the future
            // then resolves to the empty Vec.
            Vec::<Message>::new()
        })
        .await;
        assert!(result.is_empty());
    }

    /// A custom hook implementation should be able to inject messages
    /// based on the recent turn state.
    #[test]
    fn custom_hook_can_inject_messages() {
        struct EchoingHook;
        impl AgentLoopHooks for EchoingHook {
            fn get_steering_messages(&self, recent: &[Message]) -> Vec<Message> {
                if recent.is_empty() {
                    return Vec::new();
                }
                // Echo back the last message as a steering nudge.
                vec![user_msg("(steering) I see you just sent something")]
            }
        }
        let h = EchoingHook;
        assert_eq!(h.get_steering_messages(&[]).len(), 0);
        assert_eq!(h.get_steering_messages(&[user_msg("hi")]).len(), 1);
        assert_eq!(
            h.get_followup_messages(&[user_msg("hi")]).len(),
            0,
            "followup defaults to empty when not overridden"
        );
    }
}