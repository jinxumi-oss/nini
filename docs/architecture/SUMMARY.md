# nini 架构总结与评估（v0.8.0-pre1, 2026-09-25）

## 一图看懂：四个 Mermaid 图

1. **`nini-crate-deps.mmd`** — 7 个 crate 的依赖图（核心：`nini-core` 是叶子，被其他 6 个 crate 反向依赖）
2. **`nini-tui-internals.mmd`** — TUI 36 个模块的内部结构（标注 LOC + import 频率）
3. **`nini-agent-flow.mmd`** — 运行时数据流时序图（user input → TUI → Agent → Provider → Tool）
4. **`nini-hook-system.mmd`** — `AgentLoopHooks` trait 与 `Agent::run()` 中的 9 个 hook 调用点

## 数据快照

| 维度 | 数据 |
|---|---|
| 总 crate | 7 |
| 总模块 (.rs 文件) | 85 |
| 总 LOC（Rust） | ~36,000 |
| 测试数 | 714 通过 / 0 失败 |
| 第三方 crate | 326（transitive） |
| 直接第三方 | ratatui 0.29、crossterm 0.28、tokio、serde、reqwest |
| Provider 实现 | 11 个（anthropic、openai、openai_compat、openai_responses、google、cohere、mistral、groq、deepseek、fixture、fallback） |
| Tool 实现 | 6 个（bash、read、edit、write、find、grep） |
| Hook 方法 | 9 个 |
| 持久化格式 | JSONL（Pi 兼容） |

## 架构评估

### ✅ 优点

**1. 关注点分离干净**

`nini-core` 是真正的"无依赖叶子"——它只依赖 serde/tokio/reqwest 等基础库，**不依赖任何 nini 内部 crate**。
这意味着：
- 测试 `nini-core::Agent` 时不需要 ratatui / crossterm
- `nini-core` 可以独立打包给非 TUI 场景复用（如未来的 web 后端、IDE 集成）
- 重构 `nini-ai`（Provider 实现）不会影响 `nini-core` 的接口

**2. Agent-TUI 边界清晰**

`AgentEvent`（nini-core 域模型）→ `AgentEventLite`（nini-tui 扁平镜像）的转换**完全在 nini-cli 里发生**（main.rs 第 378-415 行）。
这意味着：
- TUI 永远不需要 import nini-core 内部的复杂泛型（`BoxStream`, `Result<...>` 等）
- 改 TUI 渲染不会污染 Agent 的语义
- 改 Agent 的内部事件流不会破坏 TUI（只要 main.rs 的转换匹配住）

**3. Pi 兼容的双层 hook 系统**

`AgentLoopHooks` trait 提供 9 个 hook，覆盖 Pi reference Python 的 hook #1-#9。
两个 chokepoint（`transform_context` + `convert_to_llm`）确保所有"消息进出 LLM"的路径都过同一个钩子——这是修复 ToolResult round-trip bug（v0.7.1）的关键设计。

**4. Trait-based 多态让扩展简单**

- 加新 Provider：写一个 `impl Provider for X`（11 个先例）
- 加新 Tool：写一个 `impl Tool for X`（6 个先例）
- 加新 Hook：写一个 `impl AgentLoopHooks for X`（nini-core 的 `NoopHooks` 提供默认实现）
- **0 修改 nini-core 主体代码**

**5. Fixture provider 让端到端测试无需 LLM**

`nini_ai::fixture::ProgrammedProvider` 用预设 JSON 喂流式事件，**完全脱离真实 API**。
v0.8 的 4 个新回归测试（tool selection + token counts）全跑 fixture，无需 API key。

### ⚠️ 风险与改进点

**1. `nini-tui/state.rs` 是单点瓶颈（1853 LOC，70 个 pub 方法，13 个 importer）**

```
★ state.rs (1853) AppState hub
  ★ 70 pub methods
  ★ 13 importers
```

`AppState` 字段 50+，耦合了输入缓冲、转录文本、模式、模型、token、provider、context、cost、git、theme、session、settings、abort_signal、selector、help、search、completion、scroll、palette、quit、trust 等**所有 UI 状态**。

**后果**：
- 修改 AppState 字段影响所有 13 个 importer
- v0.8 的 3 次字段新增（`provider`、`thinking_level`、duration_ms 透传）都触发了**6+ 个文件的修改**
- 单测覆盖率压力集中在 state.rs

**建议**：
- 把 `AppState` 拆成 `InputState`、`TranscriptState`、`RunState`、`SessionState`、`UiState` 5 个 sub-struct，组合而不是单体
- 或者用 ECS-style 把 transcript 抽成独立类型

**2. `nini-tui/runtime.rs` 与 `state.rs` 体量并列，但耦合极不对称**

```
runtime.rs (1854) ──reads──► state.rs (1853)
runtime.rs ← 0 importers (only via lib.rs::run)
```

`runtime.rs` 是事件循环的"权威"，但 `state.rs` 是**唯一被改的**——`runtime.rs` 几乎从不独立变化。
**建议**：把 `runtime.rs` 中**纯函数式**的部分（输入处理、键映射）拆到 `input.rs`，留下真正异步的部分。

**3. `nini-cli/main.rs` ~~是单文件 1636 LOC 的"上帝函数"~~ (v0.8.2 已拆分)**

`main()` 里塞了：
- CLI 解析（`-p`、`demo`、`info`、`-h`）
- Provider 构建（11 种分支）
- ToolRegistry 构建（6 种工具）
- TUI bootstrap（导入设置、skill、git branch）
- Agent 驱动（4 种调用模式：TUI、demo、single-shot、test）

**后果**：
- v0.8 改 `state.provider` 字段时，main.rs 第 472 行需要新增 4 行 setter 代码——这种"集中化粘合"是不可持续的
- 无法单独测试某个分支（比如 fixture provider + 单 shot 模式）

**建议**：拆成 `app.rs`（应用工厂）、`bootstrap.rs`（状态初始化）、`provider_factory.rs`（按 CLI flag 选 provider）。

**4. 缺乏 trait bound 文档**

`Agent::new(provider, tools, cfg)` 的参数看起来简单，但：
- `provider: Arc<dyn Provider>` 必须 Send + Sync
- `tools: ToolRegistry` 必须能 `.register(Arc<dyn Tool>)`
- `cfg: RunConfig` 包含 model、thinking_level、retry 等

这些 trait bound **没有 doc-comment 说明**，新加 Provider 时容易踩坑。

**建议**：在 `Provider` trait 上加 `# Errors`、`# Thread Safety` 段落。

**5. Hook panic 隔离 vs hook panic 暴露不对称**

```rust
pub fn catch_hook_panic<F, R>(label: &'static str, f: F) -> R
```

`agent_hooks.rs` 提供 panic catcher，但**只在部分 hook 上调用**（grep 显示 9 个 hook 方法里只有 3-4 个受保护）。其余 hook 触发 panic 会**直接杀掉整个 TUI**。

**建议**：要么全 hook 走 `catch_hook_panic`，要么明确文档"哪些 hook 是隔离的"。

**6. 缺少性能埋点**

`AgentEvent::TurnEnd { usage, .. }` 携带 token 计数，但**没有**：
- 每个 hook 的耗时
- Tool 执行的 P50/P95
- Provider 流的 inter-token 延迟
- TUI 渲染的 FPS

这对 Pi 这种 TUI 来说不重要，但 nini 的 token 已经在累积了——加 `metrics` crate 输出 P95 几乎零成本。

**建议**：在 `RuntimeMetrics` struct 里追踪这些，dump 到 session 文件末尾。

## 对标 Pi

| 维度 | Pi (reference) | nini | 差距 |
|---|---|---|---|
| Hook 数 | 9 | 9 | ✅ 100% |
| Provider 抽象 | ✅ | ✅ | 同 |
| Tool 抽象 | ✅ | ✅ | 同 |
| 持久化 | JSONL | JSONL | ✅ 同 |
| TUI 渲染 | custom (pi-tui) | ratatui | ⚠️ nini 落后 |
| Slash 命令 | 50+ | 12（estimate） | ⚠️ |
| Skills 系统 | loader + remote fetch | loader only | ⚠️ |
| Extension 系统 | TS + Rust | Rust only | ⚠️ |
| Setting UI | in-TUI selector | in-TUI selector | ✅ |

**v0.8.3 完成**：commands.rs (1833 LOC, 28 个 slash 命令 + 1 个 1200 LOC dispatch() 函数) 拆为 7 个子模块（registry/result/parse/status_lines/html_export/dispatch/mod）。测试 739 → 760 (+21)。

**最大的架构差距**：nini 没有 Pi 的 **prompt template** 概念。Pi 有 `/implement`、`/scout-and-plan` 这种**带技能的 prompt 模板**，nini 只有 `/help` `/quit` 这种**纯命令**。

## 下一步建议（按 ROI 排序）

1. **拆分 `AppState`**（高 ROI）—— 一次大重构，但 v0.8 那种"加字段改 6 文件"的痛苦会消失
2. **拆分 `main.rs`**（中 ROI）—— 改 `app.rs` + `bootstrap.rs` + `provider_factory.rs`，新增 CLI 模式不再痛苦
3. **添加 prompt templates**（中 ROI）—— 让 nini 拥有 Pi 的 `/implement` 等高级命令
4. **性能埋点**（低 ROI 但高价值）—— `RuntimeMetrics` + session dump
5. **Hook panic 全覆盖**（低 ROI）—— 一次小重构消除潜在崩溃
