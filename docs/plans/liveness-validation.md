# 存活阈值 第四期：校验、首响应超时分离、可见性

> 日期：2026-11-20
> 来源：第三期落地后的遗留（阈值可配了，但配错没人知道；两类超时共用一个值；当前生效值无处可查）
> 执行方式：4 个 agent 并行（nini-core / nini-ai / nini-tui / nini-cli+README），文件不重叠
> 状态：实施中

---

## 0. 本轮范围

| # | 事项 | 决定 |
|---|---|---|
| A | **阈值校验**：配错（例如把"等工具无进展"设得比 bash 的 120s 超时还短）现在完全静默 | ✅ 做：`validate()` + 启动时可见 |
| B | **首响应超时与流空闲分开**：现在"等响应头"与"chunk 之间"共用一个 idle 值，但二者性质不同（响应头应该很快到；chunk 间隔可以长） | ✅ 做：新增 `provider_response_secs` |
| C | **当前生效的阈值无处可查** | ✅ 做：`/status` 里展示（只读快照） |
| D | `/settings` 界面里交互式编辑阈值 | ❌ **不做**：`crates/nini-tui/src/selectors/settings.rs` 的模块 doc 自承"picking a category does nothing" —— `/settings` 整个是惰性的，接交互等于把一个更大的坑拖进来。改为在 `/status` 里只读展示 + 指向 settings.json |
| E | 配置文件改动即时生效（文件监视） | ❌ **不做**：`/reload` 已提供手动路径；文件监视要引入后台线程、debounce 与跨平台差异，与"极短路径"冲突 |

---

## 1. 硬约束（四个 agent 都必须遵守）

1. **只改自己负责的文件**（见 §3–§6）；不改 CHANGELOG；README 只按 §6.2 加那一小段。
2. **API 严格照本文**：字段名、函数名、签名一字不改 —— 另有 agent 并行依赖。
3. **不加依赖行**（不改任何 `Cargo.toml`）；不重构；注释用中文。
4. 写完跑 `cargo check -p <crate> 2>&1 | tail -20`；cargo 锁会排队，等即可。
5. 依赖未落盘时报错 → **耐心等待并重试**（每 60s 一次，最多等 10 分钟），不要自己实现别人的活、不要改名绕过。

---

## 2. 数据契约（M1 实现，M2/M3/M4 使用）

### 2.1 `crates/nini-core/src/liveness.rs`

`LivenessSettings` 增加一个字段（第四个阈值）：

```rust
/// 等**响应头**的上限（秒）：请求已发出，服务器却迟迟不回响应头。
///
/// 与 [`LivenessSettings::provider_stream_idle_secs`] 分开：响应头应该在
/// 很快内到达（服务器收到了请求就回），而流里的 chunk 间隔可以长得多
/// （模型在思考）。两者共用一个值，会让"首响应慢"的场景要么等太久、
/// 要么误杀健康的长流。
#[serde(alias = "providerResponseSecs")]
pub provider_response_secs: u64,
```

* `Default`：`provider_response_secs: 60`（其余不变：120 / 60 / 180）。
* `nini_core::settings::Settings.liveness` 的类型不变（还是 `Option<LivenessSettings>`），
  新字段自动跟着解析；**旧配置文件没有这个键 → 走 `#[serde(default)]` 的默认值** ✅。

### 2.2 校验函数

```rust
/// 校验阈值配置，返回**人类可读的警告**（空 = 没问题）。
///
/// 只报**真的会咬人**的情况，不做风格建议：
///  * 阈值为 0 → 立即触发（等响应头 0 秒 = 每个请求都判挂死）；
///    这种值**不能生效**，调用方应当回退到默认值。
///  * 等工具的无进展阈值短于 bash 的默认超时（120s）→ 正常的 cargo test /
///    build 会被误报成"卡住了"。
pub fn validate(settings: &LivenessSettings) -> Vec<String>
```

* 返回的每条是一个完整句子（中文），可以直接显示给用户。
* `0` 的处理：**在校验函数里指出来**，由调用方决定回退 —— 但本仓库的做法是
  **不生效非法值**：M1 另提供一个
  ```rust
  /// 把非法值（0）换成默认值，其余原样保留。
  pub fn sanitized(settings: LivenessSettings) -> LivenessSettings
  ```
  nini-cli 用 `sanitized` 写进进程全局（所以 0 永远不会生效），
  `validate` 的输出负责告诉用户"你写的这个值被换掉了"。

### 2.3 进程级生效值与警告

```rust
/// 设置本次进程生效的存活阈值（进程启动时调用一次）。
pub fn set_liveness_settings(settings: LivenessSettings);

/// 读取本次进程生效的存活阈值。
pub fn liveness_settings() -> LivenessSettings;

/// 本次进程的阈值**校验警告**（`set_liveness_settings` 时算好存下）。
/// 空 = 没有问题。
pub fn liveness_warnings() -> Vec<String>;
```

* `set_liveness_settings` 内部：先把传入值过一遍 `sanitized`（0 → 默认），
  再把 `validate` 的结果存进全局。
* `liveness_warnings()` 返回**存下来的**那一份（不是每次重算）——因为
  `sanitized` 已经替换过 0，重算会丢掉"你写的 0 被换掉了"这条信息。

---

## 3. M1 契约：nini-core

**文件**：`crates/nini-core/src/liveness.rs`（改）、`crates/nini-core/src/settings.rs`（改）

1. 按 §2.1 加 `provider_response_secs` 字段 + `Default`（60）。
2. 按 §2.2 加 `validate()` 与 `sanitized()`。
3. 按 §2.3 改 `set_liveness_settings`（存 sanitized 值 + 存 validate 的警告），
   加 `liveness_warnings()`。
4. `crates/nini-core/src/settings.rs`：**不用改结构**（`Option<LivenessSettings>` 自动
   带上新字段）；补一条单测：旧配置文件（只有三个键）→ 新字段取默认 60。
5. **单测**（在 `liveness.rs` 内）：
   * 默认值是 120 / 60 / 180 / **60**（顺序：`provider_stream_idle_secs`、
     `no_progress_model_secs`、`no_progress_tool_secs`、`provider_response_secs`）；
   * `validate` 对 0 值给出警告；对 `no_progress_tool_secs = 60`（< 120）给出
     "会被误报"的警告；对全默认返回空；
   * `sanitized` 把 0 换成默认、其余原样保留；
   * `set_liveness_settings` 之后 `liveness_warnings()` 读到刚才的警告，
     且 `liveness_settings()` 里的 0 已经被换成默认。
   ⚠️ 全局状态：改完**必须改回默认**，不要断言"别人没改过全局"。

---

## 4. M2 契约：nini-ai

**文件**：`crates/nini-ai/src/stream_idle.rs`（改）、三个 provider（改）

1. `stream_idle.rs`：
   * 现有 `idle_timeout()` → **流 chunk 之间**的空闲超时（读
     `provider_stream_idle_secs`），**语义不变**；
   * 新增：
     ```rust
     /// 当前进程生效的**等响应头**超时（`send()` 那一步）。
     ///
     /// 与 [`idle_timeout`] 分开：响应头应该在很快内到达，而流里的 chunk
     /// 间隔可以长得多（模型在思考）。两者共用一个值，会让"首响应慢"的
     /// 场景要么等太久、要么误杀健康的长流。
     pub fn response_timeout() -> Duration {
         Duration::from_secs(
             nini_core::liveness::liveness_settings().provider_response_secs,
         )
     }
     ```
   * 单测：set 全局 → `idle_timeout()` 与 `response_timeout()` 各读到自己的值；
     **测完把全局改回默认**。
2. 三个 provider（`openai.rs` / `anthropic.rs` / `openai_responses.rs`）：
   * `send_with_idle_timeout(...)` 那处的第二参数从 `crate::stream_idle::idle_timeout()`
     改成 `crate::stream_idle::response_timeout()`（**等响应头**）；
   * `next_chunk(...)` 那处**保持** `idle_timeout()`（chunk 之间）；
   * 其余一行不动。

---

## 5. M3 契约：nini-tui

**文件**：`crates/nini-tui/src/state.rs`（改）、`crates/nini-tui/src/commands/dispatch.rs`（改）

### 5.1 启动时把校验警告推进 transcript

`AppState::with_settings`（state.rs，给 `settings_snapshot` 赋值的那处）里，在拷阈值
之后，把**进程全局**的校验警告推进转录（每条一行）：

```rust
// 阈值配错了要**当场**说清楚，而不是等它咬人。
for warning in nini_core::liveness::liveness_warnings() {
    s.push_notice(warning);
}
```

（`push_notice` 是该文件既有的方法 ✅。）

### 5.2 `/status` 展示当前生效的阈值

`/status` 是**快照**语义（`crates/nini-tui/src/commands/dispatch.rs` 的 `cmd_status`
→ `build_status_lines(state)`）。在它的输出里加一段：

```
存活阈值:
  provider 响应头超时: 60s
  provider 流空闲超时: 120s
  等模型无进展: 60s
  等工具无进展: 180s
```

* 值从 `nini_core::liveness::liveness_settings()` 读（**进程当前生效值**）——
  `/status` 展示的是"现在真的在用多少"，不是"文件里写了多少"。
* 若 `nini_core::liveness::liveness_warnings()` 非空，在这段后面再列一行
  `⚠ <警告>`（每条一行）。
* 单测：构造 state → 渲染 `/status` → 断言输出里有 "存活阈值" 与四个键的秒数。
  ⚠️ `/status` 的既有输出格式别动，只**追加**这一段。

---

## 6. M4 契约：nini-cli 与文档

### 6.1 `crates/nini-cli/src/main.rs`

第三期加的那段（解析 settings → `set_liveness_settings`）之后，加：

```rust
// 阈值配错了当场说清楚：CLI 模式下走 stderr，TUI 模式由 nini-tui
// 把同一份警告推进转录（见 nini-tui 的 AppState::with_settings）。
for warning in nini_core::liveness::liveness_warnings() {
    eprintln!("nini: {warning}");
}
```

### 6.2 README（中文 + 英文两份）

`liveness` 段那一小段里**补上第四个键** `provider_response_secs`（默认 60：
等响应头的上限），并补一句校验规则：

> `no_progress_tool_secs` 必须不小于 120（bash 的默认超时），否则正常的
> `cargo test` 会被误报成"卡住了"；写 0 会被换回默认值。

（英文版同义。键名、默认值、两份措辞必须一致。）

---

## 7. 验收（主 agent 串行执行）

1. `cargo build --workspace` / `clippy --workspace --all-targets -D warnings` /
   `fmt --check` / `cargo test --workspace` 全绿。
2. 端到端 A：settings.json 里 `liveness.no_progress_tool_secs = 30`（< 120）→
   启动后**转录里立刻出现**警告行。
3. 端到端 B：settings.json 里 `liveness.provider_response_secs = 5` →
   黑洞 provider **5 秒**后报错（而不是 60/120 秒）。
4. 端到端 C：`/status` 显示四个阈值与当前生效值。

---

## 8. 实施记录

### 8.1 集成修复：`no_terminal_writes` 的白名单计数

nini-cli 新增的 `eprintln!("nini: {warning}")` 让
`crates/nini-tui/tests/no_terminal_writes.rs` 里 `crates/nini-cli/src/main.rs`
的 stderr 计数从 3 变成 4 —— 那个测试的规矩是「白名单里的计数必须与实际
写入次数一致」，所以**登记**新的一处（而不是删掉 eprintln：CLI 模式没有
转录，警告只能走 stderr）。理由已写进白名单条目。

### 8.2 TUI 模式下警告出现两次（不同通道，不同可见性）

* stderr：在**进入 alt screen 之前**打印 → TUI 运行期间用户看不到（退出后滚屏可见）；
* 转录：`AppState::with_settings` 推进 transcript → 用户**当场**能看到。

不是同一时刻的重复提示，故不去重。

### 8.3 实测（tmux + 黑洞 provider）

配置 `provider_response_secs = 5` + `no_progress_tool_secs = 30`：

* 启动后转录第一行就是 `⚠ 等工具无进展超时为 30 秒：短于 bash 的默认超时
  120 秒，正常的 cargo test / build 会被误报成卡住了`；
* 发一条消息后 **5 秒**（不是 60/120 秒）provider 判挂死 → 顶部相位变
  `retrying…`、底部 `⠋ retrying · 5s`；
* `/status` 输出：
  ```
  存活阈值:
    provider 响应头超时: 5s
    provider 流空闲超时: 120s
    等模型无进展: 60s
    等工具无进展: 30s
  ⚠ 等工具无进展超时为 30 秒：短于 bash 的默认超时 120 秒，……
  ```
