# nini TUI 鼠标复制粘贴体验改造 —— 阶段 1–3 收口计划

Status: **待执行**（2026-11-16）
来源：用户另一个 nini 会话（`hermes-agent`）在对话里写的 4 阶段计划 + 已落地的阶段 0/1 半成品
执行方式: 多 agent（render 侧 / runtime 鼠标侧 / settings 设置项，三个 agent 文件不重叠）

## 0. 需求原文

用户（2026-11-16）：「帮助其完成剩余所有任务。多agent,执行」
被帮助的对象 = 用户自己的另一个 nini 会话，它在给 **nini 自己的 TUI** 做「鼠标复制粘贴体验」改造。

它写的 4 阶段计划（原文见该会话 `~/.nini/agent/sessions/hermes-agent/20261114-014223_1433337.jsonl`）：

| 阶段 | 内容 | 状态 |
|---|---|---|
| 0 | 文本粘贴 + 读剪贴板 | ✅ 已完成（`clipboard.rs::read()`、`runtime.rs::paste_from_clipboard()`） |
| 1 | 鼠标文本选择 | 🔶 半成品：纯逻辑模块写完，**两端没接上** |
| 2 | 选择即复制 + OSC 52 兜底 + 反馈 | ⬜ 未开始 |
| 3 | 右键粘贴 + 协调打磨 | ⬜ 未开始 |

## 1. 现状取证

### 1.1 阶段 1 已写完的部分（`crates/nini-tui/src/selection.rs`，390 行，13 条单测全绿）

- `SelectionPoint { row, col }`、`SelectionGranularity{Character,Word,Line}`、
  `Selection { anchor, focus }`、`col_to_char_offset`（屏幕列→字符偏移，CJK 按显示宽度）、
  `word_bounds`（路径/kebab-case/snake_case 不切半）、`line_bounds`、`range_for_point`、
  `selection_text`（跨行 `\n`、上下拖等价、缺行跳过）。
- Pi 对齐已核：`SelectionGranularity` ← `references/pi-0.85.1/src/tui/tui-alt-screen.ts:213`；
  词边界 joiner ← 同文件 `:81-90`（`TERMINAL_WORD_SELECTION_JOINERS = {"/", "-"}`）。
- 状态位已加（`crates/nini-tui/src/state.rs`）：
  `AppState.screen_text_rows: Arc<StdMutex<Vec<String>>>`、
  `UiState.selection / selection_granularity / selection_dragged`。

### 1.2 两端没接上（阶段 1 剩下的活）

| 缺口 | 证据 | 后果 |
|---|---|---|
| 渲染器不发布屏幕行文本 | `grep screen_text_rows crates/nini-tui/src/render.rs` → 无命中 | 选区拿不到输入，`selection.rs` 的 5 个函数**生产引用 0 处**（只有自己单测在用） |
| 鼠标事件不接选区 | `grep selection crates/nini-tui/src/runtime.rs` → 无命中（只有无关的 `selection` 字样） | 单击/拖拽/双击/三击都没有 |
| 选区高亮 | `render.rs` 无 selection 引用 | 看不到选中了什么 |
| 阶段 2 / 阶段 3 | 无任何代码 | 选择不能复制，右键不能粘贴 |

### 1.3 Pi 的语义（要对齐的「口径」）

- 粒度：`getClickCount`（`tui-alt-screen.ts:1220`）—— 同一次词命中 + 500ms 内 → 2 次（词），
  再点 → 3 次（行），否则 1 次（字符）。
- 词/行范围：`getWordSelection`（`:1156`）/ `getLineSelection`（`:1193`）。
- 边缘自动滚动：`updateSelectionAutoScroll`（`:1249`）+ `autoScrollSelection`（`:1274`），
  间隔 **50ms**。
- 复制：`copyOnSelect` 默认 **true**（`:272`：`options.copyOnSelect ?? true`）。

## 2. 坐标系契约（三个 agent 必须一致，否则接不上）

- `screen_text_rows`：**可见屏幕行**的纯文本，顺序 = 绘制顺序（自上而下）。
  索引 = `screen_y - content_area.y`（`content_area` 由 `state.transcript_viewport` 发布，
  `render.rs:884`）。空转录时发布**空 Vec**（不能留上一帧的旧数据）。
- `SelectionPoint.row` = `screen_text_rows` 的索引；`.col` = **字符偏移**（不是显示列），
  由 `selection::col_to_char_offset(text, screen_col)` 从鼠标的显示列换算。
- 选区范围 = `anchor..focus`（有序后）；字符/词/行三种粒度的初始范围由
  `selection::range_for_point` 给出。

## 3. 改动

### 阶段 1 剩余

**R1（render 侧）发布屏幕行文本** — `crates/nini-tui/src/render.rs`
在 `render_transcript` 组装完 `visible_items` 之后，把每个可见行的纯文本写进
`state.screen_text_rows`。发布姿势照抄既有的 `state.line_heights.lock()`（`render.rs:881`）：
`AppState::clone` 共享同一个 `Arc`（`state.rs:2031`），所以从渲染用的快照写也能被
事件循环读到。
纯文本的取法：nini 自己在 `items_for_line` 里造的是 `Vec<RLine>`（ratatui `Line`），
最后才 `.map(ListItem::new)`（`render.rs:1463-1466`）——**从 `RLine` 的 spans 取**
（`sp.content` 已过 `ascii_safe`，就是屏幕上的字）。不要去 ratatui 的 `ListItem`
上找文本字段（它没有「纯文本」这回事）。

**R2（render 侧）选区高亮** — 同文件
在 `visible_items` 组装完之后，按**行索引**套用 `state.ui_state.selection` 的高亮：
区间内的字符加样式（没有现成主题键就用 `Modifier::REVERSED`）。
注意 span 是按**显示宽度**排布的，字符区间 → 显示列区间要换算（`unicode_width`）。

**R3（runtime 侧）鼠标选区** — `crates/nini-tui/src/runtime.rs`
- `Down(Left)`：点在滚动条列 → 走既有滚动条拖拽分支（**不动**）；点在 transcript
  内容区 → 开始选择：`row = mouse.y - vp.y`，`col = mouse.x - vp.x`（显示列）→
  `selection::col_to_char_offset`；点击次数定粒度（Pi `getClickCount`：
  同行 + 500ms 内 + 同一个词 → 2 次，再点 → 3 次，否则 1 次）。
  粒度为词/行时，初始选区 = `selection::range_for_point`。
  需要新状态字段（`UiState`）：`last_click`（时间戳/行/词范围/次数）。
- `Drag(Left)`：更新 focus；`selection_dragged = true`；更新边缘自动滚动方向。
- `Up(Left)`：收尾（focus 已在释放点）；`selection_dragged = false`；
  非拖动（单击）时选区 = 粒度范围；随后走阶段 2 的复制。
- **边缘自动滚动**：拖到内容区上/下边缘 → 每 **50ms** 滚 1 行（用既有
  `scroll_transcript`），并把选区行号同步平移（保持锚定在同一段内容）。
  实现挂在 `run_loop` 已有的 50ms tick 分支上（**不新建线程/定时器**）。
- **用户主动滚动**（滚轮 / 滚动条拖拽）→ 清空选区：选区是屏幕行坐标，滚动后失效。

**R4（runtime 侧）反馈呈现** — 不改渲染层
反馈走既有通道：`state.push_notice(...)`（→ `TranscriptLine::Notice`）+ `state.run_state.status`。

### 阶段 2：选择即复制

**R5（settings 侧）开关** — `crates/nini-tui/src/settings.rs`
- `Settings` 加 `copy_on_select: bool`，`#[serde(default = "default_copy_on_select")]`，
  默认 **true**（Pi `copyOnSelect ?? true`）。
- 公开 API（**runtime 侧按这个调用，不要改签名**）：
  ```rust
  pub const COPY_ON_SELECT_DEFAULT: bool = true;
  /// 从运行时 settings 文件读 `copy_on_select`；文件缺失/解析失败/字段缺失 → 默认值。
  pub fn copy_on_select_from_settings(path: Option<&std::path::Path>) -> bool
  ```
  实现走既有 `SettingsManager::load_from_disk`（读/解析失败 → `Self::default()`，
  `settings.rs:202-222`）。

**R6（runtime 侧）复制** — `crates/nini-tui/src/runtime.rs`
`Up(Left)` 收尾后：选区文本 = `selection::selection_text(&rows, &selection)`
（rows = `screen_text_rows`）；开关为真才复制。
- 首选 `crate::clipboard::copy(&text)`（原生，可验证）→ 成功反馈 `Copied N bytes`。
- 失败 → **OSC 52 兜底**：写 `\x1b]52;c;<base64>\x07` 到 stdout。OSC 52 **无法验证**
  → 反馈必须写明「已通过 OSC 52 写入，终端未必有剪贴板」，不能谎称已复制。
- 两条路都失败 → `Copy failed: <reason>`（沿用 `clipboard.rs` 的铁律：
  never report success we cannot verify）。
- **base64**：仓库没有任何 base64 依赖 → 在 runtime.rs 里实现一个小函数
  `fn osc52_base64(bytes: &[u8]) -> String` + 单测，**不加依赖**。

**R7（runtime 侧）可测试的剪贴板缝** — 同文件
headless CI 没有剪贴板 → 阶段 2/3 的测试需要能注入剪贴板内容。
加一个 thread_local 覆盖缝（不引入新依赖、不改 `clipboard.rs` 的公共 API）：
```rust
thread_local! { static CLIPBOARD_OVERRIDE: RefCell<Option<String>> = ... }
fn read_clipboard_text() -> Result<Option<String>, String>
fn write_clipboard_text(text: &str) -> Result<(), String>
```
测试用 `#[cfg(test)]` 的 setter 写入覆盖值。

### 阶段 3：右键粘贴 + 协调打磨

**R8（runtime 侧）右键粘贴** — 同文件
`Down(Right)`：命中 transcript 内容区（非滚动条列）→ `read_clipboard_text()` →
- `Ok(Some(text))` → `state.input.insert_str(&text)` + `state.refresh_completion()` + 状态提示；
- `Ok(None)` → **静默**（剪贴板没有文本是正常状态，不是错误）；
- `Err(reason)` → 状态提示 `paste failed: <reason>`。

**R9（runtime 侧）协调口径** — 写进注释
- nini **没有「输入框聚焦」概念**（输入框始终可编辑）→ 右键粘贴一律进输入框
  （与 Pi 一致）；转录区的选区与输入框互不影响。
- Pi 的右键粘贴是 Windows 行为；nini 在所有平台提供 → 注释里写明这是 nini 的扩展。

## 4. 测试

- **A（render）**：`screen_text_rows` 发布的内容与渲染一致；空转录发布空 Vec；
  选区高亮只落在区间内（区间外的 span 不带样式）。
- **B（runtime）**：模拟鼠标序列（Down→Drag→Up）→ 选区范围正确；单击=字符 /
  双击=词 / 三击=行；边缘自动滚动会平移选区行号；复制被触发且反馈文案分三种
  （原生成功 / OSC 52 兜底 / 失败）；右键粘贴把文本插进输入框。
- **C（settings）**：文件缺失 → 默认 true；`copy_on_select: false` → false；
  `true` → true；旧 settings 文件（没有这个字段）→ 默认 true。

## 5. 验收

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p nini-tui
cargo test --workspace
bash scripts/gate.sh
```

## 6. Out of scope

- 不做输入框内部的鼠标选择（nini 没有输入框焦点模型）。
- 不做 OSC 52 的**验证**（协议本身无法验证）——只做「如实反馈」。
- 不新增依赖（含 base64）。
- 不改 `handle_mouse` 的滚动条分支语义（只加「点在滚动条列 → 不进选择」的分流）。
- 不动 CHANGELOG / README（测试数变化时门禁会要求同步，那是收尾时统一做）。

## 7. 分工（文件不重叠，可并行）

| Agent | 范围 | 产物 |
|---|---|---|
| A（render 侧） | `crates/nini-tui/src/render.rs` + 渲染测试 | `docs/plans/mouse-copy-paste-changes-a.md` |
| B（runtime 侧） | `crates/nini-tui/src/runtime.rs`、`crates/nini-tui/src/state.rs` + 集成测试 | `docs/plans/mouse-copy-paste-changes-b.md` |
| C（settings 侧） | `crates/nini-tui/src/settings.rs` + 测试 | `docs/plans/mouse-copy-paste-changes-c.md` |

接口：C → B 的 `copy_on_select_from_settings`（签名见 R5）；A → B 的
`screen_text_rows` 发布（R1）与 `state.ui_state.selection`（已存在）。
