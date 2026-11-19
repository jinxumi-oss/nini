# Changes
- `crates/nini-core/src/agent.rs:1971-1973` | 步骤 2.1：把 `let mut continue_after_truncation = false;`（及其注释「v0.17（72h 不间断）：截断后丢掉本轮、重新请求同一轮。」）上移到顶部 follow-up 轮询之前、紧邻 `let mut follow_up_rounds = 0usize;`；保持 turn 循环体内 `retry_after_compaction` 位置不变 | 计划步骤 2.1 | 类型: 变更
- `crates/nini-core/src/agent.rs:1981-1982` | 步骤 2.2：顶部 follow-up 轮询条件放行截断续跑——新增 `let poll_after_truncation = continue_after_truncation;`（1981），条件改为 `if (!has_more_tool_calls && pending.is_empty()) || poll_after_truncation {`（1982） | 计划步骤 2.2 | 类型: 变更
- `crates/nini-core/src/agent.rs:2002-2018` | 步骤 2.2（修法后半）：轮询返回空时先 `pending.drain(..);`（2002）；非续跑走原有 `set_phase(Idle); yield PhaseChanged(Idle); return;`，续跑置 `Working` 走本轮既有收尾（2012-2018），不提前结束运行 | 计划步骤 2.2 | 类型: 变更
- `crates/nini-core/src/agent.rs:3355-3358` | 步骤 2.3：在 turn 循环末尾（`get_steering_messages` 轮询之后、循环闭合 `}` 之前）把 `continue_after_truncation = false;` 置回（3358，附注释说明标记只在本次截断续跑轮放行，置回后后续轮走常规路径） | 计划步骤 2.3 | 类型: 变更
- `crates/nini-core/src/agent.rs:2439` | 步骤 2.4（范围校验）：截断置位点 `continue_after_truncation = true;`（2439，截断分支 `break 'outer` 前）保持不变；未新增依赖、未改既有测试、未触碰 `runtime.rs`/`selection.rs`/`release/*`/CHANGELOG 等其它文件 | 计划步骤 2.4 | 类型: 信息
- `crates/nini-core/src/agent.rs:1982` | 验收说明：本条是潜在结构修复（抑制的是 follow-up 轮询的调用时机，follow-up 投递本身由既有 follow-up 测试覆盖），现有测试口径下不产生可观测行为差异；复现命令 `cargo test -p nini-core --test truncation_continues` 保持 6 passed 不回归 | 计划步骤 3 | 类型: 信息

## Commands
- `cargo fmt --all -- --check` → 退出码 0
- `cargo clippy --workspace --all-targets -- -A clippy::pedantic -A clippy::nursery -A clippy::perf -A clippy::complexity -D warnings` → 退出码 0
- `cargo test -p nini-core` → 退出码 0（273 个 lib 测试 + 全部集成测试全绿，含 `truncation_continues` 6 条、`steering_followup_hook_tests`、`overflow_recovery`、`compaction_lifecycle`）
- `cargo test --workspace --all-targets` → 退出码 0

## Reproduction
- 复现命令: `cargo test -p nini-core --test truncation_continues` → 退出码 0（红→绿：本条是潜在结构修复，不产生红测试；抑制的是 follow-up 轮询的调用时机，follow-up 投递本身由既有 follow-up 测试覆盖，故无红测试；验收以既有套件不回归为准）

## Blocked
none

---

## 评审与收口

- 评审：`docs/plans/long-task-72h-continuity-fix-review.md` → `VERDICT: CLEAN`（3 条 MINOR）。
- 落地评审意见（3 条 MINOR，均为清理，无行为变化）：
  - `agent.rs`：删除 worker 夹带的 4 处计划无关空行（约 2196、2374、2914/2917），diff 只含步骤 2 的变更。
  - `agent.rs:2433`：注释与新的守卫口径一致——`has_more_tool_calls` 是顶部 follow-up 轮询的「继续」条件之一，删除「唯一的」表述。
  - `agent.rs:2007`：注释里的行号修正为 `retry_after_compaction || continue_after_truncation` 的实际位置（agent.rs:2819）。
- 门禁收口：`bash scripts/gate.sh` → 通过 12 / 失败 0 / 跳过 4。
- 链③声明收口：worker 的修复让既有 fix 提交 `20ef77d`（复制/粘贴反馈测试确定可控）从 NONE（裸奔不变量 3 条）回到基线 2 条；按门禁规则「补断言」，在 `runtime.rs` 的 `mouse_selection_tests` 模块加 `invariant: 20ef77d…` 声明（该模块的复制/粘贴反馈断言即守它），并在 `docs/invariant-verification.md` 补了注入-观察-还原记录（注入「复制失败分支不再回写 `status = "copy failed"`」→ `a_copy_that_cannot_be_verified_says_so` 与 `a_failed_copy_leaves_a_line_in_the_transcript` 变红，还原后其余 426 条仍绿）。
- 长任务计划状态：`docs/plans/long-task-72h-continuity-plan.md` 由「待执行」改为「已完成」。
