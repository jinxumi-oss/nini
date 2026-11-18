# nini TUI 滚动粒度修复 · 实施报告 B（项 3：键盘 1 行 / 半页滚动）

- 契约：`~/.pi/agent/data/uap/nini-tui-scroll-granularity-implementation.json`（DoD **D2**）
- 计划：`docs/plans/nini-tui-scroll-drag-granularity-fix-plan.md` §二 项 3
- 角色：worker B（项 1+项 2 由 worker A 负责；项 4 拖拽平滑已在途，未回退）
- 状态：**D2 已实施**（新增 4 个动作 + 4 条默认键绑定 + 两个派发点补齐 + 7 个新测试）

---

## ① 改了什么（file:line + UX 偏离登记）

### 1.1 `crates/nini-tui/src/keys.rs`

| 位置 | 改动 |
|---|---|
| `keys.rs:207-231` | `KeyAction` 新增 4 个变体：`LineUp`（:213）、`LineDown`（:218）、`HalfPageUp`（:224）、`HalfPageDown`（:230），每个带 Pi 出处的 doc comment |
| `keys.rs:548-580` | **UX 偏离登记注释块**（见 §1.5） |
| `keys.rs:581-595` | `default_keymap()` 新增 4 条默认绑定 |
| `keys.rs:975-1070` | 新增 2 个单测：`fine_scroll_keys_are_bound`（:998）、`fine_scroll_keys_do_not_shadow_existing_ones`（:1041） |

默认键位（**均未被 Pi、也未被 nini 占用**，程序化核对过 Pi 的 64 个默认和弦与 nini 的 51 行原表）：

| 键 | 动作 | 步长 |
|---|---|---|
| `Shift+Up` | `LineUp` | 1 个 TranscriptLine |
| `Shift+Down` | `LineDown` | 1 个 TranscriptLine |
| `Alt+PageUp` | `HalfPageUp` | `max(1, page_lines / 2)` |
| `Alt+PageDown` | `HalfPageDown` | `max(1, page_lines / 2)` |

### 1.2 `crates/nini-tui/src/runtime.rs`

| 位置 | 改动 |
|---|---|
| `runtime.rs:4754-4757` | `scroll_transcript` 改为**委托**给新核心（步长仍是 `page_lines()`，整屏语义不变） |
| `runtime.rs:4759-4775` | 新增 `scroll_transcript_by_lines(state, delta_lines)`：夹紧 + `autoscroll = (next == 0)` + `mark_scroll_activity()`，与整屏翻页共用同一份实现 |
| `runtime.rs:4777-4789` | 新增 `half_page_lines(state) = (page_lines / 2).max(1)` |
| `runtime.rs:2766-2781` | **派发点 1**（`handle_key`）：新增 4 个 arm |
| `runtime.rs:5199-5214` | **派发点 2**（`apply_action`）：新增 4 个 arm（穷尽性由编译器保证：两个 `match` 都没有 `_` 通配 arm） |
| `runtime.rs:5893-6013` | 新增 `#[cfg(test)] mod fine_scroll_tests`：6 个新测试 |

方向约定：`+delta` = 往**更旧**的方向（与既有 `scroll_transcript` 的 `ScrollUp => delta 1` 同号）。
`LineUp => +1`、`LineDown => -1`、`HalfPageUp => +half`、`HalfPageDown => -half`。

### 1.3 超出声明文件范围的两处**必要耦合**改动（非静默，已在此登记）

| 文件:行 | 改动 | 为什么必须 |
|---|---|---|
| `crates/nini-tui/src/commands/dispatch.rs:1795-1798` | `action_label()` 新增 4 个 arm | **实测证明是硬需求**：删掉这 4 个 arm 后，既有测试 `slash_command_e2e::hotkeys_labels_every_binding_it_lists` **立刻 FAILED**（`/hotkeys` 会把新绑定打成 `?`）。该函数的 `other => "?"` 分支就是为「动作没登记就静默消失」准备的报警器 |
| `crates/nini-tui/src/keybindings_manager.rs:278-281` | `parse_action()` 新增 4 个动作名 | 该文件自己写着红线：「不登记它们的后果很具体：`keybindings.json` 里写 … 会被**静默忽略**」。R14（`CopyLastAssistant`/`Suspend`/`DequeueMessages`）有同样的先例。不登记 → 用户改绑这 4 个新动作后**按键一点没变**且无任何提示 |

`keybindings_manager.rs:425-445` 另加 1 个单测 `the_fine_scroll_actions_can_be_rebound`。

> 这两个文件**不在**契约的「禁并行文件」清单里，且不属于「其他源码/测试/配置」里被红线禁止的那一类（它们是本项 3 新动作的**同一功能面**：一个决定 `/hotkeys` 能不能看见它，一个决定用户能不能改绑它）。

### 1.4 未改动（刻意）

- `scripts/keymap-parity.json`（契约数据源）：**故意不改**。Pi 的 `tui.altScreen.halfPageUp/halfPageDown/lineUp/lineDown` 四个 id 的 `defaultKeys` 是 `[]`，oracle 的规则 1 只要求「**有**默认键的 id」必须登记，所以它们被跳过；规则 2/3 只遍历 `map`，`nini_classes` 也只在 `map` 引用某个动作时才被检查。改它需要新增 4 行 `map`（`keys: differ` + 理由），属于契约层面的裁决，留给 M4「更新契约」那一步（见 §④-(h)）。
- 既有 `PageUp`/`PageDown` 的翻页语义、既有测试的期望值：**一个都没动**（item 1/2 的滚轮改动也未触碰）。
- `references/pi-0.85.1/**`、README/CHANGELOG/plan.md/changes.md、修复计划文件本身、git：**未触碰**。

### 1.5 **UX 偏离登记（对 Pi 的有意差异）**

Pi 侧证据：`references/pi-0.85.1/src/tui/keybindings.ts:168-183` ——

```
"tui.altScreen.halfPageUp":   { defaultKeys: [], description: "Scroll viewport up half a page" },
"tui.altScreen.halfPageDown": { defaultKeys: [], description: "Scroll viewport down half a page" },
"tui.altScreen.lineUp":       { defaultKeys: [], description: "Scroll viewport up one line" },
"tui.altScreen.lineDown":     { defaultKeys: [], description: "Scroll viewport down one line" },
```

Pi **实现了**这四个动作（`tui-alt-screen.ts:736-751`：半页 = `Math.max(1, Math.floor(viewportHeight / 2))`，单行 = `±1`），但**一个键都不绑**（`defaultKeys: []`）——不写自定义键位表就按不出来。

**nini 的有意差异：给这四个动作绑默认键。** 理由：鼠标滚轮不一定在（没有滚轮的触控板、远程/SSH 会话），而「PageUp = 整整一屏」对逐行读一段长回答太粗。Pi 的 `defaultKeys: []` 是 Pi 的**产品选择**，不是「这些动作不该存在」。

登记位置（三处齐全，符合 D2「代码注释 + 新增测试 + 报告」）：

1. **代码注释**：`keys.rs:548-580`（绑定表旁的偏离说明）+ 每个变体的 doc comment（`keys.rs:207-231`，逐条写明 Pi 出处与「Pi 无默认键」的事实）+ `runtime.rs:2766-2770` / `5199-5203` 的派发点注释。
2. **新增测试**：`keys.rs::fine_scroll_keys_are_bound`（把 4 个和弦钉死；删掉任一绑定即失败）+ `runtime.rs::fine_scroll_tests`（6 个行为测试）。
3. **本报告**：§1.5。

**为什么选这 4 个和弦**：它们在 **Pi 和 nini 里都没被占用**，所以没有任何一个和弦会分叉掉用户已经知道的含义。程序化核对结果：

- Pi 占用：`ctrl+up` / `ctrl+shift+up` = `tui.altScreen.previousPrompt`；`ctrl+down` / `ctrl+shift+down` = `tui.altScreen.nextPrompt`；`ctrl+pageUp` / `ctrl+pageDown` = `tui.editor.pageUp/pageDown`；`alt+up` = `app.message.dequeue`；`alt+down` = `app.models.reorderDown`。
- 因此**刻意避开** `ctrl+up/down`、`ctrl+shift+up/down`、`ctrl+pageUp/pageDown`、`alt+up/down`。
- nini 占用：`Up`/`Down` = `MoveUp`/`MoveDown`；`PageUp`/`PageDown` = `ScrollUp`/`ScrollDown`；`Home`/`End`/`Shift+End`；`alt+up` = `DequeueMessages`。
- 剩下可选且未被占用的：`Shift+Up/Down`、`Alt+PageUp/PageDown`、`Ctrl+Up/Down`、`Ctrl+Shift+Up/Down`、`Alt+Left/Right`。
- **又排除** `Shift+PageUp/PageDown`（终端自己截去做 scrollback，程序收不到）、`Ctrl+Alt+Up/Down`（GNOME 切工作区）、`Ctrl+Shift+Up/Down`（部分终端把 Ctrl+Shift 折叠成 Ctrl，会产生歧义）。
- 最终取 `Shift+Up/Down`（行粒度，紧挨既有的方向键族）+ `Alt+PageUp/PageDown`（半页粒度，紧挨既有的 Page 族整屏翻页）。既有 `Shift+End` → `ScrollToBottom` 证明「带 Shift 的特殊键」在 nini 里已是成熟模式。

**终端送达前提（未实机验证）**：`Shift+Up/Down` 走 `CSI 1;2A` / `CSI 1;2B`，`Alt+PageUp/PageDown` 走 `CSI 5;3~` / `CSI 6;3~`（crossterm 0.28.1 的解析器都认：crossterm-0.28.1/src/event/sys/unix/parse.rs:303 `parse_modifiers`、:348 `parse_csi_modifier_key_code`、:619 `parse_csi_special_key_code`）。极简终端可能只发裸 `Up`/`Down` 或什么都不发 —— 见 §④-(g)。

---

## ② 命令与退出码

| 命令 | 退出码 | 结果 |
|---|---|---|
| `~/.cargo/bin/cargo test -p nini-tui --lib` | **0** | `test result: ok. 422 passed; 0 failed`（基线 413 → +9：keys.rs 2 + runtime.rs 6 + keybindings_manager.rs 1） |
| `~/.cargo/bin/rustfmt --edition 2021 --check crates/nini-tui/src/keys.rs crates/nini-tui/src/runtime.rs` | **0** | 无输出（另跑了同样命令检查 `dispatch.rs` / `keybindings_manager.rs`，也是 0） |
| `~/.cargo/bin/cargo fmt --all -- --check` | **0** | 全仓无格式差异 |
| `~/.cargo/bin/cargo test -p nini-tui`（全量，含所有集成测试） | **0** | 全部 test binary `ok`，0 failed |
| `~/.cargo/bin/cargo clippy --workspace --all-targets` | **0** | 无 warning / error |
| `python3 scripts/keymap-parity.py` | **0** | 「Pi 有默认键的 binding id：80；已登记 80」「nini 键位行：54」（原 50，+4） |
| `~/.cargo/bin/cargo test -p nini-tui --test slash_command_e2e hotkeys` | **0** | `hotkeys_labels_every_binding_it_lists ... ok`（**反证实验**：临时删掉 `dispatch.rs` 的 4 个 `action_label` arm 后此测试 FAILED，恢复后重新 ok） |

**改动前基线**：`cargo test -p nini-tui --lib` → `413 passed; 0 failed`。

---

## ③ Blocked

无。没有需要停手等裁决的阻塞项。

---

## ④ 需人确认 / 未确认项

1. **(a) 两个文件超出本项声明的文件范围**：`commands/dispatch.rs`（`action_label` 的 4 个 arm）与 `keybindings_manager.rs`（`parse_action` 的 4 个条目 + 1 个测试）。前者**实测必需**（否则既有测试 FAILED），后者是仓库自己文档化的红线（「不登记 = 静默忽略」）+ R14 先例。已在 §1.3 登记。
2. **(b) 选区在键盘滚动后不失效**：nini 的选区锚在**屏幕行坐标**上（`selection.rs` 头注释），任何滚动都会让它指向错误的内容。鼠标滚轮（`runtime.rs:2922`）与拖拽（`:3032`）都会 `clear_selection`；**键盘翻页从来不清**（`clear_selection` 的调用点只有那两处）。任务原文里「翻页清选区」这个前提对键盘路径**不成立**。本次让细滚动与既有键盘翻页**保持一致**（都不清选区），避免同一族键盘粒度出现两种行为；由此遗留的「键盘滚动后选区陈旧」是**既有问题**，本次未扩大范围去改它 —— 是否给键盘滚动补 `clear_selection`，需人确认（会改变既有 `PageUp`/`PageDown` 的行为）。
3. **(c) 步长单位不一致（既有）**：Pi 的 `lineUp`/`halfPage` 步长是**渲染行**，nini 的 `scroll_offset` 单位是 **TranscriptLine**（一条 TranscriptLine 可能渲染成 50 行）。本次沿用 nini 自己的单位（「1 行 = 1 个 TranscriptLine」），整屏翻页本来就是这个单位，属既有差异，未在本次统一。
4. **(d) 半页公式**：实现用 `max(1, page_lines / 2)`，Pi 是 `Math.max(1, Math.floor(viewportHeight / 2))`（`tui-alt-screen.ts:736-742`）—— 形状一致；`max(1, …)` 的下限来自 Pi 的防呆（一屏只有 1 行时半页为 0 会变成死键，`runtime.rs` 有对应测试）。计划文字写的是字面 `floor(page_lines/2)`，两者只在 `page_lines < 2` 时不同（此时取 1 更合理）。**若人要求严格照计划字面**（`page_lines == 1` 时步长为 0），改 `runtime.rs:4788` 一处即可。
5. **(e) 整屏翻页与 Pi 不一致（既有，未改）**：Pi 的 `pageUp` 是 `viewportHeight - PAGE_SCROLL_OVERLAP`（重叠 4 行），nini 是整 `page_lines`。项 3 明确要求「翻页保持整屏」，故未动。
6. **(f) `docs/invariant-audit.md:199` 的「39 bindings」已陈旧**：本次后实际是 54 条键位行（`/hotkeys` 表）。该文档是**实测录屏的转录**（含「Pi 39 bindings」的对照），属既有文档，红线禁止修改 → 留待 M4 收口时一并更新。
7. **(g) 终端送达未实机验证**：`Shift+Up/Down` 与 `Alt+PageUp/PageDown` 的编码序列在 crossterm 0.28.1 里可解析（已核对源码），但**未在真实终端里按过**。极简终端（不支持 `CSI 1;2A` / `CSI 5;3~`）可能只发裸 `Up`/`Down` 或不发 —— 需实机确认；若不可用，用户可用 `keybindings.json` 改绑（`parse_action` 已登记这 4 个名字）。
8. **(h) 契约（`scripts/keymap-parity.json`）未加 4 行 `map`**：Pi 的这 4 个 id 因为 `defaultKeys: []` 不在 oracle 的必登记集合里，脚本仍是绿的（退出码 0），所以**本次不需要**改。若 M4「更新契约」希望把这层关系也写进台账，建议加：
   `tui.altScreen.halfPageUp → HalfPageUp (keys: differ)`、`halfPageDown → HalfPageDown (differ)`、`lineUp → LineUp (differ)`、`lineDown → LineDown (differ)`，理由统一写「Pi 默认无绑定，nini 默认绑定 Shift/Alt+…，属有意 UX 差异」。**这属于契约裁决，未在本项内自行决定。**
9. **(i) `parse_action` 的既有缺口**：`ScrollToBottom` / `ScrollToTop` 至今**未**登记在 `parse_action` 里（本次未顺手补 —— 超出项 3 范围）。用户改绑这两个动作同样会被静默忽略。是否补齐需人确认（属独立缺陷）。
10. **(j) 实机观感**：1 行 / 半页是否够用、四个和弦是否顺手 —— 需实机确认。

---

## ⑤ 与项 1 / 项 2 的关系

worker A 的项 1（`WHEEL_SCROLL_LINES` 3→1）与项 2（Alt×5 滚轮倍率）**未被本次触碰**；本次只加键盘路径，两条路径共用 `scroll_transcript_by_lines` 的夹紧/autoscroll 逻辑，但滚轮走的是鼠标分支（`runtime.rs:2900+`），与本次改动无交集。项 4 的拖拽增量平滑（`ScrollbarDrag` 的 `last_row` / `smooth_increment`）保持已实施状态，未回退。

## ⑥ 回滚

- 删 `keys.rs:207-231` 的 4 个变体 + `:581-595` 的 4 条绑定 → 键位回到原状（但 `runtime.rs` / `dispatch.rs` / `keybindings_manager.rs` 的引用需一并删）。
- `runtime.rs`：`scroll_transcript` 改回自含实现（删 `scroll_transcript_by_lines` / `half_page_lines`），删两个派发点的 4+4 个 arm。
- `dispatch.rs` 删 4 个 `action_label` arm、`keybindings_manager.rs` 删 4 个 `parse_action` 条目 + 1 个测试。
- 各步可独立回滚，不牵动滚轮/拖拽路径。
