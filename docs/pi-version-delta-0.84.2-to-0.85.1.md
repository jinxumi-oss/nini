# Pi 0.84.2 → 0.85.1 版本差分（一次性证据）

> 生成：2026-10-23 · `python3 scripts/pi-version-delta.py --from references/pi-0.84.2 --to references/pi-0.85.1`
> 两份快照都已从仓库移除（本机 Pi 已升级到 0.85.1，**只保留一个对照物**），
> 但差分的结论必须留下——它是「目标统一」这件事的证据。

## 1. 结论

- nini 的行为层证据是「Pi 某个文件第 N 行」。两个版本之间，**90 处引用里只有 11 处仍指向同一行**，78 处指向了别的内容，1 处越界。
- 文件层面：新增 3 / 删除 3 / 变更 99 / 未变 309。
- **而当时没有任何东西会红。** 这就是「行号式 oracle」的致命处：
  引用看起来都还在，指向的却已经是另一段代码。

## 2. 指标

| 指标 | 0.84.2 | 0.85.1 |
|---|---|---|
| toolNames | bash, edit, find, grep, ls, read, write | bash, edit, find, grep, ls, powershell, read, write |

**0.85.1 新增了 `powershell` 工具**（Windows）。工具集是**行为面**，不是数量面。

## 3. 结构变化（最要紧的三条）

1. **`pi-client` / `pi-protocol` 两个包被删除**，取而代之的是 `@earendil-works/chord`。
   nini 对这一层的对应物一直是 0 节点——统一之后，这一层的**名字**也变了。

2. **keybindings 搬进了 `pi-tui`**：0.84.2 里 `tui.altScreen.top` 在 pi-tui；
   0.85.1 的 coding-agent 也多了一个同名 `keybindings.ts`（app 级键位）。
   **同名不同物**——所以别名表必须只有一份（`scripts/pi_refs.py`），否则解析到哪个全看运气。

3. **search-match 的样式从 coding-agent 层搬到了 pi-tui 层**：
   0.84.2 的 `interactive-mode` 与 pi-tui 都有 `searchMatch`；0.85.1 只剩 pi-tui 有。
   nini 照着 0.84.2 的 coding-agent 抄的那段样式，**对照物已经不在那里了**。
   这正是「分层不清」的代价：抄的时候不知道那是谁的职责，搬了以后无从察觉。

## 4. 处置

- **行号引用 → 符号引用**：`agent-loop.ts#shouldStopAfterTurn`。
  含义被钉住，行号由 `scripts/pi-oracle-lock.py --verify-refs` 在目标版本里解析；
  符号消失 = 红灯，符号挪位 = 报告位移。
- 快照只保留 `references/pi-0.85.1/`（411 个文件，sha256 锁）。
- 本机 Pi 升级到 0.85.1（`pi --version` = 0.85.1，全局四个子包同步 0.85.1）；
  回滚包留在 `/tmp/pi-0.84.2-backup.tar.gz`。
- 一次性迁移 `migrateAuthToAuthJson` 对本机是**空操作**（`auth.json` 已存在），
  升级前后 `~/.pi/agent/auth.json` 未被改动。

## 5. 漂移明细（原始树上的 90 处引用）

| 引用 | 0.84.2 该行 | 0.85.1 同一行 | 状态 |
|---|---|---|---|
| `crates/nini-cli/src/app.rs:405` → `agent-session.ts:534` |  | content: normalizedContent, | CONTENT-CHANGED |
| `crates/nini-cli/src/app.rs:425` → `agent-loop.js:96` | if (pendingMessages.length > 0) { | reasoning: nextTurnSnapshot.thinkingLevel === undefi | CONTENT-CHANGED |
| `crates/nini-cli/src/app.rs:431` → `interactive-mode.js:2491` | // This handles extension commands (execute immediat | if (text === "/resume") { | CONTENT-CHANGED |
| `crates/nini-conformance/tests/loop_semantics.rs:148` → `agent-loop.js:160` | pendingMessages = (await config.getSteeringMessages? | // Agent would stop here. Check for follow-up messag | CONTENT-CHANGED |
| `crates/nini-conformance/tests/loop_semantics.rs:212` → `agent-loop.js:163` | const followUpMessages = (await config.getFollowUpMe | // Set as pending so inner loop processes them | CONTENT-CHANGED |
| `crates/nini-conformance/tests/loop_semantics.rs:276` → `agent-loop.js:160` | pendingMessages = (await config.getSteeringMessages? | // Agent would stop here. Check for follow-up messag | CONTENT-CHANGED |
| `crates/nini-conformance/tests/loop_semantics.rs:306` → `agent-loop.js:160` | pendingMessages = (await config.getSteeringMessages? | // Agent would stop here. Check for follow-up messag | CONTENT-CHANGED |
| `crates/nini-conformance/tests/overflow_recovery.rs:16` → `agent-session.ts:1962` | private async _checkCompaction(assistantMessage: Ass | const preparation = prepareCompaction(pathEntries, s | CONTENT-CHANGED |
| `crates/nini-conformance/tests/overflow_recovery.rs:21` → `agent-session.ts:2001` | if (this._overflowRecoveryAttempted) { | if (extensionCompaction) { | CONTENT-CHANGED |
| `crates/nini-conformance/tests/overflow_recovery.rs:24` → `agent-session.ts:2016` | // but must not be included in the compact-and-retry | this._compactionAbortController.signal, | CONTENT-CHANGED |
| `crates/nini-conformance/tests/overflow_recovery.rs:294` → `agent-session.ts:1098` | if (await this._checkCompaction(msg)) { | return buildSystemPrompt(this._baseSystemPromptOptio | CONTENT-CHANGED |
| `crates/nini-conformance/tests/overflow_recovery.rs:380` → `agent-session.ts:329` | private _overflowRecoveryAttempted = false; | private _pendingNextTurnMessages: CustomMessage[] =  | CONTENT-CHANGED |
| `crates/nini-conformance/tests/overflow_recovery.rs:414` → `agent-session.ts:2001` | if (this._overflowRecoveryAttempted) { | if (extensionCompaction) { | CONTENT-CHANGED |
| `crates/nini-conformance/tests/overflow_recovery.rs:432` → `agent-session.ts:2160` | this.agent.state.messages = sessionContext.messages; | // independent of the configured context size or any | CONTENT-CHANGED |
| `crates/nini-conformance/tests/prepare_next_turn.rs:6` → `agent-session.ts:534` |  | content: normalizedContent, | CONTENT-CHANGED |
| `crates/nini-conformance/tests/prepare_next_turn.rs:45` → `agent-loop.ts:225` |  | hasMoreToolCalls = false; | CONTENT-CHANGED |
| `crates/nini-conformance/tests/prepare_next_turn.rs:216` → `agent-loop.ts:236` | ...config, |  | CONTENT-CHANGED |
| `crates/nini-conformance/tests/stop_after_turn.rs:3` → `agent-loop.ts:225` |  | hasMoreToolCalls = false; | CONTENT-CHANGED |
| `crates/nini-conformance/tests/stop_after_turn.rs:11` → `agent-loop.js:160` | pendingMessages = (await config.getSteeringMessages? | // Agent would stop here. Check for follow-up messag | CONTENT-CHANGED |
| `crates/nini-conformance/tests/stop_after_turn.rs:188` → `agent-loop.js:151` | if (await config.shouldStopAfterTurn?.({ | context: currentContext, | CONTENT-CHANGED |
| `crates/nini-conformance/tests/tool_batch_semantics.rs:13` → `agent-loop.ts:418` | const toolCalls = assistantMessage.content.filter((c | (tc) => currentContext.tools?.find((t) => t.name === | CONTENT-CHANGED |
| `crates/nini-conformance/tests/tool_batch_semantics.rs:21` → `agent-loop.ts:582` | function shouldTerminateToolBatch(finalizedCalls: Fi | toolCall: AgentToolCall; | CONTENT-CHANGED |
| `crates/nini-conformance/tests/tool_batch_semantics.rs:26` → `agent-loop.ts:216` | hasMoreToolCalls = !executedToolBatch.terminate; | await emit({ type: "turn_end", message, toolResults: | CONTENT-CHANGED |
| `crates/nini-conformance/tests/tool_batch_semantics.rs:194` → `agent-loop.ts:418` | const toolCalls = assistantMessage.content.filter((c | (tc) => currentContext.tools?.find((t) => t.name === | CONTENT-CHANGED |
| `crates/nini-conformance/tests/tool_batch_semantics.rs:236` → `agent-loop.ts:583` | return finalizedCalls.length > 0 && finalizedCalls.e | result: AgentToolResult<any>; | CONTENT-CHANGED |
| `crates/nini-conformance/tests/tool_batch_semantics.rs:396` → `agent-loop.js:216` | case "toolcall_delta": | if (partialMessage) { | CONTENT-CHANGED |
| `crates/nini-conformance/tests/tool_batch_semantics.rs:412` → `agent-loop.ts:583` | return finalizedCalls.length > 0 && finalizedCalls.e | result: AgentToolResult<any>; | CONTENT-CHANGED |
| `crates/nini-core/src/agent.rs:105` → `agent-session.ts:747` | await this._extensionRunner.emit(extensionEvent); | } | CONTENT-CHANGED |
| `crates/nini-core/src/agent.rs:481` → `agent-loop.ts:423` | return executeToolCallsSequential(currentContext, as | return executeToolCallsParallel(currentContext, assi | CONTENT-CHANGED |
| `crates/nini-core/src/agent.rs:539` → `agent-session.ts:329` | private _overflowRecoveryAttempted = false; | private _pendingNextTurnMessages: CustomMessage[] =  | CONTENT-CHANGED |
| `crates/nini-core/src/agent.rs:850` → `agent-session.ts:1098` | if (await this._checkCompaction(msg)) { | return buildSystemPrompt(this._baseSystemPromptOptio | CONTENT-CHANGED |
| `crates/nini-core/src/agent.rs:947` → `agent-loop.ts:236` | ...config, |  | CONTENT-CHANGED |
| `crates/nini-core/src/agent.rs:1034` → `agent-loop.js:96` | if (pendingMessages.length > 0) { | reasoning: nextTurnSnapshot.thinkingLevel === undefi | CONTENT-CHANGED |
| `crates/nini-core/src/agent.rs:1061` → `agent-session.ts:1098` | if (await this._checkCompaction(msg)) { | return buildSystemPrompt(this._baseSystemPromptOptio | CONTENT-CHANGED |
| `crates/nini-core/src/agent.rs:1164` → `agent-session.ts:2021` | return await this._runAutoCompaction("overflow", wil | firstKeptEntryId = result.firstKeptEntryId; | CONTENT-CHANGED |
| `crates/nini-core/src/agent.rs:1279` → `agent-loop.ts:122` | config: AgentLoopConfig, | context: AgentContext, | CONTENT-CHANGED |
| `crates/nini-core/src/agent.rs:1300` → `agent-session.ts:1994` | if (sameModel && (isContextOverflow(assistantMessage |  | CONTENT-CHANGED |
| `crates/nini-core/src/agent.rs:1304` → `agent-session.ts:2001` | if (this._overflowRecoveryAttempted) { | if (extensionCompaction) { | CONTENT-CHANGED |
| `crates/nini-core/src/agent.rs:1321` → `agent-session.ts:2016` | // but must not be included in the compact-and-retry | this._compactionAbortController.signal, | CONTENT-CHANGED |
| `crates/nini-core/src/agent.rs:1546` → `agent-loop.js:128` | newMessages.push(result); | } | CONTENT-CHANGED |
| `crates/nini-core/src/agent.rs:1576` → `agent-loop.js:216` | case "toolcall_delta": | if (partialMessage) { | CONTENT-CHANGED |
| `crates/nini-core/src/agent.rs:1579` → `agent-loop.js:582` |  |  | unresolved-in-both |
| `crates/nini-core/src/agent.rs:1639` → `agent-loop.js:128` | newMessages.push(result); | } | CONTENT-CHANGED |
| `crates/nini-core/src/agent.rs:1698` → `agent-loop.ts:418` | const toolCalls = assistantMessage.content.filter((c | (tc) => currentContext.tools?.find((t) => t.name === | CONTENT-CHANGED |
| `crates/nini-core/src/agent.rs:1730` → `agent-loop.ts:433` | async function executeToolCallsSequential( | assistantMessage: AssistantMessage, | CONTENT-CHANGED |
| `crates/nini-core/src/agent.rs:1860` → `agent-loop.js:216` | case "toolcall_delta": | if (partialMessage) { | CONTENT-CHANGED |
| `crates/nini-core/src/agent.rs:1867` → `agent-loop.ts:582` | function shouldTerminateToolBatch(finalizedCalls: Fi | toolCall: AgentToolCall; | CONTENT-CHANGED |
| `crates/nini-core/src/agent.rs:1876` → `agent-loop.ts:225` |  | hasMoreToolCalls = false; | CONTENT-CHANGED |
| `crates/nini-core/src/agent.rs:1907` → `agent-loop.ts:236` | ...config, |  | CONTENT-CHANGED |
| `crates/nini-core/src/agent.rs:1941` → `agent-loop.js:160` | pendingMessages = (await config.getSteeringMessages? | // Agent would stop here. Check for follow-up messag | CONTENT-CHANGED |
| `crates/nini-core/src/agent.rs:3205` → `agent-loop.js:160` | pendingMessages = (await config.getSteeringMessages? | // Agent would stop here. Check for follow-up messag | CONTENT-CHANGED |
| `crates/nini-core/src/agent.rs:3876` → `agent-loop.js:151` | if (await config.shouldStopAfterTurn?.({ | context: currentContext, | CONTENT-CHANGED |
| `crates/nini-core/src/agent.rs:4044` → `agent-loop.js:151` | if (await config.shouldStopAfterTurn?.({ | context: currentContext, | CONTENT-CHANGED |
| `crates/nini-core/src/agent.rs:4128` → `agent-session.ts:2160` | this.agent.state.messages = sessionContext.messages; | // independent of the configured context size or any | CONTENT-CHANGED |
| `crates/nini-core/src/agent_hooks.rs:67` → `agent-session.ts:534` |  | content: normalizedContent, | CONTENT-CHANGED |
| `crates/nini-core/src/agent_hooks.rs:174` → `agent-loop.ts:236` | ...config, |  | CONTENT-CHANGED |
| `crates/nini-core/src/tool.rs:76` → `agent-loop.ts:582` | function shouldTerminateToolBatch(finalizedCalls: Fi | toolCall: AgentToolCall; | CONTENT-CHANGED |
| `crates/nini-core/src/tool.rs:99` → `agent-loop.ts:582` | function shouldTerminateToolBatch(finalizedCalls: Fi | toolCall: AgentToolCall; | CONTENT-CHANGED |
| `crates/nini-core/src/tool.rs:216` → `agent-loop.ts:418` | const toolCalls = assistantMessage.content.filter((c | (tc) => currentContext.tools?.find((t) => t.name === | CONTENT-CHANGED |
| `crates/nini-core/tests/a3b_overflow.rs:10` → `agent-session.ts:2137` | this._summarizationRetryCallbacks({ source: "compact | if (skipAbortedCheck && assistantMessage.stopReason  | CONTENT-CHANGED |
| `crates/nini-core/tests/a3b_overflow.rs:87` → `agent-session.ts:1098` | if (await this._checkCompaction(msg)) { | return buildSystemPrompt(this._baseSystemPromptOptio | CONTENT-CHANGED |
| `crates/nini-session/src/lib.rs:473` → `agent-session.ts:3098` | * Get all user messages from session for fork select | // ================================================= | CONTENT-CHANGED |
| `crates/nini-tui/src/render.rs:1108` → `interactive-mode.ts:349` | const styleSearchMatch = (text: string) => theme.bg( | /** | CONTENT-CHANGED |
| `crates/nini-tui/src/render.rs:1127` → `interactive-mode.ts:349` | const styleSearchMatch = (text: string) => theme.bg( | /** | CONTENT-CHANGED |
| `crates/nini-tui/src/runtime.rs:1725` → `interactive-mode.js:2491` | // This handles extension commands (execute immediat | if (text === "/resume") { | CONTENT-CHANGED |
| `crates/nini-tui/src/runtime.rs:1959` → `interactive-mode.js:3505` | restoreQueuedMessagesToEditor(options) { | this.editor.setText(result.content); | CONTENT-CHANGED |
| `crates/nini-tui/src/runtime.rs:2273` → `interactive-mode.js:2491` | // This handles extension commands (execute immediat | if (text === "/resume") { | CONTENT-CHANGED |
| `crates/nini-tui/src/runtime.rs:2944` → `interactive-mode.js:2491` | // This handles extension commands (execute immediat | if (text === "/resume") { | CONTENT-CHANGED |
| `crates/nini-tui/src/runtime.rs:3046` → `interactive-mode.js:2491` | // This handles extension commands (execute immediat | if (text === "/resume") { | CONTENT-CHANGED |
| `crates/nini-tui/src/runtime.rs:3086` → `interactive-mode.js:3505` | restoreQueuedMessagesToEditor(options) { | this.editor.setText(result.content); | CONTENT-CHANGED |
| `crates/nini-tui/src/scrollbar.rs:6` → `interactive-mode.ts:885` | scrollbarStyle: (text) => theme.bg("scrollbarThumb", | scrollbarTrackStyle: (text) => theme.fg("scrollbarTr | CONTENT-CHANGED |
| `crates/nini-tui/src/signals.rs:38` → `interactive-mode.ts:3823` | // `signal-exit` checks the listener list during the |  | CONTENT-CHANGED |
| `crates/nini-tui/tests/esc_concurrency_probe.rs:26` → `agent-session.js:592` | get isIdle() { | catch { | CONTENT-CHANGED |
| `crates/nini-tui/tests/input_parity_m0.rs:5` → `interactive-mode.js:2491` | // This handles extension commands (execute immediat | if (text === "/resume") { | CONTENT-CHANGED |
| `crates/nini-tui/tests/input_parity_m0.rs:76` → `interactive-mode.js:2491` | // This handles extension commands (execute immediat | if (text === "/resume") { | CONTENT-CHANGED |
| `crates/nini-tui/tests/input_parity_m0.rs:110` → `interactive-mode.js:3505` | restoreQueuedMessagesToEditor(options) { | this.editor.setText(result.content); | CONTENT-CHANGED |
| `crates/nini-tui/tests/interrupt_editing_e2e.rs:23` → `interactive-mode.js:2219` | this.defaultEditor.onEscape = () => { | if (closed) | CONTENT-CHANGED |
| `crates/nini-tui/tests/scrollbar_e2e.rs:6` → `interactive-mode.ts:885` | scrollbarStyle: (text) => theme.bg("scrollbarThumb", | scrollbarTrackStyle: (text) => theme.fg("scrollbarTr | CONTENT-CHANGED |
| `crates/nini-tui/tests/search_match_visible.rs:16` → `interactive-mode.ts:349` | const styleSearchMatch = (text: string) => theme.bg( | /** | CONTENT-CHANGED |
