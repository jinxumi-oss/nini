# 输入与中断模型 —— 对齐 Pi 的改善计划

> 日期：2026-10-16（M0/M1 实施于同日）
> 触发：用户报告「nini 运行时按 esc 不能插入对话」
> 状态：**M0 + M1 已完成**（§8）；M2 实质已完成（`return_queue_to_editor`，见 §8.8 勘误）；M2b 双队列 / M3 / M4 未做
> 权威：本文件是 [`parity-with-pi.md`](parity-with-pi.md) **W1 阶段**的展开，不再单独排期
> 目标边界：见 [`docs/GOALS.md`](../GOALS.md)
> 相关：§33 turn divider、§34 markdown hr、§35 中止死区、§22 v0.10 里程碑

---

## 0. 结论摘要

§35 修掉的是**中止后的死区**，那是症状的一小部分。深度调研（源码基准 + tmux 实测）
得到的完整结论：

**nini 把"能否输入"绑在一个 `RunMode` 状态机上；Pi 没有这根轴。** Pi 的编辑器是一个
独立组件，`isStreaming` 从不影响它能否打字。围绕这一点，一共有 11 处差距，分四级。

而且这不是"Pi 独有、nini 缺失"的凭空补齐——**nini 的 agent 层已经建好了 steering
管道**（v0.10 R1 的队列驱动循环，每轮 poll `get_steering_messages`），**只是没有生产者**：
`crates/nini-cli/src/app.rs` 与 `crates/nini-tui/src/` 里 `get_steering_messages` 零实现，
默认返回 `Vec::new()`。这与 v0.10 的主题完全一致：**链路建好了，从未接线**。

---

## 1. Pi 的模型（源码基准）

以下全部来自 `@earendil-works/pi-coding-agent@0.84.2`
（`dist/modes/interactive/interactive-mode.js`）。

### 1.1 编辑器永不锁定

`onSubmit` 里没有"运行中禁止提交"这种分支。编辑器接受输入；**是否立即发送**
是提交路径的决策，不是输入路径的门禁。

### 1.2 运行中提交 = steer，不是排队，也不是拒绝

```js
// :2491
if (this.session.isStreaming) {
    this.editor.addToHistory?.(text);
    this.editor.setText("");
    await this.session.prompt(text, { streamingBehavior: "steer" });
    this.updatePendingMessagesDisplay();
    return;
}
```

`streamingBehavior: "steer"` 把消息注入**当前运行**（下一轮 turn 的输入），不是等这轮
跑完再发。

### 1.3 两类队列

`clearAllQueues()` 返回 `{ steering, followUp }`：

- **steering**：运行中提交的消息，注入当前运行
- **followUp**：压缩期间提交的消息，压缩结束后随下一条 prompt 发出
  （`queueCompactionMessage(text, "steer")`，`:3525`）

### 1.4 ESC 是**可替换的上下文 handler**，不是一个分支

`onEscape` 被反复替换与还原：

| 上下文 | ESC 的语义 | 源码 |
|---|---|---|
| 默认（streaming） | abort + 队列取回编辑器 | `:2219` |
| `compaction_start` | `abortCompaction()` | `:2714` |
| `auto_retry_start` | `retryEscapeHandler` | `:2748` |
| 分支摘要 | `abortBranchSummary()` | `:4237` |
| 恢复 | 还原默认 handler | `:2531` / `:2714` / `:4276` |

### 1.5 ESC 中止会**把队列还回编辑器**

```js
// :3505
restoreQueuedMessagesToEditor(options) {
    const { steering, followUp } = this.clearAllQueues();
    const allQueued = [...steering, ...followUp];
    if (allQueued.length === 0) {
        if (options?.abort) this.agent.abort();
        return 0;
    }
    const queuedText = allQueued.join("\n\n");
    const currentText = options?.currentText ?? this.editor.getText();
    const combinedText = [queuedText, currentText].filter(t => t.trim()).join("\n\n");
    this.editor.setText(combinedText);
    if (options?.abort) this.agent.abort();
}
```

**排队的消息不会因为中止而丢失**——它们回到编辑器，用户可以改后重发。

### 1.6 两个 nini 完全没有的输入语义

- **double-ESC**：`getDoubleEscapeAction()` → `tree` / `fork` / `none`（空编辑器上 500ms 内两次 ESC）
- **bash mode**：`isBashMode` 下 ESC 清空编辑器并退出 bash 模式

---

## 2. nini 的现状（实测 + 代码）

### 2.1 `RunMode` 门禁：11 处

`mode == RunMode::Editing` 出现在 `runtime.rs` 的 11 处输入分支 +
`submit_user_input`（`runtime.rs:1942`，`!= Editing` 直接 return）。

**tmux 实测（provider 延迟 10s）**：

```
运行中输入 "第二条消息"  →  ❯ 之后什么都没有
运行中按 Enter            →  transcript 里没有这条消息
```

### 2.2 ESC 是单一 if-else 分支

`runtime.rs:1625-1646` 的优先级：completion → search → pending_quit → Running abort → clear。
**没有上下文替换机制**，也没有"中止什么"的选择——中止永远是中止 agent。

### 2.3 `is_compacting` 从未被设为 `true`

```
$ grep -rn 'is_compacting = true' crates/     # 无结果
```

只有定义（`state.rs:1066`）、快照（`:1346`）、初始化 `false`（`:1365`）。

后果：`submit_user_input` 里「Pi parity: if compaction is in progress, queue the
message」（`runtime.rs:2137`）**是一条死路径**——nini 的压缩是**同步阻塞**调用
（`g.session_compact(keep)`，`:2150`），压缩期间根本没有"期间"。

### 2.4 单一队列，且不参与中断

`pending_next_turn_messages` 只在上述死路径里被写入。ESC 中止**不清空、不还原**它。

### 2.5 steering 管道空转

`agent.rs:994` 每轮 poll `hooks_ref.get_steering_messages(...)`，默认实现返回
`Vec::new()`，**driver 零实现**。

### 2.6 压缩是同步的

`submit_user_input` → `session_compact` → 阻塞返回。压缩期间 UI 不响应输入、不滚动。

---

## 3. 差距矩阵

| # | 能力 | Pi | nini | 级别 |
|---|---|---|---|---|
| 1 | 运行中输入 | 接受 | **拒绝**（§2.1 实测） | **P0** |
| 2 | 运行中提交 | steer 注入当前运行 | 无路径 | **P0** |
| 3 | steering 生产者 | `clearAllQueues().steering` | 空实现（§2.5） | **P0** |
| 4 | ESC 取回队列 | `restoreQueuedMessagesToEditor` | 无 | **P0** |
| 5 | followUp 队列 | `queueCompactionMessage` | 死路径（§2.3） | P1 |
| 6 | ESC 上下文替换 | 4 种 handler | 单一分支 | P1 |
| 7 | 中止压缩 | `abortCompaction()` | 无 | P1 |
| 8 | 压缩异步化 | `compaction_start/end` 事件 | 同步阻塞 | P1 |
| 9 | double-ESC | `/tree` `/fork` | 无 | P2 |
| 10 | bash mode | 有 | 无（`!cmd` 是另一回事） | P2 |

---

## 4. 计划

每阶段三件事：**为什么**（证据）、**怎么改**、**怎么证伪**（修复前必红的 oracle）。

### M0 —— 把差距钉成测试（先做，0.5 天）

不做功能，只让上面的判断变成会红的测试。没有 M0，M1 的验收将依赖人眼。

| 测试 | 断言 | 当前 |
|---|---|---|
| `typing_works_while_a_turn_is_running` | `mode == Running` 时 `Input` 仍写入 buffer | 红 |
| `submit_while_running_queues_instead_of_dropping` | 运行中提交进入 steering 队列 | 红 |
| `an_interrupt_returns_queued_text_to_the_editor` | ESC 后队列内容回到输入框 | 红 |
| `compaction_can_be_interrupted` | 压缩进行中 ESC 中止压缩而非清空输入 | **不可测 → 归入 M3** |

**修正（2026-10-16 执行时）**：原计划的第 5 条「`is_compacting` 存在生产路径」在 M0/M1
阶段**技术上不可测** —— nini 的压缩是同步阻塞调用，不存在"压缩进行中"的状态可供
测试进入。这本身就是缺陷的一种表现，测试不该假装能覆盖它。它随 M3 的异步化一起
变成可测。

**纪律**（§33/§34/§35 三次教训的固化）：测试只能引用**修复前后都存在**的公开 API；
接口会变的修复，oracle 必须走签名不变的更高层入口。

### M1 —— 运行中输入 + steer（P0，最大价值，2 天）

**为什么**：这是用户每天都会撞的差异；Pi 在这里的设计是"编辑器不锁定"，
nini 的 `RunMode` 是**自己发明的轴**，导致所有输入路径都要写门禁。

**怎么改**

1. **拆掉门禁轴**。`RunMode::Running` 不再禁止输入：
   - 11 处 `mode == Editing` 改为"不阻止输入"，输入类动作一律允许
   - 真正需要区分的是"提交后做什么"，不是"能不能打字"
2. **运行中提交走 steering**（Pi `:2491`）：
   ```rust
   if g.run_state.mode == RunMode::Running {
       // Pi: prompt(text, { streamingBehavior: "steer" })
       g.run_state.pending_steering_messages.push(text.clone());
       g.input.submit();                       // 清空编辑器
       g.push_user_queued(text);               // transcript 里可见，带排队标记
       mark_scroll_activity();
       return;
   }
   ```
3. **接上 steering 生产者**：在 `LiveConfigHooks` 实现
   `get_steering_messages`，从 `pending_steering_messages` 取并清空。
   **agent 侧管道已存在**（`agent.rs:994` 已在 poll），这一步只是填生产者。
4. transcript 里排队消息要有可见标记（Pi 的 pending display），否则用户不知道它还没发出去。

**证伪**：M0 的前两条 + 一个集成测试——真实 provider 下运行中提交，断言该消息出现在
**同一轮**的请求里（steer），而不是下一轮。

**风险**

- 11 处门禁改动面大，**必须先有 M0 的 5 条测试**，否则无法判断改坏了什么
- 排队消息的 transcript 呈现需要设计决定（与 §33 的分隔线问题同属排版，先查 Pi 的
  `updatePendingMessagesDisplay` 怎么做）
- `should_stop_after_turn` 的时序：§35 提过，steering 必须在 stop 判定**之后**才被 poll
  （Pi `agent-loop.ts` 的顺序），改错会出现"刚排队的消息被丢弃"

### M2 —— ESC 取回队列（P0，1 天）

**为什么**：即使 M1 做完，中止仍会丢排队消息——Pi 明确把它们还回编辑器（§1.5）。

**怎么改**：ESC 的 abort 分支改为 Pi 的 `restoreQueuedMessagesToEditor` 语义：

```rust
let restored = drain_all_queues(&mut g.run_state);   // steering + followUp
if restored.is_empty() {
    signal_abort();
} else {
    let combined = [restored.join("\n\n"), g.input.text.clone()]
        .filter(|t| !t.trim().is_empty()).join("\n\n");
    g.input.set_text(combined);                      // 队列还回编辑器
    signal_abort();                                  // 仍然中止
}
```

**证伪**：M0 的第三条。

### M3 —— 压缩异步化 + ESC handler 栈（P1，3 天）

这是本计划里最大的一块，也是最容易出回归的一块。

1. **压缩异步化**：把 `session_compact` 从提交路径的同步调用改成任务，
   期间 `is_compacting = true`（**这个字段终于有用了**，§2.3）。
2. **ESC handler 栈**：按 Pi §1.4 的四个上下文切换 ESC 语义。
   nini 可以用一个小结构替代 Pi 的"替换函数"写法：
   ```rust
   enum EscapeTarget { Idle, Agent, Compaction, Retry, BranchSummary }
   ```
   `handle_key` 的 Abort 分支按 `EscapeTarget` 分派。
3. `abort_compaction()`：`nini-core` 需要一个与 `agent.abort()` 对称的压缩中止。
4. 死路径复活：`is_compacting` 为真时，提交进入 followUp 队列（Pi `:3525`）。

**证伪**：M0 的第 4、5 条 + 「压缩期间提交的消息在压缩结束后随下一条 prompt 发出」。

**风险（必须先说清楚）**

- 同步 → 异步压缩会改变 compaction 全链路（session 写入、transcript 重建、
  context bar 数值）。v0.10 刚动过这一块（R2a/A2/A3），回归面大。
- **建议**：M3 单独一轮，先补齐 compaction 的集成测试再动异步化。

### M4 —— double-ESC 与 bash mode（P2，各 0.5 天）

低优先级，但都是 Pi 的输入语义。

- **double-ESC**：空编辑器上 500ms 内两次 ESC → `/tree`（或 `/fork`），可配置。
  nini 已有 `/tree` `/fork`，只是没有这个入口。
- **bash mode**：Pi 有独立的 bash 输入模式（`isBashMode`），ESC 清空并退出。
  nini 的 `!cmd` 是"内联 shell"，**不是同一功能**，不要混为一谈——
  这一点必须在动手前确认 Pi 的 bash mode 语义再决定是否做。

---

## 5. 依赖与顺序

```
M0 ──┬─► M1（运行中输入 + steer）──► M2（ESC 取回队列）
     │
     └─► M3（压缩异步化 + ESC 栈）  ← 需要 compaction 集成测试先行，建议单独一轮

M4 独立，随时可做
```

**M1 与 M3 有交集**：两者都会改提交路径。**先 M1 后 M3**，因为 M1 的验收更清晰，
且 M3 依赖 M0 的测试作为回归网。

---

## 6. 明确不做的事

| 不做 | 理由 |
|---|---|
| 保留 `RunMode` 作为输入门禁 | 它是全部 11 处门禁的来源；保留等于保留这个差异 |
| 顺手修 markdown 换行 | 那是独立 P0（§34 结论），混进来会让两个 P0 互相掩护 |
| 把 bash mode 当成 `!cmd` | 两者语义不同，误认会做出错的等价 |
| 「先改代码后补测试」 | §22 的纪律：先有红测试 |

---

## 7. 验收方法

1. **单测/oracle**：每条修复一个"修复前红"的断言，M0 的 5 条是全部阶段的回归网
2. **tmux 端到端**：慢 provider（`DELAY=10`）+ 脚本化 openai-compat，逐条走 §1 的行为：
   - 运行中打字 → 有回显
   - 运行中 Enter → transcript 出现排队标记，同一轮请求里带上它
   - ESC → 中止 + 排队消息回到输入框
   - 压缩中 ESC → 压缩停止，输入框内容不变
3. **回归**：每阶段跑全量（当前 1372）+ tmux 长任务场景（§34 的脚本化服务器）

---

## 8. 实施记录 —— M0 + M1（2026-10-16）

### 8.1 M0：4 红 1 绿

`crates/nini-tui/tests/input_parity_m0.rs`，5 条：

| 测试 | 修复前 | 修复后 |
|---|---|---|
| `typing_works_while_a_turn_is_running` | 红 | 绿 |
| `submitting_while_running_queues_the_message` | 红 | 绿 |
| `a_queued_message_is_visible_in_the_transcript` | 红 | 绿 |
| `an_interrupt_returns_queued_text_to_the_editor` | 红 | 绿 |
| `an_interrupt_with_an_empty_queue_keeps_the_draft` | **绿**（§35 已修） | 绿 |

失败信息就是缺陷本身：

```
an interrupt discarded what the user had queued: "being typed"
```

### 8.2 我第三次在同一个地方栽倒

M0 的第一版测试里写了 `state.run_state.pending_steering_messages` —— **修复才会引入
的字段**，于是它在父提交上**编译失败**。这是 §33、§34 之后的**第三次**。

这次的处理不是"改对"，而是**改设计**：M1 因此**复用现有的
`pending_next_turn_messages`**，不新增字段。Pi 分 `steering` / `followUp` 两类队列，
nini 先合一类 —— 少一个概念，而且是当前结构表达得出来的那一个。

> **纪律（正式写进本计划）**：写 oracle 之前先问"这个 API 在修复前存在吗"。
> 修复会新增字段/参数/类型时，测试只能引用既有 API，改用间接断言
> （transcript 可见性、队列内容、渲染结果）。

### 8.3 一个顺带发现的缺陷：不一致的门禁

统计 `handle_key` 37 个分支里的 `mode == Editing` 门禁，结果是：

```
Insert      门禁      Backspace   无门禁
Newline     门禁      Delete      无门禁
Undo        无门禁    Move*       无门禁（全部）
```

**运行中能删字、能移动光标、能撤销，却打不进新字符。** 门禁被加在了"输入"上，
没加在"编辑"上 —— 一个半锁定状态，比全锁定更让人困惑。

修复不是"删掉门禁"，是**让输入类动作与它的邻居一致**（都不设门禁）。

### 8.4 M1 的三处改动

| 改动 | 位置 | 对齐 |
|---|---|---|
| `Insert` / `Newline` 去掉模式门禁（handle_key + apply_action 两条路径） | `runtime.rs` | `interactive-mode.js:2491` |
| 运行中提交 → `queue_while_running` | `runtime.rs` | `prompt(text, {streamingBehavior:"steer"})` |
| ESC 中止 → `return_queue_to_editor` 先还队列再中止 | `runtime.rs` | `restoreQueuedMessagesToEditor({abort:true})`（`:3505`）|
| `LiveConfigHooks::get_steering_messages` —— **补上生产者** | `app.rs` | v0.10 R1 建的管道 |

最后一条是这次的核心：agent 在 `agent.rs:1041` 每轮都 poll 这个 hook 并注入
（`for queued in pending.drain(..) { self.messages.push(queued) }`），
**而产品里唯一的 hook 实现只写了 `prepare_next_turn`**。
所以管道是通的、消费者是好的、**生产者是空的** —— v0.10 主题的又一例。

### 8.5 修掉自己引入的一个缺陷

`queue_while_running` 的第一版把 `(queued: N pending; …)` 拼进消息体，
而 `push_user` 把**同一字符串写进 session** —— 模型会收到两遍：
一次来自 session，一次来自 steering，且都带着标记。

改为：transcript 与 session 都写**原文**，排队数只出现在状态栏
（`state.run_state.status = "queued: N pending"`）。

### 8.6 tmux 端到端

脚本化 openai-compat 服务器，agent 第一轮返回 tool call（让循环继续），
然后在运行中提交一条带标记的消息：

```
REQ#1 msgs=1 roles=['user']                    MIDTURN_PRESENT=False
REQ#2 msgs=4 roles=['user','assistant','tool','user']   MIDTURN_PRESENT=True
```

排队消息进入了**同一轮运行的下一次请求** —— 这就是 Pi 的 steer。

ESC 验证：

```
排队后输入框: （空）
按 ESC  →  状态: idle • aborted
           输入框: '❯ QUEUED-TEXT'
```

队列内容回到了编辑器，没有丢失。

### 8.7 全量

1372 → **1377**，clippy `-D warnings` 干净，fmt 干净。

### 8.8 未做（按计划留到后续）

> **勘误（2026-10-18）**：本节原文把 **M2（ESC 取回队列）**列为未做，这是错的——
> `runtime.rs` 的 `return_queue_to_editor` 已实现 Pi 的
> `restoreQueuedMessagesToEditor({abort:true})` 语义（见 A1 的实测）。
> 未做的是 **M2b（双队列拆分）**。详细差距表见
> [`parity-with-pi.md`](parity-with-pi.md) §2。

- **M2b** 的双队列拆分（`steering` / `followUp`）：当前合一类，ESC 时一并还回。
  Pi 区分二者是因为 followUp 在压缩后才发、steering 立即注入 —— 异步压缩
  （M3）落地后这个区分才有意义。
- **M3** 压缩异步化与 ESC handler 栈：计划中标注为高风险，建议单独一轮，
  先补 compaction 集成测试。
- **M4** double-ESC / bash mode。

---

## 9. 邻接修复：长回复截断（2026-10-16）

不属于本计划的输入模型，但同源（§34 的调查结论），一并做了。

### 9.1 现象与量化

同一条回复，只用 tmux：

| 终端宽度 | 屏幕结果 |
|---|---|
| 60 | `HEADMARK-2 填充段落内容。…填充段…` ← 尾部丢失 |
| 100 | `HEADMARK-2 填充段落内容。… TAI…` ← 尾部丢失 |
| 160 | 完整 |

90 行终端无效（不是视口）。**不是"后期"现象，是"回复越长越明显"**——
所有 fixture 都很短，所以从未变红。

### 9.2 根因

`rich.rs` 对每行调用 `truncate_line_cells`；v0.8.9 的注释解释了这个选择
（transcript 走 ratatui `List`，不换行、直接切边）。但**截断只是兜底，
不是目标行为**，而它让每条超宽回复的尾部静默消失。

`width.rs` 只有 `truncate_cells`，没有任何 wrap —— 而 Pi 有
`wrapTextWithAnsi`。

### 9.3 实现

`width::wrap_line(line, max_width) -> Vec<Line>`，对齐
`@mariozechner/pi-tui/dist/utils.js:580` 的 `wrapSingleLine`：

- **token 边界折叠**：连续非空白 / 连续空白各自成 token，行在 token 间断
- **token 超宽按 grapheme 切**（Pi 的 `breakLongWord`，`:657`）
- 每行 `trimEnd`、新行不以空白开头（`:619-628`）
- 不需要 ANSI tracker：nini 的 `Line` 用结构化 `Span`，切 Span 天然保样式

按 grapheme 而非 char 切，是为了让宽字符和带修饰符的 emoji 不会被劈开。

### 9.4 两条固化缺陷的测试被改写

| 测试 | 原断言 | 现断言 |
|---|---|---|
| `assistant_message_clamps_long_paragraphs` | 必须含 `…` | 必须含段落**结尾**，且折叠成多行 |
| `long_assistant_paragraph_shows_ellipsis` | 两种宽度都必须有标记 | 改名 `..._is_folded_not_cut`，断言结尾在 |

第三条 `no_transcript_row_overflows_at_any_width` 仍然通过，但**需要更高的帧**：
折叠后那条 transcript 远高于 30 行，它要找的 user 消息行滚出了视口。

### 9.5 第四次"测量先错"

第一次 tmux 读取看到行尾 `�`，像是字符被切一半。实际是 `cut -c` 按**字节**计数，
一个汉字 3 字节。tmux 原始字节无 U+FFFD，TestBackend 探针显示每行宽度精确
等于 `max_width` 且以完整汉字收尾。

> 本会话已四次测量先于结论出错：§33 的 grep 盲区（转义写法）、§34 的
> grep 判据与已崩溃的假服务器、§35 的固定行视口假设、本节的 `cut -c`。
> 共同点：**都是"用观察下结论"而不是"用计数/不变量下结论"**。

### 9.6 全量

1377 → **1383**，clippy `-D warnings` 干净。

---

## 10. 邻接修复：markdown 表格（2026-10-16）

同样来自 §34 的调查结论，独立一轮完成。

### 10.1 现状与根因

`markdown.rs` 的模块头表里写着 "Tables (basic)"，`ENABLE_TABLES` 也从
pulldown-cmark 切换那年起就开着。但 `Tag::Table` 的处理是：

```rust
self.out.push(Line::from(Span::styled("[table]".to_string(), …)));
```

**一个自称支持表格的模块，从第一天起就输出一行字面量。** 单元格文本落到
段落路径，于是每个单元格单独成行。

**文档说支持，代码说不支持，且没有任何测试覆盖这个差异。**

### 10.2 Pi 的规格

`@mariozechner/pi-tui/dist/components/markdown.js:543` `renderTable`：

| 要素 | 规格 |
|---|---|
| 边框 | `┌─${cells.join("─┬─")}─┐`（`:637`）|
| 内容行 | `│ ${rowParts.join(" │ ")} │` |
| 边框开销 | `3n + 1` |
| 自然宽度 | 该列最宽单元格 |
| 最小宽度 | 最长**单词**，上限 30（`getLongestWordWidth`）|
| 装不下时 | 每列 1，其余按 `(word - 1)` 权重分（`:577`）|
| 还有余量 | 按各列增长空间比例分，再逐列补舍入误差（`:625-634`）|
| 单元格 | 按列宽折行（`wrapCellText`）|
| 太窄 | 回退原始 markdown（`:557`）|
| 表头 | `theme.bold(padded)`，**无颜色槽**（`:652`）|

### 10.3 实现

`MdState::render_table` + `flush_pending` 在 `in_table` 时把 pending 转入
当前单元格（否则文本仍会走段落路径）+ `render_table_without_frame`
作为窄终端降级。

**表头样式踩了自己的坑**：先写了 `fg_style("mdTableHeader")`——一个
Pi 没有的键。`Theme::color` 对未知键不 panic，会回退到默认色，
**所以测试全绿而终端里与 Pi 不同**。已改为纯 `Modifier::BOLD`。

### 10.4 边框差 1：四个错误假设，第五个才对

`widths=[5,4,4]`、表达式必然产出 23 cells、实测却是 24。

依次排除：

1. `cut -c` 的字节/字符问题 → 本次是 `display_width`，不是 `cut`
2. `truncate_cells` 按字节判断 → 它用 `display_width`，且该分支未走到
3. `┬`(U+252C) 被算成 2 cells → 实测所有框线字符都是 1
4. pad 计算用了不同的 widths → 打印确认同一个数组
5. **打印 `rule()` 自己的输出** → `rule` 内部已拼了尾部 `─`，而调用处
   又传了 `right = "─┐"`，多出一个字符

> 这是本会话第六次"测量/推理先错"。前五次见 §34.9 / §35 / §36。
> 共同教训不变：**靠推理排除不如靠打印一次**。

### 10.5 验证

- `markdown_table_e2e` 8 条：修复前 **6 红**，修复后全绿
  （含"五行宽度下每行都不溢出"与"CJK 表格必须方正"——后者就是抓出 1-cell 错位的）
- tmux 真实二进制，单轮 / 第二轮 / 标题+列表+表格混合：框线与对齐均正确

### 10.6 一个未能复现的观察（诚实记录）

中途有一个 tmux 场景（`tbl3.py`：第二轮响应是 4 列中文表格）渲染成原始
markdown。**未能复现**：把同一条件改写为 `tbl4.py`（表格放在段落之后）
即正常；直接调 `render_markdown` 调 `auto_link` 调 transcript 累积，
四种路径单独测都正常。

**不宣称已解释**。可能的解释方向（未验证）：响应与请求的配对在那个脚本里
错位（脚本按请求序号返回，与实际轮次不一定对应）。已记录，供将来复现时
优先排查。
