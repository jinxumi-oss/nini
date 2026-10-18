# 对标 Pi 的功能实现 —— 优化计划

> 建立：2026-10-18
> 目标：把「nini 对标 Pi 的功能实现」从**名字层面齐了**推进到**语义与行为层面可证齐**
> 权威：目标的表述在 [`../GOALS.md`](../GOALS.md)；本文件是执行细则。冲突以 GOALS 为准。
> 基准：格式层 `references/spec-v0.85.1/`（Pi v0.85.1 `d981de12`，19 文件带 sha256）；
> 行为层**本机实装 Pi 0.84.2**（`~/.nvm/.../@earendil-works/pi-coding-agent`，可读源码、可运行）。
> 依据：2026-10-18 逐条核对，nini 1429 测试全绿（debug + release 两 profile）。

---

## 0. 结论摘要

三句话，每句都有可复算的证据：

**1. 命令面齐了，语义面没齐。**
`registry.rs` 注册 28 条 = Pi 23 + nini 5，**名字与参数提示逐条一致**（脚本比对：ARG 差异 0 处）。
但逐条读实现后发现 6 条命令的**行为语义**与 Pi 不同，其中 `/clone` 是反的：Pi 把它
`fork(leafId, {position:"at"})` 并**切换**到新 session（`interactive-mode.js:4163`），
nini 复制一份 JSONL 让它当"副产物"，用户接着编辑的还是原 session（`dispatch.rs:727`）。

**2. 交互语义层有一个已实测的数据正确性缺陷。**
`RunMode` 同时表达「编辑器可否输入」与「是否有 agent 在跑」，后者是唯一的 spawn 守卫。
v0.10.1 修「ESC 后四秒不能输入」时把 `mode` 在 Esc 的瞬间置为 `Editing` —— 编辑器可用了，
守卫同时消失。实测 `spawns=2 live=2`（`tests/esc_concurrency_probe.rs`）。
Pi 结构上到不了这一步：`isIdle = !_isAgentRunActive`（`agent-session.js:592`）是 agent 的属性，
与编辑器解耦，Esc 只调 `agent.abort()` 不动它。

**3. 最大的风险不是缺功能，是无法证明已对齐。**
45 条 conformance oracle 全部落在**格式层**（session JSONL / settings / skills / compaction / slash 清单）。
行为层（agent loop、TUI 交互）**oracle 数为 0**。这意味着本计划里每一条"已实现"的声明，
都只有实现者的记忆做支撑——第 1 节里那 6 条语义偏差，就是这么漏出来的。

> 主张：本计划的第一件事不是补功能，是**把行为层 oracle 建起来**（W0）。
> 否则补完 B 类还会漏出新的 A 类，就像上一轮 6 项 P0 全是"实现了但算错了"。

---

## 1. 基准与分层（不可混用）

| 层 | 对照物 | 可验证方式 |
|---|---|---|
| 格式 | `references/spec-v0.85.1/`（v0.85.1） | sha256 逐文件校验（本轮已验：19/19 一致） |
| 行为 | **本机 Pi 0.84.2**（`pi --version` = 0.84.2） | 读 `dist/**/*.js` 源码 + 录制 fixture |

**硬规则**：行为层的"Pi 是怎么做的"必须给 `文件:行号`，且**只能引 0.84.2**。
`SPEC.lock.compatClaim` 写的是 v0.85.1 —— 两者不是矛盾，是分层（T1）。
把 0.84.2 的行号"修正"成 0.85.1 等于删证据。

---

## 2. 差距总表

每条：`Pi 基准` / `nini 现状` / `有 oracle?` / `严重度`。
**严重度判据**：数据损坏 > 数据丢失 > 功能不可用 > 体验差异 > 文案漂移。

### A 类 —— 结构耦合，会损坏或丢失数据

| # | 差距 | Pi 基准 | nini 现状 | oracle | 严重度 |
|---|---|---|---|---|---|
| **A1** | Esc 后可并发 spawn 第二个 agent；两个 turn 共写一个 session 文件与一份 TokenStats | `agent-session.js:592` `isIdle = !_isAgentRunActive`；流式期间提交走 `streamingBehavior:"steer"` | `runtime.rs` Esc 分支置 `Editing`，`submit_user_input` 的守卫正是 `mode != Editing` | ✅ 已实测（`esc_concurrency_probe.rs`，`#[ignore]` 记录缺陷） | **P0** |
| **A2** | 压缩期间没有语义：提交不会排队，Esc 也不认识压缩 | `interactive-mode.js` 的 `autoCompactionEscapeHandler?` / `queueCompactionMessage(text, mode)`；`prompt()` 在压缩中**抛错**（`agent-session.js:807`） | `is_compacting` **全仓库无赋值点**（只初始化 `false`），于是 `runtime.rs:2138` 的排队分支是**不可达死代码** | ❌ | **P0** |
| **A3** | Esc 是单一 if-else 链，没有可替换的上下文 handler | 字段 `autoCompactionEscapeHandler?` / `retryEscapeHandler?`：Pi 在进入压缩/重试前**替换** Esc 处理器 | `Abort` 分支按 completion→search→pending_quit→Running→else 顺序判 | ❌ | P1 |
| **A4** | 单队列；Pi 分 `steering` / `followUp` | `restoreQueuedMessagesToEditor` 解构 `{steering, followUp}`（`:3506`） | `pending_next_turn_messages` 一条 | ❌ | P2（依赖 A2） |

### B 类 —— 功能缺失

| # | 差距 | Pi 基准 | nini 现状 | 严重度 |
|---|---|---|---|---|
| **B1** | **`/clone` 语义反了** | `fork(leafId,{position:"at"})` + 切到新 session（`:4163`） | 复制 JSONL，原 session 继续编辑（`dispatch.rs:727`） | **P1**（用户以为切了分支，实际没有） |
| **B2** | `/trust` 交互形态不同 | `/trust` → `showTrustSelector()`（`:2404`） | 命令行参数 `[trusted\|distrust\|ask\|list\|clear]`；`selectors/trust.rs` 存在却没被命令打开 | P2 |
| **B3** | `/reload` 范围小 | "keybindings, extensions, skills, prompts, themes, and context files" | "extensions + skills + settings"（`dispatch.rs:1341`） | P2 |
| **B4** | bash mode | `isBashMode` + `bashComponent` + `flushPendingBashComponents`；pi-tui 有 `ProcessTerminal` | 完全没有（`!cmd` 是内联 shell，**不是同一功能**） | P2 |
| **B5** | double-ESC | `lastEscapeTime` + `getDoubleEscapeAction()` → `tree`/`fork`/`none` | 无 | P2 |
| **B6** | 模型选择器搜索排序 | `model-search.js` `getModelSelectorSearchText`：精确 `provider/model` 前缀优先于 `openrouter/openai/gpt-5` 形式 | `selectors/model.rs` 77 行，无排序策略 | P3 |
| **B7** | 语法高亮 | `highlightCode`；9 个 `syntax*` 槽**读不到** | README 已申报为限制 | P3（需评估 synctact 的体积代价） |
| **B8** | mermaid 渲染 | `mermaidMarkdownTransformer`（markdown 变换器） | 无 | P3 |
| **B9** | provider 数 | 51（`provider_factory.rs` 原生 5） | 5 原生 + `models.json` | **R5，owner 决策**（见 §7） |

### C 类 —— 元问题（决定上面两类会不会再漏）

| # | 问题 | 证据 | 后果 |
|---|---|---|---|
| **C1** | **行为层 oracle = 0** | 45 条 conformance 全在格式层 | 「已对齐」不可证；本计划 §2 里 6 条语义偏差全部是这样漏的 |
| **C2** | 计划文档与代码失真 | `input_interrupt_parity.md` §8.8 称「M2 未做」，而 `return_queue_to_editor` 早已实现 | 下一轮照文档排期，会重做已完成的事 |
| **C3** | 投入单侧化 | `nini-ai` 自 10-12 零提交，同期 `nini-tui` +11861 行 | provider/成本层是 parity 的硬指标，却无人推进 |

### D 类 —— 小项（做的时候顺手，不单独立项）

Pi 有、nini 无：`updateTerminalTitle`、`onRightClickPaste`、`checkTmuxKeyboardSetup`、
`addCacheMissNotice`、`checkForPackageUpdates`、`formatDiagnostics`。

---

## 3. 排序规则

沿用 GOALS §3 现行规则（**按未验证路径数**排序，可枚举、脚本可复算），
并追加一条硬约束：**A 类不与 B 类并行**。理由——A1 的修法（拆分状态事实）会改动
TUI 状态机与提交路径，B 类的 B2/B4/B5 都改同一条路径；并行会互相掩护回归。

因此顺序是：**W0 → A 类 → B 类 → B 类剩余**，A 类不排期到 B 类之后。

---

## 4. 阶段

每阶段有**退出判据**（可判定）。没有退出判据的工作不排期——v0.10 已经证明过它的下场。

### W0 —— 行为层 oracle 基线（0.5 天，先于一切）

**为什么先做**：B7 那类差异是靠"逐条读实现"发现的，不是靠测试。读实现不可复算，
下一个人会重读一遍、并且可能读漏。

**做什么**：
1. 在本机 Pi 0.84.2 上录制 6 段 fixture（**真实 Pi 输出，不得由 nini 生成**）：
   - 中断时的 transcript 状态：空队列 / 单 steering / steering+followUp 各一份
   - Esc 之后立刻提交的**最终状态**（这是 A1 的 oracle：Pi 只会 steer，永远不会有两个 turn）
   - 压缩进行中提交 → Pi 抛错或排队，二选一必须录到真实行为
   - `/clone` 前后的 session 列表（Pi 有新分支且已切换）
2. 落进 `crates/nini-conformance/fixtures/pi-0.84.2-*.jsonl`，
   配套一个 runner：把同一场景喂给 nini，比对结果。

**退出判据**：nini 与 Pi 0.84.2 在这 6 个场景上的差异条目数为 0；
A1 的 fixture 存在且**当前为红**（红是预期的，它记录缺陷）。

**风险**：Pi 的行为要靠构造才能触发（需要真实 provider 或 fixture provider）。
若 Pi 侧无法自动化录制，退化为**逐条 `文件:行号` 断言**（把 Pi 源码当 oracle），
但必须写进测试注释，不允许只写在本文件里。

### W1 —— A 类：中断模型解耦（P0，2–3 天）

**A1 的最小修法**（不推翻 v0.10.1 的修复）：
- 新增一个与 `RunMode` 并列的事实，例如 `run_state.agent_alive: bool`，
  由 `AgentEventLite::Done` 置 `false`、提交 spawn 时置 `true`；
- spawn 守卫从 `mode != Editing` 改为 `!agent_alive`（**即 Pi 的 `_isAgentRunActive`**）；
- `mode` 回归它原本的职责：编辑器可否接受输入（Esc 立即置 `Editing` 保留）。

**A2 的修法**：压缩改为异步（Pi 的 `_compactionAbortController`），`is_compacting` 在
压缩开始时置 `true`；压缩中提交走队列（现在那个分支才可达）；压缩中 Esc 走 A3 的 handler。

**A3 的修法**：引入 `enum EscapeTarget { Idle, Agent, Compacting, Retry, BranchSummary }`，
`Abort` 分支按它分派——对齐 Pi 的 handler 替换写法。

**A4 的修法**：A2 落地后 `steering`/`followUp` 才有意义，否则不做（避免"两队列一个用途"的
空结构）。

**退出判据**：
1. `esc_concurrency_probe.rs` **去掉 `#[ignore]` 且绿**；
2. W0 的 6 段 fixture 全绿；
3. tmux 实测：长回复中 Esc 后立刻 Enter，transcript 无交错、session JSONL 消息顺序单调。

**风险**：A2 会动 compaction 全链路（v0.10 的 R2a/A2/A3 刚动过）。
**建议**：A1 单独一轮（纯状态字段，低风险）；A2+A3 一轮，且**先补 compaction 集成测试**。

### W2 —— B 类命令语义与交互形态（P1–P2，2 天）

B1 `/clone` 改为 fork-at-leaf 并切换；B2 `/trust` 打开已有的 `selectors/trust.rs`；
B3 `/reload` 补 keybindings/themes；B6 模型搜索排序；D 类小项。

**退出判据**：§2 的 B 表逐条勾掉，每条**至少一条 oracle**（能红的，不是跑得通的）；
命令面板与 `/help` 的描述文本与 `builtin.json` 逐条一致（脚本可复算，纳入 CI）。

### W3 —— B4 bash mode + B5 double-ESC（P2，1–2 天）

动手前必须先确认 Pi 的 `isBashMode` 语义（GOALS M4 已警告：**不要把它当成 `!cmd`**）。
`double-ESC` 直接对齐 `getDoubleEscapeAction()`，500ms 窗口。

**退出判据**：两条 oracle；且 `!cmd` 的既有行为不被改动（回归网）。

### W4 —— B7 语法高亮 + B8 mermaid（P3，先决策再动手）

B7 需要 synctact 或自研 tokenizer。**体积是硬约束**：nini 的卖点是 6.7 MB 单文件，
引入 synctact 通常 +2–5 MB。**这是产品取舍，需 owner 决策**（见 §7），不要自行引入。

**退出判据**：先出体积对比数字，再决定做不做；做了则 9 个 `syntax*` 槽至少 1 条 oracle。

### W5 —— C2/C3 治理（0.5 天）

C2：把 `input_interrupt_parity.md` §8.8 改成实际状态（M2 已做、M2b/M3/M4 未做），并在
GOALS 台账登记本文件。**规则**：任何计划文档的状态字段，必须能在代码里指出对应符号。

C3：不在代码层解决，属排期问题——但记录：**R5 未决期间 `nini-ai` 的零改动本身就是信号**，
不要用 TUI 的产出掩盖它。

---

## 5. 全局验证口径（每轮结束时必须复算）

| 指标 | 怎么算 | 本轮基线 |
|---|---|---|
| 行为层 oracle 条数 | `nini-conformance` 中以 Pi **行为**为断言的条数 | **0** |
| 格式层 oracle 条数 | 同上，格式为断言 | 45 |
| A 类未闭条数 | §2 A 表中未勾条数 | 4 |
| B 类未闭条数 | §2 B 表中未勾条数（B9 不计，owner 决策） | 8 |
| `/help` 与 `builtin.json` 文本差异 | 脚本比对 | 23 条命令中 14 条描述措辞不同 |
| 实测缺陷 | `#[ignore]` + 可复现指令 | 1（`esc_concurrency_probe.rs`） |

**单调性要求**：A 类每轮只减不增；行为层 oracle 数每轮只增不減。
任一指标反向，说明这一轮引入了新的不确定性——**这比没修更糟**。

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
| R5 | provider 是否补齐到 51 个 | 待决 | `nini-ai` 的方向；B9 |
| **R9（新）** | 语法高亮是否值得 +2–5 MB 体积 | 待决 | B7（synctact vs 自研 tokenizer） |
| **R10（新）** | `/clone` 改为"切到新分支"是否可接受（用户当前行为会变） | 待决 | B1 |

---

## 8. 与既有计划的关系

| 文档 | 处置 |
|---|---|
| 本文件 | **active**：功能面 parity 的唯一执行细则 |
| `plans/input_interrupt_parity.md` | 本文件 W1 的展开；§8.8 状态字段已按实际修正（C2） |
| `plans/v0.10-pi-parity-refactor.md` | R1–R8 历史路线，**只读**；其 §31b 构建身份已落地 |
| `plans/r2a-compaction-persistence.md` | W1/A2 的前置依赖 |
| `plans/v0.6*`–`v0.9.2*`、`tui-*.md` | frozen，不再追加 |
