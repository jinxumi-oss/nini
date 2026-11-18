# nini TUI「上下文滚动 / 侧边栏拖动」跳跃式滚动 —— 多 agent 机制分析与修复方向

- 任务契约：`~/.pi/agent/data/uap/nini-tui-scroll-page-jump-analysis.json`（只分析，不改源码）
- 执行方式：多 agent（worker A 源码与机制定位 / worker B Pi 0.85.1 对照 / worker C 体验机制分析），主线程收口成报告
- 日期：2026-11-17；起点 `git rev-parse HEAD = 4a53c6e`（分支 main；工作区存在未提交在途改动，本轮只读取证，未改源码/测试/配置、未构建、未提交）
- 红线遵守：只分析；仅新增本报告一个产物文件（Pi 对照与机制分析都并入该报告）。

---

## 一、调查范围与方法

**范围**：用户反馈「上下文显示滚动 + 侧边栏拖动，最后一页往上翻时跳跃式滚动、跨度大、看不到逐行内容，与 Pi TUI 操作体验完全不同」——定位 nini 相关代码路径与当前粒度、分析跳跃/跨度大/看不到逐行内容的机制、给出修复方向（不实现）。

**方法（三 worker 并行只读取证 + 主收口）**：
- worker A（源码与机制定位，只读）：路径总表 + 机制线索（症状→file:行→粒度证据），产物 `/tmp/pi-work/nini-scroll-drag-granularity-workerA-r2.md`（11 条机制线索、路径总表 20 行）。
- worker B（Pi 0.85.1 对照，只读）：Pi 滚动粒度基准表 + 与 nini 现状对照，产物 `/tmp/pi-work/nini-scroll-drag-granularity-workerB.md`（12 项对照、未确认项）。
- worker C（体验机制分析 + 修复方向，只读）：症状→机制映射 S1–S9 + 修复方向 D1–D6 + 体验对比，产物 `/tmp/nini-tui-scroll-page-jump-workerC.md`。
- 证据标准：所有结论带 `file:行` + 命令/真实输出摘录；无证据写「未确认」。

---

## 二、现状：滚动与拖拽路径总表与粒度基线

（来源：worker A 路径总表；行号与当前工作树一致，含项 4 平滑改动）

| # | 路径 | 触发 | 文件:行 | 当前粒度 / 逐事件映射 | 单位 |
|---|---|---|---|---|---|
| 1 | 翻页上（PageUp） | 键 PageUp→ScrollUp | `keys.rs:517-519`→`runtime.rs:2736`/`5151` | 1 按→`scroll_transcript(±1)`；step=`state.page_lines()` | **TranscriptLine 数** |
| 2 | 翻页下（PageDown） | 键 PageDown→ScrollDown | `keys.rs:521-523`→`runtime.rs:2763`/`5152` | 同上 | TranscriptLine 数 |
| 3 | 翻页步长发布 | 渲染期每帧 | `render.rs:870-877`；`state.rs:1117-1130,2808-2810` | `page_lines = lines_needed_for_items(items_per_line, visible_height, total_lines).max(1)` | 行（换算自渲染行） |
| 4 | 翻页落地 | 用户滚动分支 | `render.rs:974-985` | 按 `scroll_offset` 逐行从底部累加 `items_per_line[i]` 跳过 | 行 → 渲染行 |
| 5 | 滚轮上/下滚 | `ScrollUp/ScrollDown` | `runtime.rs:2884-2905`（step=`wheel_lines()`,`2888`） | 1 事件→`scroll_offset ±= wheel_lines()`，clamp 到 `lines.len()-1` | **3 个 TranscriptLine** |
| 6 | 滚轮步长常量 | — | `scrollbar.rs:327-332` | `WHEEL_SCROLL_LINES = 3`（注释自称「Pi 默认 3」，与 Pi 源码冲突） | 行 |
| 7 | 滚轮单位错配 | 同 5 | `runtime.rs:2888` vs `render.rs:974-985` | 滚轮动**行**，渲染/滚动条单位是**渲染行**（`scrollbar.rs:321-326` 注释自认 gap） | 行 ≠ 渲染行 |
| 8 | 滚轮无修饰键倍率 | — | `runtime.rs:2884-2905` | 无 `m.modifiers` 判定；Pi 有 Alt×5 | 缺失 |
| 9 | 滚动条按压（起拖） | `Down(Left)` 命中滚动条列 | `runtime.rs:2906-2934` | 只写 `ScrollbarDrag`（含项4平滑字段），**不产生滚动** | — |
| 10 | 滚动条拖拽（首事件） | `Drag(Left)` | `runtime.rs:2936-2986` | 完整比例映射 `step = cur - current_offset`（Pi 完整映射） | 行 |
| 11 | 滚动条拖拽（其后事件） | 同上 | `runtime.rs:2987-3001` | 50% 低通 `(delta + smooth)/2`；转向时丢滤波状态 | 行 |
| 12 | 拖拽行换算 | — | `scrollbar.rs:293-317` | 轨道行→比例(渲染行)→`lines_needed_for_items`→行数 | 轨道行→渲染行→行 |
| 13 | 边缘自动滚动 | 拖拽中指针 | `runtime.rs:3267,3283-3296,3311-3330,962` | 方向顶/底 1 行触发；50ms/tick 步长 1 行（行/渲染行单位不等价） | 行 / 渲染行 |
| 14 | 行高测量 | 渲染期逐行 | `render.rs:794-828`（`items_per_line`，可 >1）、`881-883`（发布 `line_heights`） | 一条转录行可渲染 >1 屏幕行 | 渲染行 |
| 15 | 跳到最旧/最新 | Home/End/Shift+End | `keys.rs:406-412,527-529`→`runtime.rs:2764-2765,4745-4762` | 一次性 `scroll_offset = total-1` 或 `0` | 整段跳 |

（「侧边栏」仓库内无 side bar 组件：`rg -in "sidebar|side_bar|side panel" crates/nini-tui/src/` 无命中，用户所指应为右侧滚动条——见未确认项 2。）

---

## 三、机制分析：症状 → 机制 → 与 Pi 差异

**单位体系（贯穿全文的根因）**：nini 滚动有**两套单位**——`scroll_offset` 是**转录行（TranscriptLine）数**（从底部起算），渲染/滚动条单位是**渲染行**（一条转录行可渲染多行，`items_per_line[i] > 1`）。二者只在「每条转录行恰好 1 屏幕行」时等价；一旦消息折行/工具结果很长，一次步进的跨渲染行数就与步长设定值脱钩（`scrollbar.rs:221-232` 的 `lines_needed_for_items` 只从底部往上数，作用点 `render.rs:979-984` 从当前位置起跳）。

| # | 症状点 | 代码 file:行 | 粒度证据 | 与 Pi 差异（`references/pi-0.85.1/`） |
|---|---|---|---|---|
| S1 | **最后一页往上翻：一步跨整屏、前后两屏零重叠 ⇒ 跳跃式** | `runtime.rs:4733-4743`（`scroll_transcript`，step=`page_lines`,`4735`）；`render.rs:874-877`；`render.rs:974-985` | `page_lines = 覆盖一屏所需的行数`（整屏转录行数），渲染按这 N 行逐行累加跳过，**无任何重叠补偿**（`rg -n "PAGE_SCROLL_OVERLAP" crates/nini-tui/src/` 无命中）；模型复算：24 视口一次跳 24 渲染行 = 整屏 0 重叠 | Pi 一次 PageUp = `viewportHeight - PAGE_SCROLL_OVERLAP(4)` 行、带 4 行重叠：`tui-alt-screen.ts:74`、`:726`。nini 相邻两屏内容完全不重叠 ⇒ 「看不到逐行内容」第一来源 |
| S2 | **翻页跨度随位置/行高浮动 ⇒ 跳跃式（忽小忽大）** | `scrollbar.rs:224`（`lines_needed_for_items` 从底部数）vs `render.rs:979-984`（从当前位置起跳）；`render.rs:874-877` | `page_lines` 在底部测得（多为 1 行 → `L≈H`），在当前区域这些行可能含长工具结果（`L` 条 = 数十渲染行）；模型复算：500 行/30 视口 → `page_lines=10` 行，实际跳过 48 渲染行 = 1.6 屏；反向（最后一条是 30 行长答案）→ `L=1`，顶部区域一次只走 1 渲染行。Pi 的页长只依赖 `viewportHeight`（渲染行），位置无关 | Pi 页长恒为 `viewportHeight - 4`（行），位置无关 ⇒ nini 步长随内容行高波动 ⇒ 无恒定步长 |
| S3 | **滚轮一格跨 3 条转录行 ⇒ 步长粗、且行高不均时更粗** | `runtime.rs:2888`（step=`wheel_lines()`）；`scrollbar.rs:332`（`WHEEL_SCROLL_LINES=3`） | 步长 3 **转录行**（非 3 渲染行）；`scrollbar.rs:321-326` 自认 line-vs-cell 不等价；实测钉住 `mouse_e2e.rs:84`、`:531`；Pi 默认 1：`tui-alt-screen.ts:264` `?? 1` | Pi 滚轮默认 **1 行/事件**（渲染行单位），nini 粗 ≥3 倍且随行高放大 |
| S4 | **侧边栏拖拽：单事件跨多行 ⇒ 跨度大** | `runtime.rs:2936-3018`；`scrollbar.rs:293-317` | 比例映射 `scroll_top = round(thumb_offset/max_thumb_offset × max_scroll_top)`（渲染行）→ 行数；1 指针行 ≈ `max_scroll_top/max_thumb_offset` 渲染行（模型复算：80 行→3、500 行→17、2000 行→**52**）；实测 `mouse_e2e.rs:208-211`（一次拖跨 >5 行） | 公式与 Pi `tui-alt-screen.ts:1056-1066`、`:1068-1088` **同构**，非缺陷；差异仅在 Pi「未命中轨道即跳」、nini 只写拖拽状态 |
| S5 | **项 4 平滑后仍「一跳很大」** | `runtime.rs:2983-2986`（首事件完整映射）/`2997-2999`（低通） | 低通对恒定增量是**直通**（`delta==smooth` 时 `(d+d)/2=d`）⇒ 一次大跳仍约为原量级；模型：首事件完整映射 + 其后低通 | nini 现状**优于** Pi（Pi 无低通）；继续收窄属有意偏离 Pi |
| S6 | **看不到逐行内容：键盘侧没有「1 行」一档** | `keys.rs:193-206,517-523`；全仓无 `LineUp/HalfPage`（`rg -n "LineUp|HalfPage|lineUp" crates/nini-tui/src` 0 命中） | 键盘只有 PageUp/PageDown（步长整页）；Pi 有 `lineUp/lineDown`（`scrollBy(±1)`，渲染行）与 `halfPageUp/Down`（`floor(viewportHeight/2)`）动作，且 Pi 默认无绑定（`keybindings.ts:168-185`） | nini 连可绑定动作都没有；Pi 至少提供 1 渲染行粒度动作 ⇒ 「看不到逐行」直接来源之一 |
| S7 | **看不到逐行内容：长行内部内容被整段跳过** | `render.rs:957-971`（长行只显示尾部 `start = line_items.len() - take`）、`974-1000`（跳过按整条转录行边界） | 滚动只能按整条转录行边界跳过；一条 40 行工具结果要么整条在窗外、要么整条在窗内，其**中间若干屏**在向上翻时**从未被渲染** | Pi 滚动原语是**渲染行**（`scrollBy(±1)`，`tui-alt-screen.ts:747-751`），长行每一屏都可逐行看到（Pi ScrollView 实现不在参照子集，间接支撑；见未确认项） |
| S8 | **回到底部瞬间跳到实时尾部** | `runtime.rs:4740`（翻页 `autoscroll = next==0`）、`2897`/`3006`；`render.rs:934` 尾对齐分支忽略 `scroll_offset` | 到达 `scroll_offset==0` 后视图立即改为尾对齐 ⇒ 最后一屏被实时尾部替换 | Pi 同为一次性跳转（`tui-alt-screen.ts:472-480`），一致，非缺陷 |
| S9 | **到顶后继续上翻无反馈（卡住）** | `runtime.rs:4737`（clamp 后 `next==current`） | 夹紧后不重绘提示；「最后一页」继续上翻画面完全不动 | Pi 夹紧同样无额外反馈，基本一致；差异是 nini 的「页长不定」放大不适 |

**结论**：跳跃感/跨度大/看不到逐行的根因是**「行」的定义错配**——Pi 的「行」= 渲染行（用户可见的最小单位），nini 的「行」= 转录行（数据结构最小单位）。该错配在**翻页（S1/S2）、滚轮（S3/S7）、拖拽（S4/S5）**三个入口共同投影，与用户「最后翻起来跳、看不到逐行」的体验直接对应。

---

## 四、与 Pi 0.85.1 的粒度对照

### 4.1 Pi 侧粒度基准

| 能力 | 位置（文件:行） | 粒度定义 |
|---|---|---|
| 整页滚动 PageUp/PageDown | `tui-alt-screen.ts:724-734` | 一次跳 `viewportHeight - 4` 行（常量 `:74` `PAGE_SCROLL_OVERLAP=4`），一次性到位、无动画、**无逐行呈现** |
| 半页滚动 halfPageUp/Down | `:736-742` | `floor(viewportHeight/2)` 行/次；默认**无绑定**（`keybindings.ts:168-174`） |
| 逐行滚动 lineUp/lineDown | `:744-751` | 恒 `scrollBy(±1)`（1 渲染行/次）；默认**无绑定**（`keybindings.ts:176-182`） |
| 滚轮每事件行数 | `:166-167,264` | 选项 `wheelScrollLines`，默认 **1 行/事件**（`Math.max(1, floor(options ?? 1))`） |
| 滚轮 Alt 倍率 | `:75`、`:968-971` | 常量 5；SGR button bit3(=8) 判 Alt → ×5，否则 ×1 |
| 滚轮逐事件派发 | `:680-686`、`:973-984` | 每滚轮事件 1 次 `wheelDelta`，逐行消费、无插值 |
| 滚动条拖拽 | `:129-132,1056-1066,1068-1088,1102-1105,1113-1115` | `ScrollbarDrag{scrollView, grabOffset}`；命中 thumb→`y-thumbTop`，未命中→`floor(thumbHeight/2)` 并立即跳；每 drag 事件一次绝对比例映射、无插值、无惯性 |
| 页面滚动动画 | 全文无 `ease|animat|tween|smooth` 命中 | 瞬时跳转、无补间 |
| 顶/底跳转 | `:472-480`、`:482-495` | 一次性跳 + OSC133 prompt 边界扫描跳转 |

### 4.2 对照（是否影响本轮体验）

| 粒度项 | Pi 基准 | nini 现状 | 差异 | 是否影响「跳跃/跨度大/看不到逐行」 |
|---|---|---|---|---|
| 翻页步长与重叠 | `viewportHeight-4` 行、带 4 行重叠 | 整屏转录行数、**0 重叠**（`runtime.rs:4735`、`render.rs:874-877`） | 每页跨度多 4 行、零重叠 | **是（主因之一）**：无重叠 ⇒ 相邻屏不连续、失去接缝上下文 |
| 键盘中间档 | 半页 `floor(vh/2)`、逐行 1 行（均默认无绑定） | 只有整屏一档 | 缺中间档 | **是**：只能用整屏跳 ⇒ 更显跨度大；无 1 行档 ⇒ 无法逐行 |
| 滚轮步长 | 1 行/事件 | 3 转录行/事件（`scrollbar.rs:332`） | 粗 ≥3 倍、单位错配 | **是**：一步跳过 3 行内容 |
| 滚轮单位 | 渲染行（0.84.2 旁证；0.85.1 未直接确认） | TranscriptLine 数（`runtime.rs:2865-2871`） | 单位错配 | **是**：一步视觉跨度随行高不稳定 |
| 滚轮 Alt 倍率 | ×5 | 缺失（`runtime.rs:2884-2905`） | 缺加速档 | 间接是（加剧整屏翻页的跨度感） |
| 拖拽映射公式 | 比例映射、无插值 | 同公式（`scrollbar.rs:301-317`） | 一致 | 否（公式层面）；差异仅「未命中轨道即跳」 |
| 拖拽平滑 | 无插值 | 项 4 首事件完整 + 50% 低通 | 现状优于 Pi | 否/项 4 有意偏离 Pi |
| 边缘自动滚动 | 50ms × 1 行 | 50ms × 1 行（单位行/渲染行不等价） | 单位错配 | 间接是 |

---

## 五、修复方向建议（只出方向，不实现）

（沿用既有计划 `docs/plans/nini-tui-scroll-drag-granularity-fix-plan.md` 的项号，只补本轮新发现；每条含可追溯改动点、优先级、风险、是否需人确认。）

| 方向 | 改动点 file:行 | 优先级 | 风险/边界 | 是否需人确认 |
|---|---|---|---|---|
| **D1 翻页加重叠（对齐 Pi `PAGE_SCROLL_OVERLAP=4`）** | `runtime.rs:4733-4742`（`scroll_transcript` 步长）+ `render.rs:874-877`（`page_lines` 口径对齐到**渲染行**）；Pi 口径 `tui-alt-screen.ts:74,726` | **P0**（直接消除「相邻屏零重叠 ⇒ 看不到逐行」） | `page_lines` 是 `state.rs:1130` 的 AtomicUsize、被 `state.rs:2808` 读取，改口径牵动第二派发点 `runtime.rs:5151-5152` 与既有测试；「4 行」是渲染行，nini 无「按渲染行滚动」原语，需在步长里做渲染行→转录行换算（新增概念） | **需人确认**：落地是新增「按渲染行滚动」换算，还是按转录行近似（不等价 Pi） |
| **D2 翻页用渲染行页长（D1 的实现前提，独立看 P1）** | 根因 `render.rs:874-877` + `render.rs:979-984` + `runtime.rs:4735`；可选落地 (a) 与 D1 合并（`scroll_transcript` 内做渲染行→转录行换算，复用 `render.rs:881-883` 发布的 `line_heights`）或 (b) 整体把滚动单位改为渲染行（改动面最大，非极短路径） | P0（作为 D1 前提）/ 独立看 P1 | (b) 会推翻 `scroll_offset` 单位约定、`scrollbar.rs:293-317`、`runtime.rs:2886-2905` 与既有测试（`scrollbar_units.rs`、`scroll_affordance.rs`、`transcript_scroll_to_top.rs`）；(a) 仅新增一次换算 | **需人确认**：(a) 还是 (b)（本次最关键口径分歧） |
| **D3 滚轮步长 3→1（Pi 口径）** | `scrollbar.rs:332`（`WHEEL_SCROLL_LINES` 3→1）+ `scrollbar.rs:642-643` 自测（等值式，无需改）；测试期望 `mouse_e2e.rs:84`、`:531` 改为 1 并逐条说明「钉旧步长」 | **P0**（最小改动） | ① 单位仍是转录行：改 1 后长行区仍可能跨数十渲染行（只解决「粗」不解决「跳」）；② `scrollbar.rs:321-326`/`:331` 注释自认「Pi 默认 3」需同步改 Pi 源码口径 1（`tui-alt-screen.ts:264`），避免继续误导 | 否（Pi 源码口径明确；注释冲突口径沿用上轮结论） |
| **D4 暴露逐行 / 半页滚动动作（Pi 有，nini 无）** | `keys.rs:193-195`（新增 `LineUp/LineDown/HalfPageUp/HalfPageDown`）+ `keys.rs:517-523` 绑定（**默认不绑定**，与 Pi `keybindings.ts:168-185` `defaultKeys: []` 一致）+ `runtime.rs:2736/2763` 与 `runtime.rs:5151-5152` 两个派发点各加分支（保证穷尽） | **P1**（让用户可自行变细，是「逐行可见」正解） | ② 默认不绑定 ⇒ 用户默认感知不到；若目标是「默认手感变细」应给默认键（Pi 没有）⇒ **口径需人确认**；③ 步长单位（渲染行还是转录行）失真 | **需人确认**（默认绑定与否；步长单位） |
| **D5 滚轮 Alt ×5 加速** | `runtime.rs:2884-2905` 滚轮分支加修饰键判定（Pi：`tui-alt-screen.ts:75,968-971` 按 SGR bit3 判 Alt） | P1（与上轮计划项 2 一致） | 终端对 Alt+滚轮上报方式需按 `m.modifiers` 实际验证 | 否（沿用上轮结论） |
| **D6 拖拽更细粒度（项 4 已部分实现，评估是否继续）** | `runtime.rs:2983-3000`（首事件完整映射 + 50% 低通；若要更细：首事件也改增量式，或调小低通系数/改增量累积模型） | P2（现状已优于 Pi；用户仍觉「跳」时才继续） | 继续收窄=进一步偏离 Pi（Pi `:1068-1088` 无低通），须显式登记；低通对恒定增量直通 ⇒ 仅压骤增，收窄效果有限 | **需人确认**（上轮计划项 4 已标 P2） |

**明确不建议改动的项（边界）**：`render.rs:957-971` 长行 tail-truncate（Pi 同样按视口裁剪，正常裁剪非缺陷）；`runtime.rs:4740`/`:2897`/`:3006` 的 `autoscroll = next==0` 与 Pi `scrollToEnd` 一致；`scrollbar.rs:293-317` 拖拽比例公式与 Pi 一致。

---

## 六、体验对比结论（nini vs Pi）

1. **翻页粒度**：nini 一次 PageUp = 若干条转录行，步长在转录底部测量、在当前区域生效，实际跨渲染行数在 **1 行~数屏**之间浮动（S2）；Pi 一次 PageUp = **`viewportHeight-4` 个渲染行**，位置无关、恒定。
2. **翻页连续性**：Pi 相邻两屏**保留 4 行重叠**；nini **0 重叠** ⇒ 每次翻页后旧屏内容一行都不留，是「看不到逐行内容」最直接的来源（S1）。
3. **逐行可见性**：Pi 滚动原语就是**渲染行**（`scrollBy(±1)`，`tui-alt-screen.ts:747-751`），长行可逐屏看；nini 原语是**转录行**，长行中间若干屏在向上翻时**从未被渲染**（S7），且键盘侧**没有任何一档是 1 渲染行**（S6）。
4. **滚轮细度**：Pi 默认 **1 行/事件**（`tui-alt-screen.ts:264`）；nini **3 转录行/事件**（`scrollbar.rs:332`），稳态即 Pi 的 3 倍、长行区更甚（S3）；Pi 另有 Alt×5，nini 缺（D5）。
5. **拖拽跨度**：公式与 Pi 一致（S4）；nini 额外做了项 4（首事件完整 + 50% 低通），单次体验优于 Pi，但首事件完整映射 + 低通对恒定增量直通 ⇒ 大跨度仍在（S5）。
6. **总评**：差异不在功能有无，而在**「行」的定义**：Pi 行=渲染行（用户可见最小单位），nini 行=转录行（数据结构最小单位）；跳跃感、跨度大、看不到逐行，都是这个单位错配在翻页/滚轮/拖拽入口上的投影。

---

## 七、未确认项（不自行发明业务规则）

1. **Pi 0.85.1 的 `components/scroll-view.ts` / `layout.ts` 不在参照子集**：`scrollTop` 精确单位、overscroll 语义、thumb 公式未直接确认（仅 0.84.2 旁证：`scrollBy` 按整数行、`contentHeight` 按内容行）。
2. **Pi 应用层是否覆盖 `wheelScrollLines`**：0.85.1 构造点不在子集；0.84.2 旁证（`packages/tui/...` 仅测试显式传 3、应用层无覆盖 → 走默认 1）⇒ 0.85.1 实机取值**未确认**。
3. **「侧边栏」是否就是右侧滚动条**：仓库内无 side bar 组件（rg 无命中）⇒ 若用户指其他组件则未确认。
4. **「最后一页」是否特指 `autoscroll==true` 的尾部视图**：未确认。
5. **nini 真实会话的行高分布**（长工具结果占比）⇒ 决定 S2 放大幅度；本轮只读约束未跑二进制实测 ⇒ **未确认**。
6. **量化数字**（3/17/52 行每指针行、48 渲染行等）来自按源码算法复算的 python 模型（`/tmp/sim.py`），**非真机实测**，受终端宽度/指示行占位影响可能有偏差。
7. **低通是否在真实交互中产生 `step==0`**（取决于指针行距与行高分布）⇒ 未确认。
8. 任务书指定的 worker A/B 产出在本轮执行时路径与脚本不匹配（A 产出 `/tmp/pi-work/...-r2.md`），本报告结论已由 C 独立复核仓库行号，A/B 产物可交叉核对。

---

## 八、证据索引

- worker A（路径总表 + 机制线索）：`/tmp/pi-work/nini-scroll-drag-granularity-workerA-r2.md`
- worker B（Pi 基准表 + 对照）：`/tmp/pi-work/nini-scroll-drag-granularity-workerB.md`
- worker C（症状→机制映射 + 修复方向 + 体验对比）：`/tmp/nini-tui-scroll-page-jump-workerC.md`
- 只读命令记录（代表性）：`rg -n "LineUp|HalfPage|lineUp" crates/nini-tui/src`（0 命中）；`rg -n "PAGE_SCROLL_OVERLAP|ALT_WHEEL" crates/nini-tui/src`（无命中）；`rg -in "sidebar|side_bar|side panel" crates/nini-tui/src`（无命中）；Pi 侧 `rg -n "PAGE_SCROLL_OVERLAP|wheelScrollLines" references/pi-0.85.1/src/tui/tui-alt-screen.ts`（74/75/167/264/970 命中）。
- 相关既有产物：`docs/ux-reports/nini-tui-scroll-drag-granularity-investigation.md`（上轮调查）、`docs/plans/nini-tui-scroll-drag-granularity-fix-plan.md`（上轮计划，项 1–5）、`/tmp/nini-tui-scroll-drag-increment-smooth-changes.md`（项 4 实施报告）。
