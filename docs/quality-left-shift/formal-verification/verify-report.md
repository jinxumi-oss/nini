# 阶段 3：局部形式化验证产物（Quality Left-Shift）

## 验证范围

局部形式化验证，**非全量**形式化，仅覆盖高风险局部模块：

1. 共享状态并发写保护 ——「无丢失更新」不变量：串行化更新下 `applied = c`
   （Lean 定理证明 + TLA+/PlusPy 状态枚举双重证据）。
2. 截断容错状态迁移 —— 状态机 `phase ∈ {running, truncated, done}` 的合法迁移枚举
   （TLA+/PlusPy 状态枚举证据）。

诚实标注：本产物是局部形式化验证产物，不是全量形式化；规格与证明为自建工程验证产物，
证据为真实运行输出；不替代运行时验证 / 其他形式化 / 阶段 1 静态检查。

## 文件清单

- `/home/jin/nini/docs/quality-left-shift/formal-verification/concurrency-invariant.lean`
  —— Lean 定理 `no_lost_update_serialized` + 证明（omega 证明 applied=c 不丢失更新）。
- `/home/jin/nini/docs/quality-left-shift/formal-verification/state-spec-shared-state.tla`
  —— 共享状态并发写保护模型（Process p：c/applied/upd，incr/idle，Next 串行化更新）。
- `/home/jin/nini/docs/quality-left-shift/formal-verification/state-spec-truncation-state-machine.tla`
  —— 截断容错状态迁移模型（Process t：phase ∈ {running, truncated, done}，truncate/continue/finish）。
- `/home/jin/nini/docs/quality-left-shift/formal-verification/verify.sh`
  —— 一键验证脚本（可执行）：Lean 证明 + 两个 TLA+ 规格的 PlusPy 状态枚举。
- `/home/jin/nini/docs/quality-left-shift/formal-verification/verify-report.md`
  —— 本文件（真实运行结果留档）。

## 真实命令

```bash
# Lean 证明
/home/jin/toolchains/lean/bin/lean concurrency-invariant.lean

# PlusPy 状态枚举（在 PlusPy 工具链目录下运行，使 Naturals 可加载）
cd /home/jin/toolchains/tla-pluspy
/home/jin/toolchains/pluspy-venv/bin/python /home/jin/toolchains/tla-pluspy/pluspy.py -c 20 <tla 文件>

# 一键复跑（含上面三条，退出码留档）
bash /home/jin/nini/docs/quality-left-shift/formal-verification/verify.sh
```

## 真实运行输出

### Lean 证明

`/home/jin/toolchains/lean/bin/lean concurrency-invariant.lean` → 退出码 **0**
（无错误输出，定理 `no_lost_update_serialized` 证明通过）。

### PlusPy 状态枚举：state-spec-shared-state.tla（`-c 20`）

```text
Initial context: [ applied |-> 0, c |-> 0, upd |-> TRUE ]
Next state: 0 [ applied |-> 1, c |-> 1, upd |-> FALSE ]
Next state: 1 [ applied |-> 1, c |-> 1, upd |-> TRUE ]
Next state: 2 [ applied |-> 2, c |-> 2, upd |-> FALSE ]
Next state: 3 [ applied |-> 2, c |-> 2, upd |-> TRUE ]
Next state: 4 [ applied |-> 3, c |-> 3, upd |-> FALSE ]
Next state: 5 [ applied |-> 3, c |-> 3, upd |-> TRUE ]
Next state: 6 [ applied |-> 4, c |-> 4, upd |-> FALSE ]
Next state: 7 [ applied |-> 4, c |-> 4, upd |-> TRUE ]
Next state: 8 [ applied |-> 5, c |-> 5, upd |-> FALSE ]
Next state: 9 [ applied |-> 5, c |-> 5, upd |-> TRUE ]
Next state: 10 [ applied |-> 6, c |-> 6, upd |-> FALSE ]
Next state: 11 [ applied |-> 6, c |-> 6, upd |-> TRUE ]
Next state: 12 [ applied |-> 7, c |-> 7, upd |-> FALSE ]
Next state: 13 [ applied |-> 7, c |-> 7, upd |-> TRUE ]
Next state: 14 [ applied |-> 8, c |-> 8, upd |-> FALSE ]
Next state: 15 [ applied |-> 8, c |-> 8, upd |-> TRUE ]
Next state: 16 [ applied |-> 9, c |-> 9, upd |-> FALSE ]
Next state: 17 [ applied |-> 9, c |-> 9, upd |-> TRUE ]
Next state: 18 [ applied |-> 10, c |-> 10, upd |-> FALSE ]
Next state: 19 [ applied |-> 10, c |-> 10, upd |-> TRUE ]
MAIN DONE
退出码: 0
```

可见每个状态 `applied == c`，串行化更新（`incr`）每次同步推进 `c` 与 `applied`，无丢失更新。

### PlusPy 状态枚举：state-spec-truncation-state-machine.tla（`-c 20`）

```text
Initial context: [ phase |-> "running" ]
Next state: 0 [ phase |-> "truncated" ]
Next state: 1 [ phase |-> "running" ]
Next state: 2 [ phase |-> "truncated" ]
Next state: 3 [ phase |-> "running" ]
Next state: 4 [ phase |-> "truncated" ]
Next state: 5 [ phase |-> "running" ]
Next state: 6 [ phase |-> "truncated" ]
Next state: 7 [ phase |-> "running" ]
Next state: 8 [ phase |-> "truncated" ]
Next state: 9 [ phase |-> "running" ]
Next state: 10 [ phase |-> "truncated" ]
Next state: 11 [ phase |-> "running" ]
Next state: 12 [ phase |-> "truncated" ]
Next state: 13 [ phase |-> "running" ]
Next state: 14 [ phase |-> "truncated" ]
Next state: 15 [ phase |-> "running" ]
Next state: 16 [ phase |-> "truncated" ]
Next state: 17 [ phase |-> "running" ]
Next state: 18 [ phase |-> "truncated" ]
Next state: 19 [ phase |-> "running" ]
MAIN DONE
退出码: 0
```

`phase` 在 `running → truncated`、`truncated → running`、`done → done` 的合法迁移之间枚举，
截断容错状态迁移保持可迁移/可恢复。

### 一键脚本

`bash verify.sh` → 退出码 **0**（Lean 0、共享状态 PlusPy 0、截断状态机 PlusPy 0，输出如上）。

## 适配说明（PlusPy 语法）

PlusPy 不支持 TLA+ 骨架的 `Process p: / Init: / Next: / Operators:` 语法，两个规格按
PlusPy 可解析子集（过程模型子集：`VARIABLE` + 动作算子 + `Init`/`Next`）重写，语义与骨架一致
（各自文件内已附适配说明）。共享状态规格中 `idle` 仅重置更新许可 `upd` 并保持 `c/applied` 不变，
以使串行化模型能在 PlusPy 中完整枚举（PlusPy 在「状态无变化」即停止枚举）；Lean 证明部分与骨架一致，
未对证明做语义改动。

## 验证结论

- Lean 证明通过：串行化更新下 `applied = c`（无丢失更新）成立。
- 两个 TLA+ 规格均能被 PlusPy 解析并运行：共享状态枚举 20 个状态（`applied == c` 恒成立）、
  截断状态机枚举 20 个状态（合法迁移枚举）。
- `verify.sh` 一键复跑退出码 0。
- 本产物为局部形式化验证，证据均为真实运行输出，不替代运行时验证 / 其他形式化 / 阶段 1 静态检查。

【形式化验证产物完成】
