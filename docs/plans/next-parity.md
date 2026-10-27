# v0.11 计划：把最靠前的四条 A/B 类差距闭掉（A2b / A4 / B3 / B5）

> 建立：2026-10-26
> 目标：把差距台账里 **10 条 open** 中最靠前的 4 条（A/B 类、P1–P2）闭掉
> 权威：目标是 [`../GOALS.md`](../GOALS.md) 的表述；本文件是执行细则，冲突以 GOALS 为准
> 基准：**Pi 0.85.1**（冻结，`references/TARGET.json`）。行为层的每条引用都是**符号形态**
> （`文件#符号`），且必须能在 `references/pi-0.85.1/` 里解析——**解析不到的引用不是引用**
> 台账：`scripts/parity-gaps.json` 是差距的唯一数据源；本文件不手写计数

> **执行结果（2026-10-27）**：N0、N1、**N5**、N3、N4 已完成（详见 §6）；
> **N2（A4 双队列）在本轮做完**（oracle、变异注入实测与逐条依据见 §2 的 N2 与 §9）。
> 执行过程中**发现并关掉了一条计划外的新差距 A7**（Esc 的中止信号没有消费者）——
> 它比 A4 更靠前，也更严重：它意味着「Esc 中止了这一轮」是一句**没有兑现的话**。

---

## 0. 为什么是这四条

排序规则（GOALS §3 + `parity-with-pi.md` §3）：**A→B 类，同类内 P0→P3，同严重度内按未验证路径数**。
当前 open 的 10 条按此排序：

| # | id | 级别 | 一句话 | 本轮 |
|---|---|---|---|---|
| 1 | **A2b** | P1 | Esc 在压缩期间会中止**整轮**，Pi 只中止**压缩** | ✅ |
| 2 | **A4** | P2 | 只有一条 pending 队列，Pi 分 steering / followUp 两队列 | ✅ |
| 3 | **B3** | P2 | `/reload` 范围远小于 Pi | ✅ |
| 4 | **B5** | P2 | 没有 double-ESC（Pi 用它切到 tree / fork 视图） | ✅ |
| 5 | B4 | P2 | 没有 bash mode | ❌ 见 §8 |
| 6 | C2 / C3 / C4 | P2 | 元问题与工作流，不是功能面 | ❌ 见 §8 |
| 7 | B8 | P3 | 没有 mermaid 渲染 | ❌ 见 §8 |
| 8 | D1 | P3 | 6 个终端/诊断小项 | ❌ 见 §8 |
| — | B1 | P1 | `/clone` 语义 | ❌ **owner 决策 R10 阻塞** |

**A2b 排第一**不是因为它最有用，而是因为它是**唯一一个「按 Esc 丢东西」的缺陷**：
用户在压缩期间按 Esc，nini 会把**整轮**中止掉，而 Pi 只取消压缩、让这一轮继续。
按错键的代价是整轮工作。

---

## 1. Pi 侧的事实（每条都有可解析的符号引用）

| 事实 | 引用 |
|---|---|
| 压缩开始时 Pi **替换**编辑器的 Esc 处理器为「只中止压缩」，压缩结束时换回来 | `interactive-mode.ts#compaction_start` / `interactive-mode.ts#autoCompactionEscapeHandler` |
| Pi 的中止压缩入口 | `agent-session.ts#abortCompaction` |
| Pi 分 **steering** 与 **followUp** 两个队列 | `agent-session.ts#_steeringMessages` / `agent-session.ts#_followUpMessages` / `agent-session.ts#clearQueue` |
| Enter 在流式中断 → **steer**（进当前这一轮） | `interactive-mode.ts#handleSubmit`（`prompt(text,{streamingBehavior:"steer"})`） |
| Alt+Enter → **followUp**（等这一轮结束） | `interactive-mode.ts#handleFollowUp` + `app.message.followUp`（默认键 `alt+enter`） |
| 压缩期间的提交：Enter → steer，Alt+Enter → followUp | `interactive-mode.ts#queueCompactionMessage` |
| double-ESC：编辑器为空时，500 ms 内第二次 Esc → tree / fork | `interactive-mode.ts#lastEscapeTime` + `settings-manager.ts#getDoubleEscapeAction`（默认 `"tree"`） |
| `/reload` 重载的是 settings + 资源装载器 + 键位 | `interactive-mode.ts#handleReloadCommand` → `agent-session.ts#reload` |

**引用必须能解析**：`settings-manager.ts` **目前不在快照里**，所以 `getDoubleEscapeAction`
这条引用现在是「看着像证据、解析不到任何东西」。**N0 先把它补进快照**（Pi 0.85.1 的
`dist/core/settings-manager.js` + map + 由 map 还原的 `.ts`），否则 B5 的判据无法被机器复核。

---

## 2. 工作项

每项的退出判据都是**一条命令 + 一个退出码**；每项都写明**注入了什么、哪个断言会红**
（判据不能是装饰：匹配不到东西的检查永远绿）。

### N0 —— 把 `settings-manager` 钉进快照（前置）

- 改：`references/pi-0.85.1/{dist,src}/core/settings-manager.*` + `SNAPSHOT.lock`；
  `scripts/pi-line-refs.py` 的文档名列表加 `settings-manager`。
- 判据：`python3 scripts/pi-oracle-lock.py --check` 退出 0；`--verify-refs` 退出 0 且
  `drift=0`；`cargo test -p nini-conformance --test citations_resolve` 退出 0。

### N1 —— A2b：Esc 在压缩期间只中止压缩

- Pi：压缩期间 Esc = `session.abortCompaction()`，**这一轮继续**（Pi 连 abort 都不发）。
- nini 现状：`escape_action()` 把 `EscapeTarget::Compacting` 归到 `EscapeAction::AbortTurn`
  → 中止整轮（`runtime.rs`）。
- 改法：
  1. `EscapeAction` 新增 `AbortCompaction`；`escape_action()` 在 `Compacting` 时返回它；
  2. 键处理里 `AbortCompaction` 只置「取消压缩」标志 + 清 `is_compacting`（编辑器立刻可用），
     **不发** abort 信号给这一轮；
  3. 压缩路径（`nini-core` 的压缩驱动）在每个分块边界检查取消标志，取消了就**不写**压缩结果、
     并发 `Compacting` 相位的结束事件（相位必须落下，否则 `is_compacting` 永远为真）。
- oracle：`crates/nini-tui/tests/compaction_escape.rs`
  1. 压缩期间 Esc → `escape_action()` 是 `AbortCompaction`（**不是** `AbortTurn`）；
  2. Esc 之后 `is_compacting` 立刻为 false，且**这一轮没有被中止**（abort 信号没被触发）；
  3. 压缩被取消后**还能再次提交**（不会卡在「压缩中」）；
  4. A3 的既有语义不变：`Agent` / `Retrying` / `BranchSummary` 仍然是 `AbortTurn`。
- 退出判据：`cargo test -p nini-tui --test compaction_escape` 退出 0。
- 变异注入：把 `escape_action()` 的 `Compacting` 分支改回 `AbortTurn` → 第 1 条红；
  取消压缩后**不清** `is_compacting` → 第 3 条红。

### N2 —— A4：steering 与 followUp 两个队列（**本轮已做**）

- Pi：Enter（流式中）→ **steer**（进当前轮）；Alt+Enter → **followUp**（agent 本该停下时续跑）；
  压缩期间同样两分（`interactive-mode.ts#queueCompactionMessage`）。
  `agent-session.ts#clearQueue` 一次清空两者。
- nini 此前：**只有一条** `pending_next_turn_messages`，同时被三个用途争抢：
  steering 钩子（`nini-cli` 的 `LiveConfigHooks::get_steering_messages`）、轮后自动投递、
  Esc 的 `return_queue_to_editor`。谁先谁拿 → **语义不确定**。
- 改法（**本轮已做**）：
  1. `RunState` 拆成 `pending_steer_messages` 与 `pending_followup_messages`；
  2. 路由按 Pi：流式中的 Enter → steer；**新增 `Alt+Enter` 键位**（Pi 的
     `app.message.followUp`，默认键 `alt+enter`）→ followUp；压缩期间同样两分；
  3. **消费各归其位**（逐条见下面的「消费点归位」）；
  4. `return_queue_to_editor`（Esc / Pi 的 `interactive-mode.ts#clearAllQueues`）
     **两个队列一起**退回编辑器，且 **steer 在前**。
- **队列里只装「用户说的话」**（写点归属）：
  - bash 输出（`!cmd` 的结果）**不进队列**——Pi 把它记进 session 的活消息表
    （`agent-session.ts#recordBashResult`），**不是**当用户指令。进了队列的后果有三个：
    Esc 会把它塞回输入框、steering 钩子会把它当用户的话插进当前这一轮、
    而它本来根本不是用户说的。**session 内容不变**：那段输出仍然以一条 User 消息进 session。
  - 分支摘要同理：它是**上下文**，不是用户指令，**不进队列**。
- **Alt+Enter 的四处改动面**（少改一处，键位就在某个路径上不生效）：
  | # | 位置 | 改什么 |
  |---|---|---|
  | 1 | `crates/nini-tui/src/keys.rs` | `KeyAction::FollowUp` 变体 + `default_keymap()` 里 Alt+Enter 的绑定 |
  | 2 | `crates/nini-tui/src/keybindings_manager.rs` | `parse_action` 认 `FollowUp` / `followUp` / `app.message.followUp`（用户照抄 Pi 的键位文件要生效） |
  | 3 | `crates/nini-tui/src/runtime.rs` 的 `handle_key` | 真实按键路径：Running 时入 followUp 队列，Editing 时与 Enter 同路 |
  | 4 | `crates/nini-tui/src/runtime.rs` 的 `apply_action` | 无头派发点：同上（**两个派发点必须一致**） |
- **消费点归位**（每条队列的每个消费者，写清楚在哪儿、什么时候取）：
  | 队列 | 消费者 | 位置 | 何时取 |
  |---|---|---|---|
  | steer | **steering 钩子** | `nini-cli/src/app.rs` 的 `LiveConfigHooks::get_steering_messages` → `nini_tui::runtime::take_steer_messages` | 当前这一轮的**下一次模型调用前**（Pi：`agent-session.ts#steer`） |
  | followUp | **agent 的 followUp 钩子**（不是轮后投递） | `LiveConfigHooks::get_followup_messages` → `take_followup_messages` | agent **本该停下时**，在**同一个 run 内**续跑（Pi：`agent-loop.ts#runAgentLoop`） |
  | 两者 | 轮后自动投递 —— **竞态兜底，不是 followUp 的消费者** | `nini_tui::runtime::submit_queued_turn` | 只接住「回合**收尾之后**才排进来」的消息：先 steer 后 followUp，各取**一条**，作为新一轮发出去 |
  | 两者 | Esc 退回 | `nini_tui::runtime::return_queue_to_editor`（Pi：`interactive-mode.ts#restoreQueuedMessagesToEditor` + `interactive-mode.ts#clearAllQueues`） | Esc：两个队列一起退回编辑器，**steer 在前**（Pi 拼的是 `[...steering, ...followUp]`） |
  | 两者 | 压缩期入队 | `submit_user_input_inner` 的压缩分支（Pi：`interactive-mode.ts#queueCompactionMessage`） | 压缩期间提交：Enter → steer 桶，Alt+Enter → followUp 桶，**不另起一轮** |
- oracle：`crates/nini-tui/tests/queue_routing.rs`（7 条）
  1. 流式中的 Enter → 进 **steer** 队列，不进 followUp；
  2. Alt+Enter → 进 **followUp** 队列，不进 steer；
  3. 压缩期间的 Enter → steer；压缩期间的 followUp 入口 → followUp；**都不另起一轮**；
  4. Esc → **两个队列**的消息都回到编辑器，**steer 在前**、队列清空，**退回条数 = 2**；
  5. `take_steer_messages` / `take_followup_messages` **互不越界**；
  6. `!cmd` 的输出**不进队列**，但**进 session**（一条 User 消息）；
  7. `app.message.followUp` / `FollowUp` → `KeyAction::FollowUp`；默认键 Alt+Enter → followUp，
     而 Enter 仍然是 Submit（两个键不互相吞）。
  另：`crates/nini-cli/src/app.rs` 的 `queue_hook_tests`（2 条）——**钩子各取各的队列**：
  steering 钩子只取 steer（不碰 followUp 队列），followUp 钩子只取 followUp（不碰 steer 队列）。
- 退出判据：`cargo test -p nini-tui --test queue_routing` 退出 0；
  `cargo test -p nini-cli` 退出 0；**既有测试零改动通过**——本轮**没有改动任何既有测试**
  （拆队列不改变任何既有语义，只有新行为被钉住），`cargo test --workspace --all-targets` 退出 0。
- 变异注入（**实测**，每次注入后按字节还原、`git diff` 干净）：
  1. **路由改回「两种 kind 都进 steer 队列」**（要改**两处**：`queue_while_running` 的
     FollowUp 臂 + 压缩分支的 FollowUp 臂；只改一处就只钉住一半）→ **第 2 条红**
     （followUp 队列空、steer 队列多一条）**且第 3 条红**（压缩期的 followUp 桶落进了 steer 桶）；
  2. **`take_steer_messages` 改取 followUp 队列** → **第 5 条红**（取出来的是 "F1" 而不是 "S1"），
     且 `nini-cli` 的 steering 钩子断言**同时红**。

### N5 —— A7：中止信号必须真的有消费者（执行本轮时发现，计划外）

- Pi：Esc → `restoreQueuedMessagesToEditor({ abort: true })` → `this.agent.abort()`，
  中止**真的落到 agent 上**（`agent-session.ts#abort` → `_isAgentRunActive = false`）。
- nini 现状（发现于本轮执行）：`run_state.abort_signal` 是一个 `Arc<Notify>`，
  TUI 在 Esc 时 `notify_waiters()`，而**驱动那一侧根本没人读它**。
  「aborted」只是一个状态字符串，**模型继续往下写**。而且
  `Notify::notify_waiters()` **不留存许可**，先触发后等待的一方永远等不到；
  信号又是在**键处理**里创建的，所以「轮后自动投递」那条路径根本没有信号。
- 改法：
  1. 新类型 `nini_core::abort_signal::AbortSignal`：**标志先落、通知后发**；
     等待方**先注册再看标志**——两个顺序都不丢；
  2. 信号在 `submit_user_input_inner`（每一轮的唯一入口）创建；
  3. 驱动在 `select!` 里等它，收到就 `agent.abort_handle().abort()`；
  4. 两个派发点（`handle_key` / `apply_action`）都触发它。
- oracle：`crates/nini-tui/tests/compaction_escape.rs`（信号相关断言）
  + `crates/nini-core/src/abort_signal.rs` 的 3 条单元测试。
- 退出判据：`cargo test -p nini-tui --test compaction_escape` 退出 0；
  `cargo test -p nini-core abort_signal` 退出 0。
- 变异注入：`trigger()` 不落标志 → 4 条断言变红（实测）。

### N3 —— B5：double-ESC 切到 tree / fork

- Pi：编辑器**为空**时按 Esc 记时间戳；500 ms 内再按一次 → `showTreeSelector()`
  （默认动作）或 `showUserMessageSelector()`（fork）；动作由
  `settings-manager.ts#getDoubleEscapeAction` 决定（`"none" | "tree" | "fork"`，默认 `"tree"`）。
- nini 现状：没有。
- 改法：
  1. `RunState` 加 `last_escape_ms: Option<u64>`（毫秒时间戳，**可注入**——测试不能 sleep 500 ms）；
  2. 键处理：Idle + 编辑器为空 + 距离上次 Esc < 500 ms → 打开 tree 选择器（默认）或 fork 选择器；
     否则记时间戳；
  3. 动作由设置项 `double_escape_action` 决定（默认 `"tree"`，`"none"` 什么都不做）。
- oracle：`crates/nini-tui/tests/double_escape.rs`
  1. 空编辑器 + 两次 Esc（间隔 < 500 ms）→ 打开 **tree** 选择器（`status = "open_selector:tree"`）；
  2. 间隔 > 500 ms → **不**打开（只记时间戳）；
  3. 编辑器**非空**时按两次 Esc → **不**打开（Pi 只在空编辑器时计数）；
  4. 动作设为 `"none"` → 两次 Esc 什么都不做；
  5. 动作设为 `"fork"` → 打开 fork 选择器而不是 tree。
- 退出判据：`cargo test -p nini-tui --test double_escape` 退出 0。
- 变异注入：把窗口从 500 ms 改成 5000 ms → 第 2 条红；去掉「编辑器为空」的条件 → 第 3 条红。

### N4 —— B3：`/reload` 的范围

- Pi：`/reload` → `session.reload()` → **settings + 资源装载器（skills / prompts / themes /
  AGENTS.md）+ 键位 + 运行时重建**，并重建聊天。
- nini 现状：只重载 settings（+ 重算 context window），并**诚实地说**skills 需要重启。
- 改法（**不越权**：不重建会话，不重建系统提示词——nini 的提示词在会话开始时固定）：
  1. settings（已有）；
  2. **键位**：`keybindings.json` 的用户覆盖重新加载（Pi 的 `keybindings.reload()`）；
  3. **模型目录**：`models.json` 重新读一遍（用户声明的模型会变）；
  4. 主题（已有）；
  5. **明确说清不能重载的**：skills / 提示词模板 / AGENTS.md（提示词在本会话固定），
     并给出 Pi 的对应行为作为差异说明——**不谎报**。
- oracle：`crates/nini-tui/tests/reload_scope.rs`
  1. 改了 `settings.json` 再 `/reload` → 新值生效（已有行为，守住）；
  2. 改了 `keybindings.json` 再 `/reload` → 键位覆盖生效；
  3. 改了 `models.json` 再 `/reload` → 用户声明的模型出现在目录里；
  4. `/reload` 的输出**不得**声称重载了 skills / 提示词（不能重载就要说出来）。
- 退出判据：`cargo test -p nini-tui --test reload_scope` 退出 0。
- 变异注入：让 `/reload` 不重载键位 → 第 2 条红；让输出谎称「已重载 skills」→ 第 4 条红。

---

## 3. 看板

```console
$ python3 scripts/next-parity-check.py          # 看板（会跑 cargo）
$ python3 scripts/next-parity-check.py --json   # 机读
```

**每一行 = 一条命令的退出码**。不用「看输出里有没有 ✗」——那是不可靠的解析：
输出格式一变就可能「什么都没匹配上」→ 空列表 → 判定为全绿。

---

## 4. 计划自审：这 15 处漏洞与修法

> 自审是**执行前**做的，不是执行后补的。下面每一条都是这份计划**原本会踩**的坑。

| # | 漏洞 | 后果 | 修法 |
|---|---|---|---|
| 1 | 判据写成「手感一致」「行为等价」 | 不可复算，永远绿 | 每条判据 = 一条命令 + 一个退出码 |
| 2 | 先动手再查 Pi | 照着记忆实现 Pi 的行为 | §1 的符号引用**先**落地，引用解析不了就不许动手 |
| 3 | `settings-manager.ts` 不在快照 | B5 的引用无法被机器复核 | N0 前置：先补进快照 + 记锁 |
| 4 | 只加队列字段，不改消费路径 | **死队列**：消息进去没人取 | 判据覆盖「谁取哪个队列」（N2 第 5 条） |
| 5 | Esc 语义用 if-else 加分支 | 破坏 A3 建立的「单一真相」 | 沿用 `escape_action()` 一个决策函数；判据第 4 条守住 A3 |
| 6 | 取消压缩后不清 `is_compacting` | 之后每次 Enter 都被收进队列，**永远不出** | 判据第 3 条：取消后还能再次提交 |
| 7 | double-ESC 用 wall clock 且不可注入 | 测试要么 sleep 500 ms，要么不稳定 | 时间戳存进 `RunState`，测试直接写值 |
| 8 | double-ESC 抢走普通 Esc | 非空编辑器按两下就跳走 tree | 判据第 3 条：非空时不触发 |
| 9 | `/reload` 扩大范围后谎报 | 「重载了 skills」而实际上没有 | 判据第 4 条：不能重载的必须说出来 |
| 10 | 队列拆分时漏改一处消费点 | 消息在某个路径上消失 | 判据覆盖**每个**消费点（steer 钩子 / 轮后投递 / Esc 退回） |
| 11 | 计划里的新差距 id 没进台账 | `--check-plan` 变红 | 本轮涉及的 id 在台账里改 `status`，并在 `parity-with-pi.md` 里点名 |
| 12 | 判据是「跑得通」 | 断言写错也绿 | 每项写明**注入了什么、哪个断言会红** |
| 13 | Pi 升级后引用漂移 | 语义悄悄变 | 引用符号化 + 快照锁 + `citations_resolve`（Rust 侧的第二实现） |
| 14 | 一轮里塞 4 个差距 | 中途卡住 → 半成品 | 每项**独立可验证**：做完一项就提交；卡住就停在已完成的那一项 |
| 15 | 动到 print 模式 / 无 TTY 横幅 | 回归到别的路径 | 四项全部只在 TUI 层；`cargo test --workspace` 全量绿为准 |
| 16 | **执行时才发���的缺陷**（自审漏了） | 「Esc 中止了这一轮」是一句**没有兑现的话**：中止信号**没有任何消费者**，模型继续写 | 执行中当场发现、当场关掉（新增差距 **A7**），并给它自己的 oracle（`compaction_escape.rs` 里的信号断言）——**计划漏掉的东西，执行时看见就得记进台账** |
| 17 | 既有测试变红时直接改测试 | 把旧行为又当成「正确」写死 | 按纪律先判断：这是「把旧行为当成了正确」吗？是则改测试**并写下理由**（`escape_target.rs` 的头部记着 A2b 的旧差与新理由） |
| 18 | 键位文件按文档写却不生效 | 用户改键位**毫无反应**，且无从判断 | 查到最后发现是**两个解析器**方向相反；统一成一个，并把「运行时读的格式 == 文档的格式」写成 oracle |

**自审的结论**：本计划**没有**需要停下来问 owner 的地方（不涉及范围扩张、不改判定逻辑、
不动 spec、不碰快照内容——只**增加**快照文件并记锁）。4 条差距里没有一条被 owner 决策阻塞
（B1 被 R10 阻塞，**不在本轮**）。

---

## 5. 红线

1. 不改 `references/spec-v0.85.1/`（格式层的锁）。
2. 不改 `scripts/gate.sh` 与 `scripts/invariants.py` 的**判定逻辑**（可以新增脚本与判据）。
3. 不改 `references/pi-0.85.1/` 里**已有**文件的任何字节（只允许**新增**文件并记锁）。
4. 不引入新的外部依赖。
5. 不在 N1–N4 里顺手实现 bash mode / mermaid / 扩展层（那是 §8 的事）。
6. 不谎报：做不到的事要说做不到（`/reload` 的 skills 就是做不到）。

## 6. 决策规则

| 场景 | 规则 |
|---|---|
| Pi 的行为在快照里读不出来 | 记进台账，标 `blocked-external`，**不猜** |
| nini 的架构做不到 Pi 的某个行为 | 保持 nini 语义，**在输出里写明差异**，并记一条限制 |
| 对齐后某个既有测试变红 | 先判断它是不是「把旧行为当成了正确」；是就改测试并记录理由，不是就回滚对齐 |
| 契约对齐需要改公开 API | 只允许**向后兼容**地加；破坏性改动留到架构 Stage 1 |
| 某条判据无法自动化 | 写进台账并注明「机器判定：false」，不许假装它可判定 |

## 7. 交付形态

每一项（N1…N4）单独一次提交：

1. 代码 + 测试；
2. `scripts/next-parity-check.py` 相应行变绿；
3. 台账更新（`scripts/parity-gaps.json` 的 `status` / `oracle` / `exit_criterion`）；
4. CHANGELOG 写清「变了什么行为、为什么、凭据是什么」；
5. commit message 说**行为**，不���「改了文件」。

## 8. 明确不做（以及为什么）

| 差距 | 为什么不在本轮 |
|---|---|
| **B4** bash mode | Pi 的 `isBashMode` 是**独立交互形态**（边框变色、空格转命令、Esc 退出该形态），需要先实测 Pi 的语义再设计 nini 的形态；混进本轮会让 4 项都做不扎实。**下一轮单独做**，且必须先录 Pi 的行为。 |
| **B8** mermaid | 渲染是**另一个产品面**（Pi 用外部渲染器 + 终端图像），不是「补一个能力」；需要 owner 决策「要不要带渲染依赖」。 |
| **C2** 计划文档与代码失真 | 是**元问题**：本轮做完，台账与计划的差距表会再对一次账（`--check-plan`），能顺带收一半；剩下的需要把状态字段指向代码符号，属另一件事。 |
| **C3** 投入单侧化 | 是**工作流问题**（`nini-ai` 长期无人推进），实现者不自行决定范围。 |
| **C4** 扩展层 | 已被 GOALS §4 记为「需要 owner 决策」。 |
| **D1** 终端/诊断小项 | 6 个独立小项（`updateTerminalTitle` 等），适合**一批一次**做完；本轮不塞。 |
| **B1** `/clone` | 被 **R10** 阻塞（owner 决策）：改成「切到新分支」会改变用户现有行为。 |

---

## 9. 执行结果（2026-10-27）

| 工作项 | 结果 | 凭据 |
|---|---|---|
| **N0** 对照物进快照 | ✅ | `settings-manager.{js,map,ts}` 已进 `references/pi-0.85.1/`（422 文件）；引用 138 条全部可解析（drift=0） |
| **N1** A2b：压缩期 Esc 只取消压缩 | ✅ | `crates/nini-tui/tests/compaction_escape.rs` 9 条；agent 区分「取消」与「无物可压」 |
| **N5**（计划外）A7：中止信号真的有消费者 | ✅ | `AbortSignal`（标志先落、通知后发）；驱动在 `select!` 里等它并 `agent.abort_handle().abort()` |
| **N3** B5：double-ESC | ✅ | `crates/nini-tui/tests/double_escape.rs` 7 条；`"fork"` 明确说没实现 |
| **N4** B3：`/reload` 的范围 | ✅ | `crates/nini-tui/tests/reload_scope.rs` 4 条；键位与 models.json 会重读；skills 说得更准 |
| **N2** A4：双队列 | ✅ | `crates/nini-tui/tests/queue_routing.rs` 7 条 + `crates/nini-cli/src/app.rs` 的 `queue_hook_tests` 2 条；路由按 Pi 分 steer / followUp（含压缩期两分、Esc 退回 steer 在前、bash 输出不进队列）；变异注入实测：路由改回「都进 steer」→ 第 2、3 条红；`take_steer_messages` 改取 followUp 队列 → 第 5 条红（`nini-cli` 的 steering 钩子断言同时红） |

**顺带修掉的两处真缺陷**（都在 N4 触及的代码里）：

1. **键位文件按文档的格式写，一行也不生效**——运行时用的是另一个解析器，
   而且方向相反（把 JSON 的键当按键规格、值当动作名）。现在只有一个解析器。
2. **键位路径绕过了 `NINI_AGENT_DIR`**——它自己拼 `$HOME/.nini/agent/…`，
   于是 agent 目录在别处时，键位读的是**另一个**目录里的那一份。

看板：`python3 scripts/next-parity-check.py`。

**台账收尾一条（不属于 v0.11 的工作项）**：**D3**（Pi 的可点击链接 OSC 8）在本轮**关闭**。
它的退出判据有两个分支：① ratatui 支持 hyperlink——**被上游证据排除**（ratatui 0.29 / 0.30.2、
ratatui-core 0.1.2、crossterm 0.28 / 0.29 全部 0 命中；且 ratatui 的 cell 缓冲区会丢弃
含控制字符的 grapheme、差分重绘只重画变化的 cell，承载不了有状态的 OSC 8 转义对）；
② README 的 known limitations 里明说「链接不可点击」——**已满足**（`README.md` 与
`README.zh-CN.md` 各一条）。oracle：`crates/nini-tui/tests/hyperlink_limitation_documented.rs`
（4 条；措辞改掉即红，实测过）。
