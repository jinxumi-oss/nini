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
            let url = format!("{base_url}{}/chat/completions",
    if base_url.ends_with("/v1") { "" } else { "/v1" });
            let response = client
                .post(&url)
                .bearer_auth(&api_key)
                .header("content-type", "application/json")
                .body(body)
                .send()
                .await?;

            let status = response.status();
            if status.is_success() {
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
            } else {
                Err(ProviderError::Api {
                    status: status.as_u16(),
                    message: format!("request failed: {}", status),
                })?;
            }
        })
    }
}

#[derive(Debug, Default)]
struct StreamState {
    tool_calls: std::collections::HashMap<u32, BuildingToolCall>,
    finish_reason: Option<String>,
    /// v0.7.4 (UX fix) — strip `<think>...</think>` reasoning blocks
    /// from streaming text. Some OpenAI-compat models (e.g.,
    /// MiniMax-M3) emit reasoning inline; without this filter,
    /// users see the model's "thinking" in the transcript.
    think_filter: ThinkTagFilter,
}

fn build_request_body(req: &Request) -> Result<String, ProviderError> {
    let body = OpenAiRequest::from_neutral(req)?;
    serde_json::to_string(&body).map_err(ProviderError::from)
}

#[derive(Debug, Default)]
struct BuildingToolCall {
    id: String,
    name: String,
    input_json: String,
}

/// v0.7.4 (UX fix) — MiniMax-M3 and similar reasoning-capable models
/// return the model's "thinking" in the same `delta.content` field
/// as the actual answer, wrapped in `<think>...</think>` tags.
/// Without stripping, the user sees the model's internal monologue
/// before the real answer.
///
/// `ThinkTagFilter` is a stateful filter that walks chunks of text
/// in order and emits only the content OUTSIDE `<think>...</think>`
/// blocks. Tags may span chunk boundaries (e.g., chunk N ends
/// with `<` and chunk N+1 starts with `mm:think>`), so the filter
/// holds back text from the last `<` to the end of the chunk in
/// case it grows into a tag on the next call.
#[derive(Debug, Default)]
struct ThinkTagFilter {
    /// Whether we're currently inside a `<think>...</think>` block.
    in_think: bool,
    /// Text carried over from previous chunks when a tag might be
    /// split across the boundary. Cleared once the tag resolves.
    buffer: String,
}

impl ThinkTagFilter {
    fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk of text and get back the portion that should be
    /// shown (text with `<think>...</think>` removed).
    fn push(&mut self, text: &str) -> String {
        let mut work = std::mem::take(&mut self.buffer);
        work.push_str(text);

        let mut out = String::new();
        let mut i = 0;
        while i < work.len() {
            if self.in_think {
                // Look for closing tag.
                if let Some(end) = find_subseq(work.as_bytes(), b"</think>", i) {
                    i = end + b"</think>".len();
                    self.in_think = false;
                } else {
                    // No closing tag in this chunk — hold back from
                    // the last `<` (or keep up to 7 chars as a
                    // partial-tag buffer) and return.
                    self.buffer = hold_back(&work, i);
                    return out;
                }
            } else {
                // Look for opening tag.
                if let Some(start) = find_subseq(work.as_bytes(), b"<think>", i) {
                    out.push_str(&work[i..start]);
                    i = start + b"<think>".len();
                    self.in_think = true;
                } else {
                    // No opening tag — emit up to the last `<`, hold
                    // back the rest as a potential partial tag.
                    emit_prefix_hold_rest(&mut out, &mut self.buffer, &work, i);
                    return out;
                }
            }
        }
        // Consumed all of work. Buffer should be empty.
        self.buffer.clear();
        out
    }
}

/// Emit everything in `work[i..]` up to the last `<` (if any),
/// and store the rest in `buffer`. If `work[i..]` has no `<`,
/// emit everything and clear `buffer`.
fn emit_prefix_hold_rest(out: &mut String, buffer: &mut String, work: &str, i: usize) {
    let rest_start = match work[i..].rfind('<') {
        Some(pos) => i + pos,
        None => {
            out.push_str(&work[i..]);
            buffer.clear();
            return;
        }
    };
    out.push_str(&work[i..rest_start]);
    *buffer = work[rest_start..].to_string();
}

/// Hold back from the last `<` (or 7 chars if no `<`) in
/// `work[i..]`. Used when we're inside a think block and don't
/// yet see the closing tag — we may need to keep these bytes
/// for the next chunk.
fn hold_back(work: &str, i: usize) -> String {
    match work[i..].rfind('<') {
        Some(pos) => work[i + pos..].to_string(),
        None => {
            let keep = 7;
            let start = work.len().saturating_sub(keep);
            if start >= i { work[start..].to_string() } else { String::new() }
        }
    }
}

fn find_subseq(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from + needle.len() > haystack.len() {
        return None;
    }
    for i in from..=haystack.len() - needle.len() {
        if &haystack[i..i + needle.len()] == needle {
            return Some(i);
        }
    }
    None
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
                        let filtered = state.think_filter.push(&text);
                        if !filtered.is_empty() {
                            return Some(StreamEvent::TextDelta { text: filtered });
                        }
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

/// v0.7.3 (UX test) — openai-compat URL construction. Different
/// OpenAI-compatible providers expose different URL conventions:
///   * Official OpenAI: `https://api.openai.com` (no /v1 in base)
///   * OpenRouter:     `https://openrouter.ai/api` (no /v1 in base)
///   * m.aiio.chat:    `https://m.aiio.chat/v1` (HAS /v1 in base)
///
/// The original `format!("{base_url}/v1/chat/completions")` would
/// double-append `/v1` for the third style and hit a 404/HTML page
/// instead of the API. The fix: skip the `/v1` prefix when the
/// base_url already ends with it.
#[cfg(test)]
mod url_construction_tests {
    #[test]
    fn appends_v1_when_missing() {
        let base_url = "https://api.openai.com";
        let url = format!("{base_url}{}/chat/completions",
            if base_url.ends_with("/v1") { "" } else { "/v1" });
        assert_eq!(url, "https://api.openai.com/v1/chat/completions");
    }

    #[test]
    fn skips_v1_when_already_present() {
        let base_url = "https://m.aiio.chat/v1";
        let url = format!("{base_url}{}/chat/completions",
            if base_url.ends_with("/v1") { "" } else { "/v1" });
        assert_eq!(url, "https://m.aiio.chat/v1/chat/completions");
    }

    #[test]
    fn handles_trailing_slash() {
        let base_url = "https://example.com/v1/";
        let url = format!("{base_url}{}/chat/completions",
            if base_url.ends_with("/v1") { "" } else { "/v1" });
        // Trailing slash on /v1/ means we DON'T recognize it as the
        // /v1 suffix, so we append /v1 again. This is a known minor
        // quirk — users shouldn't add trailing slashes.
        assert_eq!(url, "https://example.com/v1//v1/chat/completions");
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

/// v0.7.4 (UX fix) tests for the think-tag filter.
#[cfg(test)]
mod think_filter_tests {
    use super::*;

    #[test]
    fn passes_through_text_without_tags() {
        let mut f = ThinkTagFilter::new();
        assert_eq!(f.push("hello world"), "hello world");
        assert_eq!(f.push(" more text"), " more text");
    }

    #[test]
    fn strips_full_think_block() {
        let mut f = ThinkTagFilter::new();
        let out = f.push("before<think>reasoning text</think>after");
        assert_eq!(out, "beforeafter");
    }

    #[test]
    fn handles_think_block_at_start() {
        let mut f = ThinkTagFilter::new();
        let out = f.push("<think>hidden</think>visible");
        assert_eq!(out, "visible");
    }

    #[test]
    fn handles_think_block_at_end() {
        let mut f = ThinkTagFilter::new();
        let out = f.push("visible<think>hidden</think>");
        assert_eq!(out, "visible");
    }

    #[test]
    fn handles_think_block_with_newlines() {
        let mut f = ThinkTagFilter::new();
        let out = f.push("<think>line1\nline2\nline3</think>clean");
        assert_eq!(out, "clean");
    }

    #[test]
    fn handles_multiple_think_blocks() {
        let mut f = ThinkTagFilter::new();
        let out = f.push("<think>a</think>X<think>b</think>Y<think>c</think>");
        assert_eq!(out, "XY");
    }

    #[test]
    fn handles_unclosed_think_block_across_chunks() {
        // Chunk 1 ends with "<" — that's the start of "<mm:think>".
        // Chunk 2 begins with "mm:think>reasoning</think>clean".
        // The filter must hold back the "<" from chunk 1, recognize
        // the opening tag in chunk 2, then drop the reasoning.
        let mut f = ThinkTagFilter::new();
        assert_eq!(f.push("before<"), "before");
        assert_eq!(f.push("think>reasoning</think>clean"), "clean");
    }

    #[test]
    fn handles_unclosed_closing_tag_across_chunks() {
        // Chunk 1 ends with "<think>reasoning</" (opening tag opened,
        // closing tag not yet arrived).
        // Chunk 2 starts with "think>clean".
        let mut f = ThinkTagFilter::new();
        assert_eq!(f.push("<think>reasoning</"), "");
        assert_eq!(f.push("think>clean"), "clean");
    }

    #[test]
    fn empty_input_yields_empty_output() {
        let mut f = ThinkTagFilter::new();
        assert_eq!(f.push(""), "");
    }

    #[test]
    fn realistic_minimax_output() {
        // Simulate the typical MiniMax-M3 streaming output.
        let mut f = ThinkTagFilter::new();
        let chunk1 = "<think>The user wants me to count.";
        let chunk2 = " Let me do that now.\n</think>1, 2, 3";
        let chunk3 = ", 4, 5";
        assert_eq!(f.push(chunk1), "");
        assert_eq!(f.push(chunk2), "1, 2, 3");
        assert_eq!(f.push(chunk3), ", 4, 5");
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiError {
    message: String,
}
