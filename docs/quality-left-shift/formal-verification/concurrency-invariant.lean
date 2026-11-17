/-!局部形式化验证（阶段 3）：共享状态并发写保护 ·「无丢失更新」不变量
目标：串行化更新下证明 applied = c（已应用更新数 == 共享计数器值，不丢失更新）。
工具：Lean 4（/home/jin/toolchains/lean/bin/lean）。

模型（对应 state-spec-shared-state.tla 的 Process p）：
- applied : Nat —— 已应用的更新数；
- c       : Nat —— 共享计数器当前值；
- upd     : Bool —— 更新许可标志，串行化更新时 upd 为「是否允许推进」：
  更新只在 upd = True 时被许可推进，且 applied 与 c 同步推进，
  因此不会出现一个更新推进了 applied 却没有推进 c（或反之）——即「无丢失更新」。

不变量：在串行化更新下（每次更新同时推进 applied 与 c，步长为 k），
applied = c 在每一步都保持成立。
-/

theorem no_lost_update_serialized
    (applied : Nat) (c : Nat) (applied' : Nat) (c' : Nat) (k : Nat)
    (h : applied = c) (h1 : applied' = applied + k) (h2 : c' = c + k) :
    applied' = c' := by
  omega
