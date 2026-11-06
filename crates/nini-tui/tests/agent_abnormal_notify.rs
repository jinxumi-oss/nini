//! agent 异常结束（未产生正常 Done）时推显式终止通知 —— 回归测试（计划步骤 3）。
//!
//! 现象：agent 任务异常结束（运行中 panic / 超时 / 未走完 Done）时，watchdog
//! 只写 tracing 警告，不向 transcript 推任何用户可见事件，用户看到「断开 +
//! 静默冻结」。步骤 2 修复：异常分支补推一条显式终止通知，并抽出纯 helper
//! `watchdog_stop_notice`。
//!
//! # 为什么是源码级断言
//!
//! watchdog 的终止通知 push 位于 [`runtime::run`] 内部，真实触发需要 TTY 与
//! agent 驱动，集成测试无法触达真实 push（仓库既有约定见
//! `tests/d1_startup_wiring.rs`：`run()` 是 behind-TTY 的，因此把「接线 +
//! 文案」锁在源码层）。本文件据此锁三件事：
//!
//! 1. 纯 helper 存在，且是纯函数（不碰状态、不推送），文案与三种异常原因
//!    逐字一致；
//! 2. watchdog 的三个异常分支（无正常 Done / panic / 超时，以及无 tokio
//!    runtime 分支）都推显式终止通知，且 push 发生在 tracing::warn 之后；
//! 3. 既有状态复位（`agent_alive=false` / `is_compacting=false` /
//!    `mode→Editing`）仍保留——即本修复只加通知、不动复位。

use std::cmp::min;

const RUNTIME_SRC: &str = include_str!("../src/runtime.rs");

/// 三种异常原因 → 纯 helper 文案的逐字预期（helper 是纯函数，文案即其输出）。
const NOTICE_NO_DONE: &str = "agent run stopped: the agent task ended without a Done event";
const NOTICE_PANICKED: &str = "agent run stopped: the agent task panicked ({msg})";
const NOTICE_TIMEOUT: &str =
    "agent run stopped: the agent task did not finish within {WATCHDOG_TIMEOUT_SECS}s";

/// 三个 tokio runtime 异常分支的起点/终点标记，以及该分支应有的两处调用。
///
/// 整段覆盖一个异常分支：起点标记是分支的形参（`=> {`），终点标记是下一段
/// 的起点，因此整段恰好是那个分支。
type Branch = (&'static str, &'static str, &'static str, &'static str);

fn tokio_branches() -> [Branch; 3] {
    [
        (
            "Ok(Ok(())) => {",
            "Ok(Err(join_err)) => {",
            "watchdog_stop_notice(&stop_reason)",
            "push_notices(&watchdog_shared",
        ),
        (
            "Ok(Err(join_err)) => {",
            "Err(_) => {",
            "watchdog_stop_notice(&stop_reason)",
            "push_notices(&watchdog_shared",
        ),
        (
            "Err(_) => {",
            "/// Esc 在当前状态下该做什么",
            "watchdog_stop_notice(&stop_reason)",
            "push_notices(&watchdog_shared",
        ),
    ]
}

/// 纯 helper 必须存在：`WatchdogStopReason` 枚举 + `watchdog_stop_notice`
/// 纯函数。异常类型要有独立表示，helper 才能按原因生成对应文案。
#[test]
fn agent_abnormal_notify_watchdog_helper_exists() {
    assert!(
        RUNTIME_SRC.contains("enum WatchdogStopReason"),
        "缺失 WatchdogStopReason：异常类型没有独立表示，helper 无从按原因文案"
    );
    assert!(
        RUNTIME_SRC.contains("fn watchdog_stop_notice(reason: &WatchdogStopReason) -> String"),
        "缺失纯 helper watchdog_stop_notice"
    );
}

/// 纯 helper 是纯函数：只接收异常类型 + 原因，返回文案，不碰任何状态、不
/// 产生副作用——「便于测试」的落点就是它不依赖任何运行时状态。
#[test]
fn agent_abnormal_notify_stop_notice_helper_is_pure() {
    let fn_at = RUNTIME_SRC
        .find("fn watchdog_stop_notice")
        .expect("helper 必须存在");
    // helper 体 = 从 fn 到下一个顶层 doc 注释；该区间不得含状态操作。
    let after_fn = RUNTIME_SRC[fn_at..]
        .find("\n/// ")
        .expect("helper 后应有 doc 注释");
    let body = &RUNTIME_SRC[fn_at..fn_at + after_fn];

    assert!(
        !body.contains("push_notices"),
        "helper 不得推送事件：推送是 watchdog 分支的调用，不应落在纯 helper 里"
    );
    assert!(
        !body.contains("lock_state"),
        "helper 不得触碰状态锁：纯函数不允许改状态"
    );
    // 三种异常原因都映射进文案（逐字匹配 helper 的格式串）。
    assert!(body.contains(NOTICE_NO_DONE), "NoDone 文案缺失");
    assert!(body.contains(NOTICE_PANICKED), "Panicked 文案缺失");
    assert!(body.contains(NOTICE_TIMEOUT), "Timeout 文案缺失");
}

/// watchdog 的三个 tokio runtime 异常分支（无正常 Done / panic / 超时）都推
/// 显式终止通知，且 push 发生在 tracing::warn 之后（warn 先记原因、再推通知）。
#[test]
fn agent_abnormal_notify_watchdog_exception_branches_push_an_explicit_stop_notice() {
    for (start_marker, end_marker, expected_a, expected_b) in tokio_branches() {
        let start = RUNTIME_SRC.find(start_marker).expect("找不到分支起点");
        let end = RUNTIME_SRC.find(end_marker).expect("找不到分支终点");
        let region = &RUNTIME_SRC[start..min(end, RUNTIME_SRC.len())];

        assert!(
            region.contains(expected_a),
            "分支 {start_marker:?} 应推 watchdog_stop_notice（异常原因）"
        );
        assert!(
            region.contains(expected_b),
            "分支 {start_marker:?} 应经 push_notices 推送终止通知"
        );

        let warn_at = region
            .find("tracing::warn!(")
            .expect("分支内应有 tracing::warn!");
        let push_at = region.find(expected_b).expect("分支内应有 push_notices");
        assert!(
            push_at > warn_at,
            "分支 {start_marker:?} 的通知必须在 tracing::warn 之后推送"
        );
    }
}

/// 无 tokio runtime 的 watchdog 分支同样属于异常结束，同样要推显式终止
/// 通知（文案用 Timeout）。
#[test]
fn agent_abnormal_notify_watchdog_without_tokio_runtime_also_notifies() {
    let no_rt = RUNTIME_SRC
        .find("// No tokio runtime in this thread")
        .expect("找不到无 tokio runtime 分支");
    let end = RUNTIME_SRC
        .find("/// Esc 在当前状态下该做什么")
        .expect("找不到下一个顶层项");
    let region = &RUNTIME_SRC[no_rt..min(end, RUNTIME_SRC.len())];

    assert!(
        region.contains("watchdog_stop_notice(&WatchdogStopReason::Timeout)"),
        "无 runtime 分支应推 Timeout 文案的显式终止通知"
    );
    assert!(
        region.contains("push_notices("),
        "无 runtime 分支应经 push_notices 推送终止通知"
    );
    assert!(
        region.contains("tracing::warn!("),
        "无 runtime 分支应先写 tracing 警告"
    );
}

/// 既有状态复位必须保留：步骤 3 只加通知、不动复位。复位（is_compacting →
/// mode→Editing）必须发生在推终止通知之前。
#[test]
fn agent_abnormal_notify_state_reset_is_preserved_in_the_same_watchdog() {
    let reset_at = RUNTIME_SRC
        .find("g.run_state.is_compacting = false;")
        .expect("is_compacting 状态复位缺失：既有复位被步骤 3 破坏");
    let mode_at = RUNTIME_SRC
        .find("g.run_state.mode = RunMode::Editing;")
        .expect("mode→Editing 状态复位缺失：既有复位被步骤 3 破坏");
    let push_at = RUNTIME_SRC
        .find("push_notices(&watchdog_shared")
        .expect("watchdog 的 push_notices 缺失");

    assert!(
        reset_at < mode_at,
        "复位顺序与既有实现一致（先 is_compacting）"
    );
    assert!(
        mode_at < push_at,
        "既有状态复位必须发生在推终止通知之前：只加通知、不动复位"
    );
    for token in [
        "g.run_state.agent_alive = false;",
        "g.run_state.is_compacting = false;",
        "g.run_state.mode = RunMode::Editing;",
    ] {
        assert!(RUNTIME_SRC.contains(token), "状态复位缺失：{token}");
    }
}
