# Review

只审本次交付改动（`agent.rs`、`overflow.rs`、`truncation_continues.rs`、
`runtime.rs` 的 watchdog 段、`agent_abnormal_notify.rs`、`long_task_delivery.rs`、
`tool_batch_semantics.rs`、README/CHANGELOG 的 2136→2156 三处）。
其余并发会话改动（clipboard/lib/state/selection 等）未提。
已跑项（cargo fmt/clippy/全量测试/12 条 gate）均为绿，作为正确性基线。

- [MINOR] `crates/nini-core/src/agent.rs:2107` | 问题: 截断续跑分支无条件 `has_more_tool_calls = true`，会抑制循环顶部的 follow-up 轮询（`!has_more_tool_calls && pending.is_empty()`）。若该轮之前已有 `get_followup_messages` 队列的 follow-up，该轮之后 `has_more_tool_calls` 恒为 true，pending 永远不会被 drain，被 queued 的 follow-up 被永久丢掉——与「长任务持续交付」的目标冲突；同时代码注释称它是「唯一的继续信号」，与实际守卫也不完全一致。 | 修法: 截断续跑既不能只靠置 true 保命，也不能让它吃掉 follow-up 轮询：把标志 `continue_after_truncation` 纳入顶部 follow-up 轮询条件（如 `pending.is_empty() || continue_after_truncation`），并在轮询返回空时先 drain pending 再走既有返回逻辑，既避免误判任务已死，又能投递截断期间排队过的 follow-up。

- [MINOR] `crates/nini-core/src/agent.rs:2088` | 问题: 截断分支的压缩事件顺序与 `recover_from_overflow` 既有顺序不一致：后者是 `begin_auto_compaction → Compacting+PhaseChanged → compact → self.end_auto_compaction() → Compacted → Working+PhaseChanged`；本分支把 `yield AgentEvent::Compacted` 放在 `compact_history_via_hook()` 之后、`self.end_auto_compaction()` 之前，即 `Compacted` 发出时 `is_auto_compacting` 仍为 true（与 recover 路径在 Compacted 之后才复位）。功能上当前等价，但会让消费者在「压缩已报告」与「压缩生命周期已复位」之间产生不一致，容易分叉。 | 修法: 把 `yield AgentEvent::Compacted { summary, tokens_before }` 移到 `self.end_auto_compaction();` 之后，与 `recover_from_overflow` 顺序一致。

- [MINOR] `crates/nini-tui/src/runtime.rs:618` | 问题: 本次 watchdog 改动的注释标 `v0.15`，而同一次交付的 `crates/nini-tui/tests/agent_abnormal_notify.rs:30`、`crates/nini-core/src/agent.rs` 各改动与 `docs/plans/*.md` 均标 `v0.17`（同一处 watchdog 改动在测试文件里也明确写「v0.17 起超时只记日志并继续等同一个任务」），版本标注相互矛盾。 | 修法: 与本次交付的 v0.17 口径统一（改为 `v0.17`）；若该改动本属 v0.15 序列，则在同处（含 `agent_abnormal_notify.rs:30`）统一到同一版本号，避免版本线索失真。
