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

/// How much the model should think before answering.
///
/// v0.10 (R1f). Mirrors Pi's `ModelThinkingLevel`
/// (`pi-ai/src/types.ts:82-83`):
///
/// ```ts
/// export type ThinkingLevel = "minimal" | "low" | "medium" | "high" | "xhigh" | "max";
/// export type ModelThinkingLevel = "off" | ThinkingLevel;
/// ```
///
/// nini's `/thinking` command already accepted exactly this vocabulary
/// (`dispatch.rs`, `["off","minimal","low","medium","high","xhigh","max"]`)
/// and persisted it to settings.json — but `Request` had no field to carry
/// it, so no provider had ever been sent a value. Recorded, displayed,
/// persisted, never transmitted. This enum is the missing link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThinkingLevel {
    /// Do not think. Serialised per family as an explicit off/disable.
    Off,
    Minimal,
    #[default]
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

impl ThinkingLevel {
    /// The wire spelling, matching Pi's `ThinkingLevel` strings.
    pub fn as_str(self) -> &'static str {
        match self {
            ThinkingLevel::Off => "off",
            ThinkingLevel::Minimal => "minimal",
            ThinkingLevel::Low => "low",
            ThinkingLevel::Medium => "medium",
            ThinkingLevel::High => "high",
            ThinkingLevel::XHigh => "xhigh",
            ThinkingLevel::Max => "max",
        }
    }

    /// True when thinking is actually requested.
    pub fn is_on(self) -> bool {
        self != ThinkingLevel::Off
    }

    /// Every accepted spelling, for validation and completion UIs.
    pub const ALL: &'static [&'static str] =
        &["off", "minimal", "low", "medium", "high", "xhigh", "max"];

    /// Token budget for families that express thinking as a budget
    /// (Anthropic's `budget_tokens`) rather than an effort label.
    ///
    /// Pi defaults to 1024 when thinking is enabled and no budget is given
    /// (`anthropic-messages.ts:1067`, `:216`); the level scales that default
    /// so `/thinking max` is visibly different from `/thinking low`.
    pub fn budget_tokens(self) -> u32 {
        match self {
            ThinkingLevel::Off => 0,
            ThinkingLevel::Minimal => 1024,
            ThinkingLevel::Low => 2048,
            ThinkingLevel::Medium => 8192,
            ThinkingLevel::High => 16_384,
            ThinkingLevel::XHigh => 32_768,
            ThinkingLevel::Max => 32_768,
        }
    }
}

impl std::str::FromStr for ThinkingLevel {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "off" => Ok(ThinkingLevel::Off),
            "minimal" => Ok(ThinkingLevel::Minimal),
            "low" => Ok(ThinkingLevel::Low),
            "medium" => Ok(ThinkingLevel::Medium),
            "high" => Ok(ThinkingLevel::High),
            "xhigh" => Ok(ThinkingLevel::XHigh),
            "max" => Ok(ThinkingLevel::Max),
            other => Err(format!(
                "unknown thinking level {other:?}; expected one of {}",
                ThinkingLevel::ALL.join("|")
            )),
        }
    }
}

impl std::fmt::Display for ThinkingLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A request to a model provider.
#[derive(Debug, Clone, Default)]
pub struct Request {
    pub model: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub system: Option<String>,
    /// v0.10 (R1f): how much the model should think.
    ///
    /// `None` means "the caller expressed no opinion" and the family
    /// omits the field entirely — which is what Pi does when `reasoning`
    /// is unset (`anthropic-messages.ts:824-826`).
    pub reasoning: Option<ThinkingLevel>,
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
