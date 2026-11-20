# 存活指示器 第二期：静默自愈 + 一致性收口

> 日期：2026-11-20
> 来源：第一期（[`liveness-indicator.md`](liveness-indicator.md)）落地后的遗留项
> 执行方式：4 个 agent 并行（nini-ai 空闲超时 ×2、nini-ai 集成测试、TUI 一致性），文件不重叠，靠本文对齐
> 状态：实施中

---

## 0. 遗留清单（第一期后仍未解决的）

| 编号 | 遗留 | 证据 | 本期归属 |
|---|---|---|---|
| L1 | **provider 挂死只"说破"不自愈**：界面会喊 `⚠ no progress 74s`，但进程仍然永远等下去 | `crates/nini-ai/src/openai.rs:54` / `anthropic.rs:34` / `openai_responses.rs:30`：只设 `connect_timeout`，**故意不设总超时**（`error_body.rs:156-163` 的既有结论：总超时会杀掉健康的长流） | P1 + P2 + P3 |
| L2 | **Esc 后两处指示不一致**：顶部仍显示 `working`（相位是事实），底部活动串却消失（判据是 `RunMode`） | `render.rs` `render_key_hints`：门控 `mode == RunMode::Running` | P4 |
| L3 | **watchdog 收尾不复位相位**：agent 任务异常结束后 `phase` 仍是 `"working"` → 顶部永远转圈 | `runtime.rs:4474` / `4539`（watchdog 两个分支只复位 `agent_alive` / `is_compacting`） | P4 |
| L4 | git 跟踪门禁 `every_source_file_is_tracked_by_git` 红 | 用户工作区有两个未跟踪的 `.rs` | **主 agent 已处理**（`git add`，未 commit） |
| L5 | `cargo fmt --check` 红 | 用户工作区两个文件未格式化 | **主 agent 已处理**（`cargo fmt -p nini-cli -p nini-conformance`） |

**本期不做**（明确边界）：

* 不改 provider 的**总超时**（`error_body.rs:156-163` 的既有结论：会杀掉健康的长流）。只加**空闲超时**。
* 不把空闲超时做成用户设置项（本轮硬编码 + 常量，理由见 §2）。
* 不改 `crates/nini-core/src/settings.rs`、不改 CHANGELOG / README。
* 不新增 Cargo.toml 依赖行（`futures-util` / `tokio` / `reqwest` / `nini-core` 都是现成的）。
* 不改 Pi 的 footer / 布局结构。

---

## 1. 硬约束（四个 agent 都必须遵守）

1. **只改自己负责的文件**（见 §2），不碰其他文件；不改 CHANGELOG / README / 任何 docs（契约文档本身由主 agent 写）。
2. **API 严格照本文**，不要自行改名或加别名 —— 另有 agent 并行依赖。
3. **不加依赖行**：不许改任何 `Cargo.toml`。需要的东西都已存在。
4. 不重构、不做"顺手优化"、不扩大范围。注释用中文。
5. 写完后跑 `cargo check -p <你的 crate> 2>&1 | tail -30`（cargo 锁会排队，等即可）。
6. 单测放自己负责的文件内（沿用该文件既有测试模块）；只有 P3 可以新建 `crates/nini-ai/tests/` 下的文件。

---

## 2. P1 契约：空闲超时（核心）

### 2.1 新文件 `crates/nini-ai/src/stream_idle.rs`

**为什么需要它（不要当成"以防万一"）**：连接建立后上游一个字节都不发的情况下，
`response.bytes_stream().next()` 会永远挂着。reqwest 的 `timeout` 是**总超时**
（覆盖整个响应体），用它会杀掉健康的长流；所以只能在**chunk 之间**做空闲判定。

**精确 API**（P2/P3 依赖它，一字不改）：

```rust
//! crates/nini-ai/src/stream_idle.rs
use std::time::Duration;

/// 默认空闲超时：两个 chunk 之间超过这么久没有数据，就判定上游挂死。
///
/// 取 120s 的理由：与 `nini_tools::bash` 的默认命令超时（120s）同量级；
/// 远大于任何健康流的最大 chunk 间隔（模型思考时也可能几十秒不吐字），
/// 又远小于"永远挂着"（用户视角：75s 实测无任何变化）。
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// 从 provider 的字节流里读下一个 chunk。
///
/// * 有数据 → `Ok(Some(chunk))`
/// * 上游正常结束 → `Ok(None)`
/// * 传输错误 → `Err(ProviderError::Http(..))`（与 `From<reqwest::Error>` 同口径）
/// * **空闲超时**（连接还在，但太久没有字节）→ `Err(ProviderError::Http(
///   "provider stream stalled: no data for 120s"))`
pub async fn next_chunk<S, T>(
    stream: &mut S,
    idle: Duration,
) -> Result<Option<T>, ProviderError>
where
    S: futures_core::Stream<Item = Result<T, reqwest::Error>> + Unpin,
{
    match tokio::time::timeout(idle, stream.next()).await {
        Ok(Some(Ok(item))) => Ok(Some(item)),
        Ok(Some(Err(e))) => Err(ProviderError::from(e)),
        Ok(None) => Ok(None),
        Err(_) => Err(ProviderError::Http(format!(
            "provider stream stalled: no data for {}s",
            idle.as_secs()
        ))),
    }
}
```

* 需要 `use futures_util::StreamExt;`（`.next()`）与 `use nini_core::provider::ProviderError;`。
* `crates/nini-ai/src/lib.rs` 里加 `pub mod stream_idle;`（保持字母序）。
* **不要**为此新增 `bytes` 依赖行：上面的泛型 `<S, T>` 已经避开了具体字节类型。

### 2.2 单测（`stream_idle.rs` 模块内）

用 `#[tokio::test]` + **真实时间 + 小预算**（`idle` 取 1s，慢流取 100ms 间隔）。
> **勘误（实施中发现）**：tokio 1.53 的 `full` 特性**不含 `test-util`**，
> `#[tokio::test(start_paused = true)]` 用不了；本轮硬约束禁止改 Cargo.toml，
> 因此不引入虚拟时间，整包 `nini-ai` lib 测试约多花 1.05s。

至少覆盖：

1. 正常流：依次拿到所有 chunk。
2. 流结束：`Ok(None)`。
3. 流本身报错 → `ProviderError::Http`，且消息含上游原因（与 `From<reqwest::Error>` 同口径）。
4. **流沉默** → `ProviderError::Http`，消息含 `"stalled"`。
5. **慢但有数据**（间隔小于 idle）→ 不误报。

### 2.3 P1 还要接入 `crates/nini-ai/src/openai.rs`

把（`openai.rs:169` 附近）

```rust
while let Some(chunk_result) = byte_stream.next().await {
    let chunk = chunk_result?;
```

改成

```rust
while let Some(chunk) = crate::stream_idle::next_chunk(
    &mut byte_stream,
    crate::stream_idle::DEFAULT_IDLE_TIMEOUT,
)
.await?
{
```

其余一行不动。

> **勘误（实施中发现，原片段有类型错）**：`next_chunk` 返回
> `Result<Option<T>, ProviderError>`（内层 `Result` 已在 §2.1 里拆开），
> 所以 `?` 必须提到外层、绑定名用 `chunk`（不是 `chunk_result`）——
> 否则 E0308「expected Result, found Option」。三个 provider 都用上面这个形状。
> 传输错误的类型仍是 `ProviderError`（`From<reqwest::Error>` 那一支），
> 与改造前语义一致。

---

## 3. P2 契约：另外两个 provider 接入

`crates/nini-ai/src/anthropic.rs`（约 147 行）与
`crates/nini-ai/src/openai_responses.rs`（约 141 行）里的
`while let Some(chunk_result) = byte_stream.next().await { let chunk = chunk_result?;`
**同样**换成 §2.3 的写法（`crate::stream_idle::next_chunk(&mut byte_stream, crate::stream_idle::DEFAULT_IDLE_TIMEOUT)`）。

两个文件各改一处，其余不动。

---

## 4. P3 契约：真实 reqwest 流上的集成测试

新建 `crates/nini-ai/tests/provider_stream_idle.rs`：

* 用 `wiremock`（dev-dependency 已有）起一个 `text/event-stream` 响应：
  **先发一个 chunk，然后永不结束**（服务端挂住）。
  > **实施补充**：wiremock 0.6.5 的 `ResponseTemplate` 只有定长 body，
  > 表达不了"永不结束"（塞 `content-length` 会让 hyper panic，
  > `Transfer-Encoding: chunked` 则总会收尾）。"挂住"那条改用**裸 `TcpListener`**
  > ——照抄本 crate 既有的 `tests/error_body_timeout.rs` 同一套写法；
  > "正常结束"那条仍用 wiremock。
* 用 `reqwest` 取 `response.bytes_stream()`，调
  `nini_ai::stream_idle::next_chunk(&mut stream, Duration::from_millis(200))`：
  * 第一个 chunk → `Ok(Some(..))`；
  * 第二个 → `Err(ProviderError::Http(msg))`，`msg` 含 `"stalled"`。
* 再补一条：服务端正常结束的流 → `Ok(None)`。

断言只针对公开 API 的返回值，不碰内部实现。

> 本 crate 的 dev-dependencies 里已有 `wiremock` / `tokio-stream` / `nini-ai`（带 `test-exports` 特性），
> 不需要新增任何依赖行。

---

## 5. P4 契约：TUI 侧一致性收口

### 5.1 `crates/nini-tui/src/render.rs`

1. `activity_indicator`（纯函数）**最前面**加一条：相位为 `"idle"` 时一律返回 `None`
   （哪怕 `current_tool` 有残留也不显示）——让"是否显示"只有一个判据：相位。
2. `render_key_hints` 里活动串的门控从
   `if state.run_state.mode == RunMode::Running { … } else { None }`
   改为**无条件**调用 `activity_indicator(...)`（判据交给纯函数）。

   > 理由：`RunMode` 回答"编辑器能不能打字"，`phase` 回答"agent 是否在跑"。
   > Esc 之后 mode 回到 Editing 而 agent 还在收尾，这段时间两处必须一致。

3. 既有单测里那条"非 Running 模式不显示活动指示器"改为"相位 idle 时不显示"；
   新增一条"Editing 但 phase=working 时显示活动串"。

### 5.2 `crates/nini-tui/src/runtime.rs`

watchdog 的**两个**异常分支（约 4474 行与 4539 行：`g.run_state.agent_alive = false;` 之后）
各补两行，把相位与工具复位：

```rust
g.run_state.phase = "idle".to_string();
g.run_state.current_tool = None;
```

> 不复位的话：agent 任务异常结束后相位永远是 `"working"` → 顶部永远转圈，
> 底部活动串也永远在（本期把活动串判据换成相位后，这条从"几乎看不见"变成"一直转"）。

`Done` / `Aborted` 分支第一期已复位，**不要重复改**。

---

## 6. 验收（主 agent 串行执行）

1. `cargo build --workspace` ✅；`cargo clippy --workspace --all-targets -D warnings` ✅（CI 口径）；
   `cargo fmt -- --check` ✅；`cargo test --workspace` ✅（含 `every_source_file_is_tracked_by_git`）。
2. tmux 实测 A：30s bash 执行中 → 底部 `⠋ bash · Ns`，秒数递增。
3. tmux 实测 B：**provider 挂死 125s** → 界面出现 `⚠ no progress …`，且随后
   **provider 层报错**（转录里出现 stalled 错误，或 fallback 链切到下一个 provider）。
4. tmux 实测 C：Esc 中止 → 转录出现 `[aborted] run aborted`。
5. tmux 实测 D：Esc 之后 agent 未收尾的窗口内，顶部与底部指示一致（都在显示工作）。

---

## 7. 实施记录（实测发现的缺口与勘误）

这一节记的是**实施中真正撞到的东西**，不是计划。每条都带证据。

### 7.1 第一期设计漏了一个挂死点（已补）

第一期只给 **chunk 之间**加了空闲超时（`next_chunk`）。实测（tmux + 黑洞服务器：
accept 连接后一个字节都不发）：

```
T+69s   ⚠ no progress 69s · waiting for model
T+129s  ⚠ no progress 129s · waiting for model
T+150s  ⚠ no progress 150s · waiting for model      ← 150 秒过去，provider 层一声不吭
```

原因：reqwest 的 `send()` 等的是**响应头**。上游连上但不发响应头时，
代码还卡在 `send().await`，**根本没进到 `bytes_stream()`** —— `next_chunk` 管不到它。

补法（新增 `stream_idle::send_with_idle_timeout`）：

```rust
pub async fn send_with_idle_timeout(
    request: reqwest::RequestBuilder,
    idle: Duration,
) -> Result<reqwest::Response, ProviderError>
```

错误文案与 chunk 那一支**刻意不同**，以便转录里能分辨两种挂死：

* `provider response stalled: no headers for 120s`（还没进入流）
* `provider stream stalled: no data for 120s`（进了流但没数据）

补完后的实测（同一套黑洞服务器）：

```
T+69s   ⚠ no progress 69s · waiting for model
T+129s  ⠋ waiting for model · 129s
        转录：[error] retrying after transient error (attempt 1, delay 2000ms)
```

即：120s 判定挂死 → 错误上屏 → core 判定 transient → **自动重试**（退避 2s/4s/8s）→ 再挂再重试。
「静默挂死」从此有终点。

### 7.2 契约本身的错（两个 agent 各自撞到）

* **§2.3 的代码片段类型错**：`next_chunk` 返回 `Result<Option<T>, ProviderError>`，
  契约片段却写成 `while let Some(chunk_result) = next_chunk(...).await {` —— E0308。
  正确形状是 `while let Some(chunk) = next_chunk(...).await? {`。§2.3 已补勘误。
* **tokio 的 `full` 特性不含 `test-util`**：契约里写的
  `#[tokio::test(start_paused = true)]` 用不了（tokio 1.53）。改为真实时间 + 小预算
  （idle 取 1s），整包 `nini-ai` lib 测试约多花 1.05s。
* **wiremock 0.6.5 表达不了"永不结束的响应"**：`ResponseTemplate` 只有定长 body；
  塞 `content-length` 会让 hyper panic，`Transfer-Encoding: chunked` 则总会收尾。
  "挂住"那条改用裸 `TcpListener`（照抄本 crate 既有 `tests/error_body_timeout.rs`）。

### 7.3 活动串文案随相位（最后一条一致性）

相位 5 态已经在顶部上屏，但底部活动串在「没有工具在跑」时恒定说
`waiting for model` —— 压缩/重试/分支摘要期间与顶部不一致。改为按相位取词：
`working → waiting for model` / `retrying → retrying` / `compacting → compacting` /
`branch_summary → summarizing branch`；未知值退回 `waiting for model`。

### 7.4 没有做（边界）

* 不做 provider **总超时**（`error_body.rs:156-163` 的既有结论：会杀掉健康的长流）。
* 不把空闲超时做成用户设置项（本轮硬编码 120s，常量在 `stream_idle.rs` 顶部）。
* 不改 `crates/nini-core/src/settings.rs`、不改 CHANGELOG / README。
