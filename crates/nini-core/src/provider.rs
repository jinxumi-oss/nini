//! Shared provider types and the `Provider` trait.
//!
//! Provider implementations (Anthropic, OpenAI, etc.) live in sibling modules.

use async_stream::try_stream;
use futures_core::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use thiserror::Error;

/// Role of a message in a conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// A block of content within a message.
///
/// Phase 2 supports text and tool-call blocks. Image blocks arrive in Phase 3.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Plain text content.
    Text { text: String },
    /// Tool use request (assistant message).
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Tool result (tool message).
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
}

/// A message in a conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
    /// Timestamp (ms since epoch). 0 means unset.
    #[serde(default)]
    pub timestamp: i64,
}

/// Legacy alias — many places use `AgentMessage` as the type name.
/// This is the same as `Message`.

/// Tool specification sent to the model. Re-exported from `tool::ToolSpec`.
pub use crate::tool::ToolSpec;

/// Tool call emitted by the model during streaming.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Partial or complete JSON input. Providers typically emit `PartialJson`
    /// deltas that the client accumulates into a final `Value`.
    pub input_json: String,
}

/// Result of executing a tool (sent back in the next request).
pub type ToolResult = ContentBlock;

/// Token usage reported by the model at end of turn.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    #[serde(default)]
    pub cache_read_tokens: u32,
    #[serde(default)]
    pub cache_write_tokens: u32,
}

/// Compute context-window token count from a provider-reported Usage.
/// Mirrors Pi's `calculateContextTokens`: sum of input, output, cache
/// reads, and cache writes (the last is Pi's behavior; we approximate).
pub fn context_tokens_from_usage(usage: &Usage) -> u32 {
    usage
        .input_tokens
        .saturating_add(usage.output_tokens)
        .saturating_add(usage.cache_read_tokens)
        .saturating_add(usage.cache_write_tokens)
}

/// A single streaming event from a provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    /// Stream opened (provider sent message_start equivalent).
    MessageStart { id: String, model: String },
    /// A chunk of text content arrived.
    TextDelta { text: String },
    /// v0.8: model emitted reasoning content (inside `<think>...</think>`).
    /// Renders with dim/italic style in the TUI (Pi-style).
    ThinkingDelta { text: String },
    /// Model invoked a tool (JSON input may still be partial).
    ToolCallStart { id: String, name: String },
    ToolCallDelta {
        id: String,
        input_json_delta: String,
    },
    ToolCallStop {
        id: String,
        input_json: serde_json::Value,
    },
    /// Stream finished (provider sent message_stop equivalent).
    MessageStop { stop_reason: String, usage: Usage },
    /// Provider emitted an error event (mid-stream).
    Error { message: String },
}

/// A request to a model provider.
#[derive(Debug, Clone)]
pub struct Request {
    pub model: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub system: Option<String>,
}

/// Provider capability flags. Pi today supports all of these via dispatch;
/// nini v1 implements the streaming subset.
#[derive(Debug, Clone, Copy)]
pub struct Capabilities {
    pub streaming: bool,
    pub tool_use: bool,
    pub vision: bool,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            streaming: true,
            tool_use: true,
            vision: false,
        }
    }
}

/// The `Provider` trait: turn a request into a stream of [`StreamEvent`]s.
///
/// Implementors handle request shaping, authentication, and SSE parsing.
/// All Pi-compatible providers must produce the same event semantics.
pub trait Provider: Send + Sync {
    /// Provider name (e.g., `"anthropic"`, `"openai"`).
    fn name(&self) -> &'static str;
    /// Capabilities this provider supports.
    fn capabilities(&self) -> Capabilities;
    /// Send a request and return a stream of events.
    fn stream(
        &self,
        req: Request,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send + 'static>>;
}

/// Helper to wrap an iterator of events as a stream (used by fixture provider).
pub fn events_from_iter<I>(
    iter: I,
) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send + 'static>>
where
    I: IntoIterator<Item = StreamEvent> + 'static,
    I::IntoIter: Send + 'static,
{
    let mut iter = iter.into_iter();
    Box::pin(try_stream! {
        for ev in iter.by_ref() {
            yield ev;
        }
    })
}

/// Provider error type.
#[derive(Debug, Error, Clone)]
pub enum ProviderError {
    #[error("http error: {0}")]
    Http(String),
    #[error("sse error: {0}")]
    Sse(String),
    #[error("api error ({status}): {message}")]
    Api { status: u16, message: String },
    #[error("auth error: {0}")]
    Auth(String),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("json error: {0}")]
    Json(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("not implemented: {0}")]
    NotImplemented(String),
}

impl From<reqwest::Error> for ProviderError {
    fn from(e: reqwest::Error) -> Self {
        ProviderError::Http(e.to_string())
    }
}

impl From<serde_json::Error> for ProviderError {
    fn from(e: serde_json::Error) -> Self {
        ProviderError::Json(e.to_string())
    }
}

/// Convenience alias for `Message` — many existing call sites use `AgentMessage`.
pub type AgentMessage = Message;
