# 对标 Pi 的功能实现 —— 优化计划

> 建立：2026-10-18（**v2，已自我审查修正**）
> 目标：把「nini 对标 Pi 的功能实现」从**名字层面齐了**推进到**语义与行为层面可证齐**
> 权威：目标的表述在 [`../GOALS.md`](../GOALS.md)；本文件是执行细则。冲突以 GOALS 为准。
> 基准：格式层 `references/spec-v0.85.1/`（Pi v0.85.1 `d981de12`，19 文件带 sha256）；
> 行为层**本机实装 Pi 0.84.2**（`~/.nvm/.../@earendil-works/pi-coding-agent`，可读源码、可运行）。
> 依据：2026-10-18 逐条核对，nini 1429 测试全绿（debug + release 两 profile）。

> **v2 修订摘要**：v1 的三条核心论断经复核后**推翻了三条**，另修正五处不精确。
> 逐条见 [§9 已知不确定性](#9-已知不确定性v1-审查)。**这本身就是本计划要解决的第一类问题的证据**：
> 手工核对的结论不可靠，只有脚本与 fixture 可靠。

---

## 0. 结论摘要

**1. 命令名齐了；参数提示差 6 处，其中 2 处的提示是假的。**
脚本比对（括号配平解析 `registry.rs` 的 28 条注册）发现 **6 处 `argument_hint` 与 `builtin.json` 不一致**：
`/export` `/import` `/name` `/clone` `/logout` `/resume` —— nini 声明了 Pi 不接受的参数。
其中 **2 处实现根本不用这个参数**：

| 命令 | nini 提示 | 实现签名 | 事实 |
|---|---|---|---|
| `/export` | `[path]` | `fn cmd_export(..., _args: &str)` (`dispatch.rs:291`) | **参数被忽略**。Pi 的描述是 "specify path: .html/.jsonl"，nini 只能导出到固定目录 |
| `/clone` | `<session-id>` | `fn cmd_clone(..., _args: &str)` (`dispatch.rs:727`) | **参数被忽略**，且整体语义与 Pi 相反（见 B1） |

用户看到 `/clone <session-id>` 会以为可以指定会话，实际上打了参数也没反应。
这是 T4（诚实性）类缺陷，不是功能缺失。

**2. 交互语义层有一个已实测的数据正确性缺陷。**
`RunMode` 同时表达「编辑器可否输入」与「是否有 agent 在跑」，后者是唯一的 spawn 守卫。
v0.10.1 修「ESC 后四秒不能输入」时把 `mode` 在 Esc 的瞬间置为 `Editing` —— 编辑器可用了，
守卫同时消失。实测 `spawns=2 live=2`（`tests/esc_concurrency_probe.rs`）。
Pi 结构上到不了这一步，链路已逐行核实：

```js
// interactive-mode.js:2492 —— 提交路径唯一的分叉
if (this.session.isStreaming) { await this.session.prompt(text, { streamingBehavior: "steer" }); return; }
// agent-session.js:588  isStreaming = _isAgentRunActive
// agent-session.js:328  _isAgentRunActive = false  ← 只在 _emitAgentSettled() 里，含 post-run continuation
```

Esc 只调 `agent.abort()`（`:1574`），**不动 `_isAgentRunActive`**；因此 abort 发出到 agent 真正结束之间，
Pi 的提交仍然 steer 进当前 turn，永远不会开第二个 run。nini 的 `mode` 把这两件事合成了一件。

**3. 风险不是"没有 oracle"，是 oracle 不可执行对照。**
v1 说「行为层 oracle = 0」，**这是错的**：实测 `nini-conformance` 共 **66 条**（GOALS 里的 45 也是旧数），
其中 agent loop 行为 12 条（`loop_semantics` 4 / `stop_after_turn` 3 / `prepare_next_turn` 3 /
`tool_batch_semantics` 2）**直接引用 Pi `agent-loop.js` 的行号**；`nini-tui/tests/` 里另有若干条
引用 `interactive-mode.js:2491/3505`、`keybindings.ts`。

准确的论断是：**这些 oracle 的形式是「注释里引 Pi 行号 + 断言 nini 的行为」，
没有 Pi 侧录制的可执行对照物。** A1 恰好落在这个形式的盲区——
它引用了 Pi 的 `restoreQueuedMessagesToEditor`，却从未断言"Pi 在同一窗口不会 spawn 第二个"。

> 主张：W0 要做的不是"从零建 oracle"，是给已有的行号式 oracle **补上可执行的一侧**。
> 否则每轮修复都靠重新读 Pi 源码，而 v1 的自我审查已经证明读三遍会得出三个不同结论。

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
| **A1** | Esc 后可并发 spawn 第二个 agent。后果**不是文件损坏**（`O_APPEND` 单行写入是原子的），而是：两个 turn 的消息在 session 里**顺序交错**、`TokenStats` 混算、transcript 事件乱序 | `interactive-mode.js:2492` 用 `isStreaming` 分叉；`agent-session.js:588,328`：`isStreaming = _isAgentRunActive`，只在 `_emitAgentSettled()` 置 false（含 post-run continuation）；Esc 只 `agent.abort()`（`:1574`） | `runtime.rs` Esc 分支置 `Editing`，而 `submit_user_input` 的守卫正是 `mode != Editing` | ✅ 已实测（`esc_concurrency_probe.rs`，`#[ignore]` 记录缺陷） | **P0** |
| **A2** | 压缩期间没有语义：提交不会排队，Esc 也不认识压缩 | `interactive-mode.js` 的 `autoCompactionEscapeHandler?` / `queueCompactionMessage(text, mode)`；`prompt()` 在压缩中**抛错**（`agent-session.js:807`） | `is_compacting` **全仓库无赋值点**（只初始化 `false`），于是 `runtime.rs:2138` 的排队分支是**不可达死代码** | ❌ | **P0** |
| **A3** | Esc 是单一 if-else 链，没有可替换的上下文 handler | 字段 `autoCompactionEscapeHandler?` / `retryEscapeHandler?`：Pi 在进入压缩/重试前**替换** Esc 处理器 | `Abort` 分支按 completion→search→pending_quit→Running→else 顺序判 | ❌ | P1 |
| **A4** | 单队列；Pi 分 `steering` / `followUp` | `restoreQueuedMessagesToEditor` 解构 `{steering, followUp}`（`:3506`） | `pending_next_turn_messages` 一条 | ❌ | P2（依赖 A2） |

### B 类 —— 功能缺失

| # | 差距 | Pi 基准 | nini 现状 | 严重度 |
|---|---|---|---|---|
| **B1** | **`/clone` 语义反了，且宣称了一个不存在的参数** | `fork(leafId,{position:"at"})` + 切到新 session（`:4163`）；`argumentHint = null` | 复制 JSONL，原 session 继续编辑（`dispatch.rs:727`）；hint 却写 `Some("<session-id>")`，而签名是 `_args` | ❌ | **P1**（用户以为切了分支、实际没有；提示的参数也没用） |
| **B1b** | **`/export` 忽略路径参数** | 描述 "specify path: .html/.jsonl" | hint 写 `[path]`，签名是 `_args`（`dispatch.rs:291`），只能导出到固定目录 | ❌ | **P1**（T4：提示与实现不符） |
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
| **C1** | **oracle 无可执行的 Pi 侧对照** | `nini-conformance` 实测 **66 条**（GOALS 记的 45 已旧），agent loop 行为 12 条引 `agent-loop.js` 行号；`nini-tui/tests/` 另有引用 `interactive-mode.js`、`keybindings.ts` 的若干条——**全部是「注释引行号 + 断言 nini」** | 行号会随 Pi 升级漂移且无人核对；"Pi 在这个窗口不会 X"这类**跨状态断言**无处落脚。A1 正属此类 |
| **C1b** | **手工核对不可靠**（本计划自身的教训） | v1 的"ARG 差异 0 处"是解析器 bug 导致的假结论，真值 6 处；v1 的"行为层 0 条"是 grep 没匹配 `#[tokio::test(flavor=...)]`，真值 66 | 任何「我读过了」都不构成证据；只有脚本与 fixture 算 |
| **C2** | 计划文档与代码失真 | `input_interrupt_parity.md` §8.8 称「M2 未做」，而 `return_queue_to_editor` 早已实现 | 下一轮照文档排期，会重做已完成的事 |
| **C3** | 投入单侧化 | `nini-ai` 自 10-12 零提交，同期 `nini-tui` +11861 行 | provider/成本层是 parity 的硬指标，却无人推进 |

### D 类 —— 小项（做的时候顺手，不单独立项）

Pi 有、nini 无：`updateTerminalTitle`、`onRightClickPaste`、`checkTmuxKeyboardSetup`、
`addCacheMissNotice`、`checkForPackageUpdates`、`formatDiagnostics`。

---

## 3. 排序规则

沿用 GOALS §3 现行规则（**按未验证路径数**排序），并追加两条硬约束：

1. **A 类不与 B 类并行**——A1 的修法会改 TUI 状态机与提交路径，B2/B4/B5 改同一条路径，
   并行会互相掩护回归。故顺序是 **W0 → A 类（W1-A/B/C）→ B 类（W2…）**，A 类不排到 B 类之后。
2. **高风险项单独成批**——A1（纯状态字段）与 A2（压缩全链路）混在一批，
   回归时无法判断是哪一处引起的，故拆为 W1-A / W1-B。

**规则本身的可复算要求**（v1 的教训，见 §9）：
§2 每一条都要在 `scripts/` 下有可枚举记录（阶段、严重度、oracle 位置、状态），
排序按该记录而非记忆；**计数类指标一律脚本产出**——v1 的两次手工计数（"45 条"、"ARG 差异 0 处"）
都是错的。

---

## 4. 阶段

每阶段有**退出判据**（可判定）。没有退出判据的工作不排期——v0.10 已经证明过它的下场。

### W0 —— 给行号式 oracle 补上可执行的一侧（0.5 天，先于一切）

**为什么先做**：§2 里 6 条 ARG 差异、2 条假参数提示，全部是**手工读代码**发现的；
而本计划的自我审查又证明了手工读会出错（§9）。不先把"可执行的对照"建起来，
后面每一轮修复的验收都只能靠"再读一遍 Pi 源码"。

**先决检查（半天内必须出结论）**：本机 Pi 0.84.2 能否在**无网络 / 无 API key** 条件下
触发下列场景？能 → 录 fixture；不能 → 走降级路径（判据 c）。

**做什么**：
1. 对已有的 **12 条 agent loop 行号 oracle + TUI 侧引 `interactive-mode.js` / `keybindings.ts` 的条目**，
   逐条列「Pi 行号断言」与「可执行断言」两张表；缺口即 W0 产出。
2. 能离线触发的，录成 fixture 落进 `crates/nini-conformance/fixtures/pi-0.84.2-*.jsonl`。
3. 不能触发的，写脚本把 Pi 对应源码行号**快照下来**与 nini 的引用比对；
   Pi 一升级该脚本就红。这比注释里的行号强，因为它是自动的。

**退出判据**（三选一，不允许含糊）：
- **(a) 可录制**：≥4 个场景录成 fixture，且 A1 场景存在。
- **(b) 部分**：能录的录，不能录的至少进快照脚本。
- **(c) 不可录制**：Pi 无法离线触发 → **W0 标为「外部条件阻塞」，转而先做 W2**（不依赖 Pi 侧对照）。

**A1 的 fixture 在修好前必须是红的**：判据是「它存在且当前为红」，修好后转绿。
v1 把"全绿"和"为红"写进同一条判据，是自相矛盾的判据，已废弃。

### W1-A —— A1：中断与 spawn 解耦（P0，1 天，低风险）

**只做 A1。** A2/A3/A4 不在本批。

**修法**（可行性已对照 Pi 的 settled 时机核过）：
- 新增 `run_state.agent_alive: bool`，spawn 时置 `true`，`Done` 事件时置 `false`；
- spawn 守卫从 `mode != Editing` 改为 `!agent_alive`（等价于 Pi 的 `_isAgentRunActive`）；
- `mode` 回归本职：编辑器可否接受输入（Esc 立即置 `Editing`，保留 v0.10.1 的行为）。
- 时机已核：nini 的 `Done` 在 `app.rs:559`（agent 事件流结束后）发出，对应 Pi 的
  `_emitAgentSettled()`。Pi 的该函数覆盖 post-run continuation，**nini 的 `Agent::run`
  是否含等价阶段需在实现时再确认一次**——这是本修法唯一的未验证前提。

**退出判据**：
1. `esc_concurrency_probe.rs` **去掉 `#[ignore]` 且绿**；
2. v0.10.1 的既有修复不回退：`interrupt_editing_e2e.rs` / `input_parity_m0.rs` 全绿；
3. tmux 实测：长回复中 Esc 后立刻 Enter，transcript 无交错、session 消息顺序单调。

### W1-B —— A2 + A3：压缩异步化与 ESC handler 栈（P0→P1，3 天，高风险）

**单独成批**：A2 自评高风险（compaction 全链路 v0.10 刚动过），不与 A1 互相掩护。

- A2：压缩改异步，`is_compacting` 在压缩开始时置 `true`；压缩中提交的行为
  **必须照 Pi 实际行为定**（`agent-session.js:807` 是直接抛错，另有 `queueCompactionMessage`
  两条路径），**不能自选**。
- A3：`enum EscapeTarget { Idle, Agent, Compacting, Retry, BranchSummary }`，
  对齐 Pi 的 handler 替换写法。

**退出判据**：① A2 那个不可达分支变为可达且有 oracle；② 压缩中 / 重试中 / 分支摘要中
按 Esc 的三种行为各有 1 条 oracle；③ 现有 compaction 测试零改动通过（回归网）。

### W1-C —— A4：双队列拆分（P2，0.5 天）

**依赖 A2**。A2 未做时 steering/followUp 语义无意义，**不做就是加一个空结构**。

**退出判据**：`clearAllQueues` 的两个队列各有 1 条 oracle，ESC 还回时两者顺序与 Pi 一致。

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

| 指标 | 怎么算 | 本轮基线（2026-10-18 实测） |
|---|---|---|
| conformance 测试总数 | 脚本数 `#[test]` / `#[tokio::test(...)]` | **66**（v1 写 45、GOALS 也写 45，均已旧） |
| 其中**引 Pi 源码行号**的 | 文件内出现 `agent-loop.js` / `interactive-mode.js` / `agent-session.*` | agent loop 12 条（loop_semantics 4 / stop_after_turn 3 / prepare_next_turn 3 / tool_batch_semantics 2）+ TUI 侧若干 |
| 其中**有 Pi 侧可执行对照** | fixture 由真实 Pi 产出 | **0** ← 这才是真问题 |
| `argument_hint` 差异 | 脚本括号配平解析 `registry.rs` vs `builtin.json` | **6**（`/export` `/import` `/name` `/clone` `/logout` `/resume`） |
| 提示与实现不符 | 声明了 hint 但 cmd_* 签名为 `_args` | **2**（`/export` `/clone`） |
| A 类未闭 | §2 A 表 | 4 |
| B 类未闭 | §2 B 表（不含 B9，owner 决策） | 10 |
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

---

## 9. 已知不确定性（v1 审查）

本节记录 v1 被推翻的论断与仍未闭合的证据链。**保留它们比删掉更有用**：
这正是本计划 §4 W0 要解决的那类问题——读代码得出的结论会漂。

### 9.1 已推翻（v2 已改）

| # | v1 的说法 | 实测 | 为什么会错 |
|---|---|---|---|
| 1 | 「行为层 oracle = 0」 | **66 条**，其中 agent loop 12 条直接引 Pi 行号 | grep 用了 `^\s*#\[test\]`，漏掉 `#[tokio::test(flavor = "current_thread")]`。**手工数测试条数不可靠** |
| 2 | 「参数提示逐条一致，ARG 差异 0 处」 | **6 处**差异 | 正则 `Some\("…"\)` 对跨行格式匹配失败，且 `re.split` 前瞻在含括号实参处截断。**解析器必须括号配平** |
| 3 | 「A1 会损坏 session JSONL」 | `O_APPEND` 单行写入是原子的，文件不会损坏 | 真实后果是**消息顺序交错 + TokenStats 混算 + 事件乱序**。夸大后果会让 P0 排序失真 |

### 9.2 复核后确认为正确（不必重查）

| 断言 | 证据链 |
|---|---|
| A1 的因果链成立 | `interactive-mode.js:2492` `if (session.isStreaming) → prompt(text,{streamingBehavior:"steer"})`；`agent-session.js:588` `isStreaming = _isAgentRunActive`；`:328` 只在 `_emitAgentSettled()` 置 false；Esc 只 `agent.abort()`（`:1574`）不动它 |
| A1 修法方向可行 | nini 的 `Done` 在 `app.rs:559`（agent 事件流结束后）发出，对应 Pi 的 settled |
| B1 `/clone` 语义偏差成立 | Pi `fork(leafId,{position:"at"})` + 切换（`:4163`）vs nini 复制文件（`dispatch.rs:727`） |
| B1b `/export` 忽略参数 | `fn cmd_export(..., _args: &str)`（`dispatch.rs:291`） |
| 队列不会丢消息 | `runtime.rs:2156` 下次提交时 `mem::take` 队列并推入 transcript |

### 9.3 仍未闭合（必须在实现时验证，不得假设）

| # | 缺口 | 何时验证 |
|---|---|---|
| 1 | nini 的 `Agent::run` 是否含 Pi 的 post-run continuation（决定 `agent_alive=false` 的时机是否精确对齐） | W1-A 实现时 |
| 2 | 压缩中提交，Pi 到底是抛错还是排队（`agent-session.js:807` 抛错 vs `queueCompactionMessage` 排队，两条路径都存在） | W1-B 动手前，先录 Pi 行为 |
| 3 | 本机 Pi 能否**离线**触发中断 / 压缩场景（决定 W0 走 a/b/c 哪条） | W0 先决检查 |
| 4 | `/trust` 的选择器 `selectors/trust.rs` 是否真的没被命令打开（B2 的前提） | W2 动手前 |
| 5 | synctact 引入后的真实体积增量（B7 的 R9 决策依据，目前只是"通常 +2–5 MB"的估计） | W4 |

### 9.4 方法论结论

**「我读过了」不构成证据。** 本计划三条核心论断里有三条是手工核对的，三条都出了问题，
其中两条还是往"看起来更好"的方向错的（夸大了后果、掩盖了差异）。
W0 的全部意义就是把这件事变成脚本。
