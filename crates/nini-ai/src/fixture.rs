//! Deterministic fixture provider for tests and demos.
//!
//! Returns scripted `StreamEvent`s without any network I/O. Useful for:
//! - Unit-testing agent loops without real LLM credentials
//! - Smoke-testing CLI flows
//! - Reproducible integration tests

use async_stream::try_stream;
use futures_core::Stream;
use nini_core::provider::*;
use std::pin::Pin;

/// A scripted turn in a fixture provider.
#[derive(Debug, Clone)]
pub enum FixtureTurn {
    /// Emit a text delta.
    Text(String),
    /// Emit a single tool call with the given JSON args.
    ToolCall {
        name: String,
        args: serde_json::Value,
    },
    /// Emit a stop event with usage.
    Stop { stop_reason: String, usage: Usage },
}

/// Deterministic provider that yields a fixed sequence of events.
#[derive(Debug, Clone)]
pub struct FixtureProvider {
    events: Vec<StreamEvent>,
}

impl FixtureProvider {
    /// Build a fixture provider from a list of turns.
    pub fn from_turns(model: impl Into<String>, turns: Vec<FixtureTurn>) -> Self {
        let model = model.into();
        let mut events: Vec<StreamEvent> = Vec::new();
        let mut emitted_message_start = false;
        for (i, turn) in turns.iter().enumerate() {
            if !emitted_message_start {
                events.push(StreamEvent::MessageStart {
                    id: format!("msg_fixture_{i}"),
                    model: model.clone(),
                });
                emitted_message_start = true;
            }
            match turn {
                FixtureTurn::Text(t) => {
                    events.push(StreamEvent::TextDelta { text: t.clone() });
                }
                FixtureTurn::ToolCall { name, args } => {
                    let id = format!("toolu_fixture_{i}");
                    events.push(StreamEvent::ToolCallStart {
                        id: id.clone(),
                        name: name.clone(),
                    });
                    let args_str = serde_json::to_string(args).unwrap_or_default();
                    events.push(StreamEvent::ToolCallDelta {
                        id: id.clone(),
                        input_json_delta: args_str.clone(),
                    });
                    let parsed: serde_json::Value =
                        serde_json::from_str(&args_str).unwrap_or(serde_json::Value::Null);
                    events.push(StreamEvent::ToolCallStop {
                        id,
                        input_json: parsed,
                    });
                }
                FixtureTurn::Stop { stop_reason, usage } => {
                    events.push(StreamEvent::MessageStop {
                        stop_reason: stop_reason.clone(),
                        usage: usage.clone(),
                    });
                }
            }
        }
        Self { events }
    }

    /// Build a fixture provider that emits a single text response.
    pub fn text(model: impl Into<String>, text: impl Into<String>) -> Self {
        Self::from_turns(
            model,
            vec![
                FixtureTurn::Text(text.into()),
                FixtureTurn::Stop {
                    stop_reason: "end_turn".to_string(),
                    usage: Usage::default(),
                },
            ],
        )
    }

    /// Build a fixture provider that emits a single tool call.
    pub fn tool_call(
        model: impl Into<String>,
        name: impl Into<String>,
        args: serde_json::Value,
    ) -> Self {
        Self::from_turns(
            model,
            vec![
                FixtureTurn::ToolCall {
                    name: name.into(),
                    args,
                },
                FixtureTurn::Stop {
                    stop_reason: "tool_use".to_string(),
                    usage: Usage::default(),
                },
            ],
        )
    }

    /// Build a fixture provider that emits text after a tool result.
    pub fn text_after_tool(model: impl Into<String>, text: impl Into<String>) -> Self {
        Self::from_turns(
            model,
            vec![
                FixtureTurn::Text(text.into()),
                FixtureTurn::Stop {
                    stop_reason: "end_turn".to_string(),
                    usage: Usage::default(),
                },
            ],
        )
    }
}

impl Provider for FixtureProvider {
    fn name(&self) -> &'static str {
        "fixture"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }

    fn stream(
        &self,
        _req: Request,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send + 'static>> {
        let events = self.events.clone();
        Box::pin(try_stream! {
            for ev in events {
                yield ev;
            }
        })
    }
}

/// Programmed provider: each call to `stream()` pops the next event list from
/// a queue. Useful for tests that need different responses per turn
/// (e.g., first call returns a tool_use, second call returns the final text).
pub struct ProgrammedProvider {
    queue: std::sync::Mutex<Vec<Vec<StreamEvent>>>,
    cursor: std::sync::Mutex<usize>,
    model: String,
}

impl ProgrammedProvider {
    /// Create a new programmed provider. Each inner `Vec<StreamEvent>` is
    /// returned as one stream (one agent turn). When the queue is exhausted,
    /// subsequent calls wrap around so the agent can serve multi-turn
    /// conversations without a real LLM.
    pub fn new(turns: Vec<Vec<StreamEvent>>) -> Self {
        Self {
            queue: std::sync::Mutex::new(turns),
            cursor: std::sync::Mutex::new(0),
            model: "programmed".to_string(),
        }
    }

    /// Build a programmed provider from a list of [`FixtureTurn`]s. Each
    /// element is a single turn. The first turn gets `MessageStart` injected
    /// automatically; later turns do not (model is already known).
    pub fn from_turns(turns: Vec<Vec<FixtureTurn>>) -> Self {
        let queue: Vec<Vec<StreamEvent>> = turns
            .into_iter()
            .enumerate()
            .map(|(i, turn_list)| {
                let mut events: Vec<StreamEvent> = Vec::new();
                if i == 0 {
                    events.push(StreamEvent::MessageStart {
                        id: format!("msg_{i}"),
                        model: "programmed".to_string(),
                    });
                }
                for (j, turn) in turn_list.iter().enumerate() {
                    match turn {
                        FixtureTurn::Text(t) => {
                            events.push(StreamEvent::TextDelta { text: t.clone() });
                        }
                        FixtureTurn::ToolCall { name, args } => {
                            let id = format!("toolu_{i}_{j}");
                            events.push(StreamEvent::ToolCallStart {
                                id: id.clone(),
                                name: name.clone(),
                            });
                            let args_str = serde_json::to_string(args).unwrap_or_default();
                            events.push(StreamEvent::ToolCallDelta {
                                id: id.clone(),
                                input_json_delta: args_str.clone(),
                            });
                            let parsed: serde_json::Value =
                                serde_json::from_str(&args_str).unwrap_or(serde_json::Value::Null);
                            events.push(StreamEvent::ToolCallStop {
                                id,
                                input_json: parsed,
                            });
                        }
                        FixtureTurn::Stop { stop_reason, usage } => {
                            events.push(StreamEvent::MessageStop {
                                stop_reason: stop_reason.clone(),
                                usage: usage.clone(),
                            });
                        }
                    }
                }
                events
            })
            .collect();
        Self::new(queue)
    }
}

impl std::fmt::Debug for ProgrammedProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgrammedProvider")
            .field("remaining_turns", &self.queue.lock().unwrap().len())
            .field("model", &self.model)
            .finish()
    }
}

impl Provider for ProgrammedProvider {
    fn name(&self) -> &'static str {
        "programmed"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }

    fn stream(
        &self,
        _req: Request,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send + 'static>> {
        let events = {
            let queue = self.queue.lock().unwrap();
            if queue.is_empty() {
                Vec::new()
            } else {
                let mut cursor = self.cursor.lock().unwrap();
                let idx = *cursor % queue.len();
                *cursor = (*cursor + 1) % queue.len();
                queue[idx].clone()
            }
        };
        Box::pin(try_stream! {
            for ev in events {
                yield ev;
            }
        })
    }
}
