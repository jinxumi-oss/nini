#![allow(dead_code)] // Forward-compat fields for tool call deltas
//! OpenAI Chat Completions provider.
//!
//! Translates between nini's neutral `Request`/`StreamEvent` and OpenAI's
//! streaming chat-completions protocol. Endpoint: `POST /v1/chat/completions`.

use super::sse::{SseEvent, SseParser};
use async_stream::try_stream;
use futures_util::StreamExt;
use nini_core::provider::*;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

/// Default OpenAI API base URL.
pub const DEFAULT_BASE_URL: &str = "https://api.openai.com";

/// OpenAI provider implementation for `/v1/chat/completions`.
pub struct OpenAiProvider {
    client: Client,
    base_url: String,
    api_key: String,
}

impl OpenAiProvider {
    /// Create a new OpenAI provider with the given API key.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            base_url: DEFAULT_BASE_URL.to_string(),
            api_key: api_key.into(),
        }
    }

    /// Override the base URL (for proxies or OpenAI-compat servers).
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Override the HTTP client.
    pub fn with_client(mut self, client: Client) -> Self {
        self.client = client;
        self
    }
}

impl Provider for OpenAiProvider {
    fn name(&self) -> &'static str {
        "openai"
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
            let body = build_request_body(&req)?;
            let url = format!("{base_url}/v1/chat/completions");
            let response = client
                .post(&url)
                .bearer_auth(&api_key)
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

            let mut byte_stream = response.bytes_stream();
            let mut parser = SseParser::new();
            let mut state = StreamState::default();

            while let Some(chunk_result) = byte_stream.next().await {
                let chunk = chunk_result?;
                for sse_event in parser.feed(&chunk).map_err(|e| ProviderError::Sse(e.to_string()))? {
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

#[derive(Debug, Default)]
struct StreamState {
    tool_calls: std::collections::HashMap<u32, BuildingToolCall>,
    finish_reason: Option<String>,
}

#[derive(Debug, Default)]
struct BuildingToolCall {
    id: String,
    name: String,
    input_json: String,
}

fn build_request_body(req: &Request) -> Result<String, ProviderError> {
    let body = OpenAiRequest::from_neutral(req)?;
    serde_json::to_string(&body).map_err(ProviderError::from)
}

fn translate_sse(ev: &SseEvent, state: &mut StreamState) -> Option<StreamEvent> {
    if ev.is_eof() {
        return None;
    }
    let chunk: OpenAiChunk = match serde_json::from_str(&ev.data) {
        Ok(c) => c,
        Err(_) => return None,
    };
    let choice = chunk.choices.into_iter().next();
    match choice {
        None => {
            // Possibly an error payload or usage-only chunk
            if let Some(err) = chunk.error {
                return Some(StreamEvent::Error {
                    message: err.message,
                });
            }
            None
        }
        Some(c) => {
            if let Some(delta) = c.delta {
                if let Some(text) = delta.content {
                    if !text.is_empty() {
                        return Some(StreamEvent::TextDelta { text });
                    }
                }
                for tc in delta.tool_calls.unwrap_or_default() {
                    if let Some(id) = tc.id.clone() {
                        // New tool call starting
                        state.tool_calls.insert(
                            tc.index,
                            BuildingToolCall {
                                id: id.clone(),
                                name: tc.function.name.clone().unwrap_or_default(),
                                input_json: String::new(),
                            },
                        );
                        let name = tc.function.name.clone().unwrap_or_default();
                        return Some(StreamEvent::ToolCallStart { id, name });
                    }
                    // Continuation
                    if let Some(call) = state.tool_calls.get_mut(&tc.index) {
                        if let Some(name) = tc.function.name {
                            call.name = name;
                        }
                        if let Some(args) = tc.function.arguments {
                            call.input_json.push_str(&args);
                            return Some(StreamEvent::ToolCallDelta {
                                id: call.id.clone(),
                                input_json_delta: args,
                            });
                        }
                    }
                }
            }
            if let Some(fr) = c.finish_reason {
                state.finish_reason = Some(fr);
            }
            None
        }
    }
}

/// Final flush: emit any pending tool calls and a stop event with usage.
fn finalize(state: &mut StreamState, usage: Option<OpenAiUsage>) -> Vec<StreamEvent> {
    let mut out = Vec::new();
    let mut to_remove = Vec::new();
    for (idx, call) in state.tool_calls.iter() {
        let input_json: serde_json::Value = if call.input_json.is_empty() {
            serde_json::Value::Object(serde_json::Map::new())
        } else {
            serde_json::from_str(&call.input_json).unwrap_or(serde_json::Value::Null)
        };
        out.push(StreamEvent::ToolCallStop {
            id: call.id.clone(),
            input_json,
        });
        to_remove.push(*idx);
    }
    for idx in to_remove {
        state.tool_calls.remove(&idx);
    }
    let usage = usage.unwrap_or_default().into();
    out.push(StreamEvent::MessageStop {
        stop_reason: state
            .finish_reason
            .take()
            .unwrap_or_else(|| "stop".to_string()),
        usage,
    });
    out
}

// --- Wire ---

#[derive(Debug, Serialize)]
struct OpenAiRequest<'a> {
    model: &'a str,
    messages: Vec<OpenAiMessage<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OpenAiToolRef<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<OpenAiStreamOptions>,
}

#[derive(Debug, Serialize)]
struct OpenAiStreamOptions {
    include_usage: bool,
}

#[derive(Debug, Serialize)]
struct OpenAiMessage<'a> {
    role: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAiToolCallRef<'a>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct OpenAiToolCallRef<'a> {
    id: &'a str,
    #[serde(rename = "type")]
    kind: &'static str,
    function: OpenAiFunctionRef<'a>,
}

#[derive(Debug, Serialize)]
struct OpenAiFunctionRef<'a> {
    name: &'a str,
    arguments: &'a str,
}

#[derive(Debug, Serialize)]
struct OpenAiToolRef<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    function: OpenAiToolFunctionRef<'a>,
}

#[derive(Debug, Serialize)]
struct OpenAiToolFunctionRef<'a> {
    name: &'a str,
    description: &'a str,
    parameters: &'a serde_json::Value,
}

impl<'a> OpenAiRequest<'a> {
    fn from_neutral(req: &'a Request) -> Result<Self, ProviderError> {
        let mut messages: Vec<OpenAiMessage> = Vec::new();
        for m in &req.messages {
            match m.role {
                Role::System => {
                    if let Some(text) = m.content.iter().find_map(|c| match c {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    }) {
                        messages.push(OpenAiMessage {
                            role: "system",
                            content: Some(text),
                            tool_calls: None,
                            tool_call_id: None,
                        });
                    }
                }
                Role::User => {
                    let text: String = m
                        .content
                        .iter()
                        .filter_map(|c| match c {
                            ContentBlock::Text { text } => Some(text.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("");
                    messages.push(OpenAiMessage {
                        role: "user",
                        content: if text.is_empty() {
                            Some("")
                        } else {
                            Some(Box::leak(text.into_boxed_str()))
                        },
                        tool_calls: None,
                        tool_call_id: None,
                    });
                }
                Role::Assistant => {
                    let text: String = m
                        .content
                        .iter()
                        .find_map(|c| match c {
                            ContentBlock::Text { text } => Some(text.clone()),
                            _ => None,
                        })
                        .unwrap_or_default();
                    let tcs: Vec<OpenAiToolCallRef> = m
                        .content
                        .iter()
                        .filter_map(|c| match c {
                            ContentBlock::ToolUse { id, name, input } => {
                                let args = match serde_json::to_string(input) {
                                    Ok(s) => Box::leak(s.into_boxed_str()),
                                    Err(_) => return None,
                                };
                                Some(OpenAiToolCallRef {
                                    id,
                                    kind: "function",
                                    function: OpenAiFunctionRef {
                                        name,
                                        arguments: args,
                                    },
                                })
                            }
                            _ => None,
                        })
                        .collect();
                    messages.push(OpenAiMessage {
                        role: "assistant",
                        content: if text.is_empty() {
                            None
                        } else {
                            Some(Box::leak(text.into_boxed_str()))
                        },
                        tool_calls: if tcs.is_empty() { None } else { Some(tcs) },
                        tool_call_id: None,
                    });
                }
                Role::Tool => {
                    for c in &m.content {
                        if let ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error: _,
                        } = c
                        {
                            let role = "tool";
                            messages.push(OpenAiMessage {
                                role,
                                content: Some(content.as_str()),
                                tool_calls: None,
                                tool_call_id: Some(tool_use_id.as_str()),
                            });
                        }
                    }
                }
            }
        }
        let tools: Vec<OpenAiToolRef> = req
            .tools
            .iter()
            .map(|t| OpenAiToolRef {
                kind: "function",
                function: OpenAiToolFunctionRef {
                    name: &t.name,
                    description: &t.description,
                    parameters: &t.input_schema,
                },
            })
            .collect();
        Ok(Self {
            model: &req.model,
            messages,
            tools,
            max_tokens: req.max_tokens,
            temperature: req.temperature,
            stream: true,
            stream_options: Some(OpenAiStreamOptions {
                include_usage: true,
            }),
        })
    }
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct OpenAiChunk {
    #[serde(default)]
    choices: Vec<OpenAiChoice>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
    error: Option<OpenAiError>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    delta: Option<OpenAiDelta>,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct OpenAiDelta {
    content: Option<String>,
    tool_calls: Option<Vec<OpenAiDeltaToolCall>>,
}

#[derive(Debug, Deserialize, Default)]
struct OpenAiDeltaToolCall {
    index: u32,
    id: Option<String>,
    function: OpenAiDeltaFunction,
}

#[derive(Debug, Default, Deserialize)]
struct OpenAiDeltaFunction {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct OpenAiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
}

impl From<OpenAiUsage> for Usage {
    fn from(u: OpenAiUsage) -> Self {
        Self {
            input_tokens: u.prompt_tokens,
            output_tokens: u.completion_tokens,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        }
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiError {
    message: String,
}
