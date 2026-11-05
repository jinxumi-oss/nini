# R（resume）：`/resume <n>` 重建 transcript 的三处失真，以及提示行写进 session

## 实证（探针 `crates/nini-tui/tests/zz_probe_resume.rs`，2026-11-04，改动前实测）

| 探针 | 实证 | 定性 |
|---|---|---|
| P1 | `/resume 1` 后 transcript = `USER / ASSISTANT / DIV`。session 文件里 user+assistant+toolCall+toolResult 共 4 条 entry，transcript 只重建出两条纯文本：**ToolCall / ToolResult / 非文本块全部丢失** | 真缺陷 |
| P2 | `(loaded session <id>)` 用 `push_assistant` 写进 **session 文件**（4 行 → 5 行）。它下次 resume 会作为假 assistant 消息**再次出现在历史里**——每次 resume 积累一条 | 真缺陷（比探针注释说的"消息重复"更严重：重复不成立，session 内容污染成立） |
| P4 | 空 live + `/resume` → `USER / ASSISTANT / DIV`。"USER/ASSISTANT" 就是刚 resume 出来的历史，**没有独立幽灵行**；幽灵是 session 文件里那条假 assistant。另：`(loaded …)` 在 transcript 里**粘连进最后一条 assistant 文本**（`push_assistant` 的 same-turn 合并）——提示不属于自己的 turn | 真缺陷（提示的 turn 归属），探针 P4 的措辞需修正 |
| P3 | `/session` 不显示 `session_path`（事实补全，一行） | 顺手 |
| P5 | F1 session 选择器只列 cwd 子目录，`/resume` 列递归集 | 已知，本轮不动（不同列表是有意的还是 bug 需要 owner 语义，登记台账） |

**探针注释修正（写进回归测试的文档注释）**：P2 说"每条消息重复两遍"不成立——重建循环期间 session 被 `.take()` 掉了，`push_user/push_assistant` 只写 transcript 没写 session。真实后果是 ① transcript 只重建出文本、② 提示行永久污染 session 文件。

## 根因

`cmd_resume` 的重建循环是 **A4 之前的残留**。A4（6683f36）给 `session_load` 加了
`transcript_from_context()`——它渲染的是 **live context**（工具、bash、错误轮、
compaction 摘要——`render_context_entries`），这才是 agent 下一轮真正看到的东西。
`/resume` 的手写循环只重建了 legacy 文本，把 transcript **覆盖成了一份更差的副本**：
文本全对（A4 测试因此没抓到），工具全丢，提示行写进 session。

## 计划

1. **删重建循环**。`session_load(path)` 失败即报错返回；成功即完成（A4 保证它
   调了 `transcript_from_context`）。删 40 行手写循环 + `entry_legacy_message`
   的调用点（函数本身留着——conformance 在用它，grep 确认再动）。
2. **skipped 提示改 `push_stop_notice`**（A4/R2h 注释说的"内联在内容旁边"——
   StopNotice 正是为此设计的变体；`push_assistant` 把它变成了模型消息）。
3. **`(loaded session <id>)` 改 `push_notice`**（TUI 自己说的话，不进 session——
   v0.10.3 注释的语义），且放在 transcript 内容**之前**（`push_notice` 直接
   push，不受 same-turn 粘连影响）。
4. **`/session` 补 `session_path` 行**。
5. **回归测试 `crates/nini-tui/tests/resume_rebuild.rs`**（先在 HEAD 上跑红，再修到绿——
   测试必须抓得住旧行为）：
   - R1：resume 后 transcript 有 ToolCall/ToolResult（变异：换回手写循环 → 红）
   - R2：resume 后 session 文件与 resume 前**逐字节相同**（变异：提示写 session → 红）
   - R3：空 live + resume → transcript 以 `Notice("(loaded …)")` 开头、无 phantom；文件零新增
   - R4：skipped>0 → transcript 有 `StopNotice`
6. 删探针；修 runtime.rs 的 fmt 漂移（既有改动，非本轮）；README 徽章/正文/
   CHANGELOG 首段测试数精确同步。
7. 台账 `scripts/parity-gaps.json` 登记 **B26**（open，oracle 指向新测试，exit_criterion
   写机器判据）；GOALS.md 台账摘要行 + CHANGELOG Unreleased 段同步。
8. 提交 + `bash scripts/gate.sh` 全绿。

## 不做的

- P5（选择器目录范围）——登记台账备注，语义由 owner 定
- `entry_legacy_message` 函数本身——conformance 在用
- 改门禁、改 `render_context_entries`（它是对的）
