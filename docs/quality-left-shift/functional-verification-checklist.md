# 功能地图 → 可执行验证清单（Quality Left-Shift 运行时验证输入）

**0. 基线事实**
- nini = Pi 的 clean-room Rust 重写（v1.0.1，源码 `/home/jin/nini`）；能力：agent loop 单层折叠（MAX_TOOL_ITERATIONS=50、DEFAULT_MAX_PARALLEL_TOOLS=8、max_tokens=8192）、内置工具 + 并发写保护 mutation_queue、会话 JSONL 原子写+坏行跳过、自动压缩（阈值 + 溢出，本地确定性摘要）、截断容错（连续截断上限 + 接住半截 + 72h 兜底）、信任闸、钩子面（M1 上下文 / M2 预留）、多 provider fallback；已知缺口：无 subagent、无危险命令硬闸。
- 测试基线：conformance（66 tests，oracle=真实 Pi 源码）、core lib（261 单测）、tools（97 passed + 集成全绿）、constraint（6 passed）。
- 功能地图条目基线（与功能地图文档一致，不增删改语义）：F1 多 provider 调用与限流兜底；F2 Agent loop；F3 内置工具 + 并发写保护；F4 会话持久化；F5 自动压缩；F6 截断容错；F7 信任闸；F8 钩子面；F9 配置/模型/skills 读取；F10 危险命令处理（已知缺口·边界项，不作为通过项）。

**1. 映射表**

| 功能条目 | 对应测试/用例 | 执行命令 | 验收判据 |
|---|---|---|---|
| F1 多 provider 调用与限流兜底 | `crates/nini-ai/tests/fallback_diagnostics.rs`、`provider_headers.rs`、`pi_wire_oracle.rs`；`crates/nini-cli/tests/provider_registry.rs`、`provider_config.rs`、`models_json_provider.rs`、`default_model.rs`；`crates/nini-core/tests/provider_http_retry.rs`、`overflow_http_400.rs` | `cargo test -p nini-ai -p nini-cli -p nini-core`（覆盖上述 provider/reg/fallback 测试） | 上述 provider/reg/fallback 测试通过，断言覆盖多 provider 调用与限流兜底关键行为。 |
| F2 Agent loop | `crates/nini-core/tests/agent_integration.rs`；`crates/nini-conformance/tests/long_task_delivery.rs`（注入 backstop） | `cargo test -p nini-core -p nini-conformance`（agent 相关 + conformance long_task） | 上述测试通过，断言覆盖 Agent loop 单层折叠与断点/backstop 关键行为。 |
| F3 内置工具 + 并发写保护 | `crates/nini-tools/tests/edit_contract.rs`、`mutation_queue.rs`、`truncation_parity.rs`、`tool_contract_parity.rs`；`crates/nini-core/tests/wrapped_tool_execution_mode.rs`、`tool_execution_mode.rs` | `cargo test -p nini-tools -p nini-core`（tools 测试 + wrapped_tool_execution_mode） | 上述测试通过，断言覆盖内置工具与 mutation_queue 并发写保护（串行化）关键行为。 |
| F4 会话持久化 | `crates/nini-conformance/tests/session_corpus.rs`、`session_write.rs`、`session_fresh.rs`、`session_parity.rs`、`session_fork.rs`、`compaction_file_ops.rs` | `cargo test -p nini-conformance` | 上述测试通过，断言覆盖会话 JSONL 原子写 + 坏行跳过 + 持久化读写关键行为。 |
| F5 自动压缩 | `crates/nini-conformance/tests/a_compaction.rs`、`compaction_file_ops.rs`、`compaction_persistence.rs`；`crates/nini-core/tests/compaction_lifecycle.rs` | `cargo test -p nini-conformance -p nini-core` | 上述测试通过，断言覆盖自动压缩（阈值 + 溢出、本地确定性摘要）关键行为。 |
| F6 截断容错 | `crates/nini-core/tests/truncation_continues.rs`、`a3b_overflow.rs`、`overflow_recovery.rs`、`overflow_recovery_parity.rs`、`overflow_http_400.rs` | `cargo test -p nini-core` | 上述测试通过，断言覆盖截断容错（连续截断上限 + 接住半截 + 72h 兜底）关键行为。 |
| F7 信任闸 | `crates/nini-core/tests/trust_gate.rs`、`trust_gate_lock.rs`、`trust_persistence.rs` | `cargo test -p nini-core` | 上述测试通过，断言覆盖信任闸关键行为。 |
| F8 钩子面 | `crates/nini-core/src/agent_hooks.rs` 的 mod tests（noop/custom_hook/panic）；`crates/nini-core/src/agent.rs` 的 `steering_followup_hook_tests`、`tool_lifecycle_hook_tests` | `cargo test -p nini-core --lib` | 上述测试通过，断言覆盖钩子面（M1 上下文 / M2 before/after）关键行为。 |
| F9 配置/模型/skills 读取 | `settings.rs` 的 settings 系列单测（core lib）；`crates/nini-core/tests/model_catalog.rs`、`user_models.rs`、`skill_ecosystem.rs`、`skill_collision_precedence.rs` | `cargo test -p nini-core --lib` | 上述测试通过，断言覆盖配置/模型/skills 读取关键行为。 |
| F10 危险命令处理（已知缺口·边界项） | 无（已知缺口：无危险命令硬闸，不作为通过项） | 不执行（无硬闸） | 标注「已知缺口·无硬闸，不作为通过项」，不纳入验收通过项。 |

**2. 已执行验证输出**

（工作目录 `/home/jin/nini`，命令均先 `export PATH="$HOME/.cargo/bin:$PATH"`；以下为真实运行结果，退出码与计数为实际输出。）

- 命令：`cargo test -p nini-core --lib` → 退出码 0；`test result: ok. 261 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`（耗时 3.51s；含 agent 钩子测试 `agent::steering_followup_hook_tests`、`agent::tool_lifecycle_hook_tests` 等核心单测）。
- 命令：`cargo test -p nini-conformance` → 退出码 0；18 个测试二进制全绿、合计 75 passed / 0 failed（a_compaction 7、citations_resolve 2、compaction_file_ops 6、compaction_persistence 5、long_task_delivery 4、loop_semantics 4、overflow_recovery 7、prepare_next_turn 3、session_corpus 2、session_fork 12、session_fresh 1、session_parity 2、session_parity_0_85_1 3、session_write 5、stop_after_turn 3、tool_batch_semantics 7、v4_forward_compat 2，doc-tests 0）。注：实际总通过数为 75（基线口径 66，含 session_fork 等新增测试二进制），均为 0 failed。
- 命令：`cargo test -p nini-tools` → 退出码 0；`test result: ok. 97 passed` + 集成全绿（6/3/3/3/5，合计 117 passed / 0 failed，doc-tests 0）。
- 命令：`cargo test -p nini-core --lib constraint` → 退出码 0；`test result: ok. 6 passed; ... 255 filtered out`（约束 R1–R4 拒绝/放行/阈值用例，6 passed / 0 failed）。

**3. 诚实标注**

本清单为设计口径；执行输出为实际运行结果；F10 为已知缺口（无硬闸，不作为通过项）。
