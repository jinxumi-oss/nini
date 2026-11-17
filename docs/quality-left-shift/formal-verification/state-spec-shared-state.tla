这处是阶段 3 局部形式化验证的共享状态并发写保护模型（PlusPy 可解析版）。
模型 Process p（对应 TLA+ 骨架 state-spec-shared-state.tla）：
- c      : Nat —— 共享计数器值；
- applied: Nat —— 已应用的更新数；
- upd    : Bool —— 更新许可标志，串行化更新只在 upd=True 时被许可。
串行化更新 incr：upd=True 时 c 与 applied 各 +1（同步推进，不丢失更新），并置 upd=False；
非更新步骤 idle：保持 c/applied 不变，仅把更新许可重置为 True，使该串行模型能在 PlusPy 中逐状态枚举
（PlusPy 在「状态无变化」时即停止枚举，故 idle 只重置 upd、保留 c/applied 不变）。
不变量：在串行化更新下 applied = c（每次更新同步推进，无丢失更新）。
适配说明：本模型为 PlusPy 可解析的等价语义适配版（PlusPy 不支持 TLA+ 骨架的 Process/Init:Next: 语法，
仅支持单作用集合与过程模型子集），语义与骨架一致。
---- MODULE state_spec_shared_state ----
EXTENDS Naturals
VARIABLE c
VARIABLE applied
VARIABLE upd
Init == /\ c = 0 /\ applied = 0 /\ upd = TRUE
incr == /\ (upd = TRUE) /\ (c' = c + 1) /\ (applied' = applied + 1) /\ (upd' = FALSE)
idle == /\ (upd = FALSE) /\ (c' = c) /\ (applied' = applied) /\ (upd' = TRUE)
Next == incr \/ idle

====
