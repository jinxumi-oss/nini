# 阶段4：质量指标实际采集留档（Quality Left-Shift）

**采集时间**：2026-11-17（UTC），5 条只读采集命令于该日实际连续执行（首条命令执行起始于 2026-11-17T04:51:16Z）。

**采集口径**：按 `docs/quality-left-shift/quality-metrics.md` §0 指标定义与 §1 只读采集约束执行（只读运行与输出采集，不修改实现、不新增依赖、不改动既有测试、不向运行时注入指标值）。

**实际采集结果表**：

| 命令 | 退出码 | 关键计数（只记录真实输出） |
| --- | --- | --- |
| `cargo test -p nini-tools` | 0 | lib 单元测试 97 passed / 0 failed；integration 6+3+3+3+5=20 passed / 0 failed；doc-tests 0 → 合计 117 passed，0 failed |
| `cargo test -p nini-core --lib constraint` | 0 | 6 passed / 0 failed / 0 ignored / 255 filtered out（constraint 6 条测试全部通过） |
| `cargo test -p nini-core --lib` | 0 | 261 passed / 0 failed / 0 ignored / 0 filtered out（core lib 总数 261） |
| `cargo test -p nini-conformance` | 0 | lib 单测 0；integration 合计 75 passed / 0 failed；doc-tests 0 → 合计 75 passed，0 failed |
| `cargo test --workspace` | 0 | 198 个 test result 行（含 Doc-tests 0 行），合计 2192 passed / 0 failed / 1 ignored |

**已采集指标**（按 §0/§1.2 只读采集口径，具体数值见「实际采集结果表」，仅标注对应步骤）：
- 测试基线计数：core lib 总数 / constraint 计数 / conformance 合计 / tools 合计 / workspace 结果，分别对应结果表步骤 2、3、4、1、5，数值以结果表为准。

**未采集项（如实标注，不编造数值）**：
按 `quality-metrics.md` §0 口径，以下指标当前无改动被约束门检查的样本周期数据，暂无法采集，留空并统一标注「需样本周期数据后采集」：
- 被检查改动总数（门运行检查次数）：需样本周期数据后采集（当前缺约束门运行产生的检查记录/样本周期数据）。
- 实际拦截坏改动数（约束门实际拦截并拒绝/回退的改动条数）：需样本周期数据后采集（当前缺门运行实际拒绝记录）。
- 回退/稳定版改动计数（因改动失败回退、未进入稳定版的改动次数与进入稳定版的改动总数）：需样本周期数据后采集（当前缺门运行与稳定版改动记录）。
- 总轮次与无效轮次（review/返工/失败修复轮次）及其减少量：需样本周期数据后采集（当前缺阶段2 功能验证运行/方案轮次数据）。
- 按 §0 计算的四项质量指标（拦截命中率、回退率、无效轮次减少、质量左移达成度）：以上底层计数到位后，按同一口径汇总计算，当前暂不产出数值。

**诚实标注**：本留档只记录真实只读运行结果与已采集口径指标；未采集项如实标注，未虚构任何指标数值；指标口径定义见 `docs/quality-left-shift/quality-metrics.md`。

**完成标记**：【指标留档采集完成】
