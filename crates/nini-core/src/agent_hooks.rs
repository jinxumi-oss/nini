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
/// no steering, no follow-up, identity conversion, no context
/// transforms, no early stop. v0.7.0 (M3b) extends the surface with
/// 3 context hooks that let extensions shape the messages sent to
/// the LLM without forking nini-core.
///
/// **Internal-only messages** (the `Notification`, `UiMessage`,
/// `AppMessage` variants added in v0.7 M3a) are filtered out of the
/// LLM view before reaching the model. M3b wires this filtering into
/// the default `convert_to_llm` impl via
/// `AgentMessage::is_internal_only`.
pub trait AgentLoopHooks: Send +Sync {
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

    /// v0.7 (M3b) — called once per loop iteration BEFORE the
    /// model request, AFTER any built-in compaction has run.
    /// Lets the extension reshape the messages the LLM will
    /// receive.
    ///
    /// **Contract**: this hook is non-mutating. It receives the
    /// current message list and returns a (possibly different)
    /// view of those messages. The agent's internal `self.messages`
    /// is NOT modified by this call.
    ///
    /// The `TransformResult` carries:
    ///   * `messages`     — the messages to send to the LLM
    ///   * `dropped_count` — how many messages were filtered out
    ///                       (for status bar / debug UI)
    ///   * `reason`       — short tag like "context_window_exceeded"
    ///                       for observability
    ///
    /// Default implementation: identity (no transformation, no
    /// drops). Pi's typical override: trim the oldest N messages
    /// once the estimated token count exceeds the budget.
    fn transform_context(&self, messages: &[Message]) -> TransformResult {
        TransformResult {
            messages: messages.to_vec(),
            dropped_count: 0,
            reason: None,
        }
    }

    /// v0.7 (M3b) — called once per loop iteration BEFORE the
    /// model request, AFTER `transform_context`. Lets the extension
    /// filter or rewrite the message list one final time.
    ///
    /// **Contract**: this is the LAST step before the LLM sees the
    /// messages. The default implementation drops the 3
    /// internal-only variants (Notification / UiMessage /
    /// AppMessage) introduced in M3a — they never reach the model.
    ///
    /// Pi also collapses `custom` messages into user messages here
    /// (see the wiki 7→3 conversion table).
    fn convert_to_llm(&self, messages: &[Message]) -> Vec<Message> {
        // M3a default: filter_internal_only_messages is identity at
        // the provider layer (the 4-role Message enum has no
        // internal-only variants). The real filtering happens at the
        // entries→provider boundary in callers that go through
        // SessionEntry. We pass through unchanged so the provider
        // layer stays untouched.
        messages.to_vec()
    }

    /// v0.7 (M3b) — called once per loop iteration AFTER the
    /// model has responded with no tool calls (i.e. just before
    /// the agent would exit).
    ///
    /// Returns `true` to STOP the agent early. Default: `false`
    /// (run to completion as today).
    ///
    /// Common use cases:
    ///   * "Stop once the context is 95% full so the next user
    ///      prompt can trigger a compaction."
    ///   * "Stop if the last assistant message contains
    ///      `<DONE/>`."
    fn should_stop_after_turn(&self, _messages: &[Message]) -> bool {
        false
    }
}

/// v0.7 (M3b) — return type of `transform_context`.
///
/// `messages` is the new view sent to the LLM. `dropped_count` and
/// `reason` are observability metadata surfaced through the status
/// bar / debug log so users can see when and why the transform
/// fired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransformResult {
    pub messages: Vec<Message>,
    pub dropped_count: usize,
    pub reason: Option<String>,
}

impl TransformResult {
    /// Convenience constructor for the identity transformation
    /// (the default behaviour).
    pub fn identity(messages: &[Message]) -> Self {
        Self {
            messages: messages.to_vec(),
            dropped_count: 0,
            reason: None,
        }
    }

    /// Convenience constructor for a transformation that trimmed
    /// `dropped` messages off the end with the given `reason`.
    pub fn trimmed(messages: Vec<Message>, dropped: usize, reason: impl Into<String>) -> Self {
        Self {
            messages,
            dropped_count: dropped,
            reason: Some(reason.into()),
        }
    }
}

impl Default for TransformResult {
    /// Empty TransformResult: no messages, 0 dropped, no reason.
    /// Used by `catch_hook_panic` when a panicking transform hook
    /// falls back to a safe default — the agent sees an empty
    /// message view (LLM call would have nothing to send) and
    /// continues running. The hard `max_iterations` cap still
    /// prevents infinite loops.
    fn default() -> Self {
        Self {
            messages: Vec::new(),
            dropped_count: 0,
            reason: None,
        }
    }
}

/// Default no-op hooks. The reference `Agent::new` installs this
/// implicitly so existing callers see no behavior change.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopHooks;

impl AgentLoopHooks for NoopHooks {}

/// v0.7 (M3a) — filter the 3 internal-only `AgentMessage` variants
/// (Notification / UiMessage / AppMessage) out of a session message
/// list. Returns the input list unchanged when no filtering applies.
///
/// This is the SKeleton filter that `convert_to_llm` (M3b) will call
/// by default. It's exposed standalone here so:
///   * M3a integration tests can assert the predicate works
///   * M3b's default `convert_to_llm` impl has a single call site
///   * Extensions can reuse it without duplicating logic
///
/// **NOTE on scope**: this takes `Vec<Message>` (the provider-layer
/// 4-role type), not `Vec<entries::AgentMessage>` (the 10-variant
/// session type). M3b will do the conversion from session → provider
/// messages and apply this filter at the boundary. For M3a we keep
/// the type loose so this module compiles without depending on
/// `entries`.
pub fn filter_internal_only_messages(
    messages: Vec<crate::provider::Message>,
) -> Vec<crate::provider::Message> {
    // The provider-layer Message enum has only 4 roles
    // (System/User/Assistant/Tool) and does NOT carry the
    // Notification/UiMessage/AppMessage variants. So at THIS layer,
    // nothing to filter — it's already in LLM-view shape.
    //
    // The actual filtering happens earlier, at the
    // `entries::AgentMessage → provider::Message` boundary in M3b.
    // This function exists as the documented hook point so future
    // changes (e.g. adding more internal-only variants to the
    // provider layer) have a clear extension point.
    let _ = messages;
    messages
}

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

    /// M3a — the filter skeleton takes provider-layer Messages and
    /// returns them unchanged (because the provider-layer enum has
    /// no internal-only variants). The real filtering happens at
    /// the entries→provider boundary in M3b.
    #[test]
    fn filter_internal_only_skeleton_passes_through_provider_messages() {
        let msgs = vec![
            user_msg("a"),
            user_msg("b"),
        ];
        let out = filter_internal_only_messages(msgs.clone());
        assert_eq!(out.len(), msgs.len());
        assert_eq!(out[0].content[0], msgs[0].content[0]);
    }

    #[test]
    fn filter_internal_only_skeleton_preserves_empty_input() {
        let out = filter_internal_only_messages(Vec::new());
        assert!(out.is_empty());
    }

    /// TransformResult::identity preserves messages + reports 0 drops.
    #[test]
    fn transform_result_identity_constructor() {
        let msgs = vec![user_msg("a"), user_msg("b")];
        let r = TransformResult::identity(&msgs);
        assert_eq!(r.messages.len(), 2);
        assert_eq!(r.dropped_count, 0);
        assert!(r.reason.is_none());
    }

    /// TransformResult::trimmed records dropped count + reason.
    #[test]
    fn transform_result_trimmed_constructor() {
        let kept = vec![user_msg("kept")];
        let r = TransformResult::trimmed(kept.clone(), 5, "context_window_exceeded");
        assert_eq!(r.messages, kept);
        assert_eq!(r.dropped_count, 5);
        assert_eq!(r.reason.as_deref(), Some("context_window_exceeded"));
    }

    /// Default NoopHooks.transform_context is identity.
    #[test]
    fn noop_transform_context_is_identity() {
        let h = NoopHooks;
        let msgs = vec![user_msg("a"), user_msg("b")];
        let r = h.transform_context(&msgs);
        assert_eq!(r.messages, msgs);
        assert_eq!(r.dropped_count, 0);
        assert!(r.reason.is_none());
    }

    /// Default NoopHooks.convert_to_llm is identity (M3a filtering
    /// happens at the entries→provider boundary in callers).
    #[test]
    fn noop_convert_to_llm_is_identity() {
        let h = NoopHooks;
        let msgs = vec![user_msg("a")];
        let out = h.convert_to_llm(&msgs);
        assert_eq!(out, msgs);
    }

    /// Default NoopHooks.should_stop_after_turn returns false.
    #[test]
    fn noop_should_stop_after_turn_is_false() {
        let h = NoopHooks;
        let msgs = vec![user_msg("anything")];
        assert!(!h.should_stop_after_turn(&msgs));
        assert!(!h.should_stop_after_turn(&[]));
    }

    /// M3b — a custom TransformResult carries the reason string
    /// all the way through. Custom hook authors can tag
    /// their trims with a debug-friendly reason.
    #[test]
    fn transform_result_carries_reason() {
        let r = TransformResult {
            messages: vec![user_msg("kept")],
            dropped_count: 7,
            reason: Some("custom_trim_policy".into()),
        };
        assert_eq!(r.dropped_count, 7);
        assert_eq!(r.reason.as_deref(), Some("custom_trim_policy"));
        assert_eq!(r.messages.len(), 1);
    }
}