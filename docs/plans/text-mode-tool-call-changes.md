# Changes

- `crates/nini-core/src/text_tool_call.rs` | **新增**纯函数解析器：认得闭合的
  `<function=NAME>` + `<parameter=KEY>VALUE</parameter>` 块（闭合标记兼容 `</function>` /
  `</arg_value>` / `</tool_call>`），产出工具名 + 参数对象 + 该块在原文里的字节区间；
  另提供 `has_tool_call_marker`（有标记但认不出来时用它区分）与
  `strip_text_tool_calls`（把标记从正文里摘掉）。**不碰状态、不执行任何东西**。
- `crates/nini-core/src/lib.rs` | 导出 `pub mod text_tool_call;`。
- `crates/nini-core/src/agent.rs` | 收集完 tool_calls、准备追加 assistant 消息的位置接线：
  这一轮**没有**结构化 tool_calls 时，把正文里的文本式调用还原成真正的调用——
  只认**已注册**的工具；事件序列与结构化调用一致（`ToolCallStart`/`ToolCallStop`，
  id 前缀 `call_text_`）；标记从模型上下文里摘掉；**认不出来时推 `AgentEvent::Error`**
  （转录里的 `[error] …`），不再静默。
- `crates/nini-tui/src/state.rs` | `PendingAssistant` 增 `text_tool_call` 标志：
  文本式调用那一轮，标记**不落库**（工具调用已按结构化形式记着），
  `/resume` 不会把它带回模型上下文。
- `crates/nini-core/tests/text_tool_call_fallback.rs` | **新增**回归测试 3 条：
  ① 正文里的工具调用**真的执行**（工具跑了、结果回来了、这一轮继续）；
  ② 标记**不进入下一次请求**（它是工具调用，不是正文）；
  ③ 认不出来的标记**必须报错**（未注册工具不执行，但要有可见说明）。
- `crates/nini-core/src/text_tool_call.rs`（`mod tests`）| **新增**解析器单测 6 条：
  规范写法、多参数、三个闭合标记变体、未闭合块不算调用、散文不算调用、摘除标记、
  参数值里含 `<` 不受影响。
- `crates/nini-tui/tests/p01_persist.rs` | **新增**会话侧用例 1 条：
  文本式调用落库时是**结构化的 toolCall + toolResult**，标记不落进正文。
- `README.md` / `README.zh-CN.md` / `CHANGELOG.md` | 对外声明的测试数同步到实测 **2349**
  （门禁 T4 要求精确相等）。

## Commands

- `cargo test -p nini-core --test text_tool_call_fallback` → 退出码 0（3 passed）
- `cargo test -p nini-core text_tool_call` → 退出码 0（6 passed）
- `cargo test -p nini-tui --test p01_persist` → 退出码 0（7 passed）
- `cargo test --workspace` → 退出码 0（2349 passed / 0 failed）
- `bash scripts/gate.sh` → 退出码 0（12 通过 / 0 失败 / 4 跳过）
- `cargo fmt --all` → 干净；`cargo clippy --workspace --all-targets -- -D warnings` → 0 警告

## Reproduction

- 复现命令: `cargo test -p nini-core --test text_tool_call_fallback` → 退出码 0（修复后绿）
- 真实 provider 端到端: `NINI_PROVIDER=xfyun NINI_MODEL=spark-x2.6 nini -p "用 bash 工具执行：echo hello-from-nini-fixed"`
  → 命令执行、结果回来（修复前：模型把调用写进正文 → 工具不执行 → 模型重发同一条命令）。
- TUI 端到端: 脚本化 fixture 喂一个文本式调用 → 转录出现 `▸ bash {...}` 与工具输出，
  会话里落的是 `call_text_0`（不是标记原文）。

## Blocked
none
