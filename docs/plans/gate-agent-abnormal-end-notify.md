# Gate — agent-abnormal-end-notify

Verdict: **APPROVED**

理由：
- 现象有代码侧可证实的根因：agent 任务异常结束（无正常 Done）时，只有 watchdog 写 tracing 警告，TUI transcript 无用户可见事件，导致「断开后完全静止、无回复」。
- 改动是单点、最小化：只在 watchdog 异常分支补显式终止通知 + 抽出纯 helper；新增回归测试锁定行为；不扩范围、不改既有实现与测试。
- 验收标准：新增测试转绿、相关 crate 测试不破、门禁通过（见 `docs/plans/agent-abnormal-end-notify.md` §复现 / 验收命令）。
