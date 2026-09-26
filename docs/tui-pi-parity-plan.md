# Nini TUI 与 Pi TUI 渲染同步计划(精简版)

> v1: 在我完成调研并写出 635 行计划后,用户指出我高估了复杂度。
> 本版是诚实重写 —— 列出**真正有视觉差异**的部分,排除 nini 不需要的过度设计。

---

## 0. 关键事实

| 数据 | 值 |
|---|---|
| pi-tui 整个 JS 库 | 4336 行,17 个组件 |
| editor.js(最大单组件) | 1960 行(包含 kill ring + undo + kitty + IME + 历史 + 自动补全) |
| markdown.js | 805 行 |
| 平均组件 | <100 行 |
| interactive-mode/components | 7601 行,但 ~60% 是 pi 专属(session tree / oauth / mermaid 等) |
| Pi 实际用到的 theme 色槽 | **39 个**(不是 50+) |

**结论**:Pi TUI 看起来复杂,是因为它支持扩展 runtime 和很多 pi-specific selector。但**nini 真正需要补的视觉差异约 1500 行新代码 + 800 行测试**,不是 5000 行。

---

## 1. 排除项(不需要做)

| 我之前列的 | 为什么不做 |
|---|---|
| **完整 Component trait 抽象** | nini v1.0 不做 extension runtime,纯函数 `render_xxx(state, theme, area)` 已经够用。Pi 之所以要 Component interface 是因为它要支持 `ctx.ui.custom()` 注入 |
| **VStack / HStack / Stack 布局原语** | nini 已经在用 `ratatui::Layout`,约束足够。Pi 自己造一遍是因为它没 ratatui |
| **pulldown-cmark 解析** | 已经在 Cargo.toml 了(`pulldown-cmark = "0.13"`),只是没用起来 |
| **syntect syntax highlighting** | 不是必需,只影响代码块彩色显示。Pi 自己也是 cli-highlight,**默认关闭**,只在用户传 `highlightCode` 才开 |
| **完整主题 schema 验证** | Pi 用 TypeBox,因为 JS 弱类型;nini Rust 端 enum 已经强类型 |
| **LaTeX 渲染** | Pi 用 katex,~100KB 依赖。nini v1.0 不需要 |
| **Kitty/iTerm2 图像协议** | 可选,大多数终端不支持。降级到 `[filename]` 已够用 |
| **Mouse wheel + Kitty CSI u** | 锦上添花,不影响视觉 |
| **IME focusable / CURSOR_MARKER** | 锦上添花,nini 的 ratatui 不直接管硬件 cursor |
| **OAuth / ScopedModels / TreeSelector / SessionSelector 完整 UI** | pi 专属功能 |
| **Markdown mermaid 扩展** | 扩展,不是核心 |
| **Theme watcher 文件变化联动** | nini 已经支持 `--use-theme` 启动时加载,运行期切换不是核心需求 |

---

## 2. 真正需要做的(13 项)

### Phase 1: Theme 扩展 🟠
**目标**: 15 色槽 → 39 色槽(对齐 Pi 实际使用的)

**改动**:
- `crates/nini-tui/src/theme.rs::COLOR_NAMES` 扩展:
  - 加 `border`, `borderAccent`(已有 `borderMuted`)
  - 加 `scrollbarThumb`, `selectedBg`, `searchMatchBg`, `searchMatchText`
  - 加 `userMessageBg`, `userMessageText`
  - 加 `customMessageBg`, `customMessageText`, `customMessageLabel`
  - 加 `toolSuccessBg`, `toolErrorBg`, `toolTitle`, `toolOutput`(已有 `toolPendingBg`)
  - 加 `mdHeading`, `mdLink`, `mdLinkUrl`, `mdCode`, `mdCodeBlock`, `mdCodeBlockBorder`, `mdQuote`, `mdQuoteBorder`, `mdHr`, `mdListBullet`
  - 加 `toolDiffAdded`, `toolDiffRemoved`, `toolDiffContext`
  - 加 `thinkingText`, `bashMode`
- `dark_palette()` / `light_palette()`:对照 Pi 实际 JSON 同步颜色(暗主题用 Pi 的 `#569cd6` accent 等)
- 加 `Theme::fg()` / `Theme::bg()` 通用方法(同时接受 bold/italic 修饰)

**依赖**: 无
**预估**: ~150 行

---

### Phase 2: DynamicBorder 🟡
**目标**: 替换硬编码 ASCII 分隔符

**改动**:
- 新建 `crates/nini-tui/src/dynamic_border.rs`
  ```rust
  pub struct DynamicBorder<'a> { color_fn: &'a str }
  // 渲染 ─ × width, 用 theme.fg_style(color)
  ```
- 替换 `rich.rs::render_divider`(60 字符硬编码 → 自适应宽度)
- 替换 prompt 框的 `Borders::TOP` 标题(用 `─ input ─` 风格)
- 替换 status bar 与 transcript 之间的分隔

**依赖**: Phase 1
**预估**: ~50 行

---

### Phase 3: Hyperlinks (OSC 8) 🟠
**目标**: 真正实现 `auto_link`(目前是 stub)

**改动**:
- 重写 `crates/nini-tui/src/hyperlink.rs`
  - 正则匹配 `https?://...` URL
  - 包 OSC 8:`\x1b]8;;URL\x1b\\TEXT\x1b]8;;\x1b\\`
  - 终端能力探测(检查 `$TERM`, `$TERM_PROGRAM`, `$KITTY_WINDOW_ID`, `$ITERM_SESSION_ID`)
  - 不支持 OSC 8 的终端降级为纯文本

**依赖**: 无
**预估**: ~80 行

---

### Phase 4: Markdown 重构 🟠
**目标**: 用已有的 `pulldown-cmark` 替换手写解析器

**改动**:
- 重写 `crates/nini-tui/src/markdown.rs`
- 用 `pulldown-cmark::Parser` 解析
- 字段对应 `MarkdownTheme`(参照 Pi 的 `getMarkdownTheme()`):
  - heading:accent + bold,**h1 加 underline**
  - link:label underline(`mdLink`)+ URL 括号(`mdLinkUrl`)
  - code block:` ``` ` 边框(`mdCodeBlockBorder`)+ 内容(`mdCodeBlock`)+ 2 空格缩进
  - blockquote:`│ ` 前缀(`mdQuoteBorder`)+ 内容 italic(`mdQuote`)
  - list:unordered `- `,ordered `1. `,task `[x]/[ ]`,**4 空格缩进**
  - hr:`─` × width (`mdHr`)
  - **行间空行**:块元素后自动空行
- 集成 hyperlink(Phase 3 的 auto_link)
- 暂不集成 syntect(syntax highlighting 是可选,以后再加)

**依赖**: Phase 1, Phase 3
**预估**: ~400 行

---

### Phase 5: Editor 改造 🟠
**目标**: 在现有 `render_prompt` 基础上加 padding + 主题边框 + 多行 `❯`

**改动**:
- 修改 `crates/nini-tui/src/render.rs::render_prompt`
  - 加 `paddingX = 1`(左右各 1 空列)
  - 边框色 = `theme.fg_style("borderMuted")`(跟随主题)
  - 多行输入后续行画退色 `❯`(dim success)
  - 标题从 `input` 改为 `─ input ─`(用 DynamicBorder)
- 不重写 editor 核心(光标/kill ring/undo 已有)

**依赖**: Phase 1, Phase 2
**预估**: ~80 行(改造现有 50 行 + 新增 30 行)

---

### Phase 6: Tool execution box 🟠
**目标**: tool call/result 用背景色 Box 包裹

**改动**:
- 修改 `crates/nini-tui/src/rich.rs::render_tool_call`
  - 用 `bg("toolPendingBg")` 包装标题行
  - 标题色用 `toolTitle`,args 用 `toolOutput`
- 修改 `render_tool_result`
  - 成功:`bg("toolSuccessBg")`;失败:`bg("toolErrorBg")`
  - "Took 1.2s" pill 用 `bg("toolPendingBg")` 做小色块(替代当前贴在 label 后)

**依赖**: Phase 1
**预估**: ~100 行

---

### Phase 7: Message containers 🟠
**目标**: 用户消息背景色 box + 助手消息 padding

**改动**:
- 修改 `crates/nini-tui/src/rich.rs::render_user_message`
  - 用 `bg("userMessageBg")` 包装整行
  - `> ` 前缀色用 `userMessageText`
- 修改 `render_assistant_message`
  - 用 `paddingX=0, paddingY=0` 包装(避免与 tool execution 间出现额外空行)

**依赖**: Phase 1
**预估**: ~80 行

---

### Phase 8: Diff 渲染 🟡
**目标**: 用 3 个专用 diff 色槽替代 success/error

**改动**:
- 修改 `rich.rs::render_diff`
  - `+` → `fg("toolDiffAdded")`(通常是 success 绿,但独立色槽)
  - `-` → `fg("toolDiffRemoved")`(error 红)
  - ` ` / `…` → `fg("toolDiffContext")`(muted 灰)

**依赖**: Phase 1
**预估**: ~20 行

---

### Phase 9: SelectList 组件 🟠
**目标**: autocomplete + command palette + 6 个 selector 全部用同一个组件

**改动**:
- 新建 `crates/nini-tui/src/components/select_list.rs`
  - `SelectList { items, selected_index, max_visible, theme }`
  - `render(width) -> Vec<String>`
  - 主列(命令名)+ 描述列(对齐 32 列宽)
  - 滚动 + 滚动指示 `(N/M)`
  - 键盘:Up/Down wrap-around,Enter,Esc
- 修改 `render_completion_popup`:实例化 SelectList
- 修改 `command_palette.rs`:复用 SelectList
- 修改 `selectors/{model,thinking,session,tree,trust,settings}.rs`:全部用 SelectList

**依赖**: Phase 1
**预估**: ~250 行

---

### Phase 10: SettingsList 🟠
**目标**: `/settings` 真 UI(目前 stub)

**改动**:
- 新建 `crates/nini-tui/src/components/settings_list.rs`
  - `SettingsList { items, selected_index, theme }`
  - items: `{id, label, current_value, values: Vec<String>}`
  - 视觉:cursor `→ accent`,label/value 选中态加粗 + accent
- 修改 `selectors/settings.rs` 用新组件
- 修改 `commands/dispatch.rs` 的 `/settings` 实现

**依赖**: Phase 1, Phase 9
**预估**: ~300 行

---

### Phase 11: KeyHint 格式化 🟡
**目标**: 用 `keyHint(key, desc)` 替换硬编码 footer 字符串

**改动**:
- 新建 `crates/nini-tui/src/keyhint.rs`(小工具函数)
  ```rust
  pub fn key_hint(theme: &Theme, key: &str, desc: &str) -> Vec<Span<'static>> {
      vec![
          Span::styled(key.to_string(), theme.fg_style("dim")),
          Span::raw(format!(" {desc}")),
      ]
  }
  ```
- 修改 `render.rs::render_key_hints`:
  - `Ctrl+C` → `dim` + `quit`(muted)
  - 截断时末尾加 `…`
  - 已有 F1 toggle 保留

**依赖**: Phase 1
**预估**: ~80 行

---

### Phase 12: Working indicator 上移 🟡
**目标**: spinner 浮于 editor 上方(替代当前 transcript 角)

**改动**:
- 修改 `render.rs::render_running_indicator`
  - 渲染到 editor 上方 1 行(而非 transcript 右上)
  - 颜色按 phase 切换(working=warning, compacting=accent)

**依赖**: 无
**预估**: ~30 行

---

### Phase 13: Selector overlay 风格 🟡
**目标**: 替换 selector 渲染,统一 DynamicBorder + 背景填充

**改动**:
- 修改 `crates/nini-tui/src/selector.rs::SelectorPanel`
  - 用 `DynamicBorder(theme.fg("borderMuted"))` 替换 ratatui Block
  - 整面板背景 = `bg("toolPendingBg")`(或更合适的 `bg("background")`)
  - 内置 SelectList 渲染

**依赖**: Phase 1, Phase 2, Phase 9
**预估**: ~80 行

---

## 3. 工作量汇总(诚实版)

| Phase | 行数 | 累计 |
|---|---|---|
| 1 Theme 扩展 | 150 | 150 |
| 2 DynamicBorder | 50 | 200 |
| 3 Hyperlinks | 80 | 280 |
| 4 Markdown | 400 | 680 |
| 5 Editor | 80 | 760 |
| 6 Tool box | 100 | 860 |
| 7 Msg containers | 80 | 940 |
| 8 Diff | 20 | 960 |
| 9 SelectList | 250 | 1210 |
| 10 SettingsList | 300 | 1510 |
| 11 KeyHint | 80 | 1590 |
| 12 Working indicator | 30 | 1620 |
| 13 Selector overlay | 80 | 1700 |
| **测试代码** | ~800 | **~2500** |

**实际工作量: ~1700 行新/改造 + ~800 行测试 = ~2500 行,2~3 周单人**

---

## 4. 优先级与依赖

```
Phase 1 (Theme)
   ├─► Phase 2 (DynamicBorder)
   ├─► Phase 3 (Hyperlinks) ─► Phase 4 (Markdown)
   ├─► Phase 5 (Editor)
   ├─► Phase 6 (Tool box)
   ├─► Phase 7 (Msg containers)
   ├─► Phase 8 (Diff)
   ├─► Phase 9 (SelectList) ─► Phase 10 (SettingsList)
   │                    └─► Phase 13 (Selector overlay)
   ├─► Phase 11 (KeyHint)
   └─► Phase 12 (Working indicator)
```

---

## 5. 实施顺序

### Sprint 1(本周,1 周)
- Phase 1: Theme 扩展 ← 必做,其它都依赖
- Phase 2: DynamicBorder
- Phase 3: Hyperlinks
- Phase 8: Diff(顺手做)

### Sprint 2(下周,1 周)
- Phase 4: Markdown 重构(视觉变化最大)
- Phase 6: Tool box(视觉变化大)
- Phase 7: Msg containers

### Sprint 3(第 3 周)
- Phase 5: Editor 改造
- Phase 9: SelectList(autocomplete + command palette 立刻受益)
- Phase 11: KeyHint
- Phase 12: Working indicator

### Sprint 4(可选,1 周)
- Phase 10: SettingsList(目前 stub,可延后)
- Phase 13: Selector overlay polish

---

## 6. 验证

### 6.1 视觉对比

跑两个版本同样的输入,生成快照逐行 diff:

```bash
# nini 渲染快照
cargo test -p nini-tui --test typography_snap -- --nocapture
# 输出到 /tmp/nini-snapshots-v3/

# 期望:与 /tmp/pi-snapshots/ 文本内容对齐
```

### 6.2 视觉走查清单(精简)

每条必须 pass 才算完成对应 phase:

- [ ] 暗主题 accent 色 = `#569cd6`(Pi 同款)
- [ ] H1 标题有 underline
- [ ] 代码块用 ` ``` ` 边框 + 2 空格缩进
- [ ] 引用块用 `│ ` 前缀 + italic
- [ ] 列表用 `- ` / `1. `,任务用 `[x]` / `[ ]`,4 空格缩进
- [ ] 链接 label underline + URL 括号
- [ ] HR 用 `─` × 全宽
- [ ] Tool pending 有背景色块
- [ ] Tool success 绿色背景,error 红色背景
- [ ] User message 有背景色 box
- [ ] Editor 左右各 1 列 padding
- [ ] Editor 边框色跟主题
- [ ] 多行 `❯` 退色
- [ ] Autocomplete popup 用 SelectList 风格
- [ ] Selector overlay 用 DynamicBorder
- [ ] Footer hints `dim key + muted desc`
- [ ] Spinner 浮于 editor 上方
- [ ] URL 在 iTerm2/Kitty 中可点击

### 6.3 测试覆盖目标

| 套件 | 目标 |
|---|---|
| `nini-tui --lib` | 350+ |
| `nini-tui --test tui_e2e` | 35+ |
| `nini-tui --test typography_snap` | 15+ |

---

## 7. 一句话总结(诚实版)

nini TUI 真正缺的视觉差异 ~1700 行新代码,**不是 5000 行**。关键是 13 项渐进改造,先做 Theme 扩展 + Markdown + Editor + Tool box,1~2 周内能让 nini 的视觉体验逼近 Pi。Component trait 和 VStack 那种重量级抽象对 nini v1.0 不必要 —— ratatui::Layout 已经够用。