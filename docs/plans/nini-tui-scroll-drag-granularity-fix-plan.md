# nini TUI「上下文滚动 / 鼠标拖拽」粒度修复计划

- 任务契约：`~/.pi/agent/data/uap/nini-tui-scroll-drag-granularity.json`
- 调查依据：`docs/ux-reports/nini-tui-scroll-drag-granularity-investigation.md`（本轮多 agent 取证与行为实测）
- 本计划**只出计划，不含实现**：所有改动点均可追溯到既有代码行（`crates/nini-tui/` 源码行）或既有测试期望，未指向任何不在仓库中的新概念。
- 决策规则依据（仓库既有约定）：Pi 语义与现状冲突 → 按 Pi 改（含允许更新「钉旧行为」的既有测试并逐条说明）；两种方案可行 → 选改动面最小；口径不清 → 标「需人确认」，不自行发明业务规则。

---

## 一、需求与依据

- 需求原文：「nini Tui 上下文滚动及鼠标拖拽细粒度不够，跨度太大，调查情况并给出修复计划。多agent执行」
- 调查结论（见调查报告）：粒度问题的本质是**步长/跨度**，非功能缺失——
  1. 滚轮每事件 **3 行**（Pi 基准 1 行/事件）→ 细粒度不够；
  2. 滚轮无 **Alt×5** 加速（Pi ×5）→ 多终端体验偏弱；
  3. 键盘只有**整屏一档**（`scroll_transcript` 步长 `page_lines()`）→ 跨度太大；
  4. 滚动条拖拽与 Pi 公式一致（逐事件比例映射），**不属粒度缺陷**（但单事件跨度可跨 >5 行，是否收窄需人确认）；
  5. 选区拖拽已是字符级粒度，不属本诉求。
- 红线遵守：不改在途未提交改动；不新增依赖；本计划只新增两个产物文件（调查报告 + 本计划）。

---

## 二、修复方案（粒度项 → 改动点 → 优先级 → 验收 → 风险/边界）

### 项 1（P0）：滚轮步长 3 → 1（对齐 Pi，直接解决「细粒度不够」）

- **现状/证据**：`crates/nini-tui/src/scrollbar.rs:332` `pub const WHEEL_SCROLL_LINES: usize = 3;`；`runtime.rs:2865` 用 `wheel_lines()` 作步长；测试钉住：`crates/nini-tui/tests/mouse_e2e.rs:84`（`assert_eq!(up, 3, "Pi's wheelScrollLines default")`）、`:531`。
- **改动点**：
  1. `scrollbar.rs:332`：`WHEEL_SCROLL_LINES` 3 → 1（一行：最小改动，只动常量）。
  2. `crates/nini-tui/tests/mouse_e2e.rs:84`、`:531`（及可能钉 3 的其他测试）：期望值 3 → 1，逐条说明「钉的是旧步长，因对齐 Pi 而变」。
- **优先级**：P0（细粒度主诉直接对应）。
- **验收标准（可执行）**：
  - `cargo test -p nini-tui --test mouse_e2e` → 退出码 0（期望 1 行）；
  - `cargo fmt --all -- --check` → 退出码 0（若改动触发格式）；
  - 如有单测直接断言 `WHEEL_SCROLL_LINES` 值，同步改为 1。
- **风险与边界**：
  - 改动既有测试期望（必须逐条说明，符合仓库既有规则）；
  - 实机观感（1 行是否仍可感知）**未确认**（调查未确认项 4），实施后需实机确认。

### 项 2（P1）：滚轮补 Alt 倍率 ×5（对齐 Pi）

- **现状/证据**：`runtime.rs:2863-2881` 滚轮分支无 `m.modifiers` 判定，无 Alt 分支（Pi：`tui-alt-screen.ts:75` `ALT_WHEEL_SCROLL_MULTIPLIER = 5`、`:968-971` 按 SGR button bit3(=8) 判定 Alt 后 ×5）。
- **改动点**：
  1. `runtime.rs:2863-2881`：滚轮分支增加修饰键判定——`m.modifiers` 含 Alt（或等价判定）时 `step = step * 5`。
  2. 新增单测：Alt + 滚轮每事件步长 = 新步长 ×5（项 1 后 = 1×5 = 5 行），断言滚动方向与步长。
- **优先级**：P1（加速缺失，默认路径不受影响）。
- **验收标准（可执行）**：
  - 新增 `Alt+滚轮步长` 单测 → 退出码 0；
  - `cargo test -p nini-tui --test mouse_e2e` → 退出码 0（普通滚轮仍 1 行，默认路径不变）。
- **风险与边界**：
  - 仅影响 Alt+滚轮；判定方式对齐 Pi 的 SGR button bit3 口径（worker B 已取证）；
  - 不改变普通滚轮（项 1）与拖拽（项 4）。

### 项 3（P1）：键盘滚动手感细粒度（1 行 / 半页，对齐 Pi）

- **现状/证据**：`runtime.rs:4660` `let step = state.page_lines() as i32;`（翻页步长整屏）；`keys.rs:517-523` 仅 PageUp/PageDown/Shift+End，无逐行/半页（Pi 有 `lineUp/lineDown` 步长 1 行、`halfPageUp/Down` = `floor(viewportHeight/2)`，`tui-alt-screen.ts:736-742`，默认无绑定）。
- **改动点**：
  1. `keys.rs`：新增键位与 `KeyAction`（`LineUp`/`LineDown`/`HalfPageUp`/`HalfPageDown`），默认不绑定（与 Pi 默认空绑定一致，避免顺手加键）。
  2. `runtime.rs`：新增按**行**步进的滚动动作（步长 1 行；半页步长 `floor(page_lines()/2)`），复用 `scroll_transcript` 的步长机制但不得改变既有 `page_lines()` 翻页语义（翻页保持整屏）。
  3. `runtime.rs`：`apply_action` 第二派发点补上新动作（仓库既有纪律：第二派发点必须穷尽，避免半条动作被静默丢弃）。
- **优先级**：P1（「跨度太大」的键盘侧主因）。
- **验收标准（可执行）**：
  - 新增 `键绑定` 单测：`default_keymap` 含新动作且默认无绑定（与 Pi 一致）→ 退出码 0；
  - 新增 `行步进滚动` 单测：1 按 = 1 行、半页 = `floor(page_lines/2)` 行，夹紧到上/下界 → 退出码 0；
  - `cargo test -p nini-tui` 相关键/滚动测试 → 退出码 0（既有翻页测试仍绿）。
- **风险与边界**：
  - 新增动作默认不绑定（不改动既有键位）；
  - 必须保证既有 `scroll_transcript`（整屏翻页）语义不变；
  - 实现细节（行步长函数）须复用 `scroll_transcript` 的夹紧/autoscroll/清选区逻辑，不另造复杂度。

### 项 4（P2，需人确认）：滚动条拖拽单事件跨度收窄（可选）

- **现状/证据**：拖拽为逐事件比例映射（`runtime.rs:2926-2932` → `scrollbar.rs:293-317`），与 Pi 公式一致（worker B：公式一致、无插值）；实测一次事件可跨 >5 行（`crates/nini-tui/tests/mouse_e2e.rs:208-211`）。
- **方案**：现状**默认不改**（与 Pi 公式一致，属连续映射而非缺陷）；若用户要求更细的单事件跨度，候选为「按指针移动增量累积的平滑映射」（偏离 Pi 逐事件绝对比例），是否采纳 → **需人确认**。
- **改动点**：若采纳，`runtime.rs:2926-2932` 的拖拽映射改为增量式；否则不改动。
- **优先级**：P2（需人确认口径，不擅自改动）。
- **验收标准**：
  - 不采纳：现有拖拽测试全绿（`cargo test -p nini-tui --test mouse_e2e` 拖拽相关用例退出码 0）；
  - 采纳：新增平滑映射单测（逐事件跨度 ≤ 增量映射跨度）→ 退出码 0（需人确认后实施）。
- **风险与边界**：
  - 采纳即偏离 Pi 语义，必须显式登记为「有意差异」（仓库红线：不改 Pi 语义除非按 Pi 或人确认）；
  - 未采纳时本项不产生任何改动。

### 项 5（不改动，明确保持现状）

- **滚动条拖拽公式**（与 Pi 一致）、**选区拖拽字符粒度**、**context 条**（双方均非可滚动区）：与 Pi 现状一致或已够细，不属本诉求，保持不动（防止顺手重构，遵守「极短路径」）。

---

## 三、改动文件清单（实施时按此最小集合）

| 文件 | 改动内容 | 对应项 |
|---|---|---|
| `crates/nini-tui/src/scrollbar.rs:332` | `WHEEL_SCROLL_LINES` 3 → 1 | 项 1 |
| `crates/nini-tui/tests/mouse_e2e.rs:84,531`（及钉 3 的测试） | 期望值 3 → 1，逐条说明钉旧行为 | 项 1 |
| `crates/nini-tui/src/runtime.rs:2863-2881` | 滚轮加 Alt×5 判定 | 项 2 |
| `crates/nini-tui/src/runtime.rs`（新增细步滚动函数）+ `keys.rs`（新键位/KeyAction） | 行步进、半页滚动动作与绑定 | 项 3 |
| `runtime.rs` `apply_action` 第二派发点 | 补新动作（保证穷尽） | 项 3 |
| （若人确认采纳）`runtime.rs:2926-2932` | 拖拽改为增量平滑映射 | 项 4 |

**未触碰**：在途未提交改动、`references/pi-0.85.1/`、既有文档、依赖、git。

---

## 四、需人确认项（口径不清，不擅自决定）

1. **滚轮步长以哪份为准**：Pi 源码 `tui-alt-screen.ts:264` 默认 1，但仓库文档 `v0.10-pi-parity-refactor.md:1541` 写「默认 3」→ 按 Pi 源码改 1（推荐，改动面最小且符合对齐目标）。需人确认。
2. **Pi 应用层实际传入的 `wheelScrollLines` 值**（`createInteractiveTui` 不在参照子集）→ Pi 实际默认是否也是 1 需实机确认。需人确认。
3. **滚动条拖拽是否收窄单事件跨度**（项 4）：现状与 Pi 一致，默认不采纳。需人确认是否接受「与 Pi 相同的逐事件绝对比例」。
4. **实机观感**（1 行步长/半页是否可感知、拖拽是否更顺）→ 需实机测量后判断是否进一步调参。

---

## 五、全量回归与验收命令（实施完成后）

```
# 相关滚动/鼠标测试（项 1-3 的验收口径）
cargo test -p nini-tui --test mouse_e2e --test scrollbar_units --test mouse_overlay_wheel --test scrollbar_e2e --test scroll_indicator_no_overlap --test context_bar_wiring
# 全量回归（含新测试）
cargo test -p nini-tui
# 格式/ lint（仓库既有门禁口径）
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
# 若仓库有门禁：bash scripts/gate.sh（在干净 worktree 上执行）
```
- 验收口径：相关测试与全量回归退出码 0；新增测试必须通过；既有测试期望变更必须逐条说明（钉旧行为）。

## 六、回滚

- 项 1：单点回改 `scrollbar.rs:332` 常量与测试期望即可（步长恢复 3）。
- 项 2：滚轮分支移除 Alt 判定（默认路径恢复）。
- 项 3：移除新增键位与动作、删新增测试即可（保留既有翻页语义）。
- 项 4：未采纳则不产生任何改动；若采纳，单点回改拖拽映射即可。
- 所有改动可单独回滚，互不牵连（滚动/拖拽/键盘路径独立）。
