//! R14 第一批键位对齐的 oracle：Ctrl+U / Ctrl+X / Ctrl+Z。
//!
//! # 这一批做了什么
//!
//! | 键 | 动作 | Pi 的 binding id |
//! |----|------|------------------|
//! | Ctrl+U | `KillToLineStart` | `tui.editor.deleteToLineStart` |
//! | Ctrl+X | `CopyLastAssistant` | `app.message.copy` |
//! | Ctrl+Z | `Suspend` | `app.suspend` |
//!
//! 键位**语义层**的对照由 `scripts/keymap-parity.py` + `scripts/keymap-parity.json`
//! 负责（`tests/keymap_parity.rs` 证明那个 oracle 能红）。本文件负责另一件事：
//! **动作的行为**——绑定对了不等于行为对了。
//!
//! # 为什么每条都要断言行为
//!
//! * Ctrl+U：nini 的 `KillToLineStart` 动作此前**没有任何键绑定**，而且它的
//!   实现删到的是**上一个词**的边界（`prev_word_boundary`），不是行首。
//!   只改键位不修实现，parity 契约里写的 `delete-to-start` 就是一句假话。
//! * Ctrl+X：复制**必须让用户看见结果**——成功报字节数，失败说原因。
//!   静默的复制等于没复制（`/copy` 早就因为这个被修过一次）。
//! * Ctrl+Z：Windows 上必须**显式拒绝**（Pi 的原话），不能静默无事发生。

use nini_tui::keys::{default_keymap, resolve, Key, KeyAction, KeyCode, KeyModifiers};
use nini_tui::runtime::apply_action;
use nini_tui::state::{AppState, TranscriptLine};

const CTRL: KeyModifiers = KeyModifiers::CTRL;

fn ctrl(c: char) -> Key {
    Key::new(KeyCode::Char(c), CTRL)
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

// ====================================================================
// Ctrl+U → KillToLineStart
// ====================================================================

#[test]
fn ctrl_u_binds_kill_to_line_start() {
    assert_eq!(
        resolve(&default_keymap(), ctrl('u')),
        KeyAction::KillToLineStart,
        "Ctrl+U 必须是 KillToLineStart（Pi 的 tui.editor.deleteToLineStart）"
    );
    // 绑定表里也要真的查得到这一格（不是 resolve 的兜底逻辑蒙对的）。
    let bound = default_keymap()
        .iter()
        .find(|b| b.key == ctrl('u'))
        .map(|b| b.action);
    assert_eq!(bound, Some(KeyAction::KillToLineStart));
}

/// 删到**行首**，不是删掉整个输入框，也不是删掉上一个词。
///
/// 三行输入、光标在第二行中间：Ctrl+U 只能删掉第二行光标左边的部分，
/// 第一行与第三行都不能动。
#[test]
fn ctrl_u_deletes_to_the_line_start_only() {
    let mut state = AppState::new("test-model");
    for c in "one\ntwo\nthree".chars() {
        state.input.insert_char(c);
    }
    // 光标挪到第二行末尾（"two" 之后）。
    state.input.move_to_start();
    for _ in 0..7 {
        state.input.move_right();
    }
    assert_eq!(state.input.cursor, 7);

    apply_action(&mut state, ctrl('u'));

    assert_eq!(
        state.input.text, "one\n\nthree",
        "Ctrl+U 只能删到行首：第一行与第三行必须原样留下"
    );
    assert_eq!(state.input.cursor, 4, "光标落在行首");
}

/// 单行输入时，Ctrl+U 仍然把整行清空——nini 的老用户按 Ctrl+U 清输入框的
/// 肌肉记忆在**单行**场景下不受影响（这一条也是 `tests/tui_e2e.rs`
/// 里 `ctrl_u_clears_input` 断言的行为）。
#[test]
fn ctrl_u_still_empties_a_single_line() {
    let mut state = AppState::new("test-model");
    for c in "discard me".chars() {
        state.input.insert_char(c);
    }
    apply_action(&mut state, ctrl('u'));
    assert_eq!(state.input.text, "");
    assert_eq!(state.input.cursor, 0);
}

// ====================================================================
// Ctrl+X → CopyLastAssistant
// ====================================================================

#[test]
fn ctrl_x_binds_copy_last_assistant() {
    assert_eq!(
        resolve(&default_keymap(), ctrl('x')),
        KeyAction::CopyLastAssistant,
        "Ctrl+X 必须是 CopyLastAssistant（Pi 的 app.message.copy）"
    );
    let bound = default_keymap()
        .iter()
        .find(|b| b.key == ctrl('x'))
        .map(|b| b.action);
    assert_eq!(bound, Some(KeyAction::CopyLastAssistant));
}

/// 没有助手消息时**必须给用户看一句提示**——静默就是 bug。
#[test]
fn copy_last_assistant_without_a_message_says_so() {
    let mut state = AppState::new("test-model");

    apply_action(&mut state, ctrl('x'));

    let said = notices(&state);
    assert!(
        !said.is_empty(),
        "没有助手消息时 Ctrl+X 必须说一句（不能静默）"
    );
    assert!(
        said.iter().any(|s| s.contains("copy")),
        "提示必须说清楚发生了什么，得到：{said:?}"
    );
}

/// 有助手消息时，**成功与失败都要有话说**。
///
/// CI 里没有剪贴板，所以「成功」这一支测不了；能测的是：**不管剪贴板通没通，
/// 转录里都必须出现一条说明**（成功报字节数，失败报原因）。
#[test]
fn copy_last_assistant_always_reports_the_outcome() {
    let mut state = AppState::new("test-model");
    state.push_assistant("the answer is 42");

    apply_action(&mut state, ctrl('x'));

    let said = notices(&state);
    assert!(
        !said.is_empty(),
        "复制之后必须有一句话说明结果（成功报字节数，失败报原因）"
    );
    assert!(
        said.iter()
            .any(|s| s.contains("copied") || s.contains("clipboard")),
        "提示必须说清楚复制成没成，得到：{said:?}"
    );
}

/// 复制的是**最后一条**助手消息，不是第一条。
#[test]
fn copy_last_assistant_takes_the_last_message() {
    let mut state = AppState::new("test-model");
    state.push_assistant("first");
    // 分隔线断开「同一轮」——否则 `push_assistant` 会把两条并进同一行。
    state.push_divider();
    state.push_assistant("second");

    apply_action(&mut state, ctrl('x'));

    let said = notices(&state);
    // 成功时字节数是**最后一条**的长度（"second" = 6 字节），不是第一条的。
    let copied_second = said.iter().any(|s| s.contains("6 bytes"));
    let copied_first = said.iter().any(|s| s.contains("5 bytes"));
    assert!(
        !copied_first,
        "复制的必须是最后一条助手消息（\"second\"），得到：{said:?}"
    );
    assert!(
        copied_second || !said.iter().any(|s| s.contains("bytes")),
        "成功时报的字节数必须是最后一条的长度，得到：{said:?}"
    );
}

// ====================================================================
// Ctrl+Z → Suspend
// ====================================================================

#[test]
fn ctrl_z_binds_suspend() {
    assert_eq!(
        resolve(&default_keymap(), ctrl('z')),
        KeyAction::Suspend,
        "Ctrl+Z 必须是 Suspend（Pi 的 app.suspend）"
    );
    let bound = default_keymap()
        .iter()
        .find(|b| b.key == ctrl('z'))
        .map(|b| b.action);
    assert_eq!(bound, Some(KeyAction::Suspend));
}

/// Unix 上挂起是真的（给进程组发 SIGTSTP），但**测不得**：一挂起整个测试
/// 进程就停住了。所以这里只钉住「动作在、键在」。
#[cfg(not(windows))]
#[test]
fn suspend_exists_on_unix() {
    assert!(default_keymap()
        .iter()
        .any(|b| b.action == KeyAction::Suspend && b.key == ctrl('z')));
}

/// Windows 上必须**显式拒绝**（Pi 的原话："Suspend to background is not
/// supported on Windows"），不能静默无事发生。
///
/// Windows 分支在 Linux 上编译不到，所以这里钉的是**那句话还在**——
/// 它被删掉或者改写，这条测试就红。
#[test]
fn windows_refusal_is_still_written_down() {
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/runtime.rs"),
    )
    .expect("读 runtime.rs");
    assert!(
        src.contains("Suspend to background is not supported on Windows"),
        "Windows 上必须显式拒绝挂起（Pi 的原话），这句话不能消失"
    );
}
