这处是阶段 3 局部形式化验证的截断容错状态迁移模型（PlusPy 可解析版）。
模型 Process t（对应 TLA+ 骨架 state-spec-truncation-state-machine.tla）：
- phase : State —— 截断容错状态，取值 running（运行中）/ truncated（截断中）/ done（已完成）；
Next 定义合法迁移：running → truncated，truncated → running（截断后恢复运行），done → done（保持完成）。
操作：truncate() 置 truncated，continue() 置 running，finish() 置 done。
不变量：phase 只在上述合法迁移间变化，截断容错状态机保持可迁移/可恢复。
适配说明：本模型为 PlusPy 可解析的等价语义适配版（PlusPy 不支持 TLA+ 骨架的 Process/Init:Next: 语法，
仅支持过程模型子集），语义与骨架一致。
---- MODULE state_spec_truncation_state_machine ----
VARIABLE phase
running == "running"
truncated == "truncated"
done == "done"
Init == phase = running
truncate == /\ (phase = running) /\ phase' = truncated
continue_ == /\ (phase = truncated) /\ phase' = running
finish_ == /\ (phase = done) /\ phase' = done
Next == truncate \/ continue_ \/ finish_

====
