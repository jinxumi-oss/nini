//! 文本式工具调用的兜底 —— 回归测试。
//!
//! # 现象
//!
//! provider 侧的模型（Qwen / MiniMax / 讯飞星辰这一族）会**随机**把工具调用
//! 写进正文，而不是放进结构化的 `tool_calls` 字段：
//!
//! ```text
//! <function=bash>
//! <parameter=command>cargo test</parameter>
//! </function>
//! ```
//!
//! nini 此前把这段正文当散文：工具不执行、模型等一个永远不会来的
//! `tool_result`、于是反复重发同一条命令 —— 用户看到的就是「完全没动静」
//! （静默中断）。
//!
//! 真实证据：会话 `~/.nini/agent/sessions/nini/20261119-181606_13b7f0639.jsonl`
//! 第 231-241 条。
//!
//! # 本文件锁三件事
//!
//! 1. 正文里的工具调用**会被执行**（工具真的跑了，结果回来了）；
//! 2. 执行后，**下一次请求**里不再有那串标记（它是工具调用，不是正文）；
//! 3. 工具调用 id 与结果**配对**（assistant 的 tool_use id == tool 消息的
//!    tool_use_id），且认不出来的标记**说出来**（不再静默）。

use futures_util::{Stream, StreamExt};
use nini_core::provider::{Capabilities, Provider, ProviderError, Request, StreamEvent, Usage};
use nini_core::tool::ToolRegistry;
use nini_core::{Agent, AgentEvent, RunConfig};
use nini_tools::BashTool;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

/// 把请求记下来的 provider：按脚本逐轮回放，并把每次请求存起来。
struct RecordingProvider {
    turns: Mutex<Vec<Vec<StreamEvent>>>,
    seen: Mutex<Vec<Request>>,
}

impl RecordingProvider {
    fn new(turns: Vec<Vec<StreamEvent>>) -> Self {
        Self {
            turns: Mutex::new(turns),
            seen: Mutex::new(Vec::new()),
        }
    }
}

impl Provider for RecordingProvider {
    fn name(&self) -> &'static str {
        "recording"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
    fn stream(
        &self,
        req: Request,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send + 'static>> {
        self.seen.lock().unwrap().push(req);
        let mut turns = self.turns.lock().unwrap();
        let events = if turns.is_empty() {
            Vec::new()
        } else {
            turns.remove(0)
        };
        Box::pin(nini_core::provider::events_from_iter(events.into_iter()))
    }
}

/// 一轮「正文里写了工具调用」的模型输出。
fn text_mode_turn(text: &str) -> Vec<StreamEvent> {
    vec![
        StreamEvent::MessageStart {
            id: "msg_1".to_string(),
            model: "test-model".to_string(),
        },
        StreamEvent::TextDelta {
            text: text.to_string(),
        },
        StreamEvent::MessageStop {
            stop_reason: "end_turn".to_string(),
            usage: Usage::default(),
        },
    ]
}

const TEXT_MODE_CALL: &str =
    "<function=bash>\n<parameter=command>echo text-mode-ran</parameter>\n</function>";

/// 正文里的工具调用必须**真的执行**：工具跑了、结果回来了、这一轮继续跑。
#[tokio::test]
async fn a_tool_call_written_as_plain_text_is_executed() {
    let provider = Arc::new(RecordingProvider::new(vec![
        text_mode_turn(TEXT_MODE_CALL),
        text_mode_turn("done"),
    ]));
    let tools = ToolRegistry::new().register(Arc::new(BashTool::new()));
    let mut agent = Agent::new(provider.clone(), tools, RunConfig::new("test-model"));

    let mut stream = Box::pin(agent.run(nini_core::AgentMessage::user("run the command")));
    let mut call_id = None;
    let mut result_id = None;
    let mut result_content = String::new();
    let mut turn_ends = 0;

    while let Some(ev) = stream.next().await {
        match ev {
            Ok(AgentEvent::ToolCallStart { id, name }) if name == "bash" => {
                assert!(call_id.is_none(), "只应有一个 bash 调用");
                call_id = Some(id);
            }
            Ok(AgentEvent::ToolCallStop { id, .. }) if call_id.as_deref() == Some(id.as_str()) => {}
            Ok(AgentEvent::ToolResult { id, output, .. }) => {
                result_id = Some(id);
                result_content = output.content;
            }
            Ok(AgentEvent::TurnEnd { .. }) => turn_ends += 1,
            _ => {}
        }
    }

    let call_id = call_id.expect("正文里的工具调用应被还原成真正的调用");
    assert_eq!(
        result_id.as_deref(),
        Some(call_id.as_str()),
        "工具结果必须与调用配对（assistant 的 tool_use id == tool 消息的 tool_use_id）"
    );
    assert!(
        result_content.contains("text-mode-ran"),
        "工具应真的执行，输出: {result_content:?}"
    );
    assert_eq!(turn_ends, 2, "工具跑完后这一轮应继续（模型收到结果）");
}

/// 那串标记**不能**留在下一次请求里：它是工具调用，不是正文。
/// 留在正文里，模型下一轮会照着它再写一遍（于是又是一次静默卡死）。
#[tokio::test]
async fn the_markup_is_not_carried_into_the_next_request() {
    let provider = Arc::new(RecordingProvider::new(vec![
        text_mode_turn(TEXT_MODE_CALL),
        text_mode_turn("done"),
    ]));
    let tools = ToolRegistry::new().register(Arc::new(BashTool::new()));
    let mut agent = Agent::new(provider.clone(), tools, RunConfig::new("test-model"));

    let mut stream = Box::pin(agent.run(nini_core::AgentMessage::user("run the command")));
    while let Some(ev) = stream.next().await {
        if let Ok(AgentEvent::Error { .. }) = ev {
            panic!("这一轮不该报错");
        }
    }

    let seen = provider.seen.lock().unwrap();
    assert!(seen.len() >= 2, "应有至少两次请求，实际 {}", seen.len());
    let second = &seen[1];
    let rendered = format!("{:?}", second.messages);
    assert!(
        !rendered.contains("<function="),
        "下一次请求里不该再有文本式工具调用标记: {rendered}"
    );
    assert!(
        rendered.contains("text-mode-ran"),
        "下一次请求里应带着工具的结果: {rendered}"
    );
}

/// 认不出来的标记（这里指一个没注册的工具名）**必须说出来**。
/// 此前这里是彻底静默的：用户只看到「没动静」。
#[tokio::test]
async fn an_unrecognised_markup_is_reported_instead_of_being_silent() {
    let provider = Arc::new(RecordingProvider::new(vec![text_mode_turn(
        "<function=not_a_real_tool>\n<parameter=x>1</parameter>\n</function>",
    )]));
    let tools = ToolRegistry::new().register(Arc::new(BashTool::new()));
    let mut agent = Agent::new(provider.clone(), tools, RunConfig::new("test-model"));

    let mut stream = Box::pin(agent.run(nini_core::AgentMessage::user("do something")));
    let mut errors: Vec<String> = Vec::new();
    let mut tool_calls = 0;
    while let Some(ev) = stream.next().await {
        match ev {
            Ok(AgentEvent::Error { message }) => errors.push(message),
            Ok(AgentEvent::ToolCallStart { .. }) => tool_calls += 1,
            _ => {}
        }
    }

    assert_eq!(tool_calls, 0, "未注册的工具不该被调用");
    assert_eq!(errors.len(), 1, "必须有一条可见的错误说明，而不是静默");
    assert!(
        errors[0].contains("tool call as plain text"),
        "错误说明要讲清楚发生了什么: {:?}",
        errors[0]
    );
}
