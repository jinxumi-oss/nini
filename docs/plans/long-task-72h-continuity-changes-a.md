# Changes
- `crates/nini-core/src/agent.rs:49` | `RUNAWAY_TURN_BACKSTOP` 1000 → 20_000，注释写 72 小时推导（约 13 秒/轮，72h≈20k 轮） | 计划步骤 7 | 类型: 变更
- `crates/nini-core/src/agent.rs:57` | 新增常量 `MAX_CONSECUTIVE_TRUNCATIONS = 5`（连续长度截断容忍上限，超限才结束运行并汇报） | 计划步骤 1 | 类型: 变更
- `crates/nini-core/src/agent.rs:63` | 新增常量 `TRUNCATION_CONTINUATION_NOTICE`（长度截断后塞回上下文、与 [CONTEXT SUMMARY] 同约定的继续提示文案） | 计划步骤 1 | 类型: 变更
- `crates/nini-core/src/agent.rs:244` | 新增错误变体 `AgentError::RepeatedTruncation { attempts: usize }`（紧邻 `RunawayGuard`，文案照 plan §3 改动 1） | 计划步骤 6 | 类型: 变更
- `crates/nini-core/src/agent.rs:936` | `Agent` struct 加字段 `consecutive_truncations: usize`（紧邻 `overflow_recovery_attempted`） | 计划步骤 2 | 类型: 变更
- `crates/nini-core/src/agent.rs:1002` | `Agent::with_hooks`（`Agent::new` 经此构造）构造函数初始化 `consecutive_truncations = 0` | 计划步骤 2 | 类型: 变更
- `crates/nini-core/src/agent.rs:1880` | 循环新增标志 `continue_after_truncation`（与 `retry_after_compaction` 并列声明） | 计划步骤 4 | 类型: 变更
- `crates/nini-core/src/agent.rs:2043` | `MessageStop` 分支：删 `truncated_answer`、改成 `recoverable = overflowed`（溢出路径不动），新增 `length_truncated` 分支（计数 + 超限 `RepeatedTruncation` + `TurnEnd` + 继续提示 + 置 `has_more_tool_calls` + `continue_after_truncation` + `break 'outer`），非截断轮清零计数 | 计划步骤 3 | 类型: 变更
- `crates/nini-core/src/agent.rs:2349` | 循环收尾跳过判断改为 `if retry_after_compaction || continue_after_truncation { continue; }`（丢掉截断本轮、继续同一轮） | 计划步骤 4 | 类型: 变更
- `crates/nini-core/src/overflow.rs:401` | 删除 `is_recoverable_length` 函数（改动后无调用方，留会成 sweep 未接线候选） | 计划步骤 5 | 类型: 变更
- `crates/nini-core/tests/truncation_continues.rs` | 新建测试：T1（5 条，截断续跑/半截不进重试上下文/继续提示进上下文/重复截断结束运行/非截断轮清零）+ T3（1 条，`RUNAWAY_TURN_BACKSTOP >= 20_000` 72h 护栏），用 `nini_ai::fixture::ProgrammedProvider` 构造「总是截断」fixture | 计划步骤 0（写）/步骤 8（验收绿） | 类型: 信息

## Commands
- `cargo fmt -p nini-core` → 退出码 0
- `cargo clippy -p nini-core --all-targets -- -D warnings` → 退出码 0
- `cargo test -p nini-core --test truncation_continues`（步骤 0 复现）→ 退出码 101（红：BUG 仍在，首次截断后运行即结束）
- `cargo test -p nini-core --test truncation_continues`（步骤 8 转绿）→ 退出码 0（6 passed）
- `cargo test -p nini-core --lib agent::` → 退出码 0（50 passed，含 tool_lifecycle_hook_tests / steering_followup_hook_tests 两个跑满 20k 轮的慢模块，约 2679s）
- `cargo test -p nini-core --lib`（243 个非慢测试）→ 退出码 0（3.5s）
- `cargo test -p nini-core --tests`（集成测试）→ 除下述 3 个既有测试失败外其余全绿

## Reproduction
- 复现命令: `cargo test -p nini-core --test truncation_continues` → 退出码 101（步骤 0，红→绿：BUG 还在）→ 步骤 8 后退出码 0（全绿）

## Blocked
- 计划步骤 9 验收「`cargo test -p nini-core` 全绿」无法达成：计划步骤 3（改动 1）按设计移除「可恢复长度截断 → `recover_from_overflow` 压缩」路径（`agent.rs:2024-2076` 新增 `length_truncated` 分支），但既有测试 `crates/nini-core/tests/compaction_lifecycle.rs:90`（`an_overflow_compaction_has_a_phase_and_ends_it`）与 `crates/nini-core/tests/a3b_overflow.rs:190`（`without_a_hook_the_agent_falls_back_to_compacting_itself`）仍断言该路径会触发压缩钩子，现已失败（`cargo test -p nini-core --test compaction_lifecycle` / `--test a3b_overflow` 各 1 条失败）。任务约束「只允许改这 3 个文件 + 严禁编辑既有测试来让它通过」下，无法在不违反约束的情况下让全绿成立；需 planner 决定补计划（更新这两处既有测试，或调整改动 1 设计）后重跑。
- `cargo test -p nini-core --test workspace_modules` 中 `every_source_file_is_tracked_by_git` 失败：新测试文件 `crates/nini-core/tests/truncation_continues.rs`（未 git 跟踪）+ worker B 未跟踪的 `crates/nini-tui/src/selection.rs` 被点名；属新文件/并发改动的既有类失败（gate 时间 selection.rs 已存在未跟踪），非本次改动引入。
