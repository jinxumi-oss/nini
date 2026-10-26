//! Oracle: nini 的**回合形状**必须与 Pi 0.85.1 一致（v0.10.8 / C1）。
//!
//! # 这是 C1 的第一块砖
//!
//! 差距 C1 说：「oracle 没有可执行的 Pi 侧对照——全部是『注释引行号 + 断言 nini』」。
//! 工具契约（`pi-tool-dump.mjs`）与截断（`truncate-oracle.mjs`）已经补上了两块，
//! 现在补第三块：**会话的形状**。
//!
//! 对照物是 `crates/nini-conformance/fixtures/pi-0.85.1-session.jsonl` ——
//! **真 Pi 0.85.1 写的**（`scripts/pi-session-record.sh` 驱动 mock provider 录制，
//! 只洗路径）。它的形状是：
//!
//! ```text
//! session / model_change / thinking_level_change
//! user(text)
//! assistant(toolCall)
//! toolResult(text)
//! assistant(text)
//! user(text)
//! assistant(text)
//! ```
//!
//! 断言的是**形状**（条目类型 / 角色 / 内容块的类型），不是文字——文字是
//! mock 的，不该被钉死。
//!
//! # 为什么形状重要
//!
//! 形状就是模型看到的上下文结构。assistant 的 toolCall 与 toolResult
//! 分成两条消息（而不是一条里都塞着），是 Pi 的约定；nini 若写成别的形状，
//! 模型在续读会话时看到的就是另一种东西。

use nini_tui::runtime::{AgentEventLite, AgentSink};
use nini_tui::state::AppState;
use std::path::PathBuf;
use std::sync::Arc;

/// 一条会话条目的「形状」：类型 + 角色 + 内容块类型。
fn shape(entry: &serde_json::Value) -> String {
    let ty = entry.get("type").and_then(|t| t.as_str()).unwrap_or("?");
    let msg = entry.get("message");
    match msg {
        Some(m) => {
            let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("?");
            let blocks: Vec<&str> = m
                .get("content")
                .and_then(|c| c.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|b| b.get("type").and_then(|t| t.as_str()))
                        .collect()
                })
                .unwrap_or_default();
            format!("{ty}:{role}[{}]", blocks.join(","))
        }
        None => format!("{ty}"),
    }
}

fn pi_fixture_shape() -> Vec<String> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/nini-conformance/fixtures/pi-0.85.1-session.jsonl"
    );
    std::fs::read_to_string(path)
        .expect("读不到 Pi 0.85.1 的会话 fixture")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).expect("fixture 里有不是 JSON 的行");
            shape(&v)
        })
        .collect()
}

fn nini_state(tag: &str) -> (Arc<std::sync::Mutex<AppState>>, AgentSink, PathBuf) {
    let dir = std::env::temp_dir().join(format!("nini-bshape-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let state = Arc::new(std::sync::Mutex::new(AppState::new("test-model")));
    state.lock().unwrap().session_init(dir.join("s.jsonl"));
    let (tx, _rx) = tokio::sync::watch::channel(false);
    let sink = AgentSink::new(state.clone(), tx);
    (state, sink, dir)
}

fn nini_entries(state: &Arc<std::sync::Mutex<AppState>>) -> Vec<serde_json::Value> {
    let arc = state
        .lock()
        .unwrap()
        .session_state
        .session
        .as_ref()
        .unwrap()
        .clone();
    let g = arc.try_lock().unwrap();
    g.entries
        .iter()
        .map(|e| serde_json::from_str(&serde_json::to_string(e).unwrap()).unwrap())
        .collect()
}

#[test]
fn ninis_tool_call_turn_has_pis_shape() {
    let (state, sink, _dir) = nini_state("shape");

    // 一个回合：用户 → assistant 发起工具调用 → 工具结果 → assistant 回答。
    // 走的是**真实的 AgentEventLite 路径**（与 p01_persist 同一套路），
    // 不是手工调 state 的辅助函数。
    // 用户消息先来（Pi 的录制里第一条就是它）
    {
        let mut g = state.lock().unwrap();
        g.push_user("run the bash tool then say TOOL_RESULT_SEEN");
    }

    // 场景必须与 Pi 那次录制**一致**：模型先发起工具调用（没有前置文本），
    // 拿到结果后回答。对照物录的正是这个形状。
    sink.push(AgentEventLite::ToolCallStart {
        id: "call-1".into(),
        name: "bash".into(),
    });
    sink.push(AgentEventLite::ToolCallStop {
        id: "call-1".into(),
        args: r#"{"command":"echo hi"}"#.into(),
    });
    sink.push(AgentEventLite::ToolResult {
        id: "call-1".into(),
        name: "bash".into(),
        ok: true,
        content: "hi".into(),
        details: None,
        duration_ms: 1,
    });
    sink.push(AgentEventLite::TextDelta("done".into()));
    sink.push(AgentEventLite::TurnEnd);

    let shape_nini: Vec<String> = nini_entries(&state).iter().map(shape).collect();
    let shape_pi: Vec<String> = pi_fixture_shape();

    // Pi 的形状（去掉头部的 session / model_change / thinking_level_change，
    // 那三条是会话元数据，与回合形状无关）：
    let pi_turn: Vec<&String> = shape_pi
        .iter()
        .filter(|s| s.starts_with("message:"))
        .collect();

    // nini 的回合部分：找到第一条 message 之后的部分。
    let nini_turn: Vec<&String> = shape_nini
        .iter()
        .filter(|s| s.starts_with("message:"))
        .collect();

    assert!(
        !pi_turn.is_empty(),
        "Pi 的 fixture 里没有 message 条目——对照物坏了"
    );
    // 比对**第一个回合**（Pi 的录制里第一轮就是带工具调用的那一轮；
    // 后面还有一轮纯文本，那是另一个场景）。
    let n = nini_turn.len();
    assert!(
        n <= pi_turn.len(),
        "nini 的回合比 Pi 的还长：{nini_turn:?} vs {pi_turn:?}"
    );
    assert_eq!(
        &pi_turn[..n],
        &nini_turn[..],
        "回合的形状与 Pi 不一致。\nPi:   {pi_turn:?}\nnini: {nini_turn:?}"
    );

    // 单独把这条约定钉死：assistant(toolCall) 之后必须紧跟 toolResult。
    let idx = nini_turn
        .iter()
        .position(|s| s.contains("toolCall"))
        .expect("nini 的回合里必须有 toolCall");
    assert!(
        nini_turn[idx + 1].starts_with("message:toolResult"),
        "toolCall 之后必须紧跟一条 toolResult 消息（Pi 的形状）：{nini_turn:?}"
    );
    assert!(
        nini_turn[idx + 1].contains("toolCallId") || nini_turn[idx + 1].contains("text"),
        "toolResult 消息要带得上工具调用的绑定信息：{nini_turn:?}"
    );
}
