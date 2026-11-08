# nini max_tokens 修复入口检查

## BLOCKER 检查清单

### 1. 计划完整性 ✅
- **plan.md 已存在**：包含目标、背景、需求分解、关键决策、交付物
- **需求可追溯**：3 个需求项明确对应 2 处代码修改 + 1 个测试

### 2. 代码定位 ✅
- **app.rs:288**：build_tui_run_config 函数已定位
- **demo.rs:41**：assemble_run_config 函数已定位
- **修改点明确**：两处 RunConfig 构建逻辑

### 3. 测试策略 ✅
- **断言测试位置**：crates/nini-cli/tests/
- **测试目标**：验证 max_tokens 与 models.json 一致
- **验证方法**：构造截断场景验证 compact+retry

### 4. 风险控制 ✅
- **无破坏性变更**：models.json 已有 maxTokens 字段
- **缺省值保留**：8192 作为后备
- **与 Pi 对齐**：逻辑一致，无语义偏差

## VERDICT: GO
所有 BLOCKER 已清除，可以进入执行阶段。