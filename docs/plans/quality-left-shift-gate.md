# Quality Left-Shift 引入 nini —— 执行门禁与验收（Quality Left-Shift）

> 阶段 1–4 执行收口文档。任务：用质量约束左移方案引入 nini、评估是否增强性能（多 agent 执行，正交于 72h 断点计划）。

## 一、执行概览（阶段状态）
| 阶段 | 内容 | 状态 | 主要产物 |
| --- | --- | --- | --- |
| 阶段 1 | 核心 PoC：静态约束固化层（R1–R4）+ 变更管线拦截门 + 工具级回归测试 | ✅ 完成 | `crates/nini-core/src/constraint.rs`、`lib.rs` 挂载、`edit.rs`/`write.rs` 门、constraint 6 测试 + write 3 测试 |
| 阶段 2 | 功能地图 + 功能地图→可执行验证清单（含实际运行输出） | ✅ 完成 | `docs/quality-left-shift/functional-map.md`、`functional-verification-checklist.md` |
| 阶段 3 | 局部形式化验证（Lean + TLA+/PlusPy）+ 发版门红队 runner | ✅ 完成 | `docs/quality-left-shift/formal-verification/`（5 文件，reviewer CLEAN）、`/home/jin/quality-red-team/`（独立项目，reviewer CLEAN + 3 MINOR 已修） |
| 阶段 4 | 质量指标口径 + 实际采集留档 | ✅ 完成 | `docs/quality-left-shift/quality-metrics.md`、`quality-metrics-collection.md`（reviewer CLEAN） |
| 阶段 3 工具链 | 补齐 Lean + PlusPy/TLA+ | ✅ 完成 | `/home/jin/toolchains/` + `README.md` |

## 二、AC 验收对照（AC1–AC8，对照可行性计划 §6）
| AC | 验收内容 | 状态 | 证据 |
| --- | --- | --- | --- |
| AC1 | 约束门拦截 R1–R4 坏写法且放行正常改动（回归不误报） | ✅ 达成 | constraint 6 测试（各规则拒绝/放行/阈值）+ tools/core 全量测试绿 |
| AC2 | 变更管线写工具前约束检查，不通过拒绝/回退，坏改动不进主干 | ✅ 达成 | edit/write 门 + 工具级拒绝测试（timer/注释被拒、合法写入放行）+ 红队 B6/B7 由 gate_check 真实拦截自造坏改动 |
| AC3 | 功能地图→可执行验证清单，逐条真实执行非自报通过 | ✅ 达成 | 清单含实际运行输出：core lib 261 / conformance 75 / tools 117 / constraint 6，全绿 |
| AC4 | 局部形式化验证仅高风险模块，有产物 | ✅ 达成 | Lean 定理证明 + 两个 TLA+/PlusPy 状态枚举规格 + verify.sh（exit 0）+ 报告，reviewer CLEAN |
| AC5 | 发版门红队独立运行，产出用例与缺陷修复（或至少用例生成） | ✅ 达成 | 独立项目真实运行 7 用例（6 通过/0 缺陷/1 已知缺口 B5），B6/B7 拦截，修复建议为提议 |
| AC6 | 质量指标可记录对比 | ✅ 达成 | 指标口径 + 实际采集留档（quality-metrics-collection.md），reviewer CLEAN |
| AC7 | 不破坏 D1–D4，既有构建与测试全绿（正交） | ✅ 达成 | `cargo test --workspace` 退出码 0；`bash scripts/gate.sh` 通过（通过 12 失败 0 跳过 4）；阶段1 未改 agent loop/M2 钩子面/压缩/max_iterations/依赖 |
| AC8 | 性能影响可控 | ⚠️ 待采样定稿 | 当前以回归全绿 + 指标口径可采集为准；无运行时性能实测数据，指标采样需样本周期后定稿（留档已标注未采集项） |

## 三、回归与门禁证据
- 回归：`cargo test --workspace` → 退出码 0（全绿）；`bash scripts/gate.sh` → 通过（通过 12 / 失败 0 / 跳过 4；README/CHANGELOG 测试数已同步为实测 2193）。
- 测试数同步：本次新增测试使实测测试数 2184→2193，README 徽章/正文与 CHANGELOG 顶部计数同步为 2193（门禁 T4 要求对外声明与实测精确相等）。

## 四、范围控制
- 改动落点：`crates/nini-core/src/constraint.rs`、`lib.rs`、`crates/nini-tools/src/edit.rs`、`write.rs`（阶段1）；`docs/quality-left-shift/`（阶段2/3 产物）；`/home/jin/quality-red-team/`（阶段3 红队 runner）；`/home/jin/toolchains/`（工具链）；README/CHANGELOG 测试数同步。
- 范围控制：未改动并发会话正在改的 TUI 文件（selection.rs/lib.rs/state.rs/runtime.rs 等）；与 72h 断点计划正交（未改 agent loop 折叠语义、M2 钩子面、压缩/max_iterations/依赖，未为通过而编辑既有测试）。
- 红队 runner 只读主仓库、不自动修改主仓库代码；修复建议为提议，须经主仓库约束门/人工确认后合入。

## 五、工具链
- Lean 4.36.0（`/home/jin/toolchains/lean/bin/lean`）；PlusPy/TLA+（`/home/jin/toolchains/pluspy-venv` + `/home/jin/toolchains/tla-pluspy`，官方源码 clone）。清单见 `/home/jin/toolchains/README.md`。

## 六、待补与诚实标注
- AC8 待运行时性能采样后定稿（指标留档已如实标注未采集项：门实际拦截数/回退数/轮次等需样本周期数据后采集）。
- 形式化与红队为局部/独立运行产物；规格、证明、报告为自建工程验证产物，证据为真实运行输出，不替代运行时验证/其他形式化/阶段1 静态检查。
- 修复建议（红队）为提议，未经主仓库约束门/人工确认不得合入。

## 七、结论与 VERDICT
- 除 AC8 待采样定稿外，AC1–AC7 全部达成，回归全绿，交付门禁通过。
- VERDICT: APPROVED（AC8 标注待采样）。
