//! Agent loop: orchestrate LLM calls + tool execution + event emission.
//!
//! Phase 2 scope:
//! - Single-turn: send messages to provider, stream events, dispatch tool calls
//! - Multi-iteration: re-send to provider with tool results until stop
//! - Abort: cooperative cancellation via `AbortHandle`
//! - Event emission: `AgentEvent` stream for UI consumers

use crate::provider::{Provider, ProviderError, Request, StreamEvent, ToolSpec, Usage};
use crate::tool::{ToolContext, ToolError, ToolOutput, ToolRegistry};
use crate::{AgentMessage, ContentBlock, Role};
use async_stream::try_stream;
use futures_core::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use thiserror::Error;
use tokio::sync::Notify;

/// Maximum tool-iteration count before forcing a stop (matches spec default).
pub const MAX_TOOL_ITERATIONS: usize = 50;

/// Agent event emitted to consumers (UI, logs, etc.).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// Agent started (called once per `run`).
    AgentStart,
    /// A user turn has been appended to the conversation.
    TurnStart,
    /// A turn's response from the model is complete.
    TurnEnd { stop_reason: String, usage: Usage },
    /// Incremental text delta from the model.
    TextDelta { text: String },
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
    /// A tool finished executing.
    ToolResult { id: String, output: ToolOutput },
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
}

impl RunConfig {
    /// Create with sensible defaults.
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            system: None,
            max_tokens: Some(8192),
            temperature: None,
            max_iterations: MAX_TOOL_ITERATIONS,
            tool_context: ToolContext::default(),
            compaction: crate::compaction::CompactionSettings::default(),
        }
    }
}

/// The agent. Owns a provider, a tool registry, and the conversation history.
pub struct Agent {
    provider: Arc<dyn Provider>,
    tools: ToolRegistry,
    messages: Vec<crate::provider::Message>,
    abort: AbortHandle,
    config: RunConfig,
}

impl Agent {
    /// Create a new agent.
    pub fn new(provider: Arc<dyn Provider>, tools: ToolRegistry, config: RunConfig) -> Self {
        Self {
            provider,
            tools,
            messages: Vec::new(),
            abort: AbortHandle::new(),
            config,
        }
    }

    /// Get the abort handle.
    pub fn abort_handle(&self) -> AbortHandle {
        self.abort.clone()
    }

    /// Get the current message history.
    pub fn messages(&self) -> &[crate::provider::Message] {
        &self.messages
    }

    /// Reset the conversation history.
    pub fn clear_history(&mut self) {
        self.messages.clear();
    }

    /// Seed the conversation with existing messages.
    pub fn seed(&mut self, messages: Vec<crate::provider::Message>) {
        self.messages = messages;
    }

    /// Total estimated tokens across all messages currently in history.
    pub fn estimated_tokens(&self) -> u32 {
        crate::compaction::estimate_provider_messages_tokens(&self.messages)
    }

    /// Should the agent compact now? True when estimated tokens exceed
    /// `context_window - reserve_tokens`.
    pub fn should_compact(&self) -> bool {
        crate::compaction::should_compact(self.estimated_tokens(), &self.config.compaction)
    }

    /// Run compaction on the current message history. Returns the
    /// compaction output. v1 uses the deterministic local summary; future
    /// versions pass an LLM-backed `summary_fn`.
    pub fn compact_history<F>(&mut self, summary_fn: F) -> crate::compaction::CompactionOutput
    where
        F: FnOnce(&[crate::provider::Message], Option<String>) -> String,
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
                let keep_from = crate::compaction::find_cut_point(es).keep_from;
                let prefix_len = keep_from.min(msgs_snapshot.len());
                summary_fn(&msgs_snapshot[..prefix_len], prev)
            });

        // Splice: prepend summary message, retain suffix.
        let prefix_len = out.keep_from;
        let new_summary_msg = crate::provider::Message {
            role: crate::provider::Role::User,
            content: vec![crate::provider::ContentBlock::Text {
                text: format!("[CONTEXT SUMMARY]\n\n{}", out.summary),
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
        let config = self.config.clone();
        let abort = self.abort.clone();

        Box::pin(try_stream! {
            yield AgentEvent::AgentStart;
            yield AgentEvent::TurnStart;

            let mut iteration = 0;
            loop {
                if abort.is_aborted() {
                    yield AgentEvent::Aborted;
                    return;
                }
                iteration += 1;
                if iteration > config.max_iterations {
                    Err(AgentError::TooManyIterations(config.max_iterations))?;
                }

                // Auto-compact: if context window is exceeded, summarize the
                // older prefix of history and prepend a single summary
                // message. v1 uses the deterministic local summary; LLM
                // summarization lands when a provider is wired in.
                if crate::compaction::should_compact(
                    crate::compaction::estimate_provider_messages_tokens(&self.messages),
                    &config.compaction,
                ) {
                    let _out = self.compact_history(|msgs, _prev| {
                        // Local summary: rebuild entries from messages and
                        // delegate to the existing helper.
                        let entries: Vec<crate::Entry> = msgs
                            .iter()
                            .enumerate()
                            .map(|(i, m)| crate::Entry::message(
                                format!("cmsg_{i}"),
                                None,
                                (i as u64) + 1,
                                AgentMessage {
                                    role: m.role,
                                    content: m.content.clone(),
                                    timestamp: 0,
                                },
                            ))
                            .collect();
                        crate::compaction::generate_local_summary(&entries)
                    });
                    yield AgentEvent::Error { message: format!("compaction: tokens before={} after={}", _out.tokens_before, _out.tokens_after) };
                }

                // Build request from current history.
                let request = build_request(&config, &self.messages, tool_specs.as_slice());

                // Stream provider response, accumulating into assistant message
                // and tracking pending tool calls.
                let mut assistant_text = String::new();
                let mut tool_calls: Vec<PendingToolCall> = Vec::new();
                let mut current_call: Option<PendingToolCall> = None;
                let mut last_usage = Usage::default();
                let mut stop_reason = "end_turn".to_string();

                {
                    use futures_util::StreamExt;
                    let mut stream = Box::pin(provider.stream(request));
                    loop {
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
                            Some(Ok(StreamEvent::ToolCallStart { id, name })) => {
                                if let Some(c) = current_call.take() {
                                    tool_calls.push(c);
                                }
                                current_call = Some(PendingToolCall {
                                    id: id.clone(),
                                    name: name.clone(),
                                    input_json: String::new(),
                                });
                                yield AgentEvent::ToolCallStart { id, name };
                            }
                            Some(Ok(StreamEvent::ToolCallDelta { id, input_json_delta })) => {
                                if let Some(c) = current_call.as_mut() {
                                    if c.id == id {
                                        c.input_json.push_str(&input_json_delta);
                                        yield AgentEvent::ToolCallDelta { id, input_json_delta };
                                    }
                                }
                            }
                            Some(Ok(StreamEvent::ToolCallStop { id, input_json })) => {
                                if let Some(mut c) = current_call.take() {
                                    if c.id == id {
                                        c.input_json = serde_json::to_string(&input_json)
                                            .unwrap_or_else(|_| "null".to_string());
                                        tool_calls.push(c);
                                    }
                                }
                                yield AgentEvent::ToolCallStop { id, input_json };
                            }
                            Some(Ok(StreamEvent::MessageStop { stop_reason: sr, usage })) => {
                                stop_reason = sr;
                                last_usage = usage;
                            }
                            Some(Ok(StreamEvent::Error { message })) => {
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
                }
                if let Some(c) = current_call.take() {
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
                self.messages.push(crate::provider::Message {
                    role: Role::Assistant,
                    content: assistant_content,
                });

                yield AgentEvent::TurnEnd { stop_reason: stop_reason.clone(), usage: last_usage };

                // If no tool calls, we're done.
                if tool_calls.is_empty() {
                    return;
                }

                // Execute each tool call, accumulate results, then continue loop.
                let mut tool_results: Vec<ContentBlock> = Vec::new();
                for tc in &tool_calls {
                    if abort.is_aborted() {
                        yield AgentEvent::Aborted;
                        return;
                    }
                    let tool = tool_registry.get(&tc.name);
                    let output = match tool {
                        Some(t) => {
                            let args: serde_json::Value = serde_json::from_str(&tc.input_json)
                                .unwrap_or(serde_json::Value::Null);
                            match t.execute(args, config.tool_context.clone()).await {
                                Ok(out) => out,
                                Err(e) => ToolOutput::err(e.to_string()),
                            }
                        }
                        None => ToolOutput::err(format!("tool not found: {}", tc.name)),
                    };
                    yield AgentEvent::ToolResult { id: tc.id.clone(), output: output.clone() };
                    tool_results.push(ContentBlock::ToolResult {
                        tool_use_id: tc.id.clone(),
                        content: output.content,
                        is_error: output.is_error,
                    });
                }
                self.messages.push(crate::provider::Message { role: Role::Tool, content: tool_results });
                // Loop again: model will see tool results.
            }
        })
    }
}

#[derive(Debug, Clone)]
struct PendingToolCall {
    id: String,
    name: String,
    input_json: String,
}

fn build_request(
    config: &RunConfig,
    messages: &[crate::provider::Message],
    tools: &[ToolSpec],
) -> Request {
    Request {
        model: config.model.clone(),
        messages: messages.to_vec(),
        tools: tools.to_vec(),
        max_tokens: config.max_tokens,
        temperature: config.temperature,
        system: config.system.clone(),
    }
}

/// Convert an `nini_core::AgentMessage` (which has `timestamp`) to the
/// provider-layer `crate::provider::Message` (which doesn't).
fn to_nini_message(m: &AgentMessage) -> crate::provider::Message {
    // Reuse the agent-core role directly — it's already in the provider's
    // canonical shape (User/Assistant/Tool/System).
    crate::provider::Message {
        role: m.role,
        content: m.content.clone(),
    }
}
