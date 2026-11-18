# Changes

范围：nini TUI「鼠标复制粘贴体验」改造 —— **阶段 1 渲染侧**（R1 + R2）。
分工：worker A 起草（中途被限流打断），主 agent 收口并修完其编译/逻辑问题。

## 改了什么

- `crates/nini-tui/src/render.rs:1257` | `items_for_line` 改返回 `Vec<RenderedRow>`
  （`RenderedRow { line, text }`）而不是 `Vec<ListItem>`：ratatui 0.29 的
  `ListItem.content` 是 `pub(crate)`，**从 widget 上读不回纯文本**，所以
  「屏幕上这一行是什么」必须跟着一行走 | 步骤 R1 | 变更
- `crates/nini-tui/src/render.rs:1132` | `render_transcript` 在选好可见切片之后：
  ① 给每个可见行套用选区高亮（`apply_selection_row_highlight`），
  ② 把每行纯文本写进 `state.screen_text_rows`（`Arc<StdMutex<Vec<String>>>`，
  与 `line_heights` 同一发布姿势：clone 共享同一个 Arc），
  ③ 再包成 `ListItem` 交给 `List` | 步骤 R1+R2 | 变更
- `crates/nini-tui/src/render.rs:1355` | `highlight_char_interval` 按**字符下标**
  切 span，切之前把字符下标换算成字节偏移（CJK 一个字 3 字节 2 列，
  混用会切在字符中间并 panic） | 步骤 R2 | 变更
- `crates/nini-tui/src/render.rs:1059` | 欢迎卡早退分支先清空 `screen_text_rows`：
  欢迎卡不是屏幕行，上一帧的行留着会让鼠标选中已经不存在的文字 | 步骤 R2 | 变更
- `crates/nini-tui/src/render.rs:2810` | 新增 `screen_row_publish_tests` 模块（4 条）：
  发布的行 == 渲染器产出的行、空转录发布空 Vec、高亮只落在区间内、
  CJK 行的高亮列换算 | 步骤 R4 测试 | 变更

## 与 worker A 草稿的差异（主 agent 收口时修的）

1. **A 的草稿从 `ListItem.content` 取纯文本** → ratatui 0.29 该字段私有，编译不过；
   改成「纯文本随行一起走」（`RenderedRow`）。
2. **A 的草稿把行号当成绝对屏幕行**（`content_top + i`）→ 契约是
   `screen_text_rows` 的下标（= `screen_y - content_area.y`），改成 `i`。
3. **A 的草稿在高亮里按字节切字符串** → 改成字符下标 → 字节偏移换算。
4. **A 的草稿测试期望「一条消息一行」** → 实际渲染是「文本行 + 空行分隔」，
   改成与渲染器实际产出的行比对（断言源从硬编码改为 `items_for_line` 的输出）。

## Commands
- `cargo fmt --all -- --check` → 退出码 0
- `cargo clippy -p nini-tui --all-targets -- -D warnings` → 退出码 0
- `cargo test -p nini-tui --lib screen_row_publish` → 退出码 0（4 passed）

## Reproduction
- 复现命令: `cargo test -p nini-tui --lib screen_row_publish` → 退出码 0
  （改动前：`screen_text_rows` 永远为空 —— grep `screen_text_rows` 在 render.rs
  里零命中，选区拿不到任何输入）

## Blocked
none
