# Pi Agent 实现研究（基于 v0.85.1 source snapshot）

> 通过阅读 `references/spec-v0.85.1/` 与 node_modules 编译产物，
> 对照 nini v0.8.4 的实现做架构对比。这份文档只标记**架构层面**的差距，
> 增量改进（性能/UX/bugfix）不在范围。

参考：
- pi-coding-agent 仓库：@earendil-works/pi-coding-agent v0.85.1
- pi-agent-core（独立 npm 包）：@earendil-works/pi-agent-core v0.85.1
- pi-ai（provider SDK）：@earendil-works/pi-ai v0.85.1
- nini 当前版本：v0.8.5（commit `3bf4815`）

---

## 1. 模块清单对比

| 模块 | Pi 行数 | nini 行数 | 差距 |
|---|---|---|---|
| system-prompt / prompt_setup | 73 (核心) | 60 | ⚠️ **行为差异极大**（见 §3） |
| tools 集合 | 8 工具 × ~400 LOC | 6 工具 × ~300 LOC | ⚠️ 少 `ls`、`powershell` |
| compaction | 631 行 + 172 行 utils | 161 行 `compaction.rs` | ⚠️ nini 只有 LLM-free 本地压缩；Pi 有完整 LLM 压缩 + 切割点算法 |
| session 持久化 | JSONL v4 + 539 行 v3 兼容 | `session_state.rs` 150 行 | ⚠️ 见 §4 |
| skills | 509 行 | 200 行 | nini 实现存在但**部分未使用** |
| settings | 1371 行 | 99 行 | ⚠️ nini settings 是 1/14 的 Pi 功能 |

总规模 Pi ~6300 LOC；nini v0.8.5 总 Rust ~37000 LOC（含测试与文档）。

---

## 2. 三大架构根本差异

### 2.1 Agent 循环（双层 + follow-up）

**Pi**（`@earendil-works/pi-agent-core/dist/agent-loop.js`）：

```javascript
// 外层：处理 follow-up messages
while (true) {
    let hasMoreToolCalls = true;
    // 内层：处理当前 turn 的 tool calls + steering
    while (hasMoreToolCalls || pendingMessages.length > 0) {
        const message = await streamAssistantResponse(...);
        const toolCalls = message.content.filter((c) => c.type === "toolCall");
        if (toolCalls.length > 0) {
            // ★ 关键：length stop → 全部 tool_call args 可能被截断
            const batch = message.stopReason === "length"
                ? await failToolCallsFromTruncatedMessage(toolCalls, emit)
                : await executeToolCalls(currentContext, message, ...);
            hasMoreToolCalls = !batch.terminate;
        }
        pendingMessages = await config.getSteeringMessages?.();
    }
    // turn 结束了，但有可能 follow-up
    const followUpMessages = await config.getFollowUpMessages?.();
    if (followUpMessages.length > 0) {
        pendingMessages = followUpMessages;
        continue;   // 同一个 agent run 继续跑下一轮
    }
    break;
}
```

**nini**（`crates/nini-core/src/agent.rs:800-825`）：

```rust
loop {
    // 1. stream 一个 assistant response
    // 2. 串行/并行执行 tool_calls
    // 3. push 到 self.messages
    // 4. 下一轮 LLM 调用
    // 没有外层 follow-up loop
    // 没有 length stop 时的 tool_call 拒绝
}
```

**缺口**：
- nini 没有 **`failToolCallsFromTruncatedMessage`**。当 LLM 输出被 token 上限切断（`stop_reason: "length"`），该 turn 末尾的 tool_call 参数可能是 JSON 截断的（`{"command": "very long...` 突然断掉）。nini 会把它当成合法 JSON 解析，得到损坏参数。
- nini 没有 **follow-up messages 外循环**。Pi 可以让多个用户提问进入同一个 agent run；nini 一个 prompt = 一次 agent run。

### 2.2 Session 持久化（append-only commit graph）

**Pi**（`session/commit.ts` + `jsonl/storage.ts`）：

```typescript
export type CommittedWrite =
    | CommittedEntryWrite      // entry/compaction/branch_summary/custom
    | CommittedUsageWrite      // token 用量
    | CommittedValueSetWrite   // 命名空间 KV
    | CommittedValueDeleteWrite
    | CommittedListAppendWrite // 命名空间 list
    | CommittedListDeleteWrite;

// 每条 commit = 一行 JSON（一个对象或数组）
function serializeTransaction(writes: CommittedWrite[]): string {
    return JSON.stringify(writes.length === 1 ? writes[0] : writes);
}

// commit queue：所有 commits 串行化（不丢事件）
private commitQueue: Promise<void> = Promise.resolve();
async commit(writes: Write[], context: Context) {
    const result = this.commitQueue.then(() => this.applyCommit(writes, context));
    this.commitQueue = result.then(...);
}

// 验证：parent_id 必须已存在 + 单调 seq
export function validateCommittedWrites(writes, firstSeq, state) {
    let previousSeq = firstSeq - 1;
    for (const write of writes) {
        if (write.seq <= previousSeq) throw new Error("Non-monotonic storage sequence");
        if (write.parentId !== null && !state.hasEntryId(write.parentId)) {
            throw new Error("Missing parent entry");
        }
    }
}
```

**nini**（`crates/nini-session/src/`）：

```rust
// 仅 messages JSONL，按时间追加，没有 entry-level commit/seq/parent_id
// 没有 KV / list 命名空间
// 没有 torn-line 检测
// 没有 atomic publish（直接 fs::write）
```

**缺口**：
- 没有 **commit queue** 串行化 → 并发 commit 可能丢事件
- 没有 **parent_id 验证** → 损坏 JSONL 可能让 session 永久坏掉
- 没有 **torn-line 检测**（`splitCompleteLines`）→ 进程被 kill 时写到一半的行会让 reload 失败
- 没有 **atomic publish**（temp file + rename）→ crash 时可能产生空 session

### 2.3 系统提示词构造（tool-贡献式）

**Pi**（`system-prompt/system-prompt.ts`）：

```typescript
export function buildSystemPrompt(options: BuildSystemPromptOptions): string {
    // 1. 自定义 prompt（覆盖默认）
    if (customPrompt) { ... }
    
    // 2. 工具清单 + 工具 snippets
    const tools = selectedTools || ["read", "bash", "edit", "write"];
    const visibleTools = tools.filter((name) => !!toolSnippets?.[name]);
    const toolsList = visibleTools.map(name => `- ${name}: ${toolSnippets![name]}`).join("\n");
    
    // 3. 动态 guidelines（基于已注册工具集）
    if (hasBash && !hasGrep && !hasFind && !hasLs) {
        addGuideline("Use bash for file operations like ls, rg, find");
    }
    // ... 总是追加：
    addGuideline("Be concise in your responses");
    
    // 4. Pi 文档指针（README、docs/、examples/）
    // 5. project_context files（<project_instructions path="..."/>）
    // 6. skills（<available_skills><skill><name/><description/><location/></skill></available_skills>）
    // 7. cwd
    
    return prompt;
}
```

**nini**（`crates/nini-cli/src/prompt_setup.rs`）：

```rust
let mut s = String::from("You are nini, a Pi-compatible Rust coding agent.\n");
s.push_str(&format!("\nWorking directory: {}\n", cwd.display()));      // ✅ cwd
s.push_str("All relative paths in tool calls are resolved against this directory.\n");
if let Some(p) = &settings.provider { s.push_str(&format!("Default provider: {p}\n")); }
if let Some(m) = &settings.model { s.push_str(&format!("Default model: {m}\n")); }
s.push_str("\nAvailable tools: bash, read, write, edit, grep, find. \
           Use them to complete complex multi-step tasks.\n");
s.push_str(skills_prompt);
```

**致命差距**：
1. **工具清单是硬编码字符串** ❌。nini 的 6 个 tool **都已经实现了** `fn system_prompt_contribution() -> Option<ToolSystemPrompt>`（每个工具返回自己的 snippet + guidelines），但 `prompt_setup.rs` **根本没调用它们**！nini-core 里有一个 `build_system_prompt_with_contributions()` 函数（`tool.rs:431`）会收集所有工具的贡献并格式化进 system prompt，**但 CLI/TUI 也从未调用它**。
2. 没有 Pi 文档指针（用户问"pi 自己怎么用"时模型无法自答）。
4. 没有 `<project_context>` 块。

---

## 3. ⚠️ 立即可修的 bug（v0.8.5 已识别但未修）

### Bug 1 — system_prompt_contribution 完全未使用

`crates/nini-core/src/tool.rs:431` 有现成的 `build_system_prompt_with_contributions()`：

```rust
pub fn build_system_prompt_with_contributions(
    base: Option<&str>,
    registry: &ToolRegistry,
) -> Option<String> {
    // 收集每个 tool 的 snippet + guidelines
    // 格式化输出：
    //   ## Tool self-descriptions
    //   - bash: Execute shell commands...
    //   - read: Read a file's contents...
    //
    //   ## Tool usage guidelines
    //   - bash: Default for any shell operation...
    //   - read: Always read a file before editing it...
}
```

每个工具的 `system_prompt_contribution()` 都实现了（bash/read/write/edit/find/grep 都有），还有专门的 `bash_tool_contribution_lands_in_system_prompt` 测试（`tool.rs:705`）。**但 `prompt_setup.rs` 和 `app.rs` 从未调用它**。

**修复**（3 行）：
```rust
// crates/nini-cli/src/prompt_setup.rs::settings_to_system_prompt
let tools = tool_registry::build_tools();  // 或注入
let base_prompt = nini_core::build_system_prompt_with_contributions(
    Some(&base_prompt),
    &tools,
).unwrap_or(base_prompt);
```

### Bug 2 — length-stop 时不拒绝被截断的 tool_call

LLM 因 token 上限被切断时，assistant message 末尾的 tool_call JSON 可能是截断的（`{"command": "very long...`）。nini 当前会把它当成合法 JSON parse，结果是 `serde_json::Value::Null`，工具执行得到空参数。

**修复**（agent.rs 内 ~10 行）：
```rust
// 在 tool_execution loop 之前
if message.stop_reason == "length" && !tool_calls.is_empty() {
    for tc in &tool_calls {
        yield AgentEvent::ToolResult {
            id: tc.id.clone(),
            output: ToolOutput::err(
                "[reject] tool call truncated due to length stop"
            ),
        };
    }
    continue;
}
```

### Bug 3 — settings 字段缺失

Pi 的 `Settings` 有 ~50 个字段（terminal、theme、packages、externalEditor、shellPath、tuiMode、treeFilterMode 等）。nini 只有 ~6 个。差距巨大，但每次加一个都是 feature。

---

## 4. 其他可学习但工作量大

| 项 | Pi 复杂度 | nini 价值 | 估时 |
|---|---|---|---|
| `ls` 工具 | 150 LOC | 让模型不再用 bash ls，更安全 | 1 天 |
| `powershell` 工具 | ~300 LOC | Windows 用户 | 1 周 |
| LLM-based compaction | 170 LOC + 重写 agent | 长会话不丢上下文 | 1 周 |
| Branch summary | 173 LOC | `/tree` 已存在，可升级 | 3 天 |
| Package manager (npm/git sources) | ~600 LOC | 用户安装 skills/themes/extensions | 2 周 |
| Custom prompts (用户可覆盖 system prompt) | ~150 LOC | 与 Pi `customPrompt` 兼容 | 3 天 |
| Project context files (`<project_context>`) | ~80 LOC | 注入项目 AGENTS.md 等 | 1 天 |
| Skill location in prompt (`<skill><location>file</location>`) | 30 LOC | 模型能用 `read` 实际加载 skill 内容 | 1 小时 |
| `enableSkillCommands` (`/skill:name`) | ~200 LOC | skill 注册为 slash 命令 | 2 天 |
| `withFileMutationQueue`（并发文件编辑串行化） | ~80 LOC | 防止并发 edit 冲突 | 1 天 |
| Fork session | ~400 LOC | 用户可以 fork 历史 | 1 周 |
| Tree filter mode | ~150 LOC | `/tree` 视图增强 | 3 天 |
| HTTP/WebSocket idle timeout | ~30 LOC | 防止 hang 请求 | 半天 |
| Markdown mermaid 渲染 | ~400 LOC | TUI 渲染 mermaid | 1 周 |

---

## 5. 数据流对比图

```
用户输入 → submit_user_input → Agent.run()
                                    │
                                    ├─→ provider.stream() ←──── Anthropic/OpenAI/...
                                    │       │
                                    │       ▼ events (TextDelta, ToolCallStart, ...)
                                    │
                                    ├─→ AgentSink::push()   ←─── runtime loop 拉取
                                    │       │
                                    │       ▼ AppState::clone → render_frame
                                    │
                                    ▼ ToolExecution loop (v0.8.5: FuturesUnordered)
                                            │
                                            ├─→ BeforeExecute hook (TrustStore etc.)
                                            ├─→ Tool::execute() → ToolOutput
                                            └─→ AfterExecute hook (redact/rewrite)
```

**主要差异点**：

1. **nini 用 Arc<Mutex<AppState>> 共享 state**——runtime loop 每帧 clone 整个 state（含 transcript）。v0.8.5 已用 Arc<TranscriptState> 缓解。
2. **nini 的 AgentSink::push 单锁双锁合并**（v0.8.5 已修）。
3. **nini 的 sink_notify 已用 tokio::sync::watch**（v0.8.5 已修）。
4. **nini 的 tool_call 并行执行**（v0.8.5 已用 FuturesUnordered）。

---

## 6. 立即可学可做的 3 件事

| 优先级 | 任务 | 行数 | 影响 |
|---|---|---|---|
| **🔴 P0** | 修 Bug 1（wire up `build_system_prompt_with_contributions`） | ~10 | 让模型看到每个 tool 的 snippet，决策更准 |
| **🔴 P0** | 修 Bug 2（length-stop 拒绝截断 tool_call） | ~15 | 防止 agent 收到损坏参数 |
| **🟡 P1** | skill prompt 输出 `<skill><location>file</location>` | ~10 | 让模型能实际读 skill 内容 |
| **🟡 P1** | 加 `ls` 工具 | ~150 | 模型不再需要 bash 列目录 |

`build_system_prompt_with_contributions` + `length-stop` 是真正的 bug，且修复成本极低，应该立即做。