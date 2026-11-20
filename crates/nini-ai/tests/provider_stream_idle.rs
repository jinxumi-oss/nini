//! provider 字节流的空闲超时：跑在**真实 reqwest 字节流**上的集成测试。
//!
//! ## 背景
//!
//! 上游"连上了但一个字节都不发"时，`response.bytes_stream().next()` 会永远挂着：
//! reqwest 的 `timeout` 是**总超时**（覆盖整个响应体），用它会杀掉健康的长流，
//! 所以只能在 **chunk 之间**做空闲判定（见 `nini_ai::stream_idle`）。
//!
//! ## 这些测试钉住什么
//!
//! * 服务端先发一个 chunk 然后挂住 → 第一个 chunk 正常拿到；第二个 chunk 永远不来，
//!   必须在空闲窗口之后报 `ProviderError::Http(..)`，且消息里带 `"stalled"`。
//! * 服务端正常结束 → 读到 `Ok(None)`，不能被当成挂死。
//!
//! 断言只针对公开 API 的返回值，不碰内部实现。

use std::time::{Duration, Instant};

use nini_ai::stream_idle::{next_chunk, send_with_idle_timeout};
use nini_core::provider::ProviderError;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// 空闲窗口：测试必须快，所以用 200ms 级别，而不是生产用的 120s 默认值。
const IDLE: Duration = Duration::from_millis(200);

/// 等响应头的空闲窗口：同样用 300ms 级别，测试必须快。
const HEADERS_IDLE: Duration = Duration::from_millis(300);

/// 服务端发来的第一个 SSE chunk（两条测试共用同一份载荷）。
const FIRST_CHUNK: &[u8] = b"data: {\"delta\":\"one\"}\n\n";

/// 起一个 `text/event-stream` 服务端：**先发一个 chunk，然后永不结束**（服务端挂住）。
///
/// 为什么这里不是 wiremock：wiremock 0.6 只能回一个**固定长度**的 body
/// （`ResponseTemplate` 没有流式 body），而"发完一帧就再也不发"只能在 socket 层面
/// 做到；hyper 见到"声明的 content-length 与实际 body 不一致"会直接 panic，
/// 所以这条路在 wiremock 上走不通。本 crate 里已有的先例是 `error_body_timeout.rs`
/// ——它用裸 `TcpListener` 起一个"body 永远不结束"的响应，这里照抄同一套写法。
async fn serve_stream_that_stalls() -> String {
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
        // 先把请求读掉，服务端才回答。
        let mut buf = [0u8; 4096];
        let _ = socket.read(&mut buf).await;
        let head = "HTTP/1.1 200 OK\r\n\
                    Content-Type: text/event-stream\r\n\
                    Transfer-Encoding: chunked\r\n\
                    \r\n";
        let _ = socket.write_all(head.as_bytes()).await;
        // 写一帧 chunked body（这一帧就是上面那个 SSE chunk），然后**再也不写**。
        let mut frame = format!("{:x}\r\n", FIRST_CHUNK.len()).into_bytes();
        frame.extend_from_slice(FIRST_CHUNK);
        frame.extend_from_slice(b"\r\n");
        let _ = socket.write_all(&frame).await;
        let _ = socket.flush().await;
        // 连接一直开着：一个字节都不再发 —— 上游挂死。
        tokio::time::sleep(Duration::from_secs(120)).await;
        drop(socket);
    });
    format!("http://{addr}")
}

/// 挂死的上游必须变成一个**明确的错误**，而不是永远等下去。
#[tokio::test]
async fn a_stalled_provider_stream_is_reported_as_stalled() {
    let base = serve_stream_that_stalls().await;
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{base}/v1/chat/completions"))
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .expect("the mock server must answer");
    let status = response.status();
    assert!(
        status.is_success(),
        "the mock server must answer with a success status, got {status}"
    );

    let mut stream = response.bytes_stream();

    // 第一个 chunk 正常到达。
    let first = next_chunk(&mut stream, IDLE)
        .await
        .expect("the first chunk must not fail");
    let first = first.expect("the first chunk must arrive before the stall");
    let text = String::from_utf8_lossy(&first);
    assert!(
        text.contains("one"),
        "the first chunk must be the one the server sent: {text:?}"
    );

    // 第二个 chunk 永远不来 → 空闲窗口之后必须报错，而且必须说清是"挂住了"。
    let second = next_chunk(&mut stream, IDLE).await;
    let err = second.expect_err("a stream that never sends again must not wait forever");
    let message = match &err {
        ProviderError::Http(message) => message,
        other => panic!("expected an HTTP error, got {other:?}"),
    };
    assert!(
        message.contains("stalled"),
        "the stall must be named in the error message: {message:?}"
    );
}

/// 正常结束的流必须读成"结束"，不能被当成挂死。
#[tokio::test]
async fn a_finished_provider_stream_ends_instead_of_stalling() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            String::from_utf8_lossy(FIRST_CHUNK).to_string(),
            "text/event-stream",
        ))
        .mount(&server)
        .await;

    let client = reqwest::Client::new();
    let response = client
        .post(format!("{}/v1/chat/completions", server.uri()))
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .expect("the mock server must answer");

    let mut stream = response.bytes_stream();

    let first = next_chunk(&mut stream, IDLE)
        .await
        .expect("the chunk must not fail");
    assert!(
        first.is_some(),
        "the body must be delivered before the stream ends"
    );

    let end = next_chunk(&mut stream, IDLE).await;
    match end {
        Ok(None) => {}
        other => panic!("a finished stream must read as the end, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 响应头这一段：请求还没拿到响应，就卡在 `send().await` 里。
// ---------------------------------------------------------------------------

/// 起一个裸 TCP 服务端：accept 之后**一个字节都不写**，就那么挂着。
///
/// 这是"上游连上了但连响应头都不发"的最小复现：reqwest 卡在 `send().await`
/// 等响应头，压根进不到字节流那一步，所以 `next_chunk` 的空闲判定救不了它。
/// 同样照抄 `error_body_timeout.rs` 的裸 `TcpListener` 写法。
async fn serve_headers_that_never_arrive() -> String {
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
        // 把请求读掉，但**永远不写响应**：连响应头都没有。
        let mut buf = [0u8; 4096];
        let _ = socket.read(&mut buf).await;
        tokio::time::sleep(Duration::from_secs(120)).await;
        drop(socket);
    });
    format!("http://{addr}")
}

/// 起一个裸 TCP 服务端：正常回一个最小的 `200` 响应（`Content-Length: 0`）。
///
/// 用来证明同一个等待路径在健康上游上**不会**误报挂死。
async fn serve_a_minimal_ok_response() -> String {
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
        let mut buf = [0u8; 4096];
        let _ = socket.read(&mut buf).await;
        let head = "HTTP/1.1 200 OK\r\n\
                    Content-Length: 0\r\n\
                    Connection: close\r\n\
                    \r\n";
        let _ = socket.write_all(head.as_bytes()).await;
        let _ = socket.flush().await;
    });
    format!("http://{addr}")
}

/// 上游连上却连响应头都不发 → 必须变成明确的错误，而不是永远等下去。
#[tokio::test]
async fn a_server_that_never_sends_headers_is_reported_as_stalled() {
    let base = serve_headers_that_never_arrive().await;
    let client = reqwest::Client::new();

    let started = Instant::now();
    // 外层超时是**测试的耐心**：等待响应头这一段要是有死路，测试失败而不是挂死。
    let outcome = tokio::time::timeout(
        Duration::from_secs(10),
        send_with_idle_timeout(client.get(base.as_str()), HEADERS_IDLE),
    )
    .await
    .expect("the request hung: waiting for the response headers has no deadline");

    let err = outcome.expect_err("a server that never sends headers must not look like a success");
    let message = match &err {
        ProviderError::Http(message) => message,
        other => panic!("expected an HTTP error, got {other:?}"),
    };
    assert!(
        message.contains("stalled"),
        "the stall must be named in the error message: {message:?}"
    );
    assert!(
        message.contains("no headers"),
        "the error must say that no headers arrived: {message:?}"
    );

    let elapsed = started.elapsed();
    assert!(
        elapsed >= HEADERS_IDLE,
        "the deadline must actually be waited for ({elapsed:?} < {HEADERS_IDLE:?}): \
         the request must not fail before the idle budget"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "the error took too long to come back: {elapsed:?}"
    );
}

/// 正常回一个响应 → 不能被误判成挂死。
#[tokio::test]
async fn a_normal_response_is_not_mistaken_for_a_stall() {
    let base = serve_a_minimal_ok_response().await;
    let client = reqwest::Client::new();

    let outcome = tokio::time::timeout(
        Duration::from_secs(10),
        send_with_idle_timeout(client.get(base.as_str()), HEADERS_IDLE),
    )
    .await
    .expect("the request hung");

    let response = outcome.expect("a normal response must not be reported as a stall");
    let status = response.status();
    assert!(
        status.is_success(),
        "the mock server must answer with a success status, got {status}"
    );
}
