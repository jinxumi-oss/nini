//! Oracle: 自动投递**不能吞掉**它刚取出的消息。
//!
//! # The defect (found by the A4 review)
//!
//! `submit_queued_turn` 从队列里 `mem::take` 出**一条**消息，把它作为
//! `explicit_text` 交给 `submit_user_input_inner`，然后那条消息就不在队列里了。
//!
//! 但 `submit_user_input_inner` 的**第一件事**是检查 `agent_alive`：
//!
//! ```text
//! if g.run_state.agent_alive {
//!     let steered = g.input.submit();      // ← 读的是**输入框**，不是 explicit_text
//!     if !steered.trim().is_empty() { queue_while_running(&mut g, &steered, kind); }
//!     return;                                // ← explicit_text 被**无视**了
//! }
//! ```
//!
//! 输入框此时是空的（消息是从队列里取的，不在输入框里），于是：
//! **那条消息既不在队列里，也没被提交，也没有任何报错——它消失了。**
//!
//! 触发路径是真实的：`Done` 事件把 `auto_turn_pending` 置真，运行循环
//! 随即调 `submit_queued_turn`；而同一时刻用户按 Enter 会起**新一轮**
//! （`agent_alive = true`）。`select!` 在事件与按键之间随机挑一个，于是
//! 「先取后撞」完全可能发生。
//!
//! A4 把 followUp 队列接上真实生产者之后，这条老路被**放大**了：
//! Alt+Enter 的消息本来多半会被 agent 的 followUp 钩子取走，取不到才落到
//! 这里。
//!
//! # 钉住什么
//!
//! 1. 走**真实路径** `submit_queued_turn` 后，那条消息必须在某处：
//!    要么提交成功，要么放回队列——**不许消失**；
//! 2. 撞上 `agent_alive` 时，它回到**原来的队列**（不是被当成输入框内容丢掉）；
//! 3. 撞上 `agent_alive` 时，**运行态不许**被改成 `Editing`
//!    （agent 还在跑，状态栏却显示空闲）；
//! 4. 自动投递的文本**不被重新解释**：`!cmd` 不执行命令、`/cmd` 不派发斜杠
//!    命令（Pi 把排队的消息**原样**送出，见
//!    `agent-session.ts#steer` / `#followUp`）。

use nini_tui::runtime::{AgentDriver, SharedState};
use nini_tui::state::RunMode;
use std::sync::Arc;

/// A driver that must never be called: the agent is alive, so no new
/// turn may start.
fn dead_driver() -> AgentDriver {
    Arc::new(|_, _, _, _| unreachable!("no driver should be spawned"))
}

/// A driver that records the turns it was asked to start, and starts
/// none. Used where a turn is *expected* to start.
fn counting_driver(counter: Arc<std::sync::atomic::AtomicUsize>) -> AgentDriver {
    Arc::new(move |text, _, _, _| {
        counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let _ = text;
        tokio::spawn(async {})
    })
}

/// The state as it is when the race happens: an agent is alive, and one
/// message is waiting to be auto-delivered.
fn state_with_queued_message(text: &str) -> SharedState {
    let state = nini_tui::state::AppState::new("test-model");
    let shared: SharedState = Arc::new(std::sync::Mutex::new(state));
    {
        let mut g = shared.lock().unwrap();
        g.run_state.agent_alive = true;
        g.run_state.mode = RunMode::Running;
        g.run_state.pending_steer_messages.push(text.to_string());
        // `Done` set this; the run loop then calls `submit_queued_turn`.
        g.run_state.auto_turn_pending = true;
    }
    shared
}

fn run_auto_delivery(shared: &SharedState) {
    nini_tui::runtime::submit_queued_turn(
        shared,
        &dead_driver(),
        Arc::new(tokio::sync::Notify::new()),
        tokio::sync::watch::channel(false).0,
    );
}

#[test]
fn a_message_taken_for_auto_delivery_is_not_lost() {
    let shared = state_with_queued_message("please do the thing");

    run_auto_delivery(&shared);

    let g = shared.lock().unwrap();
    let still_queued = g
        .run_state
        .pending_steer_messages
        .contains(&"please do the thing".to_string());
    assert!(
        still_queued,
        "the message was taken out of the queue and then dropped: {:?}",
        g.run_state.pending_steer_messages
    );
}

#[test]
fn a_refused_auto_delivery_does_not_pretend_the_agent_is_idle() {
    let shared = state_with_queued_message("still here");

    run_auto_delivery(&shared);

    let g = shared.lock().unwrap();
    assert_eq!(
        g.run_state.mode,
        RunMode::Running,
        "the agent is still running — the run state must not claim otherwise"
    );
    assert!(g.run_state.agent_alive, "the agent is still alive");
}

/// A queued message must be sent **verbatim** — never re-interpreted.
///
/// Pi sends queued text as text (`agent-session.ts#steer` / `#followUp`
/// take the user's string as-is). nini's auto-delivery goes through the
/// same `submit_user_input_inner` as a typed submission, so before the
/// fix a queued `!cargo test` would **actually run the command** and a
/// queued `/clear` would **actually dispatch** — a queued sentence
/// silently becoming an executed command.
#[tokio::test]
async fn a_queued_bang_command_is_not_executed_when_delivered() {
    let shared = state_with_queued_message("!touch /tmp/nini-queue-routing-must-not-exist");

    // The agent is NOT alive here: this is the path that would really
    // execute the command. (The refused path is covered above.)
    {
        let mut g = shared.lock().unwrap();
        g.run_state.agent_alive = false;
        g.run_state.mode = RunMode::Editing;
    }
    let marker = std::path::Path::new("/tmp/nini-queue-routing-must-not-exist");
    let _ = std::fs::remove_file(marker);

    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    nini_tui::runtime::submit_queued_turn(
        &shared,
        &counting_driver(counter.clone()),
        Arc::new(tokio::sync::Notify::new()),
        tokio::sync::watch::channel(false).0,
    );

    assert!(
        counter.load(std::sync::atomic::Ordering::SeqCst) >= 1,
        "precondition: the delivery must have started a turn"
    );
    assert!(
        !marker.exists(),
        "a queued `!cmd` must be sent as text, not executed — \
         the marker file must not exist"
    );
}
