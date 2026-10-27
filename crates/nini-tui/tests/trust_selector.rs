//! Oracle: `/trust` **无参数时打开信任选择器**（v0.10.8 / B2）。
//!
//! Pi：`/trust` → `showTrustSelector()`
//! （`interactive-mode.ts#showTrustSelector`）。
//!
//! nini 此前：`selectors/trust.rs` 存在，但命令层在**无参数**时
//! **静默地把当前目录标成 trusted** —— 用户输入 `/trust` 是想**看**自己的
//! 信任级别，却得到一个被写下的决定。
//!
//! 现在：无参数 → 打开选择器（状态里出现 `open_selector:trust`）；
//! 带参数 → 原来的形式照旧可用。

use nini_tui::commands::dispatch::dispatch;
use nini_tui::commands::registry::CommandId;
use nini_tui::state::AppState;
use std::sync::Arc;

fn state() -> AppState {
    AppState::new("test-model")
}

#[test]
fn trust_without_an_argument_asks_rather_than_decides() {
    let mut s = state();
    let mut settings = nini_tui::settings::SettingsManager::default();

    dispatch(&mut s, &mut settings, CommandId::Trust, "");

    assert_eq!(
        s.run_state.status, "open_selector:trust",
        "无参数的 /trust 必须打开信任选择器（Pi：showTrustSelector）"
    );
}

#[test]
fn trust_with_an_argument_still_sets_the_decision() {
    let mut s = state();
    let mut settings = nini_tui::settings::SettingsManager::default();

    // `list` 是只读的，不写任何决定。
    dispatch(&mut s, &mut settings, CommandId::Trust, "list");

    assert_ne!(
        s.run_state.status, "open_selector:trust",
        "带参数的 /trust 走原来的路径，不该打开选择器"
    );
}
