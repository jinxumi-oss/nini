# 工具对标计划（WF-BUILTIN）——把八个内置工具对齐到 Pi 0.85.1

> 建立：2026-10-23 · 状态：**active** · 上级：`docs/plans/parity-with-pi.md` §2.1 的 **WF-BUILTIN**
> 锚点：**Pi 0.85.1（冻结中）**，唯一来源 `references/TARGET.json`
> 看板：`python3 scripts/tool-parity-check.py`

---

## 0. 目标与范围

**目标**：让 nini 的**八个原始自带工具**（read / write / edit / bash / grep / find / ls / powershell）
在**契约**（模型看到的描述 + 参数 schema）与**语义**（截断、并发、超时、变更队列）上对齐 Pi 0.85.1。

**不在范围内**（明确排除，避免混排期）：

| 排除项 | 理由 |
|---|---|
| **WF-EXTENSION（扩展层）** | 另一件工程（成本高一个数量级）。需要 owner 决策；在那之前 README 要声明"扩展工具不在 parity 范围内"。见 `parity-with-pi.md` §2.1 与差距 C4 |
| **provider / auth / 模型目录** | 属于架构层（Stage 1），与工具面正交 |
| **TUI 的工具渲染外观** | nini 已有自己的渲染层；外观对齐是 L1，不在本计划 |
| **新增 Pi 之外的工具** | 本计划只做**对齐**，不做**扩展** |

**为什么先做这一层**：工具的契约与语义是"高效、准确完成任务"的直接载体——
模型怎么调工具、工具回什么、输出占多少上下文，全在这里。而且它是**可测**的：
同一份契约可以逐条比对，同一份输出可以逐字节比对。

---

## 1. 锚点与纪律

| 纪律 | 机器判定 |
|---|---|
| 只锚定 **0.85.1**，冻结中 | `scripts/pi-target-lock.py`（8 项，并入 CI） |
| 引用形态 `文件#符号`（含义钉住，行号由脚本解析） | `scripts/pi-oracle-lock.py --verify-refs`（101/101，drift=0） |
| 差距台账唯一 | `scripts/parity-gap.py --json` / `--check-plan` |
| 计数不许手写 | 计划自检 S9 会因文档里出现手写计数而失败 |

**Pi 侧的证据从哪来**：`references/pi-0.85.1/`（411 文件，sha256 锁）——
`dist/tools-t1/`（harness 层：read/write/edit/bash）、`dist/tools-t2/`（coding 层：grep/find/ls/truncate/…）、
`dist/api/system-prompt.js`（prompt 组装）、`tool-contracts.json`（提取出的契约）。

---

## 2. 基线（实测，随脚本复算）

**工具集**：nini 原有 6 个（read/write/edit/bash/grep/find），**缺 `ls`**；Pi 0.85.1 有 8 个（多 `powershell`，
Windows 专用，本计划**只登记不实现**——见 §8）。

> **执行结果（2026-10-23）**：T1–T4 全部完成。nini 现在有 **7 个**内置工具
> （read/write/edit/bash/grep/find/ls），与 Pi 0.85.1 的差别只剩 `powershell`（有意不做）。
> 契约对齐后暴露出的**语义**差异也一并修了：bash 默认超时 120s → 无默认超时（Pi 的契约如此，
> 而 120s 会杀掉长的构建）；read 的 offset 0-indexed → 1-indexed；grep 补上
> glob/ignoreCase/literal/context/limit 四个参数（此前**少 advertise** 四个能力，模型永远不会用）；
> grep 的输出格式改成 Pi 的 `path:line: text`（此前少一个空格）并按 500 字符截断行。

**契约**：7 个工具的描述**全部**与 0.85.1 不一致（nini 的抄自 0.84.2）。

| 工具 | Pi 0.85.1 描述字数 | nini 描述字数 | 状态 |
|---|---|---|---|
| bash | 298 | 245 | ✗ |
| read | 344 | 133 | ✗ |
| edit | 326 | 493 | ✗ |
| write | 127 | 132 | ✗ |
| grep | 279 | 112 | ✗ |
| find | 223 | 201 | ✗ |
| ls | 222 | — | ✗ 缺工具 |

**截断**：Pi 统一 `truncate.js`（`DEFAULT_MAX_BYTES = 50*1024`、`GREP_MAX_LINE_LENGTH = 500`、
`truncateHead`/`truncateTail`/`truncateLine`）；nini 是 bash 自实现 1 MiB / 2000 行头尾截断，
grep 用 `max_output_lines` 截行数，read 无统一路径。

**prompt 联动**：Pi 0.85.1 的 prompt 在 **`pi-ai/dist/api/system-prompt.js`**（协议层！）里组装：
- 工具**只有提供了 snippet 才进 "Available tools" 段**（`visibleTools = tools.filter(name => !!toolSnippets?.[name])`）；
- guideline **由工具集决定**：`if ((hasBash || hasPowerShell) && !hasGrep && !hasFind && !hasLs)` →
  "Use bash for file operations like ls, rg, find"；
- 固定两条："Be concise in your responses" / "Show file paths clearly when working with files"；
- prompt 里明说 "In addition to the tools above, you may have access to other custom tools depending on the project."

nini 的 prompt 组装在 `nini-core/src/tool.rs::build_system_prompt_with_contributions`，
结构是**追加段**（`## Tool self-descriptions` / `## Tool usage guidelines`），
**没有**"工具集决定 guideline"的规则，也没有"只有带 snippet 的工具才列出"的规则。

---

## 3. 工作项

> 每项都写明：**现状 → Pi 侧依据（符号引用）→ 改法 → 退出判据（一条命令 + 一个退出码）→ 红的凭据 → 风险**。

### T1 工具契约重捕获（对应差距 B13）

**现状**：nini 的工具描述抄自 Pi 0.84.2（`crates/nini-tools/src/*.rs`），未按 0.85.1 复核；
7 个工具全部不一致。

**Pi 侧依据**：`references/pi-0.85.1/dist/tools-t1/{read,write,edit,bash}.js`、
`dist/tools-t2/{grep,find,ls}.js`；参数 schema 是同文件里的 typebox 定义。
提取结果在 `references/pi-0.85.1/tool-contracts.json`（`scripts/pi-tool-contracts.py --extract`）。

**改法**：
1. 把每个工具的 **description 与参数 schema** 按 0.85.1 重写；
2. **关键**：描述不再手抄成 Rust 字面量后与 Pi 各存一份——而是**每条描述都注明它来自
   `references/pi-0.85.1/...` 的哪个符号**，并由脚本比对（`pi-tool-contracts.py --compare`）。
   否则下一次 Pi 升级重演"手抄 → 悄悄过期"。
3. `powershell`（Windows）**只登记不实现**，并在 README 的 known limitations 写明。

**退出判据**：
```console
python3 scripts/pi-tool-contracts.py --compare     # 退出 0，且逐条 ✓
```
**红的凭据**：把任一工具的描述改回 0.84.2 的旧文本 → `--compare` 该行变 ✗ → 退出非 0。

**风险**：Pi 的 description 里含**面向模型的措辞**（"must be unique in the original file"），
逐字照抄是最忠实的；但 Pi 的措辞依赖它的工具名与参数名，改名就会失配——
所以描述与 schema 必须**同时**改，不能只改一半。

---

### T2 补 `ls` 工具（对应差距 B11 的一半）

**现状**：`nini-tools` 没有 `ls`；Pi 0.85.1 的 `ls` 在 coding 层（`dist/tools-t2/ls.js`），
参数 `path`（默认 cwd）与 `limit`（默认 500）。

**Pi 侧依据**：`references/pi-0.85.1/dist/tools-t2/ls.js`；
参数与输出格式以该文件为准（含"列名 + 大小"的排版与 limit 截断提示）。

**改法**：新增 `crates/nini-tools/src/ls.rs`（读目录、排序、limit、输出格式对齐），
在 `nini-tools/src/lib.rs` 导出、在 `nini-cli/src/tool_registry.rs` 注册。

**退出判据**：
```console
python3 scripts/tool-parity-check.py --quick      # T2 行必须是 ✓
python3 scripts/pi-tool-contracts.py --compare    # ls 也必须 ✓
cargo test -p nini-tools                          # 退出 0
```
**红的凭据**：把 `ls` 从 registry 里摘掉 → T2 行变 ✗ → 看板退出非 0。

**风险**：`ls` 的**输出格式**是契约的一部分（模型据此解析目录内容）；
格式差一个空格就是行为差。此外 Windows 上的 `ls` 与 Pi 的 `powershell` 工具并存——
nini 只做 `ls`，README 必须说明这一点。

---

### T3 工具集 ↔ system prompt 联动（对应差距 B11 的另一半 + 新发现）

**现状**：nini 的 prompt 是"基础 prompt + 两个追加段"；**没有** Pi 的三条规则：
① 只有提供 snippet 的工具才进 "Available tools"；② guideline 由工具集决定；
③ 固定的两条 guideline。

**Pi 侧依据**：`references/pi-0.85.1/dist/api/system-prompt.js`
（`visibleTools`、`hasBash`/`hasGrep`/`hasFind`/`hasLs`/`hasPowerShell` 的条件分支）。

**改法**：把 `build_system_prompt_with_contributions` 改成 **Pi 同构**的组装：
- "Available tools:" 段只列**有 snippet 的**工具；
- "Guidelines:" 段按**已启用的工具集**生成（含条件分支）；
- 保留 nini 已有的 `system_prompt_contribution()` 机制（它已经是 Pi 的 `promptSnippet` 等价物），
  但把它接到 Pi 的结构上（而不是自己发明一套追加段）。

**退出判据**：
```console
cargo test -p nini-core --test prompt_tool_linkage   # 退出 0
```
该测试必须覆盖**四种工具集组合**（全开 / 只有 bash / 没有 bash / 空集），
并断言：列出的工具、出现的 guideline、以及"snippet 缺失的工具不出现"。
**红的凭据**：把条件分支改成恒真 → 四条里至少两条变红。

**风险（本计划里最高的一项）**：改 prompt = 改模型行为。
Pi 0.85.1 把 prompt 组装放在 **pi-ai（协议层）**，nini 放在 **nini-core**——
这是**分层相反**的一处。本轮只对齐**内容**，不顺手搬层（搬层属于架构 Stage 1）。
另：nini 现有的追加段结构可能被别处依赖，改之前必须先查引用（`rg build_system_prompt_with_contributions`）。

---

### T4 截断语义对齐（对应差距 B12）

**现状**：nini 的截断是各工具各写各的（bash 1 MiB/2000 行、grep 按行数、read 无统一）；
Pi 是**一个** `truncate.js`：50KB head/tail + 行宽 500。

**Pi 侧依据**：`references/pi-0.85.1/dist/tools-t2/truncate.js`
（`DEFAULT_MAX_BYTES`、`GREP_MAX_LINE_LENGTH`、`truncateHead`、`truncateTail`、`truncateLine`）。

**改法**：
1. `nini-tools` 建**一个** truncate 模块（head/tail/line 三个函数 + Pi 的默认值）；
2. 各工具改用它，删掉各自实现；
3. `ToolContext` 的两个默认值改成 Pi 的口径（字节上限 50 KiB；行宽 500）。

**退出判据**：
```console
cargo test -p nini-tools --test truncation_parity    # 退出 0
```
该测试用**固定样本**断言：超长输出的头尾各留多少字节、grep 单行截到多少字符、
以及"进入上下文的总字节数"与 Pi 的期望值一致（期望值由 Pi 侧算出后固化成 fixture，
因为 Rust 不能直接跑 Pi 的 JS）。
**红的凭据**：把字节上限改回 1 MiB → 至少两条变红。

**风险**：截断变小 ⇒ 上下文占用下降 ⇒ **长任务的上下文构成会变**。
这正是我们要的（对齐 Pi），但它会改变现有长任务的模型行为；
因此 T4 **必须排在 T1/T2/T3 之后**，并且单独一批、单独发版。

---

## 3.5 审查对本计划做了什么（2026-10-23）

多 Agent 审查（`docs/reviews/tool-parity-review.md`）提出的 5 个问题，全部为真并已修：

| 审查发现 | 处置 |
|---|---|
| `pi-tool-contracts.py` 用**启发式**抽描述（Pi 侧「最长字符串字面量」、nini 侧「正则 + 最长注释块」），不能当 oracle；Pi 的描述里还留着 `${...}` **未插值**的占位符 | 改为 **执行 Pi 自己的代码**取运行时值：`scripts/pi-tool-dump.mjs` → `references/pi-0.85.1/tool-contracts.json`（描述、参数 schema、snippet、guidelines 全部是运行时值） |
| `--compare` 恒返回 0 → 看板靠「输出里有没有 ✗」判断，**可能误判全绿** | 判据全部改成**一条命令的退出码**；对比脚本以 `--json` 输出结构化结果 |
| 排序理由（T2 必须先于 T3）**因果不成立** | 已改写：T2/T3 可交换，真正的依赖只有 T1 的提取机制 |
| 「改 prompt 结构但不搬层」有隐藏依赖：旧标题的断言在别处、且 **CLI 与 agent 双重装配**同一段 | 断言全部更新；双重装配已修（CLI 只拼基座，工具段由 agent 拼一次），并加了「只出现一次」的 oracle |
| Pi 侧 `promptSnippet` 只有 find/grep/ls 有 | 审查不完整：**8 个工具都有**（harness 层的 bash/edit/read/write 也带）。以运行时转储为准 |

**同时纠正一处我自己的错误**：本计划原先把 `powershell` 说成"Pi 有而 nini 没有"，但没说清
**Pi 自己有两套 truncate 实现**（harness 层与 coding 层，语义基本相同）。nini 对齐的是
**coding 层**（产品层，nini 的工具对应的就是它）。

---

## 4. 顺序与理由

| 序 | 项 | 为什么在这个位置 |
|---|---|---|
| 1 | **T1 契约** | 后面每一项都要用同一套"描述与 schema 来自 Pi"的提取机制；先建机制，后改内容 |
| 2 | **T2 ls** | `ls` 的**存在与否**直接改变 prompt 的 guideline 分支（T3 的条件），所以先有工具再对 prompt |
| 3 | **T3 prompt 联动** | 依赖 T2 的工具集；同时它风险最高，单独成批、单独验证 |
| 4 | **T4 截断** | 与 T1–T3 正交，但它会改变长任务的上下文构成 ⇒ 放在最后，单独发版 |

**T1–T3 可在同一批里完成**（都在"契约/引导"这一层，互相依赖）；
**T4 单独一批**（改的是输出体量，不是契约）。

---

## 5. 判据看板与门禁

```console
python3 scripts/tool-parity-check.py           # 全量（含 cargo 的两条）
python3 scripts/tool-parity-check.py --quick   # 只跑不编译的两条
```

| 项 | 判据 | 现状 |
|---|---|---|
| T1 | `pi-tool-contracts.py --compare`：7 个工具逐条一致 | ✗ 7/7 不一致 |
| T2 | `nini-tools` 有 `ls` 且已注册 | ✗ 不存在 |
| T3 | `cargo test -p nini-core --test prompt_tool_linkage` | ✗ 测试不存在 |
| T4 | `cargo test -p nini-tools --test truncation_parity` | ✗ 测试不存在 |

**看板要进 CI**：它把"这一批做完没有"变成一次运行的结果，而不是一句"我觉得好了"。
（与 `pi-target-lock.py` 一样：门禁必须能红才有意义。）

---

## 6. 红线与决策规则

**红线**（违反即停，先问 owner）

1. 不改 `references/` 下任何快照文件（快照是对照物，改它等于改答案）。
2. 不改 `scripts/gate.sh` 与 `scripts/invariants.py` 的判定逻辑。
3. 不引入新的外部依赖（截断、schema 比对都用标准库 + 已有依赖）。
4. 不碰 `references/spec-v0.85.1/`（格式层的锁）。
5. 不在 T1–T3 里顺手实现 `powershell` 或任何 Pi 之外的工具。

**决策规则**（运行中遇到这些，不停下来问，按表执行）

| 场景 | 规则 |
|---|---|
| Pi 的描述在 0.85.1 里是拼出来的（模板 + 变量） | 取**运行时拼装后的结果**为准；无法确定就记进台账，不猜 |
| nini 的某个工具与 Pi 的**语义**天然不同（如 Windows 路径） | 保持 nini 语义，**在 description 里写明差异**，并在 README 记一条限制 |
| 对齐后某个现有测试变红 | 先判断它是不是**把旧行为当成了正确**；是就改测试并记录理由，不是就回滚对齐 |
| 契约对齐需要改公开 API（如 Tool trait） | 只允许**向后兼容**地加；破坏性改动留到架构 Stage 1 |
| 某条判据无法自动化 | 写进台账并注明"机器判定：false"，不许假装它可判定 |

---

## 7. 交付形态

每一批（T1+T2+T3 一批、T4 一批）：

1. 代码 + 测试；
2. `scripts/tool-parity-check.py` 相应行变绿；
3. 台账更新（`scripts/parity-gaps.json` 的 status / oracle / exit_criterion）；
4. CHANGELOG 写清楚"变了什么行为、为什么、凭据是什么"；
5. 一次提交，commit message 说**行为**而不是"改了文件"。

---

## 8. 未决与依赖

| # | 事项 | 状态 |
|---|---|---|
| 1 | `powershell` 工具（Pi 0.85.1 新增，Windows） | **只登记不实现**；需要 owner 决定是否在 Linux 侧也提供 |
| 2 | prompt 组装的**分层**（Pi 在 pi-ai，nini 在 nini-core） | 本轮只对齐内容；搬层属于架构 Stage 1 |
| 3 | 扩展层（WF-EXTENSION） | 需要 owner 决策；见 `parity-with-pi.md` §2.1 与 C4 |
| 4 | conformance fixture 仍是 0.84.2 录的 | 差距 C5；W0 的剩余项 |
| 5 | nini 的 `!cmd` 与 Pi 的 bash mode | 不同功能（B4），不在本计划 |
