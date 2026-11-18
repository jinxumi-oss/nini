# Changes
- `crates/nini-tui/src/runtime.rs:131` | 删掉 `WatchdogStopReason` 枚举的 `Timeout` 变体及其在 `watchdog_stop_notice` 中对应 Timeout 分支（超时不再是停止原因，只记日志并继续等同一任务）；`NoDone` / `Panicked` 分支不变 | 计划步骤 1a | 类型: 变更
- `crates/nini-tui/src/runtime.rs:622` | `WATCHDOG_TIMEOUT_SECS = 600` 保留（仍是「多久看一眼」的心跳），注释补充「超时不再是任务已死的判据，只是心跳」 | 计划步骤 1d | 类型: 变更
- `crates/nini-tui/src/runtime.rs:3834-3849` | watchdog 超时等待改为重新武装：`let mut handle = handle;` + `loop` 用 `&mut handle` 重新 `tokio::time::timeout(Duration::from_secs(WATCHDOG_TIMEOUT_SECS))`，超时只 `tracing::warn!`（写明长任务常态）后继续 await 同一任务；任务真正结束（`Ok(Ok(()))` / `Ok(Err(panic))`）才走既有状态复位 + 终止通知 | 计划步骤 1b | 类型: 变更
- `crates/nini-tui/src/runtime.rs:3886-3901` | 终止原因只保留 `NoDone` / `Panicked`；超时不再作为终止原因（对应 `Err(_)` 分支标为不可达，注释说明超时已由重新武装循环处理） | 计划步骤 1b | 类型: 变更
- `crates/nini-tui/src/runtime.rs:3940` | 无 tokio runtime 分支的终止通知文案由 `WatchdogStopReason::Timeout` 改为 `WatchdogStopReason::NoDone`（语义：没有 Done 事件） | 计划步骤 1c | 类型: 变更
- `crates/nini-tui/tests/agent_abnormal_notify.rs:40` | `tokio_branches()` 由 3 条改为 2 条（超时分支不再推终止通知） | 计划步骤 0 | 类型: 变更
- `crates/nini-tui/tests/agent_abnormal_notify.rs:131` | 新增回归测试 `agent_abnormal_notify_timeout_branch_only_warns_and_rearms_the_same_task`：断言超时分支只 `tracing::warn!` + 用 `&mut handle` 重新武装同一任务，不推 `watchdog_stop_notice`、不推 `push_notices`、不再复位 `agent_alive` / `mode` | 计划步骤 0 | 类型: 变更
- `crates/nini-tui/tests/agent_abnormal_notify.rs:69` | 纯 helper 存在性测试增加断言：`WatchdogStopReason::Timeout` 变体应被删除 | 计划步骤 0 | 类型: 变更
- `crates/nini-tui/tests/agent_abnormal_notify.rs:96-100` | helper 纯函数测试去掉 Timeout 文案断言，只保留 NoDone / Panicked 两种文案 | 计划步骤 0 | 类型: 变更
- `crates/nini-tui/tests/agent_abnormal_notify.rs:190` | 无 tokio runtime 分支测试断言由 Timeout 文案改为 NoDone 文案 | 计划步骤 0 | 类型: 变更

## Commands
- `cargo test -p nini-tui --test agent_abnormal_notify` → 退出码 101（lib 编译失败：`crates/nini-tui/src/state.rs:1961` `impl Clone for UiState` 缺 `selection`/`selection_granularity`/`selection_dragged` 字段）
- `cargo check -p nini-tui --lib` → 退出码 101（仅有 state.rs 的 `E0063` 编译错误，其余改动未触发编译）

## Reproduction
- 复现命令: `cargo test -p nini-tui --test agent_abnormal_notify` → 退出码 101（红→绿：未转绿，被 state.rs 未提交改动阻断，非本任务改动所致）

## Blocked
- **编译阻断（超出本任务范围，由他人未提交改动引起）**：`crates/nini-tui/src/state.rs` 的 `impl Clone for UiState`（约 1961）未补上 `selection`/`selection_granularity`/`selection_dragged` 字段（该文件同时含 `screen_text_rows` 与 UiState 选择字段的新增，属另一个人正在改的未提交成果）。导致 `cargo check -p nini-tui --lib` 报 `missing fields ... in initializer of UiState`，`cargo test -p nini-tui --test agent_abnormal_notify` 无法编译，因此步骤 2/3 的转绿与验收无法达成。本任务只允许改 `runtime.rs` 与 `agent_abnormal_notify.rs`，未触碰 state.rs。
