# 长任务 72h 不间断修复 —— 收掉 reviewer 的 3 条 MINOR（本轮：第 1 条）

- 需求原文（长任务，见 `docs/plans/long-task-72h-continuity-plan.md` §0）：
  「修复这个bug,多agent执行，我需要nini能72小时不间断工作……不产生这种使任务间断的bug。」
- 触发来源：该长任务交付后 `docs/plans/long-task-72h-continuity-review.md` 的评审结论 ——
  3 条 MINOR，其中 2 条（`agent.rs` 截断分支的压缩事件顺序、`runtime.rs` 的版本标注）**已落地**；
  **第 1 条未落地**（`agent.rs` 截断续跑分支无条件 `has_more_tool_calls = true`）。
- 本计划范围：只落地第 1 条 MINOR（`crates/nini-core/src/agent.rs`），不碰其他 2 条（已落地）、
  不碰其他长任务改动、不动其他会话的在途改动（`runtime.rs` 的 clipboard 段、`selection.rs` 等）。
- 门禁：`docs/plans/long-task-72h-continuity-gate.md` 已有 `VERDICT: APPROVED`（本计划不新增门禁，
  验收复用同一门禁 + 本计划的步骤 2）。

## 步骤 1 [信息] 确认待修项与已修项

- 待修（review doc 第 1 条）：截断续跑分支无条件 `has_more_tool_calls = true`，会抑制循环顶部
  的 follow-up 轮询（条件 `!has_more_tool_calls && pending.is_empty()`）；review 给出的修法是：
  把 `continue_after_truncation` 纳入顶部 follow-up 轮询条件，轮询返回空时先 `pending.drain(..)`
  再走既有返回逻辑，避免把「还有待投递的 follow-up」误判成「任务已死」。
- 已落地（review doc 第 2、3 条，无需再动）：
  - 第 2 条：`agent.rs` 截断分支的 `Compacted` 事件顺序已对齐 `recover_from_overflow`
    （先 `end_auto_compaction()` 再报 `Compacted`，代码注释明确写明 `review finding 2026-11-16`）；
  - 第 3 条：`runtime.rs` watchdog 注释已统一为 `v0.17`。

## 步骤 2 [变更] 落地 review 第 1 条（worker，仅 `crates/nini-core/src/agent.rs`）

判定标准：改动可追溯到 review doc 第 1 条的「问题/修法」两行；追溯不到不做。

1. 把 `let mut continue_after_truncation = false;`（及其注释）**上移到**顶部 follow-up 轮询之前
   （与 `let mut follow_up_rounds = 0usize;` 相邻）。理由：当前声明在 turn 循环体下方，
   顶部守卫 `if !has_more_tool_calls && pending.is_empty() {` 引不到它；保持
   `retry_after_compaction` 的位置不变（本轮不碰）。
2. 顶部 follow-up 轮询条件改为放行截断续跑：
   ```rust
   let poll_after_truncation = continue_after_truncation;
   if (!has_more_tool_calls && pending.is_empty()) || poll_after_truncation {
   ```
   轮询返回空（`followup.is_empty() || max_follow_up_rounds 已用尽`）时：
   - 先 `pending.drain(..);`（截断续跑期间队列里可能已排了消息，先清空不丢）；
   - **非续跑**：走原有 `self.set_phase(Idle); yield PhaseChanged(Idle); return;`；
   - **续跑**：不能提前结束运行（那会把「还有待投递的 follow-up」误判成任务已死），
     改为置 `Working` 走本轮既有收尾（本轮无工具调用时正常结束）。
3. 在 turn 循环**末尾**（`get_steering_messages` 轮询之后、循环闭合 `}` 之前）把标记置回
   `continue_after_truncation = false;`。理由：标记只在「本次截断续跑的那一轮」需要放行轮询，
   置回后后续每一轮都走常规路径，否则标记一直为 true 会让轮询在每一轮重复触发（空轮询空转）。
4. 不做 Out of scope：不改其他长任务改动、不改 `runtime.rs`、不新增依赖、不改既有测试。

## 步骤 3 [验收]（编排者执行）

- `cargo fmt --all -- --check` → 退出码 0。
- `cargo clippy --workspace --all-targets -- -A clippy::pedantic -A clippy::nursery -A clippy::perf -A clippy::complexity -D warnings` → 退出码 0。
- `cargo test -p nini-core` → 退出码 0（重点：`truncation_continues` 6 条、
  `steering_followup_hook_tests`、`overflow_recovery`、`compaction_lifecycle`）。
- `cargo test --workspace --all-targets` → 退出码 0。
- `bash scripts/gate.sh` → 退出码 0（含 README/CHANGELOG 测试数与实测一致）。
- 记录：该条 MINOR 是**潜在结构修复**（抑制的是对 follow-up 轮询的调用），在现有测试口径下
  不产生可观测的行为差异（follow-up 投递本身已被既有 follow-up 测试覆盖），因此验收以
  「既有行为不回归 + 门禁绿」为准，差异说明写进 changes 的 Blocked 段。

## 明确不做

- 不改 review 已落地那 2 条（第 2 条 `Compacted` 顺序、第 3 条 `v0.17` 标注）。
- 不做「去抑制」之外的任何设计变更（不改压缩、不改 `max_iterations` 语义、不改 CHANGELOG/README/依赖）。
- 不触碰在途未提交改动（`crates/nini-tui/src/runtime.rs` 的 clipboard 段、`agent_wire_e2e.rs`、
  `selection.rs`、`release/*`）。
