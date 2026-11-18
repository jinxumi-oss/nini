# 滚轮粒度修复「项1 + 项2」实施记录（worker A）

- 依据计划：`docs/plans/nini-tui-scroll-drag-granularity-fix-plan.md` 项 1（滚轮步长 3→1）与项 2（Alt×5）
- 范围声明：本次**只**实施项 1 与项 2；**未**触碰项 3（`keys.rs` 键盘细粒度）、**未**改动项 4（拖拽粒度）、**未**新增依赖、**未**做 git 写操作、**未**改 `references/`。
- 决定口径：滚轮步长以 Pi 源码为准（`references/pi-0.85.1/src/tui/tui-alt-screen.ts:264` → `options.wheelScrollLines ?? 1`，即默认 1），不使用仓库文档里写「默认 3」的旧口径。

---

## 一、改了什么

### 项 1：滚轮步长 3 → 1

| 位置 | 改动 |
|---|---|
| `crates/nini-tui/src/scrollbar.rs:331-332` | `pub const WHEEL_SCROLL_LINES: usize = 3;` → `= 1;`（一行常量） |
| `crates/nini-tui/src/scrollbar.rs:320-322` | 注释：`wheel_lines()` 的文档从「`tui-alt-screen.ts:144-146`, whose default is 3」改为「`tui-alt-screen.ts:264`, whose default is 1」（原引用行号 144-146 在 Pi 源码里是 `ActiveSearch` 接口，并非 `wheelScrollLines`，属错误引用，一并校正到真实定义行 264） |
| `crates/nini-tui/src/scrollbar.rs:331` | 常量文档注释：`/// Pi's wheelScrollLines default.` → `/// Pi's wheelScrollLines default (`tui-alt-screen.ts:264`: `?? 1`).` |

Pi 侧证据（只读）：`references/pi-0.85.1/src/tui/tui-alt-screen.ts:264`
`this.wheelScrollLines = Math.max(1, Math.floor(options.wheelScrollLines ?? 1));`

### 项 2：滚轮 Alt×5

| 位置 | 改动 |
|---|---|
| `crates/nini-tui/src/runtime.rs:2886-2894`（`MouseEventKind::ScrollUp \| ScrollDown` 分支内） | 步长计算由 `let step = crate::scrollbar::wheel_lines();` 改为：<br>`let alt = m.modifiers.contains(KeyModifiers::ALT);`<br>`let step = crate::scrollbar::wheel_lines() * if alt { 5 } else { 1 };`<br>并加注释引用 Pi `ALT_WHEEL_SCROLL_MULTIPLIER`（`tui-alt-screen.ts:75`）与 `getWheelScrollLines`（`tui-alt-screen.ts:968-971`）。 |

Pi 侧证据（只读）：
- `references/pi-0.85.1/src/tui/tui-alt-screen.ts:75` → `const ALT_WHEEL_SCROLL_MULTIPLIER = 5;`
- `references/pi-0.85.1/src/tui/tui-alt-screen.ts:968-971` →
  `private getWheelScrollLines(button: number): number { // SGR mouse button codes use bit 3 (value 8) for the Alt modifier. return (button & 8) !== 0 ? this.wheelScrollLines * ALT_WHEEL_SCROLL_MULTIPLIER : this.wheelScrollLines; }`

判定口径说明：Pi 读 SGR button code 的 bit3(=8) 作为 Alt；crossterm 0.28 在解析 SGR mouse 时把同一 bit 映射为 `KeyModifiers::ALT`（`crossterm-0.28.1/src/event/sys/unix/parse.rs` 的 `parse_cb`：`if cb & 0b0000_1000 == 0b0000_1000 { modifiers |= KeyModifiers::ALT; }`），因此在 nini 侧用 `m.modifiers.contains(KeyModifiers::ALT)` 与 Pi 等价。`runtime.rs` 顶部已有 `use crossterm::event::{..., KeyModifiers};`，无需新增 import。

### 钉旧步长（3）的既有测试期望 —— 逐条说明

以下断言钉的是**旧步长 3**（nini 自行设定的常量），**本次因对齐 Pi 源码（默认 1）而变**，不是语义放宽或削弱：

1. `crates/nini-tui/tests/mouse_e2e.rs:84`（原）`assert_eq!(up, 3, "Pi's wheelScrollLines default")` → 改为 `assert_eq!(up, 1, ...)`。钉的是「一次滚轮事件滚动几行」= 旧常量 3；Pi 默认 1，故改为 1。
2. `crates/nini-tui/tests/mouse_e2e.rs:88`（原）`assert_eq!(up, 3, "and the scroll really happened")` → 改为 `assert_eq!(up, 1, ...)`。同一变量的复读断言，同样是旧步长的钉子。
3. `crates/nini-tui/tests/mouse_e2e.rs:531`（原）`assert_eq!(offset(&shared), 3, "the wheel acts on line counts alone")` → 改为 `= 1`。该用例的目的是「滚轮只按行数计数，不需要 viewport」，步长由常量决定；断言值随常量同步，用例意图未变。
4. `crates/nini-tui/src/scrollbar.rs:643`（单元测试 `the_wheel_moves_pi_s_default_number_of_lines`）`assert_eq!(wheel_lines(), 3)` → `assert_eq!(wheel_lines(), 1)`。该断言直接钉常量值，与 `WHEEL_SCROLL_LINES` 同步。
5. `crates/nini-tui/tests/mouse_e2e.rs:8`（模块文档）`` `wheelScrollLines` option (default 3)`` → `` (default 1, `tui-alt-screen.ts:264`)``。属同一旧口径的注释钉子。

未改动但会受影响且已验证仍绿的用例：`the_wheel_stops_at_the_ends`（循环 200 次，步长变小只会更慢到达上限，断言不变）、`scrolling_up_disables_autoscroll_and_returns_to_the_tail_restores_it`（5 次 ScrollDown 回到 0，步长 1 时 5 次恰好从 1 回到 0）。

### 新增测试（纯追加到 `crates/nini-tui/tests/mouse_e2e.rs`）

- 追加辅助函数 `mouse_with(kind, col, row, modifiers)`（`:74-88` 附近）——既有 `mouse()` 恒为 `KeyModifiers::NONE`，无法表达 Alt，故新增一个带修饰键的构造器；未修改既有 `mouse()`。
- 新增用例 `alt_plus_the_wheel_scrolls_five_times_farther`：一次 Alt+滚轮 → 偏移 5（= 1×5），再一次普通滚轮 → 偏移 6（= +1），从而锁定 5:1 倍率与普通滚轮仍为 1 行。

---

## 二、命令与退出码（自检真实输出）

```
$ ~/.cargo/bin/cargo test -p nini-tui --test mouse_e2e --test scrollbar_units
test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s   # mouse_e2e
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s    # scrollbar_units
EXIT=0
```
（新增用例在输出中：`test alt_plus_the_wheel_scrolls_five_times_farther ... ok`）

```
$ ~/.cargo/bin/cargo test -p nini-tui --lib scrollbar
test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 389 filtered out; finished in 0.00s
EXIT=0
```
（`test scrollbar::tests::the_wheel_moves_pi_s_default_number_of_lines ... ok`）

```
$ ~/.cargo/bin/cargo test -p nini-tui --lib mouse
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 397 filtered out; finished in 0.00s
EXIT=0
```
（`runtime::mouse_selection_tests` 全绿；其中 `a_user_scroll_clears_the_selection` 依赖滚轮事件，与步长无关）

```
$ ~/.cargo/bin/rustfmt --edition 2021 --check crates/nini-tui/src/runtime.rs crates/nini-tui/src/scrollbar.rs crates/nini-tui/tests/mouse_e2e.rs
EXIT=0
```

---

## 三、Blocked

none

---

## 四、未确认项

1. **实机观感**：1 行步长在真实终端是否仍可感知、Alt×5 在各终端（支持 SGR 1006 的 tmux/iTerm/Windows Terminal）是否都能收到 Alt 位——本轮只有单测证据（crossterm 解析映射），未做实机验证。
2. **Pi 应用层实际传入值**：`createInteractiveTui` 不在参照子集内，Pi 运行时真正传给 `wheelScrollLines` 的值是否就是默认 1 未取证（计划「需人确认项 2」）。本轮按 Pi 源码默认值 1 实施。
3. **仓库文档旧口径**：`docs/plans/v0.10-pi-parity-refactor.md:1541` 仍写「默认 3」；按本轮约束（禁止改无关文件 / 文档）未同步，需人确认由谁修订。
