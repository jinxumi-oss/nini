# nini 72 小时不间断工作 —— 交付门禁检查

任务: `docs/plans/` 之外的当前计划见 `plan.md`（nini 72 小时不间断工作：修掉「截断即中断」等 4 处任务断点）

## BLOCKER 检查清单

### 1. 计划完整性 ✅
- `plan.md` 存在：目标 / 现象取证（代码级行号）/ 4 处断点清单 / 3 项改动（精确到代码形状）/ 测试 / 验收 / Out of scope / 分工。
- 需求可追溯：每项改动都能指回「需求原文」或「截图现象」。

### 2. 代码定位 ✅
- D1：`crates/nini-core/src/agent.rs:1987`（`truncated_answer`）、`:2318`（`TurnEnd`）、`:2351`（`has_more_tool_calls`）、`:1655-1690`（循环顶部守卫）。
- D2：`crates/nini-core/src/agent.rs:1225-1253`（`recover_from_overflow` 的 `overflow_recovery_attempted`）。
- D3：`crates/nini-tui/src/runtime.rs:624`（`WATCHDOG_TIMEOUT_SECS`）、`:3831`（`timeout`）、`:3877+`（超时分支）。
- D4：`crates/nini-core/src/agent.rs:48`（`RUNAWAY_TURN_BACKSTOP`）、`:1722`（兜底判定）。
- 证据：用户截图 + `~/.nini/agent/sessions/*/*.jsonl` 的 assistant 消息结构抽样
  （`text` + `toolCall` 同一块）→ 截图里没有工具调用行 → 24 行是**一次**响应里的复读。

### 3. 证据充分性 ✅
- 截断结束运行：`is_recoverable_length` 要求 `output_tokens < desired_max`
  （`crates/nini-core/src/overflow.rs:412`），模型烧光输出预算时为 false → 不进恢复分支 → 运行结束。
- 72h 口径下必然触发的兜底：watchdog 600s（任何一轮 > 10 分钟）、runaway 1000 轮。

### 4. 风险控制 ✅
- 不改压缩算法、不改 `max_iterations` 语义、不改 CHANGELOG/README/依赖。
- 截断续跑有上限（`MAX_CONSECUTIVE_TRUNCATIONS = 5`），不会变成新的死循环。
- watchdog 改动只影响「超时」分支：`NoDone` / `Panicked` 两个真正的异常结束分支不变。
- 与 Pi 的差异（截断后不压缩、连续多次继续）在 `plan.md` §3 改动 1 里写明理由。

### 5. 并发写入风险 ⚠️（已识别，需 worker 注意）
仓库里另有一个 nini 会话正在改 `crates/nini-tui/src/selection.rs` 与
`crates/nini-tui/src/lib.rs`（2026-11-16 11:15-11:16 落盘）。
worker 只能用**精确文本替换**改自己负责的区域；文本对不上就**停手回报**，
不得覆盖别人的改动，不得顺手整理无关文件。

## VERDICT: APPROVED

所有 BLOCKER 已清除，可以进入执行阶段。worker 按 `plan.md` §7 的分工执行，
产物写到 `docs/plans/long-task-72h-continuity-changes-a.md` / `-b.md`。
