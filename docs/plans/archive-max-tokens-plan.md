# nini max_tokens 修复计划

## 目标
让 nini 的 RunConfig.max_tokens 取 models.json 中选中模型的 maxTokens 值（缺省回退 8192），使压缩+重试机制与 Pi 一致。

## 背景
- 当前：nini 硬编码 max_tokens=8192，models.json 的 maxTokens 只用于展示
- Pi：使用 models.json 的 maxTokens（provider-composer.js:73）
- 问题：nini 自身判"不可恢复"，压缩+重试机制失效

## 需求分解

### 1. 修改 RunConfig 构建逻辑
- **位置 1**：crates/nini-cli/src/app.rs:288（build_tui_run_config）
- **位置 2**：crates/nini-cli/src/demo.rs:41（assemble_run_config）
- **改动**：取选中模型的 maxTokens，缺省回退 8192

### 2. 添加断言测试
- **位置**：crates/nini-cli/tests/
- **内容**：验证请求中的 max_tokens 等于 models.json 配置

### 3. 验证修复
- **步骤**：测试压缩+重试机制是否与 Pi 一致
- **方法**：构造截断场景，观察是否触发 compact+retry

## 关键决策
- **口径**：对齐 Pi，不改变 8192 缺省值
- **风险**：models.json 格式兼容性（已有字段，无破坏性变更）

## 交付物
1. 修改后的 app.rs 和 demo.rs
2. 新增断言测试
3. 验证报告