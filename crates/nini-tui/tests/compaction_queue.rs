//! Oracle: 压缩期间提交的消息**进队列**，不另起一轮（v0.10.8 / A2）。
//!
//! v0.11 (A4)：字段 `RunState::pending_next_turn_messages` 改名为
//! `RunState::pending_steer_messages`。队列按 Pi 的做法
//! （`agent-session.ts#_steeringMessages` / `agent-session.ts#_followUpMessages`）
//! 拆成了 steer 与 followUp 两条；压缩期的提交走的是 **Enter**，即 **steer**
//! 语义（Pi：`interactive-mode.ts#setupEditorSubmitHandler` → `prompt(text,
//! { streamingBehavior: "steer" })`）。**语义不变，只改名**。
//!
//! 此前 `run_state.is_compacting` **永远**是 false，所以下面这个分支
//! （runtime.rs：「if compaction is in progress, queue the message instead
//! of spawning an agent」）是**不可达死代码**——Pi 的
//! `interactive-mode.ts#queueCompactionMessage` 有明确语义，nini 没有。
//!
//! 判据：
//! 1. sink 收到 `PhaseChanged("compacting:overflow")` → `is_compacting` 立起来；
//! 2. 压缩期间的提交：**不**起新的一轮（driver 没被调用），消息进队列；
//! 3. 相位离开 compacting → 标志落下。

use nini_tui::runtime::{submit_user_input, AgentDriver, AgentEventLite, SharedState};
use nini_tui::state::AppState;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn fresh_state() -> SharedState {
    let dir = std::env::temp_dir().join(format!("nini-compact-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut g = AppState::new("test-model");
    g.session_init(dir.join("s.jsonl"));
    Arc::new(std::sync::Mutex::new(g))
}

#[tokio::test]
async fn a_submit_during_compaction_is_queued_not_spawned() {
    let shared = fresh_state();
    let spawns = Arc::new(AtomicUsize::new(0));
    let (tx, _rx) = tokio::sync::watch::channel(false);

    let driver: AgentDriver = {
        let spawns = Arc::clone(&spawns);
        Arc::new(move |_msg, _seed, sink, _done| {
            spawns.fetch_add(1, Ordering::SeqCst);
            let sink = sink.clone();
            tokio::spawn(async move {
                sink.push(AgentEventLite::TextDelta("answer".into()));
                sink.push(AgentEventLite::Done);
            })
        })
    };

    // 压缩开始：agent 发 Compacting 相位 → sink 把它变成状态
    {
        let sink =
            nini_tui::runtime::AgentSink::new(shared.clone(), tokio::sync::watch::channel(false).0);
        // 用**标签**而不是 Display —— Display 是给人看的，格式一变
        // 这个判断就会静默失效（review finding）。
        sink.push(AgentEventLite::PhaseChanged(
            nini_core::agent::AgentPhase::Compacting {
                reason: "overflow".into(),
                progress: None,
            }
            .tag()
            .to_string(),
        ));
    }
    {
        let g = shared.lock().unwrap();
        assert!(
            g.run_state.is_compacting,
            "收到 Compacting 相位后必须进入「正在压缩」状态——否则压缩期间提交的\
             语义不存在（Pi：queueCompactionMessage）"
        );
    }

    // 压缩期间提交一条
    {
        let mut g = shared.lock().unwrap();
        for c in "typed during compaction".chars() {
            g.input.insert_char(c);
        }
    }
    submit_user_input(
        &shared,
        &driver,
        Arc::new(tokio::sync::Notify::new()),
        tx.clone(),
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert_eq!(
        spawns.load(Ordering::SeqCst),
        0,
        "压缩期间不能另起一轮 agent（Pi：压缩期间的消息进队列）"
    );
    {
        let g = shared.lock().unwrap();
        assert!(
            g.run_state
                .pending_steer_messages
                .iter()
                .any(|m| m.contains("typed during compaction")),
            "消息必须进队列：{:?}",
            g.run_state.pending_steer_messages
        );
    }

    // 压缩结束
    {
        let sink =
            nini_tui::runtime::AgentSink::new(shared.clone(), tokio::sync::watch::channel(false).0);
        sink.push(AgentEventLite::PhaseChanged("working".to_string()));
    }
    {
        let g = shared.lock().unwrap();
        assert!(
            !g.run_state.is_compacting,
            "压缩结束后标志必须落下——否则之后的消息会一直进队列"
        );
    }
}

/// 运行结束（Done）必须**复位**压缩标志。
///
/// v0.10.8 (A2, review finding)：此前若这一轮死在 Compacting 相位之后，
/// 标志会永久为真 —— 之后每次 Enter 都被收进队列、再也不执行。
/// 症状是「按了 Enter，什么也没发生」，而且**没有任何报错**。
#[tokio::test]
async fn the_end_of_a_run_resets_the_compaction_flag() {
    let shared = fresh_state();
    {
        let sink =
            nini_tui::runtime::AgentSink::new(shared.clone(), tokio::sync::watch::channel(false).0);
        sink.push(AgentEventLite::PhaseChanged("compacting".to_string()));
    }
    assert!(
        shared.lock().unwrap().run_state.is_compacting,
        "前置条件：压缩标志应当立着"
    );

    // 这一轮结束了
    {
        let sink =
            nini_tui::runtime::AgentSink::new(shared.clone(), tokio::sync::watch::channel(false).0);
        sink.push(AgentEventLite::Done);
    }

    assert!(
        !shared.lock().unwrap().run_state.is_compacting,
        "运行结束必须复位压缩标志——否则之后的提交会被永远排队"
    );
}
