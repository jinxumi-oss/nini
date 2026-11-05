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
    /// provider 级 / model 级请求头与认证头配置。
    overrides: crate::provider_overrides::HeaderOverrides,
}

impl OpenAiResponsesProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            // Bounded connect, **no** total timeout: a streaming
            // response is supposed to stay open. See
            // `crate::error_body::http_client`.
            client: crate::error_body::http_client(),
            base_url: DEFAULT_BASE_URL.to_string(),
            api_key: api_key.into(),
            overrides: crate::provider_overrides::HeaderOverrides::default(),
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

    /// 附加到**每个**请求上的自定义请求头。
    ///
    /// 多次调用会追加。应用顺序：provider 默认头 → provider 级自定义头
    /// → model 级自定义头，同名头以后写的为准。
    pub fn with_headers(mut self, headers: Vec<(String, String)>) -> Self {
        self.overrides.headers.extend(headers);
        self
    }

    /// 覆盖认证头的名字。默认是 `Authorization: Bearer <key>`；设成
    /// `auth_header("api-key")` 后变成 `api-key: <key>`（**不带 Bearer
    /// 前缀**——Azure 的 `api-key`、Google 的 `x-goog-api-key` 都是裸值）。
    pub fn with_auth_header(mut self, name: &'static str) -> Self {
        self.overrides.auth_header = Some(name);
        self
    }

    /// 按模型名的覆盖表（键是 `req.model` 的原样值）。
    pub fn with_model_overrides(
        mut self,
        map: std::collections::BTreeMap<String, crate::provider_overrides::ModelOverride>,
    ) -> Self {
        self.overrides.model_overrides = map;
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
        // stream 是 `'static` 的，覆盖配置也要 clone 出来再捕获。
        let overrides = self.overrides.clone();

        Box::pin(try_stream! {
            // Minimal Responses mapping: turn each nini Message into an `input`
            // item of the right type. For Phase 2 we translate text messages
            // 1:1; tool use + tool result items use their Responses equivalents.
            let body = build_request_body(&req)?;
            // model 级 base URL 覆盖 provider 自己的。
            let base = match overrides.base_url_for(&req.model) {
                Some(b) => b.to_string(),
                None => base_url.clone(),
            };
            let url = crate::api_url::responses_url(&base);
            // 认证头：默认 `Authorization: Bearer <key>`；设了名字就用那个
            // 名字发裸值。
            let auth = match overrides.auth_header {
                Some(name) => (name.to_string(), api_key.clone()),
                None => ("Authorization".to_string(), format!("Bearer {api_key}")),
            };
            let defaults = vec![
                auth,
                ("content-type".to_string(), "application/json".to_string()),
            ];
            let headers = overrides.resolve(&req.model, &defaults);
            let mut response = crate::provider_overrides::apply_headers(client.post(&url), &headers)
                .body(body)
                .send()
                .await?;

            let status = response.status();
            if !status.is_success() {
                // v0.10 (fix): keep the server's explanation. See
                // `crate::error_body` — the body used to be discarded,
                // which made every failure look identical and hid the
                // reason from `nini_core::overflow`.
                let extracted = crate::error_body::extract_error_detail(&mut response).await;
                Err(ProviderError::Api {
                    status: status.as_u16(),
                    message: crate::error_body::compose_api_message(status, extracted),
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

/// v0.10 (R1f): exposed so the wire shape can be asserted directly.
/// Encodes the OpenAI Responses request body for a neutral `Request`, including the
/// `reasoning` field. Previously private, which is why the thinking level
/// had no way to be observed from a test — recorded, persisted, and never
/// visible to anything that could catch it missing.
pub fn build_request_body(req: &Request) -> Result<String, ProviderError> {
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
                // v0.10 (fix): Pi skips a user message whose content array
                // is empty (`openai-responses-shared.ts:206`: `if
                // (content.length === 0) continue;`). nini sent an input
                // item with an empty `input_text` instead.
                //
                // A user message that *has* a text block stays: Pi sends
                // the block as it is (an empty text part included), so
                // the turn is kept — the same rule as the OpenAI chat
                // path.
                if m.content.is_empty() {
                    continue;
                }
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

    let mut body = serde_json::json!({
        "model": req.model,
        "input": items,
        "stream": true,
        "max_output_tokens": req.max_tokens,
        "temperature": req.temperature,
        "tools": tools,
        // v0.10 (R1f): the Responses API takes a nested `reasoning`
        // object rather than a flat `reasoning_effort`. Pi builds the same
        // shape from the same `reasoning` level
        // (`openai-responses.ts:206-210`). `off` becomes omission.
        "reasoning": match req.reasoning.filter(|l| l.is_on()) {
            Some(l) => serde_json::json!({ "effort": l.as_str() }),
            None => serde_json::Value::Null,
        },
    });
    if body.get("reasoning").map(|v| v.is_null()).unwrap_or(false) {
        body.as_object_mut().unwrap().remove("reasoning");
    }

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
