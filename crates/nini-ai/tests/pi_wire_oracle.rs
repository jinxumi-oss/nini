//! 链② 第一批 oracle：真实 Pi 0.84.2 发出的 tool_call 流，逐事件断言。
//!
//! # 为什么这个文件存在
//!
//! `openai_wire_e2e.rs` 的输入是 **hand-built chunks**——这一点它自己在
//! 第 18 行的注释里承认了（`or hand-built`）。手搓样例有一个无法绕过的
//! 循环论证：**样例和解析器出自同一次理解，所以"我以为 SSE 长这样"的
//! 错误，样例会跟着一起错。** 链② 审计（`docs/invariant-audit.md` §4）
//! 在 `nini-ai` 找到的 4 条无保护缺陷，全部是这一类。
//!
//! # 这个文件的输入不是手搓的
//!
//! `fixtures/pi-0.84.2-openai-inline-args-toolcall.sse` 是用
//! `scripts/capture-pi-sse.sh` 在本机抓下来的**原始字节**，未经编辑：
//! Pi 0.84.2 → `scripts/capture-sse.py`(:9878) → cloop-failover(:9877)
//! → MiniMax-M3.1-Flash-Preview，prompt 要求它执行 `echo nini-sse-capture`。
//!
//! 真流的形态（这是全部断言的依据，逐字节来自 fixture）：
//!
//! ```text
//! [0] delta 只有 role            —— content / tool_calls 字段不存在
//! [1] index:0 id name:"bash" arguments:"{"     ← inline-args：name 与首段 args 同一事件
//! [2] index:0        arguments 增量             ← 无 id 无 name，必须继承 [1]
//! [3] index:0        arguments:"}"              ← 一个 JSON 被拆到 3 个事件
//! [4] finish_reason:"tool_calls"，tool_calls 为 null
//! [5] "choices":[] 的 usage-only 事件           —— 遍历 choices 不解包就 panic
//! ```
//!
//! # 复现
//!
//! ```sh
//! bash scripts/capture-pi-sse.sh          # 重新抓一份
//! python3 scripts/invariants.py           # 链③ 审计
//! cargo test -p nini-ai --test pi_wire_oracle
//! ```

//! invariant: c3944d13 —— 声明本文件守住了哪几条历史不变量。
//! 该标注让 scripts/invariants.py 认出这些断言；词法重叠看不出它们，
//! invariant: 7a71f5b3
//! invariant: 9c9783e3

#![allow(clippy::needless_return)]

use futures_core::Stream;
use futures_util::StreamExt;
use nini_ai::openai_compat::OpenAiCompatProvider;
use nini_core::provider::{ContentBlock, Message, Provider, Request, Role, StreamEvent};
use std::pin::Pin;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// 未经编辑的真 Pi 输出。改了它，这条 oracle 就退化成 hand-built 了。
const PI_REAL_SSE: &str = include_str!("fixtures/pi-0.84.2-openai-inline-args-toolcall.sse");

/// 把 fixture 原样喂给 provider，走真实 `stream()`：真 HTTP、真 SSE 字节、
/// 真解析、真 finalize。
async fn events_from_capture(sse: &str) -> Vec<StreamEvent> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse.to_string()),
        )
        .mount(&server)
        .await;

    let provider = OpenAiCompatProvider::new(server.uri(), "test-key");
    let req = Request {
        model: "test-model".to_string(),
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "run the echo".to_string(),
            }],
            timestamp: 0,
        }],
        tools: vec![],
        max_tokens: Some(256),
        temperature: None,
        system: None,
        reasoning: None,
    };

    let mut stream: Pin<
        Box<dyn Stream<Item = Result<StreamEvent, nini_core::provider::ProviderError>> + Send>,
    > = provider.stream(req);
    let mut out = Vec::new();
    while let Some(ev) = stream.next().await {
        out.push(ev.expect("provider yielded an error event"));
    }
    out
}

/// 只取 fixture 的 data 行，剥掉 `data: ` 前缀（保留 `[DONE]` 之外的原始 JSON）
fn data_lines(sse: &str) -> Vec<&str> {
    sse.lines()
        .filter_map(|l| l.strip_prefix("data: "))
        .filter(|l| *l != "[DONE]")
        .collect()
}

fn tool_starts(events: &[StreamEvent]) -> Vec<(String, String)> {
    events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::ToolCallStart { id, name } => Some((id.clone(), name.clone())),
            _ => None,
        })
        .collect()
}

fn tool_stops(events: &[StreamEvent]) -> Vec<(String, serde_json::Value)> {
    events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::ToolCallStop { id, input_json } => Some((id.clone(), input_json.clone())),
            _ => None,
        })
        .collect()
}

/// 真流里 `id` 与 `name` **只出现在第一个事件**，后两个增量事件既没有 id
/// 也没有 name（见 fixture 的 [2] [3]）。解析器必须把它们继承下来，否则
/// 调用方拿到的是三个互不相干的 tool call。
#[tokio::test]
async fn id_and_name_are_inherited_from_the_first_event() {
    let events = events_from_capture(PI_REAL_SSE).await;

    let starts = tool_starts(&events);
    assert_eq!(
        starts.len(),
        1,
        "真流只有一个 tool call，却收到 {} 次 Start：{events:?}",
        starts.len()
    );
    assert_eq!(
        starts[0].1, "bash",
        "第一个事件里 name 与 arguments 是同一个 inline 块，name 不能丢"
    );
    assert!(
        !starts[0].0.is_empty(),
        "第一个事件带 id，它是后续所有增量的归属键：{events:?}"
    );
}

/// `7a71f5b3` —— inline-args 的 tool_call 曾经只发 Start 不发 Stop，
/// 也曾经发两次 Start。真实流里 Start 与 Stop 各恰好一次。
#[tokio::test]
async fn an_inline_args_tool_call_starts_once_and_stops_once() {
    let events = events_from_capture(PI_REAL_SSE).await;
    assert_eq!(
        tool_starts(&events).len(),
        1,
        "inline-args 必须只发一次 Start（name 与 args 同事件不代表两次调用）：{events:?}"
    );
    assert_eq!(
        tool_stops(&events).len(),
        1,
        "必须恰好一次 Stop，否则调用方永远等不到可解析的参数：{events:?}"
    );
}

/// `c3944d13` —— **首个 chunk 的 tool_call args 被丢弃**。
///
/// 真流的第一个事件里 `arguments` 恰好是 `"{"`。丢掉它，后两个增量拼起来
/// 是 `"command": "echo nini-sse-capture"}`，不是合法 JSON，而流本身看起来
/// 一切正常——这正是它能躲过 hand-built 测试的原因：手搓样例通常把 args
/// 完整地塞进第一个事件，于是"只取有 name 的那个事件的首段"这种错误写法
/// 永远不会暴露。
#[tokio::test]
async fn the_first_deltas_lone_open_brace_survives_into_the_final_json() {
    let lines = data_lines(PI_REAL_SSE);
    let first_args = serde_json::from_str::<serde_json::Value>(lines[1])
        .expect("fixture line 2 is JSON")["choices"][0]["delta"]["tool_calls"][0]["function"]
        ["arguments"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        first_args, "{",
        "本 oracle 断言的就是这个孤零零的 '{{'；fixture 变了先问为什么"
    );

    let events = events_from_capture(PI_REAL_SSE).await;
    let stops = tool_stops(&events);
    assert_eq!(stops.len(), 1, "{events:?}");

    assert_eq!(
        stops[0].1,
        serde_json::json!({"command": "echo nini-sse-capture"}),
        "首块的 '{{' 丢了，跨 3 个事件的增量就拼不成合法 JSON。events: {events:?}"
    );
}

/// `9c9783e3` —— Stop 只能在**最后一个增量之后**发，且 id 必须与 Start 一致。
#[tokio::test]
async fn stop_arrives_after_the_last_delta_and_shares_the_starts_id() {
    let events = events_from_capture(PI_REAL_SSE).await;

    let start_idx = events
        .iter()
        .position(|e| matches!(e, StreamEvent::ToolCallStart { .. }))
        .expect("a tool call must start");
    let stop_idx = events
        .iter()
        .position(|e| matches!(e, StreamEvent::ToolCallStop { .. }))
        .expect("a tool call must stop");
    assert!(start_idx < stop_idx, "Stop 不能先于 Start：{events:?}");

    let deltas: Vec<&StreamEvent> = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::ToolCallDelta { .. }))
        .collect();
    assert!(
        !deltas.is_empty(),
        "真流把一个 JSON 拆到 3 个事件，至少应有 2 段 delta：{events:?}"
    );

    let start_id = match &events[start_idx] {
        StreamEvent::ToolCallStart { id, .. } => id.clone(),
        _ => unreachable!(),
    };
    let stop_id = match &events[stop_idx] {
        StreamEvent::ToolCallStop { id, .. } => id.clone(),
        _ => unreachable!(),
    };
    assert_eq!(
        start_id, stop_id,
        "增量事件不带 id，Stop 必须复用 Start 的 id，否则调用方无法归并：{events:?}"
    );
}

/// 抓包时看不到、但真流里确实存在的两个陷阱：
///
/// * `[0]` 的 delta 只有 `role`，`content` 与 `tool_calls` **字段不存在**
///   （不是 null，是缺失）——按必填字段解包的代码会 panic。
/// * `[5]` 是 `"choices":[]` 的 usage-only 事件——`choices[0]` 直接索引
///   的代码会 panic 或静默丢掉 usage。
#[tokio::test]
async fn a_usage_only_event_with_empty_choices_does_not_break_the_stream() {
    let lines = data_lines(PI_REAL_SSE);
    let last: serde_json::Value =
        serde_json::from_str(lines.last().expect("fixture has data lines")).expect("JSON");
    assert_eq!(
        last["choices"].as_array().map(Vec::len),
        Some(0),
        "本 oracle 断言的就是这条空 choices 的 usage 事件"
    );

    let events = events_from_capture(PI_REAL_SSE).await;
    assert_eq!(
        tool_stops(&events).len(),
        1,
        "空 choices 的尾事件不得让流提前结束或丢掉 Stop：{events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::MessageStop { .. })),
        "流必须正常收尾：{events:?}"
    );
}
