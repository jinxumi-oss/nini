# Changes
- `crates/nini-tui/src/runtime.rs:131` | 新增纯 helper `WatchdogStopReason` 枚举 + `watchdog_stop_notice` 纯函数（只接收异常类型+原因、返回终止通知文案，不碰状态、无副作用） | 计划步骤 2 | 类型: 变更
- `crates/nini-tui/src/runtime.rs:3728-3756` | watchdog 异常分支（无正常 Done / `Ok(Err(join_err))` panic / `Err(timeout)`）在 `tracing::warn!` 之后补 `push_notices` 推一条显式终止通知；既有状态复位（`agent_alive=false`、`is_compacting=false`、队列重投、`mode→Editing`）保持不变 | 计划步骤 2 | 类型: 变更
- `crates/nini-tui/src/runtime.rs:3760-3777` | 无 tokio runtime 的 watchdog 分支（同为异常结束）同样补 `push_notices` 显式终止通知，覆盖计划「等 tracing::warn! 处」 | 计划步骤 2 | 类型: 变更

## Commands
- `cargo check -p nini-tui` → 退出码 0（编译通过，无新增错误）
- `cargo test -p nini-tui --no-run` → 退出码 0（crate 全部测试目标编译通过）
- `cargo test -p nini-tui --test notice_batch` → 退出码 0（相关 runtime 相关测试通过，7 passed）
- `cargo fmt --all --check` → 退出码 0（格式对齐 rustfmt）
- `cargo test -p nini-tui`（全量 crate 测试）→ 未在 1800s 内结束（本环境 e2e 测试偏慢，非代码红；全量/门禁属计划步骤 4 验收范围，本步骤不做）

## Reproduction
- 复现命令: `cargo check -p nini-tui` → 退出码 0（变更可编译，无新增红）
- 复现命令: `cargo test -p nini-tui --test notice_batch` → 退出码 0（相关测试通过）

## Blocked
none
