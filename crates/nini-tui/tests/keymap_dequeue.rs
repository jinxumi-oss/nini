//! R14 最后一项：把排队的消息取回编辑器（Pi 的 `app.message.dequeue`）。
//!
//! # Pi 的证据
//!
//! * 键：Pi 的 `references/pi-0.85.1/src/ui/keybindings.ts:138` ——
//!   `"app.message.dequeue": { defaultKeys: windowsKeybindings ? "alt+q" : "alt+up" }`，
//!   即 Linux/macOS 上是 **Alt+Up**。
//! * 行为：`interactive-mode.ts:4157` 的 `handleDequeue()` 调
//!   `restoreQueuedMessagesToEditor()`（`:4389`），**不带**参数。
//! * `restoreQueuedMessagesToEditor()` 的语义（逐字照抄 Pi 的实现）：
//!   1. `clearAllQueues()` —— **两个**队列（steer 与 followUp）**一起**取空；
//!   2. `allQueued = [...steering, ...followUp]` —— **steer 在前、followUp 在后**；
//!   3. `queuedText = allQueued.join("\n\n")`；
//!   4. `combinedText = [queuedText, currentText].filter(t => t.trim()).join("\n\n")`
//!      —— 排队的内容排在**输入框原有文字的前面**，空白的部分被丢掉；
//!   5. `editor.setText(combinedText)`，队列从此为空（否则消息会被投递两次）。
//!
//! `restoreQueuedMessagesToEditor` 的另一个参数 `{ abort: true }` 是
//! **Esc 中断回合**时用的那一支（取回之后顺带 `agent.abort()`，
//! `interactive-mode.ts:1864` / `:2050` / `:2854`）。`handleDequeue()`
//! 走的是**不带** abort 的那一支 —— 取回**不**中止当前回合。
//!
//! # nini 的口径
//!
//! nini 此前**没有**这个入口：排队的消息只能等它被投递，用户改主意时取不回来。
//! 这里加 `KeyAction::DequeueMessages`，键是 **Alt+Up**（与 Pi 相同，
//! 且 nini 此前没有任何键占着 Alt+Up）。

use nini_tui::keys::{default_keymap, resolve, Key, KeyAction, KeyCode, KeyModifiers};
use nini_tui::runtime::apply_action;
use nini_tui::state::{AppState, TranscriptLine};

/// Pi 的 `app.message.dequeue` 默认键（Linux/macOS）：Alt+Up。
fn alt_up() -> Key {
    Key::new(KeyCode::Up, KeyModifiers::ALT)
}

/// 转录里所有的系统提示（`push_notice` 写进去的那些）。
fn notices(state: &AppState) -> Vec<String> {
    state
        .transcript_state
        .lines
        .iter()
        .filter_map(|l| match l {
            TranscriptLine::Notice(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

/// 用户能看到的那句话（状态栏 + 转录提示）。
fn said(state: &AppState) -> Vec<String> {
    let mut out = notices(state);
    out.push(state.run_state.status.clone());
    out
}

// ====================================================================
// 绑定
// ====================================================================

#[test]
fn alt_up_binds_dequeue_messages() {
    assert_eq!(
        resolve(&default_keymap(), alt_up()),
        KeyAction::DequeueMessages,
        "Alt+Up 必须是 DequeueMessages（Pi 的 app.message.dequeue）"
    );
    // 绑定表里也要真的查得到这一格（不是 resolve 的兜底逻辑蒙对的）。
    let bound = default_keymap()
        .iter()
        .find(|b| b.key == alt_up())
        .map(|b| b.action);
    assert_eq!(bound, Some(KeyAction::DequeueMessages));
}

/// 键不能与别的动作抢：`resolve` 返回**第一个**匹配，所以同一个键绑两次
/// 时后一个永远按不到。
#[test]
fn alt_up_is_not_bound_to_anything_else() {
    let km = default_keymap();
    let hits: Vec<KeyAction> = km
        .iter()
        .filter(|b| b.key == alt_up())
        .map(|b| b.action)
        .collect();
    assert_eq!(hits.len(), 1, "Alt+Up 被绑了 {hits:?}——只能有一个");
}

// ====================================================================
// 队列为空
// ====================================================================

/// 队列为空 → **必须给用户看提示**（Pi: `showStatus("No queued messages to
/// restore")`），输入框一个字都不能动。
#[test]
fn an_empty_queue_says_so_and_leaves_the_input_alone() {
    let mut state = AppState::new("test-model");
    for c in "typed".chars() {
        state.input.insert_char(c);
    }

    apply_action(&mut state, alt_up());

    assert_eq!(
        state.input.text, "typed",
        "没有排队消息时输入框不能变（Pi: restoreQueuedMessagesToEditor 先返回 0）"
    );
    let said = said(&state);
    assert!(
        said.iter().any(|s| s.contains("No queued messages")),
        "必须告诉用户「没有可取回的排队消息」，得到：{said:?}"
    );
}

// ====================================================================
// 一条消息
// ====================================================================

/// 队列里有一条 → 输入框里出现那条消息，**队列被清空**。
#[test]
fn one_queued_message_lands_in_the_editor_and_empties_the_queue() {
    let mut state = AppState::new("test-model");
    state
        .run_state
        .pending_steer_messages
        .push("remember the milk".into());

    apply_action(&mut state, alt_up());

    assert_eq!(
        state.input.text, "remember the milk",
        "排队的消息必须逐字进入输入框"
    );
    assert!(
        state.run_state.pending_steer_messages.is_empty(),
        "取回后 steer 队列必须清空（否则消息会被投递两次）"
    );
    assert!(
        state.run_state.pending_followup_messages.is_empty(),
        "取回后 followUp 队列必须清空"
    );
    let said = said(&state);
    assert!(
        said.iter().any(|s| s.contains("Restored 1 queued message")),
        "必须告诉用户取回了几条，得到：{said:?}"
    );
}

/// 再取一次就是空的：队列**确实**被清空了，不是复制了一份。
#[test]
fn dequeuing_twice_yields_nothing_the_second_time() {
    let mut state = AppState::new("test-model");
    state
        .run_state
        .pending_followup_messages
        .push("second thought".into());

    apply_action(&mut state, alt_up());
    assert_eq!(state.input.text, "second thought");

    // 第二次：队列已经空了，提示必须变成「没有可取回的」，输入框不能变。
    apply_action(&mut state, alt_up());
    assert_eq!(
        state.input.text, "second thought",
        "第二次取回不能改动输入框（队列已经空了）"
    );
    let said = said(&state);
    assert!(
        said.iter().any(|s| s.contains("No queued messages")),
        "第二次必须说「没有可取回的排队消息」，得到：{said:?}"
    );
}

// ====================================================================
// 多条消息：合并语义
// ====================================================================

/// 多条 → 按 Pi 的语义合并。
///
/// Pi: `allQueued = [...steering, ...followUp]`（**steer 在前**），
/// `queuedText = allQueued.join("\n\n")`。
#[test]
fn several_queued_messages_are_joined_by_a_blank_line_steer_first() {
    let mut state = AppState::new("test-model");
    state.run_state.pending_steer_messages.push("first".into());
    state.run_state.pending_steer_messages.push("second".into());
    state
        .run_state
        .pending_followup_messages
        .push("third".into());

    apply_action(&mut state, alt_up());

    assert_eq!(
        state.input.text, "first\n\nsecond\n\nthird",
        "合并结果必须与 Pi 逐字相同（steer 在前、followUp 在后、空行连接）"
    );
    let said = said(&state);
    assert!(
        said.iter()
            .any(|s| s.contains("Restored 3 queued messages")),
        "必须报出取回的条数（复数），得到：{said:?}"
    );
}

/// 两个队列都要取：只取 steer、放过 followUp 的话，用户的一半排队消息
/// 就永远取不回来了。
#[test]
fn both_queues_are_taken() {
    let mut state = AppState::new("test-model");
    state
        .run_state
        .pending_steer_messages
        .push("steered".into());
    state
        .run_state
        .pending_followup_messages
        .push("followed up".into());

    apply_action(&mut state, alt_up());

    assert_eq!(
        state.input.text, "steered\n\nfollowed up",
        "两个队列一起取，steer 在前"
    );
    assert!(state.run_state.pending_steer_messages.is_empty());
    assert!(state.run_state.pending_followup_messages.is_empty());
}

// ====================================================================
// 输入框里本来有文字
// ====================================================================

/// 编辑器里本来有文字 → Pi 把**排队的内容排在前面**，
/// 两者之间是空行。
///
/// Pi: `combinedText = [queuedText, currentText].filter(t => t.trim()).join("\n\n")`
#[test]
fn queued_messages_go_before_the_text_already_in_the_editor() {
    let mut state = AppState::new("test-model");
    for c in "already typed".chars() {
        state.input.insert_char(c);
    }
    state
        .run_state
        .pending_steer_messages
        .push("queued one".into());
    state
        .run_state
        .pending_followup_messages
        .push("queued two".into());

    apply_action(&mut state, alt_up());

    assert_eq!(
        state.input.text, "queued one\n\nqueued two\n\nalready typed",
        "Pi 把排队的内容接在**已有文字的前面**（queuedText 在前）"
    );
}

/// 输入框里只有空白 → Pi 的 `filter(t => t.trim())` 把这一段**整段丢掉**
/// （不是保留空白、也不是 trim 后留下空行）。
#[test]
fn a_whitespace_only_editor_is_dropped_entirely() {
    let mut state = AppState::new("test-model");
    for c in "   ".chars() {
        state.input.insert_char(c);
    }
    state.run_state.pending_steer_messages.push("queued".into());

    apply_action(&mut state, alt_up());

    assert_eq!(
        state.input.text, "queued",
        "空白的那一段被整段丢掉（Pi: filter(t => t.trim())）"
    );
}

/// 取回后光标落在末尾：Pi 的 `editor.setText(combinedText)` 之后光标在
/// 文本末尾，用户接着改的就是刚取回来的内容。
#[test]
fn the_caret_ends_up_at_the_end_of_the_restored_text() {
    let mut state = AppState::new("test-model");
    state.run_state.pending_steer_messages.push("queued".into());

    apply_action(&mut state, alt_up());

    assert_eq!(
        state.input.cursor,
        state.input.text.len(),
        "光标必须落在取回后的文本末尾"
    );
}

/// 取回**不**中止当前回合：Pi 的 `handleDequeue()` 走的是
/// `restoreQueuedMessagesToEditor()`（**不带** `{ abort: true }`），
/// 那个 abort 参数是 Esc 中断回合用的。
#[test]
fn dequeuing_does_not_abort_the_turn() {
    use nini_tui::state::RunMode;
    let mut state = AppState::new("test-model");
    state.run_state.mode = RunMode::Running;
    state.run_state.pending_steer_messages.push("queued".into());

    apply_action(&mut state, alt_up());

    assert_eq!(
        state.run_state.mode,
        RunMode::Running,
        "取回排队消息不等于中止回合（Pi: handleDequeue 不带 abort）"
    );
}
