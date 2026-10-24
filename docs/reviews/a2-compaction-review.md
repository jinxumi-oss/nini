# 审查报告 — 压缩生命周期（A2）

审查范围：`git diff`（工作区未提交改动）+ 2 个未跟踪测试文件。**只读审查**：未执行 cargo build/test，
因此所有"测试是否真的红"的结论均为静态推演（见 Answers 第 4 条）；未修改任何被审文件。

## Files Reviewed
- `crates/nini-core/src/agent.rs`（31 serde tag；51-64 `Display for AgentPhase`；652-670 begin/end/queue API；716 `is_compacting()`；1063-1070 阈值压缩；1341-1384 溢出分支新增 Compacting 相位 + begin/end_auto_compaction；1405-1441 未改动的第二条溢出路径）
- `crates/nini-tui/src/runtime.rs`（261-276 sink 相位→`is_compacting`；422-455 `Done` 臂；451/814 自动投递；2290-2327 `submit_queued_turn`；2355-2375 `agent_alive` steer 分支；2571-2583 `is_compacting` 排队分支；2645 `mode=Running`；2700-2760 watchdog；3052-3060 Enter 键路径；3165-3186 `queue_while_running`）
- `crates/nini-core/tests/compaction_lifecycle.rs`（1-139，2 条）
- `crates/nini-tui/tests/compaction_queue.rs`（1-107，1 条）
- `crates/nini-cli/src/app.rs`（461-470 Agent 每轮新建；573-574 `phase.to_string()` 的 Display 桥接；593 `Done` 推送）
- `scripts/parity-gaps.json`（A2 条目改写 + 新增 A2b）

## Critical (must fix)
- `crates/nini-core/src/agent.rs:1354` — **队列被倒出来又丢掉**。`let _queued = self.end_auto_compaction();` 把 Pi 的 `_pendingNextTurnMessages` 取出来直接丢弃，而上面 1352-1353 的注释说"交回给调用方（下一轮会带上它们）"——事实相反：agent 级队列里的消息在这一行被销毁。产品里 `Agent::queue_next_turn` 全仓库无人调用（只有测试），今天没有用户可见损失；一旦按注释接线，用户消息会静默消失。修法：把 `_queued` 存进 Agent 字段，在下一轮 `run()` 开头随 user message 一起送出。
- `crates/nini-core/src/agent.rs:1364-1384` — **end 不成对**。`end_auto_compaction()` 只在 Some 分支（1354）调用；None 分支（钩子返回 None）直接 `return`，`is_auto_compacting` 永远停在 true（`reset_aborts` 只在单元测试里被调用，agent.rs:2275）。今天没有产品代码读 `Agent::is_compacting()`（全仓库仅 agent.rs:716 与测试）且 Agent 每轮新建（nini-cli/src/app.rs:461-470），影响被限制在这一轮；但契约已破：`is_compacting()` 在 run 结束后仍为真，`end_auto_compaction()` 再也不会被调用。修法：None 分支也 end（或用 RAII guard 保证配对）。
- `crates/nini-tui/src/runtime.rs:274` + `422-455` — **TUI 侧的标志没有"运行结束"复位**。`is_compacting` 只在 `PhaseChanged` 时被覆写；若一轮死在 Compacting 相位之后（钩子 panic → 任务 panic，`Done` 不会推送；或 watchdog 超时），sink 见到的最后一个相位是 "compacting:overflow"，`run_state.is_compacting` 永久为 true。`Done` 臂（422-455）与 watchdog（2700-2760）都只清 `agent_alive`/`mode`。此后每次 Enter → `submit_user_input_inner`（2355 agent_alive=false、2375 mode=Editing）→ 命中 2571 分支 → 消息进 `pending_next_turn_messages` 但**永远不会执行**（没有 run → 没有 Done → 没有 auto_turn_pending），用户只看到 "(queued: N pending)"，输入框再也发不出去。修法：`Done` 臂与 watchdog 里加 `s.run_state.is_compacting = false;`。

## Warnings (should fix)
- `crates/nini-tui/src/runtime.rs:274` — 字符串前缀耦合：`phase.starts_with("compacting")` 依赖 `AgentPhase` 的 Display（agent.rs:51-64）产出的 "compacting:{reason}"。仓库内已有第二种编码证明这条链对编码敏感：nini-tui/tests/agent_wire_e2e.rs:124 用 `format!("{phase:?}")`（Debug）→ "Compacting { reason: … }"，前缀判断为假。Display 一旦改成 "Compacting: overflow" 或 "compacting (overflow)"，整条链**静默失效**，且没有任何测试会红（新的 TUI 测试把字符串写死，见 Answers 4）。
- `crates/nini-core/src/agent.rs:1405-1441` — 第二条溢出路径（`StreamEvent::Error`，provider 报 400 的**生产主路径**）仍然"偷偷"压缩：没有 Compacting 相位、没有 begin/end_auto_compaction；阈值压缩（1063-1070）同样只发 `Compacted`、不发 Compacting。A2 只覆盖了 length-stop 一条路径，`scripts/parity-gaps.json` 把 A2 标为 `fixed` 说过头了。
- `crates/nini-tui/src/runtime.rs:2571-2583` vs `2355-2375` — "死代码变活代码"的说法不成立：真实 TUI 里压缩发生在**活着的 run 内部**（agent_alive=true），提交先被 2355 的 steer 分支截走（→`queue_while_running`，3165），根本到不了 2571；按键路径同理（3052-3060：mode==Running → `queue_while_running`）。两条分支写的又是同一个 `run_state.pending_next_turn_messages`，语义无差别。唯一差别：2571 分支**不把用户原文写进 transcript**（只显示 "(queued: N pending)"），而 Pi 的 `queueCompactionMessage` 会显示待发消息（`updatePendingMessagesDisplay`）——即"新活"分支的用户可见行为比 Pi 更差。
- `crates/nini-core/src/agent.rs:1361` — 压缩期间排队的消息**不会随这次重试送出**：Pi 的 `_pendingNextTurnMessages` 随下一次 turn 送达；nini 的队列要等 run 结束后的自动投递（runtime.rs:451、814）或下一次显式提交。

## Suggestions (consider)
- `crates/nini-tui/src/runtime.rs:274` — 把相位做成类型而不是字符串：`AgentEventLite::PhaseChanged` 携带 `nini_core::AgentPhase`（或加 `PhaseKind` 枚举），sink 里 `matches!(phase, AgentPhase::Compacting{..})`；若必须保持 String 通道，用 serde tag（agent.rs:31 已 `#[serde(tag="phase", rename_all="snake_case")]`）而不是 Display 前缀；再不济也用 `phase.split(':').next() == Some("compacting")` 并加一条断言 Display 形态的单元测试。
- 在事件层加语义而不是状态层派生：新增 `AgentEvent::CompactionStarted/CompactionEnded`（或让 `Compacted` 自带 begin/end），TUI 直接映射 `is_compacting`，不必猜"最后一个相位是什么"。
- `crates/nini-core/src/agent.rs:1341/1354` — 用 RAII 作用域（`CompactionGuard`，Drop 里 end）保证 begin/end 配对，避免下一个分支再漏。
- `crates/nini-core/tests/compaction_lifecycle.rs:39-45` — `big_history(40)` 构造后被 `let _ = history;` 丢弃（agent 历史只有 1 条 user 消息），建议删掉或真的 seed 进去，避免误导。
- `crates/nini-tui/tests/compaction_queue.rs:51` — 用 `AgentPhase::Compacting{..}.to_string()` 代替字面量 "compacting:overflow"，让 Display 变更立刻变红；并补一条"钩子返回 None → 标志必须落下"的用例（现有三条断言都不覆盖 None 分支）。
- `scripts/parity-gaps.json` — A2 的 oracle 描述"压缩期间排队的消息被交回"与代码不符（agent.rs:1354 丢弃），exit_criterion 只要求两条 cargo test 退出 0（发现不了上面三条 Critical）。建议把 A2 拆成 A2（相位/标志生命周期）与 A2c（消息交回）。

## Answers
1. **标志会不会漏落下** — 不成对：begin 在 agent.rs:1341，end 只在 Some 分支 1354；None 分支 1364-1384 直接 return → `Agent::is_compacting()` 永久 true（今天无产品读取者、Agent 每轮新建，影响受限但契约已破）。**TUI 侧在 None 分支不会卡**（该分支把相位置 Idle，starts_with 判定为假）。真正会卡的是"run 死在 Compacting 相位之后"：Done 臂 422-455 与 watchdog 都不复位 → 之后**每一次显式提交**（Enter/Editing 模式）都被 2571 收进队列且永不执行；自动投递路径（2290-2327）会把消息塞回队列，8 轮（MAX_AUTO_TURNS）后停。
2. **相位字符串的耦合** — 不可靠。`starts_with("compacting")` 绑死 Display 的具体写法（agent.rs:51-64），Display 一改就**静默失效**（无测试变红）；仓库内已有第二种编码（agent_wire_e2e.rs:124 用 Debug）证明该链对编码敏感。更稳：传枚举 → `matches!(.., AgentPhase::Compacting{..})`；退一步用 serde tag；再退一步 `split(':')` + 断言 Display 的测试。
3. **重试路径** — 重跑时 `is_compacting` 已是 false：end 在 1354、Working 相位在 1359-1360，1361 `retry_after_compaction=true; break 'outer;` 之后 1509 `continue` 回到迭代顶部重建请求（agent.rs:1042-1063）。重跑期间 mode 仍是 Running（runtime.rs:2645），用户提交走 `queue_while_running`（3052-3060）→ 进 `pending_next_turn_messages` → run 结束后由 auto_turn_pending 投递（runtime.rs:451/814）。**但**压缩期排队的消息不随这次重试送出（与 Pi 不同）。
4. **oracle 是否守住产品代码** — 部分守住。core 测试 1（compaction_lifecycle.rs:63-118）真守：回滚 1341-1360 会因找不到 Compacting 相位而红，删掉 `end_auto_compaction` 会因 `!agent.is_compacting()` 而红；但它**没有**断言标志在窗口内为真，也没覆盖 None 分支。core 测试 2（120-138）**只测了测试自己**：它手动 begin/queue/end，绕开 run 循环——把 1341/1354 整段删掉它照样绿。tui 测试（compaction_queue.rs:29-106）真守 sink 映射与 2571 分支（删掉任一处 → spawns≠0 或队列为空而红），但它手工 push 字符串、不经过 app.rs:573-574 的 Display 桥接，测不到"标志曾经为真"的完整链路；三条断言没有一条会因为 agent.rs 的 None 分支漏 end 而变红。
5. **有没有引入回归** — 压缩期间 mode == Running（runtime.rs:2645，压缩在 run 内部发生），用户按 Enter 走按键路径的 Running 分支（3052-3060 → `queue_while_running`），**不是**新加的 is_compacting 分支（后者只在"run 已死但标志还立着"的异常态可达）。与 Pi 不一致的两点：①Pi 压缩期提交会显示待发消息（updatePendingMessagesDisplay），nini 的 2571 分支只显示 "(queued: N pending)"、用户原文不进 transcript；②Pi 把压缩期消息随下一轮送达，nini 要等 run 结束后的自动投递。Esc 在压缩期间不中止压缩（已单列为 A2b）。

## Summary
把压缩从"偷偷进行"变成有相位、有标志，方向正确；但三处配对/复位缺口（agent.rs:1354 丢弃队列、1364-1384 漏 end、runtime.rs:274 无运行结束复位）意味着标志仍可能永久为真并让 TUI 永久排队，字符串前缀耦合则让整条链在 Display 变更时静默失效；建议补齐 None 分支与 Done/watchdog 复位、把相位改成枚举、并把 parity-gaps 的 A2 从 `fixed` 降级或拆分。


---

# 处置（执行者回执，2026-10-23）

审查提出的 3 条 Critical **全部为真**，已修；每条都补了 oracle 并做了变异验证。

| # | Critical | 处置 | 变异验证 |
|---|---|---|---|
| 1 | `let _queued = self.end_auto_compaction();` 把队列**直接丢弃**，注释却说「交回给调用方」 | 注释改成事实：产品里排队的是 **TUI 的** `run_state.pending_next_turn_messages`，agent 这一份队列一直是空的；`end_auto_compaction()` 的职责是**复位标志与中止句柄** | —（原注释说谎，不是行为缺陷） |
| 2 | None 分支直接 return，**漏 `end_auto_compaction()`** → 标志永久为真 | 补上收尾（并补注释：压缩没成功也是「压缩结束了」） | 注入「None 分支不收尾」→ `a_failed_compaction_also_ends_the_lifecycle` 变红 |
| 3 | TUI 侧标志**无运行结束复位** → 之后每次 Enter 都被收进队列且永不执行 | Done 臂与 watchdog 都复位 `is_compacting` | 注入「运行结束不复位」→ `the_end_of_a_run_resets_the_compaction_flag` 变红 |

**Warnings 里两条也一并处理**：

- **字符串前缀耦合**（`starts_with("compacting")` 绑死 Display）：改为
  `AgentPhase::tag()`（稳定的机器标签），Display 仍是给人看的、两者**解耦**；
  CLI 的桥接改传标签。Display 改格式不再让这条链静默失效。
- **第二条溢出路径（provider 的 `StreamEvent::Error`，生产主路径）没有相位**：
  现在两条路径都有完整的压缩生命周期（相位 + 标志 + 收尾）。
- **A2 标 fixed 过头**：现在 A2 的范围是「生命周期 + 排队」，**Esc 在压缩期间中止压缩**
  单列为 **A2b**（属 EscapeTarget 栈，A3）。

**Answers 里的一条补充**：审查指出真实流程里压缩发生在**活着的 run 内**（mode == Running），
提交会先被 steer 分支截走，2571 那条分支在正常流程里其实少被走到——这**属实**，
但两条分支都是「排队」，用户的消息都不会丢；差别在于 2571 那条不把用户原文写进 transcript，
比 Pi 差一档。这条记在 A2b 里一起处理。
