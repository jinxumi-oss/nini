# 文本式工具调用的兜底 —— 让「模型把工具调用写进正文」不再静默卡死

Status: **已交付（2026-11-21）**

## 症状（用户截图）

模型在同一段对话里反复发同一条命令，每发一次就自言自语一次「上一条工具调用没有返回可见结果」，
用户只能不停发 `?` 催它。屏幕上**没有任何错误、没有任何提示**，只有「完全没动静」。

## 根因（可证伪）

1. **模型侧**：provider 侧的模型（Qwen / MiniMax / 讯飞星辰这一族）会**随机**把工具调用写进正文，
   而不是放进结构化的 `tool_calls` 字段：

   ```text
   <function=bash>
   <parameter=command>cargo test -p nini-conformance --test custom_tool_end_to_end</parameter>
   </function>
   ```

   证据：会话 `~/.nini/agent/sessions/nini/20261119-181606_13b7f0639.jsonl` 第 231-241 条 ——
   助手消息里只有这段文本，没有结构化 `toolCall` 块。

   用**真实 endpoint** 回放同一上下文（`https://maas-api.cn-huabei-1.xf-yun.com/v2/chat/completions`，
   模型 `spark-x2.6`）：同一个请求连发 3 次，结果是
   `结构化 tool_calls ✓ / 文本式 ✗ / 文本式 ✗` —— **模型侧随机降级**，与历史内容无关
   （把摘要换短、把摘要删掉，行为依旧摇摆）。

2. **nini 侧**：文本式调用此前**零处理**。`agent.rs` 只认 provider 的结构化
   `StreamEvent::ToolCallStart/Stop`，正文里的标记被当散文并进 `assistant_text`，
   于是：
   - 工具**不执行** → 模型等一个永远不会来的 `tool_result`；
   - 模型下一轮看到自己那段正文，于是**重发同一条命令**；
   - 全程**没有任何错误或提示** → 用户侧的观感就是「静默中断」。

3. 这与 v1.0.3 之前修的「无 Done 静默冻结」（`8884998` / `040e2d4`）**不是同一条路径**：
   那条是 agent 任务异常结束、watchdog 推不出终止通知；这条是**工具结果从未产生**。

## 修复（最小改动）

- **新增 `crates/nini-core/src/text_tool_call.rs`**：纯函数解析器（不碰状态、不执行任何东西）。
  认得闭合的 `<function=NAME>` + `<parameter=KEY>VALUE</parameter>` 块；
  闭合标记兼容 `</function>` / `</arg_value>` / `</tool_call>`（带零宽空格的变体）。
- **接线（`crates/nini-core/src/agent.rs`）**：当这一轮**没有**结构化 tool_calls 时，
  把正文里的标记还原成真正的调用：
  - **只认已注册的工具**（正文里提到一个不存在的工具名 → 不执行）；
  - 事件序列与结构化调用完全一致（`ToolCallStart` → `ToolCallStop` → 工具执行 → `ToolResult`），
    TUI 转录与 session 的 `tool_use`/`tool_result` 配对因此照旧成立；
  - 合成 id 前缀 `call_text_`（`TEXT_TOOL_CALL_ID_PREFIX`），唯一且不与 provider 的 id 冲突；
  - 标记从**模型上下文**里摘掉（它是工具调用，不是正文；留着模型下一轮会照着它再写一遍）；
  - **认不出来时说出来**（`AgentEvent::Error` → 转录里的 `[error] …`），不再静默。
- **会话侧（`crates/nini-tui/src/state.rs`）**：文本式调用那一轮，标记**不落库**——
  工具调用本身已按结构化形式记着（`toolCall` + `toolResult` 配对），留着标记会让
  `/resume` 把它带回模型上下文，活着的会话与恢复出来的会话讲的不是同一段历史。

## 安全边界（review 提出的三条，收紧后落在这三条上）

1. 必须**闭合**才认；没闭合的当文案。
2. 工具名必须**命中已注册工具**，否则不执行。
3. 参数值里含 `<` 不影响解析（值到下一个参数/闭合标记为止）。

## 与 Pi 0.85.1 的关系（偏差声明）

Pi **没有**这个兜底：`references/pi-0.85.1/src/loop/agent-loop.ts:222` 只消费结构化的
`toolCall` 块，`references/` 全量 grep 无文本式解析代码。因此这是**超出 Pi 的扩展**，
不是 Pi 的等价行为——目的是让 nini 在真实 provider 的随机降级下**不停摆**。

## 验收

- `cargo test -p nini-core --test text_tool_call_fallback` → 3 passed（工具真的执行了 /
  标记没进下一次请求 / 认不出来时必须报错）
- `cargo test -p nini-core text_tool_call` → 6 passed（解析器单测）
- `cargo test -p nini-tui --test p01_persist` → 7 passed（含会话侧「标记不落库」）
- 全工作区 `cargo test --workspace` → 2349 passed / 0 failed
- `bash scripts/gate.sh` → 12 通过 / 0 失败 / 4 跳过
- **真实 provider 端到端**：`nini -p "用 bash 工具执行：echo hello-from-nini-fixed"`
  （xfyun / spark-x2.6）→ 命令执行、结果回来。
- **TUI 端到端**（脚本化 fixture 喂一个文本式调用）：转录里出现 `▸ bash {...}` 与工具输出，
  会话里落的是 `call_text_0`（而不是标记原文）。

## 复现

- 复现命令: `cargo test -p nini-core --test text_tool_call_fallback` → 退出码 0（修复后绿）
- 修复前：同样的输入下工具**不执行**，模型重发同一条命令，转录里没有任何错误。
