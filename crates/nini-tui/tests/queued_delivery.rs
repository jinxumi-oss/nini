//! Oracle: 回合结束后，**排队里的消息必须自动被送出去**（v0.10.8 / A6）。
//!
//! v0.11 (A4)：字段 `RunState::pending_next_turn_messages` 改名为
//! `RunState::pending_steer_messages`。队列按 Pi 的做法
//! （`agent-session.ts#_steeringMessages` / `agent-session.ts#_followUpMessages`）
//! 拆成了 steer 与 followUp 两条；这里断言的是「回合还在跑时提交的消息」，
//! 即 **steer** 语义（Pi：`interactive-mode.ts#setupEditorSubmitHandler`
//! → `prompt(text, { streamingBehavior: "steer" })`）。**语义不变，只改名**。
//!
//! 排队消息的正常归宿：回合还活着时，agent 循环每轮 `get_steering_messages`
//! 会把它捞进当前 turn（Pi 的 steer）。可如果它是在**回合收尾之后**排进来的
//! ——典型场景是用户按了 Esc、又在 agent 还在收尾时提交了一条——就没有人再捞它了：
//! 消息永远躺在队列里，**用户说的那句话凭空消失**。
//!
//! Pi 在 session 层解决这件事（run 结算后把 pending 的消息作为 follow-up 送出去）；
//! nini 的等价物是 runtime 循环里的 `submit_queued_turn`。
//!
//! 这里断言三件事：
//! 1. 回合收尾（Done）会**标记**「该投递了」；
//! 2. 投递会把队列里的消息作为**新的一轮**送出去（driver 被再次调用）；
//! 3. 排队消息**只出现一次**——transcript 里不重复、session 里也不重复。

use nini_tui::runtime::{
    submit_queued_turn, submit_user_input, AgentDriver, AgentEventLite, SharedState,
};
use nini_tui::state::{AppState, RunMode};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn fresh_state() -> (SharedState, std::path::PathBuf) {
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("nini-queued-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let shared: SharedState = {
        let mut g = AppState::new("test-model");
        g.session_init(dir.join("s.jsonl"));
        Arc::new(std::sync::Mutex::new(g))
    };
    (shared, dir)
}

async fn type_and_submit(
    shared: &SharedState,
    driver: &AgentDriver,
    tx: tokio::sync::watch::Sender<bool>,
    text: &str,
) {
    {
        let mut g = shared.lock().unwrap();
        for c in text.chars() {
            g.input.insert_char(c);
        }
    }
    submit_user_input(
        shared,
        driver,
        Arc::new(tokio::sync::Notify::new()),
        tx.clone(),
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
}

#[tokio::test]
async fn a_message_queued_after_the_turn_ended_is_delivered_without_another_enter() {
    let (shared, _dir) = fresh_state();
    let spawns = Arc::new(AtomicUsize::new(0));
    let seen: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (tx, _rx) = tokio::sync::watch::channel(false);

    // driver：第一次调用**一直挂着**（模拟一个还在收尾的回合），
    // 第二次调用立刻结束。
    let driver: AgentDriver = {
        let spawns = Arc::clone(&spawns);
        let seen = Arc::clone(&seen);
        Arc::new(move |msg: String, _seed, sink, _done| {
            let n = spawns.fetch_add(1, Ordering::SeqCst);
            seen.lock().unwrap().push(msg);
            let sink = sink.clone();
            tokio::spawn(async move {
                if n == 0 {
                    // 第一个回合不结束：用户在这期间按了 Esc 又提交了一条
                    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                }
                sink.push(AgentEventLite::TextDelta("answer".into()));
                sink.push(AgentEventLite::TurnEnd);
                sink.push(AgentEventLite::Done);
            })
        })
    };

    // ── 第一轮 ──
    type_and_submit(&shared, &driver, tx.clone(), "first prompt").await;
    assert_eq!(spawns.load(Ordering::SeqCst), 1);

    // ── 用户按 Esc（编辑器立刻可用），然后又提交了一条 ──
    {
        let mut g = shared.lock().unwrap();
        g.run_state.mode = RunMode::Editing; // Esc 的效果
    }
    type_and_submit(&shared, &driver, tx.clone(), "second prompt").await;

    // 第二条**没有**另起一轮（agent 还活着），它进了队列
    assert_eq!(
        spawns.load(Ordering::SeqCst),
        1,
        "agent 还活着时不应该另起一轮"
    );
    {
        let g = shared.lock().unwrap();
        assert!(
            g.run_state
                .pending_steer_messages
                .iter()
                .any(|m| m.contains("second prompt")),
            "第二条应该进了队列：{:?}",
            g.run_state.pending_steer_messages
        );
    }

    // ── 第一个回合结束（Done）→ 标记「该投递了」──
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    {
        let g = shared.lock().unwrap();
        assert!(
            g.run_state.auto_turn_pending,
            "回合收尾后必须标记「该投递了」，否则排队的消息永远不会被送出"
        );
    }

    // ── 运行循环会做的事：投递 ──
    let delivered = submit_queued_turn(
        &shared,
        &driver,
        Arc::new(tokio::sync::Notify::new()),
        tx.clone(),
    );
    assert!(delivered, "投递函数应该送出了排队里的消息");
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    assert_eq!(
        spawns.load(Ordering::SeqCst),
        2,
        "排队的消息必须作为**新的一轮**被送出去——用户不该再按一次 Enter"
    );
    let seen = seen.lock().unwrap().clone();
    assert!(
        seen.iter().any(|m| m.contains("second prompt")),
        "第二轮必须收到用户那条消息：{seen:?}"
    );
}

#[tokio::test]
async fn a_delivered_message_is_not_shown_twice() {
    let (shared, _dir) = fresh_state();
    let spawns = Arc::new(AtomicUsize::new(0));
    let (tx, _rx) = tokio::sync::watch::channel(false);

    let driver: AgentDriver = {
        let spawns = Arc::clone(&spawns);
        Arc::new(move |_msg: String, _seed, sink, _done| {
            let n = spawns.fetch_add(1, Ordering::SeqCst);
            let sink = sink.clone();
            tokio::spawn(async move {
                if n == 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                }
                sink.push(AgentEventLite::TextDelta("answer".into()));
                sink.push(AgentEventLite::TurnEnd);
                sink.push(AgentEventLite::Done);
            })
        })
    };

    type_and_submit(&shared, &driver, tx.clone(), "first prompt").await;
    {
        let mut g = shared.lock().unwrap();
        g.run_state.mode = RunMode::Editing;
    }
    type_and_submit(&shared, &driver, tx.clone(), "queued prompt").await;
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;

    submit_queued_turn(
        &shared,
        &driver,
        Arc::new(tokio::sync::Notify::new()),
        tx.clone(),
    );
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // transcript 里那条消息只出现一次（排队时显示过，投递时不该再显示）
    let g = shared.lock().unwrap();
    let text: String = g
        .transcript_state
        .lines
        .iter()
        .map(|l| format!("{l:?}"))
        .collect::<Vec<_>>()
        .join("\n");
    let count = text.matches("queued prompt").count();
    assert_eq!(
        count, 1,
        "排队的消息在 transcript 里只能出现一次（显示了 {count} 次）"
    );
}
