# v0.11 计划：把最靠前的四条 A/B 类差距闭掉（A2b / A4 / B3 / B5）

> 建立：2026-10-26
> 目标：把差距台账里 **10 条 open** 中最靠前的 4 条（A/B 类、P1–P2）闭掉
> 权威：目标是 [`../GOALS.md`](../GOALS.md) 的表述；本文件是执行细则，冲突以 GOALS 为准
> 基准：**Pi 0.85.1**（冻结，`references/TARGET.json`）。行为层的每条引用都是**符号形态**
> （`文件#符号`），且必须能在 `references/pi-0.85.1/` 里解析——**解析不到的引用不是引用**
> 台账：`scripts/parity-gaps.json` 是差距的唯一数据源；本文件不手写计数

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

### N2 —— A4：steering 与 followUp 两个队列

- Pi：Enter（流式中）→ **steer**（进当前轮）；Alt+Enter → **followUp**（下一轮）；
  压缩期间同样两分。`clearQueue()` 一次清空两者。
- nini 现状：**只有一条** `pending_next_turn_messages`，同时被三个用途争抢：
  steering 钩子（`nini-cli` 的 `LiveConfigHooks::get_steering_messages`）、轮后自动投递、
  Esc 的 `return_queue_to_editor`。谁先谁拿 → **语义不确定**。
- 改法：
  1. `RunState` 拆成 `pending_steer_messages` 与 `pending_followup_messages`；
  2. 路由按 Pi：流式中的 Enter → steer；**新增 `Alt+Enter` 键位**（Pi 的
     `app.message.followUp`，默认键 `alt+enter`）→ followUp；压缩期间同样两分；
  3. 消费各归其位：steer 队列由 steering 钩子取（进当前轮），followUp 队列由轮后自动投递取；
  4. `return_queue_to_editor`（Esc / Pi 的 `clearAllQueues`）**两个队列一起**退回编辑器。
- oracle：`crates/nini-tui/tests/queue_routing.rs`
  1. 流式中的 Enter → 进 **steer** 队列，不进 followUp；
  2. Alt+Enter → 进 **followUp** 队列，不进 steer；
  3. 压缩期间的 Enter → steer；压缩期间的 Alt+Enter → followUp；
  4. Esc（`return_queue_to_editor`）→ **两个队列**的消息都回到编辑器，且队列清空；
  5. steering 钩子取走的是 **steer** 队列（`nini-cli` 侧），followUp 队列**不被**它取走。
- 退出判据：`cargo test -p nini-tui --test queue_routing` 退出 0。
- 变异注入：把路由改回「都进 steer」→ 第 2 条红；让 steering 钩子改取 followUp 队列 → 第 5 条红。

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
