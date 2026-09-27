# Nini TUI 文字排版调研报告

> 调研目标:对照 [Pi agent](https://github.com/earendil-works/pi) 项目,评估 nini TUI 的文字排版质量,识别可借鉴的设计模式与可改进的具体缺陷。
>
> 调研产物:6 张不同尺寸 / 状态下的渲染快照,位于 `/tmp/nini-snapshots/`。

---

## 1. 现状快照

### 1.1 状态栏 (`crates/nini-tui/src/render.rs::render_status`)

```
 nini (anthropic) MiniMax-M3 | • medium | ~/nini | ⎇ main | idle | [a1b2c3d4]
```

把所有信息(应用名 / provider / 模型 / thinking / cwd / git branch / phase / diff pill / ctx% / tokens / cost / theme / session id)用 ` | ` 串联成**单行**。

在 80 列终端上会被截掉尾部:`... | ⎇ main | idle | [(no session)]` 后面的 `[a1b2c3d4]` 直接丢失(见 `02_empty_80x24.txt`)。

### 1.2 提示框 (`render_prompt`)

```
 input ─────────────────────────────────────────────────────────────────────────
❯
```

- 顶边框用硬编码 `Borders::TOP` + 静态标题 `" input "`。
- 输入行只有 `❯ ` 前缀,无左右内边距 — 文本紧贴左边缘。
- 多行模式只在第一行画 `❯`,后续行只有一个空格,没有连续视觉提示。

### 1.3 快捷键提示 (`render_key_hints`)

```
 F1 help  Enter send  Shift+Enter newline  Ctrl+L model  Ctrl+C quit
```

- 当宽度不够时直接 `break`(静默截断最后几个键位,无 `…` 提示)。
- F1 提示块用 `dim` 当背景 + 白色字 — 在深色主题上对比度还行,但 `dim` 在某些主题里会接近背景色。

### 1.4 Markdown 渲染 (`crates/nini-tui/src/markdown.rs`)

从 `04_conversation_120x40.txt` 截取:

```markdown
# Plan

1. Survey current session usage
2. Pick a JWT library (`jsonwebtoken`)
...
------- rust -------
fn verify(token: &str) -> Result<Claims, Error> {
decode::<Claims>(...)}
---------------------
```

- **标题**:6 级都用同样的 `BOLD` 修饰,无字号 / 颜色 / 字符前缀区分。
- **代码块**:用 ASCII 横线 (`------- rust -------`) 作分隔,无背景色,无 left-border accent。
- **有序列表**:直接输出 `1. text`,无缩进 — 与无序列表 `  • text` 视觉权重相同。
- **引用块**:`│ ` 前缀 + 内容都用 `dim` 色,缩进只有 1 列。

### 1.5 Thinking 块 (`render_transcript` 分支)

```
  💭  The user wants to migrate from session cookies to JWT. I should first read the current middleware, identify the ses
```

- `💭` 是双宽 emoji,但 nini 用 `chars().count()` 计算光标列位置 — 在某些终端(iTerm2,WezTerm) 中 emoji 实际宽度为 2,会导致后续文本被推到错误列。
- 没有 `Ctrl+O` 折叠提示(Pi 用 `▸ Ctrl+O to expand`,nini 有类似机制但未应用到 Thinking)。

### 1.6 工具调用 / 结果(`render_tool_call` / `render_tool_result`)

```
[tool call] read {"path": "src/middleware/auth.rs"}
[tool result] Took 23ms
  use actix_web::*;

  pub async fn auth(req: ServiceRequest) -> ...
```

- `[tool result] Took 23ms` 把状态徽章和耗时挤在第一行,Pi 在长耗时下会拆成 `[took 1.2s]` pill(用背景色块),更醒目。
- 成功 / 错误分别用 `toolPendingBg` 和 `error` 色 — 但 `toolPendingBg` 在亮主题里是 `#f0f0f0` 灰底,文字本身也是 dim,几乎读不出。

### 1.7 运行中状态

`06_running_120x30.txt`:

```
 nini (openai) MiniMax-M3 | ~/nini | ⎇ feat/tui-typography | ⠸ working… | ctx  40% [███░░░░░] | in 12.3K out 4.6K | $0.0
> summarize the auth module                                                                                          ⠸
```

- 转圈 `⠸` 同时出现在状态栏尾部**和**用户消息行尾(浮在 transcript 区右上),造成"两个 spinner"视觉。
- 单行状态栏在跑长任务时,token 计数变化也会持续抖动整行。

---

## 2. Pi 的对应设计

来源:`@earendil-works/pi-coding-agent/dist/modes/interactive/components/footer.js` + `interactive-mode.js` 第 600~700 行的 layout 装配。

### 2.1 两行 footer (Pi 风格)

```typescript
// FooterComponent.render() 返回 2~3 行
return [
  theme.fg("dim", pwd + " (branch) • sessionName"),     // Line 1
  statsLeft + padding + rightSide,                      // Line 2
  sortedStatuses.join(" "),                             // Line 3 (可选)
];
```

- **Line 1**(pwd):`~/nini (main) • my-session` — cwd + git branch + session name 同行,用 `•` 分隔。
- **Line 2**(stats):`↑12.3k ↓4.6k R9k W3k CH32.0% $0.042 (sub) 40%/200k (auto)                  (openai) MiniMax-M3 • medium`
  - 左侧:累计 token / cache hit / cost / context%
  - 右侧:provider + model + thinking level
  - 中间用空格 padding 让 model 右对齐
- **Line 3**:多扩展状态合并到一行。

### 2.2 动态边框 (`DynamicBorder`)

```typescript
class DynamicBorder {
  render(width) { return [this.color("─".repeat(Math.max(1, width)))]; }
}
```

- 用 `─` (U+2500) box-drawing 字符,**宽度自适应**。
- 默认色 `theme.fg("border", ...)`,主题可改。
- Pi 在 footer / completion popup / 设置对话框 等多处用这种自适应边框。

### 2.3 编辑器边框色 = 主题 accent

`interactive-mode.js`:

```typescript
this.themeController.onThemeChange(() => {
  this.ui.invalidate();
  this.updateEditorBorderColor();    // 主题变了就重画编辑器边框
  this.ui.requestRender();
});
```

- 编辑器底边框颜色随主题 accent 变,而不是固定白。
- 边框本身是 `─` 序列 + 右下角 `INSERT`/`NORMAL` 模式徽章(vim 模式)。

### 2.4 内联 padding

`editor.js:render()`:

```typescript
const paddingX = Math.min(this.paddingX, maxPadding);
const contentWidth = Math.max(1, width - paddingX * 2);
const leftPadding = " ".repeat(paddingX);
```

- 内置 0~N 列 padding,推荐 1。
- 用户输入和左右边框始终留有空隙,视觉不挤。

### 2.5 Heading 视觉层级 (Pi 的 Markdown component)

来自 `components/markdown.js`(在 Pi TUI 内置):

| Level | 渲染 |
|---|---|
| `# H1` | bold + accent + box-drawing 上方 / 下方分隔线 |
| `## H2` | bold + accent |
| `### H3` | bold + dim |
| `#### H4+` | bold |
| `> quote` | accent left-border + dim body |
| `\`\`\`code\`\`\`` | dim + bg-toolPendingBg + 上下分隔 |

Pi 用 accent 色 + 不同 bold / dim 组合区分标题层级,**不是**只靠粗体。

### 2.6 Tool result 的耗时 pill

```typescript
first_spans.push(Span::styled("[took 1.23s]", theme.bg("toolPendingBg")));
```

- 耗时 pill 有背景色,与成功 / 错误 label 视觉分离。
- nini 当前是 `Took 1.23s ` 直接拼到 label 后面,无视觉权重。

---

## 3. 排版问题清单(按优先级)

### P0 — 影响可读性

| ID | 问题 | 文件:行 | 推荐修复 |
|---|---|---|---|
| P0-1 | 状态栏单行宽度受限,窄终端截断 | `render.rs:131-244` | 拆成 2 行:line1 cwd+branch,line2 stats+model(对齐右侧) |
| P0-2 | 标题 h1~h6 视觉无差异 | `markdown.rs:62-72` | h1 加 accent 上/下分隔线,h2 accent bold,h3 dim bold |
| P0-3 | 代码块 ASCII 分隔无背景 | `markdown.rs:127-148` | 改用 `─` box-drawing + dim background + 顶部语言标签 |
| P0-4 | 提示框边框主题色恒定白 | `render.rs:485-492` | 改用 `theme.fg_style("accent")` 边框,与 Pi DynamicBorder 一致 |
| P0-5 | Footer key hints 静默截断 | `render.rs:535-545` | 截断时附加 `…` 或保留最后一个并加省略号 |

### P1 — 影响视觉密度

| ID | 问题 | 文件:行 | 推荐修复 |
|---|---|---|---|
| P1-1 | 提示框无 padding,文字贴边 | `render.rs:472-494` | 加 `paddingX=1`(左右各 1 列) |
| P1-2 | 多行输入只有首行 `❯` | `render.rs:475-484` | 后续行也画 `│` 或退色 `❯`,给出连续提示 |
| P1-3 | 分隔符硬编码 60 个 `─` | `rich.rs::render_divider` | 改为按终端宽度动态生成 |
| P1-4 | Running 状态双 spinner | `render.rs:render_running_indicator` | 删 transcript 区右上角的 spinner,只保留状态栏那个 |
| P1-5 | 引用块颜色与正文相同,无 vertical accent | `markdown.rs:75-83` | 左边加 `│` 用 `borderAccent` 色,内容用 `dim` |
| P1-6 | Session ID 截前 8 字符 | `render.rs:241-247` | 改为完整显示 + tooltip,或用首尾各 4 字符 |

### P2 — 微调

| ID | 问题 | 文件:行 | 推荐修复 |
|---|---|---|---|
| P2-1 | Thinking emoji `💭` 宽度不可靠 | `render.rs` ThinkingText 分支 | 用 ASCII `·` 或 `[*]` 替代,避免终端宽度歧义 |
| P2-2 | 内联代码 `magenta + DIM` 在暗主题几乎看不见 | `markdown.rs::parse_inline` | 改 `theme.fg_style("code")` + Bg("toolPendingBg"),保持 fg 亮 |
| P2-3 | `input` 标题用空格而非 `─ input ─` | `render.rs:489` | 改为 `─ input ─` 风格,与 Pi DynamicBorder 对齐 |
| P2-4 | Tool call args 在一行展开,长 JSON 不换行 | `rich.rs::render_tool_call` | 超过宽度时折行 + 缩进延续 |
| P2-5 | 状态栏分隔符统一 ` \| ` | `render.rs:142 起全部` | 段内用 `•`,段间用 ` │ `,模仿 Pi |

---

## 4. 推荐实施方案

### 阶段 1:Footer 拆双行(影响最大,改起来局部)

`crates/nini-tui/src/render.rs` 新增 `render_footer_two_lines()`,将当前 `render_status` 拆成:
- `render_pwd_line`(line 1):cwd + branch + session name
- `render_stats_line`(line 2):tokens + ctx% + cost 左对齐,model + thinking 右对齐

Layout 约束从 `Length(1)` 改成:
```
Constraint::Length(1),     // pwd line
Constraint::Length(1),     // stats line
...
```

### 阶段 2:Markdown 视觉层级

修改 `crates/nini-tui/src/markdown.rs`,引入 accent / borderAccent / dim 三档:

| Element | 现有 | 推荐 |
|---|---|---|
| H1 | bold | accent fg + bold + 上下 `─` 分隔线 |
| H2 | bold | accent fg + bold |
| H3-H6 | bold | dim fg + bold |
| Quote | dim 内容 | borderAccent 左边条 + dim 内容 |
| Fenced | ASCII `----` | box-drawing `─` + bg-toolPendingBg + 语言标签左对齐 |

### 阶段 3:动态边框组件

新建 `crates/nini-tui/src/dynamic_border.rs`:

```rust
pub struct DynamicBorder<'a> { pub color: &'a str }

impl<'a> DynamicBorder<'a> {
    pub fn render(&self, width: usize, theme: &Theme) -> Vec<Span<'static>> {
        vec![Span::styled("─".repeat(width.max(1)), theme.fg_style(self.color))]
    }
}
```

替换:
- 提示框 `input` 顶/底边框
- 分隔符 60 → 动态宽度
- 状态栏 / footer 之间空行

### 阶段 4:输入框 padding + 视觉权重

修改 `render_prompt`:
- `paddingX = 1`(左右各 1 空列)
- 多行输入后续行画退色 `❯`(dim success),首行 bold success
- 边框色 = `theme.fg_style("accent")`(随主题变)

### 阶段 5:Key hints 滚动截断

`render_key_hints` 中:
```rust
// 当前:
if current_width + extra > area.width as usize { break; }
// 推荐:
if current_width + extra + 1 > area.width as usize {
    spans.push(Span::styled(" …", theme.fg_style("dim")));
    break;
}
```

---

## 5. 验证方法

`/tmp/nini-snapshots/` 已存 6 张快照作为基线。改进后用同样的 `TestBackend` 渲染对比:

```bash
# 重现基线
cargo test -p nini-tui --test tui_e2e -- --nocapture

# 新增 typography snapshot test(模板)
# crates/nini-tui/tests/typography_snap.rs
```

视觉走查清单(对照快照):
- [ ] 80 列终端状态栏**不**被截断
- [ ] h1 标题有 box-drawing 分隔
- [ ] 代码块有背景色
- [ ] 提示框边框色与 accent 一致
- [ ] 多行输入有连续视觉提示
- [ ] Footer 截断有 `…` 提示

---

## 6. 一句话总结

nini 当前 TUI 排版**功能正确但视觉粗糙**:单行 status bar 在窄屏截断、Markdown 标题无层级、提示框边框静态。借鉴 Pi 的 2 行 footer、accent-aware 动态边框、heading 视觉层级和 padding 设计,可在 ~250 行 Rust 改动内把 nini 的 TUI 排版拉到与 Pi 同等可读性水平。