//! Oracle: Esc 的语义由 **EscapeTarget** 决定（v0.10.8 / A3）。
//!
//! Pi 不是一个 if-else 链：它在进入压缩/重试/分支摘要之前**替换掉** Esc
//! 处理器（`autoCompactionEscapeHandler` / `retryEscapeHandler`，见
//! `interactive-mode.ts` 的 EscapeTarget），离开那个上下文再换回来。
//!
//! nini 此前是一条 if-else 链，于是「压缩期间的 Esc」没有独立语义——
//! 它碰巧落进 Running 那条，接近，但不是**被设计**的。
//!
//! 判据：
//! 1. 相位 → EscapeTarget 的映射是对的；
//! 2. Idle 上下文的 Esc 清空输入（Pi 的 Idle）；
//! 3. Compacting 上下文的 Esc **不清空输入**，也**不中止这一轮**——
//!    它只取消压缩（Pi 的 `autoCompactionEscapeHandler`）。
//!    **这一条在 v0.11 变了**（差距 A2b 关闭）：此前 nini 把压缩期的 Esc
//!    当成「中止整轮」，而本文件当时就写着「Pi 是只取消压缩、让回合继续；
//!    差别记在差距 A2b」。旧断言把**当时的行为**当成了正确——
//!    按 GOALS 的纪律（对齐后既有测试变红，先判断它是不是把旧行为当成了
//!    正确：是就改测试并记录理由），这里改测试，理由记在下面。
//! 4. 运行时那条与 `apply_action` 那条**行为一致**（两个 dispatch 点
//!    行为不同，正是这个项目反复吃亏的那类失真）。

use nini_tui::runtime::{apply_action, AgentEventLite, AgentSink};
use nini_tui::state::{AppState, EscapeTarget, RunMode};
use std::sync::Arc;

fn state_with_text(text: &str) -> AppState {
    let mut g = AppState::new("test-model");
    for c in text.chars() {
        g.input.insert_char(c);
    }
    g
}

fn sink_for(state: &Arc<std::sync::Mutex<AppState>>) -> AgentSink {
    AgentSink::new(state.clone(), tokio::sync::watch::channel(false).0)
}

#[test]
fn the_phase_decides_the_escape_target() {
    let shared = Arc::new(std::sync::Mutex::new(state_with_text("")));
    let sink = sink_for(&shared);

    for (phase, want) in [
        ("compacting", EscapeTarget::Compacting),
        ("retrying", EscapeTarget::Retrying),
        ("branch_summary", EscapeTarget::BranchSummary),
        ("working", EscapeTarget::Agent),
        ("idle", EscapeTarget::Idle),
    ] {
        sink.push(AgentEventLite::PhaseChanged(phase.to_string()));
        let g = shared.lock().unwrap();
        assert_eq!(
            g.run_state.escape_target, want,
            "相位 {phase} 应当对应 EscapeTarget::{want:?}"
        );
    }
}

#[test]
fn esc_in_the_idle_context_clears_the_input() {
    let mut state = state_with_text("draft text");
    state.run_state.escape_target = EscapeTarget::Idle;
    state.run_state.mode = RunMode::Editing;

    apply_action(&mut state, nini_tui::keys::Key::esc());

    assert_eq!(
        state.input.text, "",
        "Idle 上下文的 Esc 必须清空输入（Pi 的 Idle）"
    );
}

#[test]
fn esc_in_the_compacting_context_does_not_clear_the_input() {
    let mut state = state_with_text("draft text");
    state.run_state.escape_target = EscapeTarget::Compacting;
    state.run_state.mode = RunMode::Running;
    state.input.text = "draft text".to_string();

    apply_action(&mut state, nini_tui::keys::Key::esc());

    assert_eq!(
        state.input.text, "draft text",
        "压缩期间的 Esc 只取消压缩，**不是**清空输入——用户正在写的东西不能消失"
    );
    // v0.11 (A2b)：这一轮**继续**（Pi 只取消压缩），所以运行态不变。
    //
    // 旧断言写的是 `RunMode::Editing`——那是「中止整轮」之后的状态，
    // 而中止整轮正是 A2b 要关掉的错误行为：用户想取消一次压缩，
    // 丢掉的却���整轮工作。
    assert_eq!(
        state.run_state.mode,
        RunMode::Running,
        "取消压缩不等于中止这一轮（Pi：只取消压缩，回合继续）"
    );
}

#[test]
fn the_decision_is_shared_by_both_dispatch_points() {
    // 运行时那条按键处理与 `apply_action` 那条**共用** `escape_action()`
    // ——两个 dispatch 点行为不同，正是这个项目在键位表上已经付过学费的
    // 那类失真。这里断言的是那份**共享判定**的行为：同一个状态，
    // 判定必须一致。
    use nini_tui::runtime::{escape_action, EscapeAction};

    let mut s = state_with_text("draft");
    s.run_state.escape_target = EscapeTarget::Compacting;
    s.run_state.mode = RunMode::Running;
    assert_eq!(
        escape_action(&s),
        EscapeAction::AbortCompaction,
        "压缩上下文的 Esc 必须只「取消压缩」（Pi 的 autoCompactionEscapeHandler）"
    );

    s.run_state.escape_target = EscapeTarget::Idle;
    s.run_state.mode = RunMode::Editing;
    assert_eq!(
        escape_action(&s),
        EscapeAction::ClearInput,
        "Idle 上下文的 Esc 必须是「清空输入」"
    );

    s.ui_state.completion = Some(Default::default());
    assert_eq!(
        escape_action(&s),
        EscapeAction::CloseCompletion,
        "补全弹窗优先于其它上下文"
    );
}
