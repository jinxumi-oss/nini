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

### T1 对照物是分层的，不是一个版本号

**这是 nini 最容易偏离目标的地方，因为它看起来像一个待定事项，其实是一个结构事实。**

| 层 | 对照物 | 状态 |
|---|---|---|
| 文件格式层（session JSONL、settings、slash 命令清单、skills、tools、compaction） | `references/spec-v0.85.1/` — 从 Pi **v0.85.1**（`d981de12`）复制的 19 个文件，带 sha256 | ✅ 已在仓库内，可校验 |
| 行为层（agent loop、TUI 渲染、hooks、编辑器语义） | 本机实装 Pi —— `pi --version` = **0.84.2** | ⚠️ 本机只有 0.84.2 可实测 |

由此产生的三条硬规则：

1. **格式层以 spec 文件为唯一真相**，不引用版本号以外的记忆。
2. **行为层的 oracle 只能对着 0.84.2**，因为那是唯一能被实测的 Pi。
   `crates/nini-conformance/fixtures/pi-0.84.2-*.jsonl` 与代码里成片的 "Pi 0.84.2's ..." 注释**是正确做法，不是过期信息**。
3. `SPEC.lock.compatClaim` 说 "targets Pi v0.85.1 ... 23 builtin slash commands"，
   而 45 条行为 oracle 录在 0.84.2 —— **两者不是矛盾，是分层**。谁把 0.84.2 的注释"修成"0.85.1，
   谁就在删证据（`SPEC.lock.upgradeProcedure` 的前提是先有一台跑着新版本的机器）。

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

## 2. 当前状态（2026-10-17，v0.10.1 + 15 个未发布提交）

| 目标项 | 声明 | 证据 | 差距 |
|---|---|---|---|
| 版本 | v0.10.1 | tag `f183c06`，CI 5 job 全绿 | 15 个提交未发布 |
| 测试 | 1395 → **1428** 通过 / 1 ignored / 0 失败 | 本机全量 `cargo test --workspace`，连续 3 次全绿 | 原 2 个 flake 已修（`/debug` 竞争、`bash` 墙钟断言）；本轮 10-18 补齐 README 数字（原写 1418，实际 1428） |
| 命令面 | 23 + 5 | `registry.rs` 28 条 = Pi 23 ∪ nini 5 | `/share` 仍是 stub（`dispatch.rs:1543`） |
| 格式兼容 | JSONL v4 + legacy-v3 | `nini-conformance` 14 个 test 文件 | 无 |
| 行为 oracle | 45 条，录于 0.84.2 | `crates/nini-conformance` | agent loop / TUI 无 oracle（T1 分层所致，非缺陷） |
| Provider | 5 原生 + `models.json` 其余 | `provider_factory.rs` | 51 个 Pi provider 中 5 个 —— **R5 待 owner 决策**，实现者不自行扩范围 |
| 能力面投入 | — | 自 10-05 起：`nini-tui` +11861 行、`nini-ai` +1295 行；但 `nini-ai` **最后一次提交停在 10-12** | 单侧化，见 §3 |

---

## 3. 排序规则（写下来，是为了不再翻转一次）

规则本身翻过两次，留下的是**方法论**，不是结论：

| 版本 | 规则 | 为什么作废 |
|---|---|---|
| v0.10 §8 **R8** | 按**节点数比值**排序：`nini-ai` 0.21x 优先于 `nini-tui` 3.20x → 冻结 TUI | §11 自己写过"节点数是代理指标"，R8 没用；§31 用 4 个用户报告缺陷证伪 |
| §31 **M5** | 按**用户报告条数**排序 | 工作量由外部输入决定，**无上界** —— 修完 4 个报告，又是 15 个。`nini-ai` 自 10-12 起 5 天零提交、`nini-tui` 同期 +11861 行，就是这个规则失效的样子 |
| **现行** | 按**未验证路径数**排序 | 可枚举、可脚本复算、不随外部输入漂移 |

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
| R5 | 是否把 provider 补齐到 Pi 的 51 个 | 待决 | `nini-ai` 5 天零改动的解法取决于此 |
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
| **P2-1** | 发布 v0.10.2 | Unreleased 覆盖全部未发布提交；README 测试数与二进制一致 | CHANGELOG 已补本次审计的 10 项；README 数字已对齐 **1428**；**只剩打 tag** |
| **P2-2** | 回填 v0.9.2 缺口表 | 每条 ⬜ 要么 ✅+提交号，要么移入“明确不做” | 已被后续版本推翻的条目会误导下一轮调研 |
| **P2-3** | 能力面最低维持 | R5 未决期间**只做不扩范围**的修复 | 不擅自替 owner 决定范围 |

### 2026-10-17 审计补充的待办（均无 oracle，尚未开工）

按严重度降序：

**本轮 P0 已全部关闭**（2026-10-17）。剩余，按严重度降序：

1. **无语法高亮** —— Pi 有 `highlightCode`；nini 9 个 `syntax*` 槽全死。**已在 README known limitations 申报**（10-18）
2. ~~blockquote 正文无样式~~ —— **已修**（`757345a`，`blockquote_style.rs` 6 条）
3. ~~thinking 块不是 markdown~~ —— **已修**（`199e274` 的 `rich::render_thinking_block`，`thinking_markdown.rs` 4 条）
4. **键位与 Pi 的语义差异** —— Pi `Ctrl+-` undo / `Ctrl+D` 删右字符 / `Ctrl+F` 右移一字符 / `Ctrl+Shift+F` 搜索；nini 均不同
5. `move_to_start` / `move_to_end` 仍是“整个 buffer”，Pi 是“当前行”（`Ctrl+A` 在多行下的差异；P0-5 刻意未改，避免二次惊吓）

### 发布链的已知缺陷（10-18 审计新发现）

三项已在本轮关闭，保留记录以便复查：

| 原缺陷 | 关闭方式 |
|---|---|
| CI 不注入 `NINI_BUILD_COMMIT/TIME/DIRTY`，发版产物 `--version` 输出 `unknown (unknown)`（`ci.yml:77` 只跑 `cargo build --release --locked`） | 新增 `crates/nini-cli/build.rs`：从 git 自行取 commit / 时间 / dirty，环境变量可覆盖，`SOURCE_DATE_EPOCH` 支持可复现构建。CI 无需改动 |
| 无发布构建脚本，身份靠人手敲 env | `scripts/build-release.sh`：build → `--version` 必须含 commit → `info` / `demo` / `-p` 三条 smoke；`--check` 额外打印 README 测试数供人工比对 |
| 本机 `~/.local/bin/nini`（10-17 00:58 构建）落后工作树 8 项修复 | 已重建；`--version` 认到当前 commit |

### 明确不做（范围决策已定）

扩展运行时、OAuth/设备流、命令行 session 参数、LLM 分支摘要、prompt 模板与 AGENTS.md 上下文文件。
对它们的行为是**显式拒绝并说明**，不是静默忽略，也不是“顺手做掉”。

---

## 6. 计划文档台账

| 文档 | 状态 | 说明 |
|---|---|---|
| `plans/v0.10-pi-parity-refactor.md` | **active** | R1–R8 路线 + 里程碑实施记录（§12 起） |
| `plans/input_interrupt_parity.md` | **active** | M0/M1 已完成，M2/M3/M4 待做（§8.8） |
| `plans/r2a-compaction-persistence.md` | active/blocked | A1 已发布，A2 被其 §0 的 P0 阻塞 |
| `plans/v0.6*` `v0.7*` `v0.8*` `v0.9.2*` | **frozen** | 已发布版本的实施记录，不再追加 |
| `tui-pi-parity-plan.md` `tui-typography-review.md` | **frozen** | 2026-09-27 后无活动；属 R8 层的调研记录 |
| `.planner/` | **dead** | 2026-09-15 停用；保留作为 §22.1 的证据 |
| `research/pi-architecture.md` `architecture/` | reference | 只读对照物 |
