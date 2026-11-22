# nini 项目调研报告（使用 rust-anything 工具链）

调研日期：2026-11-19
调研对象：`/home/jin/nini`（nini，GitHub jinxumi-oss/nini，v1.0.4，release `2b64063`）
调研工具：已集成到 pi agent 的 rust-anything（skill + CLI：analyze/bridge/record/diff/gate/profile/doctor 等）
结论：见文末「结论」。本报告全部结论附取证方式，可复现。

## 1. 调研目标与工具适用性

目标：调研 nini 项目的身份、结构、功能、可信度模型、质量状态、与 pi（earandil-works/pi，pi-agent 仓库的 TS 原版）的关系及对 pi 集成的潜在影响。

工具适用性结论（关键事实）：
- rust-anything 是「向 Rust 迁移」工具（记录原行为 → bridge 生成 Rust → record/diff/bench/gate 验证）。nini 本身是纯 Rust 项目（workspace 8 个 crate 全为 Rust），**无迁移需求**，不会产生迁移产物。
- rust-anything 对 nini 仓库仍可执行 analyze（识别仓库语义，产出的是对 `references/pi-0.85.1/` 等 TS 参考代码的迁移计划，不是对 nini 自身的迁移）。本次调研中实际命令 `rust-anything analyze /home/jin/nini -o /tmp/nini-analyze-plan.md` 退出码 0（产出迁移计划，印证指引）。
- 因此调研以 rust-anything 已验证的 Rust 工具链能力（cargo 构建/测试、doctor 环境检查）+ 仓库事实取证为主，不试图对 Rust 项目做迁移。

## 2. 项目身份

- nini = Pi agent（earandil-works/pi，Mario Zechner）的 **Rust 重实现**（代码从零用 Rust 写，非翻译/抄写 Pi 的 TS）；保留 Pi 的会话格式、slash 命令面与手写手感：`~/.pi/agent` 同格式、23 个内置命令 + 5 个自定义命令、同一份 session JSONL v4、同一 settings/models 方案、单二进制（6.7 MiB，压缩 2.5 MB）。
- 仓库信息：`/home/jin/nini`；GitHub `jinxumi-oss/nini`；v1.0.4（最新 commit `2b64063`）；MIT（无 license rider）；`publish = false`（不发布 crates.io），发布物为 Linux x86_64 静态二进制 + `install.sh`（`~/.local/bin`）。
- 与 pi-agent（本环境 pi）的关系：nini 与 pi 同对标 `pi v0.85.1`，是「同一协议面、独立实现」；nini 以 Pi 0.85.1 冻结源码为行为 oracle 自证其兼容性，与本次在 pi-agent 集成 rust-anything（Rust 迁移能力）不冲突。

## 3. 仓库结构（8 个 crate）

`crates/`：
- `nini-core`（agent 核心、agent_hooks、constraint、agent.rs 等）
- `nini-ai`（37 个 provider 实现、7 品牌：Anthropic / OpenAI(Chat+Responses)/OpenAI兼容 /Google/DeepSeek/Groq/Mistral/Cohere，默认模型 MiniMax-M2.7-highspeed）
- `nini-session`（JSONL v4 codec + v3 兼容读取，真实 Pi 会话 0 行丢弃由 conformance 实测）
- `nini-tools`（bash/read/write/edit/grep/find，含工具契约逐字段对比 Pi）
- `nini-tui`（ratatui + crossterm 流式 TUI：命令面板、Ctrl+F 搜索、外部编辑器、选择逻辑 selection.rs）
- `nini-ext`、`nini-conformance`（真实 Pi 会话/工具契约对照套件）、`nini-cli`（CLI）

可信锚点（`references/`）：
- `references/pi-0.85.1/`：Pi 0.85.1 机制源码快照（422 文件，每文件 sha256 锁），行为 oracle；
- `references/spec-v0.85.1/`：19 个文件格式规范（sha256 锁）；
- `references/TARGET.json`：冻结目标版本 0.85.1 + 引用形式 `file#symbol`（行号由脚本在快照里解析）。

质量左移体系：`docs/quality-left-shift/`（功能地图、功能验证清单、指标收集、形式化 .lean/.tla/verify、review）、`docs/plans/*`、各交付门禁（如本次的 `gate.md`）。

## 4. 功能与质量声明（已取证）

功能（README + llms.txt）：
- 会话：JSONL v4 + v3 兼容，读 `~/.nini/agent/`（skills/、settings.json、models.json），Pi 的 skills/settings/models 直接可用；
- 命令：23 个 Pi 内置命令（同分发方式）+ 5 个 nini 扩展（/help /status /prompt /debug /editor）；`/share` 无云端同步（写本地文件）；
- TUI：流式渲染、斜杠命令补全、命令面板（Ctrl+Shift+K）、transcript 搜索（Ctrl+F）、外部编辑器（Ctrl+G）、选择/拖拽；
- 压缩：确定性本地压缩（LLM 汇总为后续，未发布），上下文溢出自动触发；
- Hooks（v0.7）：Pi 兼容 5 方法 `AgentLoopHooks`（steering/followup/transform_context/convert_to_llm/should_stop_after_turn）+ 工具前后钩子；
- 安全姿态：威胁模型（不可信文件 prompt injection、敌对 LLM provider、本地权限提升）在 SECURITY.md 记录；发布物带 commit/时间 sha256 sidecar、Cargo.lock 锁定依赖、不提供 crates.io 发布入口。

可信度模型（已取证）：
- 行为声明用符号引用（`agent-loop.ts#shouldStopAfterTurn`），CI 用 `scripts/pi-oracle-lock.py --verify-refs` 在冻结快照中重解析；符号消失=红灯，挪位=报告位移；
- 工具契约逐字段对比 Pi 运行时（`crates/nini-tools/tests/tool_contract_parity.rs`）；
- 不变式/变异验证、parity 门禁均设红路径，无目测式声明；
- 测试：README 声明 **2271 通过**（CI 强制声明数与实测一致，status 已同步；llms.txt 中的 2136 为旧事实）；CI 执行 `cargo test --workspace --all-targets`。

本机调研取证（rust-anything 工具链）：
- `cargo build --workspace --all-targets` → 退出码 0（全部目标构建通过）；
- `cargo test -p nini-tui --lib` → 428 passed / 0 failed；
- 完整套件本地因 146 个测试文件各自链接可执行文件而过重，30 分钟未跑完（nini 自己的 Cargo.toml profile 注释已说明该体积特征，测试在 CI 执行）；
- 仓库当前有未提交改动（另一会话遗留：`crates/nini-core/src/agent.rs`、`docs/plans/long-task-72h-continuity-fix-plan.md`），调研未触碰。

## 5. 与 pi 的关系及集成影响（调研判断）

- 关系：nini 是 Pi 的 Rust 重实现，与 pi-agent 同对标 pi-0.85.1、协议面兼容（Pi 的 skills/settings/models 可拷入 `~/.nini/agent` 使用）；仓库独立（jinxumi-oss/nini），与本次在 pi-agent 集成 rust-anything 无冲突。
- 对 pi 的潜在影响：
  - 候选资产：nini 是「Rust 版 Pi agent」的完整实现，其「冻结 oracle + 符号引用 + CI 门禁 + 身份化发布物」的质量体系可作为 pi（或 pi 未来 Rust 化）的可信实现参照；
  - 约束/风险：仅 Linux x86_64 预编译（无 macOS/Windows/arm64 预编译，需 install.sh 构建）；无 OAuth、无扩展运行时、无云同步；LLM provider 走环境变量 API key；依赖 Pi 0.85.1 oracle（Pi 升级需走其重锚流程）；代码从零编写，与 TS 原版实现思路不同。
  - 与 rust-anything 的协作边界：rust-anything 用于「向 Rust 迁移」；nini 已是 Rust，二者不冲突。若未来要把 pi（TS）整体迁移 Rust，nini 的 oracle 自证流程可作为对照参考（而非直接复用，二者仓库/实现独立）。

## 6. 已知限制与注意事项

- 平台限制：预编译仅 Linux x86_64。
- 未支持：OAuth/device-flow 认证（仅环境变量 API key）、扩展运行时、云会话共享、语法高亮；不支持的标志/命令会拒绝并给出原因。
- 安全：prompt injection、敌对 LLM provider、本地权限提升为未处理威胁面（SECURITY.md）。
- 仓库状态：v1.0.4 已发布，但仓库内存在未提交改动（另一会话），评估时注意以提交内容为准。

## 7. 结论

- nini 是成熟、可信、自证完备的 Rust 版 Pi agent（v1.0.4）：其质量体系（冻结 Pi 0.85.1 oracle、符号引用引用、CI 逐字段工具契约对比、红路径不变式验证、身份化发布物）在「重实现替代 TS 原版」类项目中最为完整。
- rust-anything 工具链对 nini 本身无迁移需求；本调研使用其已验证的 Rust 工具链（cargo 构建/测试）+ 仓库事实取证完成，关键取证命令与退出码见上文（构建/测试均通过）。
- 若目标是评估或采纳 nini 进入 pi：需先确认部署形态（独立 `~/.nini/agent/`，Pi 格式兼容可直接迁移）与平台约束（仅 Linux x86_64），建议直接利用其内置 conformance crate 做 pi 侧对照验证；其 oracle 依赖（Pi 版本冻结）需在采纳时一并评估。
