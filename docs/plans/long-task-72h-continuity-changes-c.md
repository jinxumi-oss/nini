# Changes
- `crates/nini-core/src/agent.rs:2066` | 补处 1：在 `length_truncated` 分支 `yield AgentEvent::TurnEnd` 之后、push 继续提示之前，加一段压缩（`begin_auto_compaction` → Compacting 相位 + `PhaseChanged` → `compact_history_via_hook` → `Compacted` → `end_auto_compaction` → Working 相位 + `PhaseChanged`），形状照抄 `recover_from_overflow` 已有顺序，但**不调用**它（不把半截输出从上下文 pop 掉），保住截断续跑既有行为 | 计划步骤 补处 1 | 类型: 变更
- `crates/nini-core/src/agent.rs:2118` | 补处 1（保住既有不变量）：在 `consecutive_truncations = 0` 之后、`let recoverable = overflowed` 之前补回 `last_usage = usage;`；此前只把该行留在截断分支，普通一轮/上下文溢出轮不再更新用量快照，导致 `estimated_tokens()` 失真、阈值压缩失效（conformance `overflow_recovery::a_run_compacts_at_threshold_without_waiting_for_an_error` 变红） | 计划步骤 补处 1 | 类型: 变更
- `crates/nini-core/src/agent.rs:815` | 补处 2a：`RunConfig` 在 `max_iterations` 附近新增字段 `runaway_turn_backstop: Option<usize>`（`None` = 用默认值，生产默认不变） | 计划步骤 补处 2 | 类型: 变更
- `crates/nini-core/src/agent.rs:877` | 补处 2b：`RunConfig::new` 初始化 `runaway_turn_backstop: None` | 计划步骤 补处 2 | 类型: 变更
- `crates/nini-core/src/agent.rs:1765` | 补处 2c：循环里 `turn_cap` 改为 `config.runaway_turn_backstop.unwrap_or(RUNAWAY_TURN_BACKSTOP)` | 计划步骤 补处 2 | 类型: 变更
- `crates/nini-core/src/agent.rs:1772` | 补处 2d：`AgentError::RunawayGuard { turns }` 的 `turns` 改用 `turn_cap`（原写死 `RUNAWAY_TURN_BACKSTOP`） | 计划步骤 补处 2 | 类型: 变更
- `crates/nini-core/src/agent.rs:4508` | 补处 2e（inline tests）：`agent_with_tool` helper 注入 `runaway_turn_backstop: Some(5)`，把 5 个跑满兜底的 inline 测试（`no_hooks_runs_v061_behavior` 等，provider 每轮重复发同一工具调用、永不结束）从 20_000 轮降到 5 轮；断言不变 | 计划步骤 补处 2 | 类型: 变更
- `crates/nini-core/src/agent.rs:4213` | 补处 2e（inline tests）：两个 `ToolCallProvider` 版 steering 测试（`steering_hook_injects_after_tool_execution`、`panicking_steering_hook_does_not_crash_run`）注入 `runaway_turn_backstop: Some(5)`，避免跑到 20_000 兜底拖垮 nini-core 套件；断言不变 | 计划步骤 补处 2 | 类型: 变更
- `crates/nini-conformance/tests/long_task_delivery.rs:268` | 补处 2e：`an_uncapped_run_that_never_settles_stops_at_the_backstop` 注入 `runaway_turn_backstop: Some(5)`，断言由 `RUNAWAY_TURN_BACKSTOP + 1` 改为「注入值 + 1」；断言语义不变 | 计划步骤 补处 2 | 类型: 变更
- `crates/nini-conformance/tests/tool_batch_semantics.rs:158` | 补处 2e（conformance 会跑到兜底否则套件变几十分钟）：`run_batch` 与 `calls_with` 注入 `runaway_turn_backstop: Some(5)`（`BatchProvider`/`CountingProvider` 永不结束、会跑到兜底），断言不变 | 计划步骤 补处 2 | 类型: 变更
- `crates/nini-core/tests/truncation_continues.rs` | 信息：本轮未改该文件内容；已 `git add` 跟踪，满足 workspace_modules 的「新源文件须 git 跟踪」不变量（runaway 72h 护栏断言原样保留） | 计划步骤 补处 2e | 类型: 信息

## Commands
- `cargo fmt -p nini-core` → 退出码 0
- `cargo clippy -p nini-core --all-targets -- -D warnings` → 退出码 0
- `cargo test -p nini-core --test a3b_overflow --test compaction_lifecycle --test truncation_continues` → 退出码 0（补处 1：既有两条「长度截断会走压缩」的测试转绿，truncation_continues 6 passed）
- `cargo test -p nini-core --test truncation_continues` → 退出码 0（6 passed，含 `the_runaway_backstop_covers_a_72_hour_run`）
- `cargo test -p nini-core --lib` → 退出码 0（255 passed；含补处 2e 注入后不再跑满 20k 兜底，总耗时 3.51s）
- `cargo test -p nini-core` → 退出码 0 除 workspace_modules 一项（见 Blocked），总耗时 8.6s（≲5 分钟）
- `cargo test -p nini-conformance` → 退出码 0（全绿，总耗时约 2s；含 long_task_delivery 与 tool_batch_semantics）

## Reproduction
- 复现命令: `cargo test -p nini-core --test truncation_continues` → 退出码 0（红→绿：旧实现下截断即中断，转绿后 6 passed 全绿）

## Blocked
- `cargo test -p nini-core --test workspace_modules` 的 `every_source_file_is_tracked_by_git` 仍失败：仅剩 `crates/nini-tui/src/selection.rs` 未 git 跟踪。该文件是**另一个并发 agent 正在改的 nini-tui 文件**（gate.md 已记录），属 `crates/nini-tui/**`（任务严禁触碰），本 worker 无法也不应改；本 worker 自己的新测试文件 `crates/nini-core/tests/truncation_continues.rs` 已 `git add` 跟踪，其余 .rs 均被 git 跟踪。此项为环境/并发残留，非本次补处引入，需 planner 在并发 agent 收尾后确认。
