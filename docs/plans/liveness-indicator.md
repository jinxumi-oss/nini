# 存活指示器（Liveness Indicator）—— P0 + P1 实施契约

> 日期：2026-11-20
> 来源：用户痛点「我无法分辨 nini 是在工作还是静默中断」
> 执行方式：4 个 agent 并行（state / runtime / render / wire+tests），文件不重叠，靠本文对齐
> 状态：实施中

---

## 0. 需求原文与实测取证

用户原话：**「痛点，我无法分辨，nini 是在工作还是静默中断。这个问题如何解决，给我方案」**

实测（tmux + `scripts/mock-provider.py`，非推断）：

| 场景 | 界面实际显示 | 缺口 |
|---|---|---|
| 30s bash 执行中 | 顶部 `⠋ working… • working • ctx 0% • in 52 out 24` | 无工具名、无已用时、无"上次活动" |
| 执行中 steer 一条消息 | `⠋ working… • queued (steer): 1 pending` | 相位词被瞬时消息顶掉 |
| provider 挂死 75s | 永远 `⠋ working…` | 无升级、无自愈 |
| 执行中按 Esc | 转录 `x bash Took 0ms / [aborted] user interrupted…` | ✅ 已可 |

结构性缺口（证据行）：

1. 状态行画在**屏幕顶部**（`render.rs` layout `[version(1), footer(2), …, transcript, prompt, hints]`），用户视线在底部。
2. 相位被丢弃：core 已产出 5 态 `AgentPhase`（`nini-core/src/agent.rs:104-126`），TUI 只按 `RunMode` 映射两态（`render.rs:381-390`）。`PhaseChanged` 在 TUI 里只用来置 `is_compacting`/`escape_target`（`runtime.rs:333-352`），**从不显示**。
3. `run_state.status` 是**单槽**，被 phase / `queued (steer)` / `model: xxx` / `editor: …` 轮流覆盖（`runtime.rs:5388`）。
4. TUI 全局**没有任何时间戳**（`state.rs` 只有 `pending_quit` / `clicked_at`）→ 30 秒与 30 分钟长得一样。
5. `AgentEvent::Aborted` 落到 `Ok(_) => continue`（`nini-cli/src/app.rs:897`）→ 整轮被 Esc 中止时转录无痕迹。

---

## 1. 硬约束（四个 agent 都必须遵守）

1. **只改自己负责的文件**（见 §2），不碰其他文件；不改 CHANGELOG / README / 任何文档。
2. **字段名、类型、语义以本文为准**，不要自行发明（四个 agent 并行，靠本文对齐）。
3. **不重构、不加依赖**（只用 `std`）、不新增抽象层、不做"顺手优化"。
4. 不改 provider 层（`crates/nini-ai/**`）——"流空闲超时"不在本轮范围。
5. 注释用中文，与仓库既有风格一致（`crates/nini-tui/src/` 里中英混排，新注释跟随所在文件的主流语言）。
6. 每个 agent 写完后必须跑 `cargo check -p nini-tui 2>&1 | tail -30` 确认能编译（cargo 锁会排队，慢是正常的，等即可）。
7. 单测放在**自己负责的文件内**的 `#[cfg(test)] mod tests`（沿用该文件既有位置）；只有 D 可以在 `crates/nini-tui/tests/` 建新文件。

---

## 2. 文件分工（不重叠）

| Agent | 文件 | 职责 |
|---|---|---|
| A | `crates/nini-tui/src/state.rs` | 新增 4 个状态字段 + Default/Clone + 单测 |
| B | `crates/nini-tui/src/runtime.rs` | 事件接线（写 A 的字段）+ `AgentEventLite::Aborted` |
| C | `crates/nini-tui/src/render.rs` | 渲染（读 A 的字段） |
| D | `crates/nini-cli/src/app.rs` + `crates/nini-tui/tests/liveness_indicator.rs` | Aborted 桥接 + 端到端契约测试 |

依赖：A 的字段名 → B、C 读；B 的 `AgentEventLite::Aborted` → D 的桥接。**不要跨文件改别人的活。**

---

## 3. 数据契约（A 实现，B/C 使用）

在 `crates/nini-tui/src/state.rs` 的 `RunState` 里新增（精确名与类型）：

```rust
/// 运行时相位（core 的 `AgentPhase::tag()`）："idle" / "working" /
/// "compacting" / "retrying" / "branch_summary"。
///
/// 与 `status` **分槽**：`status` 是瞬时消息（会被 "queued (steer)"、
/// "model: xxx" 轮流顶掉），`phase` 是事实，永不被瞬时消息覆盖。
pub phase: String,

/// 当前正在执行的工具：(工具名, 开始时刻)。`None` = 没有工具在跑。
pub current_tool: Option<(String, std::time::Instant)>,

/// 本轮（agent run）开始的时刻。
pub run_started_at: std::time::Instant,

/// 最后一次收到 agent 事件的时刻（"无进展"判定的基准）。
pub last_event_at: std::time::Instant,
```

* `Default`（`state.rs:1943`）：`phase: "idle".to_string()`、`current_tool: None`、
  `run_started_at: Instant::now()`、`last_event_at: Instant::now()`。
* 手写 `Clone for RunState`（`state.rs:1915`）：补齐这 4 个字段（`phase.clone()`，
  `current_tool.clone()`，两个 `Instant` 直接 Copy）。
* 语义不变式（写在字段注释里）：
  * `phase != "idle"` ⇔ 有一轮 agent 正在进行（或刚被中断、尚未收到收尾事件）。
  * `current_tool.is_some()` ⇔ 此刻有工具在执行；工具结束/整轮结束必须置 `None`。

---

## 4. 接线契约（B 实现）

### 4.1 事件接线（`crates/nini-tui/src/runtime.rs`）

| 位置 | 改动 |
|---|---|
| `AgentSink::push()` 里已持有锁的那一段 | 每次事件到达：`s.run_state.last_event_at = std::time::Instant::now();` |
| `AgentEventLite::ToolExecutionStart { id, name }`（当前 `runtime.rs:374-376` 把 name 丢成 `name: _`） | 记下名字与开始时刻：`s.run_state.current_tool = Some((name.clone(), std::time::Instant::now()));`（`executing_tools.insert` 保持不变） |
| `AgentEventLite::ToolResult { .. }` | `s.run_state.current_tool = None;` |
| `AgentEventLite::PhaseChanged(phase)` | **保留**现有 `is_compacting` / `escape_target` 逻辑不动；把 `s.run_state.status = phase;` 改为 `s.run_state.phase = phase.clone();`（相位不再占用 `status` 槽） |
| `AgentEventLite::Done` | `s.run_state.phase = "idle".to_string();` 且 `s.run_state.current_tool = None;` |
| `submit_user_input_inner`（`runtime.rs` 内 `g.run_state.status = "running..."` 那一段，约 4322 行） | 改为：`g.run_state.phase = "working".to_string();` + `g.run_state.run_started_at = Instant::now();` + `g.run_state.last_event_at = Instant::now();` + `g.run_state.current_tool = None;`，并**删除** `g.run_state.status = "running..."` 这一行（相位已经表达了它） |

> `status` 槽从现在起只装瞬时消息（"queued (steer): N pending"、"model: xxx" …）。
> `queue_while_running`（`runtime.rs:5388`）写 status 的行为**不变**。

### 4.2 `AgentEventLite::Aborted`（B 实现，D 桥接）

* 在 `AgentEventLite` 枚举里新增变体 `Aborted`。
* 在 `AgentSink::push()` 的 match 里处理它：向转录推一行可见痕迹
  （用 `s.push_notice(...)`，文案：`"[aborted] run aborted"`），
  并把 `s.run_state.phase = "idle".to_string();`、`s.run_state.current_tool = None;` 复位。
* 不加新的 `AgentEventLite` 变体以外的东西。

---

## 5. 渲染契约（C 实现）

### 5.1 顶部状态行的相位（`render_stats_line`，`render.rs:381-390`）

* 相位**改由 `state.run_state.phase` 决定**，不再由 `RunMode` 猜：

```rust
let phase = match state.run_state.phase.as_str() {
    "working" => AgentPhase::Working,
    "compacting" => AgentPhase::Compacting,
    "retrying" => AgentPhase::Retrying,
    "branch_summary" => AgentPhase::BranchSummary,
    _ => AgentPhase::Idle,
};
```

* spinner 只在 `phase != Idle` 时显示（`rich::spinner_frame()` 是基于时间的纯函数，随时可调）。
* **关键收益**：`RunMode::Editing` 但 agent 还活着（Esc 之后、agent 尚未收尾）时，
  相位仍然是 `working` —— 顶部不再谎报 idle。
* `run_state.status` 的显示段（`render.rs:396-404`）保持原样（瞬时消息照旧显示）。
* 现有单测 `footer_stats_line_shows_phase_when_running`（`render.rs:2581` 附近）
  必须改成"设置 `run_state.phase = "working"` 后断言出现 working"。

### 5.2 底部活动指示器（`render_key_hints`，`render.rs:2118`）

* **不新增布局行**：活动串渲染在**底部提示行的最左侧**（hints 之前），
  视线焦点处，且零布局改动（不改 Pi 的 footer 结构）。
* 只在 `RunMode::Running` 时显示；其它模式不显示。
* 文案（必须能被端到端测试断言到）：

| 状态 | 文案 |
|---|---|
| 有工具在跑 | `⠋ {工具名} · {n}s`（`n` = 工具已跑秒数，`now - tool_start`） |
| 无工具（在等模型） | `⠋ waiting for model · {n}s`（`n` = 本轮已跑秒数，`now - run_started_at`） |
| 无进展告警 | `⚠ no progress {n}s · {上下文}`，`n` = `now - last_event_at` 整秒 |

* 阈值常量（放在 `render.rs` 顶部，`pub(crate)` 或私有均可，但测试要能引用）：

```rust
/// 等模型时，多久没有任何事件就算"无进展"（秒）。
const NO_PROGRESS_MODEL_SECS: u64 = 60;
/// 等工具时，多久没有任何事件就算"无进展"（秒）。
/// 必须大于 bash 工具的默认超时（120s），否则正常的长任务会被误报。
const NO_PROGRESS_TOOL_SECS: u64 = 180;
```

* 告警文案里的"上下文"：等模型时是 `waiting for model`，等工具时是工具名。
* 建议把文案生成抽成**纯函数**（便于单测），签名自定，但至少要能传入 `now`：

```rust
fn activity_indicator(
    phase: &str,
    tool: Option<&(String, std::time::Instant)>,
    run_started_at: std::time::Instant,
    last_event_at: std::time::Instant,
    now: std::time::Instant,
) -> Option<(String, &'static str)>   // (文案, 颜色名)
```

  颜色名：正常用 `"warning"`，无进展用 `"error"`（走 `theme.fg_style`）。
* 渲染时把活动串放在 hints 之前，中间留两个空格；宽度不足时按现有
  `trim_to_width` 规则截断尾部（活动串在最左，不会被截掉）。

---

## 6. 桥接与测试（D 实现）

### 6.1 `crates/nini-cli/src/app.rs`

* 在事件桥接的 match 里，`Ok(AgentEvent::Aborted) =>` 分支显式 push
  `nini_tui::runtime::AgentEventLite::Aborted`（当前被 `Ok(_) => continue` 吞掉，约 897 行）。
* 只加这一个分支，别的都不动。

### 6.2 `crates/nini-tui/tests/liveness_indicator.rs`（新建）

端到端断言（用 `ratatui::backend::TestBackend` 渲染整帧后检查文本）：

1. `run_state.phase = "working"` → 帧里出现 `working`。
2. `run_state.phase = "compacting"` → 帧里出现 `compacting`。
3. `run_state.current_tool = Some(("bash", now - 12s))` 且 `mode = Running` →
   底部行同时包含 `bash` 与 `12s`。
4. `last_event_at = now - 200s` 且有工具 → 帧里包含 `no progress`。
5. `mode = Editing` 且 `phase = "working"` → 帧里**仍然**出现 `working`（不再谎报 idle）。

断言只针对**渲染出来的字符串**，不要依赖内部函数名。

---

## 7. 验收（由主 agent 串行执行）

1. `cargo build -p nini-tui` 通过；`cargo test -p nini-tui` 全绿。
2. tmux 实测：30s bash 执行中，底部行显示 `⠋ bash · 12s` 且秒数递增。
3. tmux 实测：provider 挂死 90s 后，底部行出现 `no progress`。
4. tmux 实测：Esc 之后 agent 尚未收尾时，顶部仍是 `working`（不是 idle）。
5. 全量 `cargo test`（整个 workspace）无回归。

## 8. 明确不做

* 不改 `crates/nini-ai/**`（provider 流空闲超时不在本轮）。
* 不新增布局行、不挪 Pi 的 footer 结构。
* 不加依赖、不做心跳上报。
* 不改 CHANGELOG / README / 其他文档。
