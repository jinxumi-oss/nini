#![allow(dead_code)] // Forward-compat fields; unused variants reserved for future error mapping
//! Anthropic Messages API provider.
//!
//! Translates between nini's neutral `Request`/`StreamEvent` types and
//! Anthropic's SSE event protocol. Endpoint: `POST /v1/messages`.

use super::sse::{SseEvent, SseParser};
use async_stream::try_stream;
use futures_util::StreamExt;
use nini_core::provider::*;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

/// Default Anthropic API base URL.
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

/// Anthropic API version sent in the `anthropic-version` header.
pub const API_VERSION: &str = "2023-06-01";

/// Provider implementation for Anthropic Messages.
pub struct AnthropicProvider {
    client: Client,
    base_url: String,
    api_key: String,
}

impl AnthropicProvider {
    /// Create a new Anthropic provider with the given API key and default base URL.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            base_url: DEFAULT_BASE_URL.to_string(),
            api_key: api_key.into(),
        }
    }

    /// Override the base URL (for testing against a proxy).
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Override the HTTP client (for testing).
    pub fn with_client(mut self, client: Client) -> Self {
        self.client = client;
        self
    }
}

impl Provider for AnthropicProvider {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }

    fn stream(
        &self,
        req: Request,
    ) -> Pin<
        Box<dyn futures_core::Stream<Item = Result<StreamEvent, ProviderError>> + Send + 'static>,
    > {
        let client = self.client.clone();
        let base_url = self.base_url.clone();
        let api_key = self.api_key.clone();

        Box::pin(try_stream! {
            // Translate request
            let body = build_request_body(&req)?;
            let url = format!("{base_url}/v1/messages");
            let response = client
                .post(&url)
                .header("x-api-key", &api_key)
                .header("anthropic-version", API_VERSION)
                .header("content-type", "application/json")
                .body(body)
                .send()
                .await?;

            let status = response.status();
            if !status.is_success() {
                Err(ProviderError::Api {
                    status: status.as_u16(),
                    message: format!("request failed: {}", status),
                })?;
            }

            // Parse SSE stream
            let mut byte_stream = response.bytes_stream();
            let mut parser = SseParser::new();
            let mut state = StreamState::default();

            while let Some(chunk_result) = byte_stream.next().await {
                let chunk = chunk_result?;
                let events = parser.feed(&chunk).map_err(|e| ProviderError::Sse(e.to_string()))?;
                for sse_event in events {
                    if let Some(ev) = translate_sse(&sse_event, &mut state) {
                        yield ev;
                    }
                }
            }
            for ev in parser.flush() {
                if let Some(ev) = translate_sse(&ev, &mut state) {
                    yield ev;
                }
            }
        })
    }
}

/// Persistent state needed to translate Anthropic SSE events.
#[derive(Debug, Default)]
struct StreamState {
    /// Currently accumulating tool call: (id, name, input_json_buffer).
    tool_calls: std::collections::HashMap<u32, BuildingToolCall>,
    /// Message id from `message_start`.
    message_id: Option<String>,
    /// Model from `message_start`.
    model: Option<String>,
}

#[derive(Debug, Default)]
struct BuildingToolCall {
    id: String,
    name: String,
    input_json: String,
}

fn build_request_body(req: &Request) -> Result<String, ProviderError> {
    let body = AnthropicRequest::from_neutral(req)?;
    serde_json::to_string(&body).map_err(ProviderError::from)
}

fn translate_sse(ev: &SseEvent, state: &mut StreamState) -> Option<StreamEvent> {
    if ev.is_eof() {
        return None;
    }
    let parsed: AnthropicStreamEvent = serde_json::from_str(&ev.data).ok()?;
    match parsed {
        AnthropicStreamEvent::MessageStart { message } => {
            state.message_id = Some(message.id.clone());
            state.model = Some(message.model.clone());
            Some(StreamEvent::MessageStart {
                id: message.id,
                model: message.model,
            })
        }
        AnthropicStreamEvent::ContentBlockStart {
            index,
            content_block,
        } => {
            if let AnthropicContentBlock::ToolUse { id, name, .. } = content_block {
                state.tool_calls.insert(
                    index,
                    BuildingToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        input_json: String::new(),
                    },
                );
                Some(StreamEvent::ToolCallStart { id, name })
            } else {
                None
            }
        }
        AnthropicStreamEvent::ContentBlockDelta { index, delta } => {
            if let AnthropicDelta::TextDelta { text } = &delta {
                Some(StreamEvent::TextDelta { text: text.clone() })
            } else if let AnthropicDelta::InputJsonDelta { partial_json } = &delta {
                if let Some(call) = state.tool_calls.get_mut(&index) {
                    call.input_json.push_str(partial_json);
                    Some(StreamEvent::ToolCallDelta {
                        id: call.id.clone(),
                        input_json_delta: partial_json.clone(),
                    })
                } else {
                    None
                }
            } else {
                None
            }
        }
        AnthropicStreamEvent::ContentBlockStop { index } => {
            if let Some(call) = state.tool_calls.remove(&index) {
                let input_json: serde_json::Value = if call.input_json.is_empty() {
                    serde_json::Value::Object(serde_json::Map::new())
                } else {
                    serde_json::from_str(&call.input_json).unwrap_or(serde_json::Value::Null)
                };
                Some(StreamEvent::ToolCallStop {
                    id: call.id,
                    input_json,
                })
            } else {
                None
            }
        }
        AnthropicStreamEvent::MessageDelta { delta, usage } => {
            // Final usage often arrives here.
            Some(StreamEvent::MessageStop {
                stop_reason: delta.stop_reason.unwrap_or_else(|| "end_turn".to_string()),
                usage: usage.unwrap_or_default().into(),
            })
        }
        AnthropicStreamEvent::MessageStop => None,
        AnthropicStreamEvent::Error { error } => Some(StreamEvent::Error {
            message: error.message,
        }),
        AnthropicStreamEvent::Ping => None,
    }
}

// --- Anthropic-specific wire types ---

#[derive(Debug, Serialize)]
struct AnthropicRequest<'a> {
    model: &'a str,
    messages: Vec<AnthropicMessage<'a>>,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<&'a str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<AnthropicTool<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    stream: bool,
}

#[derive(Debug, Serialize)]
struct AnthropicMessage<'a> {
    role: &'a str,
    content: Vec<AnthropicContent<'a>>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicContent<'a> {
    Text {
        text: &'a str,
    },
    ToolUse {
        id: &'a str,
        name: &'a str,
        input: &'a serde_json::Value,
    },
    ToolResult {
        tool_use_id: &'a str,
        content: &'a str,
        is_error: bool,
    },
}

#[derive(Debug, Serialize)]
struct AnthropicTool<'a> {
    name: &'a str,
    description: &'a str,
    input_schema: &'a serde_json::Value,
}

impl<'a> AnthropicRequest<'a> {
    fn from_neutral(req: &'a Request) -> Result<Self, ProviderError> {
        let mut messages: Vec<AnthropicMessage> = Vec::new();
        for m in &req.messages {
            let role = match m.role {
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "user", // tool results are merged into user messages in Anthropic API
                Role::System => continue, // system is separate field
            };
            let content: Vec<AnthropicContent> = m
                .content
                .iter()
                .map(|c| match c {
                    ContentBlock::Text { text } => AnthropicContent::Text { text },
                    ContentBlock::ToolUse { id, name, input } => {
                        AnthropicContent::ToolUse { id, name, input }
                    }
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => AnthropicContent::ToolResult {
                        tool_use_id,
                        content,
                        is_error: *is_error,
                    },
                })
                .collect();
            messages.push(AnthropicMessage { role, content });
        }
        let tools: Vec<AnthropicTool> = req
            .tools
            .iter()
            .map(|t| AnthropicTool {
                name: &t.name,
                description: &t.description,
                input_schema: &t.input_schema,
            })
            .collect();
        Ok(Self {
            model: &req.model,
            messages,
            max_tokens: req.max_tokens.unwrap_or(8192),
            system: req.system.as_deref(),
            tools,
            temperature: req.temperature,
            stream: true,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicStreamEvent {
    MessageStart {
        message: AnthropicMessageStart,
    },
    ContentBlockStart {
        index: u32,
        content_block: AnthropicContentBlock,
    },
    ContentBlockDelta {
        index: u32,
        delta: AnthropicDelta,
    },
    ContentBlockStop {
        index: u32,
    },
    MessageDelta {
        delta: AnthropicMessageDelta,
        usage: Option<AnthropicUsage>,
    },
    MessageStop,
    Ping,
    Error {
        error: AnthropicErrorInner,
    },
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageStart {
    id: String,
    model: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicDelta {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageDelta {
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct AnthropicUsage {
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    output_tokens: u32,
    #[serde(default)]
    cache_read_input_tokens: u32,
    #[serde(default)]
    cache_creation_input_tokens: u32,
}

impl From<AnthropicUsage> for Usage {
    fn from(u: AnthropicUsage) -> Self {
        Self {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
            cache_read_tokens: u.cache_read_input_tokens,
            cache_write_tokens: u.cache_creation_input_tokens,
        }
    }
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct AnthropicErrorPayload {
    error: AnthropicErrorInner,
}

#[derive(Debug, Deserialize)]
struct AnthropicErrorInner {
    message: String,
}
