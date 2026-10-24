//! Oracle: 压缩**有生命周期**（v0.10.8 / A2）。
//!
//! 此前压缩是「偷偷」进行的：agent 不发 Compacting 相位、TUI 的
//! `run_state.is_compacting` **永远**是 false —— 于是 runtime.rs 里那个
//! 「压缩期间提交进队列」的分支是**不可达死代码**，而 Pi 在压缩期间是有
//! 明确语义的（`agent-session.ts#_checkCompaction` → 压缩期间提交走
//! `queueCompactionMessage`）。
//!
//! 判据：
//! 1. 溢出触发压缩时，agent **先**发 `PhaseChanged(Compacting)`，
//!    **再**发 `Compacted`，压缩结束后相位回到 `Working`；
//! 2. 压缩期间 `is_compacting()` 为真，结束后为假；
//! 3. 压缩期间排队的消息被**交回**给调用方（Pi 的 `_pendingNextTurnMessages`）。

use nini_ai::fixture::{FixtureTurn, ProgrammedProvider};
use nini_core::agent::AgentPhase;
use nini_core::provider::{ContentBlock, Message, Provider, Role, Usage};
use nini_core::tool::ToolRegistry;
use nini_core::{Agent, AgentEvent, CompactionHook, CompactionResult, RunConfig};
use std::sync::{Arc, Mutex};

/// 一个回合以 length stop 结束 —— 这才会走溢出路径。
fn length_stop_provider() -> Arc<dyn Provider> {
    Arc::new(ProgrammedProvider::from_turns(vec![
        vec![
            FixtureTurn::Text("this is a long answer that got cut off".into()),
            FixtureTurn::Stop {
                stop_reason: "length".into(),
                usage: Usage::default(),
            },
        ],
        vec![
            FixtureTurn::Text("and here is the answer after compaction".into()),
            FixtureTurn::Stop {
                stop_reason: "end_turn".into(),
                usage: Usage::default(),
            },
        ],
    ]))
}

fn big_history(n: usize) -> Vec<Message> {
    (0..n)
        .map(|i| Message {
            role: if i % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            },
            content: vec![ContentBlock::Text {
                text: format!("message {i} {}", "x".repeat(50)),
            }],
            timestamp: 0,
        })
        .collect()
}

#[test]
fn an_overflow_compaction_has_a_phase_and_ends_it() {
    let provider = length_stop_provider();
    let mut agent = Agent::new(provider, ToolRegistry::new(), RunConfig::new("test-model"));

    let hook_ran = Arc::new(Mutex::new(false));
    let hook_ran2 = Arc::clone(&hook_ran);
    agent.set_compaction_hook(CompactionHook::new(move |_messages| {
        *hook_ran2.lock().unwrap() = true;
        Some(CompactionResult {
            summary: "summary of the older half".into(),
            tokens_before: 999,
            tokens_after: 10,
            messages: vec![],
        })
    }));

    let history = big_history(40);
    let events: Vec<AgentEvent> = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(async {
            let mut stream = Box::pin(agent.run(Message::user("go")));
            let mut out = Vec::new();
            while let Some(ev) = futures_util::StreamExt::next(&mut stream).await {
                out.push(ev.expect("agent event"));
            }
            out
        });
    let _ = history;

    assert!(*hook_ran.lock().unwrap(), "压缩钩子必须被调用");

    let compacting_at = events
        .iter()
        .position(|e| matches!(e, AgentEvent::PhaseChanged(AgentPhase::Compacting { .. })));
    let compacted_at = events
        .iter()
        .position(|e| matches!(e, AgentEvent::Compacted { .. }));
    // 「压缩结束后的 Working」——注意是**压缩之后**那一次：
    // 回合开始时也会发一次 Working。

    let compacting_at =
        compacting_at.expect("压缩必须发出 Compacting 相位——否则 TUI 不知道正在压缩");
    let compacted_at = compacted_at.expect("压缩必须发出 Compacted 事件");
    assert!(
        compacting_at < compacted_at,
        "Compacting 相位必须在 Compacted 之前（否则压缩期间无法排队提交）"
    );
    // 「压缩结束后的 Working」——注意是**压缩之后**那一次：
    // 回合开始时也会发一次 Working。
    let working_after = events
        .iter()
        .enumerate()
        .skip(compacted_at + 1)
        .find(|(_, e)| matches!(e, AgentEvent::PhaseChanged(AgentPhase::Working)))
        .map(|(i, _)| i);
    assert!(
        working_after.is_some(),
        "压缩结束后相位必须回到 Working（否则 TUI 会以为还在压缩）"
    );
    assert!(
        !agent.is_compacting(),
        "压缩结束后标志必须落下——否则 TUI 会永远以为在压缩"
    );
}

#[test]
fn messages_queued_during_compaction_are_handed_back() {
    // Pi: 压缩期间提交的消息进 `_pendingNextTurnMessages`，压缩结束后
    // 交给下一轮（`agent-session.ts#queueCompactionMessage`）。
    let mut agent = Agent::new(
        length_stop_provider(),
        ToolRegistry::new(),
        RunConfig::new("test-model"),
    );
    agent.begin_auto_compaction();
    assert!(
        agent.is_compacting(),
        "begin_auto_compaction 之后标志必须为真"
    );
    agent.queue_next_turn("typed during compaction");

    let drained = agent.end_auto_compaction();
    assert_eq!(
        drained,
        vec!["typed during compaction".to_string()],
        "压缩期间排队的消息必须交回给调用方，否则它就消失了"
    );
    assert!(!agent.is_compacting(), "结束后标志必须落下");
}

/// 压缩**没成功**（钩子返回 None）也是「压缩结束了」——标志必须落下。
///
/// v0.10.8 (A2, review finding)：None 分支原本直接 return，标志会**永久**为真，
/// 而 TUI 会把之后每一次提交都收进队列、再也不执行——症状是
/// 「按了 Enter，什么也没发生」。
#[test]
fn a_failed_compaction_also_ends_the_lifecycle() {
    let mut agent = Agent::new(
        length_stop_provider(),
        ToolRegistry::new(),
        RunConfig::new("test-model"),
    );
    // 装一个**返回 None** 的钩子 → 走「压缩没成功」那条分支
    // （review finding 指出的就是这条分支漏了收尾）。
    agent.set_compaction_hook(CompactionHook::new(|_messages| None));
    let events: Vec<AgentEvent> = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(async {
            let mut stream = Box::pin(agent.run(Message::user("go")));
            let mut out = Vec::new();
            while let Some(ev) = futures_util::StreamExt::next(&mut stream).await {
                out.push(ev.expect("agent event"));
            }
            out
        });

    assert!(
        !agent.is_compacting(),
        "这一轮结束之后标志必须是 false——否则 TUI 会永远把提交收进队列"
    );
    // 相位必须回到 Idle（而不是停在 compacting）
    let last_phase = events.iter().rev().find_map(|e| match e {
        AgentEvent::PhaseChanged(p) => Some(p.clone()),
        _ => None,
    });
    let last_tag = last_phase.map(|p| p.tag().to_string());
    assert!(
        last_tag.as_deref() != Some("compacting"),
        "最后一个相位不能是 Compacting：{last_tag:?}"
    );
}
