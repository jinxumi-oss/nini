# Changes
- `crates/nini-cli/src/app.rs:263` | 新增 `model_max_tokens` 辅助函数：从用户 `models.json`（Pi 拼写）取选中模型的 `maxTokens`，找不到返回 `None`，供两处 RunConfig 构建共用 | 步骤 1 | 变更
- `crates/nini-cli/src/app.rs:324` | `build_tui_run_config` 加载用户 `models.json` | 步骤 1 | 变更
- `crates/nini-cli/src/app.rs:385` | `build_tui_run_config` 的 `RunConfig.max_tokens` 取选中模型 `maxTokens`，缺省回退 `8192`（原硬编码 `8192`） | 步骤 1 | 变更
- `crates/nini-cli/src/demo.rs:45` | `assemble_run_config` 加载用户 `models.json` | 步骤 2 | 变更
- `crates/nini-cli/src/demo.rs:72` | `assemble_run_config` 的 `RunConfig.max_tokens` 取选中模型 `maxTokens`，缺省回退 `8192`（原硬编码 `8192`） | 步骤 2 | 变更
- `crates/nini-cli/src/app.rs:1868` | 新增 `app::max_tokens_tests`：TUI 门断言 `RunConfig.max_tokens` 等于 `models.json` 配置的 `maxTokens`（含缺省 `8192` 回退、带 provider 的 id 与裸 id、跨 provider 隔离） | 步骤 3 | 变更
- `crates/nini-cli/src/demo.rs:683` | 新增 `demo::max_tokens_tests`：assemble 门（demo 与 `-p` 共用）断言同上 | 步骤 3 | 变更

## Commands
- `cargo test -p nini-cli --bin nini max_tokens_tests` → 退出码 0（2 条断言测试通过）
- `cargo test -p nini-cli` → 退出码 0（bin 单测 101 条 + 全部集成测试，16 个 test result 全 ok）
- `cargo test -p nini-core --test overflow_recovery_parity --test a3b_overflow --test compaction_lifecycle` → 退出码 0（压缩+重试与 Pi 的 parity 测试全通过）

（步骤 3 的断言测试放在 crate 的 `cfg(test)` 模块 `app.rs`/`demo.rs`，而非 `tests/`：`nini-cli` 的 lib 目标只暴露 `provider_factory`，集成测试无法访问两处 `pub(crate)` 的 `RunConfig` 组装函数；这与仓库既有约定一致（`cli_flag_contract.rs` 也写明该类内部模块不可从集成测试导入）。）

## Reproduction
- 复现命令: `cargo test -p nini-cli --bin nini max_tokens_tests` → 退出码 0（修复后绿；红→绿）
- 修复前（BUG）：`RunConfig::new` 硬编码 `max_tokens: Some(8192)`（`crates/nini-core/src/agent.rs:841`），两处 assembly 又用 `..RunConfig::new(...)` 铺默认值，`models.json` 的 `maxTokens` 从未进入请求 → 断言 `Some(32768)` 失败（红）；修复后两处 assembly 取 `models.json` 的 `maxTokens`，断言转绿。

## Blocked
none
