//! An empty user turn must not be a 400.
//!
//! ## The defect
//!
//! `OpenAiRequest::from_neutral` used to serialise a user message with
//! no text as `content: ""`. A number of Chinese gateways reject that
//! outright. Measured against iFlytek Spark (`spark-x2.6`,
//! `maas-api.cn-huabei-1.xf-yun.com`):
//!
//! | user content | status | body |
//! |---|---|---|
//! | `""` | **400** | `{"error":{"code":10005,"message":"invalid request"}}` |
//! | `null` | **400** | same |
//! | `"   "` | 200 | fine |
//! | (no `messages` field) | 400 | `cannot pass empty 'messages'` |
//!
//! So one blank turn — the model answered with nothing, the user
//! pressed Enter on an empty line, a tool wrote an empty result — and
//! the whole request was refused. The conversation died on a shape
//! nobody could see.
//!
//! ## What Pi does (the specification)
//!
//! Pi never puts an empty *string* in a user message:
//!
//! * its agent builds user content as a **block array**
//!   (`pi-agent-core/dist/agent.js`: `content = [{type: "text", text:
//!   input}]`), never as a bare string;
//! * `convertMessages` maps that to a content-**parts** array
//!   (`openai-completions.ts:1233-1259`) and drops the message only
//!   when the parts array is empty (`if (content.length === 0)
//!   continue;`).
//!
//! So `content: ""` is not a Pi wire shape at all — it was an artefact
//! of nini collapsing the blocks into a string. There is nothing to
//! stay compatible with, and the turn itself is kept (Pi keeps it too,
//! as a parts array).
//!
//! nini therefore sends a single space for a blank user turn: the turn
//! survives, the message count and the role order do not change, and
//! the gateways that reject `""` accept it.
//!
//! ## What these tests pin
//!
//! 1. No user message ever goes out with an empty `content`.
//! 2. The tool_use → tool_result pairing survives an empty user turn.
//! 3. The provider's error code survives a 400, so a context overflow
//!    (`10907`) and a malformed request (`10005`) stay apart.
//! 4. The same rule on the Anthropic dialect, where Pi *drops* a blank
//!    user turn instead (`anthropic-messages.ts:1264-1268`).

use futures_core::Stream;
use futures_util::StreamExt;
use nini_ai::anthropic::AnthropicProvider;
use nini_ai::openai_compat::OpenAiCompatProvider;
use nini_ai::openai_responses::OpenAiResponsesProvider;
use nini_core::provider::{
    ContentBlock, Message, Provider, ProviderError, Request, Role, StreamEvent,
};
use std::pin::Pin;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A request that the mock answers with an empty SSE stream.
fn request_with(messages: Vec<Message>) -> Request {
    Request {
        model: "test-model".to_string(),
        messages,
        tools: vec![],
        max_tokens: Some(256),
        temperature: None,
        system: None,
        reasoning: None,
    }
}

/// A user turn with no text at all — the shape that used to be a 400.
fn empty_user_turn() -> Message {
    Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: String::new(),
        }],
        timestamp: 0,
    }
}

/// A user turn with a single space — already fine, and it must stay
/// exactly as it is (no trimming, no rewriting).
fn blank_user_turn() -> Message {
    Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "   ".to_string(),
        }],
        timestamp: 0,
    }
}

/// A user turn with no content blocks at all.
fn contentless_user_turn() -> Message {
    Message {
        role: Role::User,
        content: vec![],
        timestamp: 0,
    }
}

/// An assistant turn that asks for a tool.
fn assistant_tool_call() -> Message {
    Message {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolUse {
            id: "call_1".to_string(),
            name: "read".to_string(),
            input: serde_json::json!({"path": "a.txt"}),
        }],
        timestamp: 0,
    }
}

/// The result of that tool call.
fn tool_result() -> Message {
    Message {
        role: Role::Tool,
        content: vec![ContentBlock::ToolResult {
            tool_use_id: "call_1".to_string(),
            content: "the file".to_string(),
            is_error: false,
        }],
        timestamp: 0,
    }
}

/// Start a mock that answers every request with an empty SSE stream,
/// and remember the requests it received.
async fn mock_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string("data: [DONE]\n\n".to_string()),
        )
        .mount(&server)
        .await;
    server
}

/// Send `req` through the OpenAI-compatible provider and return the
/// body the server actually received.
async fn captured_body(server: &MockServer, req: Request) -> serde_json::Value {
    let provider = OpenAiCompatProvider::new(server.uri(), "test-key");
    let mut stream: Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send>> =
        provider.stream(req);
    while let Some(event) = stream.next().await {
        event.expect("the mock answers 200; there must be no error");
    }
    let requests = server.received_requests().await.expect("requests");
    assert_eq!(requests.len(), 1, "exactly one request must have been sent");
    serde_json::from_slice(&requests[0].body).expect("the body must be JSON")
}

/// The `messages` array of a captured body.
fn messages_of(body: &serde_json::Value) -> Vec<serde_json::Value> {
    body["messages"]
        .as_array()
        .expect("the body must carry a messages array")
        .clone()
}

// ---------------------------------------------------------------------------
// 1. The empty user turn
// ---------------------------------------------------------------------------

/// The headline: a user turn with no text must not go out as
/// `content: ""`. That string is a 400 on iFlytek and on other
/// gateways that copied its validation.
#[tokio::test]
async fn an_empty_user_turn_never_goes_out_as_an_empty_string() {
    let server = mock_server().await;
    let body = captured_body(
        &server,
        request_with(vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "hello".to_string(),
                }],
                timestamp: 0,
            },
            empty_user_turn(),
        ]),
    )
    .await;

    let messages = messages_of(&body);
    let users: Vec<&serde_json::Value> = messages.iter().filter(|m| m["role"] == "user").collect();
    assert_eq!(users.len(), 2, "both user turns must be sent: {body}");

    for user in &users {
        let content = user["content"]
            .as_str()
            .expect("the content must be a string");
        assert!(
            !content.is_empty(),
            "a user message went out with an empty content: {body}"
        );
    }

    // The blank turn is still a turn: it is present, and it carries
    // something the server will accept.
    let blank = users[1];
    assert_eq!(blank["role"], "user");
    assert_eq!(
        blank["content"], " ",
        "the blank turn must go out as a single space: {body}"
    );
}

/// A turn with a single space in it was always fine — it must reach
/// the server untouched. Pi does not trim user text, so neither does
/// nini.
#[tokio::test]
async fn a_blank_user_turn_is_left_alone() {
    let server = mock_server().await;
    let body = captured_body(&server, request_with(vec![blank_user_turn()])).await;

    let messages = messages_of(&body);
    assert_eq!(messages.len(), 1, "{body}");
    assert_eq!(
        messages[0]["content"], "   ",
        "the text must not be rewritten: {body}"
    );
}

/// The turn is kept, so the message count does not change. Dropping it
/// instead would leave `messages: []` on a fresh conversation — which
/// iFlytek rejects with a *different* 400 ("cannot pass empty
/// 'messages'").
#[tokio::test]
async fn the_blank_turn_is_kept_so_the_message_count_does_not_change() {
    let server = mock_server().await;
    let body = captured_body(&server, request_with(vec![empty_user_turn()])).await;

    let messages = messages_of(&body);
    assert_eq!(messages.len(), 1, "the turn must not be dropped: {body}");
    assert_eq!(messages[0]["role"], "user");
}

// ---------------------------------------------------------------------------
// 2. The tool_use → tool_result pairing
// ---------------------------------------------------------------------------

/// The pairing is the reason the blank turn is *kept* rather than
/// dropped: an assistant turn that asks for a tool, the tool's result,
/// and then the blank user turn must reach the server in that order,
/// with the tool call and its result still matched up.
#[tokio::test]
async fn the_tool_use_tool_result_pairing_survives_an_empty_user_turn() {
    let server = mock_server().await;
    let body = captured_body(
        &server,
        request_with(vec![
            assistant_tool_call(),
            tool_result(),
            empty_user_turn(),
        ]),
    )
    .await;

    let messages = messages_of(&body);
    assert_eq!(messages.len(), 3, "no message may be lost: {body}");

    // The order is the order of the conversation.
    assert_eq!(messages[0]["role"], "assistant", "{body}");
    assert_eq!(messages[1]["role"], "tool", "{body}");
    assert_eq!(messages[2]["role"], "user", "{body}");

    // The tool call and its result are still matched up.
    let call_id = messages[0]["tool_calls"][0]["id"]
        .as_str()
        .expect("the tool call must carry an id");
    assert_eq!(call_id, "call_1", "{body}");
    assert_eq!(
        messages[1]["tool_call_id"], "call_1",
        "the tool result must answer the tool call: {body}"
    );

    // Every tool call in the request has exactly one result.
    let calls = messages[0]["tool_calls"].as_array().expect("tool_calls");
    let results: Vec<&serde_json::Value> =
        messages.iter().filter(|m| m["role"] == "tool").collect();
    for call in calls {
        let id = call["id"].as_str().expect("an id");
        assert_eq!(
            results.iter().filter(|r| r["tool_call_id"] == id).count(),
            1,
            "the tool call {id} must have exactly one result: {body}"
        );
    }

    // And the blank user turn is still there, after the result.
    assert_eq!(messages[2]["content"], " ", "{body}");
}

/// An empty tool result goes out as Pi's placeholder, never as an
/// empty string — the same 400, one message further down.
///
/// Pi: `const toolResultText = hasText ? textResult : hasImages ?
/// "(see attached image)" : "(no tool output)";`
/// (`openai-completions.ts:1394-1396`).
#[tokio::test]
async fn an_empty_tool_result_goes_out_as_the_placeholder() {
    let server = mock_server().await;
    let body = captured_body(
        &server,
        request_with(vec![
            assistant_tool_call(),
            Message {
                role: Role::Tool,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "call_1".to_string(),
                    content: String::new(),
                    is_error: false,
                }],
                timestamp: 0,
            },
        ]),
    )
    .await;

    let messages = messages_of(&body);
    let tool = messages
        .iter()
        .find(|m| m["role"] == "tool")
        .expect("the tool result must be sent");
    assert_eq!(tool["content"], "(no tool output)", "{body}");
    assert_eq!(tool["tool_call_id"], "call_1", "{body}");
}

/// An assistant turn with neither text nor tool calls is dropped, as
/// Pi drops it ("Some providers require 'either content or tool_calls,
/// but not none'", `openai-completions.ts:1363-1372`). nini used to
/// send `{"role": "assistant"}` — no content, no tool calls.
#[tokio::test]
async fn an_assistant_turn_without_content_or_tool_calls_is_dropped() {
    let server = mock_server().await;
    let body = captured_body(
        &server,
        request_with(vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "hello".to_string(),
                }],
                timestamp: 0,
            },
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text {
                    text: String::new(),
                }],
                timestamp: 0,
            },
        ]),
    )
    .await;

    let messages = messages_of(&body);
    assert_eq!(
        messages.len(),
        1,
        "the empty assistant turn must be gone: {body}"
    );
    assert_eq!(messages[0]["role"], "user", "{body}");
}

// ---------------------------------------------------------------------------
// 3. The error code survives
// ---------------------------------------------------------------------------

/// iFlytek answers `{"error":{"code":10907,"message":"Bad Request"}}`
/// for a context that does not fit, and `{"error":{"code":10005,
/// "message":"invalid request"}}` for a malformed request. Both are
/// `400`, and the messages alone cannot tell them apart — the code
/// has to survive.
#[tokio::test]
async fn the_provider_error_code_survives_a_400() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_string(r#"{"error":{"code":10907,"message":"Bad Request"}}"#.to_string()),
        )
        .mount(&server)
        .await;

    let provider = OpenAiCompatProvider::new(server.uri(), "test-key");
    let mut stream: Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send>> =
        provider.stream(request_with(vec![empty_user_turn()]));
    let mut message = String::new();
    while let Some(event) = stream.next().await {
        if let Err(ProviderError::Api { status, message: m }) = event {
            assert_eq!(status, 400);
            message = m;
        }
    }

    assert!(
        message.contains("10907"),
        "the error code was dropped: {message:?}"
    );
    assert!(
        message.contains("Bad Request"),
        "the server's own wording was dropped: {message:?}"
    );
    assert!(
        message.starts_with("request failed: 400 Bad Request"),
        "the prefix must stay compatible: {message:?}"
    );
}

/// A string code (`context_length_exceeded`) is kept just like a
/// numeric one.
#[tokio::test]
async fn a_string_error_code_survives_a_400() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(400).set_body_string(
                r#"{"error":{"message":"prompt is too long","code":"context_length_exceeded"}}"#
                    .to_string(),
            ),
        )
        .mount(&server)
        .await;

    let provider = OpenAiCompatProvider::new(server.uri(), "test-key");
    let mut stream: Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send>> =
        provider.stream(request_with(vec![empty_user_turn()]));
    let mut message = String::new();
    while let Some(event) = stream.next().await {
        if let Err(ProviderError::Api { message: m, .. }) = event {
            message = m;
        }
    }

    assert!(
        message.contains("context_length_exceeded"),
        "the error code was dropped: {message:?}"
    );
    assert!(
        message.contains("prompt is too long"),
        "the server's own wording was dropped: {message:?}"
    );
}

/// A 400 must carry the *facts*: what the server said, and its error
/// code. The advice ("try `/compact`", "switch to a bigger model")
/// is added one layer up, by `nini_core::agent`, which knows how long
/// the conversation is; a provider-side hint would only duplicate it
/// — and guess the wrong cause.
#[tokio::test]
async fn a_400_carries_the_facts_and_no_provider_side_advice() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(400).set_body_string(
                r#"{"error":{"code":10005,"message":"invalid request"}}"#.to_string(),
            ),
        )
        .mount(&server)
        .await;

    let provider = OpenAiCompatProvider::new(server.uri(), "test-key");
    let mut stream: Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send>> =
        provider.stream(request_with(vec![empty_user_turn()]));
    let mut message = String::new();
    while let Some(event) = stream.next().await {
        if let Err(ProviderError::Api { message: m, .. }) = event {
            message = m;
        }
    }

    assert!(message.contains("invalid request"), "{message:?}");
    assert!(message.contains("10005"), "{message:?}");
    // The facts, verbatim and complete.
    assert_eq!(
        message, "request failed: 400 Bad Request: invalid request (code 10005)",
        "a 400 must keep the server's words and the vendor's code: {message:?}"
    );
    // No provider-side advice: the agent layer adds that, and only it
    // knows the session state the advice depends on.
    assert!(
        !message.contains("the server rejected the request"),
        "the provider layer must not repeat the agent layer's advice: {message:?}"
    );
    assert!(
        !message.contains("hint:"),
        "the provider layer must not add a hint: {message:?}"
    );
    // The message must not make a permanent 400 look transient — the
    // words `nini_core` matches on to decide "retry" must not appear.
    for transient in ["timeout", "timed out", "rate limit", "too many requests"] {
        assert!(
            !message.to_lowercase().contains(transient),
            "the message must not make a 400 look retryable ({transient}): {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 4. The Anthropic dialect
// ---------------------------------------------------------------------------
/// Pi *drops* a blank user turn on the Anthropic path — it filters the
/// blank text blocks out and skips the message when nothing is left
/// (`anthropic-messages.ts:1264-1268`) — because Anthropic rejects a
/// message with an empty content array. nini now does the same.
///
/// (On the OpenAI path the turn is *kept*, as a single space: Pi keeps
/// it there too, as a content-parts array.)
#[tokio::test]
async fn a_blank_user_turn_is_dropped_on_the_anthropic_dialect() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string("event: message_stop\ndata: {}\n\n".to_string()),
        )
        .mount(&server)
        .await;

    let provider = AnthropicProvider::new("test-key").with_base_url(server.uri());
    let mut stream: Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send>> =
        provider.stream(request_with(vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "hello".to_string(),
                }],
                timestamp: 0,
            },
            empty_user_turn(),
        ]));
    while let Some(event) = stream.next().await {
        event.expect("the mock answers 200; there must be no error");
    }

    let requests = server.received_requests().await.expect("requests");
    let body: serde_json::Value =
        serde_json::from_slice(&requests[0].body).expect("the body must be JSON");
    let messages = body["messages"].as_array().expect("a messages array");
    assert_eq!(
        messages.len(),
        1,
        "the blank user turn must be gone: {body}"
    );
    assert_eq!(messages[0]["content"][0]["text"], "hello", "{body}");
}

// ---------------------------------------------------------------------------
// 5. The Responses dialect
// ---------------------------------------------------------------------------

/// The Responses API has its own rule, and it is Pi's: a user message
/// whose content array is *empty* is dropped (`openai-responses-shared.ts:206`:
/// `if (content.length === 0) continue;`), while a user message that
/// carries an empty text block is kept — Pi sends the block as it is.
#[tokio::test]
async fn the_responses_dialect_follows_pis_empty_content_rule() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string("event: response.completed\ndata: {}\n\n".to_string()),
        )
        .mount(&server)
        .await;

    let provider = OpenAiResponsesProvider::new("test-key").with_base_url(server.uri());
    let mut stream: Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send>> =
        provider.stream(request_with(vec![
            contentless_user_turn(),
            empty_user_turn(),
        ]));
    while let Some(event) = stream.next().await {
        event.expect("the mock answers 200; there must be no error");
    }

    let requests = server.received_requests().await.expect("requests");
    let body: serde_json::Value =
        serde_json::from_slice(&requests[0].body).expect("the body must be JSON");
    let input = body["input"].as_array().expect("an input array");
    assert_eq!(
        input.len(),
        1,
        "the contentless user turn must be gone, the empty text one kept: {body}"
    );
    assert_eq!(input[0]["role"], "user", "{body}");
}
