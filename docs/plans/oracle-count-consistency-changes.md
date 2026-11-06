# Changes
- `docs/GOALS.md:84` | T3 段：定义 oracle 计数口径并对齐实测 —— 改为「`nini-conformance` 的测试数取口径（`cargo test -p nini-conformance` 的 passed 总数，当前实测 75，可复算），删除原「45 条」「实测 66」旧数表述」 | 计划步骤 1（任务第 1 处） | 类型: 变更
- `docs/GOALS.md:254` | §6 计划文档台账表：`plans/parity-with-pi.md` 行的说明列改为「oracle 计数口径已收口（见 T3，当前实测 75，可复算）」，替换旧的「同样已旧（实测 66）」 | 计划步骤 2（任务第 2 处） | 类型: 变更

## Commands
- `git diff --stat docs/GOALS.md` → 退出码 0（仅 GOALS.md，且只有上述两处文本改动）
- `cargo test -p nini-conformance -q 2>/dev/null | grep -oE 'test result: ok\. [0-9]+' | grep -oE '[0-9]+' | paste -sd+ | bc` → 75
- `python3 scripts/plan-selfcheck.py` → 退出码 0（PASS: 14/14 项通过）
- `python3 scripts/ledger-freshness.py` → 退出码 0（台账新鲜度：PASS —— 56/56 条合格）
- `python3 scripts/parity-gap.py --check-plan` → 退出码 0（台账与计划一致：plan cross-check OK）
- `bash scripts/gate.sh` → 退出码 1（门禁未通过：裸奔不变量回退，见 Blocked 备注）

## Reproduction
- 复现命令: `cargo test -p nini-conformance -q 2>/dev/null | grep -oE 'test result: ok\. [0-9]+' | grep -oE '[0-9]+' | paste -sd+ | bc` → 退出码 0，输出 75（口径可复算：passed 总数；基线即 75，改动后仍 75）
- 复现命令: `git diff docs/GOALS.md` → 退出码 0，diff 仅含 T3 段与 §6 台账表两处文本（无其它数字被改）
- 复现命令: `python3 scripts/plan-selfcheck.py` → 退出码 0（PASS: 14/14 项通过）
- 复现命令: `python3 scripts/ledger-freshness.py` → 退出码 0（台账新鲜度：PASS —— 56/56 条合格）
- 复现命令: `python3 scripts/parity-gap.py --check-plan` → 退出码 0（plan cross-check OK）
- 复现命令: `bash scripts/gate.sh` → 退出码 1（README/CHANGELOG/正文字测试数均为 2097 = 实测；唯「裸奔不变量回退」失败）

## Blocked
- `bash scripts/gate.sh` 退出码 1：链③ 不变量基线判定「裸奔不变量回退（现在 2 条，基线 1 条）」。根因不在本改动（docs/GOALS.md 仅 T3+§6 两处文本，见计划步骤 1/2），而是 HEAD `040e2d4`（agent-abnormal-end-notify 步骤3 新增的回归测试提交）新增了一条未留变异验证的不变量 `sha=040e2d4a`（tier=NONE，`scripts/invariants.py --json` 现 NONE=2，`scripts/gate-baseline.json` 的 `invariants_none` 仍为 1），与 `docs/GOALS.md` 改动无关。按 worker 约束未自改代码/门禁使其转绿，只停手回报失败项。
