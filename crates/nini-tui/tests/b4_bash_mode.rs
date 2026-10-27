//! B4: bash mode 的语义对齐 Pi 0.85.1。
//!
//! 三条钉住的东西：
//!
//! 1. **`!cmd` 的输出不进模型上下文**——Pi 的 bash mode 是本地逃逸
//!    （`interactive-mode.ts#handleBashCommand` → 输出只进 transcript 与
//!    session 的 `bashExecution` 消息，而 `bashExecution` 在会话转换里
//!    **不进 LLM 上下文**）。此前 nini 把输出以一条 **User 消息**冲刷进
//!    session，于是模型下一轮就看见了它。
//! 2. **`!!cmd` 仍然不注入**（隐私标记，v0.6 起就有）。
//! 3. **输入以 `!` 开头时按 Esc → 清空输入并退出 bash mode**，而且
//!    **不中止回合**（Pi: `interactive-mode.ts:2857-2861`）。
//!
//! 附带一条：bash mode 下编辑器的边框用 Pi 的 `bashMode` 主题槽
//! （`interactive-mode.ts:4167-4171` → `theme.getBashModeBorderColor()`）。
//!
//! A4（v0.11）钉住的那条**不许**被破坏：**自动投递的文本不重新解释**
//! `!` 与 `/`——排队的 `!cmd` 不会被执行。这条由
//! `tests/auto_delivery_no_loss.rs` 覆盖，本文件只补一条「A4 的标志
//! 还在」的正向断言，确保 B4 没有把解释的口子重新打开。

#![allow(clippy::let_underscore_future, clippy::let_underscore_must_use)]

use nini_tui::runtime::{escape_action, submit_user_input, EscapeAction};
use nini_tui::state::{AppState, RunMode, TranscriptLine};
use nini_tui::Key;
use std::sync::Arc;

/// The messages the model would actually see.
///
/// This is the conversion the session reader uses
/// (`nini-core/src/conversion.rs`): `bashExecution` entries are dropped
/// there (`PiMsg::BashExecution(_) => None`), a **user** message is not.
/// So "is the output in the model's context?" is exactly "is there a user
/// message carrying it?".
fn model_visible_messages(state: &AppState) -> Vec<nini_core::provider::Message> {
    let Some(arc) = state.session_state.session.clone() else {
        return Vec::new();
    };
    let guard = arc.lock().unwrap();
    guard
        .entries
        .iter()
        .filter_map(nini_core::conversion::session_entry_to_llm_message)
        .collect()
}

fn message_text(m: &nini_core::provider::Message) -> String {
    let mut out = String::new();
    for b in &m.content {
        match b {
            nini_core::provider::ContentBlock::Text { text } => out.push_str(text),
            _ => {}
        }
    }
    out
}

/// A driver that records the turns it was asked to start. A `!cmd` must
/// never reach it — a local escape does not start a turn.
fn counting_driver(counter: Arc<std::sync::atomic::AtomicUsize>) -> nini_tui::runtime::AgentDriver {
    Arc::new(move |text, _, _, _| {
        counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let _ = text;
        tokio::spawn(async {})
    })
}

fn scratch(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("nini-b4-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The state a user is in when they type a command: idle, editing, with a
/// session to write into.
fn state_with_input(
    text: &str,
    tag: &str,
) -> (Arc<std::sync::Mutex<AppState>>, std::path::PathBuf) {
    let dir = scratch(tag);
    let path = dir.join("s.jsonl");
    let mut s = AppState::new("test-model");
    s.session_init(path.clone());
    s.input.text = text.to_string();
    s.input.cursor = text.len();
    let shared: Arc<std::sync::Mutex<AppState>> = Arc::new(std::sync::Mutex::new(s));
    (shared, dir)
}

fn bash_lines(state: &AppState) -> Vec<(String, String)> {
    state
        .transcript_state
        .lines
        .iter()
        .filter_map(|l| match l {
            TranscriptLine::BashExecution { cmd, output, .. } => {
                Some((cmd.clone(), output.clone()))
            }
            _ => None,
        })
        .collect()
}

/// ── (a) `!cmd` 的输出**进**模型上下文（Pi 的语义）──────────────────────
///
/// Pi 自己写下了这条分界线（`agent-session.ts:2979`）：
/// 「If true, command output won't be sent to LLM (**!!** prefix)」——
/// 于是 `!cmd` 的输出**是**发给 LLM 的，只有 `!!cmd` 才排除。
///
/// nini 的会话转换会丢掉 `bashExecution` 角色的条目
/// （`crates/nini-core/src/conversion.rs:172`），所以 nini 要让模型看见这段
/// 输出，只能把它写成一条 **User** 消息。
///
/// （本条断言**曾经**写反过：它断言 `!cmd` 的输出不进模型上下文。那与 Pi
/// 的注释矛盾——已按 Pi 的语义改正，并在下面把「模型看得见」钉住。）
#[tokio::test]
async fn a_bang_command_output_reaches_the_model_context() {
    let marker = format!("nini-b4-marker-{}", std::process::id());
    let (shared, dir) = state_with_input(&format!("!echo {marker}"), "bang");

    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    submit_user_input(
        &shared,
        &counting_driver(counter.clone()),
        Arc::new(tokio::sync::Notify::new()),
        tokio::sync::watch::channel(false).0,
    );

    let g = shared.lock().unwrap();

    // The command ran locally …
    let lines = bash_lines(&g);
    assert_eq!(lines.len(), 1, "the command must have run: {lines:?}");
    assert!(
        lines[0].1.contains(&marker),
        "the user must still see the output: {lines:?}"
    );

    // … and it did **not** start a turn: a local escape is not a message.
    assert_eq!(
        counter.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "`!cmd` must not start an agent turn"
    );

    // The model's view of the conversation **does** carry the output: Pi
    // sends `!cmd`'s output to the LLM (only `!!` excludes it).
    let visible = model_visible_messages(&g);
    assert!(
        visible.iter().any(|m| message_text(m).contains(&marker)),
        "the output of `!cmd` must reach the model (Pi: only `!!` excludes it): {visible:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// ── (b) `!!cmd` 仍然不注入 ────────────────────────────────────────────
#[tokio::test]
async fn a_double_bang_command_output_never_reaches_the_model_context() {
    let marker = format!("nini-b4-private-{}", std::process::id());
    let (shared, dir) = state_with_input(&format!("!!echo {marker}"), "double-bang");

    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    submit_user_input(
        &shared,
        &counting_driver(counter.clone()),
        Arc::new(tokio::sync::Notify::new()),
        tokio::sync::watch::channel(false).0,
    );

    let g = shared.lock().unwrap();

    let lines = bash_lines(&g);
    assert_eq!(lines.len(), 1, "the command must have run: {lines:?}");
    assert!(
        lines[0].0.starts_with("!!"),
        "the privacy marker stays visible to the user: {lines:?}"
    );
    assert_eq!(
        counter.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "`!!cmd` must not start an agent turn"
    );

    let visible = model_visible_messages(&g);
    assert!(
        !visible.iter().any(|m| message_text(m).contains(&marker)),
        "the output of `!!cmd` must not be in the model's context: {visible:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// ── (c) Esc 退出 bash mode，且不中止回合 ─────────────────────────────
#[test]
fn escape_in_bash_mode_clears_the_input_and_aborts_nothing() {
    let mut s = AppState::new("test-model");
    s.run_state.mode = RunMode::Editing;
    s.input.text = "!ls -la".to_string();
    s.input.cursor = s.input.text.len();
    // Something the user queued must survive: Esc here aborts nothing.
    s.run_state
        .pending_steer_messages
        .push("a message I queued".to_string());

    assert!(s.is_bash_mode(), "the input is a command");

    assert_eq!(
        escape_action(&s),
        EscapeAction::ExitBashMode,
        "Esc in bash mode must exit bash mode, not clear a plain input"
    );

    nini_tui::runtime::apply_action(&mut s, Key::esc());

    assert!(
        s.input.text.is_empty(),
        "Esc in bash mode must clear the command: {:?}",
        s.input.text
    );
    assert!(!s.is_bash_mode(), "and the editor must be out of bash mode");
    assert_eq!(
        s.run_state.mode,
        RunMode::Editing,
        "Esc in bash mode must not abort the turn"
    );
    assert_ne!(
        s.run_state.status, "aborted",
        "Esc in bash mode must not report an abort"
    );
    assert_eq!(
        s.run_state.pending_steer_messages,
        vec!["a message I queued".to_string()],
        "Esc in bash mode must not touch the queue"
    );
}

/// Pi's order: streaming first (`isStreaming` → abort), bash mode second.
/// nini's "the turn is running" is `mode == Running` / `EscapeTarget::Agent`.
#[test]
fn escape_while_a_turn_is_running_still_aborts_the_turn() {
    let mut s = AppState::new("test-model");
    s.run_state.mode = RunMode::Running;
    s.input.text = "!ls -la".to_string();

    assert_eq!(
        escape_action(&s),
        EscapeAction::AbortTurn,
        "a running turn outranks bash mode (Pi: `isStreaming` is checked first)"
    );
}

/// A plain (non-bash) input still takes the old path: Esc clears it.
#[test]
fn escape_on_a_plain_input_still_clears_it() {
    let mut s = AppState::new("test-model");
    s.run_state.mode = RunMode::Editing;
    s.input.text = "hello".to_string();

    assert_eq!(escape_action(&s), EscapeAction::ClearInput);
    nini_tui::runtime::apply_action(&mut s, Key::esc());
    assert!(s.input.text.is_empty());
}

/// ── bash mode 的边框色 ────────────────────────────────────────────────
fn prompt_border_colour(state: &AppState) -> ratatui::style::Color {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let theme = nini_tui::theme::Theme::default();
    let backend = TestBackend::new(60, 20);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| nini_tui::render::render_frame_with_theme(f, state, &theme))
        .unwrap();
    let buf = terminal.backend().buffer().clone();
    // The prompt box's top-left corner is the only box-drawing corner on
    // an empty screen.
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            if let Some(cell) = buf.cell((x, y)) {
                if cell.symbol() == "┌" {
                    return cell.fg;
                }
            }
        }
    }
    panic!("the prompt box was not drawn");
}

#[test]
fn the_prompt_border_uses_the_bash_mode_colour_while_a_command_is_typed() {
    let mut s = AppState::new("test-model");
    s.input.text = "hello".to_string();
    let plain = prompt_border_colour(&s);

    s.input.text = "!ls -la".to_string();
    let bash = prompt_border_colour(&s);

    let theme = nini_tui::theme::Theme::default();
    assert_eq!(
        plain,
        theme.color("borderMuted"),
        "a plain prompt keeps the muted border"
    );
    assert_eq!(
        bash,
        theme.color("bashMode"),
        "a command in the editor gets Pi's bash-mode border colour"
    );
    assert_ne!(
        plain, bash,
        "the two borders must actually differ, or the test proves nothing"
    );
}

/// ── A4 的那条不许被破坏 ──────────────────────────────────────────────
/// 排队的消息**原样**送出：不被重新解释成命令。
#[tokio::test]
async fn a_queued_command_is_still_not_executed_when_delivered() {
    let dir = scratch("queued");
    let path = dir.join("s.jsonl");
    let mut s = AppState::new("test-model");
    s.session_init(path.clone());
    s.run_state.mode = RunMode::Editing;
    s.run_state.agent_alive = false;
    s.run_state.auto_turn_pending = true;
    s.run_state
        .pending_steer_messages
        .push("!echo should-not-run".to_string());
    let shared: Arc<std::sync::Mutex<AppState>> = Arc::new(std::sync::Mutex::new(s));

    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let started = nini_tui::runtime::submit_queued_turn(
        &shared,
        &counting_driver(counter.clone()),
        Arc::new(tokio::sync::Notify::new()),
        tokio::sync::watch::channel(false).0,
    );

    assert!(started, "the queued message must have been delivered");
    let g = shared.lock().unwrap();
    assert!(
        bash_lines(&g).is_empty(),
        "a queued `!cmd` must be sent as text, not executed"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
