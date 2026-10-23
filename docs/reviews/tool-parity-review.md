# tool-parity.md（T1–T4）只读审查

审查范围：`scripts/tool-parity-check.py`、`scripts/pi-tool-contracts.py`、`references/pi-0.85.1/dist/api/system-prompt.js`、`crates/nini-core/src/tool.rs`、`crates/nini-core/src/agent.rs`、`crates/nini-cli/src/prompt_setup.rs`、`crates/nini-cli/src/tool_registry.rs`、`crates/nini-tools/src/{read,find,grep}.rs`。

## Critical

- `scripts/pi-tool-contracts.py:178` — `--compare` **恒 `return 0`**，即使 7 条全不一致。计划 §3/T1 的退出判据"退出 0，且逐条 ✓"里的退出码部分与"红的凭据：→ 退出非 0"都不成立。
- `scripts/pi-tool-contracts.py:56-64,149` — `NINI_FILES` **没有 `ls`** → `nf = REPO / ""` 是目录 → `ndesc` 恒 `None` → ls 行永远 ✗。T2 的退出判据"ls 也必须 ✓"在当前代码下**不可达成**。
- `scripts/pi-tool-contracts.py:66-77` — Pi 侧"最长字面量"**不插值**：实测 bash/read/find/grep/ls 的描述里留着 `${DEFAULT_MAX_BYTES / 1024}`、`${DEFAULT_LIMIT}`、`${GREP_MAX_LINE_LENGTH}`。计划 §2 的字数基线（298/344/…）是占位符文本长度，不是运行时文本，违反计划 §6 决策规则第 1 条。
- `scripts/pi-tool-contracts.py:79-84,154-155` — `extract_params` 对 7 个工具全返回 `[]`（typebox 产物里没有 `properties:`），`nini_params` 硬编码 `None` → **参数 schema 从未被比对**；T1 声称"描述与 schema 同时改"没有任何机器判据。
- `scripts/tool-parity-check.py:60` — `"ls" not in lib` 是子串判断：`"tools"`/`"nini_tools"` 都含 `ls` → T2 的第二道检查恒通过，只剩 registry 的 `'"ls"'` 一道（且可能匹配到无关字符串字面量）。
- `crates/nini-cli/src/tool_registry.rs:144-147` — 断言 `build_tools()` 必须**正好 6 个工具** → T2 加 `ls` 必红（计划未列此测试）。
- `scripts/pi-tool-contracts.py:87-104` + `crates/nini-tools/src/read.rs:61-62` — nini 侧抽的是**源码文本**（保留 `\`+换行+缩进），与 Pi 运行时文本不可能逐字相等：逐字照抄后 `--compare` 仍全红；反之若把 `${DEFAULT_MAX_BYTES / 1024}` 原样写进 description 反而能"转绿"——模型看到占位符而看板说 ✓（假绿）。

## Warnings

- `crates/nini-cli/src/prompt_setup.rs:103` + `crates/nini-core/src/agent.rs:1123,2333-2334` — **双重装配**：CLI 基座已追加一次，agent 请求构建时再追加一次 → 最终请求里两段重复。T3 只改一处必然半 Pi 半旧。
- `crates/nini-core/src/tool.rs:579-651`、`crates/nini-cli/src/tool_registry.rs:140-165,168-190` — 依赖旧追加段标题（`## Tool self-descriptions` / `## Tool usage guidelines`）与 `- name:` 前缀。计划给的 `rg build_system_prompt_with_contributions` **查不到它们**（它们依赖的是标题文本，不是函数名）。
- `crates/nini-tools/tests/tool_disambiguation.rs:29-40` — 要求 find 的描述含 `bash` 与 `ls`；Pi 0.85.1 的 find 描述两者都没有 → T1 逐字对齐必红（计划未列）。
- `crates/nini-tools/src/read.rs:61` vs `references/pi-0.85.1/dist/tools-t1/read.js:8` — nini 的 offset 是 **0-indexed**，Pi 描述写 1-indexed；逐字抄描述会让模型传错偏移量（描述↔实现耦合）。
- `references/pi-0.85.1/dist/api/system-prompt.js:42` — Pi 0.85.1 的 t1 内置（read/write/edit/bash）**没有 `promptSnippet`**（只有 `dist/tools-t2/{ls,find,grep}.js` 有），而 nini 的 6 个工具都自带 nini 自造 snippet → 套用规则①得到的行为 ≠ Pi。
- `crates/nini-cli/src/prompt_setup.rs:64-104` — Pi 同构的"基座 + Available tools + Guidelines"整体住在 `pi-ai` 的 `buildSystemPrompt`；nini 的**基座在 nini-cli**（不是计划说的 nini-core），core 只管追加段 → "只改 nini-core 的 build_system_prompt_with_contributions"单独做不到。

## Suggestions

- 两侧都比**运行时值**而非源码文本：Pi 侧用 node 跑快照工厂函数（`createLsToolDefinition` 等）取运行时 `description`/`parameters`，一次性冻结成 `references/pi-0.85.1/tool-contracts.runtime.json`（typebox 可从本机 `node_modules` 借，CI 只需 Python）；nini 侧用一个 Rust 小工具输出 `spec().description` + `serde_json::to_value(input_schema)` + contribution，两份 JSON 逐字段 diff。
- 看板自身要有"红的凭据"：加一条自检（把 compare 输出伪造成全 ✓，看板必须变红），否则无法证明 T1 的判据不是恒绿。
- T3 的判据应写成"**snippet 集合**与 Pi 对齐（Pi 0.85.1 只有 ls/find/grep）"，而不是"有 snippet 就列"。

## Answers

**Q1 不可靠，会误判为全绿。** 依据：`scripts/tool-parity-check.py:44` 用 `startswith(工具名) + "✗" in line` 解析**给人看的表格**；`scripts/pi-tool-contracts.py:178` 恒退出 0；`:60-64` 在快照文件缺失/改名时 `contracts` 为空 → 一行都不打印 → `bad` 为空且 `code==0` → 看板输出"全部工具的描述与 0.85.1 一致"（`:51`）。列宽/符号/颜色/JSON 化的任何格式变化都同样让 `bad` 变空。**最小可靠改法**：看板改调 `--json` 并解析：断言 `len(rows)==7`、`set(tool)==期望集合`、每条 `identical==True`，任一不满足即 ✗；同时把 `pi-tool-contracts.py:178` 改成有差异时 `return 1`，让退出码成为一等判据（计划里的"退出非 0"才有意义）。

**Q2 不能当 oracle。** 依据：Pi 侧"最长字面量"不插值（5/7 条含 `${...}`）、且没限定在工具定义对象内（文件里任何更长的 `description:` 都会被选中）；nini 侧取**第一个** `description:`（`read.rs:69` 的 JSON-schema 字段 description 与之相邻）、并把源码续行 `\`+缩进当内容；params 抽取恒空。**最小可靠做法**：比运行时值——Pi 侧 node 执行快照拿运行时 description/parameters 并冻结成 JSON fixture（像 T4 的 fixture 做法，CI 不依赖 node）；nini 侧用 Rust 自己 dump `spec().description` 与 `input_schema`；schema 用结构化比对（参数名/类型/必填/默认值），文本只做 normalize 后比对。

**Q3 因果不成立。** 依据：`system-prompt.js:60` 的分支是 `(hasBash||hasPowerShell) && !hasGrep && !hasFind && !hasLs`；nini 现在**已有 grep 和 find**（`tool_registry.rs` 的 `build_tools()`，6 个工具）→ 该分支在 nini 里**早已被 grep/find 关掉**，与 ls 无关，所以"T2 改变 guideline 分支"不成立；ls 只在"只有 bash"的**合成测试夹具**里有影响。漏掉的依赖：(a) 真顺序是 **T1 → T2**（ls 的描述必须来自 `tools-t2/ls.js` 运行时文本，且比对机制要先修好，否则 ls 永远比不过）；(b) T3 与 T2 可交换（ls 缺席只是不出现）；(c) T3 的真依赖是"**工具集在 prompt 组装处可见**"——Pi 在 `system-prompt.js:8` 拿 `selectedTools`+`toolSnippets`，nini 基座在 `prompt_setup.rs`、聚合在 `tool.rs:472`、`agent.rs:1123` 又聚合一次，判据必须落在同一处；(d) 漏掉的真依赖是 **snippet 集合**（Pi 0.85.1 只有 ls/find/grep 有 `promptSnippet`），直接套规则① ≠ Pi。

**Q4 部分可行，但计划漏了调用方与既有断言。** 依据：(1) **有隐藏依赖**：`crates/nini-core/src/tool.rs:579-651`、`crates/nini-cli/src/tool_registry.rs:140-165,168-190` 断言旧标题与 `- name:` 前缀（Pi 的 `Guidelines:` 是扁平 `- guideline`，无前缀 → 必红），`docs/research/pi-architecture.md:195`、`CHANGELOG.md:2651` 记录旧形状——计划给的 `rg build_system_prompt_with_contributions` 只能查到函数与调用方，查不到**按标题文本**的依赖；(2) **双重装配**：`prompt_setup.rs:103` 与 `agent.rs:1123` 各追加一次 → T3 必须指定唯一装配点，否则 Pi 同构的 "Available tools:" 会出现两遍；(3) **分层结论要修正**：nini 的基座在 `nini-cli/src/prompt_setup.rs`（不是计划 §2 说的 nini-core），core 只产追加段 → "改 core 的 `build_system_prompt_with_contributions` 成 Pi 同构"单独做不到；不需要搬层，但必须先约定"基座组装 + 贡献聚合"跨 crate 的唯一分工（把工具集/启用状态显式传进去）。
