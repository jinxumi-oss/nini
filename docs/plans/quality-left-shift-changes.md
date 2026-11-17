# Quality Left-Shift 方案引入 nini —— 执行记录（阶段 1）

任务: `docs/plans/quality-left-shift-feasibility-plan.md`（可行性计划）
阶段: 1–4（多 agent 执行完成，2026-11-17）
状态: 已完成（2026-11-17，收口完成：DP 定稿 + 提交质量左移改动 + 门禁通过；AC8 待样本周期数据后采集定稿）

需求: 用质量约束左移方案引入 nini、评估能否增强性能；按计划阶段执行（多 agent，正交于 72h 断点计划）。

## Changes
- `crates/nini-core/src/constraint.rs` | 新增约束检查模块（R1–R4：热路径直接副作用/R1、跨目录后台引用/R2、注释占比/R3、定时器打补丁/R4）+ `ConstraintConfig`（R3 阈值/热路径 glob/非业务目录模式）+ `check_change`/`check_has_violations` + 6 单元测试（各规则拒绝、干净改动放行、阈值可配置）| 计划阶段 1 | 类型: 变更
- `crates/nini-core/src/lib.rs` | 新增 `pub mod constraint;`（挂载约束模块）| 计划阶段 1 | 类型: 变更
- `crates/nini-tools/src/edit.rs` | EditTool 写工具前插入约束门：change 取新内容行，不通过返回 `InvalidArgs`，坏改动不进主干（门位于 `mutation_queue` 写管线之上的工具层）| 计划阶段 1 | 类型: 变更
- `crates/nini-tools/src/write.rs` | WriteTool 写工具前插入约束门（同上）；新增工具级回归测试：定时器打补丁被拒绝、注释占比过高被拒绝、合法写入放行 | 计划阶段 1 | 类型: 变更
- `docs/plans/quality-left-shift-feasibility-plan.md` | 状态更新为「已执行阶段1 + 剩余阶段」，补充执行记录与裁剪说明 | 计划阶段 1 | 类型: 变更

## Commands
- `cargo test -p nini-core --lib constraint` → 退出码 0（6 passed）
- `cargo test -p nini-tools` → 退出码 0（97 passed + 集成 6/3/3/5 全绿）
- `cargo test -p nini-core` → 退出码 0（lib 261 passed + 集成全绿，含 `every_source_file_is_tracked_by_git`）
- `cargo fmt -p nini-core -p nini-tools` + `cargo fmt --all -- --check` → 退出码 0

## Reproduction
- 复现命令: `cargo test -p nini-core --lib constraint` → 退出码 0（约束 R1–R4 拒绝/放行/阈值用例）；`cargo test -p nini-tools -p nini-core` 复跑验证门与回归。

## Blocked
- 阶段 3（局部形式化验证 + 发版门红队）未执行：本地无 lean/tla 工具链（`which lean tla` 无输出），按可行性计划裁剪，待工具链就绪后补。
- 阶段 2（功能地图 → 运行时验证清单）与阶段 4（质量指标）未执行，待阶段 1 验证通过且 DP 决策后推进。
- 与 72h 断点计划正交：未改 agent loop 折叠语义、M2 钩子面、压缩/max_iterations/依赖，未编辑既有测试来通过。

## 阶段 2 / 4（已完成，2026-11-16）
- 阶段 2（多 agent）：产出功能地图 `docs/quality-left-shift/functional-map.md`（用户视角、三级颗粒度、F1–F10 四要素，F10 标注已知缺口）+ 功能地图→可执行验证清单 `docs/quality-left-shift/functional-verification-checklist.md`（映射表 + 实际运行输出：core lib 261 / conformance 75 / tools 97+集成 / constraint 6，全绿）。
- 阶段 3：未执行——本地无 lean/tla 工具链（`which lean tla` 无输出），按计划裁剪（局部形式化验证 + 发版红队待工具链就绪）。
- 阶段 4（多 agent）：产出质量指标与验收口径 `docs/quality-left-shift/quality-metrics.md`（指标定义/记录方式/验收对照 AC6/诚实标注）；按建议实际采集并留档 `docs/quality-left-shift/quality-metrics-collection.md`（5 条只读采集命令真实运行，计数与真实输出一致，reviewer 审查 CLEAN，未采集项如实标注）。
- **阶段 3（多 agent + reviewer）**：补齐工具链（Lean 4.36.0 + PlusPy/TLA+，落 `/home/jin/toolchains/`，清单见 `/home/jin/toolchains/README.md`）；产出局部形式化验证（`docs/quality-left-shift/formal-verification/`：Lean 定理 `no_lost_update_serialized` 证明 + 两个 TLA+/PlusPy 状态枚举规格 + `verify.sh` + `verify-report.md`，reviewer CLEAN）；产出发版门红队 runner（独立项目 `/home/jin/quality-red-team/`：cases/gate_check/runner/README，真实运行产出 report/run_history/proposals，reviewer CLEAN，3 个 MINOR 已修）；全局形式化/每次红队仍按计划排除。
- 与并发会话注意：本次全部改动只落在约束模块、core 挂载、tools edit/write 与 `docs/quality-left-shift/`、`/home/jin/toolchains/`、`/home/jin/quality-red-team/`，未触碰并发会话正在改的 TUI 文件。

## 收口（2026-11-17）
- DP 决策定稿（默认值，见可行性计划「DP 决策」）：DP1 局部形式化 + 发版门红队纳入、全局形式化/每次红队默认排除；DP2 门落点 edit/write 层；DP3 R1–R4 核心、暂不扩展；DP4 先 PoC 后全量；DP5 与 72h 计划正交协调。
- 执行收口：提交本次质量左移改动（constraint.rs、lib.rs、edit.rs、write.rs、README/CHANGELOG 测试数同步 2184→2193、docs/quality-left-shift/、docs/plans/quality-left-shift-*）；AC8 标注待样本周期数据后采集定稿。
- 门禁与回归：`cargo test --workspace` 退出码 0；`bash scripts/gate.sh` 通过（通过 12 / 失败 0 / 跳过 4）；README/CHANGELOG 测试数同步为实测 2193。
- 计划状态：可行性计划与本记录状态更新为「完成（收口完成）」。
