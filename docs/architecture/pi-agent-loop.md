# Pi Agent Loop 执行原理（源码级）

> 源码坐标：`pi-coding-agent@0.84.3` → `node_modules/@earendil-works/pi-agent-core/dist/agent-loop.js`
> 全部行号指向该文件（552 行），除非另注。
> 交叉验证：`dist/core/agent-session.js`（session 层）、`dist/core/agent-loop.d.ts`（契约注释）、`docs/rpc.md`（对外语义）。

---

## 0. 一句话

**Pi 的循环是"两层嵌套 + 一个 session 外壳"**。内层管"一轮 = 一次 LLM 调用 + 全部工具执行"，外层管"Agent 说完了但队列里还有活"。两个待办队列（Steering / Follow-up）**唯一区别是被 poll 的时机**，注入机制完全相同。

---

## 1. 三层结构总览

| 层 | 位置 | 职责 | 结束条件 |
|---|---|---|---|
| **L0 session** | `agent-session.js:747 _runAgentPrompt` | 失败重试、溢出压缩、compact-then-retry、`agent_end` 后又有队列 | `_handlePostAgentRun()` 返回 false |
| **L1 外层** | `agent-loop.js:85 while(true)` | 只干一件事：poll **Follow-up** 队列 | follow-up 为空 |
| **L2 内层** | `agent-loop.js:88 while(hasMoreToolCalls \|\| pendingMessages.length>0)` | LLM 调用 + 工具执行 + poll **Steering** 队列 | 无工具调用 **且** 无 steering |

关键：**L2 才是真正"跑任务"的循环，L1 只是一次"队列复活"**。

---

## 2. L2 内层：源码骨架

```js
// agent-loop.js:78
async function runLoop(initialContext, newMessages, initialConfig, signal, emit, streamFunction) {
    let currentContext = initialContext, config = initialConfig, firstTurn = true;

    // ★ 第 0 个 steering 读点：在最外层之前（"用户可能在等待期间就打了字"）
    let pendingMessages = (await config.getSteeringMessages?.()) || [];   // :83

    while (true) {                                                       // :85  ← L1 外层
        let hasMoreToolCalls = true;                                     // :86  ← 永远先跑一轮
        while (hasMoreToolCalls || pendingMessages.length > 0) {         // :88  ← L2 内层
            if (!firstTurn) await emit({ type: "turn_start" }); else firstTurn = false;  // :89-92

            // ① 注入 pending（steering 或 follow-up 都走这里）→ 必须在 LLM 之前
            if (pendingMessages.length > 0) {
                for (const message of pendingMessages) {
                    await emit({type:"message_start", message});
                    await emit({type:"message_end",   message});
                    currentContext.messages.push(message);
                    newMessages.push(message);
                }
                pendingMessages = [];                                   // :103
            }

            // ② 流式 LLM 响应（AgentMessage[] → Message[] 只在这一步转换）
            const message = await streamAssistantResponse(...);         // :106
            newMessages.push(message);

            // ③ error / aborted → 立刻 agent_end 收工
            if (message.stopReason === "error" || "aborted") { ...; return; }  // :108-112

            // ④ 工具执行
            const toolCalls = message.content.filter(c => c.type === "toolCall");
            hasMoreToolCalls = false;                                   // :119
            if (toolCalls.length > 0) {
                // stopReason==="length" → 参数可能被截断，整批拒绝执行
                const batch = message.stopReason === "length"
                    ? await failToolCallsFromTruncatedMessage(toolCalls, emit)   // :122
                    : await executeToolCalls(currentContext, message, config, signal, emit);
                hasMoreToolCalls = !batch.terminate;                     // :127
                for (const r of batch.messages) { currentContext.messages.push(r); newMessages.push(r); }
            }

            await emit({ type: "turn_end", message, toolResults });      // :133

            // ⑤ prepareNextTurn：换 context / model / thinkingLevel（Pi 靠它做每轮动态调整）
            const snap = await config.prepareNextTurn?.({message, toolResults, context, newMessages});  // :138
            if (snap) { currentContext = snap.context ?? currentContext; config = {...}; }

            // ⑥ shouldStopAfterTurn —— ★ 优先级高于 steering/follow-up
            if (await config.shouldStopAfterTurn?.({...})) {             // :151
                await emit({ type: "agent_end", messages: newMessages });
                return;                                                 // :158  直接跳出两层
            }

            // ⑦ ★ 第 1 个 steering 读点：每轮结束（turn_end 之后），无条件
            pendingMessages = (await config.getSteeringMessages?.()) || [];  // :160
        }

        // ---- 到这里 Agent "本来要停了" ----
        const followUpMessages = (await config.getFollowUpMessages?.()) || [];  // :163
        if (followUpMessages.length > 0) {
            pendingMessages = followUpMessages;   // ★ 塞回 pending，复用内层注入逻辑
            continue;                             // :167  回到 L1 → L2 再跑一轮
        }
        break;                                     // :170
    }
    await emit({ type: "agent_end", messages: newMessages });  // :172
}
```

---

## 3. L1 外层存在的唯一理由

假设没有外层。Agent 打完最后一个工具 → 拿到 assistant 文本 → 无 tool call → 结束。**但用户按了 Alt+Enter 排了一个"再写个说明文档"的任务，队列里躺着，没人取。**

外层把这条"死后复活的路"补上：

```
内层退出条件：无 tool call  ∧  steering 空
     ↓
外层 poll followUp
     ↓ 非空 → pendingMessages = followUp → continue → 内层保证至少再跑一次 LLM
     ↓ 空     → break → agent_end
```

**这就是"直到工具调用、Steering、Follow-up 都没有后续了，循环才算结束"的精确代码路径。**

---

## 4. Steering vs Follow-up：区别只有 poll 时机

| | Steering（插话） | Follow-up（后续） |
|---|---|---|
| 队列 | `steeringQueue` | `followUpQueue` |
| 入队 API | `agent.steer(msg)` / `session.steer(text)` | `agent.followUp(msg)` / `session.followUp(text)` |
| poll 位置 | `agent-loop.js:83`（首轮前）+ **`:160`（每轮 turn_end 之后）** | **`agent-loop.js:163`（仅 L2 退出后）** |
| 触发场景 | Agent 正忙：正在流式输出、正在跑工具 | Agent 已经"看起来做完了" |
| 典型例子 | 正在改登录 bug → "别全改，只改 auth 模块" | 改完代码 → "再写一份说明文档" |
| 语义 | **改方向**（对当前任务纠偏） | **加任务**（当前任务之后的下一件事） |
| 谁在等 | 用户在等当前任务完成 | 用户在等整个会话空闲 |

**注入路径完全相同**（都变成 `pendingMessages` → `:96-103` 推入 context）。所以从 LLM 视角看，两者在消息流里**没有任何区别**——都是"user 角色消息，出现在下一次请求之前"。

**语义差别只体现在触发时机**：
- steering 在 Agent 还有事做的时候插队 → 打断的是"下一步计划"
- follow-up 在 Agent 说完之后才插队 → 接的是"下一件事"

### 4.1 排空模式（`PendingMessageQueue`，`agent.js:47-72`）

```js
drain() {
  if (this.mode === "all")  return 所有;
  return [this.messages[0]];   // "one-at-a-time"（默认）
}
```

- `one-at-a-time`：**每个完成的 assistant turn 只投喂 1 条**。排了 3 条 steering → 3 个独立 LLM 轮次，模型每轮只看到 1 条新指令。好处：模型能"消化一条、改一轮"。
- `all`：一轮全灌进去。好处：省 token、少轮次。

`steeringMode` / `followUpMode` 都在 settings 里可配（`settings-manager.js:477-482`），旧名 `queueMode` 会自动迁移（`:213-215`）。

---

## 5. 六个容易踩的边界细节

1. **`shouldStopAfterTurn` 优先级最高**（`:151` 在 `:160`/`:163` 之前）。返回 true 时直接 `agent_end`，**队列里剩余的 steering 和 follow-up 全部被跳过**。Pi 把它用于"上下文快满了，先停下来让下一次 prompt 触发压缩"。契约明确要求：**不得抛异常**，抛异常会打断低层循环、产生非标准事件序列。

2. **`agent_end` ≠ 空闲**。低层循环结束 ≠ 整个会话结束。`agent-session.js:747`：
   ```js
   await this.agent.prompt(messages);
   while (await this._handlePostAgentRun()) { await this.agent.continue(); }
   ```
   `_handlePostAgentRun()` 依次检查：可重试错误 → 压缩后重试 → **`agent.hasQueuedMessages()`（`agent_end` 回调里新排的队列）**。
   所以 TUI/扩展要判断"真的闲了"，必须用 **`agent_settled`**（`agent-session.js:330-336`），不是 `agent_end`。

3. **并发 `prompt()` 直接抛错**。`agent.js:225`：
   > `Agent is already processing a prompt. Use steer() or followUp() to queue messages, or wait for completion.`

   这就是"运行中不能直接发消息，必须走队列"这条 UI 约束的根源。

4. **`stopReason === "length"` 时整批工具调用不执行**（`:122`）。流式 JSON 会被"尽力抢救解析"，截断的参数**可能能 parse 通过但语义残缺**。Pi 宁可全部报错让模型重发，也不执行可能损坏的参数。

5. **`terminate` 批量早停**（`shouldTerminateToolBatch`）：只有当**一批工具的每一个**结果都 `terminate === true` 时，内层才退出。这是给"结构化输出工具"用的——工具自己说"答案定了，别再问模型了"。

6. **并行 ≠ 顺序**。`executeToolCalls`（`:283`）：若任一工具 `executionMode === "sequential"`，整批降级为串行；否则预检串行、真正执行并发、`tool_execution_end` 按完成顺序发、**tool_result 消息按 assistant 原始顺序发**（provider 要求 tool_result 与 tool_use 顺序一致）。

---

## 6. 事件时序（一次"正常跑完"）

```
L0   agent_start
     └ turn_start
        ├ [注入 pending]  message_start/end  (steering 或 follow-up 的 user 消息)
        ├ message_start   (assistant partial)
        ├ message_update × N  (text_delta / thinking_delta / toolcall_delta)
        ├ message_end     (assistant 完整)
        ├ tool_execution_start × N
        ├ tool_execution_update × N
        ├ tool_execution_end × N   (按完成顺序)
        ├ message_start/end × N   (toolResult，按 assistant 原始顺序)
        └ turn_end       {message, toolResults}
        ├ prepareNextTurn?
        ├ shouldStopAfterTurn?  ──true──→ agent_end ─────────────────┐
        └ poll steering ──非空──→ turn_start … 回到 ②                 │
                                                                      │
     L1  poll followUp ──非空──→ pendingMessages=followUp → continue ─┘
                空 → break
     └ agent_end  ←───────────────────────────────────────────────────┘
L0   _handlePostAgentRun()  true → agent.continue()  (retry / compact / 新队列)
     └ agent_settled   ← 这才是"真空闲"
```

---

## 7. nini 现状差距（3 处，均已定位到行）

nini 目前只有**一层**循环：`crates/nini-core/src/agent.rs:741 loop { ... }`，对应 Pi 的 L2。

### 差距 1：缺 L1 外层 → follow-up 是"死消息"（P0）

`agent.rs:1179-1199`：

```rust
if tool_calls.is_empty() && !length_stop_reject {
    let followup = ...hooks.get_followup_messages(&messages_snapshot);
    for msg in followup { self.messages.push(msg); }   // 塞进历史
    self.set_phase(AgentPhase::Idle);
    yield AgentEvent::PhaseChanged(AgentPhase::Idle);
    return;                                            // ★ 直接结束，没有任何 LLM 调用
}
```

消息进了历史，但**不会立刻触发一轮 LLM**。用户看到的是"Agent 说完了、回到空闲"，那个 follow-up 任务要等他**再打一次字**才被处理。Pi 的语义是"Agent 说完 → 自动接下一条"。

现有测试 `followup_hook_injects_messages_on_run_end`（`agent.rs:2653`）断言 `messages().len() == 4`，恰好把这个错误行为**固化成了预期**。

**修法**：把 `tool_calls.is_empty()` 分支从 `return` 改为 `break` 出内层，外层套一层：

```rust
let mut pending: Vec<Message> = Vec::new();
loop {                                     // ← L1 外层
    let mut has_more_tool_calls = true;
    while has_more_tool_calls || !pending.is_empty() {   // ← L2 内层
        // 循环开头：注入 pending
        for m in pending.drain(..) { self.messages.push(m); yield /* message 事件 */; }
        ... 现有逻辑 ...
        if tool_calls.is_empty() && !length_stop_reject { break; }   // 不再 return
        ... 工具执行 ...
        pending = hooks.get_steering_messages(&recent);
    }
    let fu = hooks.get_followup_messages(&self.messages.clone());
    if fu.is_empty() { break; }
    pending = fu;
}
```

同时 `max_iterations`（`agent.rs:769`）要改成**同时约束外层**，否则 follow-up 可以无限复活循环。

### 差距 2：steering 只在"有工具调用"的分支里被 poll（P1）

Pi 在 `:160` **每轮末尾无条件** poll steering。nini 只在工具执行分支末尾调用 hook（`agent.rs:1451-1456`）。

后果：Agent 正在流式输出长文本（无 tool call）时用户插话 → 队列不被消费 → 一直等到**下一次 run** 才被投喂。Pi 在这种情况下会**立刻多跑一轮**处理这条插话。

现有测试 `steering_hook_fires_only_after_tool_execution`（`agent.rs:2693`）断言 `steering_count() == 0`，把这个偏差也固化了。

### 差距 3：`should_stop_after_turn` 位置不对（P2）

nini 在**循环开头**调用（`agent.rs:749-757`），Pi 在 **turn_end 之后、poll 队列之前**调用（`agent-loop.js:151`）。

后果：nini 无法实现"这一轮跑完就优雅收工，且**跳过剩余队列**"这个语义——它在开跑前就决定了停不停，队列里的东西会被白白丢掉或强行执行。

### 差距 4：没有队列抽象，也没有 TUI 入口（P1）

Pi 的 `PendingMessageQueue`（`agent.js:47`）提供 `enqueue / drain(mode) / hasItems / clear`，
`Agent` 暴露 `steer() / followUp() / clearAllQueues() / hasQueuedMessages()`，
TUI 侧 `Enter`（运行中）→ steer，`Alt+Enter` → followUp（`interactive-mode.js:2531/3371`）。

nini：`grep -rn "steer\|followUp" crates/nini-tui/ crates/nini-cli/` → **零命中**。
nini 的 steering/follow-up 只是 hook trait 的两个方法，没有面向用户的入口，也没有队列语义（一次性 drain，没有 `one-at-a-time` / `all` 模式）。

---

## 8. 修复优先级

| 优先级 | 事项 | 理由 |
|---|---|---|
| P0 | 加 L1 外层 + `break` 替代 `return` + 修 `followup_hook_injects_messages_on_run_end` 断言 | follow-up 功能当前是坏的 |
| P1 | steering poll 移到内层末尾（无条件）+ 修对应测试 | 长文本流式输出时插话无响应 |
| P1 | 引入 `PendingMessageQueue`（含 `one-at-a-time`/`all`）+ `Agent::steer/follow_up` + TUI Enter/Alt+Enter | 让两个入口真正可用 |
| P2 | `should_stop_after_turn` 移到 `turn_end` 之后 | 语义正确性 |
| P2 | `max_iterations` 覆盖外层 | 防无限复活 |
| P3 | 引入 `agent_settled` 等价事件 | TUI 状态正确性 |
