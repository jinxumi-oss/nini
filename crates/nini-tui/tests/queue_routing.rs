//! Oracle: Pi 的**两条队列**（steer / followUp）必须各走各的路（v0.11 / A4）。
//!
//! # Pi 依据（每条都能在 `references/pi-0.85.1/` 里解析）
//!
//! | Pi 符号 | 事实 |
//! |---|---|
//! | `agent-session.ts#_steeringMessages` | steer 队列：Enter 提交的消息落这里 |
//! | `agent-session.ts#_followUpMessages` | followUp 队列：Alt+Enter 提交的消息落这里 |
//! | `agent-session.ts#steer` | steer 的入队口 |
//! | `agent-session.ts#followUp` | followUp 的入队口 |
//! | `agent-session.ts#clearQueue` | 一次清空**两个**队列 |
//! | `agent-session.ts#recordBashResult` | bash 结果记进 session 的活消息表——**不是**用户指令 |
//! | `interactive-mode.ts#setupEditorSubmitHandler` | Enter：流式中 `prompt(text,{streamingBehavior:"steer"})`；压缩中 `queueCompactionMessage(text,"steer")` |
//! | `interactive-mode.ts#handleFollowUp` | Alt+Enter：`prompt(text,{streamingBehavior:"followUp"})`；压缩中 `queueCompactionMessage(text,"followUp")` |
//! | `interactive-mode.ts#queueCompactionMessage` | 压缩期的两个桶：`"steer"` 与 `"followUp"` |
//! | `interactive-mode.ts#restoreQueuedMessagesToEditor` | Esc 退回编辑器：`[...steering, ...followUp]` —— **steer 在前** |
//! | `interactive-mode.ts#clearAllQueues` | 两个队列一起清 |
//! | `agent-loop.ts#runAgentLoop` | 循环里取 steer / followUp |
//!
//! Pi 的 app 级键位表（快照里的 `src/ui/keybindings.ts`）把
//! `app.message.followUp` 的默认键定为 `alt+enter`（Windows 上是 `ctrl+q`）。
//! 这里**不**把它写成 `keybindings.ts#…` 形态的引用：引用解析器把这个名字
//! 解析到 `src/tui/keybindings.ts`（pi-tui 那个包的文件，里面没有 followUp），
//! 写上去会变成一条**解析不到**的引用——而解析不到的引用不是引用。
//! 行为依据改用 `interactive-mode.ts#handleFollowUp`（它就是接到这个动作的地方）。
//!
//! # 本文件钉住的行为
//!
//! nini 此前只有**一条** `pending_next_turn_messages`，三个用途争抢它
//! （steering 钩子 / 轮后自动投递 / Esc 退回），谁先谁拿 → 语义不确定。
//! 拆成两条之后，下面这些**路由**必须成立：
//!
//! | # | 断言 | Pi 依据 |
//! |---|---|---|
//! | 1 | 回合在跑时 Enter → **steer** 队列（followUp 空） | `interactive-mode.ts#setupEditorSubmitHandler` + `agent-session.ts#_steeringMessages` |
//! | 2 | 回合在跑时 Alt+Enter → **followUp** 队列（steer 空） | `interactive-mode.ts#handleFollowUp` + `agent-session.ts#_followUpMessages` |
//! | 3 | 压缩期两分：Enter → steer，followUp 入口 → followUp；**都不另起一轮** | `interactive-mode.ts#queueCompactionMessage` |
//! | 4 | Esc 把两个队列**一起**退回编辑器，**steer 在前**，队列清空，退回条数 = 2 | `interactive-mode.ts#restoreQueuedMessagesToEditor` + `interactive-mode.ts#clearAllQueues` |
//! | 5 | `take_steer_messages` / `take_followup_messages` **互不越界** | `agent-session.ts#_steeringMessages` 与 `#_followUpMessages` 是两个独立队列 |
//! | 6 | `!cmd` 的输出**不进队列**，但**进 session**（一条 User 消息） | `agent-session.ts#recordBashResult` |
//! | 7 | `app.message.followUp` / `FollowUp` → `KeyAction::FollowUp`；默认键 Alt+Enter → followUp | `interactive-mode.ts#handleFollowUp` |
//!
//! 第 5 条的另一半（**钩子各取各的队列**）在 `nini-cli` 侧：
//! `crates/nini-cli/src/app.rs` 的 `LiveConfigHooks` 单元测试。
//!
//! # 怎么跑
//!
//! ```console
//! $ cargo test -p nini-tui --test queue_routing
//! ```
//!
//! 走真实无头派发点（`nini_tui::runtime::apply_action`）——不是自己重写一遍
//! 派发逻辑，那样测的就不是产品代码了。

use nini_tui::keys::{Key, KeyAction, KeyModifiers};
use nini_tui::runtime::{
    apply_action, return_queue_to_editor, submit_user_input, submit_user_input_with_kind,
    take_followup_messages, take_steer_messages, AgentDriver, AgentEventLite, AgentSink, QueueKind,
    SharedState,
};
use nini_tui::state::{AppState, RunMode};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// A state with a session, in its own temp directory.
///
/// 目录带 tag：同一个进程里并行跑的测试不能共用一个目录。
fn fresh_state(tag: &str) -> SharedState {
    let dir = std::env::temp_dir().join(format!("nini-queue-routing-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut g = AppState::new("test-model");
    g.session_init(dir.join("s.jsonl"));
    Arc::new(std::sync::Mutex::new(g))
}

/// A driver that only counts how many turns it was asked to run.
fn counting_driver(spawns: Arc<AtomicUsize>) -> AgentDriver {
    Arc::new(move |_msg, _seed, sink, _done| {
        spawns.fetch_add(1, Ordering::SeqCst);
        let sink = sink.clone();
        tokio::spawn(async move {
            sink.push(AgentEventLite::TextDelta("answer".into()));
            sink.push(AgentEventLite::Done);
        })
    })
}

fn type_text(state: &mut AppState, text: &str) {
    for c in text.chars() {
        state.input.insert_char(c);
    }
}

/// The text of a provider-layer message (the shape `take_*` returns).
fn provider_text(msg: &nini_core::provider::Message) -> String {
    msg.content
        .iter()
        .filter_map(|b| match b {
            nini_core::provider::ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

/// The text of a session-layer user message.
fn user_text(msg: &nini_core::UserMessage) -> String {
    match &msg.content {
        nini_core::StringOrContentBlocks::String(s) => s.clone(),
        nini_core::StringOrContentBlocks::Blocks(blocks) => blocks
            .iter()
            .filter_map(|b| match b {
                nini_core::PiContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(""),
    }
}

// ── 1. 流式中的 Enter → steer ─────────────────────────────────────────────

/// Pi: `interactive-mode.ts#setupEditorSubmitHandler` —— 流式中的 Enter 走
/// `prompt(text,{streamingBehavior:"steer"})`，落 `agent-session.ts#_steeringMessages`。
///
/// 判据：**只**进 steer 队列。followUp 队列必须还是空的——两条队列是
/// 两件不同的事，混在一起就没有「Alt+Enter 等 agent 做完再跑」这层意思了。
#[test]
fn a_submit_while_a_turn_is_running_goes_to_the_steer_queue() {
    // 沙箱：键位解析会读 `<agent dir>/keybindings.json`（用户自己的文件）。
    // 这里要的是**默认**键位，于是把 agent 目录指到一个空目录去。
    let _sandbox = nini_core::paths::agent_lock();

    let mut state = AppState::new("test-model");
    state.run_state.mode = RunMode::Running; // 回合在跑
    type_text(&mut state, "steer me");

    apply_action(&mut state, Key::enter());

    assert_eq!(
        state.run_state.pending_steer_messages,
        vec!["steer me".to_string()],
        "流式中的 Enter 必须进 steer 队列（Pi：streamingBehavior:\"steer\"）"
    );
    assert!(
        state.run_state.pending_followup_messages.is_empty(),
        "Enter 不得进 followUp 队列：{:?}",
        state.run_state.pending_followup_messages
    );
}

// ── 2. 流式中的 Alt+Enter → followUp ──────────────────────────────────────

/// Pi: `interactive-mode.ts#handleFollowUp` —— Alt+Enter 走
/// `prompt(text,{streamingBehavior:"followUp"})`，落 `agent-session.ts#_followUpMessages`。
///
/// 判据：**只**进 followUp 队列。
#[test]
fn alt_enter_while_a_turn_is_running_goes_to_the_followup_queue() {
    let _sandbox = nini_core::paths::agent_lock();

    let mut state = AppState::new("test-model");
    state.run_state.mode = RunMode::Running; // 回合在跑
    type_text(&mut state, "later");

    apply_action(&mut state, Key::enter().with_modifiers(KeyModifiers::ALT));

    assert_eq!(
        state.run_state.pending_followup_messages,
        vec!["later".to_string()],
        "流式中的 Alt+Enter 必须进 followUp 队列（Pi：streamingBehavior:\"followUp\"）"
    );
    assert!(
        state.run_state.pending_steer_messages.is_empty(),
        "Alt+Enter 不得进 steer 队列：{:?}",
        state.run_state.pending_steer_messages
    );
}

// ── 3. 压缩期两分 ────────────────────────────────────────────────────────

/// Pi: `interactive-mode.ts#queueCompactionMessage(text, mode)` —— 压缩期间
/// Enter 进 `"steer"` 桶、Alt+Enter 进 `"followUp"` 桶，**两个桶**。
/// 压缩期间**不起新的一轮**：消息等着，压缩完了再说。
///
/// 这是「两条队列」最容易露馅的地方：压缩期是**另一个**入队口，
/// 不是复用流式那条路——两处的 kind 分派必须一致。
#[tokio::test]
async fn during_compaction_enter_is_steer_and_the_followup_entry_is_followup() {
    let shared = fresh_state("compaction");
    let spawns = Arc::new(AtomicUsize::new(0));
    let driver = counting_driver(Arc::clone(&spawns));
    let (tx, _rx) = tokio::sync::watch::channel(false);

    // 压缩开始：agent 报 Compacting 相位 → sink 把它变成状态。
    // 用**标签**（"compacting"）而不是 Display（"compacting:overflow"）：
    // runtime 里判的是精确相等，Display 一变这个判断就静默失效。
    {
        let sink = AgentSink::new(shared.clone(), tx.clone());
        sink.push(AgentEventLite::PhaseChanged(
            nini_core::agent::AgentPhase::Compacting {
                reason: "overflow".into(),
                progress: None,
            }
            .tag()
            .to_string(),
        ));
    }
    assert!(
        shared.lock().unwrap().run_state.is_compacting,
        "前置条件：压缩标志应当立着"
    );

    // Enter → steer
    {
        let mut g = shared.lock().unwrap();
        type_text(&mut g, "steer during compaction");
    }
    submit_user_input(
        &shared,
        &driver,
        Arc::new(tokio::sync::Notify::new()),
        tx.clone(),
    );

    // Alt+Enter（kind = FollowUp 的那个入口）→ followUp
    {
        let mut g = shared.lock().unwrap();
        type_text(&mut g, "followup during compaction");
    }
    submit_user_input_with_kind(
        &shared,
        &driver,
        Arc::new(tokio::sync::Notify::new()),
        tx.clone(),
        QueueKind::FollowUp,
    );

    assert_eq!(
        spawns.load(Ordering::SeqCst),
        0,
        "压缩期间不得另起一轮（Pi：queueCompactionMessage 只是入队）"
    );
    let g = shared.lock().unwrap();
    assert_eq!(
        g.run_state.pending_steer_messages,
        vec!["steer during compaction".to_string()],
        "压缩期的 Enter 必须进 steer 桶"
    );
    assert_eq!(
        g.run_state.pending_followup_messages,
        vec!["followup during compaction".to_string()],
        "压缩期的 followUp 入口必须进 followUp 桶"
    );
}

// ── 4. Esc：两个队列一起退回，steer 在前 ─────────────────────────────────

/// Pi: `interactive-mode.ts#restoreQueuedMessagesToEditor` —— 退回编辑器时
/// 拼的是 `[...steering, ...followUp]`：**steer 在前**。
/// `interactive-mode.ts#clearAllQueues` —— 两个队列一起清。
///
/// 顺序不是审美问题：退回编辑器的那段文字是用户**要编辑**的内容，
/// steer 是「打断当前这一轮的话」、followUp 是「做完这件事再做那个」，
/// 顺序反了，语义就反了。
#[test]
fn escape_returns_both_queues_to_the_editor_with_steer_first() {
    let _sandbox = nini_core::paths::agent_lock();

    let mut state = AppState::new("test-model");
    state.run_state.mode = RunMode::Running; // 回合在跑 → Esc 是「中止这一轮」
    state
        .run_state
        .pending_steer_messages
        .push("S1".to_string());
    state
        .run_state
        .pending_followup_messages
        .push("F1".to_string());

    apply_action(&mut state, Key::esc());

    let text = state.input.text.clone();
    let steer_at = text
        .find("S1")
        .expect("steer 队列的消息必须回到编辑器（Pi：restoreQueuedMessagesToEditor）");
    let followup_at = text
        .find("F1")
        .expect("followUp 队列的消息必须回到编辑器（Pi：restoreQueuedMessagesToEditor）");
    assert!(
        steer_at < followup_at,
        "steer 必须在 followUp 之前（Pi 拼的是 [...steering, ...followUp]）：{text:?}"
    );
    assert!(
        state.run_state.pending_steer_messages.is_empty(),
        "退回之后 steer 队列必须清空"
    );
    assert!(
        state.run_state.pending_followup_messages.is_empty(),
        "退回之后 followUp 队列必须清空"
    );

    // 退回的**条数**：两个队列里的消息，一条都不能少。
    let mut other = AppState::new("test-model");
    other
        .run_state
        .pending_steer_messages
        .push("S1".to_string());
    other
        .run_state
        .pending_followup_messages
        .push("F1".to_string());
    assert_eq!(
        return_queue_to_editor(&mut other),
        2,
        "两个队列的消息必须一起退回编辑器（Pi：clearAllQueues 一次清两个）"
    );
}

// ── 5. 两条队列互不越界 ───────────────────────────────────────────────────

/// Pi: `agent-session.ts#_steeringMessages` 与 `agent-session.ts#_followUpMessages`
/// 是**两个**队列；取 steer 不能把 followUp 一起取走，反之亦然。
///
/// 判据：取走一条之后，另一条**还在原地**。
#[test]
fn taking_one_queue_never_takes_the_other() {
    let mut state = AppState::new("test-model");
    state
        .run_state
        .pending_steer_messages
        .push("S1".to_string());
    state
        .run_state
        .pending_followup_messages
        .push("F1".to_string());

    let steer = take_steer_messages(&mut state);
    assert_eq!(steer.len(), 1, "steer 队列里有一条");
    assert!(
        provider_text(&steer[0]).contains("S1"),
        "取出来的必须是 steer 那条：{:?}",
        provider_text(&steer[0])
    );
    assert!(
        state.run_state.pending_steer_messages.is_empty(),
        "取完的 steer 队列必须空"
    );
    assert_eq!(
        state.run_state.pending_followup_messages,
        vec!["F1".to_string()],
        "取 steer 不得动 followUp 队列——它们是两条独立的队列"
    );

    let followup = take_followup_messages(&mut state);
    assert_eq!(followup.len(), 1, "followUp 队列里有一条");
    assert!(
        provider_text(&followup[0]).contains("F1"),
        "取出来的必须是 followUp 那条：{:?}",
        provider_text(&followup[0])
    );
    assert!(
        state.run_state.pending_followup_messages.is_empty(),
        "取完的 followUp 队列必须空"
    );
}

// ── 6. bash 输出不进队列，但进 session ───────────────────────────────────

/// Pi: `agent-session.ts#recordBashResult` —— bash 的结果记进 session 的
/// 活消息表，它**不是**用户说的话。
///
/// 判据两半：
/// * **不进队列**：进了队列，Esc 会把它塞回输入框、steering 钩子会把它当成
///   用户的话插进当前这一轮——而它本来不是用户说的；
/// * **进 session**：否则 `/resume` 之后这段输出就没了。
#[tokio::test]
async fn a_bash_command_is_not_queued_but_is_recorded_in_the_session() {
    let shared = fresh_state("bash");
    let spawns = Arc::new(AtomicUsize::new(0));
    let driver = counting_driver(Arc::clone(&spawns));
    let (tx, _rx) = tokio::sync::watch::channel(false);

    {
        let mut g = shared.lock().unwrap();
        type_text(&mut g, "!echo nini-queue-routing");
    }
    submit_user_input(
        &shared,
        &driver,
        Arc::new(tokio::sync::Notify::new()),
        tx.clone(),
    );

    {
        let g = shared.lock().unwrap();
        assert!(
            g.run_state.pending_steer_messages.is_empty(),
            "bash 输出不是用户的话，不得进 steer 队列：{:?}",
            g.run_state.pending_steer_messages
        );
        assert!(
            g.run_state.pending_followup_messages.is_empty(),
            "bash 输出不是用户的话，不得进 followUp 队列：{:?}",
            g.run_state.pending_followup_messages
        );
    }

    // session 里有一条 User 消息，内容是这段输出（Pi：recordBashResult）
    let g = shared.lock().unwrap();
    let session = g
        .session_state
        .session
        .as_ref()
        .expect("这个 state 初始化过 session");
    let guard = session.lock().unwrap();
    let user_messages: Vec<String> = guard
        .entries
        .iter()
        .filter_map(|e| match e {
            nini_core::SessionEntry::Message(m) => match &m.message {
                nini_core::entries::AgentMessage::User(u) => Some(user_text(u)),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert!(
        user_messages
            .iter()
            .any(|t| t.contains("[bash $ echo nini-queue-routing")),
        "bash 的结果必须进 session（Pi：recordBashResult）：{user_messages:?}"
    );
}

// ── 7. followUp 这个动作是可达的 ────────────────────────────────────────

/// Pi: `interactive-mode.ts#handleFollowUp` 是接到 `app.message.followUp`
/// 这个动作上的。动作名有三种写法（nini 的 camelCase、Pi 的 camelCase 变体、
/// Pi 键位文件里的带点名），**都得认**——用户照抄一份 Pi 的键位文件到 nini
/// 不该一行都不生效。
///
/// 判据：名字认得、默认键位（Alt+Enter）也认得，而 Enter 仍然是 Submit
/// （steer）——两个键不能互相吞掉。
#[test]
fn the_followup_action_is_reachable_by_name_and_by_key() {
    use nini_tui::keybindings_manager::parse_action;

    assert_eq!(
        parse_action("app.message.followUp"),
        Some(KeyAction::FollowUp),
        "Pi 键位文件里的带点名必须认"
    );
    assert_eq!(
        parse_action("FollowUp"),
        Some(KeyAction::FollowUp),
        "nini 的 camelCase 动作名必须认"
    );
    assert_eq!(
        parse_action("followUp"),
        Some(KeyAction::FollowUp),
        "Pi 的 camelCase 动作名必须认"
    );

    let keymap = nini_tui::default_keymap();
    assert!(
        keymap
            .iter()
            .any(|b| b.key == Key::enter().with_modifiers(KeyModifiers::ALT)
                && b.action == KeyAction::FollowUp),
        "Alt+Enter 必须是 followUp（Pi 的默认键）"
    );
    assert!(
        keymap
            .iter()
            .any(|b| b.key == Key::enter() && b.action == KeyAction::Submit),
        "Enter 仍然是 Submit（steer）——两个键不能互相吞掉"
    );
}
