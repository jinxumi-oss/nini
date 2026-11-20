//! 存活指示器（liveness indicator）—— 端到端契约测试。
//!
//! # 为什么需要它
//!
//! 用户痛点：「我无法分辨 nini 是在工作还是静默中断」。
//! 契约见 `docs/plans/liveness-indicator.md`：
//!
//! * 顶部状态行的**相位**由 `run_state.phase` 决定（不再是「Running 就 working、
//!   其它就 idle」的猜测）——所以 Esc 之后 agent 还没收尾时，顶部仍然是
//!   `working`，不再谎报 idle。
//! * 底部提示行最左侧给出**活动指示器**：工具名 + 已跑秒数；长时间收不到
//!   任何事件时升级成 `no progress` 告警。
//!
//! # 断言的是渲染出来的字符串
//!
//! 这里刻意不引用任何内部函数名/私有字段结构：只把 `AppState` 摆成某个样子，
//! 用 `ratatui::backend::TestBackend` 渲染整帧，然后检查用户**真正看到**的
//! 文本。渲染层重写（换布局、换提示行位置）时，只要用户看到的仍然对，测试
//! 就应该继续绿。

use nini_tui::render::render_frame;
use nini_tui::state::{AppState, RunMode};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::time::{Duration, Instant};

/// 渲染整帧并拼成一段文本（逐行用 `\n` 分隔）。
///
/// 抄自 `tests/version_line.rs` 的 `frame_text`：同样的构造 AppState +
/// TestBackend 的写法，不自创一套。
fn frame_text(width: u16, height: u16, state: &AppState) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| render_frame(f, state)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let area = *buffer.area();
    let mut out = String::new();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buffer.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out.push('\n');
    }
    out
}

/// 相位 = working：顶部必须显示 working。
#[test]
fn phase_working_is_visible_in_the_frame() {
    let mut state = AppState::new("test-model");
    state.run_state.phase = "working".to_string();

    let frame = frame_text(120, 30, &state);
    assert!(
        frame.contains("working"),
        "phase=working 时帧里没有 \"working\"，用户分辨不出 nini 在干活：\n{frame}"
    );
}

/// 相位 = compacting：顶部必须显示 compacting（而不是笼统的 working）。
#[test]
fn phase_compacting_is_visible_in_the_frame() {
    let mut state = AppState::new("test-model");
    state.run_state.phase = "compacting".to_string();

    let frame = frame_text(120, 30, &state);
    assert!(
        frame.contains("compacting"),
        "phase=compacting 时帧里没有 \"compacting\"：\n{frame}"
    );
}

/// 有工具在跑：底部必须同时出现**工具名**与**已跑秒数**。
///
/// 30 秒的 bash 和 30 分钟的 bash 长得一样正是本轮要修的缺口，所以两个数字
/// 都得在用户视线所在的那一行上。
#[test]
fn running_tool_shows_its_name_and_elapsed_seconds() {
    let mut state = AppState::new("test-model");
    let now = Instant::now();
    state.run_state.mode = RunMode::Running;
    state.run_state.phase = "working".to_string();
    state.run_state.current_tool = Some(("bash".to_string(), now - Duration::from_secs(12)));

    let frame = frame_text(160, 30, &state);
    assert!(
        frame.contains("bash"),
        "工具在跑时帧里没有工具名 \"bash\"：\n{frame}"
    );
    assert!(
        frame.contains("12s"),
        "工具在跑时帧里没有已跑秒数 \"12s\"：\n{frame}"
    );
}

/// 长时间收不到任何事件（有工具在跑）→ 必须升级成 `no progress` 告警。
///
/// 基准是 `last_event_at`（最后一次收到 agent 事件的时刻），不是本轮开始
/// 时刻：工具跑了很久但事件一直在来，就不该报警。
#[test]
fn a_stale_last_event_shows_the_no_progress_warning() {
    let mut state = AppState::new("test-model");
    let now = Instant::now();
    state.run_state.mode = RunMode::Running;
    state.run_state.phase = "working".to_string();
    state.run_state.current_tool = Some(("bash".to_string(), now - Duration::from_secs(200)));
    state.run_state.last_event_at = now - Duration::from_secs(200);

    let frame = frame_text(160, 30, &state);
    assert!(
        frame.contains("no progress"),
        "200 秒没有任何事件时帧里没有 \"no progress\" 告警：\n{frame}"
    );
}

/// Esc 之后 agent 尚未收尾：mode 回到 Editing，顶部**仍然**是 working。
///
/// 这一条锁的是本轮最大的行为变化：相位不再由 `RunMode` 猜，所以「能打字」
/// 不再等于「agent 死了」。旧实现下这一帧会显示 idle。
#[test]
fn editing_mode_still_reports_the_working_phase() {
    let mut state = AppState::new("test-model");
    state.run_state.mode = RunMode::Editing;
    state.run_state.phase = "working".to_string();

    let frame = frame_text(120, 30, &state);
    assert!(
        frame.contains("working"),
        "mode=Editing 且 phase=working 时帧里没有 \"working\"（顶部谎报 idle 了）：\n{frame}"
    );
}
