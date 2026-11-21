//! provider 字节流的**空闲超时**保护。
//!
//! 为什么需要它（不是"以防万一"）：连接建立后上游一个字节都不发的情况下，
//! `response.bytes_stream().next()` 会永远挂着 —— reqwest 的 `timeout` 是
//! **总超时**（覆盖整个响应体），用它会杀掉健康的长流；所以只能在
//! **chunk 之间**做空闲判定。
//!
//! 判据只有一个：**上一次拿到 chunk 之后，又等了多久**。超过 [`DEFAULT_IDLE_TIMEOUT`]
//! 仍没有下一个 chunk，就判定上游挂死，把错误抛回 provider 层
//! （`fallback` 链因此可以切到下一个 provider）。

use std::time::Duration;

use futures_util::StreamExt;
use nini_core::provider::ProviderError;

/// 默认空闲超时：两个 chunk 之间超过这么久没有数据，就判定上游挂死。
///
/// 这个 120s 就是 `LivenessSettings::default()` 里
/// `provider_stream_idle_secs` 的默认值 —— 两处必须一致。
///
/// 取 120s 的理由：与 `nini_tools::bash` 的默认命令超时（120s）同量级；
/// 远大于任何健康流的最大 chunk 间隔（模型思考时也可能几十秒不吐字），
/// 又远小于"永远挂着"（用户视角：75s 实测无任何变化）。
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// 当前进程生效的 provider 流空闲超时。
///
/// 读的是进程级存活阈值（启动时由 nini-cli 从 settings.json 解析后写入）。
/// 注意：**已在飞行中的请求不受影响** —— 超时在调用 `next_chunk` 时就已经定下，
/// 改配置对下一次请求生效。
pub fn idle_timeout() -> Duration {
    Duration::from_secs(nini_core::liveness::liveness_settings().provider_stream_idle_secs)
}

/// 从 provider 的字节流里读下一个 chunk。
///
/// * 有数据 → `Ok(Some(chunk))`
/// * 上游正常结束 → `Ok(None)`
/// * 传输错误 → `Err(ProviderError::Http(..))`（与 `From<reqwest::Error>` 同口径）
/// * **空闲超时**（连接还在，但太久没有字节）→ `Err(ProviderError::Http(
///   "provider stream stalled: no data for 120s"))`
pub async fn next_chunk<S, T>(stream: &mut S, idle: Duration) -> Result<Option<T>, ProviderError>
where
    S: futures_core::Stream<Item = Result<T, reqwest::Error>> + Unpin,
{
    match tokio::time::timeout(idle, stream.next()).await {
        Ok(Some(Ok(item))) => Ok(Some(item)),
        Ok(Some(Err(e))) => Err(ProviderError::from(e)),
        Ok(None) => Ok(None),
        Err(_) => Err(ProviderError::Http(format!(
            "provider stream stalled: no data for {}s",
            idle.as_secs()
        ))),
    }
}

/// 发请求并等待响应头；超过 `idle` 仍没有响应头就判定上游挂死。
///
/// 为什么需要它：reqwest 的 `send()` 等的是**响应头**。上游连上但不回
/// 响应头时（连接已建立、请求已发出、服务端不吭声），`send()` 会永远
/// 挂着——那不是"chunk 之间的空闲"，是"还没进入流"，`next_chunk` 管不到。
///
/// 错误文案与 chunk 的那一句**刻意不同**，以便在转录里区分两种挂死：
///   * 响应头超时 → "provider response stalled: no headers for {n}s"
///   * chunk 空闲超时 → "provider stream stalled: no data for {n}s"
pub async fn send_with_idle_timeout(
    request: reqwest::RequestBuilder,
    idle: Duration,
) -> Result<reqwest::Response, ProviderError> {
    match tokio::time::timeout(idle, request.send()).await {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(e)) => Err(ProviderError::from(e)),
        Err(_) => Err(ProviderError::Http(format!(
            "provider response stalled: no headers for {}s",
            idle.as_secs()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// chunk 类型：单测里用 `Vec<u8>` 代替 reqwest 的 `Bytes`（泛型签名不挑字节类型）。
    type TestChunk = Vec<u8>;

    /// 造一个只吐 chunk、从不报错的流。
    fn chunk_stream(
        chunks: Vec<TestChunk>,
    ) -> impl futures_core::Stream<Item = Result<TestChunk, reqwest::Error>> + Unpin {
        stream::iter(chunks.into_iter().map(|c| Ok::<_, reqwest::Error>(c)))
    }

    /// 起一个只回一次 `429` 的极简 HTTP 服务，返回它的地址。
    ///
    /// 用它造一个**内容确定**的 `reqwest::Error`（status error，消息里必然带
    /// "429"），不去依赖"连不上"那种由系统文案决定的错误。
    async fn serve_429() -> String {
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
            // 先读掉请求头，免得客户端在等应答时把连接写满。
            let mut buf = [0u8; 4096];
            let _ = socket.read(&mut buf).await;
            let head = "HTTP/1.1 429 Too Many Requests\r\n\
                        Content-Length: 0\r\n\
                        Connection: close\r\n\
                        \r\n";
            let _ = socket.write_all(head.as_bytes()).await;
            let _ = socket.flush().await;
        });
        format!("http://{addr}/")
    }

    /// 起一个**accept 后永不回响应头**的极简服务，返回它的地址。
    ///
    /// 用来复现第一期设计的缺口：连接建立、请求发出，服务端就是不吭声，
    /// reqwest 卡在 `send()` 等响应头。
    async fn serve_no_headers() -> String {
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
            // 先读掉请求头，免得客户端在等应答时把连接写满。
            let mut buf = [0u8; 4096];
            let _ = socket.read(&mut buf).await;
            // 关键：**一个字节都不写**，只把 socket 攥在手里直到本进程结束。
            // 若此时 drop 掉 socket，连接会被关闭，reqwest 会立刻报
            // "connection closed" 之类的传输错误，而不是超时——那就测不到
            // "响应头挂死"这条路径了。
            tokio::time::sleep(Duration::from_secs(30)).await;
        });
        format!("http://{addr}/")
    }

    /// 起一个只回一次 `200 OK` 的极简服务，返回它的地址。
    async fn serve_200() -> String {
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
        format!("http://{addr}/")
    }

    /// 正常流：依次拿到所有 chunk。
    #[tokio::test]
    async fn a_normal_stream_yields_every_chunk() {
        let mut s = chunk_stream(vec![vec![1u8], vec![2u8], vec![3u8]]);

        let mut got = Vec::new();
        while let Some(chunk) = next_chunk(&mut s, Duration::from_secs(1))
            .await
            .expect("a normal stream must not fail")
        {
            got.push(chunk);
        }

        assert_eq!(got, vec![vec![1u8], vec![2u8], vec![3u8]]);
    }

    /// 流结束：`Ok(None)`。
    #[tokio::test]
    async fn a_finished_stream_returns_none() {
        let mut s = chunk_stream(vec![]);

        let out = next_chunk(&mut s, Duration::from_secs(1))
            .await
            .expect("a finished stream must not fail");

        assert!(out.is_none(), "a finished stream must yield None");
    }

    /// 流本身报错 → `ProviderError::Http`，消息含上游原因。
    #[tokio::test]
    async fn a_stream_error_becomes_an_http_error() {
        let url = serve_429().await;
        let response = reqwest::get(&url)
            .await
            .expect("the mock server must answer");
        let err = response
            .error_for_status()
            .expect_err("429 must be an error");

        let mut s = stream::iter(vec![Err::<TestChunk, reqwest::Error>(err)]);

        match next_chunk(&mut s, Duration::from_secs(1)).await {
            Err(ProviderError::Http(msg)) => assert!(
                msg.contains("429"),
                "the upstream reason must survive the conversion: {msg}"
            ),
            other => panic!("expected ProviderError::Http, got {other:?}"),
        }
    }

    /// 流沉默 → `ProviderError::Http`，消息含 "stalled"。
    #[tokio::test]
    async fn a_silent_stream_reports_a_stall() {
        let mut s = Box::pin(stream::pending::<Result<TestChunk, reqwest::Error>>());

        let err = next_chunk(&mut s, Duration::from_secs(1))
            .await
            .expect_err("a silent stream must time out");

        match err {
            ProviderError::Http(msg) => {
                assert!(
                    msg.contains("stalled"),
                    "the message must say the stream stalled: {msg}"
                );
                assert_eq!(
                    msg, "provider stream stalled: no data for 1s",
                    "the stall message must name the idle budget"
                );
            }
            other => panic!("expected ProviderError::Http, got {other:?}"),
        }
    }

    /// 慢但有数据（间隔小于 idle）→ 不误报。
    #[tokio::test]
    async fn a_slow_but_alive_stream_is_not_a_stall() {
        // 每 100ms 吐一个 chunk，间隔（100ms）远小于 idle（1s）。
        let mut s = Box::pin(stream::unfold(0u8, |count| async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            Some((Ok::<_, reqwest::Error>(vec![count + 1]), count + 1))
        }));

        for expected in 1u8..=3 {
            let chunk = next_chunk(&mut s, Duration::from_secs(1))
                .await
                .expect("a slow but alive stream must not fail")
                .expect("a slow but alive stream must yield chunks");
            assert_eq!(chunk, vec![expected]);
        }
    }

    /// 上游连上却永不回响应头 → `ProviderError::Http`，消息含 "stalled" 与 "no headers"。
    #[tokio::test]
    async fn a_server_without_headers_reports_a_stall() {
        let url = serve_no_headers().await;
        let request = reqwest::Client::new().get(&url);

        let err = send_with_idle_timeout(request, Duration::from_secs(1))
            .await
            .expect_err("a silent server must time out");

        match err {
            ProviderError::Http(msg) => {
                assert!(
                    msg.contains("stalled"),
                    "the message must say the response stalled: {msg}"
                );
                assert!(
                    msg.contains("no headers"),
                    "the message must say no headers arrived: {msg}"
                );
            }
            other => panic!("expected ProviderError::Http, got {other:?}"),
        }
    }

    /// 进程级的存活阈值 → `idle_timeout()` 跟着变。
    ///
    /// 测完把全局改回原值（并行测试共享同一份进程级状态）。
    #[test]
    fn the_idle_timeout_follows_the_process_wide_settings() {
        let original = nini_core::liveness::liveness_settings();
        nini_core::liveness::set_liveness_settings(nini_core::liveness::LivenessSettings {
            provider_stream_idle_secs: 7,
            ..original
        });

        let timeout = idle_timeout();

        // 先恢复全局再断言：断言失败也不会污染同 crate 的其它测试。
        nini_core::liveness::set_liveness_settings(original);

        assert_eq!(timeout, Duration::from_secs(7));
    }

    /// 正常回响应头 → `Ok(response)`，不误报。
    #[tokio::test]
    async fn a_server_answering_headers_is_not_a_stall() {
        let url = serve_200().await;
        let request = reqwest::Client::new().get(&url);

        let response = send_with_idle_timeout(request, Duration::from_secs(1))
            .await
            .expect("a healthy server must not be reported as stalled");

        assert_eq!(
            response.status().as_u16(),
            200,
            "the response must be the one the server sent"
        );
    }
}
