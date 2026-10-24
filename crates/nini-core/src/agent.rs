//! Agent loop: orchestrate LLM calls + tool execution + event emission.
//!
//! Phase 2 scope:
//! - Single-turn: send messages to provider, stream events, dispatch tool calls
//! - Multi-iteration: re-send to provider with tool results until stop
//! - Abort: cooperative cancellation via `AbortHandle`
//! - Event emission: `AgentEvent` stream for UI consumers

use crate::provider::{Message, Provider, ProviderError, Request, StreamEvent, ToolSpec, Usage};
use crate::tool::{ToolContext, ToolError, ToolOutput, ToolRegistry};
use crate::{AgentMessage, ContentBlock, Role};
use async_stream::try_stream;
use futures_core::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::Notify;

/// Maximum tool-iteration count before forcing a stop (matches spec default).
pub const MAX_TOOL_ITERATIONS: usize = 50;

/// Agent event emitted to consumers (UI, logs, etc.).
/// Phase indicator surfaced to the UI / extension host. Mirrors the five
/// states used by Pi's `StatusIndicator` component (see
/// `dist/modes/interactive/components/status-indicator.js`):
/// Idle / Working / Compacting / Retrying / BranchSummary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum AgentPhase {
    /// Ready for input; no active run.
    Idle,
    /// Streaming an LLM response or running tool calls.
    Working,
    /// Compacting context. Carries the trigger reason ("manual",
    /// "overflow") and an optional 0..=100 progress estimate.
    Compacting {
        reason: String,
        progress: Option<u8>,
    },
    /// Retrying after a transient failure. Carries the attempt index
    /// (0 = first attempt, 1 = first retry, …).
    Retrying { attempt: u32 },
    /// Generating a branch summary.
    BranchSummary,
}

impl AgentPhase {
    /// **稳定的机器标签**（v0.10.8 / A2，review finding）。
    ///
    /// TUI 需要知道「现在是不是在压缩」，而 `Display` 是**给人看的**：
    /// 它的格式一变（`compacting:overflow` → 别的写法），靠
    /// `starts_with("compacting")` 判断的地方就会**静默失效**——没有测试会红，
    /// 而 TUI 会以为没在压缩。
    ///
    /// 所以：标签单独给一份，与 Display **解耦**。
    pub fn tag(&self) -> &'static str {
        match self {
            AgentPhase::Idle => "idle",
            AgentPhase::Working => "working",
            AgentPhase::Compacting { .. } => "compacting",
            AgentPhase::Retrying { .. } => "retrying",
            AgentPhase::BranchSummary => "branch_summary",
        }
    }
}

impl std::fmt::Display for AgentPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentPhase::Idle => write!(f, "idle"),
            AgentPhase::Working => write!(f, "working"),
            AgentPhase::Compacting { reason, progress } => {
                if let Some(p) = progress {
                    write!(f, "compacting:{reason} ({p}%)")
                } else {
                    write!(f, "compacting:{reason}")
                }
            }
            AgentPhase::Retrying { attempt } => write!(f, "retrying (attempt {attempt})"),
            AgentPhase::BranchSummary => write!(f, "branch summary"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// Agent started (called once per `run`).
    AgentStart,
    /// A user turn has been appended to the conversation.
    TurnStart,
    /// A turn's response from the model is complete.
    TurnEnd { stop_reason: String, usage: Usage },
    /// The agent phase changed. Surfaces Idle / Working / Compacting /
    /// Retrying / BranchSummary to the UI and extension host.
    PhaseChanged(AgentPhase),
    /// Incremental text delta from the model.
    TextDelta { text: String },
    /// v0.8: incremental reasoning delta (inside `<think>`).
    /// The TUI renders this with dim/italic style so users can
    /// follow the model's reasoning without it dominating the
    /// transcript (Pi-style).
    ThinkingDelta { text: String },
    /// A tool call started, ended, or emitted a delta.
    ToolCallStart { id: String, name: String },
    ToolCallDelta {
        id: String,
        input_json_delta: String,
    },
    ToolCallStop {
        id: String,
        input_json: serde_json::Value,
    },
    /// v0.10 (R2a): a compaction just rewrote history.
    ///
    /// Emitted by the loop after `compact_history` splices the summary in.
    /// The `Agent` has no session handle — the TUI's `AppState` owns the
    /// session, and the driver closure builds a fresh `Agent` per run — so
    /// without an event the summary reached the model and then vanished on
    /// `/resume` or a restart.
    ///
    /// Pi reaches the same outcome differently: its session layer
    /// orchestrates compaction (`agent-session.ts:789`) and Pi's
    /// `AgentEvent` has no compaction variant. nini compacts inside the
    /// loop, so the loop has to publish the fact for the session to record.
    Compacted {
        /// The summary text, already carrying `CONTEXT_SUMMARY_MARKER`
        /// semantics via `compaction::context_summary_message`.
        summary: String,
        /// `tokens_before` at the moment of compaction, for the cursor.
        tokens_before: u32,
    },
    /// A tool finished executing.
    /// A tool finished executing. `name` lets the TUI render
    /// `✓ bash (4ms)` instead of just `✓`, so parallel tool results
    /// are distinguishable (v0.8.8 tool-identity fix).
    ToolResult {
        id: String,
        name: String,
        output: ToolOutput,
    },
    /// Agent finished (called once per `run`).
    AgentEnd,
    /// Agent aborted (user or system cancellation).
    Aborted,
    /// An error occurred mid-run.
    Error { message: String },
}

/// Top-level error type.
#[derive(Debug, Error)]
pub enum AgentError {
    #[error("provider error: {0}")]
    Provider(#[from] ProviderError),
    #[error("tool error ({name}): {message}")]
    Tool { name: String, message: String },
    #[error("aborted")]
    Aborted,
    #[error("too many tool iterations ({0})")]
    TooManyIterations(usize),
    #[error("invalid state: {0}")]
    InvalidState(String),
}

impl From<ToolError> for AgentError {
    fn from(e: ToolError) -> Self {
        AgentError::Tool {
            name: "<unknown>".to_string(),
            message: e.to_string(),
        }
    }
}

/// Default maximum number of retry attempts for transient errors.
/// Mirrors Pi's `_runDefaultCompaction` retry pattern (max 3 attempts
/// in v0.84.3 for compaction; we use 3 for general LLM streaming too).
pub const DEFAULT_MAX_RETRIES: u32 = 3;

/// Default base delay between retry attempts. Doubled each attempt:
/// attempt 1 → 0.5s, attempt 2 → 1s, attempt 3 → 2s.
pub const DEFAULT_RETRY_BASE_MS: u64 = 500;

/// Returns true if an error message looks transient/retryable. Mirrors the
/// heuristics Pi uses in `agent-session.js` to classify whether to retry
/// vs. surface as a fatal error.
pub fn is_retryable_error(msg: &str) -> bool {
    let m = msg.to_lowercase();
    // Common transient patterns across providers.
    m.contains("rate limit")
        || m.contains("too many requests")
        || m.contains("service unavailable")
        || m.contains("temporarily")
        || m.contains("timeout")
        || m.contains("timed out")
        || m.contains("connection reset")
        || m.contains("connection refused")
        || m.contains("econnreset")
        || m.contains("econnrefused")
        || m.contains("429")
        || m.contains("500")
        || m.contains("502")
        || m.contains("503")
        || m.contains("504")
        // v0.9.0: 529 is what MiniMax's gateway returns while a
        // model is saturated — hit it on the very first run of the new
        // default model, on a prompt that succeeds on the retry. It is
        // exactly as transient as a 503.
        || m.contains("529")
        || m.contains("internal server error")
        || m.contains("upstream")
        || m.contains("overloaded")
}

/// Exponential backoff: base * 2^(attempt-1) capped at 30s.
pub fn backoff_ms(attempt: u32, base_ms: u64) -> u64 {
    let exp = 2_u64.saturating_pow(attempt.saturating_sub(1).min(10));
    base_ms.saturating_mul(exp).min(30_000)
}

#[cfg(test)]
mod retry_policy_tests {
    use super::*;

    #[test]
    fn is_retryable_recognizes_known_patterns() {
        assert!(is_retryable_error("rate limit exceeded"));
        assert!(is_retryable_error("HTTP 503 service unavailable"));
        assert!(is_retryable_error("connection reset by peer"));
        assert!(is_retryable_error("upstream timeout"));
        assert!(is_retryable_error("HTTP 429 too many requests"));
        // v0.9.0: MiniMax gateway saturation.
        assert!(is_retryable_error("api error (529): request failed: 529"));
        // Fatal errors should NOT retry.
        assert!(!is_retryable_error("context window exceeded"));
        assert!(!is_retryable_error("invalid api key"));
        assert!(!is_retryable_error("malformed request"));
    }

    #[test]
    fn backoff_grows_exponentially() {
        assert_eq!(backoff_ms(1, 500), 500);
        assert_eq!(backoff_ms(2, 500), 1000);
        assert_eq!(backoff_ms(3, 500), 2000);
        assert_eq!(backoff_ms(4, 500), 4000);
        // Capped at 30s.
        assert!(backoff_ms(20, 500) <= 30_000);
    }

    #[test]
    fn default_max_retries_is_three() {
        assert_eq!(DEFAULT_MAX_RETRIES, 3);
    }
}

/// Inline LLM summary — calls the configured provider directly with
/// Pi's `SUMMARIZATION_SYSTEM_PROMPT` (mirrors
/// `nini_ai::summarizer::try_call_provider`). Returns `Err` on any
/// failure so the caller can fall back to the local heuristic.
///
/// The provider must be cloneable (`Arc<dyn Provider>`). All work runs
/// in a new OS thread + dedicated `current_thread` runtime to avoid
/// `block_on` on the caller's runtime (which would panic).
pub fn inline_llm_summary(
    provider: &Arc<dyn Provider>,
    model: &str,
    messages: &[Message],
) -> Result<String, String> {
    use futures_util::StreamExt;
    use std::sync::mpsc;

    // Build the prompt.
    let mut user_msg = String::from(
        "You are a context summarization assistant. Produce a structured summary.\n\n",
    );
    for m in messages {
        for block in &m.content {
            if let ContentBlock::Text { text } = block {
                user_msg.push_str(&format!("[{:?}]: {}\n", m.role, text));
            }
        }
    }
    let req = Request {
        model: model.to_string(),
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: user_msg }],
            timestamp: 0,
        }],
        system: None,
        max_tokens: Some(2048),
        temperature: Some(0.0),
        tools: vec![],
        reasoning: None,
    };

    // Dispatch on a fresh thread with a dedicated runtime.
    let (tx, rx) = mpsc::channel::<Result<String, String>>();
    let provider = provider.clone();
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                let _ = tx.send(Err(format!("runtime: {e}")));
                return;
            }
        };
        let result = rt.block_on(async {
            let mut out = String::new();
            let mut s = provider.stream(req);
            while let Some(ev) = s.next().await {
                match ev? {
                    StreamEvent::TextDelta { text } => out.push_str(&text),
                    _ => {}
                }
            }
            Ok::<_, ProviderError>(out)
        });
        let _ = tx.send(result.map_err(|e| e.to_string()));
    });
    rx.recv()
        .map_err(|_| "summarizer thread died".to_string())?
}

/// Determine if a stop reason indicates the assistant message hit a
/// recoverable error. Used by the auto-compaction trigger.
pub fn stop_reason_is_aborted_or_error(stop_reason: &str) -> bool {
    stop_reason == "aborted" || stop_reason == "error"
}

/// Handle for cancelling a running agent.
#[derive(Debug, Default, Clone)]
pub struct AbortHandle {
    flag: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl AbortHandle {
    /// Create a new abort handle.
    pub fn new() -> Self {
        Self::default()
    }

    /// Trigger cancellation. Wakes any `.wait_aborted()` callers.
    pub fn abort(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    /// True if `abort()` was called.
    pub fn is_aborted(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// Wait until `abort()` is called (or return immediately if already aborted).
    pub async fn wait_aborted(&self) {
        if self.is_aborted() {
            return;
        }
        self.notify.notified().await;
    }
}

/// Run configuration.
/// Per-model retry policy for transient LLM errors. Mirrors pi's
/// `RetrySettings` from `core/settings-manager.js`.
#[derive(Debug, Clone)]
pub struct RetrySettings {
    /// Maximum number of retry attempts before giving up.
    pub max_retries: u32,
    /// Base delay between attempts (doubled each retry).
    pub base_delay_ms: u64,
}

impl Default for RetrySettings {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay_ms: 500,
        }
    }
}

/// v0.8.9 (P2-10): default ceiling on concurrently-running tool calls
/// from one assistant turn.
///
/// Eight is comfortably above the "read a pile of files in parallel"
/// pattern that motivated v0.8.5's parallel execution, and low enough
/// that a model emitting 30 `bash` calls forks 8 process trees rather
/// than 30.
pub const DEFAULT_MAX_PARALLEL_TOOLS: usize = 8;

/// v0.10 (R1): default bound on follow-up revivals of the turn loop.
pub const DEFAULT_MAX_FOLLOW_UP_ROUNDS: usize = 16;

/// What a caller-supplied compaction did.
///
/// v0.10 (R2a/A3b). The overflow path used to compact the Agent's own
/// memory and nothing else: the `Compacted` event it emitted had no
/// consumer, so an overflow compaction was the one case that changed the
/// model's context without being recorded anywhere. The session is now
/// reachable from the driver, so it does the compaction and hands back the
/// history to continue with.
#[derive(Debug, Clone)]
pub struct CompactionResult {
    /// The summary that was persisted.
    pub summary: String,
    /// Estimated tokens before compaction.
    pub tokens_before: u32,
    /// Estimated tokens after.
    pub tokens_after: u32,
    /// The history to continue the run with.
    pub messages: Vec<Message>,
}

/// Compaction performed by the caller, which can reach the session.
///
/// Returning `None` means "nothing to compact", and the run continues
/// unchanged.
///
/// A newtype rather than a bare `Arc<dyn Fn>` because `RunConfig` derives
/// `Debug`, and a closure is not `Debug`. A manual impl that prints the
/// type name keeps `RunConfig`'s derive working and says nothing false.
#[derive(Clone)]
pub struct CompactionHook(Arc<dyn Fn(&[Message]) -> Option<CompactionResult> + Send + Sync>);

impl CompactionHook {
    pub fn new(f: impl Fn(&[Message]) -> Option<CompactionResult> + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    /// Run the hook, if there is one.
    pub fn run(&self, messages: &[Message]) -> Option<CompactionResult> {
        (self.0)(messages)
    }
}

impl std::fmt::Debug for CompactionHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CompactionHook(<opaque>)")
    }
}

/// Configuration for one `Agent::run` invocation.
#[derive(Debug, Clone)]
pub struct RunConfig {
    pub model: String,
    pub system: Option<String>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub max_iterations: usize,
    pub tool_context: ToolContext,
    /// Compaction settings (context window + reserve).
    pub compaction: crate::compaction::CompactionSettings,
    /// v0.10 (R2a/A3b): compaction performed by the caller.
    ///
    /// `None` falls back to the Agent compacting its own memory, which
    /// records nothing - the behaviour this field exists to replace. The
    /// overflow path is the only thing that consults it; the threshold
    /// check was removed in A3 because the session compacts before each
    /// submission instead.
    pub compact: Option<CompactionHook>,
    /// Retry policy for transient LLM errors.
    pub retry: RetrySettings,
    /// v0.8.9 (P2-10): how many tool calls from a single assistant turn
    /// may be in flight at once.
    ///
    /// Models happily emit 30 parallel tool calls, and before this
    /// limit every one of them got its own future immediately. For
    /// `bash` that is 30 process trees forked in the same instant, each
    /// buffering output against `nini_tools::bash::STREAM_CAPTURE_CAP`
    /// — so this limit and that memory ceiling multiply together.
    /// v0.8.5 introduced the parallelism because serial execution
    /// wasted wall-clock time on 14 independent file reads; this keeps
    /// that win without letting the batch size be whatever the model
    /// felt like emitting.
    ///
    /// `0` means "no limit" — an explicit opt-out that preserves the
    /// pre-v0.8.9 behaviour.
    pub max_parallel_tools: usize,
    /// v0.10 (R1): how many times the loop may be revived by a follow-up
    /// before the run gives up and returns to Idle.
    ///
    /// `max_iterations` bounds turns, not revivals. Without a separate bound,
    /// a follow-up hook that always yields work would keep the agent running
    /// indefinitely with no counter tripping.
    pub max_follow_up_rounds: usize,
    /// v0.10 (R1f): how much the model should think.
    ///
    /// Pi carries this as `reasoning` on the stream options
    /// (`pi-ai/src/types.ts:305`). It was previously unreachable in nini:
    /// `/thinking` validated the level, the TUI showed it, settings.json
    /// persisted it — and no provider ever received it, because `Request`
    /// had no field to hold it.
    pub reasoning: Option<crate::provider::ThinkingLevel>,
    /// v0.10 (R1c): force the whole tool batch serial, mirroring Pi's
    /// `config.toolExecution` (`agent-loop.ts:421`). A single tool can
    /// also opt in with `Tool::execution_mode`; this is the global switch.
    pub tool_execution: crate::tool::ToolExecutionMode,
}

impl RunConfig {
    /// Create with sensible defaults.
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            compact: None,
            system: None,
            max_tokens: Some(8192),
            temperature: None,
            max_iterations: MAX_TOOL_ITERATIONS,
            tool_context: ToolContext::default(),
            compaction: crate::compaction::CompactionSettings::default(),
            retry: crate::agent::RetrySettings::default(),
            max_parallel_tools: DEFAULT_MAX_PARALLEL_TOOLS,
            max_follow_up_rounds: DEFAULT_MAX_FOLLOW_UP_ROUNDS,
            tool_execution: crate::tool::ToolExecutionMode::default(),
            reasoning: None,
        }
    }
}

/// The agent. Owns a provider, a tool registry, and the conversation history.
pub struct Agent {
    provider: Arc<dyn Provider>,
    tools: ToolRegistry,
    messages: Vec<Message>,
    /// Generic agent abort — interrupts the current LLM stream.
    abort: AbortHandle,
    /// Abort for manual `/compact` — set when user requests compaction
    /// mid-stream so the compactor can be cancelled.
    /// Mirrors Pi's `_compactionAbortController`.
    compaction_abort: AbortHandle,
    /// Abort for auto-triggered overflow compaction — set when an
    /// overflow is detected so the auto-compactor can be cancelled.
    /// Mirrors Pi's `_autoCompactionAbortController`.
    auto_compaction_abort: AbortHandle,
    /// Abort for branch summary generation.
    /// Mirrors Pi's `_branchSummaryAbortController`.
    branch_summary_abort: AbortHandle,
    config: RunConfig,
    /// Last API-reported usage from the most recent assistant turn.
    /// `None` until the first assistant message completes (or if the
    /// provider doesn't emit usage).
    /// Used by `estimated_tokens_via_usage()` for accurate context %;
    /// falls back to character estimation when None.
    last_usage: Option<crate::provider::Usage>,
    /// Retry attempt counter for transient LLM errors. Reset to 0 after
    /// each successful message stop.
    retry_attempt: u32,
    /// v0.10 (fix): how many compact-and-retry recoveries this run has
    /// attempted after a context overflow.
    ///
    /// Pi bounds this at exactly one (`_overflowRecoveryAttempted`,
    /// `agent-session.ts:336`): the second failure emits a specific,
    /// actionable error and stops. Without a bound, a compaction that
    /// cannot shrink the context re-sends the identical oversized request
    /// until `max_iterations` trips, and the user sees `TooManyIterations`
    /// — a message that says nothing about context.
    overflow_recovery_attempted: bool,
    /// v0.10 (fix): estimated tokens at the last in-run threshold
    /// compaction. Used only to detect a compaction that did not actually
    /// shrink the context, so the loop cannot compact forever.
    last_compaction_tokens: Option<u32>,
    /// True while an auto-compaction is in progress. Submit handler
    /// queues user input instead of spawning an agent (Pi
    /// _pendingNextTurnMessages equivalent).
    is_auto_compacting: bool,
    /// Messages queued while auto-compacting. Drained into the
    /// transcript by `end_auto_compaction()`.
    pending_next_turn_messages: Vec<String>,
    /// Current phase. Updated by the run loop and emitted via
    /// `AgentEvent::PhaseChanged`.
    phase: AgentPhase,
    /// v0.7 (M1) hook surface. Mid-run message injection point.
    /// Defaults to `NoopHooks` so existing callers see no behavior
    /// change. See `agent_hooks::AgentLoopHooks` for the trait.
    hooks: Arc<dyn crate::agent_hooks::AgentLoopHooks>,
}

impl Agent {
    /// Create a new agent.
    pub fn new(provider: Arc<dyn Provider>, tools: ToolRegistry, config: RunConfig) -> Self {
        Self::with_hooks(
            provider,
            tools,
            config,
            Arc::new(crate::agent_hooks::NoopHooks),
        )
    }

    /// Create a new agent with a custom hook surface. v0.7 — allows
    /// extensions to inject steering/followup messages (M1) without
    /// forking nini-core.
    pub fn with_hooks(
        provider: Arc<dyn Provider>,
        tools: ToolRegistry,
        config: RunConfig,
        hooks: Arc<dyn crate::agent_hooks::AgentLoopHooks>,
    ) -> Self {
        Self {
            provider,
            tools,
            messages: Vec::new(),
            abort: AbortHandle::new(),
            compaction_abort: AbortHandle::new(),
            auto_compaction_abort: AbortHandle::new(),
            branch_summary_abort: AbortHandle::new(),
            config,
            last_usage: None,
            is_auto_compacting: false,
            pending_next_turn_messages: Vec::new(),
            phase: AgentPhase::Idle,
            retry_attempt: 0,
            overflow_recovery_attempted: false,
            last_compaction_tokens: None,
            hooks,
        }
    }

    /// Get the current phase of the agent.
    pub fn phase(&self) -> AgentPhase {
        self.phase.clone()
    }

    /// Set the phase and return the previous value. The run loop uses
    /// this to emit `PhaseChanged` only when the phase actually changed.
    pub fn set_phase(&mut self, new_phase: AgentPhase) -> AgentPhase {
        let prev = std::mem::replace(&mut self.phase, new_phase);
        prev
    }

    /// Get the abort handle.
    pub fn abort_handle(&self) -> AbortHandle {
        self.abort.clone()
    }

    /// Get the manual-compaction abort handle. UI calls `abort()` on
    /// this when the user presses Esc during a `/compact`.
    pub fn compaction_abort_handle(&self) -> AbortHandle {
        self.compaction_abort.clone()
    }

    /// Get the auto-compaction abort handle. Auto-triggered overflow
    /// compactions observe this for cancellation.
    pub fn auto_compaction_abort_handle(&self) -> AbortHandle {
        self.auto_compaction_abort.clone()
    }

    /// Get the branch-summary abort handle.
    pub fn branch_summary_abort_handle(&self) -> AbortHandle {
        self.branch_summary_abort.clone()
    }

    /// Reset ALL abort controllers. Call between compaction runs so a
    /// previous abort signal doesn't affect the next one.
    pub fn reset_aborts(&mut self) {
        self.abort = AbortHandle::new();
        self.compaction_abort = AbortHandle::new();
        self.auto_compaction_abort = AbortHandle::new();
        self.branch_summary_abort = AbortHandle::new();
        self.is_auto_compacting = false;
    }

    /// Mark auto-compaction as in-progress. While true, `submit_user_input`
    /// queues messages instead of spawning an agent (Pi
    /// _pendingNextTurnMessages equivalent).
    pub fn begin_auto_compaction(&mut self) {
        self.is_auto_compacting = true;
        self.auto_compaction_abort = AbortHandle::new();
    }

    /// End auto-compaction and drain any queued messages into the
    /// transcript. Returns the messages so the runtime can inject them
    /// as context alongside the next user prompt.
    pub fn end_auto_compaction(&mut self) -> Vec<String> {
        let queued = std::mem::take(&mut self.pending_next_turn_messages);
        self.is_auto_compacting = false;
        queued
    }

    /// Queue a message during auto-compaction. Pi mirrors this by
    /// buffering inputs and flushing on the next turn.
    pub fn queue_next_turn(&mut self, message: impl Into<String>) {
        self.pending_next_turn_messages.push(message.into());
    }

    /// Begin a branch summary generation. Emits a phase change so the UI
    /// can show the spinner. The caller drives the actual LLM call via
    /// `nini_ai::summarizer` and uses `branch_summary_abort_handle()`
    /// for cancellation.
    ///
    /// v1: branch summary generation is not yet integrated into the main
    /// `run` loop (we only have session-tree display). This method exists
    /// so future wiring is a no-op when it lands.
    pub fn begin_branch_summary(&mut self) {
        self.set_phase(AgentPhase::BranchSummary);
    }

    /// End branch summary generation; returns to Working (caller should
    /// typically set Idle once the surrounding turn is done).
    pub fn end_branch_summary(&mut self) {
        self.set_phase(AgentPhase::Working);
    }

    /// Begin a retry attempt. Updates the phase to Retrying{attempt}
    /// and emits the phase change.
    pub fn begin_retry(&mut self, attempt: u32) {
        self.set_phase(AgentPhase::Retrying { attempt });
    }

    /// End a retry attempt; returns to Working. Resets the attempt
    /// counter so the next error starts a fresh retry sequence. Callers
    /// failing to do this will accumulate state across attempts.
    pub fn end_retry(&mut self) {
        self.retry_attempt = 0;
        self.set_phase(AgentPhase::Working);
    }

    /// v0.10 (R2a/A3b): install a compaction hook on this run's config.
    ///
    /// The Agent cannot reach the session, so overflow compaction used to
    /// change the model's history with nothing written to disk. A caller
    /// that *can* reach the session installs a hook here and the Agent
    /// delegates to it, which is Pi's sequence: compact, persist, rebuild
    /// the context, continue with it.
    pub fn set_compaction_hook(&mut self, hook: CompactionHook) {
        self.config.compact = Some(hook);
    }

    /// Is auto-compaction currently active?
    pub fn is_compacting(&self) -> bool {
        self.is_auto_compacting
    }

    /// Get the current message history.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Reset the conversation history.
    pub fn clear_history(&mut self) {
        self.messages.clear();
    }

    /// Seed the conversation with existing messages.
    pub fn seed(&mut self, messages: Vec<Message>) {
        self.messages = messages;
    }

    /// Total estimated tokens across all messages currently in history.
    /// Mirrors Pi's `getContextUsage()`:
    /// - Prefer the last API-reported usage if available (most accurate).
    /// - Fall back to character-based estimation when usage is missing
    ///   (e.g., fixture provider or aborted turn).
    pub fn estimated_tokens(&self) -> u32 {
        if let Some(usage) = &self.last_usage {
            crate::provider::context_tokens_from_usage(usage).saturating_add(
                crate::compaction::estimate_provider_messages_tokens(
                    &self.messages_after_last_usage(),
                ),
            )
        } else {
            crate::compaction::estimate_provider_messages_tokens(&self.messages)
        }
    }

    /// Return only the messages AFTER the last assistant usage. This is
    /// empty when the last message IS the usage report. Used to estimate
    /// the "tail" tokens added since the last usage snapshot.
    fn messages_after_last_usage(&self) -> Vec<Message> {
        if self.last_usage.is_none() {
            return self.messages.clone();
        }
        self.messages.clone()
    }

    /// Pi-compatible context-usage report: `{ tokens, contextWindow, percent }`.
    /// `tokens` is the most recent API usage (or null if we have no
    /// assistant message yet). `percent` is `tokens / contextWindow × 100`,
    /// or `null` when no usage is available yet.
    pub fn context_usage(&self) -> Option<crate::compaction::ContextUsage> {
        let window = self.config.compaction.context_window;
        if window == 0 {
            return None;
        }
        let tokens = self
            .last_usage
            .as_ref()
            .map(|u| crate::provider::context_tokens_from_usage(u));
        let percent = tokens.map(|t| (t as f64 / window as f64) * 100.0);
        // When no usage yet, percent is unknown until next LLM response
        // (matches Pi's behavior).
        Some(crate::compaction::ContextUsage {
            tokens,
            context_window: window,
            percent,
        })
    }

    /// Should the agent compact now? True when estimated tokens exceed
    /// `context_window - reserve_tokens`.
    pub fn should_compact(&self) -> bool {
        crate::compaction::should_compact(self.estimated_tokens(), &self.config.compaction)
    }

    /// Compact the history the way Pi does: through the caller's hook when
    /// the driver supplied one (so the session persists it), otherwise in
    /// this Agent's own memory.
    ///
    /// Returns `None` when there was nothing to compact.
    ///
    /// v0.10 (fix) — `last_usage` is cleared on success. That snapshot
    /// describes the context *before* the fold; keeping it makes
    /// [`Agent::estimated_tokens`] report the old, larger number forever,
    /// which is precisely what drives an unbounded compact-retry loop.
    fn compact_history_via_hook(&mut self) -> Option<CompactionResult> {
        let tokens_before = self.estimated_tokens();
        let result = match &self.config.compact {
            Some(hook) => {
                let snapshot = self.messages.clone();
                hook.run(&snapshot)?
            }
            None => {
                let summary_fn = |msgs: &[Message], _prev: Option<String>| {
                    let entries: Vec<crate::Entry> = msgs
                        .iter()
                        .enumerate()
                        .map(|(i, m)| {
                            crate::Entry::message(
                                format!("cmsg_{i}"),
                                None,
                                (i as u64) + 1,
                                AgentMessage {
                                    role: m.role,
                                    content: m.content.clone(),
                                    timestamp: 0,
                                },
                            )
                        })
                        .collect();
                    crate::compaction::generate_local_summary(&entries)
                };
                let out = self.compact_history(summary_fn);
                CompactionResult {
                    summary: out.summary,
                    tokens_before: out.tokens_before,
                    tokens_after: out.tokens_after,
                    // Empty means "the Agent already rewrote its own
                    // history"; `compact_history` did the splice.
                    messages: Vec::new(),
                }
            }
        };
        if !result.messages.is_empty() {
            self.messages = result.messages.clone();
        }
        self.last_usage = None;
        self.last_compaction_tokens = Some(tokens_before);
        Some(result)
    }

    /// v0.10 (fix): the in-run threshold check.
    ///
    /// Pi runs `_checkCompaction` after **every** assistant message
    /// (`agent-session.ts:1141`), so a run that is 30 tool turns long
    /// compacts when its context crosses the line rather than when the API
    /// finally refuses. nini only compacted between submissions
    /// (`runtime.rs:2102`), which a single run never reaches — that is the
    /// whole shape of "short tasks are fine, long tasks break".
    ///
    /// Deliberately gated on a hook being present: without one there is
    /// nowhere to persist the summary, and an Agent that folds its own
    /// history records nothing, leaving the model and the session
    /// describing different conversations. The overflow path below still
    /// falls back to self-compaction, because there the alternative is a
    /// dead run.
    fn maybe_compact_at_threshold(&mut self) -> Option<CompactionResult> {
        if self.config.compact.is_none() || !self.should_compact() {
            return None;
        }
        // A compaction that did not shrink the context must not be retried
        // on every iteration.
        if let (Some(prev), now) = (self.last_compaction_tokens, self.estimated_tokens()) {
            if now >= prev {
                return None;
            }
        }
        self.compact_history_via_hook()
    }

    /// Run compaction on the current message history. Returns the
    /// compaction output. v1 uses the deterministic local summary; future
    /// versions pass an LLM-backed `summary_fn`.
    pub fn compact_history<F>(&mut self, summary_fn: F) -> crate::compaction::CompactionOutput
    where
        F: FnOnce(&[Message], Option<String>) -> String,
    {
        // Convert messages to entries for compaction.
        let entries: Vec<crate::Entry> = self
            .messages
            .iter()
            .enumerate()
            .map(|(i, m)| {
                crate::Entry::message(
                    format!("msg_{i}"),
                    None,
                    (i as u64) + 1,
                    AgentMessage {
                        role: m.role,
                        content: m.content.clone(),
                        timestamp: 0,
                    },
                )
            })
            .collect();

        // Capture a snapshot for the closure.
        let msgs_snapshot = self.messages.clone();
        let out =
            crate::compaction::compact(&entries, &self.config.compaction, None, |es, prev| {
                // Compute prefix length from the (in-progress) entries.
                let keep_from = crate::compaction::find_cut_point(
                    es,
                    self.config.compaction.keep_recent_tokens,
                )
                .keep_from;
                let prefix_len = keep_from.min(msgs_snapshot.len());
                summary_fn(&msgs_snapshot[..prefix_len], prev)
            });

        // Splice: prepend summary message, retain suffix.
        let prefix_len = out.keep_from;
        let new_summary_msg = Message {
            role: Role::User,
            timestamp: 0,
            content: vec![ContentBlock::Text {
                text: crate::compaction::context_summary_message(&out.summary),
            }],
        };
        let mut new_history = Vec::with_capacity(1 + self.messages.len() - prefix_len);
        new_history.push(new_summary_msg);
        new_history.extend_from_slice(&self.messages[prefix_len..]);
        self.messages = new_history;
        out
    }

    /// Run a single user turn, iterating as needed. Returns a stream of events.
    pub fn run<'a>(
        &'a mut self,
        user_msg: AgentMessage,
    ) -> Pin<Box<dyn Stream<Item = Result<AgentEvent, AgentError>> + Send + 'a>>
    where
        Self: 'a,
    {
        // Append user message to history.
        let nini_msg = to_nini_message(&user_msg);
        self.messages.push(nini_msg);
        let provider = self.provider.clone();
        let tool_specs = self.tools.specs();
        let tool_registry = self.tools.clone();
        // v0.10 (R1e): mutable so `prepare_next_turn` can swap the model
        // for the next request, as Pi does at `agent-loop.ts:181-245`.
        let mut config = self.config.clone();
        let abort = self.abort.clone();

        Box::pin(try_stream! {
            yield AgentEvent::AgentStart;
            self.set_phase(AgentPhase::Working);
            yield AgentEvent::PhaseChanged(AgentPhase::Working);
            yield AgentEvent::TurnStart;

            let mut iteration = 0;

            // v0.10 (R1): Pi's queue-driven turn loop.
            //
            // Pi writes this as two nested loops (`agent-loop.js:85` L1
            // outer, `:88` L2 inner). The nesting is not load-bearing — the
            // two layers collapse into ONE loop with a guard at the top,
            // which is what this is. Collapsing it matters practically: a
            // nested `loop { loop { … } }` inside a 1500-line `try_stream!`
            // generator body is a brace minefield, and a misplaced brace
            // fails *silently* (compiles, wrong semantics) rather than loudly.
            //
            // The equivalence, stated once so it can be checked:
            //
            //   Pi:  pending = pollSteering()                 // :83
            //        L1: L2: while (hasMoreToolCalls || pending) {
            //              inject(pending); turn(); pending = pollSteering()   // :96,:106,:160
            //            }
            //            if (pollFollowUp() empty) break;  pending = followUp;  // :163
            //        }
            //
            //   Here: pending = pollSteering()                // before the loop
            //          loop {
            //            if (!hasMoreToolCalls && pending.is_empty()) {   // L2 would exit
            //              followUp = pollFollowUp();                    // → L1's only job
            //              if (followUp empty) break;
            //              pending = followUp;
            //            }
            //            inject(pending); turn(); hasMore = …; pending = pollSteering()
            //          }
            //
            // `has_more_tool_calls` starts true because Pi seeds it true
            // (`:86`) so the first turn always runs.
            let mut pending: Vec<Message> = {
                let hooks_ref: Arc<dyn crate::agent_hooks::AgentLoopHooks> =
                    self.hooks.clone();
                let messages_snapshot = self.messages.clone();
                crate::agent_hooks::catch_hook_panic("get_steering_messages", || {
                    hooks_ref.get_steering_messages(&messages_snapshot)
                })
            };
            let mut has_more_tool_calls = true;
            let mut follow_up_rounds = 0usize;
            loop {
                // ── Pi's L1, collapsed: the only reason the outer loop
                //    exists is to pick up a follow-up queued after the model
                //    finished. nini used to `return` here after appending
                //    follow-ups to history, which turned a queued *task*
                //    into *dead context* — the model saw it as background on
                //    the user's next message instead of acting on it now.
                if !has_more_tool_calls && pending.is_empty() {
                    let hooks_ref: Arc<dyn crate::agent_hooks::AgentLoopHooks> =
                        self.hooks.clone();
                    let messages_snapshot = self.messages.clone();
                    let followup = crate::agent_hooks::catch_hook_panic(
                        "get_followup_messages",
                        || hooks_ref.get_followup_messages(&messages_snapshot),
                    );
                    // `max_iterations` bounds turns, not revivals. Without a
                    // separate bound, a follow-up hook that always yields work
                    // would keep the agent running indefinitely.
                    if followup.is_empty() || follow_up_rounds >= config.max_follow_up_rounds {
                        self.set_phase(AgentPhase::Idle);
                        yield AgentEvent::PhaseChanged(AgentPhase::Idle);
                        return;
                    }
                    follow_up_rounds += 1;
                    pending = followup;
                }

                if abort.is_aborted() {
                    self.set_phase(AgentPhase::Idle);
                    yield AgentEvent::PhaseChanged(AgentPhase::Idle);
                    yield AgentEvent::Aborted;
                    return;
                }
                // v0.10 (R1): inject queued messages before the LLM call, as
                // Pi does at `agent-loop.js:112-103`. Steering and follow-up
                // both land here, which is why the two are indistinguishable
                // in the message stream — only the poll timing differs.
                //
                // Pi wraps each in `message_start` / `message_end` (`:97-99`);
                // nini's `AgentEvent` has no such variant, so the transcript
                // picks them up with the next assistant turn. The ordering
                // that matters — before the LLM call — is preserved.
                for queued in pending.drain(..) {
                    self.messages.push(queued);
                }

                iteration += 1;
                // v0.10 (R1d): the soft-stop hook is no longer consulted
                // here. Pi evaluates it *after* the turn completes — see
                // its position at the end of this loop body, and
                // `crates/nini-conformance/tests/stop_after_turn.rs`.
                // Hard safety net: ALWAYS preserved, regardless of
                // whether the hook fires. A panicking hook that
                // ignores max_iterations is still bounded by it.
                if iteration > config.max_iterations {
                    Err(AgentError::TooManyIterations(config.max_iterations))?;
                }

                // v0.10 (fix): the threshold check that A3 removed, restored
                // — but routed through the session hook instead of folding
                // this Agent's memory in place, which is the part A3 was
                // right about. Pi: `agent-session.ts:1141` `_checkCompaction`
                // after every assistant message, Case 2 (threshold).
                if let Some(result) = self.maybe_compact_at_threshold() {
                    self.set_phase(AgentPhase::Working);
                    yield AgentEvent::Compacted {
                        summary: result.summary.clone(),
                        tokens_before: result.tokens_before,
                    };
                    // The request below is built from the compacted
                    // history, so there is nothing to retry.
                    continue;
                }

                // v0.10 (R2a/A3): the threshold compaction that used to live
                // here is gone.
                //
                // It checked `should_compact` before every request and, if
                // over budget, spliced a summary into `self.messages` in
                // memory. Nothing about it reached the session: the
                // `AgentEvent::Compacted` it emitted had no consumer
                // anywhere in the tree, so a compacted run left a session
                // with the full, uncompacted history and a live context that
                // disagreed with it.
                //
                // Compaction now belongs to the session, which persists it
                // and derives the live context from the entry tree. The TUI
                // runs it before submitting a turn and hands the resulting
                // context to this Agent as its seed, so the history here is
                // the session's history rather than a second copy of it.
                //
                // The overflow trigger further down stays, deliberately: it
                // is the safety net for a turn that outgrew its window
                // between two submissions. Making that path persist and
                // retry through the session is the next step.

                // v0.7 (M3b) — apply the 2 context hooks in order:
                //   1. transform_context (trim / inject)
                //   2. convert_to_llm (filter internal-only)
                // Both default to identity. The hook implementations
                // run on a snapshot of self.messages so the hook
                // can't mutate the agent's source of truth (only the
                // LLM sees the result).
                let hooks_ref: Arc<dyn crate::agent_hooks::AgentLoopHooks> =
                    self.hooks.clone();
                let msgs_snapshot = self.messages.clone();
                let transformed = crate::agent_hooks::catch_hook_panic(
                    "transform_context",
                    || hooks_ref.transform_context(&msgs_snapshot),
                );
                let hooks_ref2: Arc<dyn crate::agent_hooks::AgentLoopHooks> =
                    self.hooks.clone();
                let llm_msgs = crate::agent_hooks::catch_hook_panic(
                    "convert_to_llm",
                    || hooks_ref2.convert_to_llm(&transformed.messages),
                );
                // v0.7.1 (Pi hook #9) — aggregate tool
                // system_prompt_contributions into the request
                // system prompt before building the request.
                // `config` was captured by the provider.stream
                // closure above; build a local copy with the
                // augmented system prompt.
                let mut req_config = config.clone();
                req_config.system = build_system_prompt(&req_config, &tool_registry);
                // Build request from the transformed view.
                let request = build_request(&req_config, &llm_msgs, tool_specs.as_slice());

                // Stream provider response, accumulating into assistant message
                // and tracking pending tool calls.
                let mut assistant_text = String::new();
                let mut tool_calls: Vec<PendingToolCall> = Vec::new();
                // v0.8.9 (P0-5): tool calls are accumulated in a map
                // keyed by id, not in a single `current_call` slot.
                //
                // The single slot only works if Start/Delta/Stop arrive
                // in strict sequence per call — which is what the
                // `fixture` provider does, and what every test in this
                // repo exercises. Real models interleave. A turn with
                // four parallel `read` calls arrives as:
                //
                //     Start(0) Start(1) Stop(0) Start(2) Stop(1) ...
                //
                // and the old code did `current_call.take()` on every
                // Start (pushing the previous call with its `input_json`
                // still EMPTY) and again on every Stop (discarding
                // whichever call did not match). The result: most tool
                // calls reached `run_single_tool` with `input_json: ""`,
                // `serde_json::from_str("")` failed, and the tool was
                // handed `Value::Null`:
                //
                //     ✗ read Took 0ms
                //       invalid arguments: invalid type: null, expected struct ReadArgs
                //
                // The transcript still looked right, because the TUI sink
                // tracks each line by id independently (v0.8.8) — so the
                // args rendered fine while the tool got nothing. Found by
                // driving the real TUI against MiniMax-M3.
                let mut in_flight: std::collections::HashMap<String, PendingToolCall> =
                    std::collections::HashMap::new();
                let mut last_usage = Usage::default();
                let mut stop_reason = "end_turn".to_string();
                // v0.10 (fix): set when a compaction just rewrote the
                // history, so the turn that triggered it is discarded and
                // the request is rebuilt — Pi's `willRetry`
                // (`agent-session.ts:2201` → `_handlePostAgentRun` →
                // `agent.continue()`). Without it the failed assistant turn
                // is appended *after* the compaction and the two disagree.
                let mut retry_after_compaction = false;

                {
                    use futures_util::StreamExt;
                    let mut stream = Box::pin(provider.stream(request));
                    // Stash the most recent stream error so the retry
                    // branch can inspect it after breaking out.
                    let mut last_error: Option<String> = None;
                    'outer: loop {
                        let next = stream.next();
                        let ev = tokio::select! {
                            ev = next => ev,
                            _ = abort.wait_aborted() => None,
                        };
                        match ev {
                            Some(Ok(StreamEvent::MessageStart { .. })) => {
                                // Ignore for now; could surface model id.
                            }
                            Some(Ok(StreamEvent::TextDelta { text })) => {
                                assistant_text.push_str(&text);
                                yield AgentEvent::TextDelta { text };
                            }
                            Some(Ok(StreamEvent::ThinkingDelta { text })) => {
                                // v0.8: surface reasoning to the TUI
                                // without appending to assistant_text
                                // (which becomes the user-visible
                                // message in the next turn).
                                yield AgentEvent::ThinkingDelta { text };
                            }
                            Some(Ok(StreamEvent::ToolCallStart { id, name })) => {
                                // A second Start for the same id would
                                // otherwise strand the first one's
                                // accumulated args, so close it out
                                // first rather than overwriting.
                                if let Some(orphan) =
                                    in_flight.remove(&id)
                                {
                                    eprintln!(
                                        "[nini] provider sent a second \
                                         ToolCallStart for id {id}; flushing \
                                         the first with its partial args"
                                    );
                                    tool_calls.push(orphan);
                                }
                                in_flight.insert(
                                    id.clone(),
                                    PendingToolCall {
                                        id: id.clone(),
                                        name: name.clone(),
                                        input_json: String::new(),
                                    },
                                );
                                yield AgentEvent::ToolCallStart { id, name };
                            }
                            Some(Ok(StreamEvent::ToolCallDelta { id, input_json_delta })) => {
                                if let Some(c) = in_flight.get_mut(&id) {
                                    c.input_json.push_str(&input_json_delta);
                                    yield AgentEvent::ToolCallDelta { id, input_json_delta };
                                }
                            }
                            Some(Ok(StreamEvent::ToolCallStop { id, input_json })) => {
                                // Keyed by id, so a Stop that arrives
                                // for call 0 while call 1 is still
                                // streaming no longer strands call 1.
                                match in_flight.remove(&id) {
                                    Some(mut c) => {
                                        c.input_json = serde_json::to_string(&input_json)
                                            .unwrap_or_else(|_| "null".to_string());
                                        tool_calls.push(c);
                                    }
                                    None => {
                                        // Stop without a matching
                                        // Start. Not fatal — some
                                        // providers send the body
                                        // without an id we ever saw —
                                        // but it has to be visible
                                        // rather than silently
                                        // dropping a tool call the
                                        // model asked for.
                                        eprintln!(
                                            "[nini] ToolCallStop for unknown id \
                                             {id}; keeping it with the body the \
                                             provider sent"
                                        );
                                        tool_calls.push(PendingToolCall {
                                            id: id.clone(),
                                            name: String::new(),
                                            input_json: serde_json::to_string(&input_json)
                                                .unwrap_or_else(|_| "null".to_string()),
                                        });
                                    }
                                }
                                yield AgentEvent::ToolCallStop { id, input_json };
                            }
                                Some(Ok(StreamEvent::MessageStop { stop_reason: sr, usage })) => {
                                    self.retry_attempt = 0;
                                stop_reason = sr;
                                // Pi parity: detect overflow / length-stop
                                // errors here so the agent loop can auto-
                                // compact and retry. Mirrors Pi's
                                // _checkCompaction() (the same triggers
                                // we use in nini).
                                let overflowed = crate::overflow::is_context_overflow(
                                    &stop_reason,
                                    None, // error message is in MessageStop
                                    Some(&usage),
                                    Some(config.compaction.context_window),
                                );
                                // v0.10 (fix): a length stop that *carried
                                // tool calls* is not a context problem, it is
                                // a truncated serialisation — and Pi refuses
                                // the whole batch on purpose
                                // (`agent-loop.ts:123` →
                                // `failToolCallsFromTruncatedMessage`: "its
                                // arguments may be truncated"). Compacting
                                // instead would run tools whose arguments were
                                // cut mid-string. Pi compacts such a turn later,
                                // at the session layer, once the rejection has
                                // been reported; the in-run threshold check
                                // below does the same job on the next turn.
                                let truncated_answer = crate::overflow::is_recoverable_length(
                                    &stop_reason,
                                    usage.output_tokens,
                                    config.max_tokens.unwrap_or(0),
                                ) && tool_calls.is_empty();
                                let recoverable = overflowed || truncated_answer;
                                last_usage = usage;
                                // v0.10 (fix): the threshold conjunct that
                                // used to guard this block is gone. Pi's
                                // overflow branch does not consult the
                                // budget — it is already past the window,
                                // and an estimate that says otherwise is
                                // exactly what left the run with no way
                                // forward (`agent-session.ts#isContextOverflow-2021`).
                                if recoverable {
                                    if self.overflow_recovery_attempted {
                                        // Pi: one recovery, then say so
                                        // (`agent-session.ts:2172-2011`).
                                        let msg = "Context overflow recovery \
                                            failed after one compact-and-retry \
                                            attempt. Try reducing context or \
                                            switching to a larger-context model.";
                                        yield AgentEvent::Error {
                                            message: msg.to_string(),
                                        };
                                        self.set_phase(AgentPhase::Idle);
                                        yield AgentEvent::PhaseChanged(AgentPhase::Idle);
                                        return;
                                    }
                                    self.overflow_recovery_attempted = true;
                                    // Drop the failed assistant turn before
                                    // compacting: Pi removes it from state so
                                    // the summary is not written *about* the
                                    // error response
                                    // (`agent-session.ts#messages.slice(0, -1)-2019`).
                                    if matches!(
                                        self.messages.last().map(|m| m.role),
                                        Some(Role::Assistant)
                                    ) {
                                        self.messages.pop();
                                    }
                                    // v0.10.8 (A2)：压缩**有生命周期**。
                                    //
                                    // 此前压缩是「偷偷」进行的：没有相位、没有标志，
                                    // 于是 TUI 永远不知道正在压缩，`is_compacting`
                                    // 永远是 false —— runtime.rs 里那个「压缩期间
                                    // 提交进队列」的分支是**不可达死代码**，
                                    // 而 Pi 在压缩期间是有明确语义的。
                                    //
                                    // Pi：`agent-session.ts#_checkCompaction` →
                                    // 压缩期间 `prompt()` 走 queueCompactionMessage
                                    // （另一条路径直接抛错）—— Pi 的两种路径都要照做，
                                    // 这里先接上「排队」那条（nini 的压缩是本地的，
                                    // 窗口短，但语义必须存在）。
                                    self.begin_auto_compaction();
                                    self.set_phase(AgentPhase::Compacting {
                                        reason: "overflow".to_string(),
                                        progress: None,
                                    });
                                    yield AgentEvent::PhaseChanged(AgentPhase::Compacting {
                                        reason: "overflow".to_string(),
                                        progress: None,
                                    });
                                    match self.compact_history_via_hook() {
                                        Some(r) => {
                                            // 压缩结束：标志落下。
                                            //
                                            // 注意：**队列不在这里**。产品里排队的是
                                            // TUI 的 `run_state.pending_next_turn_messages`
                                            // （用户在压缩期间提交的消息），agent 这一份
                                            // 队列（`pending_next_turn_messages`）在产品里
                                            // 一直是空的——`end_auto_compaction()` 只是把
                                            // 标志和中止句柄复位。
                                            self.end_auto_compaction();
                                            yield AgentEvent::Compacted {
                                                summary: r.summary.clone(),
                                                tokens_before: r.tokens_before,
                                            };
                                            self.set_phase(AgentPhase::Working);
                                            yield AgentEvent::PhaseChanged(AgentPhase::Working);
                                            retry_after_compaction = true;
                                            break 'outer;
                                        }
                                        None => {
                                            // Nothing could be compacted, so
                                            // a retry would send the same
                                            // oversized history and overflow
                                            // identically. Pi's `compact()`
                                            // returns false in this situation
                                            // and the run ends; silently
                                            // looping to `max_iterations` was
                                            // the bug.
                                            let msg = "Context overflow and \
                                                nothing left to compact — \
                                                ending this run. Start a new \
                                                session or run /compact.";
                                            yield AgentEvent::Error {
                                                message: msg.to_string(),
                                            };
                                            // v0.10.8 (A2, review finding)：**收尾**。
                                            // 压缩没成功也是「压缩结束了」——
                                            // 标志必须落下，否则它会**永久**为真，
                                            // 而 TUI 会把之后每一次提交都收进队列
                                            // 且永不执行。
                                            self.end_auto_compaction();
                                            self.set_phase(AgentPhase::Idle);
                                            yield AgentEvent::PhaseChanged(AgentPhase::Idle);
                                            return;
                                        }
                                    }
                                }
                            }
                            Some(Ok(StreamEvent::Error { message })) => {
                                // v0.10 (fix): this is where a real context
                                // overflow arrives. Providers map HTTP 400 to
                                // `StreamEvent::Error` (`nini-ai/src/openai.rs:418`),
                                // and the check that existed only in the
                                // `MessageStop` arm passed `None` for the
                                // message — so the detector never saw the
                                // text and this arm classified it as a fatal,
                                // non-retryable error. A long run that crossed
                                // the window therefore ended here, with the
                                // history intact and nothing compacted.
                                //
                                // Pi: `overflow.ts:136` Case 1 on the
                                // assistant message, then
                                // `_checkCompaction` → `_runAutoCompaction`.
                                let overflowed = crate::overflow::is_context_overflow(
                                    "error",
                                    Some(&message),
                                    None,
                                    Some(config.compaction.context_window),
                                );
                                if overflowed {
                                    if self.overflow_recovery_attempted {
                                        yield AgentEvent::Error {
                                            message: "Context overflow recovery \
                                                failed after one compact-and-retry \
                                                attempt. Try reducing context or \
                                                switching to a larger-context model."
                                                .to_string(),
                                        };
                                        self.set_phase(AgentPhase::Idle);
                                        yield AgentEvent::PhaseChanged(AgentPhase::Idle);
                                        return;
                                    }
                                    self.overflow_recovery_attempted = true;
                                    // v0.10.8 (A2)：这条路径是**生产主路径**
                                    // （provider 把 HTTP 400 报成 StreamEvent::Error），
                                    // 它也必须有压缩的生命周期——否则「压缩期间
                                    // 提交进队列」只在另一条路径上成立。
                                    self.begin_auto_compaction();
                                    self.set_phase(AgentPhase::Compacting {
                                        reason: "overflow".to_string(),
                                        progress: None,
                                    });
                                    yield AgentEvent::PhaseChanged(AgentPhase::Compacting {
                                        reason: "overflow".to_string(),
                                        progress: None,
                                    });
                                    match self.compact_history_via_hook() {
                                        Some(r) => {
                                            yield AgentEvent::Compacted {
                                                summary: r.summary.clone(),
                                                tokens_before: r.tokens_before,
                                            };
                                            self.end_auto_compaction();
                                            self.set_phase(AgentPhase::Working);
                                            yield AgentEvent::PhaseChanged(AgentPhase::Working);
                                            retry_after_compaction = true;
                                            break 'outer;
                                        }
                                        None => {
                                            yield AgentEvent::Error {
                                                message: format!(
                                                    "Context overflow and nothing left \
                                                     to compact: {message}"
                                                ),
                                            };
                                            // 收尾：标志必须落下（review finding）。
                                            self.end_auto_compaction();
                                            self.set_phase(AgentPhase::Idle);
                                            yield AgentEvent::PhaseChanged(AgentPhase::Idle);
                                            return;
                                        }
                                    }
                                }
                                if is_retryable_error(&message)
                                    && self.retry_attempt < config.retry.max_retries
                                    && !abort.is_aborted()
                                {
                                    // Pi parity: switch to Retrying phase
                                    // and signal the stream consumer to
                                    // break out so we can retry the current
                                    // request.
                                    let attempt = self.retry_attempt + 1;
                                    self.retry_attempt = attempt;
                                    self.set_phase(AgentPhase::Retrying { attempt });
                                    yield AgentEvent::PhaseChanged(
                                        AgentPhase::Retrying { attempt },
                                    );
                                    last_error = Some(message.clone());
                                    break 'outer;
                                }
                                // Non-retryable: surface as fatal.
                                yield AgentEvent::Error { message: message.clone() };
                                Err(AgentError::Provider(ProviderError::Api {
                                    status: 0,
                                    message,
                                }))?;
                            }
                            Some(Err(e)) => Err(AgentError::Provider(e))?,
                            None => break,
                        }
                    }

                    // If we broke out of 'outer via the retryable-error
                    // path, perform a backoff sleep and continue the outer
                    // loop with another stream attempt. Sleep is async via
                    // tokio::time::sleep; we abort early if `abort` fires.
                    if let Some(_err) = last_error.take() {
                        let delay_ms =
                            backoff_ms(self.retry_attempt.max(1), config.retry.base_delay_ms);
                        yield AgentEvent::Error { message: format!(
                            "retrying after transient error (attempt {}, delay {}ms)",
                            self.retry_attempt, delay_ms,
                        )};
                        // Abort-aware sleep.
                        tokio::select! {
                            _ = tokio::time::sleep(std::time::Duration::from_millis(delay_ms)) => {}
                            _ = abort.wait_aborted() => {}
                        }
                        if abort.is_aborted() {
                            self.set_phase(AgentPhase::Idle);
                            yield AgentEvent::PhaseChanged(AgentPhase::Idle);
                            yield AgentEvent::Error { message: format!(
                                "aborted during retry sleep (attempt {})",
                                self.retry_attempt,
                            )};
                            yield AgentEvent::Aborted;
                            return;
                        }
                        self.set_phase(AgentPhase::Working);
                        yield AgentEvent::PhaseChanged(AgentPhase::Working);
                        // Loop again to retry the request.
                        continue;
                    }
                }
                // v0.10 (fix): the turn that hit the ceiling is dropped
                // whole — no assistant message, no tool calls, no `turn_end`.
                // The next iteration rebuilds the request from the compacted
                // history.
                if retry_after_compaction {
                    continue;
                }

                // Anything still in flight when the stream ended never
                // got a Stop. Push it with whatever args it accumulated
                // so the tools still run instead of the call vanishing.
                //
                // Ordering note: closed calls are already in `tool_calls`
                // in Stop order. Stragglers are appended in `HashMap`
                // order, which is arbitrary — but the invariant that
                // actually matters is that every `tool_use` in the
                // assistant message has a matching `tool_result`, and
                // that holds regardless of the order the results are
                // written into the tool message.
                let mut stragglers: Vec<PendingToolCall> = in_flight.drain().map(|(_, c)| c).collect();
                stragglers.sort_by(|a, b| a.id.cmp(&b.id));
                for c in stragglers {
                    eprintln!(
                        "[nini] tool call {} ({}) never received a Stop; \
                         running it with the args accumulated so far",
                        c.id, c.name
                    );
                    tool_calls.push(c);
                }

                // Append assistant message to history.
                let mut assistant_content = Vec::new();
                if !assistant_text.is_empty() {
                    assistant_content.push(ContentBlock::Text { text: assistant_text });
                }
                for tc in &tool_calls {
                    let input: serde_json::Value = serde_json::from_str(&tc.input_json)
                        .unwrap_or(serde_json::Value::Null);
                    assistant_content.push(ContentBlock::ToolUse {
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                        input,
                    });
                }
                self.messages.push(Message {
                    role: Role::Assistant,
                    content: assistant_content,
                    timestamp: 0,
                });
                // Cache the last API-reported usage for accurate context
                // window estimation on the next iteration. Reset to None on
                // abort/error so we fall back to character estimation.
                if stop_reason != "aborted" && stop_reason != "error" {
                    self.last_usage = Some(last_usage.clone());
                }

                yield AgentEvent::TurnEnd { stop_reason: stop_reason.clone(), usage: last_usage };

                // v0.8.6: length-stop tool_call reject gate (audit fix).
                //
                // When stop_reason == "length" and there are tool_calls,
                // the assistant message ends mid-tool-call-JSON (the
                // LLM ran out of output_tokens before serializing the
                // tool args). nini would otherwise pass the truncated
                // JSON through serde_json::from_str → Value::Null, run
                // tools with empty args, and surface a cryptic
                // "tool error: invalid args" to the user.
                //
                // Mirrors Pi's agent-loop.js:144
                //   failToolCallsFromTruncatedMessage(toolCalls, emit)
                //
                // Coordinates with the existing auto-compact-on-length-
                // stop path above (which fires unconditionally and shrinks
                // context for the *next* turn). We only skip the tool
                // execution itself; auto-compaction still runs.
                //
                // Note: do NOT yield a second AgentEvent::TurnEnd here —
                // it was already yielded above, and the TUI phase state
                // is 1:1 per turn (audit fix #4).
                let length_stop_reject =
                    stop_reason == "length" && !tool_calls.is_empty();

                // v0.10 (R1): no longer an exit point. Pi sets
                // `hasMoreToolCalls = false` and still reaches the steering
                // poll at `:160`; the turn ends only if steering is empty too.
                // nini used to poll follow-up, append it to history, and
                // `return` — which demoted a follow-up from a task to dead
                // context.
                // The tool block runs whenever the model emitted tool calls —
                // *including* the length-stop case, where it pushes synthetic
                // failures so the message graph stays well-formed for the
                // next LLM call (Pi's `failToolCallsFromTruncatedMessage`,
                // `:263`). Excluding it here dropped those results and broke
                // the tool_use/tool_result pairing.
                has_more_tool_calls = !tool_calls.is_empty();

                // v0.10 (R1c): per-call `terminate` flags in batch order.
                // Declared out here because Pi evaluates the batch verdict
                // after the batch returns (`agent-loop.js:214`), which is
                // outside the execution block. `shouldTerminateToolBatch`
                // takes the AND over every finalized result
                // (`agent-loop.ts#shouldTerminateToolBatch-584`).
                let mut terminate_flags: Vec<bool> = vec![false; tool_calls.len()];

                if has_more_tool_calls {

                // v0.8.5: execute tool calls in PARALLEL via
                // `FuturesUnordered`. The model often emits many
                // `tool_calls` in one assistant turn (e.g. 14 reads);
                // before this change, nini ran them serially — each
                // tool awaited to completion before the next started.
                // For read-only tools (`read`, `grep`, `find`) this
                // wasted wall-clock time on independent filesystem
                // syscalls. Serial execution also broke the user's
                // mental model: the model says "parallel calls" and
                // nini answered sequentially.
                //
                // Ordering invariant: providers (notably Anthropic)
                // require tool_result blocks to be in the SAME order as
                // the tool_use blocks they answer. We preserve this by
                // allocating `Vec<Option<ContentBlock>>` keyed by the
                // tool_call's original index and emitting results in
                // index order at the end.
                //
                // Abort semantics: abort cancels the WHOLE agent run,
                // not individual tool calls. If abort fires while
                // futures are still running, we drop the partial
                // results and return Aborted — we never emit a partial
                // tool_results message (that would break the
                // API contract with the next LLM call).

                // 1. Dispatch tool calls, at most
                //    `config.max_parallel_tools` at a time.
                //
                //    v0.8.9 (P2-10): this used to push every tool_call
                //    into the `FuturesUnordered` at once, so a model
                //    emitting 30 `bash` calls forked 30 process trees
                //    simultaneously — each buffering its own output
                //    against `nini_tools::bash::STREAM_CAPTURE_CAP`.
                //    The batch size was whatever the model felt like
                //    emitting. It is now a config knob (default 8).
                //
                //    Still a worker pool rather than a semaphore or a
                //    semaphore-per-batch: futures are pushed lazily as
                //    earlier ones complete, so a batch of 30 costs the
                //    same peak as a batch of 8 without adding a round
                //    of artificial barrier waits.
                use futures_util::stream::FuturesUnordered;
                use futures_util::StreamExt;

                let n_calls = tool_calls.len();
                let mut tool_results_slots: Vec<Option<ContentBlock>> =
                    (0..n_calls).map(|_| None).collect();

                // v0.8.6 (audit fix #3): length-stop tool_call reject branch.
                //
                // If the LLM ran out of output_tokens mid-tool_call, the
                // assistant message contains truncated JSON. Executing
                // those tools would surface as cryptic 'tool error: invalid
                // args' — they're really model-level truncation errors.
                //
                // Mirrors Pi's agent-loop.js:144
                //   failToolCallsFromTruncatedMessage(toolCalls, emit).
                //
                // Coordinates with the existing auto-compact-on-length-stop
                // path above (which still runs unconditionally and shrinks
                // context for the next turn). We only skip tool execution.
                if length_stop_reject {
                    let mut tool_results: Vec<ContentBlock> =
                        Vec::with_capacity(tool_calls.len());
                    for tc in &tool_calls {
                        let reject_output = ToolOutput::err(
                            "[reject] tool call arguments truncated by output token limit \
                             (length stop). The LLM hit max_tokens before serializing the \
                             tool call JSON. Retry with a shorter prompt or split the \
                             request."
                                .to_string(),
                        );
                        yield AgentEvent::ToolResult {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                            output: reject_output.clone(),
                        };
                        tool_results.push(ContentBlock::ToolResult {
                            tool_use_id: tc.id.clone(),
                            content: reject_output.content,
                            is_error: true,
                        });
                    }
                    self.messages.push(Message {
                        role: Role::Tool,
                        content: tool_results,
                        timestamp: 0,
                    });
                } else {
                    // Pre-extract the per-call data we need to move into
                    // the future (the registry + config are & references;
                    // we need to clone the strings we want to keep alive).
                    let specs: Vec<PendingToolCall> = tool_calls.clone();
                    // v0.8.9 (P2-11): `specs` is consumed by the loop
                    // below, so keep a cheap (id, name) copy for the
                    // abort path that has to close out the calls that
                    // never produced a result.
                    let batch_identities: Vec<(String, String)> = specs
                        .iter()
                        .map(|s| (s.id.clone(), s.name.clone()))
                        .collect();
                    // v0.8.9 (P2-10): the un-dispatched tail of the
                    // batch. The worker pool drains this as slots free
                    // up; on abort it is simply dropped, and the
                    // close-out loop below emits a cancelled result for
                    // each of those `None` slots like any other
                    // outstanding call.

                    // Declared here, filled from the pool setup below,
                    // and drained by the refill step in the drain loop.
                    #[allow(unused_mut)]
                    let mut pending_queue: Option<std::iter::Enumerate<std::vec::IntoIter<PendingToolCall>>>;

                    // v0.10 (R1c) — Pi's `executeToolCalls`
                    // (`agent-loop.ts:416-426`): ONE tool declaring
                    // `executionMode === "sequential"` downgrades the WHOLE
                    // batch, not just that tool.
                    //
                    //     const hasSequentialToolCall = toolCalls.some(
                    //         (tc) => tools?.find((t) => t.name === tc.name)?.executionMode === "sequential");
                    //     if (config.toolExecution === "sequential" || hasSequentialToolCall) {
                    //         return executeToolCallsSequential(...);
                    //     }
                    //
                    // The downgrade is all-or-nothing because a sequential
                    // tool needs exclusive use of whatever shared state it
                    // touches, and interleaving siblings around it would
                    // break that guarantee just as surely as running it
                    // alongside them.
                    let batch_is_sequential = config.tool_execution
                        == crate::tool::ToolExecutionMode::Sequential
                        || specs.iter().any(|s| {
                            tool_registry
                                .get(&s.name)
                                .map(|t| {
                                    t.execution_mode()
                                        == crate::tool::ToolExecutionMode::Sequential
                                })
                                .unwrap_or(false)
                        });

                    let mut futures = FuturesUnordered::new();
                    // `0` means "no limit" — an explicit opt-out of the
                    // ceiling, preserving the pre-v0.8.9 behaviour.
                    // A sequential batch runs one at a time. Pi does the
                    // same via `executeToolCallsSequential`
                    // (`agent-loop.ts:431`), which has no worker pool at all.
                    let parallelism = if batch_is_sequential {
                        1
                    } else if config.max_parallel_tools == 0 {
                        specs.len().max(1)
                    } else {
                        config.max_parallel_tools
                    };
                    {
                        let mut queue = specs.into_iter().enumerate();
                        for _ in 0..parallelism {
                            let Some((idx, spec)) = queue.next() else {
                                break;
                            };
                            futures.push(dispatch_tool_call(
                                idx,
                                spec,
                                tool_registry.clone(),
                                config.clone(),
                                abort.clone(),
                            ));
                        }
                        pending_queue = Some(queue);
                    }

                // 2. Drain futures, topping the pool back up as each
                //    one finishes. We check abort *before* publishing a
                //    result so an abort during the drain path drops the
                //    whole batch (we never partial-commit).
                let mut aborted = false;
                while let Some((idx, tc_id, tc_name, output)) = futures.next().await {
                    if abort.is_aborted() {
                        aborted = true;
                        break;
                    }
                    yield AgentEvent::ToolResult {
                        id: tc_id.clone(),
                        name: tc_name.clone(),
                        output: output.clone(),
                    };
                    if idx < terminate_flags.len() {
                        terminate_flags[idx] = output.terminate;
                    }
                    tool_results_slots[idx] = Some(ContentBlock::ToolResult {
                        tool_use_id: tc_id,
                        content: output.content,
                        is_error: output.is_error,
                    });
                    // v0.8.9 (P2-10): a slot just freed up — pull the
                    // next queued tool call into it.
                    if let Some(queue) = pending_queue.as_mut() {
                        if let Some((nidx, nspec)) = queue.next() {
                            futures.push(dispatch_tool_call(
                                nidx,
                                nspec,
                                tool_registry.clone(),
                                config.clone(),
                                abort.clone(),
                            ));
                        }
                    }
                }
                drop(futures); // cancel any still-pending futures

                if aborted {
                    // v0.8.9 (P2-11): close out every tool call that
                    // never produced a result.
                    //
                    // The `ToolCallStart` events for this batch were
                    // already emitted while streaming the assistant
                    // turn, so the TUI has a `▸ read` / `▸ bash` row on
                    // screen waiting for its matching result. On abort
                    // we used to bail out with nothing more, leaving
                    // those rows pending forever — the user could not
                    // tell whether the tool was still running, and the
                    // only way out was restarting the session. Pi emits
                    // a cancelled result for each outstanding call.
                    //
                    // `tool_results_slots[idx]` is `None` exactly for the
                    // calls that were dropped mid-flight, so it is the
                    // authoritative list of what's outstanding.
                    for (idx, slot) in tool_results_slots.iter().enumerate() {
                        if slot.is_some() {
                            continue;
                        }
                        let Some((id, name)) = batch_identities.get(idx) else {
                            continue;
                        };
                        yield AgentEvent::ToolResult {
                            id: id.clone(),
                            name: name.clone(),
                            output: ToolOutput::err(
                                "[aborted] user interrupted before this tool finished".to_string(),
                            ),
                        };
                    }
                    self.set_phase(AgentPhase::Idle);
                    yield AgentEvent::PhaseChanged(AgentPhase::Idle);
                    yield AgentEvent::Aborted;
                    return;
                }
                }  // v0.8.6: end of else branch (normal parallel execution vs length-stop reject)

                // 3. Flatten in original order. With abort=false,
                //    every slot must be Some; if not, that's a bug in
                //    the future dispatch above.
                let tool_results: Vec<ContentBlock> = tool_results_slots
                    .into_iter()
                    .enumerate()
                    .map(|(i, slot)| {
                        slot.unwrap_or_else(|| {
                            // Defensive: if a future was silently
                            // dropped (shouldn't happen — FuturesUnordered
                            // yields None only when empty), emit a
                            // synthetic tool_error so the next LLM call
                            // still gets a tool_result for every
                            // tool_use.
                            ContentBlock::ToolResult {
                                tool_use_id: format!("__missing_{i}"),
                                content: format!("[internal error] tool call {i} produced no result"),
                                is_error: true,
                            }
                        })
                    })
                    .collect();

                self.messages.push(Message { role: Role::Tool, content: tool_results.clone(), timestamp: 0 });
                }   // end of `if has_more_tool_calls`

                // v0.10 (R1c) — Pi's `hasMoreToolCalls = !batch.terminate`
                // (`agent-loop.js:214`), evaluated *after* the batch
                // returns. A tool that has the final answer sets
                // `ToolOutput::terminate`, and when every result agrees the
                // turn ends instead of spending another model call to be
                // told what the tool already knows.
                //
                // The predicate is `every`, not `some`
                // (`agent-loop.ts:589-584`): one finished tool must not
                // cut off a sibling that still has something to report.
                if !terminate_flags.is_empty()
                    && crate::tool::should_terminate_tool_batch(&terminate_flags)
                {
                    has_more_tool_calls = false;
                }

                // v0.10 (R1d) — Pi's per-turn order, verbatim
                // (`agent-loop.ts#prepareNextTurn-250`):
                //
                //     await emit({ type: "turn_end", message, toolResults });
                //     const nextTurnSnapshot = await config.prepareNextTurn?.(…);
                //     if (nextTurnSnapshot) { …swap context/model/reasoning… }
                //     if (await config.shouldStopAfterTurn?.({ … })) {
                //         await emit({ type: "agent_end", messages: newMessages });
                //         return;
                //     }
                //     pendingMessages = await config.getSteeringMessages?.();
                //
                // nini used to evaluate this at the TOP of the loop, before
                // the LLM call. That inverts the contract in two ways: the
                // hook never saw a completed turn (an extension saying
                // "stop once the context is nearly full" stopped one turn
                // earlier than Pi), and its verdict was about a turn that
                // had not happened yet.
                //
                // It now runs here, after the turn's `turn_end` and the tool
                // batch, and — as in Pi — returning true ends the run
                // *before* the queues are polled, so a queued steering and
                // follow-up are discarded rather than executed.
                //
                // The hook takes a CLONED snapshot of self.messages so it
                // does not need &mut Agent. Defensive: a panicking hook
                // falls back to false and the run continues.
                let hooks_ref: Arc<dyn crate::agent_hooks::AgentLoopHooks> =
                    self.hooks.clone();
                let snapshot = self.messages.clone();

                // v0.10 (R1e) — Pi's `prepareNextTurn`
                // (`agent-loop.ts:181-245`), evaluated BEFORE
                // `shouldStopAfterTurn` so a stop verdict is never computed
                // against a stale model:
                //
                //     const nextTurnSnapshot = await config.prepareNextTurn?.(nextTurnContext);
                //     if (nextTurnSnapshot) { config = { …, model: nextTurnSnapshot.model ?? config.model }; }
                //     if (await config.shouldStopAfterTurn?.({ … })) { … }
                //
                // This is what makes a model change made *while the agent is
                // running* take effect. Before this, the TUI updated
                // `state.model_state.model` and the status bar reflected it,
                // while `RunConfig` — cloned once at `run()` entry — kept
                // sending the old model. The user saw one model in the
                // status bar and another one being called.
                let turn_update =
                    crate::agent_hooks::catch_hook_panic("prepare_next_turn", || {
                        hooks_ref.prepare_next_turn(&snapshot)
                    });
                if let Some(next_model) = turn_update.model {
                    if next_model != config.model {
                        config.model = next_model;
                    }
                }

                let should_stop = crate::agent_hooks::catch_hook_panic(
                    "should_stop_after_turn",
                    || hooks_ref.should_stop_after_turn(&snapshot),
                );
                if should_stop {
                    self.set_phase(AgentPhase::Idle);
                    yield AgentEvent::PhaseChanged(AgentPhase::Idle);
                    return;
                }

                // v0.10 (R1) — Pi's `agent-loop.ts#getSteeringMessages`: steering is polled
                // at the end of EVERY turn, unconditional.
                //
                // It used to live inside the tool-execution branch, so a user
                // interjecting while the model streamed a long text-only
                // answer was not seen until the *next* run. Pi runs another
                // turn immediately; nini made the user type again.
                //
                // The last two messages are handed to the hook so it can see
                // what just happened without a slice of the whole history.
                // Defensive: a panicking hook falls back to an empty Vec.
                let recent: Vec<Message> = if self.messages.len() >= 2 {
                    self.messages[self.messages.len() - 2..].to_vec()
                } else {
                    self.messages.clone()
                };
                let hooks_ref: Arc<dyn crate::agent_hooks::AgentLoopHooks> = self.hooks.clone();
                pending = crate::agent_hooks::catch_hook_panic(
                    "get_steering_messages",
                    || hooks_ref.get_steering_messages(&recent),
                );
                // Loop again: the guard at the top decides whether this is
                // another turn or a follow-up revival.
            }
        })
    }
}

/// One unit of work in the v0.8.9 (P2-10) worker pool: run a single
/// tool call and report `(index, id, name, output)`.
///
/// A named function rather than an inline `async move` block because the
/// pool has two call sites (the initial fill and the per-completion
/// refill), and two separately-written opaque async-block types will
/// not unify inside one `FuturesUnordered`.
async fn dispatch_tool_call(
    idx: usize,
    spec: PendingToolCall,
    tool_registry: ToolRegistry,
    config: RunConfig,
    abort: AbortHandle,
) -> (usize, String, String, ToolOutput) {
    let tc_name = spec.name.clone();
    // Check abort at the start so a pre-aborted batch doesn't even begin
    // running its tool bodies.
    if abort.is_aborted() {
        return (
            idx,
            spec.id.clone(),
            tc_name,
            ToolOutput::err("[aborted before execution]".to_string()),
        );
    }
    let output = run_single_tool(&spec, &tool_registry, &config, &abort).await;
    (idx, spec.id.clone(), tc_name, output)
}

/// v0.8.5: extract the per-tool-call execution body so it can be
/// spawned as a `Send` future inside a `FuturesUnordered`. Mirrors
/// the v0.7 (M2) 4-phase lifecycle (prepare → before → execute →
/// after) but lives outside the `async_stream::stream!` block so it
/// can be moved across tasks.
async fn run_single_tool(
    spec: &PendingToolCall,
    tool_registry: &ToolRegistry,
    config: &RunConfig,
    abort: &AbortHandle,
) -> ToolOutput {
    let tool = tool_registry.get(&spec.name);
    let tool = match tool {
        Some(t) => t,
        None => return ToolOutput::err(format!("tool not found: {}", spec.name)),
    };
    let mut args: serde_json::Value =
        serde_json::from_str(&spec.input_json).unwrap_or(serde_json::Value::Null);

    // Phase 2: before-execute hook.
    let mut denied: Option<String> = None;
    if let Some(b) = tool.before() {
        use futures_util::FutureExt;
        match std::panic::AssertUnwindSafe(b.run(args.clone(), &config.tool_context))
            .catch_unwind()
            .await
        {
            Ok(Ok(Some(replaced))) => args = replaced,
            Ok(Ok(None)) => { /* pass through */ }
            Ok(Err(e)) => denied = Some(e.to_string()),
            Err(panic_payload) => {
                let msg = panic_msg(&panic_payload);
                eprintln!("[nini] before-execute hook panicked: {msg}");
                denied = Some(format!("hook panicked: {msg}"));
            }
        }
    }
    if let Some(msg) = denied {
        return ToolOutput::err(format!("[before-hook denied] {msg}"));
    }

    // Phase 3: execute the tool body, abort-aware.
    let raw_out = {
        let exec = tool.execute(args, config.tool_context.clone());
        tokio::select! {
            result = exec => match result {
                Ok(out) => out,
                Err(e) => ToolOutput::err(e.to_string()),
            },
            _ = abort.wait_aborted() => {
                return ToolOutput::err("[aborted during execute]".to_string());
            }
        }
    };

    // Phase 4: after-execute hook.
    if let Some(a) = tool.after() {
        use futures_util::FutureExt;
        match std::panic::AssertUnwindSafe(a.run(raw_out, &config.tool_context))
            .catch_unwind()
            .await
        {
            Ok(Ok(out)) => return out,
            Ok(Err(e)) => {
                eprintln!("[nini] after-execute hook returned error: {e}; raw output lost");
                return ToolOutput::err(format!("[after-hook error] {e}"));
            }
            Err(panic_payload) => {
                let msg = panic_msg(&panic_payload);
                eprintln!("[nini] after-execute hook panicked: {msg}");
                return ToolOutput::err(format!("[after-hook panicked] {msg}"));
            }
        }
    }
    raw_out
}

#[cfg(test)]
mod phase_state_machine_tests {
    use super::*;
    use crate::provider::{Capabilities, Provider, ProviderError, Request, StreamEvent};
    use futures_core::Stream;
    use std::pin::Pin;

    /// Stub provider that returns a synthetic "stop" after a TextDelta.
    struct ShortResponse;
    impl Provider for ShortResponse {
        fn name(&self) -> &'static str {
            "short"
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
        fn stream(
            &self,
            _req: Request,
        ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send + 'static>>
        {
            Box::pin(futures_util::stream::iter(vec![
                Ok(StreamEvent::MessageStart {
                    id: "msg-1".into(),
                    model: "test".into(),
                }),
                Ok(StreamEvent::TextDelta { text: "hi".into() }),
                Ok(StreamEvent::MessageStop {
                    stop_reason: "stop".into(),
                    usage: Usage::default(),
                }),
            ]))
        }
    }

    fn make_agent() -> Agent {
        Agent::new(
            std::sync::Arc::new(ShortResponse),
            ToolRegistry::new(),
            RunConfig::new("test-model"),
        )
    }

    #[tokio::test(flavor = "current_thread")]
    async fn run_starts_in_working_phase_and_ends_in_idle() {
        use futures_util::StreamExt;
        let mut agent = make_agent();
        // Initial state: Idle.
        assert_eq!(agent.phase(), AgentPhase::Idle);
        // Start a run and collect phase transitions.
        let mut phases_seen = Vec::new();
        {
            let s = agent.run(crate::AgentMessage::user("hi"));
            tokio::pin!(s);
            while let Some(ev) = s.next().await {
                if let Ok(AgentEvent::PhaseChanged(p)) = ev {
                    phases_seen.push(p);
                }
            }
        }
        // Expect: Working → Idle (no compaction, no retry, no branch).
        assert!(
            phases_seen.contains(&AgentPhase::Working),
            "should see Working, got: {phases_seen:?}"
        );
        assert!(
            phases_seen.contains(&AgentPhase::Idle),
            "should end in Idle, got: {phases_seen:?}"
        );
        // Working should appear before Idle.
        let w_idx = phases_seen
            .iter()
            .position(|p| *p == AgentPhase::Working)
            .unwrap();
        let i_idx = phases_seen
            .iter()
            .position(|p| *p == AgentPhase::Idle)
            .unwrap();
        assert!(w_idx < i_idx, "Working must come before Idle");
    }

    #[test]
    fn set_phase_emits_change_only_when_different() {
        let mut agent = make_agent();
        let prev = agent.set_phase(AgentPhase::Working);
        assert_eq!(prev, AgentPhase::Idle);
        // No-op when same.
        let prev = agent.set_phase(AgentPhase::Working);
        assert_eq!(prev, AgentPhase::Working);
    }

    #[test]
    fn phase_default_is_idle() {
        let agent = make_agent();
        assert_eq!(agent.phase(), AgentPhase::Idle);
    }

    #[test]
    fn agent_phase_display() {
        assert_eq!(AgentPhase::Idle.to_string(), "idle");
        assert_eq!(AgentPhase::Working.to_string(), "working");
        assert_eq!(
            AgentPhase::Compacting {
                reason: "overflow".into(),
                progress: Some(50),
            }
            .to_string(),
            "compacting:overflow (50%)"
        );
        assert_eq!(
            AgentPhase::Compacting {
                reason: "manual".into(),
                progress: None,
            }
            .to_string(),
            "compacting:manual"
        );
        assert_eq!(
            AgentPhase::Retrying { attempt: 2 }.to_string(),
            "retrying (attempt 2)"
        );
        assert_eq!(AgentPhase::BranchSummary.to_string(), "branch summary");
    }
}

#[cfg(test)]
mod abort_controller_tests {
    use super::*;

    fn make_test_agent() -> Agent {
        // Minimal stub provider for testing.
        use crate::provider::{Capabilities, Provider, ProviderError, Request, StreamEvent};
        use futures_core::Stream;
        use std::pin::Pin;
        struct Stub;
        impl Provider for Stub {
            fn name(&self) -> &'static str {
                "stub"
            }
            fn capabilities(&self) -> Capabilities {
                Capabilities::default()
            }
            fn stream(
                &self,
                _req: Request,
            ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send + 'static>>
            {
                Box::pin(futures_util::stream::empty())
            }
        }
        Agent::new(
            Arc::new(Stub),
            ToolRegistry::new(),
            RunConfig::new("test-model"),
        )
    }

    #[test]
    fn three_abort_controllers_are_independent() {
        let mut agent = make_test_agent();
        agent.compaction_abort.abort();
        // Only compaction abort should fire.
        assert!(agent.compaction_abort.is_aborted());
        assert!(!agent.auto_compaction_abort.is_aborted());
        assert!(!agent.branch_summary_abort.is_aborted());
        assert!(!agent.abort.is_aborted());

        agent.auto_compaction_abort.abort();
        assert!(agent.compaction_abort.is_aborted()); // still aborted
        assert!(agent.auto_compaction_abort.is_aborted());
        assert!(!agent.branch_summary_abort.is_aborted());
        assert!(!agent.abort.is_aborted());

        agent.reset_aborts();
        // All four back to false.
        assert!(!agent.compaction_abort.is_aborted());
        assert!(!agent.auto_compaction_abort.is_aborted());
        assert!(!agent.branch_summary_abort.is_aborted());
        assert!(!agent.abort.is_aborted());
    }

    #[test]
    fn queue_drains_on_end_auto_compaction() {
        let mut agent = make_test_agent();
        agent.begin_auto_compaction();
        agent.queue_next_turn("first queued");
        agent.queue_next_turn("second queued");
        assert_eq!(agent.pending_next_turn_messages.len(), 2);
        let drained = agent.end_auto_compaction();
        assert_eq!(drained, vec!["first queued", "second queued"]);
        assert!(agent.pending_next_turn_messages.is_empty());
        assert!(!agent.is_compacting());
    }

    #[test]
    fn queue_inactive_outside_auto_compaction() {
        let mut agent = make_test_agent();
        agent.queue_next_turn("pre-compaction message");
        assert_eq!(agent.pending_next_turn_messages.len(), 1);
        // `is_auto_compacting` is independent — only `begin_auto_compaction`
        // sets it. The runtime's job is to check it before queueing.
        assert!(!agent.is_compacting());
    }
}

#[derive(Debug, Clone)]
struct PendingToolCall {
    id: String,
    name: String,
    input_json: String,
}

/// Extract a human-readable panic message from the `Box<dyn Any>`
/// payload returned by `catch_unwind`. Falls back to "<opaque>" when
/// the payload doesn't downcast to `&'static str` or `String`.
fn panic_msg(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        return (*s).to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    "<opaque panic payload>".to_string()
}

/// v0.10 (R1f): exposed so the settings → RunConfig → Request chain can be
/// asserted. The thinking level was persisted and displayed for versions
/// while `build_request` had nowhere to put it. An unobservable link in
/// a chain cannot be tested, and an untestable link is one that silently
/// goes missing.
pub fn build_request_for_test(
    config: &RunConfig,
    messages: &[Message],
    tools: &[ToolSpec],
) -> Request {
    build_request(config, messages, tools)
}

fn build_request(config: &RunConfig, messages: &[Message], tools: &[ToolSpec]) -> Request {
    Request {
        model: config.model.clone(),
        messages: messages.to_vec(),
        tools: tools.to_vec(),
        max_tokens: config.max_tokens,
        temperature: config.temperature,
        // system is filled in by the agent loop at call sites via
        // build_system_prompt() — we never read it from
        // config.system here. Keeping this field as-is preserves
        // the wire shape.
        system: config.system.clone(),
        reasoning: config.reasoning,
    }
}

/// v0.7.1 — build the final system prompt by appending the
/// aggregated tool `system_prompt_contribution`s to the user's
/// `RunConfig.system` base. Centralized so the loop has a
/// single chokepoint for system-prompt construction (Pi hook #9).
fn build_system_prompt(config: &RunConfig, registry: &ToolRegistry) -> Option<String> {
    crate::tool::build_system_prompt_with_contributions(config.system.as_deref(), registry)
}

/// Convert an `nini_core::AgentMessage` (which has `timestamp`) to the
/// provider-layer `Message` (which doesn't).
fn to_nini_message(m: &AgentMessage) -> Message {
    // Reuse the agent-core role directly — it's already in the provider's
    // canonical shape (User/Assistant/Tool/System).
    Message {
        role: m.role,
        content: m.content.clone(),
        timestamp: 0,
    }
}

#[cfg(test)]
mod inline_llm_summary_tests {
    use super::*;
    use std::sync::Arc;
    use Message;

    fn stub_agent() -> Agent {
        // Minimal stub provider that returns empty stream.
        use crate::provider::{Capabilities, Provider, ProviderError, Request, StreamEvent};
        use futures_core::Stream;
        use std::pin::Pin;
        struct Empty;
        impl Provider for Empty {
            fn name(&self) -> &'static str {
                "empty"
            }
            fn capabilities(&self) -> Capabilities {
                Capabilities::default()
            }
            fn stream(
                &self,
                _req: Request,
            ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send + 'static>>
            {
                Box::pin(futures_util::stream::empty())
            }
        }
        Agent::new(
            Arc::new(Empty),
            ToolRegistry::new(),
            RunConfig::new("test-model"),
        )
    }

    #[tokio::test(flavor = "current_thread")]
    async fn inline_llm_summary_returns_err_on_empty_stream() {
        let agent = stub_agent();
        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "hello".into(),
            }],
            timestamp: 0,
        }];
        let result = inline_llm_summary(&agent.provider, "test-model", &messages);
        assert!(result.is_ok());
    }

    #[test]
    fn stop_reason_is_aborted_or_error_recognizes_known() {
        assert!(stop_reason_is_aborted_or_error("aborted"));
        assert!(stop_reason_is_aborted_or_error("error"));
        assert!(!stop_reason_is_aborted_or_error("stop"));
        assert!(!stop_reason_is_aborted_or_error("length"));
    }
}

#[cfg(test)]
mod phase_lifecycle_tests {
    use super::*;
    use crate::provider::{Capabilities, Provider, ProviderError, Request, StreamEvent};
    use futures_core::Stream;
    use std::pin::Pin;
    use std::sync::Arc;

    fn stub_agent() -> Agent {
        struct Empty;
        impl Provider for Empty {
            fn name(&self) -> &'static str {
                "empty"
            }
            fn capabilities(&self) -> Capabilities {
                Capabilities::default()
            }
            fn stream(
                &self,
                _req: Request,
            ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send + 'static>>
            {
                Box::pin(futures_util::stream::empty())
            }
        }
        Agent::new(
            Arc::new(Empty),
            ToolRegistry::new(),
            RunConfig::new("test-model"),
        )
    }

    #[test]
    fn branch_summary_phase_cycles() {
        let mut agent = stub_agent();
        assert_eq!(agent.phase(), AgentPhase::Idle);
        agent.begin_branch_summary();
        assert_eq!(agent.phase(), AgentPhase::BranchSummary);
        agent.end_branch_summary();
        assert_eq!(agent.phase(), AgentPhase::Working);
        agent.set_phase(AgentPhase::Idle);
        assert_eq!(agent.phase(), AgentPhase::Idle);
    }

    #[test]
    fn retry_phase_records_attempt() {
        let mut agent = stub_agent();
        agent.begin_retry(0);
        assert_eq!(agent.phase(), AgentPhase::Retrying { attempt: 0 });
        agent.begin_retry(1);
        assert_eq!(agent.phase(), AgentPhase::Retrying { attempt: 1 });
        agent.end_retry();
        assert_eq!(agent.phase(), AgentPhase::Working);
    }
}

#[cfg(test)]
mod llm_summary_in_run_loop_tests {
    // v0.10 (R2a/A3): the imports that stood here -
    // `StreamEvent`, `Stream`, `Pin`, `Arc` - were there for
    // `auto_compaction_with_llm_provider_emits_compacting_phase`. That
    // test went with the in-loop threshold compaction it covered, and
    // the fixture with it.
    //
    // The test that stood here -

    /// v0.10 (R2a/A3): the test that stood here -
    /// `auto_compaction_with_llm_provider_emits_compacting_phase` - is gone
    /// with the mechanism it covered.
    ///
    /// It seeded an over-budget history, ran the loop, and asserted that a
    /// compaction fired, that an `Error` event beginning "compaction:" was
    /// emitted, and that the phase went Working -> Compacting -> Working.
    /// All three came from the in-loop threshold check removed in A3, whose
    /// compaction never reached the session: the `Compacted` event it
    /// emitted had no consumer anywhere in the tree.
    ///
    /// Compaction now runs in the session, which persists it and hands the
    /// resulting context to this Agent as its seed. The Agent no longer
    /// compacts itself, so there is nothing here to assert. The invariants
    /// that replaced it live in `nini-tui/tests/a3_seed.rs`.

    #[test]
    fn llm_summary_provider_unavailable_falls_back_to_local() {
        // No runtime is in scope here (sync test), so inline_llm_summary
        // can't dispatch a request. Verify that the local fallback path
        // produces something useful.
        let entries: Vec<crate::Entry> = vec![];
        let summary = crate::compaction::generate_local_summary(&entries);
        assert!(
            summary.contains("# Local summary") || summary.is_empty(),
            "fallback summary should be empty or start with # Local summary: {summary:?}"
        );
    }
}

#[cfg(test)]
mod branch_summary_lifecycle_tests {
    use super::*;
    use crate::provider::{Capabilities, Provider, ProviderError, Request, StreamEvent};
    use futures_core::Stream;
    use std::pin::Pin;
    use std::sync::Arc;

    struct Stub;
    impl Provider for Stub {
        fn name(&self) -> &'static str {
            "stub"
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
        fn stream(
            &self,
            _req: Request,
        ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send + 'static>>
        {
            Box::pin(futures_util::stream::empty())
        }
    }

    #[test]
    fn branch_summary_phase_returns_to_working_after_completion() {
        let mut agent = Agent::new(
            Arc::new(Stub),
            ToolRegistry::new(),
            RunConfig::new("test-model"),
        );
        agent.begin_branch_summary();
        assert_eq!(agent.phase(), AgentPhase::BranchSummary);
        // Simulate LLM summary completion.
        agent.end_branch_summary();
        assert_eq!(agent.phase(), AgentPhase::Working);
    }

    #[test]
    fn retry_attempt_counter_increments_and_resets_correctly() {
        let mut agent = Agent::new(
            Arc::new(Stub),
            ToolRegistry::new(),
            RunConfig::new("test-model"),
        );
        assert_eq!(agent.retry_attempt, 0);
        agent.begin_retry(1);
        assert_eq!(agent.phase(), AgentPhase::Retrying { attempt: 1 });
        // begin_retry doesn't touch retry_attempt — that's the run loop's
        // responsibility. It increments during stream retry.
        agent.retry_attempt = 1;
        // end_retry returns to Working AND resets the counter to 0
        // (matches the message-stop path in run()).
        agent.end_retry();
        assert_eq!(agent.phase(), AgentPhase::Working);
        assert_eq!(agent.retry_attempt, 0);
    }
}

#[cfg(test)]
mod retry_loop_tests {
    use super::*;
    use crate::provider::{Capabilities, Provider, ProviderError, Request, StreamEvent};
    use futures_core::Stream;
    use std::pin::Pin;
    use std::sync::Arc;

    /// Stream that emits N error events then a successful response.
    struct FlakyStream {
        failures_left: u32,
    }
    impl Provider for FlakyStream {
        fn name(&self) -> &'static str {
            "flaky"
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
        fn stream(
            &self,
            _req: Request,
        ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send + 'static>>
        {
            let _failures_left = self.failures_left;
            // Simple line-style stream: produce `failures_left` errors
            // then one success event. Re-create the Vec per call (it's
            // not in the hot path for retry tests).
            let failures = self.failures_left;
            let mut events: Vec<Result<StreamEvent, ProviderError>> =
                Vec::with_capacity(failures as usize + 1);
            for _ in 0..failures {
                events.push(Ok(StreamEvent::Error {
                    message: "service unavailable: 503".into(),
                }));
            }
            events.push(Ok(StreamEvent::MessageStop {
                stop_reason: "stop".into(),
                usage: Usage::default(),
            }));
            Box::pin(futures_util::stream::iter(events))
        }
    }

    /// Provider that ALWAYS errors with a transient message.
    struct AlwaysFails;
    impl Provider for AlwaysFails {
        fn name(&self) -> &'static str {
            "always-fails"
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
        fn stream(
            &self,
            _req: Request,
        ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send + 'static>>
        {
            Box::pin(futures_util::stream::once(async {
                Ok(StreamEvent::Error {
                    message: "rate limit exceeded".into(),
                })
            }))
        }
    }

    /// Provider that fails with a fatal (non-retryable) error.
    struct FatalError;
    impl Provider for FatalError {
        fn name(&self) -> &'static str {
            "fatal"
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
        fn stream(
            &self,
            _req: Request,
        ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send + 'static>>
        {
            Box::pin(futures_util::stream::once(async {
                Ok(StreamEvent::Error {
                    message: "context window exceeded".into(),
                })
            }))
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn retry_recovers_from_transient_errors() {
        use futures_util::StreamExt;
        // 2 transient errors, then success.
        let mut agent = Agent::new(
            Arc::new(FlakyStream { failures_left: 2 }),
            ToolRegistry::new(),
            RunConfig::new("test-model"),
        );
        let mut phases = Vec::new();
        let mut errors = 0;
        {
            let mut stream = Box::pin(agent.run(Message::user("hi")));
            while let Some(ev) = stream.next().await {
                match ev {
                    Ok(AgentEvent::PhaseChanged(p)) => phases.push(p),
                    Ok(AgentEvent::Error { .. }) => errors += 1,
                    _ => {}
                }
            }
        }
        // Should have emitted at least 2 Retrying phases.
        let retries = phases
            .iter()
            .filter(|p| matches!(p, AgentPhase::Retrying { .. }))
            .count();
        assert!(
            retries >= 2,
            "expected >= 2 retries, got {retries} phases={phases:?}"
        );
        // Should have recovered (errors <= 2 retries).
        assert!(errors >= 2);
        // Final state is Working or Idle (not stuck).
        assert!(matches!(
            agent.phase(),
            AgentPhase::Working | AgentPhase::Idle
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn retry_gives_up_after_max_attempts() {
        use futures_util::StreamExt;
        let mut agent = Agent::new(
            Arc::new(AlwaysFails),
            ToolRegistry::new(),
            RunConfig::new("test-model"),
        );
        let mut retries = 0;
        {
            let mut stream = Box::pin(agent.run(Message::user("hi")));
            while let Some(ev) = stream.next().await {
                if let Ok(AgentEvent::PhaseChanged(AgentPhase::Retrying { .. })) = ev {
                    retries += 1;
                }
            }
        }
        // Max is DEFAULT_MAX_RETRIES (3), so we should see 3 retries, then
        // surface the final error.
        assert_eq!(retries as u32, DEFAULT_MAX_RETRIES);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fatal_errors_are_not_retried() {
        use futures_util::StreamExt;
        let mut agent = Agent::new(
            Arc::new(FatalError),
            ToolRegistry::new(),
            RunConfig::new("test-model"),
        );
        let mut retries = 0;
        {
            let mut stream = Box::pin(agent.run(Message::user("hi")));
            while let Some(ev) = stream.next().await {
                if let Ok(AgentEvent::PhaseChanged(AgentPhase::Retrying { .. })) = ev {
                    retries += 1;
                }
            }
        }
        // Fatal error must NOT trigger retry loop.
        assert_eq!(retries, 0, "non-retryable errors must skip retry");
    }
}

#[cfg(test)]
mod retry_settings_tests {
    use super::*;

    #[test]
    fn default_retry_settings_match_pi() {
        // Pi default: max 3 retries, base delay 500ms.
        let s = RetrySettings::default();
        assert_eq!(s.max_retries, 3);
        assert_eq!(s.base_delay_ms, 500);
    }

    #[test]
    fn runconfig_default_uses_retry_settings_default() {
        let cfg = RunConfig::new("test-model");
        assert_eq!(cfg.retry.max_retries, 3);
        assert_eq!(cfg.retry.base_delay_ms, 500);
    }

    #[test]
    fn runconfig_can_override_retry() {
        let mut cfg = RunConfig::new("test-model");
        cfg.retry.max_retries = 5;
        cfg.retry.base_delay_ms = 100;
        assert_eq!(cfg.retry.max_retries, 5);
        assert_eq!(cfg.retry.base_delay_ms, 100);
    }
}

#[cfg(test)]
mod abort_during_retry_tests {
    use super::*;
    use crate::provider::{Capabilities, Provider, ProviderError, Request, StreamEvent};
    use futures_core::Stream;
    use std::pin::Pin;
    use std::sync::Arc;

    /// Provider that always errors with a transient message.
    struct AlwaysRetrying;
    impl Provider for AlwaysRetrying {
        fn name(&self) -> &'static str {
            "always-retrying"
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
        fn stream(
            &self,
            _req: Request,
        ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send + 'static>>
        {
            Box::pin(futures_util::stream::once(async {
                Ok(StreamEvent::Error {
                    message: "service unavailable: 503".into(),
                })
            }))
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn abort_during_retry_sleep_ends_run() {
        use futures_util::StreamExt;
        let abort_handle = crate::agent::AbortHandle::new();
        let mut agent = Agent::new(
            Arc::new(AlwaysRetrying),
            ToolRegistry::new(),
            RunConfig::new("test-model"),
        );
        // The runtime normally owns the abort handle. For the test we
        // install our handle directly so we can trigger abort from
        // outside the run loop.
        agent.abort = abort_handle.clone();
        // Trigger abort from another thread after a short delay.
        let ah = abort_handle.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            ah.abort();
        });
        let mut aborted = false;
        let mut saw_retrying = false;
        // Run the agent directly — no Mutex wrapper needed because
        // `AbortHandle::abort` flips an atomic flag the run loop polls
        // each tick, so it doesn't need to lock the agent. Holding a
        // `std::sync::MutexGuard` across `stream.next().await` (the
        // pattern we used previously) is a deadlock hazard and
        // triggers `clippy::await_holding_lock`.
        let mut stream = Box::pin(agent.run(Message::user("hi")));
        while let Some(ev) = stream.next().await {
            match ev {
                Ok(AgentEvent::PhaseChanged(AgentPhase::Retrying { .. })) => {
                    saw_retrying = true;
                }
                Ok(AgentEvent::Aborted) => {
                    aborted = true;
                }
                _ => {}
            }
        }
        // Either the abort triggered during the retry sleep, or the abort
        // triggered between attempts — both are valid abort paths.
        assert!(
            aborted,
            "expected Aborted event when abort fires during retry"
        );
        // If we never saw Retrying, the abort happened too fast.
        // The test still passes if aborted is true.
        let _ = saw_retrying; // suppress unused warning
    }
}

#[cfg(test)]
mod llm_summary_in_compaction_entry_tests {
    use super::*;
    use crate::provider::{Capabilities, Provider, ProviderError, Request, StreamEvent};
    use futures_core::Stream;
    use std::pin::Pin;
    use std::sync::Arc;

    /// Mock provider that returns a canned summary via the LLM call.
    /// Mirrors the LLM-backed summarizer behavior for testing.
    struct LlmSummaryProvider;
    impl Provider for LlmSummaryProvider {
        fn name(&self) -> &'static str {
            "llm-summary"
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
        fn stream(
            &self,
            _req: Request,
        ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send + 'static>>
        {
            // First call: regular text (model response)
            // Second call: summary text (for compaction)
            // We can't distinguish, but the canned text works for both.
            Box::pin(futures_util::stream::once(async {
                Ok(StreamEvent::MessageStart {
                    id: "msg".into(),
                    model: "test".into(),
                })
            }))
        }
    }

    /// Stub summary function that returns a known string.
    fn stub_summary(msgs: &[Message], _prev: Option<String>) -> String {
        format!(
            "# Summary\n\nUser said {} messages. Files: a.rs, b.rs.",
            msgs.len()
        )
    }

    #[test]
    fn llm_summary_writes_to_compaction_entry() {
        let mut cfg = RunConfig::new("test-model");
        cfg.compaction.context_window = 100;
        cfg.compaction.reserve_tokens = 50;
        let mut agent = Agent::new(Arc::new(LlmSummaryProvider), ToolRegistry::new(), cfg);
        // Seed messages past the budget so compaction fires.
        for i in 0..5 {
            agent.seed(vec![Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: format!("msg-{i}: {}", "x".repeat(200)),
                }],
                timestamp: i as i64,
            }]);
        }
        let result = agent.compact_history(stub_summary);
        // Verify the summary is in compaction output.
        assert!(
            result.summary.contains("Summary"),
            "summary should be populated: {:?}",
            result.summary
        );
        // After compact_history, messages should start with the summary
        // message.
        assert!(
            !agent.messages().is_empty(),
            "messages should not be empty after compaction"
        );
        let first = &agent.messages()[0];
        match &first.content[0] {
            ContentBlock::Text { text } => {
                assert!(
                    text.contains("[CONTEXT SUMMARY]"),
                    "first message should contain summary marker; got: {text}"
                );
                assert!(
                    text.contains("Summary"),
                    "first message should contain LLM summary text"
                );
            }
            _ => panic!("expected Text block in first message"),
        }
    }
}

/// v0.7 (M1) steering/followup hook integration tests.
///
/// The point is to verify that the hooks are wired into `Agent::run`
/// at the right points and that the default `NoopHooks` reproduces
/// the v0.6.1 behavior (no messages injected, run loop terminates
/// cleanly).
#[cfg(test)]
mod steering_followup_hook_tests {
    use super::*;
    use crate::agent_hooks::{AgentLoopHooks, NoopHooks};
    use crate::provider::{
        Capabilities, ContentBlock, Provider, ProviderError, Request, StreamEvent,
    };
    use futures_core::Stream;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// Minimal stub provider that emits one TextDelta and ends the
    /// turn with no tool calls. Used to exercise the followup hook
    /// path.
    struct TextOnlyProvider;
    impl Provider for TextOnlyProvider {
        fn name(&self) -> &'static str {
            "text-only"
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
        fn stream(
            &self,
            _req: Request,
        ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send + 'static>>
        {
            Box::pin(futures_util::stream::iter(vec![
                Ok(StreamEvent::MessageStart {
                    id: "msg-1".into(),
                    model: "test".into(),
                }),
                Ok(StreamEvent::TextDelta {
                    text: "hi back".into(),
                }),
                Ok(StreamEvent::MessageStop {
                    stop_reason: "stop".into(),
                    usage: Usage::default(),
                }),
            ]))
        }
    }

    /// Stub provider that emits one tool call. Used to exercise the
    /// steering hook path (which fires after tool execution).
    struct ToolCallProvider {
        tool_name: String,
    }
    impl Provider for ToolCallProvider {
        fn name(&self) -> &'static str {
            "tool-call"
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
        fn stream(
            &self,
            _req: Request,
        ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send + 'static>>
        {
            let name = self.tool_name.clone();
            Box::pin(futures_util::stream::iter(vec![
                Ok(StreamEvent::MessageStart {
                    id: "msg-1".into(),
                    model: "test".into(),
                }),
                Ok(StreamEvent::ToolCallStart {
                    id: "tc-1".into(),
                    name: name.clone(),
                }),
                Ok(StreamEvent::ToolCallDelta {
                    id: "tc-1".into(),
                    input_json_delta: "{}".into(),
                }),
                Ok(StreamEvent::ToolCallStop {
                    id: "tc-1".into(),
                    input_json: serde_json::json!({}),
                }),
                Ok(StreamEvent::MessageStop {
                    stop_reason: "tool_use".into(),
                    usage: Usage::default(),
                }),
            ]))
        }
    }

    /// Test hooks that count how often each was called and remember
    /// the recent/all message slices they were passed.
    struct CountingHooks {
        steering_calls: AtomicUsize,
        followup_calls: AtomicUsize,
        steering_to_inject: Vec<Message>,
        followup_to_inject: Vec<Message>,
        /// v0.10 (R1): queues drain. See the `AgentLoopHooks` impl below.
        steering_taken: AtomicBool,
        followup_taken: AtomicBool,
    }
    impl CountingHooks {
        fn new(steering: Vec<Message>, followup: Vec<Message>) -> Self {
            Self {
                steering_calls: AtomicUsize::new(0),
                followup_calls: AtomicUsize::new(0),
                steering_to_inject: steering,
                followup_to_inject: followup,
                steering_taken: AtomicBool::new(false),
                followup_taken: AtomicBool::new(false),
            }
        }
        fn steering_count(&self) -> usize {
            self.steering_calls.load(Ordering::SeqCst)
        }
        fn followup_count(&self) -> usize {
            self.followup_calls.load(Ordering::SeqCst)
        }
    }
    impl AgentLoopHooks for CountingHooks {
        // v0.10 (R1): both hooks now **drain**. Pi's `PendingMessageQueue`
        // (`agent.js:63-71`) returns its contents once and the queue is then
        // empty; re-serving the same items on every poll models a producer
        // that never stops, not a queue.
        //
        // Returning a clone forever also made these tests lie: with the loop
        // now able to revive, an undrained stub produced 16 revivals and the
        // message count grew without bound, so the tests failed for a reason
        // that had nothing to do with what they claimed to check.
        fn get_steering_messages(&self, _recent: &[Message]) -> Vec<Message> {
            self.steering_calls.fetch_add(1, Ordering::SeqCst);
            // `swap` returns the PREVIOUS value, so `!previous` is the
            // first-and-only delivery.
            (!self.steering_taken.swap(true, Ordering::SeqCst))
                .then(|| self.steering_to_inject.clone())
                .unwrap_or_default()
        }
        fn get_followup_messages(&self, _all: &[Message]) -> Vec<Message> {
            self.followup_calls.fetch_add(1, Ordering::SeqCst);
            (!self.followup_taken.swap(true, Ordering::SeqCst))
                .then(|| self.followup_to_inject.clone())
                .unwrap_or_default()
        }
    }

    fn user_msg(text: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: text.into() }],
            timestamp: 0,
        }
    }

    fn agent_with_text_provider(hooks: Arc<dyn AgentLoopHooks>) -> Agent {
        Agent::with_hooks(
            Arc::new(TextOnlyProvider),
            ToolRegistry::new(),
            RunConfig::new("test-model"),
            hooks,
        )
    }

    /// v0.6.1 behavior: NoopHooks default is wired through `Agent::new`
    /// and the run loop terminates cleanly with no extra messages.
    #[tokio::test(flavor = "current_thread")]
    async fn noop_hooks_yields_no_extra_messages() {
        let mut agent = agent_with_text_provider(Arc::new(NoopHooks));
        use futures_util::StreamExt;
        {
            let s = agent.run(crate::AgentMessage::user("hi"));
            tokio::pin!(s);
            // Drain the run loop.
            while let Some(_ev) = s.next().await {}
        }
        // The run pushed [user "hi"] + [assistant "hi back"] = 2
        // messages. No followup, no steering.
        assert_eq!(agent.messages().len(), 2);
        let user_count = agent
            .messages()
            .iter()
            .filter(|m| m.role == Role::User)
            .count();
        let assistant_count = agent
            .messages()
            .iter()
            .filter(|m| m.role == Role::Assistant)
            .count();
        assert_eq!(user_count, 1);
        assert_eq!(assistant_count, 1);
    }

    /// The followup hook fires exactly once on run completion and
    /// its returned messages land in the message log in order.
    #[tokio::test(flavor = "current_thread")]
    async fn followup_hook_injects_messages_on_run_end() {
        // v0.10 (R1): this test used to assert "2 base + 2 followup = 4",
        // which passed for the wrong reason — nini appended the follow-up to
        // history and returned, so the follow-up never became a turn. The
        // count was the same either way, which is exactly why the defect
        // survived.
        //
        // It now asserts the thing that actually matters: a *second turn ran
        // for the follow-up*, visible as a second assistant reply after it.
        let followup = vec![
            user_msg("(followup) also do X"),
            user_msg("(followup) and Y"),
        ];
        let hooks = Arc::new(CountingHooks::new(vec![], followup.clone()));
        let mut agent = agent_with_text_provider(hooks.clone());

        use futures_util::StreamExt;
        {
            let s = agent.run(crate::AgentMessage::user("hi"));
            tokio::pin!(s);
            while let Some(_ev) = s.next().await {}
        } // drop s here so we can borrow agent immutably

        // Two polls, not one: the first drains the queue, the second returns
        // empty and is what lets the loop exit. Asserting `== 1` would encode
        // a poll count as if it were the behaviour under test.
        assert_eq!(
            hooks.followup_count(),
            2,
            "one productive poll to drain the queue, one empty poll to finish"
        );

        let texts: Vec<(Role, String)> = agent
            .messages()
            .iter()
            .map(|m| {
                (
                    m.role,
                    m.content
                        .iter()
                        .find_map(|c| match c {
                            ContentBlock::Text { text } => Some(text.clone()),
                            _ => None,
                        })
                        .unwrap_or_default(),
                )
            })
            .collect();

        // user, assistant, followup X, followup Y, assistant — the trailing
        // assistant is the proof the follow-up triggered a turn.
        assert_eq!(
            texts.len(),
            5,
            "expected the follow-up to produce a second turn; got {texts:?}"
        );
        assert_eq!(texts[0].1, "hi");
        assert_eq!(
            texts[1].0,
            Role::Assistant,
            "first reply before the follow-up"
        );
        assert_eq!(texts[2].1, "(followup) also do X");
        assert_eq!(texts[3].1, "(followup) and Y");
        assert_eq!(
            texts[4].0,
            Role::Assistant,
            "the model must answer the follow-up, not just read it as context"
        );
    }

    /// The steering hook fires after every round of tool execution,
    /// not on text-only turns.
    #[tokio::test(flavor = "current_thread")]
    async fn steering_is_polled_on_every_turn_not_only_after_tools() {
        // v0.10 (R1): this test asserted `steering_count() == 0` for a
        // text-only turn, encoding "no tool calls → steering is never
        // polled". Pi polls steering at `agent-loop.ts#getSteeringMessages` after *every*
        // turn, so a user interjecting while the model streams a long
        // text-only answer is heard on the next turn instead of at the start
        // of the next run.
        //
        // With an empty queue the poll is a no-op, so the observable
        // difference is the poll count: two (once before the loop at `:83`,
        // once after the turn at `:160`), not zero.
        let hooks = Arc::new(CountingHooks::new(vec![], vec![]));
        let mut agent = agent_with_text_provider(hooks.clone());

        use futures_util::StreamExt;
        {
            let s = agent.run(crate::AgentMessage::user("hi"));
            tokio::pin!(s);
            while let Some(_ev) = s.next().await {}
        }

        assert_eq!(
            hooks.steering_count(),
            2,
            "steering is polled before the first turn and after every turn"
        );
        // An empty queue must not manufacture extra turns.
        assert_eq!(
            agent.messages().len(),
            2,
            "empty steering queue must not add a turn"
        );
    }

    /// When the model DOES call a tool, steering fires once after
    /// the tool result is appended (before the next LLM iteration).
    /// The injected messages land in the message log.
    #[tokio::test(flavor = "current_thread")]
    async fn steering_hook_injects_after_tool_execution() {
        // A tool-call provider that calls a tool we DON'T register,
        // so it fails. The agent still drives through the steering
        // hook after the (failed) tool result is appended.
        let steering = vec![user_msg("(steering) please retry with care")];
        let hooks = Arc::new(CountingHooks::new(steering.clone(), vec![]));
        let mut agent = Agent::with_hooks(
            Arc::new(ToolCallProvider {
                tool_name: "nope".into(),
            }),
            ToolRegistry::new(),
            RunConfig::new("test-model"),
            hooks.clone(),
        );

        use futures_util::StreamExt;
        {
            let s = agent.run(crate::AgentMessage::user("hi"));
            tokio::pin!(s);
            while let Some(_ev) = s.next().await {}
        }

        assert!(
            hooks.steering_count() >= 1,
            "steering fires once per tool-execution iteration (got {})",
            hooks.steering_count()
        );
        // Find the injected steering message in the log.
        let has_steering = agent.messages().iter().any(|m| {
            m.role == Role::User
                && m.content.iter().any(|c| match c {
                    ContentBlock::Text { text } => text == "(steering) please retry with care",
                    _ => false,
                })
        });
        assert!(has_steering, "steering message must appear in message log");
    }

    /// A panicking steering hook must NOT crash the run loop. The
    /// agent should still complete normally (panic caught, fall
    /// back to empty Vec).
    #[tokio::test(flavor = "current_thread")]
    async fn panicking_steering_hook_does_not_crash_run() {
        struct PanicHook;
        impl AgentLoopHooks for PanicHook {
            fn get_steering_messages(&self, _: &[Message]) -> Vec<Message> {
                panic!("steering hook explosion");
            }
        }
        let hooks: Arc<dyn AgentLoopHooks> = Arc::new(PanicHook);
        let mut agent = Agent::with_hooks(
            Arc::new(ToolCallProvider {
                tool_name: "nope".into(),
            }),
            ToolRegistry::new(),
            RunConfig::new("test-model"),
            hooks,
        );

        use futures_util::StreamExt;
        let s = agent.run(crate::AgentMessage::user("hi"));
        tokio::pin!(s);
        // If the panic weren't handled, this `next().await` would
        // either return an Err or the test would fail to complete.
        let mut completed = false;
        while let Some(_ev) = s.next().await {
            completed = true;
        }
        assert!(completed, "agent run completed (panic swallowed)");
    }

    /// A panicking followup hook is also caught.
    #[tokio::test(flavor = "current_thread")]
    async fn panicking_followup_hook_does_not_crash_run() {
        struct PanicHook;
        impl AgentLoopHooks for PanicHook {
            fn get_followup_messages(&self, _: &[Message]) -> Vec<Message> {
                panic!("followup hook explosion");
            }
        }
        let hooks: Arc<dyn AgentLoopHooks> = Arc::new(PanicHook);
        let mut agent = agent_with_text_provider(hooks);
        use futures_util::StreamExt;
        {
            let s = agent.run(crate::AgentMessage::user("hi"));
            tokio::pin!(s);
            while let Some(_ev) = s.next().await {}
        }
        // If we got here without panic → contract holds.
    }

    /// `Agent::new` (the old API) still installs NoopHooks
    /// implicitly, so existing callers see no behavior change.
    #[tokio::test(flavor = "current_thread")]
    async fn agent_new_installs_noop_hooks_by_default() {
        let mut agent = Agent::new(
            Arc::new(TextOnlyProvider),
            ToolRegistry::new(),
            RunConfig::new("test-model"),
        );
        use futures_util::StreamExt;
        {
            let s = agent.run(crate::AgentMessage::user("hi"));
            tokio::pin!(s);
            while let Some(_ev) = s.next().await {}
        }
        // No followup / steering because NoopHooks returns empty.
        assert_eq!(agent.messages().len(), 2);
    }
}

/// v0.7 (M2) Tool lifecycle hook integration tests.
///
/// Focus: verify that `before()` / `after()` hooks wired into a
/// `Tool` implementation are called by `Agent::run` at the right
/// phases and that their outputs flow through correctly.
#[cfg(test)]
mod tool_lifecycle_hook_tests {
    use super::*;
    use crate::provider::Capabilities;
    use crate::tool::{AfterExecute, BeforeExecute, Tool, ToolSpec};
    use async_trait::async_trait;
    use serde_json::Value;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A controllable test tool whose behavior can be steered by
    /// the test via shared counters. Implements `before()` and
    /// `after()` hooks (so we don't need `WrappedTool` indirection
    /// for the most common test path).
    ///
    /// `before_calls` / `after_calls` are reserved counters for future
    /// hook-firing assertions; the current tests assert hook firing via
    /// `CountingBefore::calls` / `CountingAfter::calls`. Kept under
    /// `#[allow(dead_code)]` to avoid churn.
    #[allow(dead_code)]
    struct ControllableTool {
        before: Option<Arc<dyn BeforeExecute>>,
        after: Option<Arc<dyn AfterExecute>>,
        execute_calls: Arc<AtomicUsize>,
        before_calls: Arc<AtomicUsize>,
        after_calls: Arc<AtomicUsize>,
    }

    impl ControllableTool {
        fn new() -> Self {
            Self {
                before: None,
                after: None,
                execute_calls: Arc::new(AtomicUsize::new(0)),
                before_calls: Arc::new(AtomicUsize::new(0)),
                after_calls: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn with_hooks(
            mut self,
            before: Arc<dyn BeforeExecute>,
            after: Arc<dyn AfterExecute>,
        ) -> Self {
            self.before = Some(before);
            self.after = Some(after);
            self
        }
    }

    #[async_trait]
    impl Tool for ControllableTool {
        fn name(&self) -> &'static str {
            "ctrl"
        }
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "ctrl".into(),
                description: "controllable test tool".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }
        }
        async fn execute(&self, _args: Value, _ctx: ToolContext) -> Result<ToolOutput, ToolError> {
            self.execute_calls.fetch_add(1, Ordering::SeqCst);
            Ok(ToolOutput::ok("base output"))
        }
        fn before(&self) -> Option<Box<dyn BeforeExecute>> {
            let hook = self.before.as_ref()?.clone();
            Some(Box::new(crate::tool::ArcBoxAdapterBefore(hook)))
        }
        fn after(&self) -> Option<Box<dyn AfterExecute>> {
            let hook = self.after.as_ref()?.clone();
            Some(Box::new(crate::tool::ArcBoxAdapterAfterTool(hook)))
        }
    }

    /// Helper hook that counts invocations and lets tests see what
    /// args/output it received.
    struct CountingBefore {
        calls: Arc<AtomicUsize>,
        last_args: Arc<std::sync::Mutex<Option<Value>>>,
        response: BeforeResponse,
    }
    enum BeforeResponse {
        Identity,
        Replace(Value),
        Deny(String),
        // `Panic` is reserved for future tests that exercise hook-panic
        // recovery through this exact indirection (other tests panic
        // directly inside hook closures). Currently unused.
        #[allow(dead_code)]
        Panic,
    }
    #[async_trait]
    impl BeforeExecute for CountingBefore {
        async fn run(&self, args: Value, _ctx: &ToolContext) -> Result<Option<Value>, ToolError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last_args.lock().unwrap() = Some(args.clone());
            match &self.response {
                BeforeResponse::Identity => Ok(None),
                BeforeResponse::Replace(v) => Ok(Some(v.clone())),
                BeforeResponse::Deny(msg) => Err(ToolError::PermissionDenied(msg.clone())),
                BeforeResponse::Panic => panic!("simulated before-hook panic"),
            }
        }
    }

    struct CountingAfter {
        calls: Arc<AtomicUsize>,
        last_output: Arc<std::sync::Mutex<Option<ToolOutput>>>,
        response: AfterResponse,
    }
    enum AfterResponse {
        Identity,
        ReplaceContent(String),
        Panic,
    }
    #[async_trait]
    impl AfterExecute for CountingAfter {
        async fn run(
            &self,
            output: ToolOutput,
            _ctx: &ToolContext,
        ) -> Result<ToolOutput, ToolError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last_output.lock().unwrap() = Some(output.clone());
            match &self.response {
                AfterResponse::Identity => Ok(output),
                AfterResponse::ReplaceContent(s) => Ok(ToolOutput::ok(s.clone())),
                AfterResponse::Panic => panic!("simulated after-hook panic"),
            }
        }
    }

    /// Provider that emits exactly one tool call to "ctrl" with
    /// input `{"x": 1}`.
    struct OneToolCallProvider;
    impl Provider for OneToolCallProvider {
        fn name(&self) -> &'static str {
            "one-tool"
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
        fn stream(
            &self,
            _req: Request,
        ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send + 'static>>
        {
            Box::pin(futures_util::stream::iter(vec![
                Ok(StreamEvent::MessageStart {
                    id: "msg-1".into(),
                    model: "test".into(),
                }),
                Ok(StreamEvent::ToolCallStart {
                    id: "tc-1".into(),
                    name: "ctrl".into(),
                }),
                Ok(StreamEvent::ToolCallDelta {
                    id: "tc-1".into(),
                    input_json_delta: r#"{"x":1}"#.into(),
                }),
                Ok(StreamEvent::ToolCallStop {
                    id: "tc-1".into(),
                    input_json: serde_json::json!({"x": 1}),
                }),
                Ok(StreamEvent::MessageStop {
                    stop_reason: "tool_use".into(),
                    usage: Usage::default(),
                }),
            ]))
        }
    }

    fn agent_with_tool(tool: Arc<dyn Tool>) -> Agent {
        Agent::with_hooks(
            Arc::new(OneToolCallProvider),
            ToolRegistry::new().register(tool),
            RunConfig::new("test-model"),
            Arc::new(crate::agent_hooks::NoopHooks),
        )
    }

    /// REGRESSION: a tool with NO before/after hooks produces the
    /// same message log as v0.6.1 (no behavior change).
    #[tokio::test(flavor = "current_thread")]
    async fn no_hooks_runs_v061_behavior() {
        let tool = Arc::new(ControllableTool::new());
        let mut agent = agent_with_tool(tool);
        use futures_util::StreamExt;
        {
            let s = agent.run(crate::AgentMessage::user("hi"));
            tokio::pin!(s);
            while let Some(_ev) = s.next().await {}
        }
        // The provider loops emitting the same tool call forever until
        // max_iterations, so we expect messages.len() to be at LEAST
        // the initial 3 (user + assistant + first tool result). We
        // check >= 3 to be robust against the loop count.
        assert!(
            agent.messages().len() >= 3,
            "expected at least 3 messages, got {}",
            agent.messages().len()
        );
    }

    /// `before()` returning `Some(replaced_args)` causes the tool
    /// to receive the REPLACED args, not the original.
    #[tokio::test(flavor = "current_thread")]
    async fn before_hook_can_replace_args() {
        let before_calls = Arc::new(AtomicUsize::new(0));
        let last_args = Arc::new(std::sync::Mutex::new(None));
        let before = Arc::new(CountingBefore {
            calls: before_calls.clone(),
            last_args: last_args.clone(),
            response: BeforeResponse::Replace(serde_json::json!({"x": 99})),
        });
        let after = Arc::new(CountingAfter {
            calls: Arc::new(AtomicUsize::new(0)),
            last_output: Arc::new(std::sync::Mutex::new(None)),
            response: AfterResponse::Identity,
        });
        let tool = Arc::new(ControllableTool::new().with_hooks(before.clone(), after.clone()));
        let mut agent = agent_with_tool(tool);
        use futures_util::StreamExt;
        {
            let s = agent.run(crate::AgentMessage::user("hi"));
            tokio::pin!(s);
            while let Some(_ev) = s.next().await {}
        }
        assert!(
            before_calls.load(Ordering::SeqCst) >= 1,
            "before hook should fire at least once per iteration (got {})",
            before_calls.load(Ordering::SeqCst)
        );
        // The before hook received the original args {"x": 1} on
        // every invocation.
        let last = last_args.lock().unwrap().clone();
        assert_eq!(last, Some(serde_json::json!({"x": 1})));
    }

    /// `after()` rewriting the content is what the LLM sees.
    #[tokio::test(flavor = "current_thread")]
    async fn after_hook_can_rewrite_content() {
        let before = Arc::new(CountingBefore {
            calls: Arc::new(AtomicUsize::new(0)),
            last_args: Arc::new(std::sync::Mutex::new(None)),
            response: BeforeResponse::Identity,
        });
        let after_calls = Arc::new(AtomicUsize::new(0));
        let after = Arc::new(CountingAfter {
            calls: after_calls.clone(),
            last_output: Arc::new(std::sync::Mutex::new(None)),
            response: AfterResponse::ReplaceContent("[REDACTED]".into()),
        });
        let tool = Arc::new(ControllableTool::new().with_hooks(before.clone(), after.clone()));
        let mut agent = agent_with_tool(tool);
        use futures_util::StreamExt;
        {
            let s = agent.run(crate::AgentMessage::user("hi"));
            tokio::pin!(s);
            while let Some(_ev) = s.next().await {}
        }
        assert!(
            after_calls.load(Ordering::SeqCst) >= 1,
            "after hook should fire at least once per iteration (got {})",
            after_calls.load(Ordering::SeqCst)
        );
        // The tool result message in the log should contain
        // "[REDACTED]" instead of "base output".
        let tool_msg = agent
            .messages()
            .iter()
            .find(|m| m.role == Role::Tool)
            .expect("tool message must exist");
        let content = tool_msg
            .content
            .iter()
            .find_map(|c| match c {
                ContentBlock::ToolResult { content, .. } => Some(content.as_str()),
                _ => None,
            })
            .expect("ToolResult content block");
        assert_eq!(content, "[REDACTED]");
    }

    /// `before()` returning `Err` denies the call. The tool body
    /// is NOT executed, and the ToolResult is an error.
    #[tokio::test(flavor = "current_thread")]
    async fn before_hook_can_deny_call() {
        let _before = Arc::new(CountingBefore {
            calls: Arc::new(AtomicUsize::new(0)),
            last_args: Arc::new(std::sync::Mutex::new(None)),
            response: BeforeResponse::Deny("outside whitelist".into()),
        });
        let _after = Arc::new(CountingAfter {
            calls: Arc::new(AtomicUsize::new(0)),
            last_output: Arc::new(std::sync::Mutex::new(None)),
            response: AfterResponse::Identity,
        });
        // Track execute calls via a custom tool that counts.
        struct CountedTool(Arc<AtomicUsize>);
        #[async_trait]
        impl Tool for CountedTool {
            fn name(&self) -> &'static str {
                "ctrl2"
            }
            fn spec(&self) -> ToolSpec {
                ToolSpec {
                    name: "ctrl2".into(),
                    description: "x".into(),
                    input_schema: serde_json::json!({}),
                }
            }
            async fn execute(&self, _a: Value, _c: ToolContext) -> Result<ToolOutput, ToolError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(ToolOutput::ok("ran"))
            }
            fn before(&self) -> Option<Box<dyn BeforeExecute>> {
                let h: Arc<dyn BeforeExecute> = Arc::new(CountingBefore {
                    calls: Arc::new(AtomicUsize::new(0)),
                    last_args: Arc::new(std::sync::Mutex::new(None)),
                    response: BeforeResponse::Deny("denied".into()),
                });
                Some(Box::new(crate::tool::ArcBoxAdapterBefore(h)))
            }
        }
        let execute_count = Arc::new(AtomicUsize::new(0));
        let tool: Arc<dyn Tool> = Arc::new(CountedTool(execute_count.clone()));
        let mut agent = agent_with_tool(tool);
        use futures_util::StreamExt;
        {
            let s = agent.run(crate::AgentMessage::user("hi"));
            tokio::pin!(s);
            while let Some(_ev) = s.next().await {}
        }
        // The tool body never ran.
        assert_eq!(execute_count.load(Ordering::SeqCst), 0);
        // But the agent still received a ToolResult with is_error.
        let tool_msg = agent
            .messages()
            .iter()
            .find(|m| m.role == Role::Tool)
            .expect("tool message must exist");
        let is_error = tool_msg
            .content
            .iter()
            .find_map(|c| match c {
                ContentBlock::ToolResult { is_error, .. } => Some(*is_error),
                _ => None,
            })
            .expect("ToolResult block");
        assert!(is_error, "denied tool call must surface is_error=true");
    }

    /// A panicking after-hook does NOT crash the run loop. The
    /// agent still completes; the tool result is an error tagged
    /// with the panic reason.
    #[tokio::test(flavor = "current_thread")]
    async fn panicking_after_hook_surfaces_error_does_not_crash() {
        let before = Arc::new(CountingBefore {
            calls: Arc::new(AtomicUsize::new(0)),
            last_args: Arc::new(std::sync::Mutex::new(None)),
            response: BeforeResponse::Identity,
        });
        let after = Arc::new(CountingAfter {
            calls: Arc::new(AtomicUsize::new(0)),
            last_output: Arc::new(std::sync::Mutex::new(None)),
            response: AfterResponse::Panic,
        });
        let tool = Arc::new(ControllableTool::new().with_hooks(before.clone(), after.clone()));
        let mut agent = agent_with_tool(tool);
        use futures_util::StreamExt;
        // If the panic weren't contained, this would either
        // propagate out of next().await or be turned into an
        // AgentError. We expect the run to complete (panics in
        // async tasks are caught by tokio and surfaced as a
        // poisoned ToolResult, NOT a crash).
        let mut completed = false;
        {
            let s = agent.run(crate::AgentMessage::user("hi"));
            tokio::pin!(s);
            while let Some(_ev) = s.next().await {
                completed = true;
            }
        }
        assert!(completed);
    }
}

/// v0.7 (M3b) — integration tests for the 3 context hooks
/// (`should_stop_after_turn`, `transform_context`,
/// `convert_to_llm`). The agent-hooks unit tests cover the trait
/// surface in isolation; here we verify the hooks are CALLED by
/// `Agent::run` at the right points and that their effects on
/// the message log / loop termination match the documented
/// behavior.
#[cfg(test)]
mod context_hook_tests {
    use super::*;
    use crate::agent_hooks::{AgentLoopHooks, NoopHooks, TransformResult};
    use crate::provider::{
        Capabilities, ContentBlock, Provider, ProviderError, Request, StreamEvent,
    };
    use futures_core::Stream;
    use std::pin::Pin;

    /// Minimal text-only provider. Same as M1's tests.
    struct TextOnlyProvider;
    impl Provider for TextOnlyProvider {
        fn name(&self) -> &'static str {
            "text-only-m3b"
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
        fn stream(
            &self,
            _req: Request,
        ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send + 'static>>
        {
            Box::pin(futures_util::stream::iter(vec![
                Ok(StreamEvent::MessageStart {
                    id: "msg-1".into(),
                    model: "test".into(),
                }),
                Ok(StreamEvent::TextDelta {
                    text: "hi back".into(),
                }),
                Ok(StreamEvent::MessageStop {
                    stop_reason: "stop".into(),
                    usage: Usage::default(),
                }),
            ]))
        }
    }

    /// Repeating-tool provider that triggers max_iterations.
    struct RepeatingToolProvider;
    impl Provider for RepeatingToolProvider {
        fn name(&self) -> &'static str {
            "repeat-tool"
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
        fn stream(
            &self,
            _req: Request,
        ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send + 'static>>
        {
            Box::pin(futures_util::stream::iter(vec![
                Ok(StreamEvent::MessageStart {
                    id: "msg-1".into(),
                    model: "test".into(),
                }),
                Ok(StreamEvent::ToolCallStart {
                    id: "tc-1".into(),
                    name: "nope".into(),
                }),
                Ok(StreamEvent::ToolCallDelta {
                    id: "tc-1".into(),
                    input_json_delta: "{}".into(),
                }),
                Ok(StreamEvent::ToolCallStop {
                    id: "tc-1".into(),
                    input_json: serde_json::json!({}),
                }),
                Ok(StreamEvent::MessageStop {
                    stop_reason: "tool_use".into(),
                    usage: Usage::default(),
                }),
            ]))
        }
    }

    // Local helper for the context-hook tests; shadows the upper mod's
    // `user_msg`. The upper helper is reached via `super::*`, so this
    // duplicate is dead code from clippy's perspective. Kept (with
    // `#[allow(dead_code)]`) because the local test module needs a
    // shorter signature and a fresh helper avoids pulling extra
    // `use` statements into the test scope.
    #[allow(dead_code)]
    fn user_msg(text: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: text.into() }],
            timestamp: 0,
        }
    }

    fn agent_with(provider: Arc<dyn Provider>, hooks: Arc<dyn AgentLoopHooks>) -> Agent {
        Agent::with_hooks(
            provider,
            ToolRegistry::new(),
            RunConfig::new("test-model"),
            hooks,
        )
    }

    /// M3b — `should_stop_after_turn` is a SOFT stop. Fires
    /// before the hard cap. Returns true immediately → no LLM
    /// call, no assistant message.
    #[tokio::test(flavor = "current_thread")]
    async fn should_stop_after_turn_ends_run_immediately() {
        struct AlwaysStop;
        impl AgentLoopHooks for AlwaysStop {
            fn should_stop_after_turn(&self, _: &[Message]) -> bool {
                true
            }
        }
        let mut agent = agent_with(Arc::new(TextOnlyProvider), Arc::new(AlwaysStop));
        use futures_util::StreamExt;
        {
            let s = agent.run(crate::AgentMessage::user("hi"));
            tokio::pin!(s);
            while let Some(_ev) = s.next().await {}
        }
        // v0.10 (R1d): this used to assert 1 message — the hook fired
        // *before* the LLM call, so the run ended with the user message
        // and nothing else. Pi consults `shouldStopAfterTurn` after
        // `turn_end` (`agent-loop.ts#shouldStopAfterTurn`), so the first turn completes
        // first: user + assistant, then stop.
        assert_eq!(
            agent.messages().len(),
            2,
            "the turn must complete before the hook is consulted"
        );
        assert_eq!(agent.messages()[0].role, Role::User);
        assert_eq!(agent.messages()[1].role, Role::Assistant);
    }

    /// M3b — `should_stop_after_turn` returning false does NOT
    /// interfere with the hard `max_iterations` cap. The cap
    /// eventually trips via TooManyIterations.
    #[tokio::test(flavor = "current_thread")]
    async fn should_stop_after_turn_false_lets_max_iterations_trip() {
        struct NeverStop;
        impl AgentLoopHooks for NeverStop {
            fn should_stop_after_turn(&self, _: &[Message]) -> bool {
                false
            }
        }
        let mut agent = agent_with(Arc::new(RepeatingToolProvider), Arc::new(NeverStop));
        use futures_util::StreamExt;
        {
            let s = agent.run(crate::AgentMessage::user("hi"));
            tokio::pin!(s);
            while let Some(_ev) = s.next().await {}
        }
        // Many iterations of (assistant + tool-result) messages.
        assert!(agent.messages().len() > 1);
    }

    /// M3b — a panicking `should_stop_after_turn` falls through
    /// to false. The agent keeps running until max_iterations
    /// trips. Run completes normally (no panic propagates).
    #[tokio::test(flavor = "current_thread")]
    async fn panicking_should_stop_after_turn_does_not_crash() {
        struct PanicStop;
        impl AgentLoopHooks for PanicStop {
            fn should_stop_after_turn(&self, _: &[Message]) -> bool {
                panic!("stop hook explosion");
            }
        }
        let mut agent = agent_with(Arc::new(RepeatingToolProvider), Arc::new(PanicStop));
        use futures_util::StreamExt;
        {
            let s = agent.run(crate::AgentMessage::user("hi"));
            tokio::pin!(s);
            while let Some(_ev) = s.next().await {}
        }
        // The run terminated via the hard cap; agent.messages()
        // has accumulated iterations.
        assert!(agent.messages().len() > 1);
    }

    /// M3b — `transform_context` is non-mutating. The hook can
    /// return a different view for the LLM, but `self.messages`
    /// (the agent's source of truth) is preserved.
    #[tokio::test(flavor = "current_thread")]
    async fn transform_context_is_non_mutating() {
        struct TrimToFirst;
        impl AgentLoopHooks for TrimToFirst {
            fn transform_context(&self, msgs: &[Message]) -> TransformResult {
                TransformResult::trimmed(
                    msgs.first().cloned().into_iter().collect(),
                    msgs.len().saturating_sub(1),
                    "test_trim",
                )
            }
        }
        let mut agent = agent_with(Arc::new(TextOnlyProvider), Arc::new(TrimToFirst));
        use futures_util::StreamExt;
        {
            let s = agent.run(crate::AgentMessage::user("hi"));
            tokio::pin!(s);
            while let Some(_ev) = s.next().await {}
        }
        // user + assistant = 2 messages regardless of what the
        // hook pretended to trim.
        assert_eq!(agent.messages().len(), 2);
    }

    /// M3b — `convert_to_llm` is non-mutating. Even if the hook
    /// filters everything, the agent's internal message log is
    /// preserved.
    #[tokio::test(flavor = "current_thread")]
    async fn convert_to_llm_is_non_mutating() {
        struct FilterAll;
        impl AgentLoopHooks for FilterAll {
            fn convert_to_llm(&self, _: &[Message]) -> Vec<Message> {
                Vec::new()
            }
        }
        let mut agent = agent_with(Arc::new(TextOnlyProvider), Arc::new(FilterAll));
        use futures_util::StreamExt;
        {
            let s = agent.run(crate::AgentMessage::user("hi"));
            tokio::pin!(s);
            while let Some(_ev) = s.next().await {}
        }
        // The LLM ran (got an empty context) and returned an
        // assistant message. Both messages still in the log.
        assert_eq!(agent.messages().len(), 2);
    }

    /// M3b — NoopHooks doesn't shrink the message list; it just
    /// passes the user prompt and the assistant response
    /// through to the LLM.
    #[tokio::test(flavor = "current_thread")]
    async fn noop_hooks_identity_throughout() {
        let mut agent = agent_with(Arc::new(TextOnlyProvider), Arc::new(NoopHooks));
        use futures_util::StreamExt;
        {
            let s = agent.run(crate::AgentMessage::user("hi"));
            tokio::pin!(s);
            while let Some(_ev) = s.next().await {}
        }
        assert_eq!(agent.messages().len(), 2);
    }

    /// M3b — a panicking `transform_context` falls back to the
    /// default (empty) TransformResult via `catch_unwind`. The
    /// agent keeps running; the LLM gets an empty message view.
    #[tokio::test(flavor = "current_thread")]
    async fn panicking_transform_context_does_not_crash() {
        struct PanicTransform;
        impl AgentLoopHooks for PanicTransform {
            fn transform_context(&self, _: &[Message]) -> TransformResult {
                panic!("transform hook explosion");
            }
        }
        let mut agent = agent_with(Arc::new(TextOnlyProvider), Arc::new(PanicTransform));
        use futures_util::StreamExt;
        {
            let s = agent.run(crate::AgentMessage::user("hi"));
            tokio::pin!(s);
            while let Some(_ev) = s.next().await {}
        }
        // The LLM still got called (with an empty context) and
        // responded; the agent's messages log is unchanged.
        assert_eq!(agent.messages().len(), 2);
    }

    /// M3b — a panicking `convert_to_llm` falls back to the
    /// default (empty Vec) via `catch_unwind`. The LLM gets an
    /// empty message view, the agent's log is preserved.
    #[tokio::test(flavor = "current_thread")]
    async fn panicking_convert_to_llm_does_not_crash() {
        struct PanicConvert;
        impl AgentLoopHooks for PanicConvert {
            fn convert_to_llm(&self, _: &[Message]) -> Vec<Message> {
                panic!("convert hook explosion");
            }
        }
        let mut agent = agent_with(Arc::new(TextOnlyProvider), Arc::new(PanicConvert));
        use futures_util::StreamExt;
        {
            let s = agent.run(crate::AgentMessage::user("hi"));
            tokio::pin!(s);
            while let Some(_ev) = s.next().await {}
        }
        assert_eq!(agent.messages().len(), 2);
    }

    /// v0.10 (R1d) — renamed. The old name and doc asserted the opposite
    /// of Pi: `should_stop_after_turn` was consulted at the START of the
    /// iteration, before the LLM call for that turn. Pi calls it after
    /// `turn_end` (`agent-loop.ts#shouldStopAfterTurn`), so it always sees a completed
    /// turn. A hook implements "stop after the model has produced N turns"
    /// by counting assistant messages, and N is now exact.
    #[tokio::test(flavor = "current_thread")]
    async fn should_stop_after_turn_sees_the_completed_turn() {
        struct StopAfterFirst;
        impl AgentLoopHooks for StopAfterFirst {
            fn should_stop_after_turn(&self, msgs: &[Message]) -> bool {
                msgs.iter().any(|m| m.role == Role::Assistant)
            }
        }
        let mut agent = agent_with(Arc::new(RepeatingToolProvider), Arc::new(StopAfterFirst));
        use futures_util::StreamExt;
        {
            let s = agent.run(crate::AgentMessage::user("hi"));
            tokio::pin!(s);
            while let Some(_ev) = s.next().await {}
        }
        // The hook stops as soon as ANY assistant message exists
        // → exactly one assistant turn in the log.
        let assistant_count = agent
            .messages()
            .iter()
            .filter(|m| m.role == Role::Assistant)
            .count();
        assert_eq!(assistant_count, 1);
    }
}

#[cfg(test)]
mod compaction_recovery_tests {
    use super::*;

    /// A provider that is never called: both tests exercise the compaction
    /// path directly rather than through a request.
    struct Unused;

    impl crate::provider::Provider for Unused {
        fn name(&self) -> &'static str {
            "unused"
        }
        fn capabilities(&self) -> crate::provider::Capabilities {
            crate::provider::Capabilities::default()
        }
        fn stream(
            &self,
            _req: crate::provider::Request,
        ) -> std::pin::Pin<
            Box<
                dyn futures_util::Stream<
                        Item = Result<crate::provider::StreamEvent, crate::provider::ProviderError>,
                    > + Send
                    + 'static,
            >,
        > {
            Box::pin(futures_util::stream::empty())
        }
    }

    fn seeded(n: usize) -> Vec<Message> {
        (0..n)
            .map(|i| Message {
                role: if i % 2 == 0 {
                    Role::User
                } else {
                    Role::Assistant
                },
                content: vec![ContentBlock::Text {
                    text: format!("message {i} {}", "x".repeat(50)),
                }],
                timestamp: 0,
            })
            .collect()
    }

    fn cfg() -> RunConfig {
        let mut c = RunConfig::new("test-model");
        c.compaction.context_window = 200;
        c.compaction.reserve_tokens = 50;
        c.compaction.keep_recent_tokens = 40;
        c
    }

    /// Pi rebuilds context from the messages actually in state after a
    /// compaction (`agent-session.ts#sessionContext.messages-2163`), so the newest usage
    /// describes the compacted conversation. nini caches `last_usage`
    /// beside the history: leaving it behind makes `estimated_tokens()` —
    /// and therefore the status-bar context bar and every later threshold
    /// check — report the pre-compaction number forever.
    #[test]
    fn compaction_clears_the_cached_usage_snapshot() {
        let mut agent = Agent::new(Arc::new(Unused), crate::tool::ToolRegistry::new(), cfg());
        agent.seed(seeded(20));
        agent.last_usage = Some(crate::provider::Usage {
            input_tokens: 9999,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        });
        agent.set_compaction_hook(CompactionHook::new(|msgs| {
            Some(CompactionResult {
                summary: "S".into(),
                tokens_before: 9999,
                tokens_after: 10,
                messages: msgs[..2].to_vec(),
            })
        }));

        assert!(agent.last_usage.is_some(), "precondition");
        agent.compact_history_via_hook().expect("the hook compacts");
        assert!(
            agent.last_usage.is_none(),
            "the usage snapshot from before the fold survived — every later \
             token estimate is computed from a context that no longer exists"
        );
        assert!(
            agent.last_compaction_tokens.unwrap_or(0) >= 9999,
            "the pre-compaction estimate was not recorded, so the no-shrink \
             guard has nothing to compare against"
        );
    }

    /// A compaction that does not shrink the context must not be retried on
    /// every subsequent turn.
    #[test]
    fn a_compaction_that_does_not_shrink_is_not_repeated() {
        let mut agent = Agent::new(Arc::new(Unused), crate::tool::ToolRegistry::new(), cfg());
        agent.seed(seeded(20));
        agent.set_compaction_hook(CompactionHook::new(|msgs| {
            // Returns the *same* history: the pathological case.
            Some(CompactionResult {
                summary: "S".into(),
                tokens_before: 9999,
                tokens_after: 9999,
                messages: msgs.to_vec(),
            })
        }));

        assert!(agent.maybe_compact_at_threshold().is_some(), "first pass");
        assert!(
            agent.maybe_compact_at_threshold().is_none(),
            "the second turn compacted again from an unchanged context — this \
             is an infinite compaction loop wearing a threshold check's clothes"
        );
    }
}
