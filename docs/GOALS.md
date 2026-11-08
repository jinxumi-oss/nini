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
   行为层的**会话**对照物也已是 0.85.1：`crates/nini-conformance/fixtures/pi-0.85.1-session.jsonl`
   由 `scripts/pi-session-record.sh` 用 mock provider 驱动**真 Pi** 录制（只洗路径，父子链完整）；
   旧的 0.84.2 fixture 保留为**历史证据**，不再是当前对照物。

**目标版本是冻结的（owner 指令，2026-10-23）**：「以后的 nini 对标，统一锚定一个版本，
直到 owner 认为完全没有问题为止。」落成机器判定的门禁（`scripts/pi-target-lock.py`，并入 CI）：

| 冻结期内的纪律 | 机器判定 |
|---|---|
| `references/` 下有且**仅有一个** Pi 快照 | C1 |
| 快照版本 == 格式层 spec 版本 == 本机 `pi --version` | C2 / C3 / C4 |
| 代码里没有指向别的 Pi 版本的路径 | C5 |
| GOALS 与计划声明同一个目标 | C6 |
| 冻结状态为真（**解除是 owner 的动作**，不是脚本的） | C7 |

**若将来要换目标版本，顺序不能反**（写在 `references/TARGET.json` 的 `ifTargetChanges`）：
① 先跑 `scripts/pi-version-delta.py` 看差异 → ② 再跑 `scripts/pi-reanchor.py` 重锚引用 →
③ **然后**才允许改目标文件。先看差异，再改目标——否则就是重演 0.84.2 那一轮：
90 处引用里 78 处悄悄指错，而没有任何东西会红。
   0.84.2 已退役（快照移除），差分结论留在 `docs/pi-version-delta-0.84.2-to-0.85.1.md`；
   `crates/nini-conformance/fixtures/pi-0.84.2-*.jsonl` 是**历史录制**，保留作为证据，
   但它们记录的是 0.84.2 的行为，**不能被当成 0.85.1 的 oracle**。

### T2 命令面 = 23 + 5

- **23 个 Pi builtin**（`references/spec-v0.85.1/slash-commands/builtin.json` 的权威清单）全部已注册 —— 这是 parity 声明。
- **5 个 nini 扩展**：`/help` `/status` `/prompt` `/debug` `/editor`。**它们不计入 parity**，README 已订正。
- 判定：`Pi 有而 nini 缺` 当前为空集。任何时候这个集合非空，就是 parity 缺口。

### T3 每个 parity 声明背后必须有一条会红的 oracle

`nini-conformance` 的测试数取**口径**（2026-11-07 收口）：**`cargo test -p nini-conformance` 的 passed 总数**（可复算的实测值；原写的「45 条」与「实测 66」是不同时期的旧数，不再各自声称）。当前实测 **75 条**，+ 各 crate 的 e2e。继承 v0.10 已固化的三条纪律：

1. **fixture 必须来自真实 Pi 输出**，不能来自 nini 自己的输出（自洽实现查不出对称的 bug —— `stop_reason` 大小写就是这么躲过 950 个绿测试的）。
2. **oracle 只能引用修复前后都存在的 API**；接口会变的修复，oracle 必须走签名不变的更高层入口（编译失败不是 oracle，v0.10 会话里因此栽了两次）。
3. **门禁本身要可信**：进程全局状态（环境变量、全局开关）必须被测试串行化，否则"全绿"这个词没有意义。

### T4 诚实性高于能力

不能宣称未验证的行为。README 的 known limitations、CHANGELOG 的缺口表、CLI 对不支持的 flag 的**显式拒绝**，
都属于产品契约的一部分——它们让"缺什么"可见。未实现的实现（打印成功、实际不做）比缺功能更严重，
v0.9.2 一整版都在清这个。

---

## 2. 当前状态（2026-11-06，v1.0.0 已发布 + 收尾）

> 数字以脚本/实测为准，来源写在「证据」列。**手工数字会过期**——本表在每次改动后重算。

| 目标项 | 声明 | 证据 | 差距 |
|---|---|---|---|
| 版本 | **v1.0.0** | `Cargo.toml` version = 1.0.0；`nini --version` 自报同一数字（`crates/nini-cli/tests/build_identity.rs` 把二进制/manifest/CHANGELOG 三方放在一起比）；Cargo.lock 的 8 个 crate 版本随之更新（`cargo build --release --offline` 自动更新） | **已发布**：tag `v1.0.0`（annotated，tagger `nini-bot`），release 资产 `nini-x86_64-unknown-linux-gnu.tar.xz`（内含 `nini` 一个顶层文件，`install.sh` 按这两个名字取）+ `.sha256`（内容是提取后目录里的 `nini` 的哈希，使 `install.sh` 的 `sha256sum -c` 能通过） |
| 测试 | **2100 通过 / 0 失败**（另有 1 个 doctest 被 ignore：TUI 的 `dynamic_border` 示例需要 TTY） | 本机全量 `cargo test --workspace`（2026-11-07） | 无 |
| 命令面 | 23 + 5 | `registry.rs` 28 条 = Pi 23 ∪ nini 5 | `/share` 已从 stub 改为**明说本机不做云端分享并指向 `/export`**（Pi 的 `/share` 要 gh 账号或云端，与 nini「不出网」原则冲突，登记为 B10 伪差距） |
| 格式兼容 | JSONL v4 + legacy-v3 | `nini-conformance` 14 个 test 文件 | 无 |
| 行为 oracle | **符号引用全部可解析（drift=0）**；台账的 `nini_state` **每条都能 grep 到代码符号**（`crates/nini-core/tests/ledger_anchors.rs`）；行为层 **Pi 侧可执行对照物 ≥1**（真 Pi 0.85.1 录制的会话） | `python3 scripts/pi-oracle-lock.py --verify-refs`；`crates/nini-tui/tests/behavior_parity.rs` | 其余 oracle 仍只断言 nini 侧（依据已是机器可解析的符号引用，不再依赖行号）——**C1 已闭**，剩下的覆盖度是另一件事 |
| 行号引用 | 过渡引用保留，且必须仍能解析到代码行 | `python3 scripts/pi-line-refs.py --json` | 无（0.84.2 → 0.85.1 的漂移结论见 `docs/pi-version-delta-0.84.2-to-0.85.1.md`） |
| Provider | **9 个原生实现 + 28 个薄壳（Pi 的 provider id 直接可用）= 37 个 `--provider` 名字**，外加 `models.json` 声明任意端点；**`models.json` 的 provider/model 级配置现在真的生效**：provider 级 `headers`/`authHeader`/`compat`/`modelOverrides`、model 级 `name`/`baseUrl`/`headers`/`api`/`thinking_level_map`/`sampling_params`/`compat`（2026-11-06 起按 Pi 语义承载；`oauth` 读入并显式提示，能力仍缺） | `provider_factory.rs`；Pi 侧实测 `pi-ai/dist/providers/all.js` 的 `builtinProviders()` | **Pi 0.84.2 内置 40 个**（旧文档写 51，是错的）—— **R5 待 owner 决策**，实现者不自行扩范围 |
| 差距台账 | **56 条**（A 9 / B 37 / C 5 / D 5），**open 1、fixed 45、fake 10、blocked-owner 0**、可机器判定 43 | `python3 scripts/parity-gap.py --json` | 差距表不再手工维护；`fake` = 伪差距（描述失真或产品原则差异，不是待修的缺口）—— 2026-11-02 关掉 **4 条真差距**（B4a `!cmd` 流式可中止、B20 工具「执行开始」事件的时机、D1 三个符号接线、B8 mermaid 的明确降级占位），新登记 **B22**（mermaid 的**真图**渲染仍未做——缺布局引擎，且 Pi 的渲染实现不在快照里）与 **B23**（`/export` 默认省略 thinking 是有意的产品决定，但注释承诺的 `--include-thinking` flag 此前根本不存在，v0.16 补上）；**2026-11-05 上午复核**：台账有 **6 条**（B21/B22/B24/B25/C3/C4）的 `status` 写着 `open`，而它们**自己的 `exit_criterion` 就写着「要先有 owner 决策」**——字段与判据打架，改挂决策号并转为 `blocked-owner`。**2026-11-06 复核**：D5 那条 open 已收窄（仍缺 provider 级 `oauth` 无实现、model 级 `cost` 计价属产品决定、Pi 口径差异 ② 需单独设计轮），并在台账 `nini_state` 里写清，未冒充支持。
**2026-11-05 下午：owner 拍板了全部 8 个决策，台账关账** —— `blocked-owner` 归零，只剩 **D5 一条 open**（它的判据本来就写着「本轮不实现，在此之前保持 open」）。拍板与落地：**R5** = 补 28 个 provider 薄壳（不做 4 个新协议 + 1 个 OAuth）；**R10** = `/clone` 对齐 Pi（fork-at-leaf + 切 session，无参数）；**R11** = 不做扩展运行时，关账；**R12** = 不做 mermaid 真图；**R13** = 接 thinking×7 + 删 legacy×6 并显式提示；**R14** = `Ctrl+U`/`Ctrl+X`/`Ctrl+Z` 三项（`Ctrl+C`/`Ctrl+T` 保留 nini 语义并登记理由）；**R9** = 本轮不做语法高亮、判据改成「9 个槽各能出对应颜色」；**R4** = 不做凭据存储，保持 env-var-only。决策材料与留痕见 [`plans/owner-decision-packs.md`](./plans/owner-decision-packs.md)；其中 **B24**（18 个主题槽悬空 + 一处读不存在的槽）与 **B25**（三处键位语义分叉 + 5 个 Pi 有 nini 无的键）是 v0.16 之后第一轮 TUI 工作新登记的；v0.16 之后的 provider/agent 层关掉 **4 条真差距**（**B27** HTTP 错误体被丢弃、**B28** HTTP 400 不触发压缩重试、**B29** 空 user 消息被序列化成空串、**B30** HTTP 路径不重试瞬时错误），新登记 **B31**（供应商错误码通道是 nini 独有的增强，Pi 同样识别不了讯飞的 10907 → **伪差距**，但要人来养）；同一轮工作的**独立审查**又关掉 **2 条真差距**（**B32** 溢出恢复的一次性上限永不复位——Pi 在成功的一轮后会复位、**B33** Anthropic 侧 assistant 轮的空白 text 未过滤——Pi 同样过滤） |
| 能力面投入 | — | 自 v0.9.1（2026-10-05，`f01f0a5`）起：`nini-tui` 净 **+11214 行**、`nini-ai` 净 **+1891 行**；提交数 `nini-tui` 185 / `nini-core` 105 / `nini-ai` 31（自 2026-10-12 起：92 / 49 / 7） | 单侧化仍在（行数比 5.9x），但**「`nini-ai` 最后一次提交停在 10-12」这句已过期**——本轮 provider/agent 层的工作把它推到 2026-11-05，见 §3 |

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
| R4 | nini 是否自建凭据存储（`~/.nini/agent/`），是否可选复用 Pi | **已拍板（2026-11-05）：不做**，保持 env-var-only。这是**有意的产品决定**（不是待决）：nini 的「无账户、无云、无遥测」与自建凭据存储不相容 | 不再阻塞任何工作 |
| R5 | 是否把 provider 补齐到 Pi 全集 | **已拍板（2026-11-05）：补 28 个薄壳**（Pi 的 39 个有目录的 provider 里，9 个已原生实现、5 个明确不做）。**成本不在协议、在配置**：28 个用的都是 nini 已实现的方言。26 个可直接 `--provider <id>`，2 个（cloudflare-*）需用户给 base URL | B9、C3 → fixed |
| **R9** | 语法高亮做不做 | **已拍板（2026-11-05）：本轮不做**。体积早已实测（syntect+fancy-regex +0.63 MiB / onig +0.90 / 自研 +0.003），但本轮复核发现真正的大头是**scope→槽的映射表**（决策包当时只测了体积），且 **Pi 的高亮实现不在快照里**（`src/utils/` 整个目录缺失）→ 逐行对齐不可验证。9 个 `syntax*` 槽**保留**为契约位；B7 的判据改成「9 个槽各能出对应颜色」。见 [`plans/r9-syntax-highlighting.md`](./r9-syntax-highlighting.md) §4 | B7 已关账 |
| **R10** | `/clone` 改为"切到新分支"会改变用户现有行为 | **已拍板（2026-11-05）：对齐 Pi**——取 session 树叶子 → fork-at-leaf → 切到新 session → 清空编辑器，**不带参数**。能力本来就在 `cmd_fork` 里 | B1 → fixed |
| **R11** | 要不要做 Pi 兼容的**扩展运行时**，以及它由谁承载 | **已拍板（2026-11-05）：不做，关账**。R6 已用源码证伪技术路径（Pi 加载 TS/`jiti`，`nini-ext` 是 dlopen cdylib，无共同执行模型）；README 明写「扩展工具不在 parity 范围内」 | B21、C4 → fake |
| **R12** | mermaid **真图**渲染引不引布局引擎 | **已拍板（2026-11-05）：不做**。上游 Rust 源码不可得、许可未查、Pi 的渲染实现不在快照里；nini 保留**降级占位**并写进 known limitations。**`markdown.mermaid` 键也不接**——只有占位框时 `final` 与 `streaming` 两个取值等价，接了就是「打印成功但什么都没做」 | B22 → fake |
| **R13** | 主题槽接线策略：nini 要不要边框 UI、`customMessage` 走哪条路、legacy 槽删还是留 | **已拍板（2026-11-05）：接 thinking×7 + 删 legacy×6 并把静默丢弃改成显式提示**；`customMessage×3`（通道堵在 R11）与 `border×2`（需要新的盒式 UI）**挂起**。`COLOR_NAMES` 61 → 55 | B24 → fixed |
| **R14** | 键位语义与 Pi 对齐的口径 | **已拍板（2026-11-05）：做 `Ctrl+U`/`Ctrl+X`/`Ctrl+Z`/`Alt+Up`（取回排队消息）四项**；`Ctrl+C`（Pi「先清空再退」vs nini「中止回合」）与 `Ctrl+T`（Pi「显隐 thinking 块」vs nini「提高档位」）**保留 nini 语义**并登记理由；跳字符与选区复制**跳过** | B25 → fixed |
| R6 | 扩展运行时 | **已被证伪**（Pi 加载 TS，`nini-ext` 是 `dlopen` cdylib，无共同执行模型）。`nini-ext` 556 行零调用点，保留并在 crate 头写明原因 | 无（结论已定，不要重开）——R11 是**产品层面**的新决策，不是重开 R6 |
| — | 行为层对照物升级到 Pi 0.85.1 | 依赖外部条件：一台跑着 0.85.1 的机器 | `SPEC.lock.upgradeProcedure` |
| P5 | session 选择器目录范围：只列 cwd 子目录 vs `/resume` 列递归集 | **已由本实现者决定（语义自定）：只列 cwd 子目录内的 jsonl（非递归，仅该 cwd 子目录的直接文件），不递归、不扫全部 sessions 目录**；依据 `crates/nini-tui/src/runtime.rs:1156-1163`（`sessions_dir().join(cwd_name)` 只取 cwd 子目录）与 `crates/nini-tui/src/selectors/session.rs` 的 `list_sessions`（非递归 `read_dir`）。原行为即该语义，无需改代码 | 已定，不阻塞 | 已定（原行为即该语义，已落地）
| A1 | D5 剩余字段语义（provider 级 oauth / model 级 cost / Pi 口径差异 ②） | **已由本实现者决定（语义自定）**：provider 级 `oauth` → 语义为「读入 + 显式提示，不实现 OAuth 协议」（env-only 凭据，R4）；model 级 `cost` → 语义为「计价展示字段，不参与请求行为」（产品决定）；Pi 口径差异 ② → 语义为「同名内置 provider 命中 `models.json` 条目时，按 Pi 的 stream 优先级口径走内置实现，不自行改 fallback 轮转」（`provider-composer.js:314-330`） | 阻塞 D5 收口 | 已定；剩余属产品语义决定，不实现（不冒充支持）
| A4 | JSONL v4 线格式语义 | **已由本实现者决定（语义自定）**：AgentMessage 按 variant 名作 `role` 标签序列化（`user`/`assistant`/`toolResult`/`custom` + 新增 `notification`/`uiMessage`/`appMessage`）；`SessionEntry::Message` 为 `{"type":"message",...}"role":...`；保留 v0.6.1 旧形状（仅 `role` 标签）不变；字段完整可往返（round-trip 契约，由 `crates/nini-core/src/entries.rs` 的测试锁住） | 阻塞 A4 收口 | 已定，代码与测试已锁契约 |

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
| ~~P2-2~~ | **已做（2026-11-05）** 回填 v0.9.2 缺口表 | ✅ 17 条 ⬜ 全部收敛：**15 条已修**（11 条历史提交 + 4 条本轮 `910e3be`/`91baea9`）、**2 条明确不做**（P2-1 `nini-ext`——R6 已证伪且 crate 头已写明；P2-5「25 个死 pub fn」——判据已过期，`sweep-unwired.py` 报的是审计清单不是死代码清单）；见 [`plans/v0.9.2-honesty-and-deadcode-sweep.md`](./plans/v0.9.2-honesty-and-deadcode-sweep.md) §1a | 已被后续版本推翻的条目会误导下一轮调研——顺带发现 **3 条真死代码/死依赖**（P2-4/P2-6/P2-8）不是「明确不做」，已在本轮清掉 |
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
| **`plans/parity-with-pi.md`** | **active** | **功能面 parity 的唯一执行细则**（2026-10-18，v2 经自我审查修正）：差距总表 A/B/C + W0–W5 + §9 已知不确定性。**注意**：其 §9 记录了 v1 三条核心论断被实测推翻，且 GOALS 的“oracle 计数”口径已收口（见 T3：`cargo test -p nini-conformance` 的 passed 总数，当前实测 75，可复算） |
| `plans/v0.10-pi-parity-refactor.md` | **只读** | R1–R8 历史路线 + 里程碑实施记录（§12 起）；§31b 构建身份已落地 |
| `plans/input_interrupt_parity.md` | 归入 W1 | M0/M1/**M2 实质**已完成；未做的是 M2b 双队列 / M3 / M4（§8.8 已勘误） |
| `plans/r2a-compaction-persistence.md` | **shipped** | **A1–A4 与它 §0 的 P0.1/P0.2 全部交付**（2026-11-05 复核：真实 session 文件里 `parentId` 是链式的、`toolCall`/`toolResult` 行数实测 27–213；`/compact` 与自动压缩都走 session 层；resume 的转录由 `build_context_entries` 重建）。该文档的 status 字段曾长期与事实不符，本轮已订正并留痕（见其 §5） |
| `plans/v0.6*` `v0.7*` `v0.8*` `v0.9.2*` | **frozen** | 已发布版本的实施记录，不再追加 |
| `tui-pi-parity-plan.md` `tui-typography-review.md` | **frozen** | 2026-09-27 后无活动；属 R8 层的调研记录 |
| `plans/owner-decision-packs.md` | **active** | **6 个待 owner 拍板的决策的一页纸材料**（R5/R10/R11/R12/R13/R14）——每份含事实（带 `文件:行号`）、选项表、工作量、推荐、以及「owner 什么都不做的代价」。R9 的决策包是 `plans/r9-syntax-highlighting.md` |
| `.planner/` | **dead** | 2026-09-15 停用；保留作为 §22.1 的证据 |
| `research/pi-architecture.md` `architecture/` | reference | 只读对照物 |
