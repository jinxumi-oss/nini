# nini 目标与状态（唯一入口）

> 建立：2026-10-17
> 作用：把"nini 要做成什么"写成**可判定的常量**，并把散在 20 个文档里的状态收到一处。
> 效力：本文件是**目标的权威表述**；`docs/plans/*` 是执行细则。两者冲突时以本文件为准，
> 且冲突本身就是需要修的缺陷（请连同证据提 issue，不要自行选一个信）。

---

## 0. 为什么需要这一个文件

2026-10-17 审计到的客观事实（都可复现）：

| 事实 | 证据 |
|---|---|
| 计划文档 20 份，**没有唯一入口**，新工作落在哪份靠记忆 | `docs/plans/` 18 份 + 2 份 09-27 冻结文档 + `.planner/`（09-15 废弃仍在） |
| 一份计划里混了三个主题 | `v0.10-pi-parity-refactor.md` 2272 行 = R1–R8 路线 + TUI 缺陷（§27–30/§33–35）+ 版本自报构建身份（§31b） |
| 章节编号重复，引用因此有歧义 | `§31` 出现两次（`:1650` R8 审查、`:1749` `--version`） |
| 计划状态字段与事实不符 | `input_interrupt_parity.md` 头写"计划待执行"，§8 已记录 M0+M1 完成 |
| 对外声明与代码不符 | README 宣称"28 个 Pi 兼容命令"，Pi 的 `builtin.json` 是 **23** 个，多出的 5 个是 nini 扩展 |
| 状态数字停在发布那天 | README 徽章 1360 测试，实际 1395；CHANGELOG 落后 `v0.10.1..HEAD` 15 个提交 |

---

## 1. 目标：四条可判定的常量

### T1 对照物只有一个版本：**0.85.1**（2026-10-23 统一）

**这是 nini 最容易偏离目标的地方，因为它看起来像一个待定事项，其实是一个结构事实。**

| 层 | 对照物 | 状态 |
|---|---|---|
| 文件格式层（session JSONL、settings、slash 命令清单、skills、tools、compaction） | `references/spec-v0.85.1/` — 从 Pi **v0.85.1**（`d981de12`）复制的 19 个文件，带 sha256 | ✅ 已在仓库内，可校验 |
| 行为层（agent loop、TUI 渲染、hooks、编辑器语义） | `references/pi-0.85.1/` — **Pi 0.85.1 的机制面源码快照，411 个文件，带 sha256 锁** | ✅ 已在仓库内，可校验；本机 Pi 也已升级到 0.85.1 |

**为什么要统一（2026-10-23 的实测）**：此前格式层锁 0.85.1、行为层只能对 0.84.2，
本机只装得下 0.84.2。那不是"分层"，是**目标不唯一**——同一段代码，两个版本可能给出两种答案。
实测代价：0.84.2 → 0.85.1 之间，**90 处行号引用里只有 11 处仍指向同一行**，
78 处指向了别的内容，1 处越界，而**当时没有任何东西会红**。
（证据全文：`docs/pi-version-delta-0.84.2-to-0.85.1.md`）

由此产生的三条硬规则：

1. **一个目标版本 = 0.85.1，格式层与行为层同版本。**
   格式层真相在 `references/spec-v0.85.1/`，行为层真相在 `references/pi-0.85.1/`，
   两者同源同版本；不再有"这个行为只能对另一个版本"的分叉。
2. **行为层的 oracle 用符号引用，不用行号引用。**
   写法：`agent-loop.ts#shouldStopAfterTurn` —— 含义被钉住，行号由
   `scripts/pi-oracle-lock.py --verify-refs` 在目标版本里解析；
   **符号消失 = 红灯，符号挪位 = 报告位移**。行号引用（`agent-loop.ts:225`）只作为过渡保留，
   且必须仍能被解析——否则门禁红。
3. **对照物进仓库 + 带哈希。** 本机 Pi 升级后，行号漂移由脚本检出，不靠人记得。
   0.84.2 已退役（快照移除），差分结论留在 `docs/pi-version-delta-0.84.2-to-0.85.1.md`；
   `crates/nini-conformance/fixtures/pi-0.84.2-*.jsonl` 是**历史录制**，保留作为证据，
   但它们记录的是 0.84.2 的行为，**不能被当成 0.85.1 的 oracle**。

### T2 命令面 = 23 + 5

- **23 个 Pi builtin**（`references/spec-v0.85.1/slash-commands/builtin.json` 的权威清单）全部已注册 —— 这是 parity 声明。
- **5 个 nini 扩展**：`/help` `/status` `/prompt` `/debug` `/editor`。**它们不计入 parity**，README 已订正。
- 判定：`Pi 有而 nini 缺` 当前为空集。任何时候这个集合非空，就是 parity 缺口。

### T3 每个 parity 声明背后必须有一条会红的 oracle

`nini-conformance` 45 条 + 各 crate 的 e2e。继承 v0.10 已固化的三条纪律：

1. **fixture 必须来自真实 Pi 输出**，不能来自 nini 自己的输出（自洽实现查不出对称的 bug —— `stop_reason` 大小写就是这么躲过 950 个绿测试的）。
2. **oracle 只能引用修复前后都存在的 API**；接口会变的修复，oracle 必须走签名不变的更高层入口（编译失败不是 oracle，v0.10 会话里因此栽了两次）。
3. **门禁本身要可信**：进程全局状态（环境变量、全局开关）必须被测试串行化，否则"全绿"这个词没有意义。

### T4 诚实性高于能力

不能宣称未验证的行为。README 的 known limitations、CHANGELOG 的缺口表、CLI 对不支持的 flag 的**显式拒绝**，
都属于产品契约的一部分——它们让"缺什么"可见。未实现的实现（打印成功、实际不做）比缺功能更严重，
v0.9.2 一整版都在清这个。

---

## 2. 当前状态（2026-10-23，v0.10.7）

> 数字以脚本/实测为准，来源写在「证据」列。**手工数字会过期**——本表在每次改动后重算。

| 目标项 | 声明 | 证据 | 差距 |
|---|---|---|---|
| 版本 | **v0.10.7** | tag `v0.10.7` 已推送；HEAD 比 tag 多 2 个提交 | 见下方“发布链” |
| 测试 | **1554 通过 / 0 ignored / 0 失败** | 本机全量 `cargo test --workspace --all-targets`（2026-10-23） | A1 修复后 probe 的 5 条不再 ignored |
| 命令面 | 23 + 5 | `registry.rs` 28 条 = Pi 23 ∪ nini 5 | `/share` 仍是 stub —— **「名字齐」不等于「语义齐」**，已登记为 B10 |
| 格式兼容 | JSONL v4 + legacy-v3 | `nini-conformance` 14 个 test 文件 | 无 |
| 行为 oracle | **66 条**（v0.10 记的 45 已旧），其中 agent loop 12 条直接引 Pi 行号 | `crates/nini-conformance` | **无可执行的 Pi 侧对照物**：全是“注释引行号 + 断言 nini”。v3 已把行为层源码快照锁进 `references/pi-0.84.2/`（sha256），行号漂移从此可机器检出 |
| 行号引用 | 以脚本输出为准（`.js` 与 `.ts` 都要数） | `python3 scripts/pi-line-refs.py --json` | v2 文档写 31（只数了 `.js`）；另有 1 条已被检出越界（`agent-loop.js:582` 实为 `.ts` 行号） |
| Provider | **9 个原生实现（7 个品牌）+ `models.json` 其余** | `provider_factory.rs`；Pi 侧实测 `pi-ai/dist/providers/all.js` 的 `builtinProviders()` | **Pi 0.84.2 内置 40 个**（旧文档写 51，是错的）—— **R5 待 owner 决策**，实现者不自行扩范围 |
| 差距台账 | **19 条**（A 4 / B 11 / C 3 / D 1），open 16、blocked-owner 3 | `python3 scripts/parity-gap.py --json` | 差距表不再手工维护 |
| 能力面投入 | — | 自 10-05 起：`nini-tui` +11861 行、`nini-ai` +1295 行；但 `nini-ai` **最后一次提交停在 10-12** | 单侧化，见 §3 |

---

## 3. 排序规则（写下来，是为了不再翻转一次）

规则本身翻过两次，留下的是**方法论**，不是结论：

| 版本 | 规则 | 为什么作废 |
|---|---|---|
| v0.10 §8 **R8** | 按**节点数比值**排序：`nini-ai` 0.21x 优先于 `nini-tui` 3.20x → 冻结 TUI | §11 自己写过"节点数是代理指标"，R8 没用；§31 用 4 个用户报告缺陷证伪 |
| §31 **M5** | 按**用户报告条数**排序 | 工作量由外部输入决定，**无上界** —— 修完 4 个报告，又是 15 个。`nini-ai` 自 10-12 起 5 天零提交、`nini-tui` 同期 +11861 行，就是这个规则失效的样子 |
| **现行** | 按**未验证路径数**排序 | 可枚举、可脚本复算、不随外部输入漂移 |
| **现行（功能面补充，2026-10-23）** | **功能面 parity 的排序以 [`plans/parity-with-pi.md` §3](./plans/parity-with-pi.md) 为准**：A→B 类，同类内 P0→P3，同严重度内再用上面的「未验证路径数」 | 两条规则原本互相冲突，而 GOALS 规定「冲突本身就是缺陷」。v3 合并为一条 |

**未验证路径数**的定义：一个已实现的、用户可见的行为中，没有对应 oracle 的那些。
`scripts/sweep-unwired.py` 是同源工具（它查"定义了没被调用"，本规则查"跑了但没被验证"）——
theme 键清单 vs 使用该键的组件，是 §31.4 指出的可枚举入口。

**开工上界（硬性）**：任何一轮 TUI 或任何一轮工作，开工前必须写下**退出判据**（可判定）。
没有退出判据的工作，v0.10 会话里已经证明过它的下场：它会变成下一个无界的东西。
例：「`nini-tui` 全部 theme 键各至少有 1 条 oracle，且用户可见的 6 类渲染单元（行/表/块/分隔/滚动/光标）各有 e2e」——
这条满足即停，不追加新花样。

---

## 4. 决策台账（owner 拍板，实现者不自行决定）

沿用 v0.10 §23.3 的纪律：这类问题的性质是**产品决策**，不是 parity 细节。

| # | 决策 | 现状 | 阻塞什么 |
|---|---|---|---|
| R4 | nini 是否自建凭据存储（`~/.nini/agent/`），是否可选复用 Pi | 待决。`6abadd9` 已整体回滚过一次，回滚理由是产品判断而非技术障碍 | auth 层全部工作 |
| R5 | 是否把 provider 补齐到 Pi 全集（**Pi 0.84.2 实测 40 个**；旧文档写 51 是错的） | 待决 | `nini-ai` 5 天零改动的解法取决于此 |
| **R9** | 语法高亮是否值得 +2–5 MB 体积（synctact vs 自研） | 待决 | 阻塞 `parity-with-pi.md` B7 |
| **R10** | `/clone` 改为"切到新分支"会改变用户现有行为 | 待决 | 阻塞 B1 |
| R6 | 扩展运行时 | **已被证伪**（Pi 加载 TS，`nini-ext` 是 `dlopen` cdylib，无共同执行模型）。`nini-ext` 556 行零调用点，保留并在 crate 头写明原因 | 无（结论已定，不要重开） |
| — | 行为层对照物升级到 Pi 0.85.1 | 依赖外部条件：一台跑着 0.85.1 的机器 | `SPEC.lock.upgradeProcedure` |

---

## 5. 下一轮（唯一列表，验收均可证伪）

| # | 事项 | 验收 | 为什么是它 |
|---|---|---|---|
| ~~P0-1~~ | **已做** 门禁去噪：`/debug` 的进程全局竞争 | ✅ 连续 4 次全量 0 失败；`DEBUG_LOCK` + `cmd_debug` 改用全局真值 | 兼带修好 `--verbose` 后首次 `/debug` 会关掉日志的真 bug |
| ~~P0-2~~ | **已做（部分）** footer / `/hotkeys` 与绑定表一致 | ✅ `footer_binding_truth.rs` 2 条绿；`/hotkeys` 改为从 keymap 派生 | 2026-10-17 审计发现同一失真有 **3 份副本** |
| ~~P0-3~~ | **已做** 搜索索引覆盖全文 | ✅ `search_index_coverage.rs` 4 条绿 | 原先只索引每行前 60 字符 |
| ~~P0-4~~ | **已做** 搜索命中可见 + 滚动跟随 | ✅ `search_match_visible.rs` 4 条绿 | 与 P0-3 配套，只修索引不修呈现等于半条 |
| ~~P0-5~~ | **已做** `↑`/`↓` 加 Pi 的 ladder 语义 | ✅ `prompt_vertical_motion.rs` 7 条绿 | 补齐视觉行几何 + 渲染发布宽度 |
| ~~P1-1~~ | **已做** `ScrollToTop` + `Home`/`End` 归 transcript | ✅ `transcript_scroll_to_top.rs` 6 条绿；`Ctrl+E` 补齐 | Pi 的 `Home` 被 viewport listener 优先消费 |
| ~~P2-1~~ | **已做** 发布 v0.10.2 | ✅ 版本已 bump、CHANGELOG 已封版、`--version` 带 profile、CI 增两道门禁；**待 push tag** | CHANGELOG 已补本次审计的 10 项；README 数字已对齐 **1428** |
| ~~P0-6~~ | **已做（2026-10-23）** 修 ESC 并发 spawn（见 §P0-6） | ✅ `RunState::agent_alive` 与 `mode` 解耦；`cargo test -p nini-tui --test esc_concurrency_probe` **不带 --ignored** 退出 0（5 条）；全工作区 1554 通过 / 0 ignored；两条变异注入均验证断言会红 | 修法与证据见 `docs/plans/parity-with-pi.md` §4 W1-A |
| **P2-2** | 回填 v0.9.2 缺口表 | 每条 ⬜ 要么 ✅+提交号，要么移入“明确不做” | 已被后续版本推翻的条目会误导下一轮调研 |
| **P2-3** | 能力面最低维持 | R5 未决期间**只做不扩范围**的修复 | 不擅自替 owner 决定范围 |

### 2026-10-17 审计补充的待办（均无 oracle，尚未开工）

按严重度降序：

### P0-6（2026-10-18 定位，**2026-10-23 已修**）：ESC 后可并发 spawn 第二个 agent

**本会话最严重的发现。** 复现：
`cargo test -p nini-tui --test esc_concurrency_probe -- --ignored --nocapture` → `spawns=2 live=2`。

触发路径就是正常用法：**运行中在编辑器里打字（不提交）→ Esc → Enter**。

根因是**一个字段承担两个职责**：`RunMode` 同时表示“编辑器可否接受输入”与
“是否有 agent 在跑”，而后者是唯一的 spawn 守卫。

- v0.10.1 为修“ESC 后四秒不能输入”（真实缺陷，已修），在按下 Esc 的**瞬间**把 `mode` 置为 `Editing`
- 于是同一行同时做了两件事：让编辑器可用了（对的），**并拆掉了并发守卫**（错的）
- abort 是 `Notify`，agent task 在自己下一次 `select!` 才观察到；在此之前
  `submit_user_input` 的 `mode != Editing` 守卫已失效

Pi 结构上到不了这一步：`AgentSession.isIdle = !_isAgentRunActive`
（`agent-session.js:592`）是 **agent 的属性**，与编辑器解耦；Esc 只调 `agent.abort()`
而不动它；流式期间的提交走 `streamingBehavior: "steer"` 进入当前 turn，永不 spawn 第二个。

后果：两个 agent turn 并发写同一个 `session JSONL`、共享一份 `TokenStats`、
事件交错进同一个 transcript。

**退出判据**：把“编辑器可用”与“agent 存活”拆成两个事实（前者跟 abort 走，
后者跟 `_isAgentRunActive` 的等价物走），且 probe 转为**不忽略**且绿。

**本轮 P0 已全部关闭**（2026-10-17）。剩余，按严重度降序：

1. **无语法高亮** —— Pi 有 `highlightCode`；nini 9 个 `syntax*` 槽全死。**已在 README known limitations 申报**（10-18）
2. **ESC 上下文 handler 栈缺失**（Pi `EscapeTarget`：Idle / Agent / Compaction / Retry / BranchSummary，计划里的 M3）—— 当前 `Abort` 是一条 if-else 链；且 `is_compacting` **无任何赋值点**（全仓库只初始化为 `false`），因此 `runtime.rs:2138` 的“压缩期间提交进队列”分支是**不可达死代码**
3. ~~blockquote 正文无样式~~ —— **已修**（`757345a`，`blockquote_style.rs` 6 条）
4. ~~thinking 块不是 markdown~~ —— **已修**（`199e274` 的 `rich::render_thinking_block`，`thinking_markdown.rs` 4 条）
5. **键位与 Pi 的语义差异** —— Pi `Ctrl+-` undo / `Ctrl+D` 删右字符 / `Ctrl+F` 右移一字符 / `Ctrl+Shift+F` 搜索；nini 均不同
6. `move_to_start` / `move_to_end` 仍是“整个 buffer”，Pi 是“当前行”（`Ctrl+A` 在多行下的差异；P0-5 刻意未改，避免二次惊吓）

### 发布链：二进制身份的四重歧义（10-18 关闭）

同一个 `nini 0.10.1` 曾经同时指四种东西：落后 9 个提交的二进制、CI 里自称 `unknown (unknown)` 的产物、以及 `release` 与 `debug` 两个行为不同的程序（`panic = abort`、overflow checks 关、LTO 开）。四条均已关闭，且都是**可判定**的：

| 原歧义 | 关闭方式 | 判定方法 |
|---|---|---|
| 二进制落后工作树 | `crates/nini-cli/build.rs` 自取 commit / 时间 / dirty | `nini --version` |
| CI 产物自称 `unknown` | 同上；`rerun-if-changed` 覆盖 `Cargo.toml`（改 `[profile]` 会重算） | CI `Version must name its source` 步骤 |
| release 与 debug 难以分辨 | 身份行带 `[release]` / `[debug]` | CI grep `release` |
| **用户 hook panic 直接杀进程** | 移除 `[profile.release] panic = "abort"` | `cargo run --release --example panic_probe -p nini-cli` |

最后一条是本轮最严重的发现：`cargo` 对 **test target 强制 `panic = unwind`**，
因此 `cargo test --release` **结构上无法**验证交付物的 panic 语义——
hook 隔离的测试全绿，而交付二进制里 `catch_hook_panic` 是死代码（已实测：abort 下 exit 134）。
`examples/` 是唯一“由真实 profile 构建且非 test harness”的 target，故探针放那里并进了 CI。
代价：6 054 472 → 6 716 744 字节（+11%）。

### 身份字段的精度（不要把它当构建日志读）

| 字段 | 真实语义 | 已知偏差 |
|---|---|---|
| commit / dirty | build.rs 运行时刻的 git 状态 | 源码改动但未提交时**不触发**重跑，字段保持上次值（`+dirty` 仍正确） |
| 时间 | build.rs 运行时刻，即**编译起点** | LTO 链接在其后数十秒完成；实测相差约 36 s |
| `[release]` / `[debug]` | cargo 的 `PROFILE` | 准（同一 profile 内无差异） |

因此“时间与 commit 不一致”不等于出错，但**“时间很新 + 带 `+dirty`”意味着工作树在编译后又被改过**。

真正的同步判据是三条同时成立：① `cargo build --release` 立即 `Finished in 0.2s`（无重编）；
② 无任何源文件 mtime 晚于二进制；③ **在真终端里验证行为**——
`tmux` 跑二进制看屏幕，是唯一不依赖自报的方法。`/hotkeys` 显示 `Home ?` 就是这么发现的
（`action_label` 漏了新 action，落到 `other => "?"` 臂），已修于 `dc75bca` 并加 oracle。

### 明确不做（范围决策已定）

扩展运行时、OAuth/设备流、命令行 session 参数、LLM 分支摘要、prompt 模板与 AGENTS.md 上下文文件。
对它们的行为是**显式拒绝并说明**，不是静默忽略，也不是“顺手做掉”。

---

## 6. 计划文档台账

| 文档 | 状态 | 说明 |
|---|---|---|
| **`plans/parity-with-pi.md`** | **active** | **功能面 parity 的唯一执行细则**（2026-10-18，v2 经自我审查修正）：差距总表 A/B/C + W0–W5 + §9 已知不确定性。**注意**：其 §9 记录了 v1 三条核心论断被实测推翻，且 GOALS 内的“45 条 oracle”同样已旧（实测 66） |
| `plans/v0.10-pi-parity-refactor.md` | **只读** | R1–R8 历史路线 + 里程碑实施记录（§12 起）；§31b 构建身份已落地 |
| `plans/input_interrupt_parity.md` | 归入 W1 | M0/M1/**M2 实质**已完成；未做的是 M2b 双队列 / M3 / M4（§8.8 已勘误） |
| `plans/r2a-compaction-persistence.md` | active/blocked | A1 已发布，A2 被其 §0 的 P0 阻塞 |
| `plans/v0.6*` `v0.7*` `v0.8*` `v0.9.2*` | **frozen** | 已发布版本的实施记录，不再追加 |
| `tui-pi-parity-plan.md` `tui-typography-review.md` | **frozen** | 2026-09-27 后无活动；属 R8 层的调研记录 |
| `.planner/` | **dead** | 2026-09-15 停用；保留作为 §22.1 的证据 |
| `research/pi-architecture.md` `architecture/` | reference | 只读对照物 |
