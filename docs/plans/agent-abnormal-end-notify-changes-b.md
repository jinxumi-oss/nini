# Changes
- `crates/nini-tui/tests/agent_abnormal_notify.rs:69` | 新增回归测试文件（计划步骤 3）：锁住「agent 任务异常结束（未产生正常 Done）时 watchdog 推显式终止通知、既有状态复位保持不变」；纯 helper 存在且为纯函数、三种异常分支（无 Done/panic/超时/无 tokio runtime）都经 push_notices 推通知且 push 在 tracing::warn 之后、状态复位仍先于 push | 计划步骤 3 | 类型: 变更
- `crates/nini-tui/tests/agent_abnormal_notify.rs:85-167` | 新增 5 个断言：helper 存在且纯（不触碰状态、不推送、文案逐字对应三种原因）、三分支接线与 warn 顺序、无 runtime 分支 Timeout 文案、既有状态复位保留（is_compacting→mode→Editing 先于 push） | 计划步骤 3 | 类型: 变更

## Commands
- `cargo test -p nini-tui agent_abnormal_notify` → 退出码 0（5 passed，0 failed；红→绿） | 计划步骤 3
- `cargo test -p nini-tui --test esc_concurrency_probe a_driver_that_never_says_done_still_reopens_the_editor` → 退出码 124（既有测试在本环境挂起，非本步骤改动引入） | 计划步骤 4 尝试
- `cargo test -p nini-tui`（相关 crate 全量）→ 未完成：workspace 内既有测试挂起，无法跑完 | 计划步骤 4
- `bash scripts/gate.sh` → 未完成：其 `cargo test --workspace` 步骤同样会撞上上述挂起测试 | 计划步骤 4

## Reproduction
- 复现命令: `cargo test -p nini-tui agent_abnormal_notify` → 退出码 0（新增回归测试转绿）

## Blocked
- 既有测试 `crates/nini-tui/tests/esc_concurrency_probe.rs` 的 `a_driver_that_never_says_done_still_reopens_the_editor` 在本环境挂起：隔离运行 `cargo test -p nini-tui --test esc_concurrency_probe a_driver_that_never_says_done_still_reopens_the_editor` 在 900s 内被 timeout 杀掉（rc=124，仅打印到「has been running for over 60 seconds」），它同时是 `cargo test -p nini-tui` 与 `bash scripts/gate.sh`（workspace 测试）的必经步骤，故全量验收无法完成。本步骤只新增测试文件、未改 src/ 与既有测试，与挂起无关，属环境/既有状态问题。
