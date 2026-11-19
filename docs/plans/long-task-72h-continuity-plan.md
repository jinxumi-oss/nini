# nini 72 小时不间断工作：修掉「截断即中断」等 4 处任务断点

Status: **已完成**（2026-11-16 执行：多 agent 落地 D1–D4 与测试；2026-11-19 收口：reviewer 的 3 条 MINOR 全部落地（其中 1 条为格式/注释清理、1 条是 review 已落地的压缩事件顺序、1 条是版本标注，本次收口处理其格式/注释两项），门禁通过）
执行方式: 多 agent（worker A 核心 / worker B watchdog / reviewer 审查）

## 0. 需求原文（用户，2026-11-16）

> nini项目，修复这个bug,多agent执行，我需要nini能72小时不间断工作，直到任务汇报或者完成，不产生这种使任务间断的bug。

配图现象（nini TUI，MiniMax-M3，模型 id `u 0c38b0j`）：

- 转录里连续约 24 行**完全相同**的 `Let me read scrollbar.rs.`
- 结尾一行红字 `Response was truncated before completion.`
- 状态栏 `idle`、`ctx 0%`、`in 5.1M out 67.5K`
  （`in/out` 是**跨轮累计**，见 `crates/nini-tui/src/render.rs:436-443`：`run_state.tokens.input += …`）

**验收口径**：一次任务跑到「模型给出汇报 / 任务完成」为止，途中不得因为 nini 自身的机制而中断；单次连续工作时间目标 72 小时。

## 1. 现象取证（代码级）

模型侧事实（从截图 + 本机 session 语料交叉确认）：

- nini 的 assistant 消息 = `text` + `toolCall` 同一块（`~/.nini/agent/sessions/*/*.jsonl` 逐条抽样：
  `[036] text:'找到 Pi 参考实现。让我读取…' | toolCall:'read({…})'`）。
- 截图里**没有任何工具调用行**（工具调用渲染成 `⏺ read(...)` 一行 + 一行结果，
  `crates/nini-tui/src/render.rs:1287-1327`），因此那 24 行是**同一个响应里的散文**，
  模型退化成复读同一句话，把输出预算烧光 → `stop_reason = "length"`。
- 24 行只对应**一次**截断：若每轮各截断一次，CLI 会给每轮都推一条
  `Response was truncated before completion.`（`crates/nini-cli/src/app.rs:846-851`），
  截图里只有一条。

nini 侧链条（每一环都有行号）：

1. `crates/nini-core/src/agent.rs:1987` —
   `truncated_answer = is_recoverable_length("length", output_tokens, max_tokens) && tool_calls.is_empty()`
   → `crates/nini-core/src/overflow.rs:412`：`output_tokens < desired_max`。
   模型把输出预算烧光时 `output_tokens >= max_tokens` → **false**。
2. 于是 `recoverable == false`（`agent.rs:1993`）→ **不进压缩/重试分支** →
   这一轮被当作正常一轮结束：`TurnEnd{stop_reason:"length"}`（`agent.rs:2318`）。
3. `has_more_tool_calls = !tool_calls.is_empty()`（`agent.rs:2351`）= false，
   循环顶部守卫（`agent.rs:1655-1690`）判定「没有后续消息」→ **运行结束**。
4. TUI 收到 `TurnEnd` → CLI 推 `StopReason("Response was truncated before completion.")`
   （`crates/nini-cli/src/app.rs:846-851`）→ 用户看到红字，**任务停在半路**，
   必须手动说「继续」。

结论：**截断 = 任务中断**。这是长任务最常见的断点。

## 2. 断点清单（4 处，全部要修）

| # | 断点 | 位置 | 触发条件 | 后果 |
|---|---|---|---|---|
| D1 | 长度截断直接结束运行 | `agent.rs:1987` + `agent.rs:2318` + 循环顶部守卫 | 模型输出撞上限且本轮无工具调用 | 任务停在半路 |
| D2 | 截断重试只有一次 | `agent.rs:1225-1253`（`recover_from_overflow` 的 `overflow_recovery_attempted`） | 第二次仍截断 | 报 `Context overflow recovery failed after one compact-and-retry attempt.` 后结束 |
| D3 | TUI watchdog 把「慢」误判成「死」 | `crates/nini-tui/src/runtime.rs:624`（`WATCHDOG_TIMEOUT_SECS = 600`）、`:3831`（`tokio::time::timeout`）、`:3877+`（超时分支复位状态 + 推 `agent run stopped: … 600s`） | 任何一轮 > 10 分钟 | 谎报「任务已停止」+ 放开编辑器 → 用户再提交一次会有**两个 agent 同时写同一个 session** |
| D4 | runaway 兜底 1000 轮 | `agent.rs:48`（`RUNAWAY_TURN_BACKSTOP = 1000`）、`agent.rs:1722` | 用户未设 `max_iterations` 时跑满 1000 轮 | 报 `runaway guard: …` 后结束；72h 连续工作必然撞上 |

D3/D4 是「72 小时」这个口径直接要求的：**任何按时间/轮次触发的兜底都必须在 72 小时内不触发**。

## 3. 改动

### 改动 1（D1 + D2，核心）：长度截断不是终点 —— 丢掉半截输出 + 继续提示 + 继续同一轮

文件：`crates/nini-core/src/agent.rs`

**新常量**（放在 `agent.rs:48` 附近，与 `RUNAWAY_TURN_BACKSTOP` 同区）：

```rust
/// 连续长度截断的容忍上限（v0.17）。
///
/// 截断是模型侧失败（输出上限），不是 nini 的失败。nini 的职责是
/// **把这一轮接住并继续**，而不是把任务扔回给用户。上限的作用是
/// 兜住「模型每次都截断」这种真正的死局：超过它就结束运行并汇报，
/// 而不是永远烧输出预算。
pub const MAX_CONSECUTIVE_TRUNCATIONS: usize = 5;
```

**新字段**（Agent struct，紧邻 `overflow_recovery_attempted`）：`consecutive_truncations: usize`，
两个构造函数（`Agent::new` / `Agent::with_hooks`）都初始化为 `0`。

**新常量（继续提示文案）**，与 `[CONTEXT SUMMARY]`（`crates/nini-core/src/compaction.rs:577`）同一约定：

```rust
/// 长度截断后塞回上下文的继续提示（v0.17）。
///
/// 英文：nini 的系统提示词是英文（`crates/nini-cli/src/prompt_setup.rs:70`
/// 「You are nini, a Pi-compatible Rust coding agent.」），提示词同语言。
const TRUNCATION_CONTINUATION_NOTICE: &str = "[TRUNCATED RESPONSE — CONTINUE]\n\n\
Your previous response was cut off by the output token limit before it finished, and it did not make any tool call.\n\
Continue the task now: take the next concrete step (call a tool), or — if the task is already done — report the result.\n\
Do not repeat the previous output.";
```

**改 `MessageStop` 分支**（`agent.rs:1936` 起的那段）：

1. 把 `truncated_answer` 从 `recoverable` 里摘掉（截断不再走 `recover_from_overflow`）：
   - `let recoverable = overflowed;`（真正的上下文溢出路径**不动**）
   - `truncated_answer` 变量删除。
2. 新增分支（放在 `recoverable` 分支**之前**）：

```rust
// v0.17（72h 不间断）：**长度截断不是终点**。
//
// 现象（用户截图）：模型把同一句 "Let me read scrollbar.rs." 复读了
// 二十几遍、撞上输出上限；nini 把这一轮当成「结束」，转录里留下一行
// 红字 "Response was truncated before completion."，**任务停在半路**，
// 用户必须手动说「继续」。
//
// 旧路径两条都会结束运行：
//   a) output_tokens < max_tokens → recoverable → 压缩 + 重试**一次**；
//      第二次仍截断 → "recovery failed after one compact-and-retry attempt"
//      → 运行结束（agent.rs:1225-1253）。
//   b) output_tokens >= max_tokens（模型把输出预算烧光，截图正是这种）
//      → 不进 recover 分支 → 这一轮正常结束 → 循环顶部 pending 为空
//      → **运行结束**。
//
// 新路径：截断 = 这一轮没写完。半截输出**不进模型的重试上下文**
// （Pi 同款：agent-session.ts:2194 "The message remains in session history
// but is excluded from retry context"），塞一句继续提示，然后**继续同一轮**。
// 连续截断超过 MAX_CONSECUTIVE_TRUNCATIONS 才结束运行，并给出明确汇报。
let length_truncated = stop_reason == "length" && tool_calls.is_empty();
if length_truncated {
    self.consecutive_truncations += 1;
    if self.consecutive_truncations > MAX_CONSECUTIVE_TRUNCATIONS {
        Err(AgentError::RepeatedTruncation {
            attempts: MAX_CONSECUTIVE_TRUNCATIONS,
        })?;
    }
    last_usage = usage;
    // 这一轮到此为止：半截输出交给 session/transcript（和普通一轮一样），
    // 但**不**进 self.messages（= 模型的重试上下文）。
    yield AgentEvent::TurnEnd {
        stop_reason: stop_reason.clone(),
        usage: last_usage.clone(),
    };
    // 继续提示（User 角色，和 [CONTEXT SUMMARY] 同一约定）。
    self.messages.push(Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: TRUNCATION_CONTINUATION_NOTICE.to_string(),
        }],
        timestamp: 0,
    });
    // 这一轮还没完：循环必须继续。`has_more_tool_calls` 是循环顶部守卫
    // 唯一的「继续」信号（agent.rs:1655），必须显式置 true——上一轮若是
    // 「无工具调用且被 terminate」，它此时是 false，守卫会直接结束运行。
    has_more_tool_calls = true;
    // 丢掉本轮（半截输出不进上下文），重新请求。
    continue_after_truncation = true;
    break 'outer;
}
// 任何没有发生截断的一轮 → 计数清零。
self.consecutive_truncations = 0;
```

3. 循环里新增一个与 `retry_after_compaction` 并列的标志 `continue_after_truncation`（声明在
   `retry_after_compaction` 旁边），并把跳过本轮剩余收尾的判断改成：

```rust
if retry_after_compaction || continue_after_truncation {
    continue;
}
```

4. **删除** `crates/nini-core/src/overflow.rs:412` 的 `is_recoverable_length`
   —— 改动后没有调用方（留着会成为仓库 sweep 的「未接线」候选；
   `scripts/sweep-unwired.py` 会把它列成新缺陷）。它对 Pi 的引用
   （`overflow.ts:172`）写进新分支的注释里。

5. **新错误变体** `AgentError::RepeatedTruncation { attempts: usize }`
   （`crates/nini-core/src/agent.rs` 的 `AgentError` enum，紧邻 `RunawayGuard`），
   文案口径（和 `RunawayGuard`/`TooManyIterations` 一致：说清发生了什么 + 用户能做什么）：

```
the response was truncated {attempts} times in a row (output token limit) and produced no tool call; the task is unfinished — the history is kept, so send another message to continue
```

**不压缩的取舍**（要在注释里写明，避免后来人以为是漏了）：
Pi 在截断后压缩并重试一次（`agent-session.ts:2162-2205`）。nini 这里**不压缩**：
压缩会把长任务的工作记忆换成摘要，而截断是**输出侧**失败（输入上下文不是病因）；
上下文该缩的时候，循环顶部既有的 `maybe_compact_at_threshold()`（`agent.rs:1735`）会按阈值压缩。

### 改动 2（D3）：watchdog 超时不再等于「任务已死」

文件：`crates/nini-tui/src/runtime.rs`

- `WATCHDOG_TIMEOUT_SECS = 600`（`:624`）保留（它仍是「多久该看一眼」的心跳），但
  **超时分支不再复位状态、不再推「任务已停止」**。
- 把 `runtime.rs:3831` 附近的

```rust
let outcome = tokio::time::timeout(
    std::time::Duration::from_secs(WATCHDOG_TIMEOUT_SECS),
    handle,
).await;
```

  改成**重新武装**的等待：超时只 `tracing::warn!`（说明「这一轮还在跑，长任务是常态」），
  然后**继续 await 同一个任务**；只有任务真的结束（`Ok(Ok(()))` 没走 Done / `Ok(Err(panic))`）
  才做既有的状态复位 + 终止通知。

- `WatchdogStopReason::Timeout` 变体与 `watchdog_stop_notice` 里对应的 Timeout 文案删除
  （超时不再有「终止通知」这回事）。
- `runtime.rs:3924` 附近「没有 tokio runtime」那个分支现在推的是 Timeout 文案，改推
  `WatchdogStopReason::NoDone`（语义：没有 Done 事件）。

**为什么必须改**：72h 连续工作里，任何一个超过 10 分钟的轮次都会撞上现在的超时分支。
它会（a）在转录里谎报「agent run stopped」、（b）把 `agent_alive` 清掉并把编辑器放回
Editing —— 用户此时再提交一次，`submit_user_input_inner` 的 `agent_alive` 守卫失效，
**两个 agent 同时写同一个 session 文件**。

### 改动 3（D4）：runaway 兜底覆盖 72 小时

文件：`crates/nini-core/src/agent.rs`

- `RUNAWAY_TURN_BACKSTOP`（`:48`）从 `1000` 提到 `20_000`，注释写清推导：
  72 小时 ÷ 约 13 秒/轮 ≈ 20k 轮；用户显式设了 `max_iterations` 时不受此值影响。
- `crates/nini-conformance/tests/long_task_delivery.rs` 里那条
  `an_uncapped_run_that_never_settles_stops_at_the_backstop` 断言用
  `RUNAWAY_TURN_BACKSTOP + 1`（是常量引用，**不用改**）；但它会真的跑满 20k 轮，
  需要确认测试耗时可接受（provider 是内存 fixture）。

## 4. 测试（新增，锁住行为）

### T1（核心）`crates/nini-core/tests/truncation_continues.rs`

用一个「总是截断」的 fixture provider（`nini_ai::fixture::ProgrammedProvider`）：

1. `a_truncated_response_does_not_end_the_run`：第 1 轮 `stop_reason="length"`、无工具调用；
   第 2 轮正常 `end_turn`。断言：**模型被调用 ≥ 2 次**，且运行**正常结束**（不是错误结束）。
2. `the_truncated_text_is_not_in_the_retry_context`：断言第 2 次请求的上下文里
   **没有**第 1 轮那段被截断的文本。
3. `the_continuation_notice_is_in_the_retry_context`：断言第 2 次请求的上下文里有
   `[TRUNCATED RESPONSE — CONTINUE]`。
4. `repeated_truncations_stop_the_run`：provider 永远截断 → 断言运行以
   `RepeatedTruncation` 结束，且 provider 调用次数 ≤ `MAX_CONSECUTIVE_TRUNCATIONS + 1`。
5. `a_turn_without_truncation_resets_the_counter`：截断 → 正常一轮 → 再截断 → 断言仍在跑
   （计数被清零）。

### T2 `crates/nini-tui/tests/agent_abnormal_notify.rs`（改现有文件）

现有文件锁的是「超时 → 推终止通知 + 复位状态」。按改动 2 更新为：
- 超时分支**不再**推终止通知、**不再**复位状态（源码级断言，仿文件里既有的
  `RUNTIME_SRC` 字符串断言写法）。
- `NoDone` / `Panicked` 两个分支仍然推终止通知（既有断言保持）。

### T3（改动 3 的护栏）`crates/nini-core/tests/truncation_continues.rs` 内追加

- `the_runaway_backstop_covers_a_72_hour_run`：断言
  `RUNAWAY_TURN_BACKSTOP >= 20_000`（源码级常量断言，注释写推导）。

## 5. 验收

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p nini-core
cargo test -p nini-tui
cargo test -p nini-conformance
cargo test --workspace
bash scripts/gate.sh          # 基线不许退步（修了 bug 必须有断言）
```

## 6. Out of scope（明确不做）

- 不做「重复内容检测器」：截断续跑 + 继续提示（明写「不要重复上一轮输出」）已经打断复读；
  再加一个检测器属于新概念，没有证据要求。
- 不改压缩算法、不改 compaction 触发阈值。
- 不改 `max_iterations` 的语义（用户设了上限时以用户为准）。
- 不动 CHANGELOG / README / 依赖版本。
- 不做 Pi 的 `_runAutoCompaction("overflow")` 语义对齐（本次只保证「不中断」）。

## 7. 分工（多 agent）

| Agent | 范围 | 产物 |
|---|---|---|
| worker A | 改动 1 + 改动 3 + T1 + T3（都在 `crates/nini-core`） | `docs/plans/long-task-72h-continuity-changes-a.md` |
| worker B | 改动 2 + T2（`crates/nini-tui`） | `docs/plans/long-task-72h-continuity-changes-b.md` |
| reviewer | 审查两个 diff + 跑测试 | `docs/plans/long-task-72h-continuity-review.md` |

worker A 与 worker B 改的文件不重叠，可并行。
