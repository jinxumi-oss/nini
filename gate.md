# 鼠标复制粘贴体验改造 —— 交付门禁检查

任务：当前计划见 `plan.md`（nini TUI 鼠标复制粘贴体验改造 —— 阶段 1–3 收口）

## BLOCKER 检查清单

### 1. 计划完整性 ✅
- `plan.md` 存在：现状取证（行号级）/ 坐标系契约 / 9 项改动（R1–R9）/ 测试 / 验收 / Out of scope / 分工。
- 需求可追溯：每一项改动都能指回「原会话的 4 阶段计划」或「Pi 参考实现的具体行号」。

### 2. 代码定位 ✅
- 阶段 1 半成品：`crates/nini-tui/src/selection.rs`（390 行，13 条单测全绿）、
  `state.rs` 的 `screen_text_rows` / `selection` / `selection_granularity` / `selection_dragged`。
- 两端缺口：`render.rs` 无 `screen_text_rows` 发布、`runtime.rs` 无 selection 引用（grep 零命中）。
- Pi 口径：`tui-alt-screen.ts:213`（粒度）、`:81-90`（词 joiner）、`:1220`（getClickCount）、
  `:1156`/`:1193`（词/行范围）、`:1249`/`:1274`（边缘自动滚动，50ms）、`:272`（copyOnSelect 默认 true）。

### 3. 证据充分性 ✅
- `selection.rs` 的 5 个核心函数生产引用 0 处 → 特性对用户不可见（grep 实证）。
- 复制/粘贴链路存在且可用：`clipboard.rs::copy()`（原生，可验证）、`clipboard.rs::read()`
  （区分「无文本」与「不可达」），阶段 0 已接进 `Ctrl+V`。

### 4. 风险控制 ✅
- 三个 agent 的文件不重叠（render.rs / runtime.rs+state.rs / settings.rs）；
  唯一跨 agent 的是 C → B 的一个函数签名，已在计划里写死。
- 不新增依赖（base64 自己实现，带单测）。
- 剪贴板在 headless CI 不可达 → 计划里给了 thread_local 覆盖缝，保证阶段 2/3 可测。
- 选区是屏幕行坐标 → 用户滚动即清选区（写进计划，避免「高亮跳行」的假象）。

### 5. 与并发会话的关系 ✅
- 用户自己的另一个 nini 会话（`hermes-agent`）此前已在本仓库留下未提交改动
  （`selection.rs` / `lib.rs` / `state.rs` 的选区字段 / `runtime.rs`+`clipboard.rs` 的阶段 0）。
  本次是在**同一份半成品上**继续收口，不重做、不丢弃它的成果。
- 阶段 0 的成果（`Ctrl+V` 贴文本）保留原样，不重写。

## VERDICT: APPROVED

可以进入执行阶段。worker 按 `plan.md` §7 的分工执行，产物写到
`docs/plans/mouse-copy-paste-changes-{a,b,c}.md`。
