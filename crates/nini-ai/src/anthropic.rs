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
    /// provider 级 / model 级请求头与认证头配置。
    overrides: crate::provider_overrides::HeaderOverrides,
}

impl AnthropicProvider {
    /// Create a new Anthropic provider with the given API key and default base URL.
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

    /// 附加到**每个**请求上的自定义请求头。
    ///
    /// 多次调用会追加。应用顺序：provider 默认头 → provider 级自定义头
    /// → model 级自定义头，同名头以后写的为准。
    pub fn with_headers(mut self, headers: Vec<(String, String)>) -> Self {
        self.overrides.headers.extend(headers);
        self
    }

    /// 覆盖认证头的名字。默认是 `x-api-key: <key>`；设成
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
        // stream 是 `'static` 的，覆盖配置也要 clone 出来再捕获。
        let overrides = self.overrides.clone();

        Box::pin(try_stream! {
            // Translate request
            let body = build_request_body(&req)?;
            // model 级 base URL 覆盖 provider 自己的。
            let base = match overrides.base_url_for(&req.model) {
                Some(b) => b.to_string(),
                None => base_url.clone(),
            };
            let url = crate::api_url::anthropic_messages_url(&base);
            // 认证头：默认 `x-api-key: <key>`；设了名字就用那个名字发裸值。
            let auth = match overrides.auth_header {
                Some(name) => (name.to_string(), api_key.clone()),
                None => ("x-api-key".to_string(), api_key.clone()),
            };
            let defaults = vec![
                auth,
                ("anthropic-version".to_string(), API_VERSION.to_string()),
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

/// v0.10 (R1f): exposed so the wire shape can be asserted directly.
/// Encodes the Anthropic Messages request body for a neutral `Request`, including the
/// `reasoning` field. Previously private, which is why the thinking level
/// had no way to be observed from a test — recorded, persisted, and never
/// visible to anything that could catch it missing.
/// Whether this model accepts `temperature`, per Pi's model data.
///
/// Pi reads the flag off the model rather than the provider
/// (`anthropic-messages.ts:182`), so it is a property of the model id, not
/// of the endpoint. The catalog is generated from the same data files Pi
/// ships, which is what makes this answerable without a table written by
/// hand.
fn model_supports_temperature(model: &str) -> bool {
    let id = model.rsplit('/').next().unwrap_or(model);
    nini_core::model_runtime::ModelRuntime::with_defaults()
        .all()
        .into_iter()
        .find(|m| m.provider == "anthropic" && m.id.rsplit('/').next() == Some(id))
        .map(|m| m.supports_temperature)
        .unwrap_or(true)
}

pub fn build_request_body(req: &Request) -> Result<String, ProviderError> {
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
    /// v0.10 (R1f): extended thinking.
    ///
    /// Wire shape from Pi (`anthropic-messages.ts:1050-1072`):
    ///   * thinking requested, older model → `{type:"enabled", budget_tokens, display}`
    ///   * explicitly off                        → `{type:"disabled"}`
    ///   * no opinion (`None`)                   → field omitted entirely
    ///
    /// The budget comes from `ThinkingLevel::budget_tokens`; Pi's own
    /// default when thinking is on and no budget is given is 1024
    /// (`:1067`).
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<AnthropicThinking>,
    stream: bool,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicThinking {
    Enabled {
        budget_tokens: u32,
        display: &'static str,
    },
    Disabled,
}

#[derive(Debug, Serialize)]
struct AnthropicMessage<'a> {
    role: &'a str,
    content: Vec<AnthropicContent<'a>>,
}

#[derive(Debug, Clone, Serialize)]
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
        // v0.10 (R1f) — Pi: `if (!options?.reasoning) { thinkingEnabled: false }`
        // (`anthropic-messages.ts:824-826`). `Off` is explicit, `None` is
        // silence; the two are different requests.
        let thinking = req.reasoning.map(|lvl| {
            if lvl.is_on() {
                AnthropicThinking::Enabled {
                    budget_tokens: lvl.budget_tokens(),
                    display: "summarized",
                }
            } else {
                AnthropicThinking::Disabled
            }
        });
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
            // v0.10 (fix): Anthropic rejects a message with no content
            // (`content: []` is a 400), and nini used to send exactly
            // that for a user turn with no text.
            //
            // Pi's Anthropic path filters the blank text blocks out of a
            // **user** turn and drops the turn when nothing is left
            // (`anthropic-messages.ts:1264-1270`):
            //
            // ```text
            // 1264  const filteredBlocks = blocks.filter((b) => {
            // 1265      if (b.type === "text") {
            // 1266          return b.text.trim().length > 0;
            // 1267      }
            // 1268      return true;
            // 1269  });
            // 1270  if (filteredBlocks.length === 0) continue;
            // ```
            //
            // Pi does the same for an **assistant** turn (`:1276-1330`):
            //
            // ```text
            // 1276  } else if (msg.role === "assistant") {
            // 1277      const blocks: ContentBlockParam[] = [];
            // 1279      for (const block of msg.content) {
            // 1280          if (block.type === "text") {
            // 1281              if (block.text.trim().length === 0) continue;
            // 1282              blocks.push({ type: "text", text: sanitizeSurrogates(block.text) });
            // 1321          } else if (block.type === "toolCall") {
            // 1322              blocks.push({ type: "tool_use", id: block.id, name: block.name, input: … });
            // 1328          }
            // 1329      }
            // 1330      if (blocks.length === 0) continue;
            // ```
            //
            // The subtlety: Pi's `blocks` at `:1330` is the **filtered**
            // array — the blank text block was already skipped at
            // `:1281`. So an assistant turn that carries nothing but a
            // blank text block is dropped **whole** by Pi, and nini used
            // to keep it (`{"role":"assistant","content":[{"type":"text",
            // "text":""}]}`) — a shape Anthropic rejects.
            //
            // The filter therefore applies to *both* roles, and it only
            // ever touches **text** blocks: a `tool_use` / `tool_result`
            // block is never blank-filtered, so an assistant turn that
            // carries a tool call survives even when its text is empty
            // (the tool_use / tool_result pairing must not break).
            let content = match m.role {
                Role::User | Role::Assistant => {
                    let kept: Vec<AnthropicContent> = content
                        .into_iter()
                        .filter(|block| match block {
                            AnthropicContent::Text { text } => !text.trim().is_empty(),
                            _ => true,
                        })
                        .collect();
                    if kept.is_empty() {
                        continue;
                    }
                    kept
                }
                _ => content,
            };
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
            // Pi: `if (options?.temperature !== undefined &&
            //       !options?.thinkingEnabled && compat.supportsTemperature)`
            // (`anthropic-messages.ts:1021`).
            //
            // The `!thinkingEnabled` half was missed here. An earlier
            // change concluded from Pi's *OpenAI* path - where temperature
            // really is sent unconditionally - that Pi always sends it, and
            // carried that across to Anthropic. One provider's rule is not
            // another's rule.
            //
            // The other half, `compat.supportsTemperature`, is per-model and
            // comes from the catalog: real Pi data declares
            // `supportsTemperature: false` for `claude-opus-4-7`,
            // `claude-opus-5` and others. It lands with the catalog work.
            // Both halves of Pi's condition now
            // (`anthropic-messages.ts:1021`):
            //
            //   options?.temperature !== undefined
            //   && !options?.thinkingEnabled
            //   && compat.supportsTemperature
            //
            // The second is per-model and comes from the catalog, which is
            // generated from the same data files Pi ships.
            // `claude-opus-4-7`, `claude-opus-4-8` and `claude-opus-5`
            // declare `supportsTemperature: false`.
            //
            // A model the catalog does not have counts as supporting
            // temperature, which is Pi's own default
            // (`model.compat?.supportsTemperature ?? true`, `:182`) and the
            // safe direction: a value Pi would not have sent is dropped
            // rather than invented.
            temperature: req.temperature.filter(|_| {
                !req.reasoning.map(|l| l.is_on()).unwrap_or(false)
                    && model_supports_temperature(&req.model)
            }),
            thinking,
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
