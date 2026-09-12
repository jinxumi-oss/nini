#![allow(dead_code)] // Forward-compat fields for streaming variants
//! OpenAI Responses API provider.
//!
//! Distinct from `/v1/chat/completions`: uses `response.create` with item-based
//! I/O. Phase 2 emits minimal Responses support; richer features (file_search,
//! code_interpreter) arrive in Phase 3.

use super::sse::{SseEvent, SseParser};
use async_stream::try_stream;
use futures_util::StreamExt;
use nini_core::provider::*;
use reqwest::Client;
use std::pin::Pin;

/// Default OpenAI Responses API base URL (same host as Chat Completions).
pub const DEFAULT_BASE_URL: &str = "https://api.openai.com";

/// OpenAI Responses API provider.
pub struct OpenAiResponsesProvider {
    client: Client,
    base_url: String,
    api_key: String,
}

impl OpenAiResponsesProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            base_url: DEFAULT_BASE_URL.to_string(),
            api_key: api_key.into(),
        }
    }

    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    pub fn with_client(mut self, client: Client) -> Self {
        self.client = client;
        self
    }
}

impl Provider for OpenAiResponsesProvider {
    fn name(&self) -> &'static str {
        "openai-responses"
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
            // Minimal Responses mapping: turn each nini Message into an `input`
            // item of the right type. For Phase 2 we translate text messages
            // 1:1; tool use + tool result items use their Responses equivalents.
            let body = build_request_body(&req)?;
            let url = format!("{base_url}/v1/responses");
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

            while let Some(chunk_result) = byte_stream.next().await {
                let chunk = chunk_result?;
                for sse_event in parser.feed(&chunk).map_err(|e| ProviderError::Sse(e.to_string()))? {
                    if let Some(ev) = translate_sse(&sse_event) {
                        yield ev;
                    }
                }
            }
            for ev in parser.flush() {
                if let Some(ev) = translate_sse(&ev) {
                    yield ev;
                }
            }
        })
    }
}

fn build_request_body(req: &Request) -> Result<String, ProviderError> {
    let mut items: Vec<serde_json::Value> = Vec::new();
    for m in &req.messages {
        match m.role {
            Role::System => {
                if let Some(text) = m.content.iter().find_map(|c| match c {
                    ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                }) {
                    items.push(serde_json::json!({
                        "type": "message",
                        "role": "system",
                        "content": [{"type": "input_text", "text": text}],
                    }));
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
                items.push(serde_json::json!({
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": text}],
                }));
            }
            Role::Assistant => {
                for c in &m.content {
                    match c {
                        ContentBlock::Text { text } => {
                            items.push(serde_json::json!({
                                "type": "message",
                                "role": "assistant",
                                "content": [{"type": "output_text", "text": text}],
                            }));
                        }
                        ContentBlock::ToolUse { id, name, input } => {
                            items.push(serde_json::json!({
                                "type": "function_call",
                                "call_id": id,
                                "name": name,
                                "arguments": serde_json::to_string(input)?,
                            }));
                        }
                        _ => {}
                    }
                }
            }
            Role::Tool => {
                for c in &m.content {
                    if let ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } = c
                    {
                        items.push(serde_json::json!({
                            "type": "function_call_output",
                            "call_id": tool_use_id,
                            "output": content,
                            "status": if *is_error { "failed" } else { "completed" },
                        }));
                    }
                }
            }
        }
    }

    let tools: Vec<serde_json::Value> = req
        .tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "type": "function",
                "name": t.name,
                "description": t.description,
                "parameters": t.input_schema,
            })
        })
        .collect();

    let body = serde_json::json!({
        "model": req.model,
        "input": items,
        "stream": true,
        "max_output_tokens": req.max_tokens,
        "temperature": req.temperature,
        "tools": tools,
    });

    serde_json::to_string(&body).map_err(ProviderError::from)
}

fn translate_sse(ev: &SseEvent) -> Option<StreamEvent> {
    if ev.is_eof() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(&ev.data).ok()?;
    let event_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");

    // Responses API event names: response.output_text.delta, response.completed, etc.
    match event_type {
        "response.output_text.delta" => {
            let text = v
                .get("delta")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .to_string();
            Some(StreamEvent::TextDelta { text })
        }
        "response.function_call_arguments.delta" => {
            let delta = v
                .get("delta")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .to_string();
            let item_id = v
                .get("item_id")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .to_string();
            Some(StreamEvent::ToolCallDelta {
                id: item_id,
                input_json_delta: delta,
            })
        }
        "response.output_item.added" => {
            if v.get("item")
                .and_then(|i| i.get("type"))
                .and_then(|t| t.as_str())
                == Some("function_call")
            {
                let id = v
                    .get("item")
                    .and_then(|i| i.get("call_id"))
                    .and_then(|i| i.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = v
                    .get("item")
                    .and_then(|i| i.get("name"))
                    .and_then(|i| i.as_str())
                    .unwrap_or("")
                    .to_string();
                Some(StreamEvent::ToolCallStart { id, name })
            } else {
                None
            }
        }
        "response.output_item.done" => {
            if v.get("item")
                .and_then(|i| i.get("type"))
                .and_then(|t| t.as_str())
                == Some("function_call")
            {
                let id = v
                    .get("item")
                    .and_then(|i| i.get("call_id"))
                    .and_then(|i| i.as_str())
                    .unwrap_or("")
                    .to_string();
                let args = v
                    .get("item")
                    .and_then(|i| i.get("arguments"))
                    .and_then(|a| a.as_str())
                    .unwrap_or("{}");
                let input_json: serde_json::Value =
                    serde_json::from_str(args).unwrap_or(serde_json::Value::Null);
                Some(StreamEvent::ToolCallStop { id, input_json })
            } else {
                None
            }
        }
        "response.completed" | "response.incomplete" => {
            let usage = v
                .get("response")
                .and_then(|r| r.get("usage"))
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let usage: Usage = serde_json::from_value(usage).unwrap_or_default();
            let stop_reason = v
                .get("response")
                .and_then(|r| r.get("status"))
                .and_then(|s| s.as_str())
                .unwrap_or("completed")
                .to_string();
            Some(StreamEvent::MessageStop { stop_reason, usage })
        }
        "error" => {
            let message = v
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error")
                .to_string();
            Some(StreamEvent::Error { message })
        }
        _ => None,
    }
}
