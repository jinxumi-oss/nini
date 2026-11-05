//! Reading a failed response's body must never be able to hang nini.
//!
//! ## What was broken
//!
//! `crate::error_body::extract_error_detail` read the body of a failed
//! response with `response.chunk().await` and **no deadline**. The
//! providers build their HTTP client with `Client::new()`, whose
//! `timeout` is `None` — so nothing bounded the read either.
//!
//! A server that answers `400` and then keeps the body open (a chunked
//! response that never terminates, a proxy that streams an error page
//! lazily) therefore made nini wait forever. That is **strictly worse
//! than the behaviour before the body was read at all**: the error was
//! known the moment the status line arrived, and it used to come back
//! at once.
//!
//! ## What these tests pin
//!
//! * A body that never ends is abandoned after
//!   `nini_ai::error_body::ERROR_BODY_READ_TIMEOUT`.
//! * The request still fails — with a *usable* message
//!   (`request failed: 400 Bad Request`), not with a hang and not with a
//!   panic.
//! * The deadline is a property of the code, not of the test's
//!   patience: the test fails (rather than hangs) when it is removed.

use futures_core::Stream;
use futures_util::StreamExt;
use nini_ai::anthropic::AnthropicProvider;
use nini_ai::error_body::ERROR_BODY_READ_TIMEOUT;
use nini_ai::openai_compat::OpenAiCompatProvider;
use nini_core::provider::{
    ContentBlock, Message, Provider, ProviderError, Request, Role, StreamEvent,
};
use std::pin::Pin;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

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

/// Serve a `400` whose body **never ends**.
///
/// The socket is kept open on purpose: the response announces a
/// `Content-Length` it never delivers, so a reader that waits for the
/// body waits forever. A reader with a deadline gives up; one without
/// hangs — which is exactly the difference these tests measure.
async fn serve_body_that_never_ends() -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the mock server must bind");
    let addr = listener
        .local_addr()
        .expect("the mock server must have an address");
    tokio::spawn(async move {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        // Read (part of) the request, so the server does not answer
        // before the client has spoken.
        let mut buf = [0u8; 4096];
        let _ = socket.read(&mut buf).await;
        let head = "HTTP/1.1 400 Bad Request\r\n\
                    Content-Type: text/plain\r\n\
                    Content-Length: 512\r\n\
                    \r\n\
                    partial error text";
        let _ = socket.write_all(head.as_bytes()).await;
        let _ = socket.flush().await;
        // The body never ends: hold the connection open.
        tokio::time::sleep(Duration::from_secs(120)).await;
        drop(socket);
    });
    format!("http://{addr}")
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

/// The headline case: a body that never ends must not hang the request.
#[tokio::test]
async fn a_body_that_never_ends_does_not_hang_the_request() {
    let base = serve_body_that_never_ends().await;
    let provider = OpenAiCompatProvider::new(base, "test-key");

    let started = Instant::now();
    // The outer deadline is the *test's* patience: if the provider
    // hangs, the test fails with a clear message instead of hanging
    // until the harness gives up.
    let outcome = tokio::time::timeout(Duration::from_secs(30), error_from(&provider)).await;
    let elapsed = started.elapsed();

    let err = outcome.expect(
        "the request hung: the error body read has no deadline, so a body that never \
         ends blocks the error forever",
    );
    let (status, message) = api_error(err);

    assert_eq!(status, 400, "the real HTTP status must be preserved");
    assert_eq!(
        message, "request failed: 400 Bad Request",
        "a body that never ends must degrade to the plain message"
    );
    assert!(
        elapsed < Duration::from_secs(15),
        "the error took too long to come back: {elapsed:?}"
    );
    assert!(
        elapsed >= ERROR_BODY_READ_TIMEOUT,
        "the body was not waited for at all ({elapsed:?} < {ERROR_BODY_READ_TIMEOUT:?}): \
         the read must actually block on the never-ending body"
    );
}

/// The same for the Anthropic dialect — the deadline lives in the
/// shared helper, so every provider that reads an error body inherits
/// it.
#[tokio::test]
async fn a_body_that_never_ends_does_not_hang_the_anthropic_request() {
    let base = serve_body_that_never_ends().await;
    let provider = AnthropicProvider::new("test-key").with_base_url(base);

    let started = Instant::now();
    let outcome = tokio::time::timeout(Duration::from_secs(30), error_from(&provider)).await;
    let elapsed = started.elapsed();

    let err = outcome.expect("the request hung: the error body read has no deadline");
    let (status, message) = api_error(err);

    assert_eq!(status, 400);
    assert_eq!(message, "request failed: 400 Bad Request");
    assert!(
        elapsed < Duration::from_secs(15),
        "the error took too long to come back: {elapsed:?}"
    );
}

/// The deadline is short enough to be worth having: it is a couple of
/// seconds, not a minute. A regression that raised it to "practically
/// forever" must be visible here.
#[test]
fn the_error_body_deadline_is_short() {
    assert!(
        ERROR_BODY_READ_TIMEOUT <= Duration::from_secs(5),
        "the error-body deadline must stay in the 2–5 s range: {ERROR_BODY_READ_TIMEOUT:?}"
    );
    assert!(
        ERROR_BODY_READ_TIMEOUT >= Duration::from_secs(1),
        "the error-body deadline must not be so short that a slow gateway is cut off: \
         {ERROR_BODY_READ_TIMEOUT:?}"
    );
}
