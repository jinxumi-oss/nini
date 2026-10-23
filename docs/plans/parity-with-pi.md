# 对标 Pi 的功能实现 —— 优化计划 v3

> 建立：2026-10-18（v2）→ **v3：2026-10-23（重写，修 v2 的 13 处漏洞）**
> 目标：把「nini 对标 Pi 的功能实现」从**名字层面齐了**推进到**语义与行为层面可证齐**
> 权威：目标的表述在 [`../GOALS.md`](../GOALS.md)；本文件是执行细则。冲突以 GOALS 为准。
> 基准：格式层 `references/spec-v0.85.1/`（Pi v0.85.1 `d981de12`，19 文件带 sha256）；
> 行为层 **Pi 0.85.1**，源码快照锁在 `references/pi-0.85.1/`（411 文件 + `SNAPSHOT.lock` sha256）。
> 机器台账：**`scripts/parity-gaps.json`** 是差距的唯一数据源，本文件不再手工维护计数。

---

## 0. v3 为什么重写

v2（2026-10-18）的自我审查推翻了 v1 的三条论断，这本身是对的。但**v2 自己也带了 13 处漏洞**，
其中 5 处会直接让执行走偏。逐条修法见 [§10 修订记录](#10-v3-修订记录13-处漏洞的修法映射)。

v3 的三条硬变化：

1. **差距表不再写在文档里。** v2 的 §2 是 Markdown 表格，§5 的「A 类未闭 4 / B 类未闭 10 / 实测缺陷 1」
   是手工计数——而 v2 §9.4 刚刚判过「手工计数不可靠」的死刑。v3 把差距搬进
   `scripts/parity-gaps.json`，由 `scripts/parity-gap.py --json` 产出**所有**统计，
   文档只引用脚本输出，不复述数字。
2. **行为层对照物上锁，且引用改成符号形态。** v2 的 oracle 全部指向「本机 Pi」的
   `dist/*.js` 行号——实测 0.84.2 → 0.85.1 之间，90 处引用只有 11 处仍指向同一行，
   而**没有任何东西会红**。v3 把机制面源码快照进 `references/pi-0.85.1/`（411 文件 sha256），
   并把引用改成 `文件#符号`：含义被钉住，行号由脚本解析，符号消失即红灯。
3. **每条判据必须机器可判定。** v2 的 W2 判据含「逐条勾掉」、W1-A 含「tmux 实测」这类
   无法复算的项；v3 的每个 DoD 都写成**一条命令 + 一个退出码**。

---

## 1. 基准与分层（不可混用）

| 层 | 对照物 | 可验证方式 | 锁在哪 |
|---|---|---|---|
| 格式 | Pi v0.85.1（19 文件） | sha256 逐文件校验 | `references/spec-v0.85.1/SPEC.lock` |
| 行为 | **Pi v0.85.1**（411 个机制面文件） | 快照内解析 + 快照 sha256 | `references/pi-0.85.1/SNAPSHOT.lock` |

**一个目标版本**（2026-10-23 统一）：格式层与行为层同版本，不再分叉。
此前行为层只能对 0.84.2（本机装得下的那个），代价是
**90 处行号引用里只有 11 处在 0.85.1 里仍指向同一行**，且没有任何东西会红
（证据：`docs/pi-version-delta-0.84.2-to-0.85.1.md`）。

**硬规则**

1. 行为层的「Pi 是怎么做的」用**符号引用**：`文件#符号`，例如
   `agent-loop.ts#shouldStopAfterTurn`。含义被钉住，行号由脚本在目标版本里解析。
   `scripts/pi-oracle-lock.py --verify-refs` 逐条解析：**符号消失即退出 1**。
2. 行号引用（`agent-loop.ts:225`）只作为过渡保留，且必须仍能解析到**代码行**
   （不是注释行——注释会随代码移动，行为不会）。
3. **快照是行为层的唯一真相。** Pi 升级 → 快照失效 → `--check` 红 → 必须重录并重审 oracle。
   别名表只有一份（`scripts/pi_refs.py`）：0.85.1 里 `keybindings.ts` **同名不同物**
   （pi-tui 的 TUI 键位 vs coding-agent 的 app 键位），抄三份就会重演「同一份失真三份副本」。

> 已知的一处语义漂移（v3 记录在案）：`crates/nini-core/src/agent.rs` 中若干处
> `agent-loop.js:NNN` 实际写的是 `.ts` 的行号（npm 包不发 `.ts`，作者当时对着还原出的
> TS 源码抄行号）。快照已含 `.ts` 还原版（来自 `.js.map` 的 `sourcesContent`），
> v3 的 W0 负责把这些引用改写成 `.ts:` 形式并逐条复核。

---

## 2. 差距总表

**本节不列内容。** 差距的唯一数据源是 `scripts/parity-gaps.json`；
渲染与统计由 `scripts/parity-gap.py` 负责，校验由 `scripts/parity-gap.py --check-plan` 负责。

```console
$ python3 scripts/parity-gap.py --json      # 机读：总数、按类/状态/批次分布、逐条
$ python3 scripts/parity-gap.py              # 人读表格
$ python3 scripts/parity-gap.py --check-plan docs/plans/parity-with-pi.md   # 文档与台账对账
```

**当前规模**：以 `python3 scripts/parity-gap.py --json` 的输出为准（分类分布、状态分布、
机器可判定占比、被哪个 owner 决策阻塞的条数）。本文件**不手写这些数字**——
`scripts/plan-selfcheck.py` 的 S9 会因为文档里出现手写计数而失败。
需要看数字就跑脚本，需要看趋势就比 `docs/parity-snapshots/` 里的历史 JSON。

**分类口径（v3 修正）**

| 类 | 含义 | 处置 |
|---|---|---|
| A | 结构耦合，会损坏/丢失/错序数据 | 必须先于 B 类；每轮只减不增 |
| B | 功能缺失或语义不符 | 按批次推进 |
| C | 元问题（决定 A/B 会不会再漏） | W0/W5 |
| D | 小项 | 做的时候顺手，不单独立项 |

**与 v2 的差异**：v2 的 C 表有 C1/C1b/C2/C3 四行，v3 把 C1b（手工核对不可靠）并入 C1
（它本来就是 C1 的证据），并新增 **B10 = `/share` 仍是 stub**——v2 的 B 表里没有它，
而 README 承认它是唯一一条返回「尚未实现」的命令。**parity 声明不能把「命令名齐」当「语义齐」。**

### 2.1 工具的两层：原始自带 vs 扩展（**两件不同的工程**）

**这一节是 v3 新增的**，因为把两者混在一张待办表里是本项目反复犯的错
（10-05 的「缺 `ls` 作废」就是混层查错了目录）。

#### Pi 自己的划分（权威措辞来自 `dist/core/sdk.d.ts:25-47`）

```ts
noTools?: "all" | "builtin";
//  - "all": start with no tools enabled
//  - "builtin": disable the default built-in tools (read, bash, edit, write)
//    but keep extension/custom tools enabled
tools?: string[];        // "Otherwise it enables the default built-in tools
                         //  (read, bash, edit, write). Extension/custom tools
                         //  remain enabled unless noTools changes that default."
customTools?: ToolDefinition[];   // "in addition to built-in tools"
```

| 层 | Pi 的名字 | 内容（0.85.1） | 归属 | 性质 |
|---|---|---|---|---|
| **原始自带 · harness 层** | built-in | read, write, edit, bash | `pi-agent-core/dist/harness/tools/` | **函数**：进程内实现 + schema |
| **原始自带 · coding 层** | built-in | grep, find, ls（+ 0.85.1 的 powershell） | `pi-coding-agent/dist/core/tools/` | **函数** |
| **扩展层** | extension / custom | **Pi 默认一个都不带**（唯一内置扩展 llama.cpp 是隐藏的，且不注册工具） | 扩展运行时 `dist/core/extensions/` | **插件** |

Pi 的三个工厂函数把边界写死：`createCodingTools()` = [read, bash, edit, write]
（= Pi 说的 built-in 四件套）；`createReadOnlyTools()` = [read, grep, find, ls]；
`createAllTools()` = 全部。

#### 四条判据（区分两层的**可判定**依据）

| 判据 | 原始自带工具 | 扩展工具 |
|---|---|---|
| **谁发布** | 随二进制发布，升级即变 | 第三方插件，独立版本 |
| **谁进 system prompt** | **自动进**（`Available tools` 段），并且工具集决定 guideline（`system-prompt.js:53-60` 的 `hasBash/hasGrep/hasFind/hasLs/hasRead`） | **默认不进**——除非扩展自己提供 `promptSnippet` / `promptGuidelines` |
| **谁渲染** | 统一由 TUI 渲染（Pi 的 `ToolExecutionComponent`） | **可以自带** `renderCall` / `renderResult` / `renderShell` |
| **可否被关掉** | `tools` 白名单 / `excludeTools` / `noTools: "builtin"` | 默认不受 `noTools: "builtin"` 影响（只有 `noTools: "all"` 才关） |

#### nini 两侧的现状

| | 原始自带（T1） | 扩展（T2） |
|---|---|---|
| 工具集 | `nini-tools` 有 read/write/edit/bash/grep/find —— **缺 `ls`**（B11）；契约抄自 0.84.2，**未按 0.85.1 重捕获**（B13） | `nini-ext` 的 `register_tool(Arc<dyn Tool>)` —— **只能塞进一个函数式工具** |
| prompt 归属 | ✅ 已有 `Tool::system_prompt_contribution()`（snippet + guidelines，组装确定性排序） | ✅ 同上（同一个 trait） |
| 渲染归属 | 统一由 `nini-tui` 渲染 | ❌ **无按工具注册渲染器的入口**（`rich.rs` 按名字通用渲染） |
| 执行期反馈 | 无流式更新回调 | ❌ 无 `onUpdate`、无 `signal`（`ToolContext` 只有 cwd/两个预算） |

**结论**：nini 已经有「扩展工具」的**一半契约**（prompt 贡献），缺另一半
（渲染钩子、流式更新、signal）。所以「扩展运行时不可用」这句话不准确——
准确的说法是：**nini 的扩展只能贡献「一个函数式工具 + 一段 prompt」，
不能贡献「一个带自己外观与流式进度的工具」**。

#### 两条**独立**的工作流（不要混排期）

| 工作流 | 范围 | 退出判据 |
|---|---|---|
| **WF-BUILTIN** | 八个内置工具的**契约**（描述 + 参数 schema，按 0.85.1 重捕获）与**语义**（截断、超时、并发、变更队列） | `references/pi-0.85.1/tool-contracts.json` 与 nini 逐条对齐；语义差异各有 oracle |
| **WF-EXTENSION** | 扩展运行时：渲染钩子、流式更新、signal、受限采样 | 每项能力各有 1 条 oracle；在此之前 **README 明确声明「扩展工具不在 parity 范围内」** |

**WF-BUILTIN 是「高效、准确完成任务」的载体；WF-EXTENSION 是「生态」，成本高一个数量级。**

**v3 这一轮又新增了三条**（都是执行过程中实测发现的，不在 v2 的表里）：
**A5**（TUI 启动后不创建 session 文件，普通会话不落盘）、**A6**（中断后排队的消息
不会被自动投递）、**D2**（无 TTY 时的横幅硬编码 `nini v0.4.0`）。详见台账。

---

## 3. 排序规则（与 GOALS 合并后的唯一版本）

v2 的排序规则（A→B）与 GOALS §3 的现行规则（按未验证路径数排序）**互相冲突**，
而 GOALS 明文规定「冲突本身就是需要修的缺陷」。v3 合并为：

1. **功能面按 A→B 排**（A 类结构缺陷优先，因为它会损坏数据）。
2. **同一类内按严重度 P0→P3**；同严重度内按**未验证路径数**降序（GOALS §3 的原规则，保留）。
3. **被 owner 决策阻塞的条目单列**（`blocked-owner`），不参与排序，因为它们的工作量由外部输入决定。
4. **被外部条件阻塞的条目单列**（`blocked-external`），并写明降级路径。
5. **每轮开工前必须写下退出判据（可判定 = 一条命令 + 一个退出码）**（GOALS §3 硬性要求）。

> 排序的输入是 `scripts/parity-gap.py --json` 的 `by_round` / `by_severity` / `machine_checkable`，
> 不是记忆。**任何一轮结束都必须重跑该脚本，并把前后两个 JSON 存进 `docs/parity-snapshots/`**——
> 「A 类每轮只减不增、oracle 数每轮只增」这条单调性要求，需要有历史快照才能被检查。

---

## 4. 阶段

每个阶段的退出判据都是**可执行的布尔判定**。

### W0 —— 把「行号式 oracle」变成有锁的、可枚举的台账

**为什么先做**：v2 的 6 处参数提示差异、2 处假参数，全部是手工读代码发现的；
v2 的自我审查又证明了手工读会出错（§9）。v3 的 W0 已经有实物交付（见下），
剩下的工作是**迁移存量**与**补 Pi 侧可执行对照**。

**已交付（v3）**

| 产物 | 作用 | 验证命令 |
|---|---|---|
| `scripts/parity-gaps.json` | 差距的唯一数据源，12 个必填字段 | `python3 scripts/parity-gap.py --json` |
| `scripts/parity-gap.py` | 台账校验 + 机读统计 + 与本文件对账 | `python3 scripts/parity-gap.py --check-plan docs/plans/parity-with-pi.md` |
| `scripts/pi-line-refs.py` | 全量行号引用清单（数量以 `scripts/pi-line-refs.py --json` 的输出为准） | `python3 scripts/pi-line-refs.py --json` |
| `references/pi-0.85.1/` + `SNAPSHOT.lock` | Pi 0.85.1 行为层源码快照（411 文件 sha256） | `python3 scripts/pi-oracle-lock.py --check` |
| `scripts/pi-oracle-lock.py` | 快照校验 + 引用逐条解析 | `python3 scripts/pi-oracle-lock.py --verify-refs` |

**未交付（W0 剩余，退出判据如下）**

1. **把 `.js` 行号引用统一为 `.ts` 行号**（npm 包不发 `.ts`，作者当年对着还原源码抄行号）。
   退出判据：`python3 scripts/pi-oracle-lock.py --verify-refs` 退出 0 且 `drift=0`。
2. **补 Pi 侧可执行对照**：用 `scripts/mock-provider.py` 起本地 OpenAI-compat 端点，
   临时把 Pi 的 `models.json` 指过去，录制 A1（中断）与 A2（压缩）两个场景的 fixture。
   退出判据：`crates/nini-conformance/fixtures/pi-0.84.2-*.jsonl` 新增 ≥2 个文件，
   且 `python3 scripts/parity-gap.py --json` 中 A1/A2 的 `oracle` 字段不再是 `none`。
3. **录不到怎么办**：如果 mock provider 也无法触发（Pi 侧不可离线），**不得跳过 W0**，
   也不得降级为「手工核对」——按 §3 规则 4 标 `blocked-external`，并把该条差距的
   `exit_criterion` 改写为「快照行号校验通过 + 人工复核记录进 `docs/invariant-verification.md`」。
   **注意：这条分支不允许顺延 A1 的修复到 B 类之后**（v2 漏洞 1）。

### W1-A —— A1：中断与 spawn 解耦（P0）✅ **v3 已完成**

- 修法：`RunState` 新增 `agent_alive: bool`（与 `mode` 解耦），spawn 时置 true，
  终止路径（`Done` 事件 + watchdog）置 false；`mode` 回归「编辑器可否接受输入」本职。
- 附加：watchdog 用 `run_seq` 单调序号保证「迟到的 reap 不会解除新一轮的守卫」。
- 退出判据：
  1. `cargo test -p nini-tui --test esc_concurrency_probe` 退出 0（**不带 `--ignored`**，5 条）；
  2. `cargo test -p nini-tui` 退出 0（361 unit + 全部集成，0 failed）；
  3. 变异验证两条均红：删掉 `Done` 里的 `agent_alive = false` → `the_done_event_itself_clears_the_guard` 红；
     拆掉 watchdog 的复位 → `a_driver_that_never_says_done_still_reopens_the_editor` 红。

### W1-B —— A2 + A3：压缩异步化与 ESC handler 栈（P0→P1，高风险）

- A2：`is_compacting` 必须有赋值点；压缩期间的提交行为**以 Pi 实测为准**
  （`agent-session.js` 的抛错路径与 `queueCompactionMessage` 路径并存，**必须先录 Pi 行为再定**）。
- A3：引入 `enum EscapeTarget { Idle, Agent, Compacting, Retry, BranchSummary }`，对齐 Pi 的 handler 替换。
- **与 W1-A 的关系**：两者都改 Esc 路径与运行态。v2 把它们分批的理由（避免互相掩护回归）
  在 v3 仍然成立，但**必须同批落地、同一轮验证**，否则「哪一处引起的回归」不可判定。
- 退出判据：
  1. `runtime.rs` 里那个「压缩期间提交进队列」的分支**从不可达变为可达**，且有 1 条 oracle；
  2. 压缩中/重试中/分支摘要中按 Esc 的三种行为各有 1 条 oracle；
  3. 现有 compaction 测试零改动通过；
  4. `python3 scripts/parity-gap.py --json` 中 A2/A3 的 `status` 变为 `fixed`。

### W1-C —— A4：双队列拆分（P2）

依赖 A2。退出判据：`clearAllQueues` 的两个队列各有 1 条 oracle，ESC 还回时两者顺序与 Pi 一致。

### W2 —— B 类命令语义与交互形态（P1–P2）

B1 `/clone`、B1b `/export`、B2 `/trust`、B3 `/reload`、B6 模型搜索排序、B10 `/share`、D 类小项。
- 退出判据：
  1. `scripts/parity-gaps.json` 中 W2 的每条 `status` 变为 `fixed`；
  2. 每条**至少一条会红的 oracle**（不是「跑得通」）；
  3. `argument_hint` 与 `references/spec-v0.85.1/slash-commands/builtin.json` 逐条一致
     （可脚本复算，纳入 CI）。

### W3 —— B4 bash mode + B5 double-ESC（P2）

动手前必须先确认 Pi 的 `isBashMode` 语义（**不要把它当成 `!cmd`**）。
退出判据：两条 oracle；且 `!cmd` 的既有行为不被改动。

### W4 —— B7 语法高亮 + B8 mermaid（P3，先决策再动手）

B7 需 owner 决策（R9：体积 +2–5 MB vs nini 的单文件卖点）。
退出判据：**若 owner 决定不做**，该条的关闭条件是「已明确拒绝并写进 README known limitations
+ 有 1 条 oracle 断言该拒绝行为」；若做，则 9 个 `syntax*` 槽至少 1 条 oracle。

### W5 —— C2/C3 治理

C2：任何计划文档的状态字段必须能在代码里指出对应符号。
C3：provider/成本层是排期问题，记录在案，不在代码层解决。

---

## 5. 门禁与验证口径

### 5.1 交付门禁

`scripts/gate.sh` 是唯一判定脚本，锁两条基线：**裸奔不变量数不得增加**、**测试数不得减少**。
本轮新增 `fix:` 提交会改变不变量总数与测试数，因此：

> **基线刷新规则**：先确认 `invariants_none` 未增加、测试数未减少，再用
> `bash scripts/gate.sh --update-baseline` 显式刷新，并把刷新前后的 JSON 记入
> `docs/parity-snapshots/`。**禁止修改 `scripts/gate.sh` 与 `scripts/invariants.py` 的判定逻辑**
> （它们是「门禁本身要可信」的一部分）。

### 5.2 每轮必须复算的口径（全部由脚本产出）

| 指标 | 命令 |
|---|---|
| 差距总数 / 按类 / 按状态 / 按批次 | `python3 scripts/parity-gap.py --json` |
| 机器可判定判据占比 | 同上（`machine_checkable` / `not_machine_checkable`） |
| 行号引用总数与分布 | `python3 scripts/pi-line-refs.py --json` |
| 快照一致性 + 行号解析 | `python3 scripts/pi-oracle-lock.py --check --verify-refs` |
| 裸奔不变量数 | `python3 scripts/invariants.py --json` |
| 测试数 | `cargo test --workspace --all-targets` |

**单调性要求**：A 类每轮只减不增；`machine_checkable` 每轮只增不减。
任一指标反向，说明这一轮引入了新的不确定性——**这比没修更糟**。
反向发生时必须在本文件 §9 记录，不得静默。

---

## 6. 明确不做（范围决策已定，不要重开）

| 项 | 理由 |
|---|---|
| 扩展运行时 | 已被证伪：Pi 加载 TS，`nini-ext` 是 `dlopen` cdylib，无共同执行模型 |
| OAuth / 设备流 | 产品决策；对应 API key 登录可另议 |
| 命令行 session 参数 | `--continue-session` 等已显式拒绝并指向 TUI 命令 |
| LLM 分支摘要 | 明确不做 |
| prompt 模板 / AGENTS.md | 明确拒绝并说明，不是静默忽略 |

---

## 7. 决策台账（owner 拍板，实现者不自行决定）

| # | 决策 | 现状 | 阻塞什么 |
|---|---|---|---|
| R4 | nini 是否自建凭据存储 | 待决（`6abadd9` 已整体回滚一次） | 整个 auth 层；`/login` 的 Pi 形态 |
| R5 | provider 是否补齐到 Pi 全集 | 待决。**v3 实测：Pi 0.84.2 内置 40 个，nini 原生 9 个实现（7 个品牌）**，v2 文档写的「51 / 5」两个数都是错的 | `nini-ai` 的方向；B9 |
| R9 | 语法高亮是否值得 +2–5 MB 体积 | 待决 | B7 |
| R10 | `/clone` 改为「切到新分支」是否可接受 | 待决 | B1 |
| — | clean-room 表述 vs 读 Pi 源码的方法论 | **待决（v3 新增）** | 见 §9 未闭合项 3 |

---

## 8. 与既有计划的关系

| 文档 | 处置 |
|---|---|
| 本文件 | **active**：功能面 parity 的唯一执行细则（v3） |
| `plans/input_interrupt_parity.md` | 本文件 W1 的展开；§8.8 状态字段已按实际修正（C2） |
| `plans/v0.10-pi-parity-refactor.md` | R1–R8 历史路线，**只读** |
| `plans/r2a-compaction-persistence.md` | W1/A2 的前置依赖 |
| `plans/v0.6*`–`v0.9.2*`、`tui-*.md` | frozen，不再追加 |

---

## 9. 已知不确定性（v3 更新）

**保留它们比删掉更有用**：这正是 W0 要解决的那类问题——读代码得出的结论会漂。

### 9.1 已被推翻（不再重开）

| # | 旧说法 | 实测 |
|---|---|---|
| 1 | 行为层 oracle = 0 | 66 条（`#[test]` + `#[tokio::test(...)]` 都要数） |
| 2 | 参数提示逐条一致，ARG 差异 0 处 | 6 处差异 |
| 3 | A1 会损坏 session JSONL | `O_APPEND` 单行写入原子，真实后果是**顺序交错 + TokenStats 混算 + 事件乱序** |
| 4 | Pi 有 51 个 provider、nini 有 5 个 | Pi 0.84.2 **40 个**、nini **9 个实现**；v3 已按实测写 |
| 5 | nini 侧 `cmd_export` 在 291 行、`cmd_clone` 在 727 行 | 实为 **300 / 805**；文档行号会漂，必须能被机器查出 |

### 9.2 仍未闭合（实现时必须验证，不得假设）

| # | 缺口 | 何时验证 |
|---|---|---|
| 1 | Pi 压缩期提交是抛错还是排队（两条路径都存在） | W1-B 动手前先录 Pi 行为 |
| 2 | 全部行号引用中，有多少是**语义漂移**（行号在范围内但内容已不是那件事） | W0 剩余项 1：`--verify-refs` 打印逐条内容供人工复核 |
| 3 | README 的「clean-room 重写（照着 Pi 的可观察行为和文件格式写，**不看 Pi 的源码**）」与本计划的方法论（读 Pi 源码并引用行号、录真 Pi fixture）**互相矛盾** | **owner 决策**：改表述，还是改方法论；实现者不自行选 |
| 4 | synctact 引入后的真实体积增量 | W4（R9 决策依据） |

### 9.3 方法论结论（不变）

**「我读过了」不构成证据。** v3 把差距、计数、行号、快照哈希全部交给脚本；
人工只保留一件事：**把脚本解析出来的内容与自己的理解对上**——并把核对结果写进
`docs/invariant-verification.md`。

---

## 10. v3 修订记录（13 处漏洞的修法映射）

| # | 漏洞（v2） | v3 修法 | 落地位置 |
|---|---|---|---|
| 1 | W0 的 (c) 分支允许把 P0 降级到 B 类之后 | (c) 分支明令**不得顺延 A1**；改为 `blocked-external` + 快照校验降级路径 | §4 W0 未交付 3 |
| 2 | A2 判据依赖未闭合前提 | W1-B 判据第 4 条要求先产出 Pi 行为记录 | §4 W1-B |
| 3 | v1 已废弃的「fixture 为红」写法复活 | 判据拆成「Pi 侧 fixture 存在」+「nini 侧断言当前为红」两件事 | §4 W0 未交付 2 |
| 4 | §3 自定的台账脚本不存在，全部指标手工计数 | `scripts/parity-gaps.json` + `parity-gap.py` 交付，§2/§5 只引用脚本输出 | §2、§5.2 |
| 5 | 与 GOALS 的排序规则冲突、两个「唯一列表」 | §3 合并为唯一版本；GOALS 同步改写 | §3 + `docs/GOALS.md` |
| 6 | W1-A / W1-B 改同一状态机却分批 | 改为**同批落地、同一轮验证** | §4 W1-B |
| 7 | 「Pi 能否离线触发」被当二元问题 | 给出可行解：`scripts/mock-provider.py` + 临时 `models.json` 指向本地端点 | §4 W0 未交付 2 |
| 8 | 判据含不可复算的人工项（tmux 实测） | 人工项降级为辅助确认；主判据改为脚本判定 | §4 W1-A/W2 |
| 9 | W2 判据对 owner 阻塞项不可达 | 补「owner 决定不做」的关闭条件 | §4 W4 |
| 10 | 计划与 `gate.sh` 的交互缺失 | §5.1 写明基线刷新规则与禁令 | §5.1 |
| 11 | oracle 迁移成本低估（v2 只数了 `.js` 那一半） | 数字按实测改写，并给出 W0 剩余项的逐条清单 | §4 W0、§9.2 |
| 12 | 行为层对照物无版本锁 | `references/pi-0.85.1/` + `SNAPSHOT.lock` + `pi-oracle-lock.py` | §1、§4 W0 |
| 13 | `/share` stub 不在 B 表里；parity 声明把「名字齐」当「语义齐」 | 新增 B10；分类口径明写 | §2 |
