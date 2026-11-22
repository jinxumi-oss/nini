# nini「超越 pi」优化计划：稳定性 · 效率 · 准确性 · 工具可扩展性

> 建立：2026-11-20
> 目标：nini 核心已具备 pi 的基础能力，本计划在**稳定性、效率、准确性、工具可扩展性**四个维度超越 pi agent。
> 约束：**优化，不是过度新增**；每一处改动必须能追溯到需求或实测证据，追溯不到的删掉。
> 权威：目标是本文件；`docs/GOALS.md` 的四条常量与 `docs/plans/parity-with-pi.md` 的方法论是执行前提，冲突以 GOALS 为准。

---

## 0. 定位：什么叫「超越」，什么叫「过度工程」

**「超越 pi」不是新增功能堆叠。** nini 已有的四套机器——冻结 oracle（`references/pi-0.85.1/` + `pi-oracle-lock.py`）、conformance fixture、invariant 验证（`scripts/invariants.py` + `docs/invariant-verification.md`）、交付门禁（`scripts/gate.sh`）——**本身就是 pi 没有的稳定性/准确性资产**。本计划的工作是：**把这四套机器的覆盖范围扩到四个维度，并对现有实现做成本优化**，而不是另起一套「超越框架」。

**过度工程的判定（与 AGENTS.md「极短路径」一致）**：
1. 能复用 `config_value` / `ToolRegistry` / `conformance` / `invariant` 却新建平行系统的，删；
2. 没有实测证据就引入依赖、抽象层、设计模式、框架的，删；
3. 改动不能追溯到需求原文或复现证据的，删；
4. 把 nini 内部正确性问题塞进 `parity-gaps.json`（该台账只记「与 pi 的差距」）的，不塞——塞进去会让台账说谎。

---

## 1. 基线（2026-11-20 实测，来源写在每行）

| 事实 | 值 | 证据 |
|---|---|---|
| 版本 | **v1.0.4**（发布提交 `2b64063`） | `git log`；`release/` 产物 |
| 测试 | **2271 通过 / 0 失败** | 最新提交 `766635d`「测试数声明与实测对齐到 2271」；`cargo test --workspace` |
| 差距台账 | **58 条：fixed 48 / fake 10 / open 0** | `python3 scripts/parity-gap.py --json` |
| invariant | **161 条：SELF_TEST 117 / ANNOTATED 35 / CORROBORATED 6 / WEAK 1 / NONE 2** | `python3 scripts/invariants.py --json` |
| 2 条 NONE 详情 | `78593581` clipboard 第二入口（`image_paste.rs`）；`040e2d4a` watchdog 持锁死锁（`runtime.rs`） | 同上 |
| 流式渲染提速 | **E2/E3 已落地**（`wrap_line` 不再逐字素分配、一帧一行只解析一次） | commit `6a61723`，随 v1.0.4 发布 |
| 配置值解析 | `config_value.rs` 已实现 `$VAR`/`${VAR}`/`!cmd` 并接入 `provider_factory.rs` | `crates/nini-core/src/config_value.rs`、`provider_factory.rs:885` |
| 工具注册 | `ToolRegistry` 已有 `register`/`register_mut`，信任门控已包裹每个工具 | `crates/nini-core/src/tool.rs`、`trust_gate.rs` |
| Provider | 9 原生 + 28 薄壳 = 37 个 `--provider` 名 | `GOALS.md` §2；`provider_factory.rs` |
| 已知欠账 | headers 值取字面值（未走 `config_value`）；D5 剩余 model 级字段 | `crates/nini-core/src/settings.rs` 注释；`parity-gaps.json` D5 |

> 关键：**台账已关账**（open 0）。本计划是「超越」，不是「补 parity 欠账」，因此**不新建 pi-差距台账**，工作项直接在本文件登记（§4），机器判定仍复用既有 `gate.sh` / `invariants.py` / `cargo test`。

---

## 2. 方法论（全部沿用，不新建）

1. **每条工作项必须有会红的 oracle**，不是「跑得通」。判据 = 一条命令 + 一个退出码。
2. **oracle 只能引用修复前后都存在的 API**；接口会变的修复，oracle 走签名不变的更高层入口。
3. **先取证再动手**：热点、缺陷、字段语义都要有实测/行号证据，不许猜。
4. **变异注入**：每条关键修复都要记录「把实现改坏 → 哪个断言红」，否则判据是装饰。
5. **单调性门禁**：每轮 `invariants_none` 不增、测试数不减、`pi-oracle-lock --verify-refs` `drift=0`。
6. **不新增依赖、不引入抽象层/设计模式/框架**，除非有实测证据证明现有实现不够（R9 教训：体积估算被实测从 +2–5MB 推翻到 +0.63MB）。

---

## 3. 排序规则（唯一版本）

1. **先还债，再增强**：能补断言/接线的（稳定性 NONE、headers 值）先做，成本最低、可判定最直接。
2. **同成本下，影响用户可感知行为优先**（渲染帧率 > 配置字段 > 内部接线）。
3. **依赖前序的项后置**（如 per-tool 信任策略依赖信任门控已接线）。
4. **被 owner 决策阻塞的不排期**，写清缺什么；本计划暂无此类。
5. **条件性工作项**（先取证，无证据则不做并如实登记）排最后。

按此排序：**S1 → E1 → A1 → X1 → A2 → X2 → E2**。

---

## 4. 工作项

### S1 —— 补上 2 条 NONE invariant 的最小断言（稳定性 · 还债）

**现状取证**：`78593581`（`image_paste.rs` 第二剪贴板入口）、`040e2d4a`（`runtime.rs` watchdog 持锁死锁）都是**已修复但无自测**的真实缺陷（`invariants.py --json` tier=NONE）。pi 没有形式化/变异验证体系，这是 nini 已有、未兑现的资产。

**改动（最小，不新建框架）**：
- `78593581`：在 `crates/nini-tui` 为第二剪贴板入口加**一条单测**，覆盖其成功与失败缝（复用既有 clipboard 的测试缝，不新建 clipboard 抽象）。
- `040e2d4a`：在 `crates/nini-tui` 加**一条并发探针**（参照既有 `esc_concurrency_probe.rs` 的模式），断言 watchdog 推终止通知的路径**不在持锁状态下**调用 `lock_state`。

**oracle**：
- `cargo test -p nini-tui --test <image_paste_or_watchdog>` 退出 0；
- 变异注入：把 watchdog 的「先释放锁再取状态」改回「持锁调用」→ 探针红；把剪贴板第二入口的失败缝改回静默成功 → 单测红。

**退出判据**：`python3 scripts/invariants.py --json` 的 `NONE` 计数从 2 降到 0；两条留痕写进 `docs/invariant-verification.md`（注入什么、哪个断言红）。

**防过度工程**：不为这两条新建「稳定性测试框架」或「并发验证库」；复用 `nini-tui` 既有测试模式与 `invariant-verification.md` 留痕。

---

### E1 —— 把已落地的流式渲染提速固化为回归门禁（效率 · 防回退）

**现状取证**：E2/E3 已在 v1.0.4 落地（`6a61723`：`wrap_line` 逐字素分配消除、一帧一行只解析一次），边际成本已从 0.345 ms/KB 压到目标区间。**但提速本身还没有一条「改坏就会红」的回归 oracle**——这是最大的工程债：将来一次「顺手优化」就能让它悄悄回退，而全仓测试仍绿。

**改动**：
- 把 `crates/nini-tui/tests/streaming_render_perf.rs` 的阈值（帧成本、边际成本）纳入 `scripts/gate.sh` 门禁项，而非只在开发时跑一次。
- 不新增 profiling 框架；复用该测试的 release-profile 测量模式。

**oracle**：
- `bash scripts/gate.sh` 全绿；
- 变异注入：把 `wrap_line` 改回逐字素 `Vec<(String,Style)>` 分配 → 门禁红；把 `render_transcript` 改回双解析 → 门禁红。

**退出判据**：`bash scripts/gate.sh` 退出 0，且两条变异各自变红（留痕进 `docs/plans/nini-tui-streaming-render-perf-gate.md` 的变异段）。

**防过度工程**：不做「通用渲染性能分析器」「帧预算调度器」；pi 没有这些，加了即债。

---

### A1 —— headers 值复用 `config_value` 解析（准确性 · 复用而非新建）

**现状取证**：`settings.rs` 注释明示 `headers` 的值**取字面值**，未走 Pi 的 `resolveHeadersOrThrow`；而 `config_value.rs` 已实现全部解析规则并已接入 `provider_factory.rs:885`。这是**一处接线缺失，不是能力缺失**——再写一个解析器就是重复造轮子。

**改动**：在 `provider_headers_of`（`provider_factory.rs:355`）与 model 级 header 构造处，调用已有的 `resolve_config_value_detailed`，按 Pi 口径（解析失败即该头不发/报错，与 Pi 同语义）接线。**一行接线，复用全套测试**。

**oracle**：
- `cargo test -p nini-core --test <headers_resolve>` 退出 0：`models.json` 里 `"X-Key": "$MY_VAR"` 与 `"X-Key": "!cmd"` 都能解析出正确值；缺失变量时按 Pi 语义处理；
- 变异注入：把接线改回字面值 → 断言红。

**退出判据**：oracle 退出 0；`config_value` 既有测试零改动通过（证明未破坏复用件）。

**防过度工程**：不新建 schema 校验、不引入「配置值推断」；显式 struct + alias 已足够。

---

### X1 —— `ToolRegistry` 端到端接线验证（工具可扩展性 · 修缺陷非新增）

**现状取证**：`ToolRegistry`（`tool.rs:316`）已有 `register`/`register_mut`，`trust_gate.rs` 已能包裹每个工具。但**自定义工具是否真的进入系统提示词的工具列表、snippet、guidelines、信任门控、截断**，需要端到端验证——断了就是「声明了扩展点但用户不可见」的静默缺陷（v0.9.2 的教训）。

**改动**：
- 核对 `prompt_setup.rs` / `agent.rs` 的工具段装配，确认自定义工具进入 snippet 与 guidelines；
- 确认信任门控覆盖自定义工具（复用 `trust_gate.rs`，不改其结构）。

**oracle**：
- 新增 conformance 测试（`crates/nini-conformance/tests/`）：注册一个自定义工具，断言它出现在系统提示词的工具列表、被信任门控覆盖、能被调用；
- 变异注入：摘掉 snippet 接线 → 系统提示词里该工具消失 → 红；绕过信任门控 → 自定义工具在 distrusted 目录仍被调用 → 红。

**退出判据**：`cargo test -p nini-conformance --test <custom_tool_parity>` 退出 0。

**防过度工程**：**不重建**插件加载器、不引入 dlopen 热插拔、不做工具市场（TS 扩展路径已被证伪，`parity-with-pi.md` §6）。

---

### A2 —— D5 剩余 model 级字段接线（准确性 · 收敛构造点）

**现状取证**：v0.17 已把 provider/model 级 `headers`/`authHeader`/`compat`/`baseUrl`/`api`/`thinking_level_map`/`sampling_params` 接到请求；D5 仍剩 model 级 `cost`、`samplingParams`、`thinkingLevelMap` 等未完全承载。**注意**：`provider_factory.rs` 的 `Request` 有 30 个构造点、20 个在 oracle/测试里，**逐个加字段会打破 oracle 构造点**。

**改动**：用 `..Default::default()` 收敛（而非给 30 个构造点逐个加字段），按 v0.17 已建立的 `HeaderOverrides::resolve` 合并顺序（provider 默认 → provider 级 → model 级）承载剩余字段。**只接真正进入请求的字段**，`oauth`（无实现、产品决策）不动。

**oracle**：
- 对 `references/pi-0.85.1/model-config.js` 的 `ModelDefinitionSchema`/`ModelOverrideSchema` 逐字段对照；
- 变异注入：去掉 model 级 `cost` 承载 → 对照 oracle 红。

**退出判据**：oracle 退出 0；**`Request` 的 oracle 构造点零破坏**（`cargo test --workspace` 2271 不减）。

**防过度工程**：不为 D5 新建「模型配置校验框架」；显式字段 + alias 足够。

---

### X2 —— 按工具名的最小信任策略表（工具可扩展性 · 只做最小）

**现状取证**：`trust_gate.rs` 已对**所有**工具调用做信任检查（这是 nini 已超越 pi 的点——pi 只门控项目资源），且 `project_trust.rs` 注释已预留「v0.8 工具级策略」。但当前**所有工具继承同一 cwd 决策**，无按工具名区分。

**改动（只做最小）**：在 `ProjectTrustStore::check_and_warn` 上加**一个按工具名的策略表**（默认空 = 全部继承 cwd 决策），让用户能对特定工具（如 `bash`）单独收紧。**不引入沙箱、不引入生命周期、不引入热加载**。

**oracle**：
- 配置「`bash` 在该目录 distrusted、`read` trusted」后，断言 `bash` 被拒、`read` 放行；
- 变异注入：去掉工具名策略 → 两工具行为都变 → 红。

**退出判据**：`cargo test -p nini-core --test <per_tool_trust>` 退出 0。

**防过度工程**：这是**条件性增强**——若 owner 认为「信任粒度已够」，本项降级为「登记为已知边界」并加一条说明性 oracle（断言当前是「全工具继承 cwd」），**不硬做**。

---

### E2 —— 找下一个效率热点（效率 · 先取证，无证据则不做）

**现状取证**：流式渲染热点已处理。其余可能热点（启动时间、工具调用延迟、上下文用量计算、压缩摘要）**无实测数据，不许猜**。

**改动**：用既有的 release-profile 测量模式（同 `streaming_render_perf.rs`）对候选热点**先测**；**只有实测出「成本与长度线性/平方增长」或「每调用固定高开销」的，才动手**；测不出就如实登记「本轮无热点，不新增优化」。

**oracle**：测量脚本 + 阈值（同 E1 的门禁化方式）。

**退出判据**：若动手，退出判据同 E1（阈值进 gate + 变异红）；若不动手，退出判据是「实测报告 + 登记无热点」，**不假装做了**。

**防过度工程**：这是本计划唯一明确「可能什么都不做」的项，目的是防止为了「超越」而制造伪优化。

---

## 5. 每轮通用门禁（可判定，一条命令 + 一个退出码）

| 检查 | 命令 | 要求 |
|---|---|---|
| 交付门禁 | `bash scripts/gate.sh` | 退出 0；`invariants_none` 不增、测试数不减 |
| 台账单调性 | `python3 scripts/parity-gap.py --json` | open 不增（本计划不新增 pi-差距，故 open 保持 0） |
| 符号引用 | `python3 scripts/pi-oracle-lock.py --verify-refs` | 退出 0、`drift=0` |
| 测试数 | `cargo test --workspace` | ≥ 2271 且 0 失败 |
| 快照 | 前后 JSON 存 `docs/parity-snapshots/` | 守住单调性，供历史检查 |

> **禁止修改** `scripts/gate.sh` 与 `scripts/invariants.py` 的判定逻辑——它们是「门禁本身要可信」的一部分。

---

## 6. 反过度工程 / 防工程债硬约束

| 禁止 | 理由 |
|---|---|
| 新建平行的验证/度量/渲染/解析框架 | 已有 conformance/invariant/perf-test/config_value，复用 |
| 给 `Request` 逐构造点加字段 | 用 `..Default` 收敛，否则 oracle 构造点全碎 |
| 引入新依赖做「增强」 | 除非有实测体积/收益数据（R9 教训） |
| 抽象层 / 设计模式 / 框架化 | AGENTS.md 极短路径硬禁止，追溯不到需求即删 |
| 防御性扩展 / 顺手优化 / 补无关测试 / 动无关文件 | 改无关格式、改 CHANGELOG/文档即债（需求没明确要求） |
| 把 nini 内部正确性问题塞进 `parity-gaps.json` | 该台账只记「与 pi 的差距」，塞进去让台账说谎 |
| 为「超越」而新增 pi 没有的体系（稳定性仪表盘、性能分析器、插件运行时） | pi 没有，加了就是债，不是超越 |

**验收标准达成即停**，不做「以防万一」。需求口径不清时，不自行发明业务规则，写成「需要人确认：X」并交人决策。

---

## 7. 明确不做（范围决策已定，不重开）

| 项 | 理由 |
|---|---|
| TS 扩展运行时 | 已证伪（Pi 加载 TS，nini 无共同执行模型），`--extension` 显式拒绝是正确行为 |
| OAuth / 凭据存储 | 产品决策（R4），保持 env-var-only |
| mermaid 真图渲染 | 需布局引擎，Pi 实现不在快照（R12，不做） |
| 语法高亮 | R9 已拍板本轮不做，判据改为「9 个槽各能出对应颜色」 |
| 通用性能分析 / 稳定性度量体系 | 复用既有测试与 gate 即可，新建即债 |

---

## 8. 风险与未知

| 风险 | 处置 |
|---|---|
| E2 无实测热点 | 如实登记不动手，不制造伪优化（§4 E2 已写死） |
| X2 信任粒度需求不确定 | 降级为「登记边界 + 说明性 oracle」，不硬做 |
| A2 构造点扩散 | 用 `..Default` 收敛，`cargo test --workspace` 2271 不减作为硬门禁 |
| S1 并发探针难确定性复现死锁 | 参照 `esc_concurrency_probe.rs` 既有模式；若无法确定性复现，改用「源码结构 oracle + 人工变异留痕」，不假装有并发测试 |
| 流式渲染回归 | E1 把阈值纳入 gate，变异注入留痕 |

---

## 9. 里程碑

| 阶段 | 内容 | 退出判据 |
|---|---|---|
| M1 还债与固化 | S1、E1 | `invariants.py` NONE=0；`gate.sh` 全绿含渲染阈值；变异留痕 |
| M2 准确性与扩展 | A1、X1 | headers 解析 oracle 绿；自定义工具 conformance 绿 |
| M3 增强与验证 | A2、X2、E2 | D5 对照绿且构造点不增；per-tool 信任 oracle 绿（或降级登记）；E2 有实测报告 |

每阶段结束：跑 §5 全部门禁，前后快照存 `docs/parity-snapshots/`，CHANGELOG 只记录**实际交付**，不预写未做的能力。
