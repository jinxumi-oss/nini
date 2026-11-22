# 子代理系统 —— 现状调研报告

日期：2026-11-22
对象：nini（Pi 0.85.1 的 clean-room Rust 重写，v1.0.4，8 crate workspace）

> **战略前提（2026-11-22 owner 决策）**：自 **v1.1** 起，nini **脱离 Pi 0.85.1 对标，自行进化发展**。
> 本报告据此口径撰写：Pi 快照不再是设计目标与正确性判据，子代理作为 nini 原生能力自由设计。
> 决策全文见 `docs/plans/v1.1-independence-decision.md`。

## 1. 结论摘要

**nini 当前完全没有子代理（subagent）能力，Pi 0.85.1 也没有。**

- 文档实证：`docs/quality-left-shift/functional-map.md:20`、`functional-verification-checklist.md:4`、
  `docs/plans/quality-left-shift-feasibility-plan.md:106/128-131` 均把「无 subagent」登记为已知缺口。
- 参考实现实证：`references/pi-0.85.1/tool-contracts.json` 只有 8 个内置工具
  （bash / edit / find / grep / ls / powershell / read / write），无 `spawn`/`delegate`/`agent` 类工具；
  `references/pi-0.85.1/src/` 全量 grep `subagent|spawnAgent|delegate` 只有 find/grep 注释里
  "delegate search to remote systems" 的无关字样。
- **v1.1 起脱离 Pi 对标（见 `docs/plans/v1.1-independence-decision.md`）**：子代理不再是
  「nini 没有而 Pi 有/没有的对齐项」，而是 **nini 原生的自主进化能力**——
  无对齐负担、无参考实现可抄，按 nini 自身需求自设计自实现，验收判据为 nini 自建 oracle。

## 2. 现有架构（子代理要接入的关键点）

### 2.1 Agent 核心（`crates/nini-core/src/agent.rs`，5830 行）

- `pub struct Agent`：持有 `Arc<dyn Provider>`、`ToolRegistry`、`messages: Vec<Message>`、
  4 个 `AbortHandle`（通用 abort / 手动压缩 / 溢出压缩 / branch summary）、`RunConfig`、
  `hooks: Arc<dyn AgentLoopHooks>`、`last_usage` 等。
- `Agent::run(user_msg) -> Pin<Box<dyn Stream<Item = Result<AgentEvent, AgentError>> + Send>>`
  —— **单层折叠循环**（Pi 的两层嵌套循环被折叠为一个 loop + 守卫），事件流式产出。
- `AgentEvent`：`AgentStart / TurnStart / TurnEnd / PhaseChanged / TextDelta / ThinkingDelta /
  ToolCallStart / ToolExecutionStart / ToolCallDelta / ToolCallStop / Compacted / ToolResult /
  AgentEnd / Aborted / Error`。
- `AgentError`：`Provider / Tool / Aborted / TooManyIterations / RunawayGuard / RepeatedTruncation /
  InvalidState`。
- 构建入口：`Agent::new(provider, tools, config)` / `Agent::with_hooks(...)`；
  `seed(messages)` 从会话历史重建上下文；`set_compaction_hook(...)`。

### 2.2 工具层（`crates/nini-core/src/tool.rs` + `crates/nini-tools/`）

- `pub trait Tool`（async_trait）：`name() / spec() / execution_mode() / before() / after() /
  execute(args: Value, ctx: ToolContext) -> Result<ToolOutput, ToolError>`。
- `ToolRegistry`：`register / register_mut / get / specs / call`，可 clone。
- **`ToolContext` 只有三个字段：`cwd`、`max_output_bytes`、`max_output_lines`。**
  没有事件 sink、没有 abort、没有会话/历史句柄 —— 这是子代理工具落地的**核心约束**：
  子代理要跑自己的 Agent 循环、要上报事件、要能被取消，仅靠 `ToolContext` 不够，
  需要给工具执行注入更多能力（新上下文类型或 trait object 注入）。
- `ToolOutput`：`content / is_error / details / duration_ms / terminate`。
- 内置 7 工具（`crates/nini-cli/src/tool_registry.rs::build_tools`）：bash、read、write、edit、
  grep、find、ls（powershell 有意不做）。
- 工具执行在 agent.rs 循环内联：`FuturesUnordered` worker pool，`max_parallel_tools` 上限，
  顺序保持 tool_result 与 tool_use 同序；abort 时整个 run 取消，丢弃部分结果。

### 2.3 TUI 运行时（`crates/nini-tui/src/runtime.rs`）

- `pub type AgentDriver = Arc<dyn Fn(String, Vec<Message>, AgentSink, Arc<Notify>) -> JoinHandle<()>>`
  —— **每次提交消息都新建一个 Agent**（`app.rs:715`），从 session seed 重建历史，
  事件经 `AgentSink`（持 `SharedState` + watch notify + 轮次 seq）推入 TUI 状态。
- `AgentEventLite`（TUI 侧轻量事件）：TextDelta / ThinkingDelta / ToolCallStart /
  ToolExecutionStart / ToolCallStop / ToolResult / TurnEnd / TurnFailed / Error / StopReason /
  Usage / PhaseChanged / Aborted / Done。
- 看门狗：`WATCHDOG_TIMEOUT_SECS = 600`，超时只记日志并继续等同一任务（72h 长任务语义）。
- 渲染：`render.rs`（3717 行）+ `state.rs`（4268 行），转录区按工具结果渲染。

### 2.4 Provider（`crates/nini-core/src/provider.rs`）

- `pub trait Provider`：`stream(Request) -> Stream<Result<StreamEvent>>`，`Arc<dyn Provider>` 共享。
- `Request`：model / messages / tools / max_tokens / temperature / system / reasoning。
- `nini-ai` 实现多家 provider + fallback 链；fixture provider 供测试。

### 2.5 配置与设置

- `RunConfig`（每次 run 克隆）：model / system / max_tokens / max_iterations（0=不限）/
  runaway_turn_backstop / tool_context / compaction / max_parallel_tools / max_follow_up_rounds /
  tool_execution / reasoning 等。
- `Settings`（settings.json，serde，支持 camelCase alias，如 `maxIterations`）：TUI flush 时
  **merge** 保留用户不管理的键 —— 新增子代理设置键要沿用这个惯例。

### 2.6 其他相关面

- `mutation_queue`（nini-tools）：串行化文件写操作 —— 子代理若共享 cwd，写保护需考虑。
- `trust_gate::apply_project_trust`：项目信任闸在工具注册表组装后统一包裹 —— 子代理工具
  若复用主注册表则天然被闸；若自带工具集需重新过闸。
- `nini-ext`（扩展宿主）：**未接线且方案被认定错误**（Pi 扩展是 JS/TS，dlopen 无法加载），
  与子代理无关，不应复用其路线。
- 测试：2271 个测试；`nini-conformance` 以真实 Pi 源码为 oracle；`nini-tui/tests/` 大量 e2e。
  子代理无 Pi 对照，测试要自建 oracle/fixture。

## 3. 子代理升级的关键设计问题（待计划阶段决策）

1. **工具注入**：`ToolContext` 无 sink/abort/历史句柄。方案候选：
   a. 扩展 `ToolContext`（加可选的 `SubagentHandle`，向后兼容）；
   b. 子代理工具作为 `Arc<dyn Tool>` 注册进主注册表，通过构造时捕获的句柄（Arc 共享
      Agent 工厂 + 事件转发器）工作 —— 不动 ToolContext，工具自带能力。
   推荐倾向 (b)：工具自持句柄，核心 trait 零改动。

2. **子代理 Agent 的构建**：每次子代理调用新建独立 `Agent`（独立消息历史、独立 Provider
   复用、独立 RunConfig 派生），运行完把最终文本作为 `ToolOutput` 回传主循环。
   需要决定：历史是否保留（回传摘要即可，完整历史留在子代理内丢弃）。

3. **事件流**：子代理内部事件是否透传到 TUI 转录？候选：只回传最终结果（简单）；
   透传为缩进/标记的子事件（体验好但需新事件变体 + 渲染支持）。

4. **取消语义**：主循环 abort 应传播到子代理；子代理自身的看门狗/上限（max_iterations、
   runaway backstop、连续截断）需独立配置。

5. **并发与资源**：子代理内的工具调用仍走 `max_parallel_tools`；多个子代理并行时需要
   全局并发上限（防止 N 个子代理 × M 个 bash 的进程爆炸）—— 现有 `max_parallel_tools`
   只约束单 batch。

6. **信任与写保护**：子代理复用主注册表（继承 trust gate 与 mutation_queue）；若允许
   子代理自定义工具集，需明确闸的覆盖范围。

7. **配置面**：新增设置键（启用/禁用、默认并发、默认模型覆盖、超时、事件透传开关），
   沿用 settings.json merge 惯例与 CLI flag 习惯。

8. **测试策略**：脱离 Pi 对标后，conformance 层的 Pi fixture 不适用于子代理；
   用 fixture provider 驱动子代理端到端，自建契约测试（工具 schema、终止、取消、
   并发上限）+ 变异注入，验收判据是 nini 自建 oracle 而非 Pi 对照。

## 4. 建议的下一步

1. 基于本报告写升级计划（改动清单、文件分工、测试矩阵、验收标准），参照现有
   `docs/plans/` 计划文档的格式与 `gate.md` 门禁流程。
2. 计划中明确：子代理工具的命名（如 `task` / `agent` / `delegate`，v1.1 起按 nini 自身口径
   自主决定，无需为 Pi 拼写让路）、参数 schema、默认并发与上限、事件透传级别。
3. 按计划分阶段实现：核心（子代理工具 + Agent 派生执行）→ TUI（事件呈现 + 取消）→
   配置与测试收口。
4. 版本口径：本升级随 **v1.1.0** 开发，版本号与文档定位按
   `docs/plans/v1.1-independence-decision.md` 同步。
