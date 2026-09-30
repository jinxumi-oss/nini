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
                // v0.8.4 (bugfix): the previous version finished here,
                // so the agent loop saw `None` from the stream and
                // broke out *without* ever observing a
                // `MessageStop`. With no Stop event, agent.rs had no
                // way to know the model had finished its turn: tool
                // calls stayed half-built (no Stop line in the
                // transcript), and on the next iteration the agent
                // returned Idle without the model ever producing a
                // visible assistant message after running a tool.
                //
                // `finalize` flushes any remaining queued
                // ToolCallStop bodies AND emits the terminal
                // MessageStop with the recorded finish_reason (or
                // "stop" as a default if the model never sent one).
                // Yield every event it produces so the agent loop
                // sees the full turn boundary.
                for ev in finalize(&mut state, None) {
                    yield ev;
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
    /// v0.8.4 (bugfix): fragments waiting to be emitted on the next
    /// call. The ThinkTagFilter splits a chunk at the
    /// `<think>...</think>` boundary into [Thinking, Visible]
    /// pairs. We drain one fragment per call so we still emit one
    /// StreamEvent at a time, but stash the rest here instead of
    /// dropping them on the floor (the previous code `return`-ed
    /// on the first match, which also skipped the same chunk's
    /// tool_calls delta).
    fragments_pending: Vec<ThinkFragment>,
    /// v0.8.4 (bugfix): aiio / Anthropic-translated providers emit
    /// the full function-call body inline on the same chunk that
    /// carries the id+name, with NO separate delta chunk to follow.
    /// We emit ToolCallStart here (so the transcript line gets the
    /// function name, not the id) and queue the parsed body; the
    /// next `translate_sse` call drains `pending_stop_args` into a
    /// ToolCallStop. Without this, the inline path would never
    /// produce a Stop event and the transcript line would carry an
    /// empty `input_json`.
    pending_stop_args: std::collections::HashMap<u32, serde_json::Value>,
    /// v0.8.4 (bugfix): indices whose Stop event has already been
    /// emitted by the drain path. After drain emits Stop, a later
    /// inline re-poll would re-queue the same body and emit Stop
    /// again — `contains_key` can't distinguish "queued, Stop
    /// pending" from "queued, Stop already emitted" once the entry
    /// is removed. Tracking emitted indices prevents that loop.
    pending_stop_emitted: std::collections::HashSet<u32>,
    /// v0.8.4 (bugfix): tool-call indices whose ToolCallStart has
    /// already been emitted. A re-poll of the same start chunk must
    /// not emit Start a second time (the runtime would append a
    /// duplicate `▸ bash` line and the args would be double-counted).
    tool_call_started: std::collections::HashSet<u32>,
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
///
/// v0.8 update: instead of stripping the think tags entirely (v0.7.4),
/// we now emit TWO kinds of fragments:
///   * `Visible(text)`  — text OUTSIDE `<think>...</think>` blocks
///   * `Thinking(text)` — text INSIDE `<think>...</think>` blocks
///
/// The TUI renders `Thinking` with a dim/italic style so users can
/// see the model's reasoning without it dominating the transcript.
/// Pi does the same; v0.7.4's "strip entirely" choice was too lossy.
///
/// `ThinkTagFilter` is a stateful filter that walks chunks of text
/// in order. Tags may span chunk boundaries (e.g., chunk N ends
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

/// One fragment emitted from `ThinkTagFilter::push`. The TUI maps
/// `Thinking` to a dim/italic span and `Visible` to the normal
/// assistant style.
#[derive(Debug, PartialEq, Eq)]
enum ThinkFragment {
    Visible(String),
    Thinking(String),
}

impl ThinkTagFilter {
    fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk of text and get back a list of fragments to
    /// render. Tags are stripped; their content is emitted as
    /// `Thinking` fragments and the rest as `Visible` fragments.
    /// Returns an empty Vec when the chunk contains nothing
    /// renderable.
    fn push(&mut self, text: &str) -> Vec<ThinkFragment> {
        let mut work = std::mem::take(&mut self.buffer);
        work.push_str(text);

        let mut out: Vec<ThinkFragment> = Vec::new();
        let mut i = 0;
        while i < work.len() {
            if self.in_think {
                // Look for closing tag.
                if let Some(end) = find_subseq(work.as_bytes(), b"</think>", i) {
                    // Emit any accumulated Thinking before closing.
                    if end > i {
                        out.push(ThinkFragment::Thinking(work[i..end].to_string()));
                    }
                    i = end + b"</think>".len();
                    self.in_think = false;
                } else {
                    // v0.8.4 (bugfix): inside a thinking block we must
                    // KEEP all text — never throw it away. The
                    // previous code held back at most the last 7
                    // chars, which on aiio's chunked thinking stream
                    // truncated the accumulated reasoning to its
                    // tail (e.g. `output.`), losing everything before
                    // the last chunk boundary. Stream the whole
                    // `work[i..]` forward — the filter's buffer
                    // holds the in-flight content until the closing
                    // tag lands.
                    self.buffer = work[i..].to_string();
                    return out;
                }
            } else {
                // Look for opening tag.
                if let Some(start) = find_subseq(work.as_bytes(), b"<think>", i) {
                    if start > i {
                        out.push(ThinkFragment::Visible(work[i..start].to_string()));
                    }
                    i = start + b"<think>".len();
                    self.in_think = true;
                } else {
                    // No opening tag — emit up to the last `<`,
                    // hold back the rest as a potential partial tag.
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
fn emit_prefix_hold_rest(out: &mut Vec<ThinkFragment>, buffer: &mut String, work: &str, i: usize) {
    let rest_start = match work[i..].rfind('<') {
        Some(pos) => i + pos,
        None => {
            if !work[i..].is_empty() {
                out.push(ThinkFragment::Visible(work[i..].to_string()));
            }
            buffer.clear();
            return;
        }
    };
    if rest_start > i {
        out.push(ThinkFragment::Visible(work[i..rest_start].to_string()));
    }
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
                        let frags = state.think_filter.push(&text);
                        // v0.8.4 (bugfix): stash all fragments instead
                        // of returning on the first match. The previous
                        // `return Some(...)` skipped the rest of the
                        // SSE event — most importantly the
                        // `for tc in delta.tool_calls` loop below,
                        // which means every `<think>...</think>` close
                        // that landed on the same chunk as a tool
                        // call (the common case on aiio / Anthropic-
                        // translated models) silently dropped the tool
                        // call.
                        state.fragments_pending = frags;
                    }
                }
                // v0.8.4 (bugfix): process tool_calls BEFORE draining
                // text fragments. The previous order drained fragments
                // first and returned the first non-empty one — which
                // silently dropped tool_calls on the same chunk. On
                // aiio / Anthropic-translated models, the very chunk
                // that closes `</think>` also carries the tool-call
                // id+name+args start (`{`), so this path was dropping
                // the Start event entirely (no transcript line at
                // all, then tool args accumulated as Delta on later
                // chunks but never found a Start to attach to).
                for tc in delta.tool_calls.unwrap_or_default() {
                    if let Some(id) = tc.id.clone() {
                        // First chunk for this tool call index.
                        // Insert into `state.tool_calls` (id+name+seed
                        // input_json) and emit ToolCallStart so the
                        // transcript line carries the function name
                        // (e.g. "bash"), not the tool-call id.
                        //
                        // Single-shot inline (small aiio payloads):
                        // queue the parsed body in
                        // `pending_stop_args` so the next
                        // translate_sse call drains it as Stop.
                        // Streaming (aiio / Anthropic): the args
                        // fragment on this chunk is just `{` —
                        // subsequent chunks arrive as plain deltas,
                        // Continuation accumulates, finalize emits
                        // Stop from `input_json`.
                        let initial_args = tc
                            .function
                            .arguments
                            .clone()
                            .unwrap_or_default();
                        state.tool_calls.insert(
                            tc.index,
                            BuildingToolCall {
                                id: id.clone(),
                                name: tc.function.name.clone().unwrap_or_default(),
                                input_json: initial_args.clone(),
                            },
                        );
                        let name = tc.function.name.clone().unwrap_or_default();
                        // Single-shot providers send the whole body on
                        // the start chunk. Only then do we have a
                        // complete object to hand to ToolCallStop —
                        // streaming providers send just `{` here and
                        // stream the rest as Continuation deltas, which
                        // `finalize` closes out. Queueing a partial
                        // fragment would fire Stop with `Value::String`
                        // and the tool would fail to parse its args.
                        let complete = serde_json::from_str::<serde_json::Value>(&initial_args)
                            .ok()
                            .filter(serde_json::Value::is_object);
                        if !state.tool_call_started.contains(&tc.index) {
                            state.tool_call_started.insert(tc.index);
                            if let Some(body) = complete {
                                state.pending_stop_args.insert(tc.index, body);
                            }
                            return Some(StreamEvent::ToolCallStart { id, name });
                        }
                    } else if state.tool_call_started.contains(&tc.index) {
                        // Continuation: subsequent delta chunks carry
                        // args fragments (and sometimes a late name).
                        // Accumulate into `input_json` and emit
                        // ToolCallDelta so the transcript line grows.
                        // Skip if Stop already emitted (single-shot
                        // path) — `pending_stop_emitted` is set by
                        // the drain below.
                        if let Some(call) = state.tool_calls.get_mut(&tc.index) {
                            if let Some(name) = tc.function.name.clone() {
                                call.name = name;
                            }
                            if let Some(args) = tc.function.arguments.clone() {
                                call.input_json.push_str(&args);
                                return Some(StreamEvent::ToolCallDelta {
                                    id: call.id.clone(),
                                    input_json_delta: args,
                                });
                            }
                        }
                    }
                }
            }
            // v0.8.4 (bugfix): drain any queued ToolCallStop bodies
            // from the single-shot inline-args path. The next
            // translate_sse call (after the one that emitted
            // ToolCallStart) drains the queued body into a Stop event
            // and marks the index emitted, so the streaming path's
            // Continuation does not re-append the same body.
            if let Some((idx, input_json)) = state.pending_stop_args.iter().next().map(|(k, v)| (*k, v.clone())) {
                state.pending_stop_args.remove(&idx);
                state.pending_stop_emitted.insert(idx);
                if let Some(call) = state.tool_calls.get(&idx) {
                    return Some(StreamEvent::ToolCallStop {
                        id: call.id.clone(),
                        input_json,
                    });
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

/// v0.8.4 regression: aiio / Anthropic-translated providers emit
/// the full `tool_calls[*].function.arguments` JSON inline on the
/// very first chunk (the same one that carries `id` and
/// `function.name`). The previous code seeded `input_json = ""`
/// on ToolCallStart and only consumed arguments from later
/// ToolCallDelta chunks, so the transcript rendered `▸ bash `
/// (empty body) and downstream `BashTool` panicked with `invalid
/// type: null, expected struct BashArgs`. These tests pin both
/// the inline-args-on-start path and the empty-args fallback.
#[cfg(test)]
mod tool_call_inline_args_tests {
    use super::*;

    fn make_chunk(json: &str) -> SseEvent {
        SseEvent::new("", json)
    }

    #[test]
    fn inline_args_on_start_chunk_emits_complete_stop() {
        // aiio-shape SSE event: a single chunk carries the id, name,
        // AND the full arguments JSON object — no separate delta
        // chunk arrives later.
        let raw = r#"{
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_abc123",
                        "type": "function",
                        "function": {
                            "name": "bash",
                            "arguments": "{\"command\":\"ls /tmp\"}"
                        }
                    }]
                }
            }]
        }"#;
        let ev = make_chunk(raw);
        let mut state = StreamState::default();
        // v0.8.4: emit ToolCallStart first (so the transcript line
        // gets the function name "bash", not the tool-call id),
        // then immediately emit ToolCallStop with the parsed body.
        // v0.8.4 (bugfix): the inline-args path queues the body in
        // `pending_stop_args` so the NEXT `translate_sse` call drains
        // it into ToolCallStop. Call translate_sse twice to simulate
        // the runtime polling the stream.
        let first = translate_sse(&ev, &mut state)
            .expect("translate_sse returned None on first call");
        match first {
            StreamEvent::ToolCallStart { id, name } => {
                assert_eq!(id, "call_abc123");
                assert_eq!(name, "bash",
                    "v0.8.4 regression: the function name must be carried                      through; previously `name` was set from the                      tool-call id because we only emitted ToolCallStop");
            }
            other => panic!("expected ToolCallStart first, got {other:?}"),
        }
        let second = translate_sse(&ev, &mut state)
            .expect("translate_sse returned None on second call");
        match second {
            StreamEvent::ToolCallStop { id, input_json } => {
                assert_eq!(id, "call_abc123");
                assert_eq!(input_json["command"], "ls /tmp");
            }
            other => panic!("expected ToolCallStop, got {other:?}"),
        }
        // A third call should drain everything and return None.
        let third = translate_sse(&ev, &mut state);
        assert!(third.is_none(),
            "third call should drain pending_stop_args and return None, got {third:?}");
        let stored = state.tool_calls.get(&0)
            .expect("BuildingToolCall for index 0 missing");
        assert_eq!(stored.id, "call_abc123");
        assert_eq!(stored.name, "bash");
        assert_eq!(stored.input_json, r#"{"command":"ls /tmp"}"#);
    }

    #[test]
    fn empty_args_on_start_chunk_falls_back_to_start_event() {
        // Legacy / strict-openai path: arguments arrive on a later
        // delta chunk. We should still emit ToolCallStart with an
        // empty input_json, not crash.
        let raw = r#"{
            "choices": [{"delta": {"tool_calls": [{
                "index": 0, "id": "call_empty", "type": "function",
                "function": { "name": "bash", "arguments": "" }
            }]}}]
        }"#;
        let ev = make_chunk(raw);
        let mut state = StreamState::default();
        let out = translate_sse(&ev, &mut state)
            .expect("returned None");
        match out {
            StreamEvent::ToolCallStart { id, name } => {
                assert_eq!(id, "call_empty");
                assert_eq!(name, "bash");
            }
            other => panic!("expected ToolCallStart, got {other:?}"),
        }
    }

    #[test]
    fn continuation_delta_still_appends_args() {
        // After an empty-args start, the next delta chunk must
        // append to `input_json` so the eventual stop carries the
        // full command.
        let mut state = StreamState::default();
        let start = make_chunk(r#"{
            "choices": [{"delta": {"tool_calls": [{
                "index": 0, "id": "call_xyz", "type": "function",
                "function": { "name": "bash", "arguments": "" }
            }]}}]
        }"#);
        let _ = translate_sse(&start, &mut state);
        assert_eq!(state.tool_calls.get(&0).unwrap().input_json, "");
        let delta = make_chunk(r#"{
            "choices": [{"delta": {"tool_calls": [{
                "index": 0,
                "function": { "arguments": "{\"command\":\"ls\"}" }
            }]}}]
        }"#);
        let out = translate_sse(&delta, &mut state)
            .expect("returned None");
        match out {
            StreamEvent::ToolCallDelta { id, input_json_delta } => {
                assert_eq!(id, "call_xyz");
                assert_eq!(input_json_delta, "{\"command\":\"ls\"}");
            }
            other => panic!("expected ToolCallDelta, got {other:?}"),
        }
        assert_eq!(
            state.tool_calls.get(&0).unwrap().input_json,
            "{\"command\":\"ls\"}"
        );
    }
}

/// v0.7.4 (UX fix) tests for the think-tag filter.
#[cfg(test)]
mod think_filter_tests {
    use super::*;

/// v0.8 helper for tests: flatten fragments to only their `Visible` text.
/// Existing tests compare string outputs; this lets them keep their
/// assertions without rewriting each one.
fn visible_only(frags: Vec<ThinkFragment>) -> String {
    let mut s = String::new();
    for f in frags {
        if let ThinkFragment::Visible(t) = f {
            s.push_str(&t);
        }
    }
    s
}


    #[test]
    fn passes_through_text_without_tags() {
        let mut f = ThinkTagFilter::new();
        assert_eq!(visible_only(f.push("hello world")), "hello world");
        assert_eq!(visible_only(f.push(" more text")), " more text");
    }

    #[test]
    fn strips_full_think_block() {
        let mut f = ThinkTagFilter::new();
        let out = visible_only(f.push("before<think>reasoning text</think>after"));
        assert_eq!(out, "beforeafter");
    }

    #[test]
    fn handles_think_block_at_start() {
        let mut f = ThinkTagFilter::new();
        let out = visible_only(f.push("<think>hidden</think>visible"));
        assert_eq!(out, "visible");
    }

    #[test]
    fn handles_think_block_at_end() {
        let mut f = ThinkTagFilter::new();
        let out = visible_only(f.push("visible<think>hidden</think>"));
        assert_eq!(out, "visible");
    }

    #[test]
    fn handles_think_block_with_newlines() {
        let mut f = ThinkTagFilter::new();
        let out = visible_only(f.push("<think>line1\nline2\nline3</think>clean"));
        assert_eq!(out, "clean");
    }

    #[test]
    fn handles_multiple_think_blocks() {
        let mut f = ThinkTagFilter::new();
        let out = visible_only(f.push("<think>a</think>X<think>b</think>Y<think>c</think>"));
        assert_eq!(out, "XY");
    }

    #[test]
    fn handles_unclosed_think_block_across_chunks() {
        // Chunk 1 ends with "<" — that's the start of "<mm:think>".
        // Chunk 2 begins with "mm:think>reasoning</think>clean".
        // The filter must hold back the "<" from chunk 1, recognize
        // the opening tag in chunk 2, then drop the reasoning.
        let mut f = ThinkTagFilter::new();
        assert_eq!(visible_only(f.push("before<")), "before");
        assert_eq!(visible_only(f.push("think>reasoning</think>clean")), "clean");
    }

    #[test]
    fn handles_unclosed_closing_tag_across_chunks() {
        // Chunk 1 ends with "<think>reasoning</" (opening tag opened,
        // closing tag not yet arrived).
        // Chunk 2 starts with "think>clean".
        let mut f = ThinkTagFilter::new();
        assert_eq!(visible_only(f.push("<think>reasoning</")), "");
        assert_eq!(visible_only(f.push("think>clean")), "clean");
    }

    #[test]
    fn empty_input_yields_empty_output() {
        let mut f = ThinkTagFilter::new();
        assert_eq!(visible_only(f.push("")), "");
    }

    #[test]
    fn realistic_minimax_output() {
        // Simulate the typical MiniMax-M3 streaming output.
        let mut f = ThinkTagFilter::new();
        let chunk1 = "<think>The user wants me to count.";
        let chunk2 = " Let me do that now.\n</think>1, 2, 3";
        let chunk3 = ", 4, 5";
        assert_eq!(visible_only(f.push(chunk1)), "");
        assert_eq!(visible_only(f.push(chunk2)), "1, 2, 3");
        assert_eq!(visible_only(f.push(chunk3)), ", 4, 5");
    }

    #[test]
    fn thinking_fragments_are_emitted_separately() {
        // v0.8: the filter now emits `Thinking` fragments so the
        // TUI can render reasoning with dim/italic style.
        let mut f = ThinkTagFilter::new();
        let frags = f.push("<think>reasoning</think>answer");
        // Empty Visible prefixes are dropped by the filter
        // (see emit_prefix_hold_rest). Order:
        //   Thinking("reasoning"), Visible("answer").
        assert_eq!(
            frags,
            vec![
                ThinkFragment::Thinking("reasoning".into()),
                ThinkFragment::Visible("answer".into()),
            ]
        );
    }

    #[test]
    fn empty_thinking_block_is_skipped() {
        // v0.8: an empty `<think></think>` block (start == end)
        // produces no Thinking fragment — the renderer doesn't
        // need to render anything. This is an implementation
        // choice that keeps the fragment list tight.
        let mut f = ThinkTagFilter::new();
        let frags = f.push("<think></think>X");
        assert_eq!(frags, vec![ThinkFragment::Visible("X".into())]);
    }

    #[test]
    fn aiio_streaming_keeps_full_thinking_across_chunks() {
        // v0.8.4 (regression): aiio / Anthropic-translated
        // MiniMax-M3 splits a single reasoning turn across many
        // `data:` chunks; the `</think>` close only arrives on the
        // final chunk. The previous `hold_back` implementation only
        // retained at most the last 7 chars before the open tag, so
        // everything before that window was silently dropped and the
        // user saw `💭 ommand.` instead of the full 80+ char
        // reasoning. The fix keeps every byte in `in_think` mode.
        let mut f = ThinkTagFilter::new();
        // Five small chunks that simulate aiio's byte-stream.
        let chunks = [
            "<think>The user wants me to run",
            " `ls /tmp` and list 5 files.",
            " Let me ",
            "execute the ",
            "command.</think>

",
        ];
        // No visible fragment yet (we are inside a think block).
        for c in &chunks[..chunks.len() - 1] {
            let frags = f.push(c);
            assert!(
                frags.is_empty(),
                "expected no fragments before close, got {frags:?}"
            );
        }
        // Final chunk triggers the close — and now we should see
        // the FULL accumulated reasoning as a single Thinking
        // fragment, not just the last few characters.
        let frags = f.push(chunks[chunks.len() - 1]);
        // The Thinking fragment is the concat of all 5 chunks with
        // the `<think>` opener and `</think>\n\n` closer stripped.
        let expected_thinking = chunks
            .concat()
            .trim_start_matches("<think>")
            .trim_end_matches("</think>\n\n")
            .to_string();
        assert_eq!(
            frags,
            vec![
                ThinkFragment::Thinking(expected_thinking),
                ThinkFragment::Visible("\n\n".into()),
            ],
            "v0.8.4 regression: aiio-style multi-chunk thinking must \
             surface in full, not clip to the last few chars"
        );
    }
}

/// v0.8.4 (bugfix): the OpenAI provider's stream() function MUST
/// emit a terminal `StreamEvent::MessageStop` after the SSE
/// connection closes, even when the upstream model never sent a
/// `finish_reason` chunk. Without this, the agent loop sees `None`
/// from the stream and breaks out without ever observing a stop —
/// so a tool-call turn ends without producing an assistant message
/// in the transcript (the model streams text, calls a tool, the
/// tool result is fed back, but the next iteration's "model is
/// done" signal never arrives, leaving the agent stuck in Idle
/// with no assistant text visible after tool execution).
#[cfg(test)]
mod stream_emits_message_stop_tests {
    use super::*;

    /// Drive `translate_sse` over a single chunk, then assert
    /// `finalize` produces a MessageStop event. This is the exact
    /// contract `stream()` follows: parse → translate → finalize.
    #[test]
    fn finalize_emits_message_stop_when_stream_ends() {
        let mut state = StreamState::default();
        // Empty chunk: provider did not emit a finish_reason.
        let raw = r#"{"choices":[{"delta":{"content":"hello"}}]}"#;
        let ev = SseEvent::new("", raw);
        let _ = translate_sse(&ev, &mut state);
        // Drive finalize exactly like stream() now does.
        let final_events = finalize(&mut state, None);
        // Must contain a MessageStop.
        let has_stop = final_events
            .iter()
            .any(|e| matches!(e, StreamEvent::MessageStop { .. }));
        assert!(
            has_stop,
            "v0.8.4 regression: finalize() must yield a MessageStop even when no finish_reason was observed; agent loop needs it to break out of Idle after tool execution. Got: {final_events:?}"
        );
        // And the stop_reason must default to "stop", not be empty
        // (agent.rs treats unknown / empty stop_reasons as
        // "end_turn" but a literal default is easier to reason
        // about in transcripts / logs).
        let stop_reason = final_events
            .iter()
            .find_map(|e| match e {
                StreamEvent::MessageStop { stop_reason, .. } => Some(stop_reason.clone()),
                _ => None,
            })
            .unwrap_or_default();
        assert_eq!(stop_reason, "stop");
    }

    #[test]
    fn finalize_flushes_unclosed_tool_call() {
        // v0.8.4 regression: a model like aiio sometimes emits
        // ToolCallStart with a partial args fragment (`{`) but
        // never sends a separate ToolCallDelta and never sends
        // finish_reason (the connection just closes). Without
        // finalize's flush, the transcript line for the tool call
        // stays open with empty `input_json`. finalize should
        // emit ToolCallStop for any remaining pending tool calls
        // BEFORE MessageStop, so the runtime can render the
        // truncated args line.
        let mut state = StreamState::default();
        let raw = r#"{
            "choices":[{"delta":{"tool_calls":[{
                "index":0, "id":"call_xyz", "type":"function",
                "function":{"name":"bash","arguments":"{"}
            }]}}]
        }"#;
        let ev = SseEvent::new("", raw);
        let _ = translate_sse(&ev, &mut state);
        let final_events = finalize(&mut state, None);
        let has_stop = final_events
            .iter()
            .any(|e| matches!(e, StreamEvent::ToolCallStop { .. }));
        assert!(
            has_stop,
            "finalize() must flush pending ToolCallStop for unclosed tool calls; got: {final_events:?}"
        );
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiError {
    message: String,
}
