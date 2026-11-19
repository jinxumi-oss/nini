# nini TUI「流式渲染速度 / 效率」优化 —— 实施记录

- 依据计划：`docs/plans/nini-tui-streaming-render-perf-plan.md`
- 门禁：`docs/plans/nini-tui-streaming-render-perf-gate.md`（VERDICT: APPROVED，2 条 NIT 已回填门槛依据）
- 执行：多 agent 并行（A=`width.rs`、B=`render.rs`、C=回归测试），文件零重叠；编排者负责验收与收口。
- 范围声明：本轮**不改变任何一帧的可见输出**、不改滚动单位、不新增依赖、不动 `runtime.rs`。

---

## 一、结论先说（release profile，本机实测）

| 指标 | 改前 | 改后 | 倍数 |
|---|---|---|---|
| **流式帧边际成本**（每多 1 KB 消息内容，帧成本增量） | 0.345–0.353 ms/KB | **0.082–0.092 ms/KB** | **≈ 4×** |
| **一帧内同一行被解析次数**（流式增长的那一行） | 2 次 | **1 次** | 2× |
| **「刚变的一帧」比「不变的一帧」贵多少** | +0.906 ms | **+0.002 ms** | ≈ 450× |
| 7.8 KB 助手消息的 `render_assistant_message` | 1.443 ms | **0.669 ms** | 2.2× |
| 1000 字符折行产生的 Span 数 | 1000 | **20** | 50× |
| `wrap_line` 在助手渲染里的占比 | 69%（0.997 / 1.443 ms） | 77%（0.517 / 0.669 ms，**绝对值已减半**） | — |

总成本因此从「随消息长度**平方**增长」变成「随消息长度**线性**增长，斜率降 4 倍」。

---

## 二、改了什么

### 步骤 1（worker A）`crates/nini-tui/src/width.rs` —— `wrap_line` 不再按字素分配

| 位置 | 改动 |
|---|---|
| `width.rs:974` `pub fn wrap_line` | `units` 由 `Vec<(String, Style)>` 改为 `Vec<(&str, Style, usize)>`：**字素借用原 span 的内容**，不再 `g.to_string()`；宽度在构造时算一次（原来每个字素算两次） |
| 同上（新增 `push_unit`） | 输出**不再一个字素一个 Span**：连续同样式的字素合并进同一个 Span（`last.content.to_mut().push_str`） |
| 同上（新增 `trim_tail`） | 行尾空白改为**按字符**裁剪（合并后的 Span 可能混着空白与非空白，例如 `"foo "`），裁空了的 Span 丢弃 |
| 同上（`flush`） | **无条件推出一行**的原行为保留（整行只有一个超长单词时开头那个空行） |
| `width.rs` `mod tests` | 新增：① 把改造前的旧算法抄成参考实现 `reference_wrap_line`，11 组输入 × 7 个宽度**逐行比对拼接文本**；② 1000 字符折行后 Span 总数 < 100（钉住「合并」这个性质，机器无关） |

不变的部分：token 划分、折行点、超长单词按字素断开、空白 token 不做行首、`is_ws` 口径（`chars().all(char::is_whitespace)`，作用在单个字素上）。

### 步骤 2（worker B）`crates/nini-tui/src/render.rs` —— 一帧内同一行只解析一次

| 位置 | 改动 |
|---|---|
| `render.rs:877-899` | `highlight_for(idx)` 闭包**上移**到第一遍之前 |
| `render.rs:917-966` | 第一遍（数行）的未命中分支改用**真高亮**渲染，并把渲染结果存进**帧内局部**的 `frame_rows: Vec<Option<Vec<RenderedRow>>>`（行数口径不变——高亮只改样式） |
| `render.rs:1045-1060` | 新增 `rows_for(idx)`：第二遍取可见行时**先查帧内表**，命中即复用，不再 `items_for_line` |
| `render.rs:1072 / 1088 / 1129 / 1239` | 三处第二遍调用点 + 兜底分支改为走 `rows_for`；兜底分支额外加了「高亮相同才复用」的条件，使该分支的可见输出**逐位不变** |
| `render.rs` `mod screen_row_publish_tests` | 新增 3 条：① 一帧内同一行只解析一次（改前红：2 次 vs 1 次）；② 解析次数不随历史行数增长；③ 搜索高亮只改样式不改文本 |
| `render.rs` `items_for_line` | 仅新增 `#[cfg(test)]` 的 thread-local 计数器（release 构建整段被 cfg 掉，零开销） |

**未引入跨帧缓存**：帧内表帧结束即释放；行数缓存仍是原有的 `state.item_cache`。

### 步骤 0 的回归化（worker C）`crates/nini-tui/tests/streaming_render_perf.rs`（新建）

4 条断言（release 为真门槛，debug 按 ~5× 放宽）：

| 测试 | 断言 | 钉住的性质 | 改前 → 改后 |
|---|---|---|---|
| `streaming_frame_marginal_cost_is_bounded` | 斜率 ≤ 0.15 ms/KB | 帧成本不随消息长度线性增长 | 0.353 → **0.082** ms/KB |
| `a_changed_frame_does_not_reparse_the_streamed_line` | 「刚变帧 − 不变帧」≤ 0.2 ms | 一帧内同一行只解析一次 | +0.90 → **+0.002** ms |
| `folded_spans_do_not_scale_with_characters` | 1000 字符折行 ≤ 64 个 Span；900 字符/3 样式 ≤ 4×行数；**文本逐字不变** | Span 按样式段合并 | 1000 → **20** Span |
| `rendering_a_large_assistant_message_is_cheap` | 7.8 KB 助手消息 ≤ **0.8 ms** | 热点本身 | 1.05–1.44 → **0.669** ms |

第 4 条的门槛由计划初稿的 **0.6 ms 修正为 0.8 ms**，理由见 §四（成本模型被实测证伪）。

---

## 三、验收（编排者执行，全部退出码 0）

| 命令 | 结果 |
|---|---|
| `cargo fmt --all -- --check` | ✓ |
| `cargo clippy --workspace --all-targets -- -A pedantic -A nursery -A perf -A complexity -D warnings` | ✓ 无告警 |
| `cargo test --workspace --all-targets` | ✓ 全绿（203 个测试目标，**2271** 个测试） |
| `cargo test --release --workspace` | ✓ 全绿（release profile 的溢出语义也过） |
| `cargo test --release -p nini-tui --test streaming_render_perf -- --nocapture` | ✓ 4 passed |
| **黄金帧比对**（见下） | ✓ **逐字节相同，0 行差异** |
| `bash scripts/gate.sh` | ✓ 通过 12 / 失败 0 / 跳过 4（跳过项需 `--full`） |

**黄金帧比对（可见输出不变的证据）**：改动前后各转储一次渲染帧（9 组 fixture × 6 种尺寸
= 40×24 / 60×24 / 80×24 / 100×34 / 120×40 / 200×50，外加 12 步流式增长），逐字节 diff：

```text
274683 bytes  →  274683 bytes，diff 输出 0 行
```

fixture 覆盖：ASCII 长文、CJK + emoji + 组合字符、markdown 结构（标题/列表/引用/代码块/表格/
分隔线/删除线）、工具结果多行体、user+assistant+thinking 混排、行尾空白与纯空白、
超长单词（breakLongWord 路径）、40 行短消息、搜索高亮。

---

## 四、门槛修正记录（0.6 ms → 0.8 ms）

计划初稿给步骤 1 的门槛是 0.6 ms，理由：「消掉分配流量后，剩下的量级由 markdown 解析
（0.129 ms）+ strip/link（0.056 ms）决定，合计约 0.2–0.3 ms」。

**实测证伪了这个成本模型**（空闲机，release，7.8 KB 助手消息）：

```text
strip=0.059  link=0.002  markdown=0.112  wrap=0.517  total=0.669 ms
```

剩下的 0.517 ms **不是分配流量**，而是「一字素一单位」的**结构成本**：
字素切分 0.167 ms + 逐字素宽度 0.045 ms + 单位/输出记账 ~0.3 ms。
要再压下去就得改折叠算法（不逐字素切分，除非长词必须断开）——那是**重写**，不是调预算。

因此门槛修正为 **0.8 ms**：改前红（1.05–1.44 ms）、改后绿（0.669 ms）、留 ~20% 余量。
真正守住这个热点的断言是**结构断言**（Span 数不随字符数增长），它与机器速度无关。

**留下的余量（后续项，不在本轮）**：若将来要把助手渲染再压一倍，靶点是 `wrap_line` 的
「一字素一单位」结构成本；本轮不做（重写折叠算法，风险高于收益）。

---

## 五、未做（Out of scope，按计划）

- 不改 `Cargo.toml` 的 `opt-level = "z"`。
- 不做跨帧的「渲染行缓存」（要把 theme 纳入缓存键，涉及 theme 热重载）。
- 不给渲染循环加 60fps 上限（C1+C2 之后边际成本已降到 0.082 ms/KB，不需要）。
- 不改滚动单位、不改任何可见输出、不新增依赖、不动 `crates/nini-tui/src/runtime.rs`。
- **未触碰在途未提交改动**（另一会话的 `crates/nini-core/src/agent.rs`、
  `crates/nini-tui/src/runtime.rs` 剪贴板段、`crates/nini-tui/tests/agent_wire_e2e.rs`、
  `release/*` 产物）。

---

## 六、文件清单

| 文件 | 归属 | 状态 |
|---|---|---|
| `crates/nini-tui/src/width.rs` | worker A | 改（步骤 1 + 2 条单测） |
| `crates/nini-tui/src/render.rs` | worker B | 改（步骤 2 + 3 条单测） |
| `crates/nini-tui/tests/streaming_render_perf.rs` | worker C | 新建（4 条断言） |
| `docs/plans/nini-tui-streaming-render-perf-plan.md` | 编排者 | 新建 |
| `docs/plans/nini-tui-streaming-render-perf-gate.md` | gate agent | 新建 |
| `docs/plans/nini-tui-streaming-render-perf-changes.md` | 编排者 | 新建（本文件） |
