//! The server's own words must survive a failed request.
//!
//! ## What was broken
//!
//! Every provider answered a non-2xx response with
//! `format!("request failed: {}", status)` and **threw the response
//! body away**. A user saw
//!
//! ```text
//! api error (400): request failed: 400 Bad Request
//! ```
//!
//! — zero information — while the reason (`prompt is too long`,
//! `invalid api key`, `model not found`) sat unread in the body.
//!
//! Two things were broken by that, not one:
//!
//! 1. The user could not diagnose the failure.
//! 2. `nini_core::overflow::is_context_overflow()` recognises a context
//!    overflow by *matching the error text* (`"prompt is too long"`,
//!    `"context_length"`, …). With the body discarded the text never
//!    reached the detector, so a context overflow was indistinguishable
//!    from any other 400 and the compaction/retry path never ran.
//!
//! ## What these tests pin
//!
//! The server's wording must appear **verbatim** in the error message,
//! for every provider and every body shape. A regression that drops the
//! body again turns these red — the whole point of the file.
//!
//! ## What changed since this file was written (v0.10)
//!
//! Two things were added to the message, and the exact-equality
//! assertions below were updated to match:
//!
//! 1. **The provider's error code.** `{"error": {"code": 10907, "message":
//!    "Bad Request"}}` used to yield only "Bad Request" — the code was
//!    dropped, so a context overflow (`10907`) and a malformed request
//!    (`10005`) were indistinguishable. The code is now kept:
//!    `... : Bad Request (code 10907)`. The *verbatim* rule is
//!    untouched: the server's own words are still there, unchanged.
//! 2. **The provider layer carries facts only.** A 400 ends with the
//!    server's own words and the vendor's error code — and *nothing
//!    else*. The advice ("try `/compact`", "switch to a bigger model")
//!    belongs to the agent layer, which knows how long the
//!    conversation is; a provider-side hint only guessed at causes
//!    and duplicated the agent's own wording.
//!
//! ## The layer split (and why the hint was removed)
//!
//! `nini_core::agent::annotate_request_rejection(status, text)` appends
//! the actionable advice to the very same text, keyed on the HTTP
//! status it receives in `ProviderError::Api { status, message }`.
//! Both hints stacked up in the user's terminal:
//!
//! ```text
//! request failed: 400 Bad Request: Bad Request (code 10907) [hint: …] — hint: …
//! ```
//!
//! — long, repetitive, and the provider half was *wrong* about the
//! cause (the real failure was a context overflow, iFlytek `10907`).
//! The provider layer now says only what it knows, and the test
//! below pins that: no "the server rejected the request" may appear in
//! a provider-layer message.

use futures_core::Stream;
use futures_util::StreamExt;
use nini_ai::anthropic::AnthropicProvider;
use nini_ai::cohere::CohereProvider;
use nini_ai::deepseek::DeepSeekProvider;
use nini_ai::google::GoogleProvider;
use nini_ai::groq::GroqProvider;
use nini_ai::mistral::MistralProvider;
use nini_ai::openai_compat::OpenAiCompatProvider;
use nini_ai::openai_responses::OpenAiResponsesProvider;
use nini_core::provider::{
    ContentBlock, Message, Provider, ProviderError, Request, Role, StreamEvent,
};
use std::pin::Pin;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A request that is trivial but valid — the point of these tests is the
/// *response*, not the request.
fn sample_request() -> Request {
    Request {
        model: "test-model".to_string(),
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "hello".to_string(),
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

/// Mount a single mock answer on `path` and return the mock server.
async fn serve(status: u16, request_path: &str, body: &str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(request_path))
        .respond_with(ResponseTemplate::new(status).set_body_string(body.to_string()))
        .mount(&server)
        .await;
    server
}

/// Run one request through a provider and return the error it produced.
async fn error_from(provider: &dyn Provider) -> ProviderError {
    let mut stream: Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send>> =
        provider.stream(sample_request());
    while let Some(event) = stream.next().await {
        match event {
            Err(err) => return err,
            Ok(_) => continue,
        }
    }
    panic!("the provider produced no error at all");
}

/// The API error's status and message, or a panic if it was not an API error.
fn api_error(err: ProviderError) -> (u16, String) {
    match err {
        ProviderError::Api { status, message } => (status, message),
        other => panic!("expected an API error, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 1. The OpenAI shape: {"error": {"message": ...}}
// ---------------------------------------------------------------------------

/// The headline case: the server explains the 400, and the explanation
/// must reach the user.
#[tokio::test]
async fn the_openai_error_message_reaches_the_user() {
    let server = serve(
        400,
        "/v1/chat/completions",
        r#"{"error":{"message":"prompt is too long","type":"invalid_request_error","code":"context_length_exceeded"}}"#,
    )
    .await;

    let provider = OpenAiCompatProvider::new(server.uri(), "test-key");
    let (status, message) = api_error(error_from(&provider).await);

    assert_eq!(status, 400, "the real HTTP status must be preserved");
    assert!(
        message.contains("prompt is too long"),
        "the server's own wording was dropped: {message:?}"
    );
    assert_eq!(
        message,
        "request failed: 400 Bad Request: prompt is too long (code context_length_exceeded)",
        "the message format changed; the prefix must stay compatible"
    );
}

/// The overflow detector matches on text, so the *whole* sentence has to
/// be there — not only its first word.
#[tokio::test]
async fn the_full_overflow_sentence_is_preserved() {
    let server = serve(
        400,
        "/v1/chat/completions",
        r#"{"error":{"message":"prompt is too long: 300000 tokens > 200000"}}"#,
    )
    .await;

    let provider = OpenAiCompatProvider::new(server.uri(), "test-key");
    let (_, message) = api_error(error_from(&provider).await);

    assert!(
        message.contains("prompt is too long: 300000 tokens > 200000"),
        "the overflow sentence was mangled: {message:?}"
    );
    assert_eq!(
        message,
        "request failed: 400 Bad Request: prompt is too long: 300000 tokens > 200000"
    );
}

// ---------------------------------------------------------------------------
// 2. The gateway shape: {"message": ...} at the top level
// ---------------------------------------------------------------------------

/// A gateway (iFlytek-style) that reports the failure as a bare
/// top-level `message`.
#[tokio::test]
async fn a_top_level_message_reaches_the_user() {
    let server = serve(
        400,
        "/v1/chat/completions",
        r#"{"message":"invalid api key"}"#,
    )
    .await;

    let provider = OpenAiCompatProvider::new(server.uri(), "test-key");
    let (status, message) = api_error(error_from(&provider).await);

    assert_eq!(status, 400);
    assert!(
        message.contains("invalid api key"),
        "the top-level message was dropped: {message:?}"
    );
    assert_eq!(message, "request failed: 400 Bad Request: invalid api key");
}

// ---------------------------------------------------------------------------
// 3. 401 with an OpenAI/Anthropic error object
// ---------------------------------------------------------------------------

/// A rejected key must say *which* key problem it was.
#[tokio::test]
async fn an_authentication_error_reaches_the_user() {
    let server = serve(
        401,
        "/v1/messages",
        r#"{"type":"error","error":{"type":"authentication_error","message":"invalid_api_key"}}"#,
    )
    .await;

    let provider = AnthropicProvider::new("test-key").with_base_url(server.uri());
    let (status, message) = api_error(error_from(&provider).await);

    assert_eq!(status, 401, "the real HTTP status must be preserved");
    assert!(
        message.contains("invalid_api_key"),
        "the authentication error was dropped: {message:?}"
    );
    assert_eq!(message, "request failed: 401 Unauthorized: invalid_api_key");
}

/// The same for the OpenAI Responses endpoint.
#[tokio::test]
async fn an_responses_api_authentication_error_reaches_the_user() {
    let server = serve(
        401,
        "/v1/responses",
        r#"{"error":{"message":"invalid_api_key","type":"authentication_error"}}"#,
    )
    .await;

    let provider = OpenAiResponsesProvider::new("test-key").with_base_url(server.uri());
    let (status, message) = api_error(error_from(&provider).await);

    assert_eq!(status, 401);
    assert!(
        message.contains("invalid_api_key"),
        "the authentication error was dropped: {message:?}"
    );
}

// ---------------------------------------------------------------------------
// 4. A plain-text body
// ---------------------------------------------------------------------------

/// Some proxies answer with text, not JSON. It must still be reported.
#[tokio::test]
async fn a_plain_text_body_reaches_the_user() {
    let server = serve(500, "/v1/chat/completions", "upstream model crashed").await;

    let provider = OpenAiCompatProvider::new(server.uri(), "test-key");
    let (status, message) = api_error(error_from(&provider).await);

    assert_eq!(status, 500);
    assert!(
        message.contains("upstream model crashed"),
        "the plain-text body was dropped: {message:?}"
    );
    assert_eq!(
        message,
        "request failed: 500 Internal Server Error: upstream model crashed"
    );
}

// ---------------------------------------------------------------------------
// 5. An empty body: the old message, unchanged
// ---------------------------------------------------------------------------

/// Nothing to say → nothing added. The old format must survive exactly,
/// so every existing assertion (and the log format) still holds.
#[tokio::test]
async fn an_empty_body_falls_back_to_the_plain_message() {
    let server = serve(400, "/v1/chat/completions", "").await;

    let provider = OpenAiCompatProvider::new(server.uri(), "test-key");
    let (status, message) = api_error(error_from(&provider).await);

    assert_eq!(status, 400);
    assert_eq!(
        message, "request failed: 400 Bad Request",
        "an empty body must not change the message"
    );
}

/// The same for the Anthropic dialect.
#[tokio::test]
async fn an_empty_body_falls_back_to_the_plain_message_for_anthropic() {
    let server = serve(400, "/v1/messages", "").await;

    let provider = AnthropicProvider::new("test-key").with_base_url(server.uri());
    let (status, message) = api_error(error_from(&provider).await);

    assert_eq!(status, 400);
    assert_eq!(message, "request failed: 400 Bad Request");
}

// ---------------------------------------------------------------------------
// 6. The compat brands inherit the fix
// ---------------------------------------------------------------------------

/// The OpenAI-compat brands (DeepSeek, Groq, Mistral, Cohere, Google)
/// are thin wrappers over the OpenAI provider, so they inherit the fix.
/// Each one is checked, because a wrapper that grew its own HTTP call
/// would silently reintroduce the bug.
#[tokio::test]
async fn every_openai_compat_brand_reports_the_server_text() {
    let server = serve(
        404,
        "/v1/chat/completions",
        r#"{"error":{"message":"model not found"}}"#,
    )
    .await;
    let uri = server.uri();

    let providers: Vec<(&str, Box<dyn Provider>)> = vec![
        (
            "openai-compat",
            Box::new(OpenAiCompatProvider::new(uri.clone(), "k")),
        ),
        (
            "deepseek",
            Box::new(DeepSeekProvider::with_base_url(uri.clone(), "k")),
        ),
        (
            "groq",
            Box::new(GroqProvider::with_base_url(uri.clone(), "k")),
        ),
        (
            "mistral",
            Box::new(MistralProvider::with_base_url(uri.clone(), "k")),
        ),
        (
            "cohere",
            Box::new(CohereProvider::with_base_url(uri.clone(), "k")),
        ),
        (
            "google",
            Box::new(GoogleProvider::with_base_url(uri.clone(), "k")),
        ),
    ];

    for (name, provider) in providers {
        let (status, message) = api_error(error_from(provider.as_ref()).await);
        assert_eq!(status, 404, "{name}: the real HTTP status must survive");
        assert!(
            message.contains("model not found"),
            "{name}: the server's text was dropped: {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 7. A hostile body must not become a wall of text
// ---------------------------------------------------------------------------

/// A gateway's HTML error page must not be pasted into the message.
///
/// The reason is not politeness: the message ends up in a terminal, in
/// logs and in the transcript. A whole page is unreadable there.
#[tokio::test]
async fn an_html_error_page_is_reduced_to_its_text() {
    let server = serve(
        502,
        "/v1/chat/completions",
        "<html><head><title>502</title></head><body><h1>502 Bad Gateway</h1><p>nginx</p></body></html>",
    )
    .await;

    let provider = OpenAiCompatProvider::new(server.uri(), "test-key");
    let (_, message) = api_error(error_from(&provider).await);

    assert!(
        message.contains("502 Bad Gateway"),
        "the useful text was lost: {message:?}"
    );
    assert!(
        !message.contains('<'),
        "raw markup leaked into the message: {message:?}"
    );
    assert!(
        message.len() < 1024,
        "the message is far too long for an error: {} bytes",
        message.len()
    );
}

/// A huge body must be cut down, and the cut must be visible.
#[tokio::test]
async fn a_huge_body_is_truncated() {
    let server = serve(
        400,
        "/v1/chat/completions",
        &format!(r#"{{"error":{{"message":"{}"}}}}"#, "x".repeat(50_000)),
    )
    .await;

    let provider = OpenAiCompatProvider::new(server.uri(), "test-key");
    let (_, message) = api_error(error_from(&provider).await);

    assert!(
        message.len() <= nini_ai::error_body::MAX_COMPOSED_MESSAGE_BYTES,
        "the message was not capped: {} bytes",
        message.len()
    );
    assert!(
        message.contains("truncated"),
        "the truncation must be visible: {message:?}"
    );
}

// ---------------------------------------------------------------------------
// 8. The provider layer states facts, the agent layer states advice
// ---------------------------------------------------------------------------

/// The provider layer must not carry the agent layer's wording.
///
/// `nini_core::agent::annotate_request_rejection(status, text)` appends
/// the actionable advice ("try `/compact`, shrink the task, or switch
/// to a model that accepts more") to the text the provider hands it —
/// it keys on the HTTP status that arrives in
/// `ProviderError::Api { status, message }`, so it needs nothing from
/// the provider but the facts.
///
/// While the provider appended a hint of its own, the user saw both:
///
/// ```text
/// request failed: 400 Bad Request: Bad Request (code 10907) [hint: the server
/// rejected the request; an empty user message, a wrong model id, …] — hint: the
/// server rejected the request; the conversation may be longer than the model accepts …
/// ```
///
/// Two sentences that say the same thing, and the first one *guesses*
/// the cause — the real cause of that failure was a context overflow
/// (iFlytek `10907`), which the provider layer cannot know.
///
/// This test is the guard: reintroduce the provider-side hint and it
/// goes red.
#[tokio::test]
async fn the_provider_layer_adds_no_advice_of_its_own() {
    let server = serve(
        400,
        "/v1/chat/completions",
        r#"{"error":{"code":10907,"message":"Bad Request"}}"#,
    )
    .await;

    let provider = OpenAiCompatProvider::new(server.uri(), "test-key");
    let (status, message) = api_error(error_from(&provider).await);

    assert_eq!(status, 400);
    assert!(
        !message.contains("the server rejected the request"),
        "the provider layer must not repeat the agent layer's advice: {message:?}"
    );
    for advice in ["hint:", "/compact", "switch to a model"] {
        assert!(
            !message.contains(advice),
            "the provider layer must not give advice ({advice}): {message:?}"
        );
    }
    // The facts are still all there.
    assert_eq!(
        message, "request failed: 400 Bad Request: Bad Request (code 10907)",
        "the facts must survive the removal of the hint"
    );
}

/// The same for a body-less 400 and for a 400 that carries only a code.
#[tokio::test]
async fn the_provider_layer_adds_no_advice_to_any_400() {
    for body in ["", r#"{"code":10005,"message":"invalid request"}"#] {
        let server = serve(400, "/v1/chat/completions", body).await;
        let provider = OpenAiCompatProvider::new(server.uri(), "test-key");
        let (_, message) = api_error(error_from(&provider).await);
        assert!(
            !message.contains("the server rejected the request"),
            "the provider layer must not repeat the agent layer's advice: {message:?}"
        );
        assert!(
            !message.contains("hint:"),
            "no hint may survive: {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 9. A body that carries only a code: the code must survive
// ---------------------------------------------------------------------------

/// iFlytek answers some failures with a body that carries **only** a
/// code — no message at all:
///
/// ```text
/// {"code":10907}
/// ```
///
/// Before the fix the message search found nothing, the body fell
/// through to the plain-text path, and the *whole JSON* became the
/// message: `request failed: 400 Bad Request: {"code":10907}`. The
/// code — the one thing that says "this was a context overflow" — was
/// there for a human to read, but the machine-readable form the vendor
/// extractor needs (`(code 10907)`) was gone.
#[tokio::test]
async fn a_body_with_only_a_code_keeps_the_code() {
    let server = serve(400, "/v1/chat/completions", r#"{"code":10907}"#).await;

    let provider = OpenAiCompatProvider::new(server.uri(), "test-key");
    let (status, message) = api_error(error_from(&provider).await);

    assert_eq!(status, 400);
    assert!(message.contains("10907"), "the code was lost: {message:?}");
    assert!(
        !message.contains("{\"code\""),
        "the raw JSON became the message: {message:?}"
    );
    assert_eq!(
        message, "request failed: 400 Bad Request: (code 10907)",
        "the code must be labelled, not bare"
    );
}

/// The same for the string form of a code.
#[tokio::test]
async fn a_body_with_only_a_string_code_keeps_the_code() {
    let server = serve(
        400,
        "/v1/chat/completions",
        r#"{"code":"context_length_exceeded"}"#,
    )
    .await;

    let provider = OpenAiCompatProvider::new(server.uri(), "test-key");
    let (_, message) = api_error(error_from(&provider).await);

    assert!(
        message.contains("context_length_exceeded"),
        "the code was lost: {message:?}"
    );
    assert!(
        !message.contains("{\"code\""),
        "the raw JSON became the message: {message:?}"
    );
    assert_eq!(
        message,
        "request failed: 400 Bad Request: (code context_length_exceeded)"
    );
}
