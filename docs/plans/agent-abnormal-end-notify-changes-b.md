# Changes
- `crates/nini-tui/tests/agent_abnormal_notify.rs:69` | 新增回归测试文件（计划步骤 3）：锁住「agent 任务异常结束（未产生正常 Done）时 watchdog 推显式终止通知、既有状态复位保持不变」；纯 helper 存在且为纯函数、三种异常分支（无 Done/panic/超时/无 tokio runtime）都经 push_notices 推通知且 push 在 tracing::warn 之后、状态复位仍先于 push | 计划步骤 3 | 类型: 变更
- `crates/nini-tui/tests/agent_abnormal_notify.rs:85-167` | 新增 5 个断言：helper 存在且纯（不触碰状态、不推送、文案逐字对应三种原因）、三分支接线与 warn 顺序、无 runtime 分支 Timeout 文案、既有状态复位保留（is_compacting→mode→Editing 先于 push） | 计划步骤 3 | 类型: 变更

## Commands
- `cargo test -p nini-tui agent_abnormal_notify` → 退出码 0（5 passed，0 failed；红→绿） | 计划步骤 3
- `cargo test -p nini-tui --test esc_concurrency_probe` → 退出码 0（7 passed，0 failed，其中死锁曾致 `a_driver_that_never_says_done_still_reopens_the_editor` 永久挂起，修复后 1.16s 转绿） | 计划步骤 4
- `cargo test -p nini-tui`（相关 crate 全量）→ 退出码 0（108 个 `test result` 全 ok，0 失败） | 计划步骤 4
- `bash scripts/gate.sh` → 退出码 0（12 通过 / 0 失败 / 4 跳过，含 `cargo test --workspace` 全量、格式化/lint、对外声明 T4 精确相等） | 计划步骤 4

## Reproduction
- 复现命令: `cargo test -p nini-tui agent_abnormal_notify` → 退出码 0（新增回归测试转绿）

## Blocked
none —— 步骤 1-4 全部完成：异常结束显式终止通知已接线（三处异常分支 + 无 runtime 分支），持有锁死锁已修，回归测试 5 条转绿，全量 crate 测试与工作区门禁均通过。
