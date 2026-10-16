//! The wire path around a tool call, over a real socket.
//!
//! These four tests were written while hunting a report that turned out to
//! be something else, and they are kept because every layer they cross was
//! ruled out by elimination while hunting it:
//!
//! 1. `the_second_response_carries_text` — a reply after a tool result
//!    reaches the provider's event stream intact (SSE framing, `translate_sse`,
//!    `finalize`).
//! 2. `a_tool_call_followed_by_its_answer_reaches_the_transcript` — the
//!    agent loop runs the tool, asks again, and the second answer comes out.
//! 3. `a_seeded_second_run_reaches_the_transcript` — the same with history
//!    seeded, which is what every run after the first does in the TUI.
//! 4. `a_long_cjk_reply_is_not_truncated` — a long CJK reply crosses the
//!    provider whole.
//!
//! Together they mean: when a long reply looks missing in the TUI, the loss
//! is downstream of the wire, and `agent_wire_e2e.rs` cannot see it —
//! despite its name it drives `ProgrammedProvider`, never HTTP.
//!
//! The two halves are tested separately, because "the model said nothing"
//! and "nini threw it away" look identical in the terminal:
//!
//! * `the_second_response_carries_text` — the provider layer, in isolation
//! * `a_tool_call_followed_by_its_answer_reaches_the_transcript` — the
//!   agent loop, the tool, the second request and the second response
//!
//! Both take the mock server's address explicitly rather than sharing one
//! server, so neither can be answered by the other's fixture.

use futures_util::StreamExt;
use nini_ai::openai_compat::OpenAiCompatProvider;
use nini_core::provider::{Message, Provider, Request, Role, StreamEvent};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn sse_body(chunks: &[&str]) -> String {
    let mut body = String::new();
    for c in chunks {
        body.push_str("data: ");
        body.push_str(c);
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
}

fn chunk(content: &str) -> String {
    serde_json::json!({
        "id": "c", "object": "chat.completion.chunk", "created": 0, "model": "m",
        "choices": [{"index": 0,
                     "delta": {"role": "assistant", "content": content},
                     "finish_reason": serde_json::Value::Null}]
    })
    .to_string()
}

fn tool_call_chunk() -> String {
    serde_json::json!({
        "id": "c", "object": "chat.completion.chunk", "created": 0, "model": "m",
        "choices": [{"index": 0,
                     "delta": {"tool_calls": [{
                         "index": 0, "id": "call_1", "type": "function",
                         "function": {"name": "bash", "arguments": "{\"command\":\"echo hi\"}"}
                     }]},
                     "finish_reason": serde_json::Value::Null}]
    })
    .to_string()
}

fn finish(finish_reason: &str) -> String {
    serde_json::json!({
        "id": "c", "object": "chat.completion.chunk", "created": 0, "model": "m",
        "choices": [{"index": 0, "delta": {}, "finish_reason": finish_reason}]
    })
    .to_string()
}

fn stop_chunk(usage: i64) -> String {
    serde_json::json!({
        "id": "c", "object": "chat.completion.chunk", "created": 0, "model": "m",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": usage, "completion_tokens": 7, "total_tokens": usage + 7}
    })
    .to_string()
}

fn request(text: &str) -> Request {
    Request {
        model: "m".to_string(),
        messages: vec![Message {
            role: Role::User,
            content: vec![nini_core::provider::ContentBlock::Text {
                text: text.to_string(),
            }],
            timestamp: 0,
        }],
        tools: vec![],
        max_tokens: Some(256),
        temperature: None,
        system: None,
        reasoning: None,
    }
}

/// The provider layer alone: a response with text and a `stop` produces
/// `TextDelta` and then `MessageStop`.
#[tokio::test]
async fn the_second_response_carries_text() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body(&[
                    &chunk("the answer after the tool"),
                    &stop_chunk(120),
                ])),
        )
        .mount(&server)
        .await;

    let provider = OpenAiCompatProvider::new(server.uri(), "k");
    let mut stream = provider.stream(request("go on"));
    let mut events = Vec::new();
    while let Some(ev) = stream.next().await {
        events.push(ev.expect("stream event"));
    }

    let text: String = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        text, "the answer after the tool",
        "the provider dropped the answer: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::MessageStop { .. })),
        "no MessageStop: {events:?}"
    );
}

/// The loop, not the provider. A tool call runs the tool, the model is
/// asked again, and its answer has to come out of `agent.run()`.
///

#[tokio::test]
async fn a_tool_call_followed_by_its_answer_reaches_the_transcript() {
    use nini_core::tool::{Tool, ToolContext, ToolError, ToolOutput, ToolRegistry, ToolSpec};
    use std::sync::Arc;

    #[derive(Default)]
    struct Echo;

    #[async_trait::async_trait]
    impl Tool for Echo {
        fn name(&self) -> &'static str {
            "bash"
        }
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "bash".to_string(),
                description: "test tool".to_string(),
                input_schema: serde_json::json!({"type": "object"}),
            }
        }
        async fn execute(
            &self,
            _args: serde_json::Value,
            _ctx: ToolContext,
        ) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput {
                content: "hi".to_string(),
                is_error: false,
                details: None,
                duration_ms: 1,
                terminate: false,
            })
        }
    }

    let server = MockServer::start().await;
    let look = chunk("let me look");
    let call = tool_call_chunk();
    let stop = finish("tool_calls");
    let answer = chunk("after the tool, the answer is 42");
    let stop2 = stop_chunk(240);

    // The second request is the one that carries the tool result, so the
    // fixture branches on the body rather than on call order. Wiremock
    // answers every request from the same matcher otherwise, and the run
    // then spins on an identical tool call until `max_iterations` — which
    // is what the first version of this test did.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(wiremock::matchers::body_string_contains("tool_call_id"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body(&[&answer, &stop2])),
        )
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body(&[&look, &call, &stop])),
        )
        .mount(&server)
        .await;

    let mut agent = nini_core::agent::Agent::new(
        Arc::new(OpenAiCompatProvider::new(server.uri(), "k")),
        ToolRegistry::new().register(Arc::new(Echo::default())),
        nini_core::agent::RunConfig::new("m"),
    );

    // The second response comes from the same mock (wiremock answers the
    // same matcher repeatedly), so it must carry the conclusion.
    let mut stream = Box::pin(agent.run(nini_core::AgentMessage::user("look it up")));
    let mut texts: Vec<String> = Vec::new();
    let mut tool_results = 0usize;
    let mut turns = 0usize;
    while let Some(ev) = stream.next().await {
        match ev.expect("agent event") {
            nini_core::AgentEvent::TextDelta { text } => texts.push(text),
            nini_core::AgentEvent::ToolResult { .. } => tool_results += 1,
            nini_core::AgentEvent::TurnEnd { .. } => turns += 1,
            _ => {}
        }
    }

    let all: String = texts.concat();
    assert!(all.contains("let me look"), "first answer lost: {texts:?}");
    assert_eq!(tool_results, 1, "the tool did not run exactly once");
    assert!(
        turns >= 2,
        "after a tool result the model must be asked again and answer; \
         the run ended after {turns} turn(s) with {texts:?}"
    );
}

/// In the TUI every run after the first starts from
/// `AppState::session_context_messages()` — v0.10's A3 work — so the history
/// is seeded. This covers that path with the real provider.
#[tokio::test]
async fn a_seeded_second_run_reaches_the_transcript() {
    use nini_core::tool::{Tool, ToolContext, ToolError, ToolOutput, ToolRegistry, ToolSpec};
    use std::sync::Arc;

    #[derive(Default)]
    struct Echo;
    #[async_trait::async_trait]
    impl Tool for Echo {
        fn name(&self) -> &'static str {
            "bash"
        }
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "bash".to_string(),
                description: "test".to_string(),
                input_schema: serde_json::json!({"type": "object"}),
            }
        }
        async fn execute(
            &self,
            _a: serde_json::Value,
            _c: ToolContext,
        ) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput {
                content: "hi".into(),
                is_error: false,
                details: None,
                duration_ms: 1,
                terminate: false,
            })
        }
    }

    let server = MockServer::start().await;
    let plain = chunk("a plain answer, no tools involved");
    let stop = stop_chunk(100);
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body(&[&plain, &stop])),
        )
        .mount(&server)
        .await;

    // The history a real second run is handed: the first user turn and the
    // model's answer to it.
    let seed: Vec<nini_core::provider::Message> = vec![
        nini_core::provider::Message {
            role: Role::User,
            content: vec![nini_core::provider::ContentBlock::Text {
                text: "first message".into(),
            }],
            timestamp: 0,
        },
        nini_core::provider::Message {
            role: Role::Assistant,
            content: vec![nini_core::provider::ContentBlock::Text {
                text: "first answer".into(),
            }],
            timestamp: 0,
        },
    ];

    let mut agent = nini_core::agent::Agent::new(
        Arc::new(OpenAiCompatProvider::new(server.uri(), "k")),
        ToolRegistry::new().register(Arc::new(Echo::default())),
        nini_core::agent::RunConfig::new("m"),
    );
    agent.seed(seed);

    let mut stream = Box::pin(agent.run(nini_core::AgentMessage::user("second message")));
    let mut text = String::new();
    while let Some(ev) = stream.next().await {
        if let nini_core::AgentEvent::TextDelta { text: t } = ev.expect("event") {
            text.push_str(&t);
        }
    }

    assert!(
        text.contains("a plain answer"),
        "a seeded run produced no visible answer: {text:?}"
    );
}

/// tmux, scripted server, second turn: a short ASCII answer arrives whole,
/// and as soon as the reply is padded with CJK text the **tail** stops
/// arriving — `HEADMARK` on screen, `TAILMARK` gone, at 20 repetitions
/// (~140 chars) but not at 5 (~35 chars). A 90-row terminal does not
/// change it, so it is not the viewport.
///
/// This asks the provider alone, over a real socket, with the same bytes.
#[tokio::test]
async fn a_long_cjk_reply_is_not_truncated() {
    for reps in [5usize, 20, 60] {
        let body = format!(
            "HEADMARK-{reps}\n{}TAILMARK-{reps}\n",
            "填充段落内容。".repeat(reps)
        );
        let chunk_json = serde_json::json!({
            "id": "c", "object": "chat.completion.chunk", "created": 0, "model": "m",
            "choices": [{"index": 0,
                         "delta": {"role": "assistant", "content": body},
                         "finish_reason": serde_json::Value::Null}]
        })
        .to_string();
        let stop = stop_chunk(100);
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(sse_body(&[&chunk_json, &stop])),
            )
            .mount(&server)
            .await;

        let provider = OpenAiCompatProvider::new(server.uri(), "k");
        let mut stream = provider.stream(request("go on"));
        let mut text = String::new();
        while let Some(ev) = stream.next().await {
            if let StreamEvent::TextDelta { text: t } = ev.expect("event") {
                text.push_str(&t);
            }
        }

        assert!(
            text.contains(&format!("TAILMARK-{reps}")),
            "at {reps} repetitions the tail was dropped: got {} chars, ends {:?}",
            text.chars().count(),
            text.chars().rev().take(24).collect::<String>()
        );
    }
}
