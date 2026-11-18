# Changes

范围：nini TUI「鼠标复制粘贴体验」改造 —— **阶段 1 鼠标侧 + 阶段 2 复制 + 阶段 3 右键粘贴**
（R3 / R4 / R6 / R7 / R8 / R9）。
分工：worker B 起草（中途被限流打断，只写完剪贴板测试缝的一半），
主 agent 收口：补齐鼠标选区、复制、右键粘贴，并修掉 A/B 留下的编译错误与逻辑 bug。

## 改了什么

- `crates/nini-tui/src/runtime.rs:2966` | 新增选区辅助函数：
  `selection_row_at` / `selection_row_at_drag`（拖拽越界时**夹到最后一行**，
  否则拖到边缘就断了，边缘自动滚动永远启动不了）、`selection_point_at`
  （终端列 → 字符下标）、`click_count_for`（Pi `getClickCount`：500ms 窗口、
  同行同词才累加，1→字符/2→词/3→行，第四次回 1）、`granularity_for`、
  `begin_selection`、`extend_selection`、`finish_selection`、`auto_scroll_direction`、
  `auto_scroll_step`、`clear_selection` | 步骤 R3 | 变更
- `crates/nini-tui/src/runtime.rs:2910` | `Down(Left)`：不在滚动条列 → 开始选择；
  **判定顺序改了**：先判滚动条列再取 `vp.geometry()`（`geometry()` 在内容装得下时
  是 `None`，原来先取它会让短转录里的每次点击都提前 return，什么都不发生）| 步骤 R3 | 变更
- `crates/nini-tui/src/runtime.rs:2951` | `Drag(Left)`：无滚动条拖拽 → 延伸选区；
  焦点用 Pi `updateSelectionFocus` 的规则（跟**初始范围**比，跨过起点时翻转锚点）| 步骤 R3 | 变更
- `crates/nini-tui/src/runtime.rs:2977` | `Up(Left)`：收尾选区 → 触发复制 | 步骤 R3+R6 | 变更
- `crates/nini-tui/src/runtime.rs:3005` | `Down(Right)`：右键把剪贴板文本贴进提示框
  （`Ok(None)` 静默、`Err` 给状态栏提示）| 步骤 R8 | 变更
- `crates/nini-tui/src/runtime.rs:3236` | `copy_selection`：原生剪贴板优先 →
  失败则 OSC 52 兜底（**无法验证**，反馈必须写明）→ 都失败则 `Copy failed: …`；
  开关读 `settings::copy_on_select_from_settings` | 步骤 R6 | 变更
- `crates/nini-tui/src/runtime.rs:3296` | 剪贴板测试缝：`CLIPBOARD_OVERRIDE` +
  `read_clipboard_text` / `write_clipboard_text` 包装（headless CI 没有剪贴板）| 步骤 R7 | 变更
- `crates/nini-tui/src/runtime.rs:1365` | 50ms tick 分支跑一次 `auto_scroll_step`：
  边缘自动滚动（Pi `autoScrollSelection`，不新建线程/定时器）；滚动时选区行号同步平移 | 步骤 R3 | 变更
- `crates/nini-tui/src/runtime.rs:2870` / `:2934` | 滚轮与滚动条拖拽后清空选区
  （选区是屏幕行坐标，滚动会让高亮指向别的文字）| 步骤 R3 | 变更
- `crates/nini-tui/src/state.rs:1745` | UiState 新增 `selection_initial`
  （Pi `selectionInitialRange`：拖拽要跟**初始**范围比，否则跨过起点会来回翻转）、
  `selection_auto_scroll_dir`；`last_click` 的类型路径修正为 `crate::state::` | 步骤 R3 | 变更
- `crates/nini-tui/src/osc52.rs`（新增） | base64 编码 + OSC 52 写入（仓库无 base64 依赖，
  自己实现 20 行；这是 TUI 里**唯一**写 stdout 的地方，因此在
  `no_terminal_writes` 的白名单里加了一条带理由的例外）| 步骤 R6 | 变更
- `crates/nini-tui/src/runtime.rs:5310` | 新增 `mouse_selection_tests`（14 条）：
  单击选字符、拖拽选区间、向上拖选同样的文本、双击选词、三击选行、
  点击计数循环与窗口外重置、滚动条列不产生选区、边缘自动滚动平移行号、
  复制三种反馈路径、右键粘贴、用户滚动清空选区 | 步骤 R4 测试 | 变更

## 与 worker B 草稿的差异（主 agent 收口时修的）

1. **B 的草稿留了一个没有函数体的声明**（`fn read_clipboard_text(...) -> ...;`）
   → 补成完整实现，并补 `write_clipboard_text` 与测试 setter。
2. **B 的草稿把 `last_click` 的类型写成 `crate::MouseClickRecord`**（结构体在
   `state.rs` 里）→ 改成 `crate::state::MouseClickRecord`。
3. **B 的草稿没有实现鼠标选区本体**（R3）→ 主 agent 按 plan §2 的坐标系契约补齐。
4. **B 的草稿缺 `selection_initial`** → 拖拽跨越起点会来回翻转（review 发现）。

## Commands
- `cargo fmt --all -- --check` → 退出码 0
- `cargo clippy --workspace --all-targets -- -D warnings` → 退出码 0
- `cargo test -p nini-tui` → 退出码 0
- `cargo test --workspace` → 退出码 0

## Reproduction
- 复现命令: `cargo test -p nini-tui --lib mouse_selection` → 退出码 0
  （改动前：`grep -n selection crates/nini-tui/src/runtime.rs` 零命中，
  鼠标完全接不上选区）

## Blocked
none
