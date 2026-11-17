# Review

审查对象：`docs/quality-left-shift/quality-metrics-collection.md`（仅审查该文件，不改动任何文件）。

## 结论
通过（CLEAN）：结构七项齐全；结果表与已采集指标的计数与真实 `cargo test` 运行输出逐项一致；未采集项未编造数值、如实标注为待样本周期数据后采集；留档内容未越界到其它文件。仅以下 3 项为 MINOR 文案/标注层面的改进项，均不改变真实性与功能行为。

## Findings
- [MINOR] `docs/quality-left-shift/quality-metrics-collection.md:27-30` | 问题: 未采集项第 2/3/4 条未采用需求要求的统一标注「需样本周期数据后采集」，只写了具体缺失数据来源（「需门运行实际拒绝记录后采集」「需门运行与稳定版改动记录后采集」「需阶段2 功能验证运行/方案轮次数据后采集」），四项标注表述不统一。 | 修法: 统一为「需样本周期数据后采集（当前缺 X 记录）」式表述，四项均如实标注为待样本周期数据后采集，且不编造任何数值。
- [MINOR] `docs/quality-left-shift/quality-metrics-collection.md:7-15,17-23` | 问题: 「实际采集结果表」与「已采集指标」对同一组计数重复列示（tools 合计 117、workspace 2192 passed/198 行等），属文案冗余，不影响留档真实性。 | 修法: 保留一处主计数（以结果表为准），另一处仅标注对应步骤编号即可；该精简非必要，保留亦可。
- [MINOR] `docs/quality-left-shift/quality-metrics-collection.md:15,23` | 问题: workspace 行的「198 个 test result 行」未注明计数口径（是否含 Doc-tests 的 0 行及各测试二进制行），复跑时口径易误读。 | 修法: 标注口径，如「198 个 test result 行（含 Doc-tests 0 行）」，便于按同一口径复核复现。
