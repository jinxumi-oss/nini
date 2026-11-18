# nini TUI「上下文滚动 / 鼠标拖拽」粒度调查（多 agent 只读取证 + 行为实测）

- 任务契约：`~/.pi/agent/data/uap/nini-tui-scroll-drag-granularity.json`（原始需求逐字、DoD、红线、决策规则）
- 执行方式：多 agent（worker A 源码取证 / worker B Pi 对照 / worker C 行为实测），主线程收口
- 日期：2026-11-17；起点 `git rev-parse HEAD = 4a53c6e`（分支 main，工作区存在未提交的 in-flight 改动，本轮只读，未触碰）
- 红线遵守：未改任何源码/测试/配置，未提交，未新增依赖；只新增本调查产物文件。

---

## 一、调查范围与方法

**范围**：`crates/nini-tui` 下「上下文滚动」（transcript 滚动：滚轮、键盘翻页、滚动条拖拽、页面跳转、边缘自动滚动）与「鼠标拖拽」（滚动条拖拽、选区拖拽、拖到边缘自动滚动）的全部代码路径、粒度常量与逐事件映射；对照 Pi 0.85.1 参考行为作粒度基准。

**方法（三 worker 并行 + 主收口）**：
- worker A（源码取证，只读）：`rg -n` + `sed -n` 取证路径总表、粒度常量清单、渲染/状态字段，产出 `/tmp/pi-work/nini-scroll-drag-granularity-workerA.md`（177 行）。
- worker C（行为实测，可运行测试）：`cargo test -p nini-tui` 仅跑 6 个滚动/鼠标相关测试目标，记录真实退出码与输出，确认当前粒度被测试钉住，产出 `/tmp/nini-worker-c-behavior-report.md`（138 行）。
- worker B（Pi 0.85.1 对照，只读）：按 Pi 源码取粒度基准与差异，产出 `docs/plans/context-scroll-granularity-pi-baseline.md`（本轮收口时已并入本报告，单独文件不再保留）。
- 证据标准：所有结论带「文件:行 + 命令/真实输出摘录」；未拿到证据的显式标注「未确认」。

---

## 二、现状：上下文滚动与鼠标拖拽的路径与粒度

（来源：worker A 路径总表与常量清单；行号均为真实 `rg`/`read` 输出）

### 2.1 上下文滚动路径

| 路径 | 触发 | 文件:行 | 逐事件/逐动作映射 | 当前粒度 |
|---|---|---|---|---|
| 滚轮上/下滚 | `MouseEventKind::ScrollUp/Down` | `runtime.rs:2863-2872` | 1 事件 → `scroll_offset +=/− step`，clamp 到 `lines.len()-1` | `step = wheel_lines() = 3`（`runtime.rs:2865`、`scrollbar.rs:327-332`） |
| 滚轮单位错配 | 同上 | `runtime.rs:2865-2871` vs `render.rs:1023-1035` | 滚轮动的是 `TranscriptLine` 数，滚动条/渲染单位是**渲染行** | 3 行「行」≠ 3 渲染行（`scrollbar.rs:321-326` 注释自认 gap） |
| 滚轮无修饰键加速 | — | `runtime.rs:2863-2881` | 全段无 `m.modifiers` 判定 | 无 Alt 倍率（`rg "ALT_WHEEL" src/` 无命中） |
| 键盘翻页 | `PageUp/PageDown` → `ScrollUp/Down` 动作 | `keys.rs:517-523` → `runtime.rs:2736/2763` | 1 按 → `scroll_transcript(±1)` | `scroll_transcript` 实际步长 = `state.page_lines()`（`runtime.rs:4660`）＝**整屏**（渲染期发布 `render.rs:874-877`） |
| 页面跳转 | `ScrollToTop/Bottom` | `keys.rs:406-412/527-529` → `runtime.rs:2764-2765/4757-4762` | 1 按 → 一次性跳（`scroll_to_top` 设 `total-1`） | 整段跳，无中间态 |
| 滚动条拖拽 | `Down` 命中滚动条列 → `Drag/Up` | `runtime.rs:2883-2907` / `2909-2943` / `2945-2954` | 按下只写 `scrollbar_drag`（不滚动）；拖拽 1 事件 → 比例映射写 `scroll_offset`；松手结束 | 比例映射（非固定步长）：`scrollbar.rs:301-317` |
| 滚动条拖拽行→行换算 | `line_offset_for_thumb_offset` | `scrollbar.rs:293-317` | `scroll_top = round(thumb_offset/max_thumb_offset × max_scroll_top)`（行）→ `items_skipped` → `lines_needed_for_items` | 像素粒度 = `max_scroll_top / max_thumb_offset` 行 |
| 边缘自动滚动 | 拖拽中指针位置 | `runtime.rs:3208-3221`（方向）/ `3236-3265`（步进）/ `3259-3262`（选区平移） | 顶/底 1 行触发；每 tick（50ms）`scroll_offset ±1 行`；选区按 1 渲染行平移 | 触发带 1 行、步长 1 行、选区单位 1 渲染行（两单位不等价） |

### 2.2 鼠标拖拽路径

| 路径 | 触发 | 文件:行 | 逐事件/逐动作映射 | 当前粒度 |
|---|---|---|---|---|
| 滚动条拖拽 | `Down/Drag/Up(Left)` 命中滚动条列 | `runtime.rs:2883-2945` + `scrollbar.rs` | 按下→记 `grab_offset`（i32 可负）；每个 `Drag` 事件 → 比例映射写 `scroll_offset`；松手→结束 | 逐事件比例映射，一次事件可跨多行（见 C 的实测证据） |
| 选区拖拽（字符粒度） | `Drag(Left)` 且无滚动条拖拽 | `runtime.rs:3129-3196` | 1 事件 → focus = 指针所在字符 `(col, col+1)`（`runtime.rs:3154-3156`） | 字符级，拖动全程不随速度/位置改粒度 |
| 选区拖拽（词/行粒度） | 按下时由双击/三击决定 | `runtime.rs:3057-3112`（`click_count_for`、`granularity_for`、`begin_selection`） | 拖动全程沿用按下的粒度（词=词边界，行=整屏幕行） | 按下瞬间定粒度，拖动不动态调整 |
| 拖到边缘自动滚动（方向） | 拖拽中指针位置 | `runtime.rs:3208-3221` | 顶/底 1 行触发 | 触发带仅 1 行 |

### 2.3 粒度基线常量清单（关键）

| 常量 | 值 | 文件:行 | 作用 |
|---|---|---|---|
| `WHEEL_SCROLL_LINES` | `3` | `scrollbar.rs:332` | 滚轮每事件行数（注释自称 Pi 默认，与 Pi 源码冲突，见第四节） |
| `wheel_lines()` | 返回上值 | `scrollbar.rs:327-329` | 滚轮步长读取口 |
| 滚轮步长自测 | `3` | `scrollbar.rs:642-643` | `assert_eq!(wheel_lines(), 3)` |
| 拖拽比例公式 | 见 `scrollbar.rs:301-317` | `scrollbar.rs:293-317` | `round(ratio × max_scroll_top)` 行映射 |
| `ScrollbarDrag.grab_offset` | `i32`（可负） | `runtime.rs:2790-2800` | 按下点相对 thumb 顶的行偏移 |
| 自动滚动 tick | `50 ms`、步长 1 行 | `runtime.rs:962`、`3243-3246` | 边缘自动滚动节奏 |
| 双击判定窗口 | `DOUBLE_CLICK_INTERVAL_MS = 500` | `runtime.rs:3063` | 双击/三击粒度切换 |
| 点击计数上限 | `(prev.count % 3) + 1` | `runtime.rs:3071` | 1/2/3 → 字符/词/行 |
| 选区行平移 | `1` 渲染行/tick | `runtime.rs:3259-3262` | 与「1 行」不等价（`line_heights` 可 >1） |

### 2.4 行为实测结论（worker C，真实命令输出）

- 6 个相关测试目标全部可运行、退出码 0，合计 `40 passed / 0 failed / 0 ignored`。
- **当前粒度被测试钉住**：
  - 滚轮 = 每事件 **3 行**：`crates/nini-tui/tests/mouse_e2e.rs:84`（`assert_eq!(up, 3, "Pi's wheelScrollLines default")`）、`:531`（未渲染仍为 3）。
  - 拖拽 = 逐事件连续映射、一次事件可跨 **>5 行**（非 1 行一跳）：`mouse_e2e.rs:208-211`（`assert!(after > 5, ...)`）、`:404-408`（抓点差 2 行 → 同指针落点不同）、`:333-343`（仅 Drag 驱动位移，`Moved` 忽略）。
  - 粒度单位是**渲染行**而非转录行：`scrollbar_units.rs:211-218`（落点 = `line_offset_for_thumb_offset(&f.heights, ...)`）。
  - 弹窗滚轮粒度 = 1 项/事件（`mouse_overlay_wheel.rs:68/83/97`），与转录本 3 行/事件不对称。
- 无失败项；无未确认项（测试目标全部可运行）。

---

## 三、与 Pi 0.85.1 的粒度对照（worker B 基准）

Pi 侧基准（可执行取证）：
- 滚轮默认 **1 行/事件**（`tui-alt-screen.ts:264` `?? 1`）；Alt 倍率 **×5**（`:75` 常量、`:968-971` 判定）。
- 逐行滚动 `scrollBy(±1)`；`lineUp/lineDown` 步长恒 1 行；`halfPageUp/Down` = `floor(viewportHeight/2)`，默认无绑定。
- PageUp/PageDown = **一次一页**，页长 = `viewportHeight - PAGE_SCROLL_OVERLAP(4)`。
- 滚动条拖拽：`ScrollbarDrag{scrollView, grabOffset}`；命中 thumb → `y - thumbTop`，**未命中轨道 → `floor(thumbHeight/2)` 并立即跳转**；每个 drag 事件调用一次 `scrollScrollbarToPointer`（比例映射、无插值）。
- Pi 的 context/状态条不是可滚动区（statusContainer 为普通 `Container`）。

Pi 侧基准（可执行取证，worker B 原文见 `docs/plans/context-scroll-granularity-pi-baseline.md` 已并入本文件）：

| 能力 | 位置（文件:行） | 粒度定义 |
|---|---|---|
| 滚轮每事件行数选项 | `tui-alt-screen.ts:166-167` | 选项声明：「Number of logical lines moved for each mouse-wheel event.」 |
| 滚轮默认值 | `tui-alt-screen.ts:264` | **默认 1 行/事件**（`Math.max(1, Math.floor(options.wheelScrollLines ?? 1))`） |
| Alt 倍率常量 | `tui-alt-screen.ts:75` | `ALT_WHEEL_SCROLL_MULTIPLIER = 5` |
| Alt 倍率判定 | `tui-alt-screen.ts:968-971` | SGR button bit3(=8) 判 Alt → ×5，否则 ×1 |
| 滚轮逐事件派发 | `tui-alt-screen.ts:680-686` | 每滚轮事件 1 个 wheel 事件，`wheelDelta = 方向 × 行数` |
| 逐行滚动 | `tui-alt-screen.ts:747-751` | `scrollBy(±1)`，步长恒 1 行；默认无绑定 |
| 整页滚动 | `tui-alt-screen.ts:725-734` | 一次一页，页长 `viewportHeight - PAGE_SCROLL_OVERLAP(4)`（`:74`） |
| 半页滚动 | `tui-alt-screen.ts:736-742` | `floor(viewportHeight/2)`，默认无绑定 |
| scrollToTop/Bottom | `tui-alt-screen.ts:472-480` | 一次性跳转（`scrollToStart/scrollToEnd`） |
| scrollToPrompt | `tui-alt-screen.ts:482-495` | 按 1 行扫描找 OSC133 prompt 边界后一次跳转（ctrl+up/down 绑定） |
| 滚动条拖拽结构 | `tui-alt-screen.ts:129-132` | `ScrollbarDrag{scrollView, grabOffset}` |
| 抓取偏移计算 | `tui-alt-screen.ts:1102-1105` | 命中 thumb → `y - thumbTop`；未命中 → `floor(thumbHeight/2)` 并立即跳转 |
| 拖拽逐事件映射 | `tui-alt-screen.ts:1068-1088` | 每 drag 事件调一次 `scrollScrollbarToPointer`，无节流/插值 |
| 比例映射公式 | `tui-alt-screen.ts:1056-1066` | `scrollTop = round(thumbOffset/maxThumbOffset × maxScrollTop)`，无插值 |
| 拖拽释放 | `tui-alt-screen.ts:1113-1115` | 释放即停，无惯性 |
| 选择拖拽边缘自动滚动 | `tui-alt-screen.ts:1264-1285` | 50ms 定时，每次 `scrollBy(direction)` 走 1 行 |
| 滚动条模式设置 | `core/settings-manager.ts:144,1222-1225` | `auto/always/hidden`，无粒度设置；全文无 wheel 设置 |

关键差异（详见 Pi 对照表）：
1. **滚轮步长**：nini 3 vs Pi 1 → 不一致（「细粒度不够」的直接表现）。
2. **滚轮 Alt 倍率**：nini 缺失 vs Pi ×5 → 缺失（多终端滚轮加速能力不足）。
3. **键盘滚动手感**：nini 只有整屏一档（`scroll_transcript` 步长 `page_lines()`）vs Pi 有 1 行/半页（默认无绑定，但粒度粒度更细）→ 「跨度太大」的键盘侧表现。
4. **滚动条拖拽**：映射公式与 Pi 一致（无插值），结构对应，**不属粒度缺陷**；但 Pi 未命中轨道会立即跳转、nini 不跳（`runtime.rs:2904` 恒用 thumb 公式）。
5. **context 条**：双方都不是可滚动区，一致。

---

## 四、结论：哪些属于「细粒度不够 / 跨度太大」

| # | 粒度项 | 现状（证据） | 问题定性 |
|---|---|---|---|
| 1 | 滚轮每事件行数 | `WHEEL_SCROLL_LINES = 3`（`scrollbar.rs:332`） | **细粒度不够**（Pi 基准 1 行/事件）；且注释与 Pi 源码冲突（`v0.10-pi-parity-refactor.md:1541` 写 3，Pi 源码 `tui-alt-screen.ts:264` 为 1） |
| 2 | 滚轮 Alt 倍率 | 无（`runtime.rs:2863-2881` 无 modifiers 判定） | 加速缺失（Pi ×5），多终端体验偏弱 |
| 3 | 键盘滚动手感 | 只有整屏一档（`scroll_transcript` 步长 `page_lines()`，`runtime.rs:4660`） | **跨度太大**（单次动作跨度 = 整屏）；Pi 有 1 行/半页粒度（默认无绑定） |
| 4 | 滚动条拖拽逐事件跨度 | 逐事件比例映射、一次事件可跨 >5 行（`mouse_e2e.rs:208-211` 实测） | 与 Pi 公式一致，属「逐事件连续映射」；是否需要进一步收窄单事件跨度 → **需人确认**（见修复计划） |
| 5 | 选区拖拽粒度 | 字符级（`runtime.rs:3154-3156`） | 粒度已细（字符），不属本诉求 |
| 6 | 边缘自动滚动 | 触发带 1 行、步长 1 行、选区 1 渲染行 | 节奏偏细但非「跨度太大」主因；两单位不等价需留意（`runtime.rs:3243-3262`） |

---

## 五、未确认项（全部显式标注，未自行发明业务规则）

1. **Pi 应用层实际传入的 `wheelScrollLines` 值**：`TuiAltScreen` 构造在 `createInteractiveTui`（`ui/interactive-mode.ts:537-543`），该文件不在参照子集内 → 实际传入值**未确认**。
2. **Pi 的 `layout.ts` / `scroll-view.ts` 不在参照子集**：`getScrollbarGeometry`、`overscroll` 语义、行高不均处理**未确认**。
3. **仓库文档与 Pi 源码的冲突口径**：`docs/plans/v0.10-pi-parity-refactor.md:1541` 称 Pi 滚轮默认 3，Pi 源码为 1 → 需人确认以哪份为准。
4. **滚轮步长改 1 的实机观感**：静态代码不可判定 → **未确认**（需实机测量）。
5. **Pi 侧 context 条组件源码不在子集**：Pi 状态条是否自行滚动**未确认**。
6. **Pi 实际滚轮/翻页粒度是否与子集内基准一致**（App 覆盖是否传值）**未确认**。

---

## 六、证据索引

- worker A 原始取证：`/tmp/pi-work/nini-scroll-drag-granularity-workerA.md`（路径总表、常量清单、渲染/状态字段、命令摘录）
- worker C 原始行为实测：`/tmp/nini-worker-c-behavior-report.md`（6 目标退出码、粒度断言体现）
- worker B 原始 Pi 对照（已并入本报告第三、四节；仓库独立文件本轮已删除）：`docs/plans/context-scroll-granularity-pi-baseline.md`（Pi 基准表、对照表、未确认项）
- 相关既有测试（钉住当前粒度）：`crates/nini-tui/tests/mouse_e2e.rs`、`scrollbar_units.rs`、`mouse_overlay_wheel.rs`、`scrollbar_e2e.rs`、`scroll_indicator_no_overlap.rs`、`context_bar_wiring.rs`
