# nini TUI「流式渲染速度 / 效率」优化计划

- 需求原文：「nini Tui 项目流试渲染速度效率优化，先写计划，多agent执行」
  （「流试」按「流式」理解：模型输出**流式到达**时，TUI 的每帧渲染成本与 CPU 开销。）
- 取证：`/tmp/pi-work/active/nini-streaming-perf/repro.md`（本机 release profile 实测，全部数字可复现）。
- 门禁：`docs/plans/nini-tui-streaming-render-perf-gate.md`。
- 本计划**只出计划**，执行由多 agent 并行落地（§6 分工）。

---

## 一、要解决的问题（需求 → 现象 → 量化口径）

**现象**：模型正在流式输出时，nini 的 TUI 每来一个 token 就重绘一帧；
帧成本随「已累积的这条消息长度」线性增长，于是**总成本随消息长度平方增长**。

**量化口径**（本轮采用，全部可机测）：

| 口径 | 定义 | 现状实测 |
|---|---|---|
| 帧成本 | 一次 `terminal.draw` 的墙钟时间 | 随消息长度线性增长 |
| 边际成本 | 每多 1 KB 消息内容，帧成本增加多少 | **0.345 ms/KB**（2 次解析/帧） |
| 热点占比 | 助手消息渲染里 `wrap_line` 的占比 | **69%**（0.997 / 1.443 ms，7.8 KB 消息） |

**目标**：在不改变任何一帧**可见输出**的前提下，把边际成本从 0.345 ms/KB 压到 ≤ 0.15 ms/KB
（≈ 2.3× 以上），并让「同一行在一帧里被解析两次」变成一次（再 ≈ 2×）。

---

## 二、取证（实测 + 行号）

### E1 流式帧成本随消息长度线性增长

测量：`crates/nini-tui/tests/streaming_render_perf.rs`（本轮新增，release profile），
模拟「60 轮真实工具结果的历史 + 一条正在增长的助手消息」，每加 24 字节渲染一帧：

```text
STREAM acc=  600 bytes  frame=0.731 ms
STREAM acc= 1200 bytes  frame=0.904 ms
STREAM acc= 2400 bytes  frame=1.337 ms
STREAM acc= 4800 bytes  frame=2.238 ms
STREAM steady (no change) median = 1.332 ms
```

- 斜率：**0.345 ms/KB**（(2.238 − 0.731) / 4.2 KB）。
- 「steady（内容不变）」= 1.332 ms，而「同一时刻内容在变」= 2.238 ms，
  差值 0.906 ms ≈ **一次**该长度消息的解析成本（0.172 ms/KB × 4.8 KB = 0.83 ms，量级吻合）
  ⇒ **同一帧内这条消息被解析了两次**（见 E3）。

### E2 热点是 `wrap_line`，不是 markdown 解析

测量（同一测试文件，`render_markdown` 链路拆解，7.8 KB 助手消息）：

```text
strip_ansi = 0.054 ms
auto_link  = 0.002 ms
markdown   = 0.129 ms
wrap_line  = 0.997 ms   ← 占 render_assistant_message(1.443 ms) 的 69%
```

根因在 `crates/nini-tui/src/width.rs`：

- `width.rs:795` `let mut units: Vec<(String, Style)>` +
  `width.rs:798` `units.push((g.to_string(), span.style))`
  → **每个字素（grapheme）一次堆分配**；
- `width.rs:836 / 851 / 864` `cur.push(Span::styled(t.clone(), *st))`
  → 输出里**每个字素一个 Span**（又一次分配）；
- 连带后果：`render.rs:1714` 的收尾 pass 对**每个 Span** 调一次
  `crate::width::ascii_safe(&sp.content)` → 又一次分配/拷贝。

7.8 KB 文本 ⇒ 约 7800 字素 ⇒ 约 1.6 万次分配，**全部花在布局之外的分配器流量上**。
（对照：markdown 解析本身只要 0.129 ms。）

### E3 一帧内同一行被解析两次

`crates/nini-tui/src/render.rs::render_transcript`：

- 第一遍（数行）：`render.rs:901` `items_for_line(...)` —— 缓存未命中时**整行渲染一遍**只为了拿行数；
- 第二遍（画可见行）：`render.rs:1037 / 1058 / 1105 / 1222` `items_for_line(...)`
  —— 可见行**再渲染一遍**。

流式时每帧至少有一行缓存未命中 ⇒ 每帧 2 次整行解析。E1 的差值 0.906 ms 即此。

### E4 每个 delta 都会唤醒渲染循环

`crates/nini-tui/src/runtime.rs`：

- `AgentSink::push` 末尾 `runtime.rs:585` `let _ = self.notify.send(true);`
  —— `watch::Sender::send` **无论值是否变化都会通知**，所以每个 token delta 都会唤醒循环；
- `runtime.rs:1005` 的 `event_rx.changed()` 分支 → 循环回到顶部 → `runtime.rs:1063`
  `terminal.draw(...)` → **一 delta 一帧**（`watch` 只合并「已排队」的通知，不合并帧）。

⇒ 帧率上限 = min(delta 速率, 1/帧成本)。帧成本一旦随消息变长，循环就会**持续满负荷**，
用户按键被排在渲染后面（输入迟滞）。

---

## 三、步骤

> 判定标准：每一处改动都要能追溯到上面的取证行；追溯不到的，不做。

### 步骤 0 [信息]（已完成）复现与热点定位

- 产出：`crates/nini-tui/tests/streaming_render_perf.rs`（测量用例）+ 本计划 §2 的四组数字。
- 复现命令：
  `cargo test --release -p nini-tui --test streaming_render_perf -- --nocapture --test-threads=1`
- 现状输出（改前）：边际成本 **0.345 ms/KB**；`wrap_line` 占助手渲染 **69%**。

### 步骤 1 [变更]（P0）`width::wrap_line`：字素不再各自分配，输出按样式合并成段

- **worker**：A。**文件**：`crates/nini-tui/src/width.rs`（仅此一个文件）。
- **现状**：`width.rs:773-880`。
- **改法**（行为不变，只改分配与 Span 粒度）：
  1. `units` 由 `Vec<(String, Style)>` 改为**借用**原 span 内容的 `Vec<(&str, Style, usize)>`：
     字素不再单独 `to_string()`；宽度在构造时算一次（原来每个字素算两次）。
  2. 输出不再「一个字素一个 Span」：**连续同样式的字素合并成一个 Span**
     （文本、样式、行列数完全不变，只是 Span 变少）。
  3. 折行末尾「去掉行尾空白」的语义必须**逐字保持**：原实现里每个字素各占一个 Span，
     所以「尾部全空白的 Span 被弹出」==「尾部空白字符被删掉」。合并后一个 Span 可能
     混着空白与非空白（如 `"foo "`），因此收尾改为**按字符**裁掉尾部空白，空 Span 直接丢弃。
  4. 原实现里 `flush` 会**无条件**推出一行（即使 `cur` 为空也会推一个空行，
     例如「一整行只有一个超长单词」的开头），这一行为必须原样保留。
- **复现断言（红 → 绿）**：
  - 新增单测（`width.rs` 内 `mod tests`）：
    a) **文本等价**：与一份「朴素逐字素实现」的参考实现（写在测试里）逐行比对**拼接后的文本**
       （比文本，不比 Span 结构）——覆盖 CJK / emoji / 组合字符 / 超长单词 / 行尾空白 /
       纯空白 token / 多空格；
    b) **结构收敛**：一条 1000 字符的行折行后，**Span 总数不得随字符数线性增长**
       （改前 ≈ 1000，改后应为「样式段数」量级）——钉住「合并」的机器无关断言；
    c) 现有的宽度/截断相关单测全部保持绿。
  - 数值门槛：`cargo test --release -p nini-tui --test streaming_render_perf -- --nocapture`
    里 7.8 KB 助手消息的 `render_assistant_message` ≤ **0.8 ms**（现状 1.443 ms）。
    **门槛依据（已按实测修正）**：本计划初稿写的是 0.6 ms，理由是「消掉分配流量后剩下的
    量级由 markdown 解析（0.129 ms）+ strip/link（0.056 ms）决定，合计约 0.2–0.3 ms」。
    **实测证伪了这个成本模型**：改后空闲机实测 `strip=0.059 link=0.002 markdown=0.112
    wrap=0.517 total=0.669 ms` —— 剩下的 0.517 ms 不是分配流量，而是「一字素一单位」的
    **结构成本**（字素切分 0.167 + 逐字素宽度 0.045 + 单位/输出记账 ~0.3）。
    要再压下去就得改折叠算法（不逐字素切分，除非长词必须断开），属重写、已列 Out of scope。
    故门槛由 0.6 修正为 **0.8 ms**：改前红（1.05–1.44 ms）、改后绿（0.669 ms）、留 ~20% 余量。
    真正的守卫是**结构断言**（Span 数不随字符数增长，机器无关）。
- **回滚命令**：`git checkout -- crates/nini-tui/src/width.rs`

### 步骤 2 [变更]（P1）`render::render_transcript`：一帧内同一行只解析一次

- **worker**：B。**文件**：`crates/nini-tui/src/render.rs`（仅此一个文件）。
- **现状**：E3 的两遍解析。
- **改法**（最小改动，不引入新缓存概念）：
  1. 把 `highlight_for(idx)` 闭包**上移**到第一遍之前（现在定义在 `render.rs:1010` 之后）；
     第一遍的未命中分支改用 `highlight_for(idx)` 渲染（**行数与高亮无关**，行数口径不变）。
  2. 第一遍渲染出来的行，存进一个**帧内局部**的 `Vec<Option<Vec<RenderedRow>>>`
     （按行号索引，长度 = 行数；不是跨帧缓存，帧结束即释放）。
  3. 第二遍（以及兜底分支 `render.rs:1222`）取可见行时：**先查这张帧内表**，
     命中就直接复用，不再重新 `items_for_line`。
- **复现断言（红 → 绿）**：
  - 新增单测（`render.rs` 内 `mod tests`）：构造「一条会增长的助手消息」，
    断言**一帧内 `items_for_line` 的调用次数**不随行数增长；
  - 新增单测：搜索高亮开启时，渲染结果与高亮关闭时的**文本**一致（高亮只改样式）；
  - 数值门槛：同一条消息，**内容不变**的一帧与**内容刚变**的一帧，耗时差 ≤ 0.2 ms
    （现状 0.906 ms / 4.8 KB）。
    **门槛依据**：E1 的差值 0.906 ms ≈ 一次 4.8 KB 解析；步骤 1 之后一次解析本身已降到
    ≈ 0.3 ms 量级（步骤 1 单独生效时差值 ≈ 0.3 ms，步骤 2 生效后差值 ≈ 0），
    0.2 ms 是「步骤 2 生效 + 步骤 1 生效」的联合门槛，留 2× 余量。
- **回滚命令**：`git checkout -- crates/nini-tui/src/render.rs`

### 步骤 3 [验收]（编排者执行）

- `cargo fmt --all -- --check` → 退出码 0。
- `cargo clippy --workspace --all-targets -- -A clippy::pedantic -A clippy::nursery -A clippy::perf -A clippy::complexity -D warnings` → 退出码 0。
- `cargo test -p nini-tui` → 退出码 0。
- `cargo test --workspace --all-targets` → 退出码 0。
- **黄金帧比对**：改动前后，`/tmp/nini-golden-before.txt` 与改动后的帧转储**逐字节相同**
  （覆盖 9 组 fixture × 6 种尺寸 + 12 步流式增长）。
- `bash scripts/gate.sh` → 退出码 0（含 README/CHANGELOG 测试数与实测一致）。

---

## 四、明确不做（Out of scope）

- 不改 `opt-level = "z"`（`Cargo.toml`）：影响整个产物的体积/速度权衡，与本需求无直接证据支撑。
- 不做跨帧的「渲染行缓存」（要把 theme 纳入缓存键，涉及 theme 热重载，改动面大；本轮收益不抵）。
- **不给渲染循环加 60fps 上限**：步骤 1+2 之后，边际成本 ≈ 0.05 ms/KB，
  20 KB 的消息每帧 ≈ 0.4 ms，不再需要额外限帧；限帧会改变「用户按键到画面更新」的时序，
  属于可见行为变更，收益不抵风险。
- 不改滚动单位、不改任何可见输出、不新增依赖、不动 `crates/nini-tui/src/runtime.rs`。
- 不改 CHANGELOG / README 正文（只由编排者在收口时同步**测试数**这一处数字）。

---

## 五、风险与边界

| 风险 | 处置 |
|---|---|
| 步骤 1 改 Span 粒度 → 可见输出变化 | 黄金帧逐字节比对（步骤 3）；文本等价单测 |
| 步骤 1 的行尾空白语义漂移 | 收尾按**字符**裁空白，并保留「无条件推出一行」的原行为；单测钉死 |
| 步骤 2 的搜索高亮路径走偏 | 第一遍用真高亮渲染、行数口径不变；补「搜索高亮下渲染文本不变」断言 |
| 性能断言在慢 CI 上假红 | 结构性断言（Span 数、调用次数）为主；时间断言留足余量（0.15 vs 0.345 ms/KB） |
| 与在途未提交改动冲突 | 步骤 1/2 的文件与在途改动零重叠；`runtime.rs` 本轮不动 |

---

## 六、多 agent 分工（文件互不重叠）

| worker | 范围 | 文件 | 交付 |
|---|---|---|---|
| **A** | 步骤 1 | `crates/nini-tui/src/width.rs` | `wrap_line` 改造 + `mod tests` 单测（文本等价 / 结构收敛） |
| **B** | 步骤 2 | `crates/nini-tui/src/render.rs` | 帧内行复用 + `mod tests` 单测（调用次数 / 搜索高亮等价） |
| **C** | 步骤 0 的回归化 | `crates/nini-tui/tests/streaming_render_perf.rs`（新建） | 流式边际成本回归测试 + 报告改前/改后数字 |

- A、B、C **文件零重叠**，可并行；C 的测试在 A、B 落地前会红（那是「改前」的证据），落地后转绿。
- 编排者（我）负责：黄金帧比对、全量测试、门禁、测试数与对外声明同步、最终收口文档。
- **纪律**：不动在途未提交改动（`crates/nini-core/src/agent.rs`、`crates/nini-tui/src/runtime.rs`
  的剪贴板段、`CHANGELOG.md` / `README.md` 的测试数由编排者在收口时统一改）。

---

## 七、需人确认

- **无**：本轮不改变任何可见输出，不新增用户可见配置，不改产品口径。
  （若步骤 3 的黄金帧比对出现任何差异，视为「可见输出改变」，须回退该项并重新评估。）
