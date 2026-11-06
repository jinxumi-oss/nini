# Agent 异常结束（无正常 Done）时给 TUI 显式终止通知 —— 让「断开后冻结、无回复」可观测

Status: **待执行（2026-11-07）**：现象为「agent 执行到下一步后自动断开，之后完全静止：无回复、不往下一步走」。本计划针对其代码侧可证实的根因处理：agent 任务异常结束（未走到正常 Done）时，TUI 没有向 transcript 推任何用户可见事件，只有 watchdog 写 tracing 警告，用户看到的是「断开 + 静默冻结」。

## 现状取证（2026-11-07，文件:行）

- `crates/nini-tui/src/runtime.rs` `run_loop` 的 `select!` 靠 `done_for_select`（`done.notify_waiters()`）唤醒；正常结束会推 `AgentEventLite::Done`。
- agent 任务由 `crates/nini-cli/src/app.rs` 的 `AgentDriver`（app.rs:656 附近）用 `tokio::spawn` 起，循环里正常结束会推 `Done` 并 `done.notify_waiters()`。
- 但任务**异常结束**（运行中 panic / 超时 / 未走完 Done）时，只有 watchdog 能捕获：`runtime.rs` watchdog 在 `Ok(Err(join_err))` / `Err(timeout)` 等分支**只写 `tracing::warn!`**（约 `runtime.rs:3703`），**不向 transcript 推任何用户可见事件**。
- 于是 run_loop 不再产生新事件，agent 看起来「断开」且完全静止，用户看不到任何原因。

## 目标

当 agent 任务异常结束（未产生正常 Done）时，TUI 向 transcript 推**一条显式终止通知**（明确告知「agent 已停止/出错」），并保留既有的状态复位（watchdog 已做的 `agent_alive=false` / `is_compacting=false` / 队列重投 / `mode→Editing`）。

## 步骤

1. [信息] 现状取证（见上，文件:行）。
2. [变更] `crates/nini-tui/src/runtime.rs` watchdog 的异常分支：在 `tracing::warn!` 处补向 transcript 推显式终止通知的代码；抽出纯 helper（纯函数，便于测试）。
3. [变更] 新增回归测试文件，锁住「agent 任务异常结束会推显式终止通知」这一行为（只新增测试文件，不改既有测试、不改既有实现）。
4. [验收] 新增测试转绿；相关 crate 测试不破；`scripts/gate.sh` 通过。

## 复现 / 验收命令

- 新增测试：`cargo test -p nini-tui <测试名>` → 退出码 0（红→绿）
- 相关 crate 全量：`cargo test -p nini-tui`
- 全 workspace：`cargo test --workspace`
- 门禁：`bash scripts/gate.sh`

## Blocked
none
