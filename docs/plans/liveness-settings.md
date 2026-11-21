# 存活指示器 第三期：时间阈值可配

> 日期：2026-11-20
> 来源：第一、二期落地后的遗留（三个阈值硬编码）
> 执行方式：4 个 agent 并行（nini-core / nini-tui / nini-ai / nini-cli+README），文件不重叠，靠本文对齐
> 状态：实施中

---

## 0. 目标与边界

**目标**：把三个硬编码的阈值变成 `settings.json` 可配：

| 阈值 | 现值 | 位置 | 作用 |
|---|---|---|---|
| provider 流空闲超时 | 120s | `nini-ai/src/stream_idle.rs` | 两个 chunk 之间、或等响应头，超过这么久没数据 → 判定上游挂死 |
| 等模型无进展 | 60s | `nini-tui/src/render.rs` | 界面升级为 `⚠ no progress` |
| 等工具无进展 | 180s | `nini-tui/src/render.rs` | 同上（必须 > provider 空闲超时） |

**边界（明确不做）**：

* 不新增斜杠命令、不做设置 UI（用户手改 `settings.json`）。
* 不改 provider **总超时**（`error_body.rs:156-163` 的既有结论）。
* 不给这些键做 Pi 兼容别名之外的任何映射；不加依赖行。
* 不改 `crates/nini-tui/src/settings.rs`（TUI 自己的 Settings 结构）—— 见 §2 的"唯一解析点"。

---

## 1. 架构：一份配置、一处解析、进程级生效

```
~/.nini/agent/settings.json
        │  "liveness": { "provider_stream_idle_secs": 60, … }
        ▼
nini_core::settings::load_settings(cwd)          ← 唯一解析点（nini-core）
        │  Settings.liveness: Option<LivenessSettings>
        ▼
nini_core::liveness::set_liveness_settings(...)   ← 进程级生效（nini-core）
        │
        ├──► nini_ai::stream_idle::idle_timeout()          （provider 空闲超时）
        └──► nini-tui: AppState 启动时拷进 RunState        （两个 no_progress 阈值）
                     └──► render::activity_indicator(…, 阈值)
```

**为什么 provider 侧要走"进程级"**：nini-ai 的 `Provider` trait 只有 `stream(req)`，
没有 settings 句柄；要把它塞进 provider 构造参数就得改 `provider_factory.rs`
的构造链（那里有别人未提交的工作）。进程级配置绕开构造链，且**只有一处**可写。

**为什么 TUI 侧要拷进 RunState 而不是直接读全局**：`activity_indicator` 是纯函数，
单测直接调用它。全局状态在 `cargo test` 的并行线程间会互相污染 → 拷进
`RunState` 一次，渲染层读不可变快照，纯函数保持纯。

**唯一解析点为什么是 nini-core 而不是 nini-tui**：`nini_tui::settings::Settings`
不认识 `liveness` 段（serde 忽略未知字段 ✅），而 `nini_core::settings::Settings`
本来就带 camelCase 别名机制（`maxIterations` 那一套）。两套 Settings 各解析一次
同一份文件、可能得出不同值 → 只留 nini-core 一处。

---

## 2. 数据契约（L1 实现，L2/L3/L4 使用）

### 2.1 新文件 `crates/nini-core/src/liveness.rs`

```rust
//! 存活指示器的时间阈值与进程级生效值。
//!
//! 为什么是"进程级"：provider 的流空闲超时在 nini-ai 层（那里没有 settings
//! 句柄），TUI 的"无进展"阈值在渲染层。两者要读同一份用户配置，又都不能
//! 各自去读盘 —— 于是配置在进程启动时解析一次，之后各处从同一处读。

use std::sync::{LazyLock, RwLock};

/// 存活指示器的三个时间阈值（秒）。
///
/// 键名风格与 nini-core 其它设置一致：snake_case 为主，camelCase 作为别名。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct LivenessSettings {
    /// provider 流的空闲超时：两个 chunk 之间、或等待响应头，超过这么久
    /// 没有数据就判定上游挂死。
    #[serde(alias = "providerStreamIdleSecs")]
    pub provider_stream_idle_secs: u64,
    /// 等模型时，多久没有任何事件就算"无进展"。
    #[serde(alias = "noProgressModelSecs")]
    pub no_progress_model_secs: u64,
    /// 等工具时，多久没有任何事件就算"无进展"。
    ///
    /// 必须大于 [`LivenessSettings::provider_stream_idle_secs`]，
    /// 否则正常的长工具会被误报成挂死。
    #[serde(alias = "noProgressToolSecs")]
    pub no_progress_tool_secs: u64,
}

impl Default for LivenessSettings {
    fn default() -> Self {
        Self {
            provider_stream_idle_secs: 120,
            no_progress_model_secs: 60,
            no_progress_tool_secs: 180,
        }
    }
}

/// 进程级生效值。`LazyLock` + `RwLock`：写一次、读无数次（每帧一次）。
static SETTINGS: LazyLock<RwLock<LivenessSettings>> =
    LazyLock::new(|| RwLock::new(LivenessSettings::default()));

/// 设置本次进程生效的存活阈值（进程启动时调用一次）。
pub fn set_liveness_settings(settings: LivenessSettings) {
    *SETTINGS.write().unwrap_or_else(|e| e.into_inner()) = settings;
}

/// 读取本次进程生效的存活阈值。
pub fn liveness_settings() -> LivenessSettings {
    *SETTINGS.read().unwrap_or_else(|e| e.into_inner())
}
```

* `crates/nini-core/src/lib.rs` 加 `pub mod liveness;`（按字母序放在合适位置）。
* **单测**（该文件内）：默认值是 120/60/180；`set` 之后 `liveness_settings()` 读到新值；
  读一个副本不会拿到同一份可变引用（`Copy`）。
  ⚠️ 全局状态 + 并行测试：本文件里的单测若改全局，**必须**在同一测试内改回默认值，
  并且不要断言"别人没改过"（只断言自己 set 的值能被读回）。

### 2.2 `crates/nini-core/src/settings.rs`

```rust
/// 存活指示器的时间阈值（秒）。缺失 → 用 `LivenessSettings::default()`。
#[serde(default)]
pub liveness: Option<crate::liveness::LivenessSettings>,
```

并在 `impl Settings` 里加：

```rust
/// 存活阈值：文件里没有这一段就用默认值。
pub fn liveness_settings(&self) -> crate::liveness::LivenessSettings {
    self.liveness.unwrap_or_default()
}
```

* **单测**：`{"liveness": {"provider_stream_idle_secs": 30}}` 只覆盖一个键，其余取默认；
  缺整段 → 全默认；camelCase 别名（`providerStreamIdleSecs`）也能认。

---

## 3. L2 契约：nini-tui

### 3.1 `crates/nini-tui/src/state.rs`

`RunState` 加两个字段（精确名与类型）：

```rust
/// 等模型时，多久没有任何事件就算"无进展"（秒）。来自进程的存活阈值。
pub no_progress_model_secs: u64,
/// 等工具时，多久没有任何事件就算"无进展"（秒）。来自进程的存活阈值。
pub no_progress_tool_secs: u64,
```

* `Default`：`nini_core::liveness::LivenessSettings::default()` 的两个值（**不要另写数字**）。
* 手写 `Clone for RunState`：补齐这两个字段（`Copy`）。
* `AppState::with_settings(...)`（`state.rs` 里给 `settings_snapshot` 赋值的那处）：
  构造完 `RunState` 后，把**进程全局**的值拷进来：
  ```rust
  let lv = nini_core::liveness::liveness_settings();
  s.run_state.no_progress_model_secs = lv.no_progress_model_secs;
  s.run_state.no_progress_tool_secs = lv.no_progress_tool_secs;
  ```
  （`with_settings` 拿到的 `crate::settings::Settings` 是 **nini-tui 自己那套**，
  它没有 liveness 段 —— 所以这里从**全局**取，不要从 `settings` 取。）

### 3.2 `crates/nini-tui/src/render.rs`

* `activity_indicator` 的**签名**改为多两个阈值参数（纯函数，不读全局）：

```rust
pub fn activity_indicator(
    phase: &str,
    tool: Option<&(String, std::time::Instant)>,
    run_started_at: std::time::Instant,
    last_event_at: std::time::Instant,
    now: std::time::Instant,
    no_progress_model_secs: u64,
    no_progress_tool_secs: u64,
) -> Option<(String, &'static str)>
```

* 删掉文件顶部的 `NO_PROGRESS_MODEL_SECS` / `NO_PROGRESS_TOOL_SECS` 两个常量
  （阈值改由调用方传入）。
* `render_key_hints` 的调用处传
  `state.run_state.no_progress_model_secs, state.run_state.no_progress_tool_secs`。
* **更新既有单测**：`activity_indicator` 的调用点（该文件内的测试模块）补两个参数；
  断言语义不变（60 / 180 这两个值在测试里显式传）。
* **新增单测**：传入 5 秒的阈值 → 5 秒无事件即出现 `no progress`（证明阈值真的生效，
  而不是写死的 60/180）。

### 3.3 `crates/nini-tui/src/runtime.rs`

* `/reload`（重读 settings 的那条路径）之后，把全局的阈值**重新拷进** `RunState`
  （否则用户改了 settings.json 再 /reload，界面阈值不变）。
  实现上：在该路径拿到新设置之后，加两行
  `g.run_state.no_progress_model_secs = …; g.run_state.no_progress_tool_secs = …;`
  （值从 `nini_core::liveness::liveness_settings()` 取）。

---

## 4. L3 契约：nini-ai

* `crates/nini-ai/src/stream_idle.rs`：
  * 保留 `DEFAULT_IDLE_TIMEOUT`（它就是默认值 120s 的来源，注释里写明它来自
    `LivenessSettings::default()`）；
  * 新增：

```rust
/// 当前进程生效的 provider 流空闲超时。
///
/// 读的是进程级存活阈值（启动时由 nini-cli 从 settings.json 解析后写入）。
/// 注意：**已在飞行中的请求不受影响** —— 超时在调用 `next_chunk` 时就已经定下，
/// 改配置对下一次请求生效。
pub fn idle_timeout() -> Duration {
    Duration::from_secs(
        nini_core::liveness::liveness_settings().provider_stream_idle_secs,
    )
}
```

  * 单测：set 一个非默认的全局值 → `idle_timeout()` 读到它；**测完把全局改回默认**
    （否则会污染同 crate 的其它测试）。
* `crates/nini-ai/src/openai.rs`、`anthropic.rs`、`openai_responses.rs`：
  三处 `crate::stream_idle::DEFAULT_IDLE_TIMEOUT` 换成 `crate::stream_idle::idle_timeout()`。

---

## 5. L4 契约：nini-cli 与文档

### 5.1 `crates/nini-cli/src/main.rs`

在 `main()` 里（`Args::parse()` 之后、任何 provider 构造之前）加：

```rust
// 存活指示器的阈值：进程启动时解析一次，之后 provider 层与 TUI 层
// 都从 nini_core::liveness 这同一处读。
{
    let cwd = std::env::current_dir().unwrap_or_default();
    let settings = nini_core::settings::load_settings(&cwd);
    nini_core::liveness::set_liveness_settings(settings.liveness_settings());
}
```

### 5.2 README（中文 + 英文两份，键名与说明必须一致）

在讲 `settings.json` 的 `retry` 段的那段之后，加一小段：

```
`settings.json` 的 `liveness` 段控制存活指示器的时间阈值：
`provider_stream_idle_secs`（默认 120，provider 流多久没数据算挂死）、
`no_progress_model_secs`（默认 60，等模型时多久没动静算"无进展"）、
`no_progress_tool_secs`（默认 180，等工具时的同一阈值；必须大于第一个）。
```

英文版同义。**不要**改 README 的其它段落。

---

## 6. 验收（主 agent 串行执行）

1. `cargo build --workspace` / `clippy --workspace --all-targets -D warnings` / `fmt --check` /
   `cargo test --workspace` 全绿。
2. 端到端：写一份 `settings.json`（`liveness.provider_stream_idle_secs = 5`）→
   provider 挂死 **5 秒**后界面就报错（不再是 120s）。
3. 端到端：`no_progress_model_secs = 5` → 挂死 5 秒后底部出现 `⚠ no progress`。
4. 默认值回归：不写 `liveness` 段 → 行为与第二期一致（120/60/180）。

## 7. 硬约束（四个 agent 都必须遵守）

1. **只改自己负责的文件**（见 §2–§5 的分工）；不改 CHANGELOG；README 只按 §5.2 加那一小段。
2. **API 严格照本文**：字段名、函数名、签名一字不改 —— 另有 agent 并行依赖。
3. **不加依赖行**（不改任何 `Cargo.toml`）；不重构；注释用中文。
4. 写完跑 `cargo check -p <crate> 2>&1 | tail -20`；cargo 锁会排队，等即可。
5. 依赖未落盘时报错（`liveness` 模块不存在等）→ **耐心等待并重试**（每 60s 一次，
   最多等 10 分钟），不要自己实现别人的活、不要改名绕过。

---

## 8. 实施记录（实测与勘误）

### 8.1 生产启动路径不经过 `with_settings`

契约 §3.1 只点名 `AppState::with_settings`，但**生产路径**是
`runtime::initial_state` → `AppState::new` → `AppState::default()`，根本不走
`with_settings`。只在 `with_settings` 里拷全局，用户配的阈值永远进不了 TUI。
已改为 `AppState::new` 委托 `with_settings`（两者对 `settings_snapshot` 与
`context_window` 的赋值等价，全量测试无回归）。

### 8.2 `/reload` 的 settings 解析器不认 `liveness` 段

`cmd_reload`（`crates/nini-tui/src/commands/dispatch.rs`）用的是 nini-tui 自己的
`SettingsManager::load_from_disk`，它**只解析自己那套键**，`liveness` 段会被
serde 静默忽略。所以 `/reload` 要生效，必须：

1. 用 `nini_core::settings::load_settings(cwd)` 重解析（复用 `cmd_reload` 里已有的
   `cwd`，不引入第二个 cwd 来源）→ `set_liveness_settings(lv)`；
2. 把 `lv` 的两个 no_progress 阈值拷进 `state.run_state`。

顺带：`/reload` 之后 **provider 的流空闲超时也随之更新**（同一份配置、一处解析、
一个进程全局）。

### 8.3 `load_settings` 的项目级覆盖

`nini_core::settings::load_settings` 对**每个**可覆盖字段都做项目级覆盖
（`.nini/settings.json` 覆盖用户级）。`liveness` 段若不补这一行，项目级配置会被
静默丢弃 —— 已补。

### 8.4 实测：两个超时的交互

配置 `provider_stream_idle_secs = 5` + `no_progress_model_secs = 5` 时，
**provider 的空闲超时先触发**：5 秒时转录出现
`[error] retrying after transient error (attempt 1, delay 2000ms)`，
顶部相位变成 `retrying…`，底部活动串变成 `⠋ retrying · 15s`。

此时 `⚠ no progress` **不出现** —— 因为 `no_progress` 的计时基准是
`last_event_at`，而那条 Error 事件刚刚把它刷新了。这是合理的：既然 provider
已经明确说了"挂死并在重试"，"多久没动静"这条告警就是多余的。
把 `no_progress_model_secs` 设得比 `provider_stream_idle_secs` 小，
告警就会先出现。

### 8.5 实测：默认值回归

不写 `liveness` 段时，T+12s 底部是 `⠋ waiting for model · 11s`（无告警），
与第二期行为一致（60s 才报警、120s 才判挂死）。
