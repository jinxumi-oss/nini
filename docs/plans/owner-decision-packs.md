# Owner 决策包 —— 6 个待拍板的一页纸

> 建立：2026-11-05（v0.16.0 之后）
> 作用：把台账里**卡在 owner 手里**的 6 个决策，从「一句话提问」变成「事实 + 选项 + 代价」的可拍板材料。
> 权威：决策本身属 owner（`docs/GOALS.md` §4 决策台账）；本文件只提供**证据**与**选项**，不替 owner 拍板。
> 机器数据：`python3 scripts/parity-gap.py --json`（台账）、`python3 scripts/keymap-parity.py`（键位）。

---

## 0. 这份文件是什么

### 0.1 它是什么

- 一份**决策材料**，不是实现计划，也不是待办清单。每一节的结构固定：
  **事实**（带可核实的 `文件:行号` 证据）→ **选项**表（口径 / 要动哪些文件 / 工作量估计 / 后果与风险）
  → **推荐** → **拍板后怎么验收** → **如果 owner 什么都不做，代价是什么**。
- 覆盖 6 个决策：**R5、R10、R11、R12、R13、R14**。
- 每个决策下面都列了它**实际卡住哪些台账条目**。

### 0.1b 证据从哪来（引用能不能复算）

本文件的引用分两类，**能不能复算不一样**，先说清楚：

| 类别 | 长什么样 | 能不能在仓库里复算 |
|---|---|---|
| **仓库内** | `crates/…`、`references/pi-0.85.1/…` | **能**——仓库里就有那个文件与那一行 |
| **本机 Pi 安装包** | 标着「安装包」的 `theme.js:242`、`components/tree-selector.js:673`、`src/parse.ts:88`、`package.json:60` 等 | **不能**——这些文件只在**本机安装的 Pi** 里（`~/.nvm/versions/node/v*/lib/node_modules/@earendil-works/pi-coding-agent/`），**不在 `references/pi-0.85.1/` 快照里**。快照是本项目唯一可复算的对照物，所以这类的结论**只能作为旁证**，不作为判据 |

写这一节是因为「引用了一个仓库里没有的文件」和「引用了一个仓库里有的文件」不是一回事：
前者无法被第三方复算。后者（占绝大多数）都能。

### 0.2 它不是什么

- 不是「实现者自行决定」的授权书。每一节的「推荐」都只是**从工程事实出发的倾向**；
  在 owner 在 `docs/GOALS.md` §4 写下结论之前，实现者不动手。
- 不是覆盖率报告。台账里的条数只用来定位，不用来评价工作。

### 0.3 一处必须先纠正的口径

台账 `blocked-owner` 一共 **9 条**：`B1、B7、B9、B21、B22、B24、B25、C3、C4`。

| 决策 | 卡住的台账条目 | 条数 |
|---|---|---|
| R5 | `B9`、`C3` | 2 |
| R10 | `B1` | 1 |
| R11 | `B21`、`C4` | 2 |
| R12 | `B22` | 1 |
| R13 | `B24` | 1 |
| R14 | `B25` | 1 |
| **小计** | | **8** |
| （R9） | `B7`（语法高亮）——已有独立决策包 `docs/plans/r9-syntax-highlighting.md` | 1 |
| **合计** | | **9** |

也就是说：**这 6 个决策卡住 8 条，不是 9 条**；第 9 条属于 R9，不在本文件范围内。

### 0.4 证据口径

- **快照**（`references/pi-0.85.1/`）与**本机同版本安装包**（`@earendil-works/pi-coding-agent`，同为 0.85.1）
  是两个来源。凡是引用安装包的证据，本文件都写明「来源是安装包不是快照」——原因与 R9 决策包相同：
  快照是子集，缺文件不等于上游没有。
- 涉及 **0.84.2** 的旧口径（`docs/GOALS.md` §2 里「Pi 0.84.2 内置 40 个」）本文件一律用 **0.85.1** 的实测值；
  实测 0.85.1 的 `builtinProviders()` **也是 40 个**，两版数字相同。
- 所有「行数 / 条数 / 提交数」都是**本文件写作时的实测值**，会随提交漂移。复算命令都写在相应小节里。

### 0.5 拍板之后怎么落地（本文件只做准备，不代劳）

1. 在 `docs/GOALS.md` §4 决策台账里给该决策写一行结论。
   ⚠️ 现状：§4 只有 `R4 / R5 / R9 / R10 / R6` 五行，**R11–R14 四行缺失**——
   而 `scripts/parity-gap.py:81` 的 `DECISIONS` 白名单**已经**包含 `R11 R12 R13 R14`。
   落地时必须把 §4 与白名单补齐，否则台账与 GOALS 各说一套。
2. 改 `scripts/parity-gaps.json` 里对应条目的 `status`（`blocked-owner` → `open`/`fixed`/`fake`）
   与 `exit_criterion`（把「等 owner 拍板」换成可判定的判据）。
3. 实现只做被拍板的那一条口径，不顺手扩范围。

---

## 1. R5 —— provider 覆盖面要不要追平 Pi

**台账**：`B9`（P1，provider 覆盖面）、`C3`（P2，投入单侧化）。

### 1.1 事实

| 事实 | 证据 |
|---|---|
| Pi 0.85.1 内置 **40 个** provider | `references/pi-0.85.1/src/provider/all.ts:89-131` 的 `builtinProviders()`；dist 版 `references/pi-0.85.1/dist/provider/all.js:67` |
| nini 只认 **9 个非 fixture 名** + `fixture` | `crates/nini-cli/src/provider_factory.rs:510` 的 `build_provider`，分支在 `:526-628`：`anthropic / openai / openai-responses / openai-compat / google / deepseek / groq / mistral / cohere` |
| 品牌交集只有 **6 个** | `anthropic、deepseek、google、groq、mistral、openai`；**Pi 有而 nini 无的品牌 34 个**，`cohere` 是 nini 独有 |
| 40 个按「协议方言」分类（逐个读 `references/pi-0.85.1/src/api/*.ts`） | **T1 已原生 6**；**T2 只差 baseUrl + env key（方言 nini 已有）22**；**T2b Cloudflare 专有 stream/auth 2**（`cloudflare-ai-gateway`、`cloudflare-workers-ai`，用 `cloudflareStreams(...)` 包装）；**T3 方言现成但带 OAuth 4**（`github-copilot / kimi-coding / openrouter / xai`）；**T4 需要新方言 6**（`amazon-bedrock`→bedrock-converse-stream、`azure-openai-responses`、`google-vertex`、`openai-codex`→openai-codex-responses、`opencode`→含 google-generative-ai、`radius`→pi-messages） |
| nini 的 `models.json` 通道**只支持 3 种 api** | `crates/nini-cli/src/provider_factory.rs:424-448`：`openai-completions` / `anthropic-messages` / `openai-responses`，其余报 `No API provider registered for api: {other}` |
| Pi 每家的**模型目录对账做不了** | `references/pi-0.85.1/src/api/*.models.ts` 一律 `import values from "./data/<id>.json"`，而 `src/api/data/` **不在快照里** → 「40 家各带哪些模型」无法逐家核对 |
| nini 自带目录：**112 模型 / 6 provider** | `crates/nini-core/data/model-catalog.json`（`scripts/model-catalog.py` 的 `REACHABLE` 也是这 6 家） |
| 投入单侧化（**实测，本文件写作时**） | `git log --oneline -- crates/nini-ai \| wc -l` = **31**；`--since=2026-10-12` = **7**。`crates/nini-tui` 同口径 = **185 / 92**。源码行数 `crates/nini-ai/src` = **5 553**，`crates/nini-tui/src` = **27 514** |
| 一个薄壳 provider 的成本 | `crates/nini-ai/src/groq.rs` 70 行、`deepseek.rs` 69、`mistral.rs` 73、`cohere.rs` 75、`google.rs` 86 → **约 70–90 行 + `provider_factory` 里 ~10 行分支** |
| `/login` 的已知清单有 10 家，含 nini 没有原生实现的 `together` | `crates/nini-tui/src/commands/dispatch.rs:1026-1037` |

**复算命令**：`python3 - <<'PY'` 扫 `references/pi-0.85.1/src/api/*.ts` 的 `api:` 字段；
`git log --since=2026-10-12 --oneline -- crates/nini-ai | wc -l`。

### 1.2 选项

| 口径 | 要动哪些文件 | 工作量估计 | 后果与风险 |
|---|---|---|---|
| **A 不追平**，把「40」的口径改成「nini 覆盖 9 个原生 + models.json 其余」 | `scripts/parity-gaps.json`（`B9` 的 `nini_state`/`status`）、`docs/GOALS.md` §2 Provider 行 | **2 文件 × ~15 行** | 风险：34 个 Pi 品牌在 nini 里要手写 `models.json`（`api` 只能是那 3 个方言 → T4 那 6 家根本写不出来）；`C3` 的「provider 投入单侧化」继续存在，且没有任何 owner 决策解释它为什么不解决 |
| **B 追平「能追平的那部分」**：只做 T2 的 22 家中**用户真正常用**的若干家（按 Pi 品牌逐个落薄壳），T3/T4 明确登记为不做 | `crates/nini-ai/src/<brand>.rs`（每家 ~70–90 行）、`crates/nini-cli/src/provider_factory.rs`（每家 ~10 行分支）、`crates/nini-core/data/model-catalog.json`（补模型条目）、`docs/GOALS.md` §4 | 每家 **≈ 1.5 文件 × ~90 行**；补 10 家 ≈ **15 文件 × ~900 行**；补满 22 家 ≈ **33 文件 × ~2 000 行** | 风险：①「哪些家常用」需要 owner 给名单，否则就是又一轮自选范围；② `models.json` 通道已能覆盖 T2 的 22 家（只差 baseUrl + key），薄壳的真实增量是**内置默认值与目录条目**，不是「能不能用」——收益比看上去小；③ 仍然差 8 家（T3 的 4 + T4 的 4 + Cloudflare 2），覆盖面叙事会变成「34/40」这种不好解释的数 |
| **C 全追平 40** | 同 B，再加 OAuth 运行时（nini 目前**没有**任何 OAuth 代码）、6 个新方言的协议实现、Cloudflare 的 stream 层 | **不可估**：OAuth 一家 ≥ 1 文件 × ~300 行起，6 个方言按 `openai.rs`(1 524 行)/`anthropic.rs`(568 行)/`sse.rs`(488 行) 的量级，每个 ≈ **1–2 文件 × ~300–600 行** | 风险：与 nini「单文件、无运行时、只读环境变量拿凭据」的产品原则正面冲突（`crates/nini-tui/src/commands/dispatch.rs:1018-1024` 明确写了 nini 不在 TUI 里跑 OAuth）；`R4`（是否自建凭据存储）**本身也还没拍板**，C 与 R4 相互依赖 |

### 1.3 推荐

**推荐 B，但把口径改成「按方言分层，而不是按品牌数追平」**，并请 owner 明确一件事：
**T2 的 22 家，用 `models.json` 覆盖（0 行 Rust 代码）还是做内置薄壳（~90 行/家）？**

理由：

1. **「9 vs 40」这个对比本身是失真的**。40 家里 33 家用的是 nini **已经会说的三种方言**
   （`provider_factory.rs:424-448`），差的只是 baseUrl 和 key——而这两样 `models.json` 就能给。
   真正**语言层面做不到**的只有 **7 家**（T4 的 6 + Cloudflare 的 2 中真正需要专有封装的）。
   把决策建立在「40」这个数上，会推动去做 22 份用户本来就能自己写 30 行配置换来的东西。
2. **成本极不对称**：一个薄壳 ≈ 70–90 行（`groq.rs` 70 行是现成模板），
   而 OAuth/新方言动辄几百行。把「投入单侧化」（C3）解掉的最短路径是**补用户真正常用的那几家薄壳**，
   一次提交就能让 `git log --since=2026-10-12 -- crates/nini-ai` 从 **7** 变成两位数——这正是 C3 的 exit_criterion 要的形状。
3. **C 会把 R4 拖进来**。OAuth 7 家（含 `anthropic` 自己的 OAuth 路径）需要凭据存储，
   而 `docs/GOALS.md` §4 的 R4 至今「待决」。在 R4 没定之前选 C，等于把一个未决问题再套一层。
4. **B 不堵死 A**。若 owner 之后想全追平，B 的成果是 C 的前置子集，不是浪费。

### 1.4 拍板后怎么验收

- **口径可机器复算**：`python3 scripts/parity-gap.py --json` 里 `B9.status` 不再是 `blocked-owner`；
  `B9.nini_state` 里写清**新的口径数字**（例如「nini 原生 N 家 + 可经 models.json 覆盖 M 家 + 明确不做 K 家」），
  并给出**复算该数字的命令**（扫 `provider_factory.rs` 的 match 分支 + 扫 `references/pi-0.85.1/src/api/*.ts` 的 `api:` 字段）。
- **B9 的 exit_criterion 改成可判定**：脚本能算出 nini 支持的 provider 名集合，且 `builtinProviders()` 的 40 被登记进
  `references/pi-0.85.1/SNAPSHOT.lock` 可对账（该 lock 文件已存在，见 `scripts/pi-oracle-lock.py:57`）。
- **C3 的 exit_criterion 改成可判定**：`git log --since=2026-10-12 --oneline -- crates/nini-ai | wc -l` 的**新数字**被写进台账，
  且 nini-ai 的 provider 覆盖面**有 ≥1 轮可计量的提交**。
- 若选 A：`docs/GOALS.md` §2 的 Provider 行与 §4 的 R5 行同步改口径，且 README 明说「不做 Pi 全集」。

### 1.5 如果 owner 什么都不做，代价是什么

- `B9` 与 `C3` **永远停在 `blocked-owner`**，台账的「可机器判定」比例不再上升（当前 56 条里 43 条可判定）。
- `docs/GOALS.md` §2 的 Provider 行继续写「**Pi 0.84.2 内置 40 个**（旧文档写 51，是错的）——R5 待 owner 决策」，
  读者拿到的信息是「nini 差 31 个」，而**其中 22 个 nini 现在就能用**——这是**已知的误导性表述**。
- `C3` 说的「投入单侧化」会随时间**自动恶化**：nini-ai/src 5 553 行 vs nini-tui/src 27 514 行，
  且自 2026-10-12 起 nini-ai 只有 7 次提交。**没有任何 owner 决策说明这是有意的取舍**，
  所以它只是「没人做」，不是「决定不做」。
- 最贵的一条：`models.json` 的 `api` 白名单只有 3 个（`provider_factory.rs:424-448`），
  这意味着 **T4 那 6 家在 nini 里连配置都写不出来**。不拍板就意味着这个能力边界永远不会被写进文档。

---

## 2. R10 —— `/clone` 的语义要不要改成 Pi 那样

**台账**：`B1`（P1）。

### 2.1 事实

| 事实 | 证据 |
|---|---|
| nini 的 `/clone` **只复制文件，不切会话** | `crates/nini-tui/src/commands/dispatch.rs:895-922`：`std::fs::copy` → `<stem>-clone-<ts>.jsonl`，输出「use /resume to switch」 |
| nini 的 `/fork` **已经是 fork-at-leaf 并切会话** | 同文件 `:747-893`：`fork_at` / `fork_at_inclusive`（`crates/nini-session/src/lib.rs:428` / `:434`）→ 写新文件 → `state.session_load(path)`（`crates/nini-tui/src/state.rs:2476`）**切到新 session** |
| Pi 的 `/clone` = fork-at-leaf + 切新 session，**且无参数** | `references/pi-0.85.1/src/ui/interactive-mode.ts:3039` 分发 → `:5186-5208` `handleCloneCommand()` → `runtimeHost.fork(leafId, { position: "at" })` → 提示「Cloned to new session」 |
| nini 的命令表**已经不宣称** `<session-id>` | `crates/nini-tui/src/commands/registry.rs:153-163`（v0.15 的 B1a 已删参数提示） |
| ⚠️ **台账的 exit_criterion 与 Pi 的实际行为冲突** | `scripts/parity-gaps.json` 的 `B1.exit_criterion` 写「cmd_clone **接受 session-id 参数**、走 fork-at-leaf 并切换当前 session」；但 Pi 的 `/clone` **根本没有参数**（`interactive-mode.ts:5186` 的签名 `handleCloneCommand()` 无参） |
| session tree 机制**已具备**，不是空白 | `crates/nini-session/src/tree.rs:121` `get_path_to`；`crates/nini-session/src/lib.rs:620` `get_path`；`crates/nini-core/src/entries.rs:409` `set_parent_id` |

**复算**：`grep -n "fn cmd_clone" -A 30 crates/nini-tui/src/commands/dispatch.rs`。

### 2.2 选项

| 口径 | 要动哪些文件 | 工作量估计 | 后果与风险 |
|---|---|---|---|
| **A 保持现状**：nini 的 `/clone` = 复制当前 JSONL，**不是**分支 | `scripts/parity-gaps.json`（`B1`）、`docs/GOALS.md` §4 | **2 文件 × ~10 行** | 风险：`/clone` 与 `/fork` 在 nini 里是两件不同的事（一个复制文件、一个切分支），而 Pi 的 `/clone` 就是 fork。用户在 Pi 里形成的肌肉记忆在 nini 里不成立；台账要把 `B1` 判为 **fake（产品差异）**，但目前**没有任何文档说明**这层差异 |
| **B 对齐 Pi**：`/clone` = 在当前 leaf 处 fork-at-leaf，**切到新 session**（不带参数） | `crates/nini-tui/src/commands/dispatch.rs`（`cmd_clone` 重写，**复用 `cmd_fork` 已有的 fork + `session_load` 逻辑**，≈ 40–60 行）、`crates/nini-tui/src/commands/registry.rs`（描述 1 行）、`crates/nini-tui/tests/` 新增 1 个测试文件（≈ 60–100 行）、`scripts/parity-gaps.json`（`B1.exit_criterion` 改口径）、`docs/GOALS.md` §4 | **4 文件 × ~130 行** | 风险：**用户可见行为变更**。现有 `/clone` 的产物文件名是 `<stem>-clone-<ts>.jsonl`；改成 fork 后产物名走 `fork_path_for`（`crates/nini-session/src/lib.rs:1232`，格式 `<stamp>_<id>.jsonl`），**已有用户的 clone 文件不会被删，但命名规则变了**；另外 `/clone` 会从「不打断当前会话」变成「切走当前会话」——这是**语义反转**，不是加个参数 |
| **C 折中**：`/clone` 保留复制语义，但**接受可选的 session-id**，复制指定会话 | `dispatch.rs`（≈ 40 行）、`registry.rs`（2 行）、测试 1 文件（≈ 60 行）、台账 + GOALS | **4 文件 × ~110 行** | 风险：这条正是台账 exit_criterion 字面写的口径，但 **Pi 没有这个参数**，做完 nini 与 Pi 仍然不一致，却把差距从「语义」缩小成「多一个没用的参数」——**看起来更接近、实际上更难解释** |

### 2.3 推荐

**推荐 B（对齐 Pi），并同时改掉 `B1.exit_criterion` 里那句与 Pi 冲突的「接受 session-id 参数」。**

理由：

1. **能力已经现成，差的只是接线**。`cmd_fork`（`dispatch.rs:747-893`）已经把「fork-at-leaf → 写新文件 →
   `state.session_load` 切会话」三步做完了；`/clone` 要做的是同一件事，**只是不弹 fork 点列表**。
   这是 `极短路径`：不是新造机制，是把已有机制接到另一个命令上。
2. **Pi 的口径很清楚，且与 nini 的另一处能力一致**。Pi 的 `/clone` 无参数（`interactive-mode.ts:5186`），
   在当前 leaf 处 `position:"at"` 分叉。这与 nini 的 `/fork at N` 的 inclusive 语义
   （`crates/nini-session/src/lib.rs:434`）**是同一个东西**。
3. **台账现在的 exit_criterion 会把人带错**。它写着「接受 session-id 参数」，但 Pi 没有这个参数；
   若照它实现，nini 会得到一个 Pi 没有、也没人用的参数。**这句话本身需要 owner 确认口径**（见 §7 的待确认清单）。
4. **A 的代价不是零**。判 fake 本身是合法的产品决定，但它必须**写下来**（README + GOALS §4），
   否则 `/clone` 的语义差异只能靠用户自己撞出来。选 A 的话，请一并接受「要写文档」这个工作量。

### 2.4 拍板后怎么验收

- `crates/nini-tui/tests/` 下有 **1 条 oracle**：执行 `/clone` 后**当前 session 已切换**，且
  **原分支仍在 session tree 中**（`crates/nini-session/src/tree.rs:121` 的 `get_path_to` 可查）。
- `/clone` 的产物路径符合 `fork_path_for`（`crates/nini-session/src/lib.rs:1232`），不再出现 `-clone-` 后缀。
- `scripts/parity-gaps.json` 的 `B1.exit_criterion` **不再出现「接受 session-id 参数」**（除非 owner 明确要 C）。
- `crates/nini-tui/src/commands/registry.rs` 里 `/clone` 的描述与实际行为一致（不再暗示「用 /resume 切换」）。

### 2.5 如果 owner 什么都不做，代价是什么

- `B1` 永久 `blocked-owner`；它是 **P1**，是台账里优先级最高的未决项之一。
- 更实际的问题：`cmd_clone` 的注释自己写着「The current in-memory transcript keeps editing the original;
  the clone is a side artifact」——**这是有意为之的设计，但没有 owner 背书**。
  下一个来改 `/clone` 的人（或下一个 agent）会看到一段「看起来是临时实现」的注释，
  然后**很可能顺手改成对齐 Pi**——那正是台账禁止实现者自行决定的事。
- `/clone` 与 `/fork` 在 nini 里语义重叠但行为不同（复制 vs 切分支），这个区别目前**只存在于源码注释里**，
  不在任何用户可见文档里。

---

## 3. R11 —— 要不要做 Pi 兼容的扩展运行时

**台账**：`B21`（P2，工具级 executionMode 无声明通道）、`C4`（P2，扩展层能力缺口）。

### 3.1 事实

| 事实 | 证据 |
|---|---|
| nini 的扩展 crate **零调用点** | `crates/nini-ext/` 共 3 文件 **556 行**（`lib.rs` 157 / `loader.rs` 218 / `runtime_api.rs` 181），只出现在 `Cargo.toml:9` 的 workspace 成员列表里；**没有任何 crate 依赖它** |
| nini **明确拒绝** `--extension` | `crates/nini-cli/src/main.rs:564-569`：`refuse("--extension", "the extension runtime was removed from scope: it was specified as a Rust cdylib and Pi loads TypeScript, so no amount of wiring would have connected them")` |
| nini 的 `Tool` trait **有** prompt 贡献、**有** executionMode 默认值，**没有** signal / 流式更新 / 按工具渲染器 | `crates/nini-core/src/tool.rs:206`（trait）、`:247` `execution_mode()` 默认 `Parallel`、`:291` `system_prompt_contribution()`；`ToolContext`（`:151-163`）**只有** `cwd` / `max_output_bytes` / `max_output_lines` |
| Pi 的 `ToolDefinition` **有**那三样 | `references/pi-0.85.1/src/ext/types.ts:451-497`：`promptSnippet`(:459)、`promptGuidelines`(:461)、`constrainedSampling`(:465)、`renderShell`(:467)、`prepareArguments`(:470)、`executionMode`(:479)、`execute(..., signal, onUpdate, ...)`(:481-489)、`renderCall`(:491)、`renderResult`(:494) |
| Pi 的内建工具**一个都没声明** executionMode | 全仓 grep `executionMode` 只有两处：`src/ext/types.ts:479`（类型定义）与 `src/loop/agent-loop.ts:418`（读取处）。**快照内可见的内建工具是 7 个**（`dist/tools-t1/`: bash/edit/read/write；`dist/tools-t2/`: find/grep/ls）；本机同版本安装包另有 `dist/core/tools/powershell.js` → 合计 **8 个**，与 `B21` 的「8 个」一致（口径：安装包） |
| nini 的渲染是**按工具名通用处理**，没有「按工具注册渲染器」的入口 | `crates/nini-tui/src/rich.rs:141` `render_tool_call` / `:243` `render_tool_result` |
| **离线做不了「引 JS 引擎」这条路** | 本机 cargo registry 缓存 915 个 crate，**没有** rquickjs / boa / deno / quickjs 任何一个 → 无法离线构建、无法实测体积 |
| nini 内建工具 7 个 | `crates/nini-tools/src/`：`bash.rs:83` `edit.rs:128` `find.rs:31` `grep.rs:70` `ls.rs:51` `read.rs:55` `write.rs:28` |

### 3.2 选项

| 口径 | 要动哪些文件 | 工作量估计 | 后果与风险 |
|---|---|---|---|
| **A 关账**：不做扩展运行时，把 `B21`/`C4` 判为「产品原则差异」；按 `C4.exit_criterion` 的要求，**README 声明「扩展工具不在 parity 范围内」** | `README.md`（声明 1–3 行）、`scripts/parity-gaps.json`（`B21`/`C4` 的 `status` + `nini_state` + `exit_criterion`）、`docs/GOALS.md` §4 | **3 文件 × ~20 行** | 风险：`nini-ext` 556 行继续以「零调用点」的状态留在 workspace 里，`cargo build` 每次都要编它；**要么留着并在 crate 头写明「不可用」**（现状已有，`:3` 的 `Status: NOT WIRED`），**要么删掉**（再减 556 行，但丢掉了「曾经试过」的记录）——这本身是个 owner 口味题 |
| **B 只补「工具能力」，不补「运行时」**：给 `Tool` 加三样 Pi 有的东西——① 按工具注册渲染器 ② 执行期流式更新回调 ③ signal/取消；**不引入任何第三方扩展的装载机制** | `crates/nini-core/src/tool.rs`（`ToolContext` 加 signal + update 回调、`Tool` 加渲染钩子，≈ 100–140 行）、`crates/nini-core/src/agent.rs`（执行期接线 ≈ 40–60 行）、`crates/nini-tui/src/rich.rs`（渲染器注册表 + 分发 ≈ 60–100 行）、`crates/nini-tui/tests/` 新增 1 文件（≈ 80–120 行） | **4 文件 × ~300 行** | 风险：**B21 仍然关不掉**。`B21` 要的是「用户能声明 executionMode 的入口」——**没有扩展运行时，就没有任何人能声明**，补了工具能力也只是一个「将来会有用」的接口。收益全部落在 `C4` 的 ①②③ 上（那 3 项是**给 nini 自己的工具**用的，不是给扩展用的） |
| **C 做真扩展运行时**（让第三方扩展能注册工具） | 需要一个 JS 运行时（QuickJS 类）或一个进程外协议；再加一个能加载 Pi 的 `.ts` 扩展的加载器 | **不可估**：`rquickjs`/`boa` 本地**不可得**（见 §3.1），体积与工作量需联网才能测；即便能引，还需要「Pi 的扩展 API（28 类运行时事件、TypeBox schema、ExtensionContext）」到 Rust 的映射层 | 风险：与 `docs/GOALS.md` §4 的 **R6 已被证伪**的结论**正面冲突**——R6 的原文理由是「Pi 加载 TS，`nini-ext` 是 `dlopen` cdylib，无共同执行模型」。**选 C 等于重开 R6**，需要 owner 明确推翻它。同时 nini 的「单文件、无运行时」卖点会被打破 |

### 3.3 推荐

**推荐 A（关账 + README 声明）。** 若 owner 同时希望内部工具能力更强，**B 可以单独立项**，但它**不是 R11 的必要条件**。

理由：

1. **R6 已经用源码证伪过一次**。Pi 的扩展加载是 `jiti` 加载 `.ts`（`src/ext/types.ts` 的 `ToolDefinition` 是 TS 接口），
   nini 的 `nini-ext` 是 `libloading` + C ABI 符号（`crates/nini-ext/src/lib.rs` 头注释有完整对照表）。
   **没有共同的执行模型**——这不是「工作量问题」，是「能不能”的问题。
2. **B21 的 exit_criterion 自己承认「机器判定: false（nini 没有运行时扩展系统）」**。
   一条**没有任何 oracle、也没有承载主体**的差距，继续挂着只会稀释台账。
   A 的价值恰恰是**把这条明确关掉，并留下 README 声明**——`C4.exit_criterion` 的前半句已经这么要求了：
   「在此之前 README 必须声明『扩展工具不在 parity 范围内』」。
3. **B 的收益是真的，但归属该另开条目**。`C4` 的三项（按工具注册渲染器 / 流式更新 / signal）确实是真差距，
   它们对 **nini 自己的 7 个工具**也有价值（比如 bash 长命令想流式回显、想给 read 一个专属渲染器）。
   把它们混进 R11，会让 R11 变成一个「什么都沾一点」的包，验收判据写不出来。
   **建议：R11 选 A；把 `C4` 拆成 C4a（工具能力三项，走 B 的口径）与 C4b（扩展工具不在 parity 范围，随 A 关掉）。**
4. **C 的代价是推翻一个已被证据关掉的决策**。如果 owner 认为该重开 R6，那需要新的证据
   （例如：Pi 扩展生态的实际使用规模、或一个不依赖 JS 引擎的执行模型提案），
   而不是「先做了再说」。

### 3.4 拍板后怎么验收

- `README.md` 有一句明确的「扩展工具不在 parity 范围内 / nini 不加载第三方扩展」——
  **按文本断言**（一条测试读 README 找这句话），不靠「没接所以没用」这种隐式状态。
- `scripts/parity-gaps.json`：`B21.status` 与 `C4.status` 不再是 `blocked-owner`；
  `C4.exit_criterion` 改成上面那句 README 声明（可 grep）。
- `crates/nini-ext` 要么**删掉**（`crates/nini-ext/` 整目录 + `Cargo.toml:9` 一行），
  要么**留下但加门禁**：一条测试断言「workspace 里没有任何 crate 依赖 `nini-ext`」
  （现状成立：`grep -rn "nini_ext" crates --include=*.rs` 除 `crates/nini-ext` 外无命中）。
  **二选一必须写进拍板结论**——「留着」也是一个决定，不写就等于没人决定。
- 若 owner 另立 C4a（工具能力三项）：判据落在 `crates/nini-tui/tests/`——
  「注册一个按工具定制的渲染器后，该工具的调用行与结果行用的是它自己的样式」
  「工具执行中收到 signal 时立刻中止并返回错误」。

### 3.5 如果 owner 什么都不做，代价是什么

- `B21` 与 `C4` 两条 P2 永久 `blocked-owner`；台账里 `C` 类（成本/能力面）会一直有一条不可判定的尾巴。
- **README 不声明** → 用户按 Pi 的经验装一个扩展，nini 静默忽略；`--extension` 会打印一段解释
  （`main.rs:564-569`）但那要用户**先知道有这个 flag**。
- `nini-ext` 556 行继续以「零调用点」状态留在 workspace：每次全量构建都编它，
  而 `cargo build` 的产物里带着一个**永远不会被调用**的扩展加载器。
- 还有一个隐性成本：`B21` 的 `exit_criterion` 说「将来若要支持，先要有一个能注册工具的运行时入口（R11）」——
  **R11 不拍板，这条永远是「将来」**。而 `C4` 的三项工具能力是**当下就能用**的，
  被捆在一个「将来」里，就一直没有单独排期的理由。

---

## 4. R12 —— mermaid 要不要画真图（引不引布局引擎）

**台账**：`B22`（P3）。

### 4.1 事实

| 事实 | 证据 |
|---|---|
| nini 现在画的是**降级占位**（不是图） | `crates/nini-tui/src/mermaid.rs:188` `render_mermaid_block`（整文件 **278 行**）：画一个「`◇ mermaid · <kind>` + 源码逐行保留」的框 |
| nini 的接线点在哪 | `crates/nini-tui/src/markdown.rs:70` `render_markdown_with_mermaid` |
| nini **没有** `markdown.mermaid` 这个 settings 键 | `crates/nini-tui/src/settings.rs:33` 的 `Settings` 结构体里没有 `markdown` 字段 |
| Pi 的键与取值 | `references/pi-0.85.1/src/core/settings-manager.ts:62` `type MermaidRenderingMode = "off" \| "final" \| "streaming"`；`:66` `markdown?: { mermaid?: MermaidRenderingMode }`（默认 `streaming`，见 `:1350` `getMermaidRenderingMode`）；`:1355` 写回 |
| Pi 的接线 | `references/pi-0.85.1/src/ui/interactive-mode.ts:131` import、`:439` 构造、`:2025` 放进 markdown transformer 链 |
| ⚠️ **Pi 的画图实现不在快照里，但本机同版本安装包里有** | **来源是安装包不是快照**：本机 `…/@earendil-works/pi-coding-agent/dist/modes/interactive/components/mermaid.js`（**74 行**）`import { render } from "grok-mermaid"`；其 `package.json:60` 依赖 `"grok-mermaid": "0.2.2"` |
| 那个布局引擎是什么 | `grok-mermaid` 0.2.2：**纯 TS 自带布局引擎**，零运行时依赖；`src/` 共 **4 546 行 TS**（`parse.ts` 1 150 / `layout.ts` 1 015 / `width-data.ts` 993 / `canvas.ts` 373 / `labels.ts` 326 / `layout-seq.ts` 203 / `graph.ts` 142 / `source-box.ts` 89 / `width.ts` 74 / `ansi.ts` 34）；`dist/` **346 760 B**（含 `.d.ts` 与 sourcemap），磁盘 **644 KB**；支持 **flowchart / state / class / er / sequence** 五类（`src/parse.ts:88`） |
| **Rust 原版存在，但本机与仓库都拿不到** | `grok-mermaid` 的 README 自述它是 `xai-org/grok-build` 里 `crates/codegen/xai-grok-markdown/src/mermaid.rs` 的 **TS 移植** → **Rust 原版存在，但不在本仓库、不在本机**（未查到，需 owner 提供或允许联网） |
| Rust 侧引入体积**无法离线实测** | 本机 cargo registry 缓存 915 个 crate，**没有**任何 mermaid / graph / layout 相关 crate |
| Pi 的画图行为细节（安装包） | `mermaid.js`：`mode==="off"` 或消息类型是 `assistant-thinking` → **原样返回 markdown**；流式期间 `mode!=="streaming"` → 也原样返回；画出的图 `art.width > context.availableWidth` → **回退成源码**；非流式且有 warnings → 追加一行告警文本 |

### 4.2 选项

| 口径 | 要动哪些文件 | 工作量估计 | 后果与风险 |
|---|---|---|---|
| **A 只接 settings 键，不画真图**：加 `markdown.mermaid`（`off`/`final`/`streaming`，默认 `streaming`），三种取值真的改变渲染结果（`off` → 不画占位框，直接显示源码） | `crates/nini-tui/src/settings.rs`（字段 + 解析，≈ 40–60 行）、`crates/nini-tui/src/markdown.rs`（按 mode 分发，≈ 30–50 行）、接线 1 处（≈ 20–30 行）、`crates/nini-tui/tests/` 新增 1 文件（≈ 60–100 行） | **4 文件 × ~180 行** | 风险：`off` 之外的两个取值在 nini 里**没有可观测差异**（nini 只有占位框，没有真图可随流式重绘）→ 键接上了，语义却是空的；`B22` 只能关掉「模式未接到 settings 键」这**一半**，另一半（真图）还在。而且 `final` 与 `streaming` 在 nini 里必然同义，这个「同义」要写进文档，否则又是一处口径失真 |
| **B 自己写一个 Rust 布局引擎** | 新增 `crates/nini-tui/src/mermaid_layout.rs`（或独立 crate）+ 解析 + 画布 + 标签测量 | **量级参考**：`grok-mermaid` 的 TS 版是 **4 546 行**（含 width-data 993 行的字符宽度表）；一个 Rust 移植或等价实现 **≥ 2 000 行，且质量对不齐**。体积 **未实测**（本地无可参考 crate） | 风险：**这是 R9 量级的另一个「体积未知」**，而这次连 TS 原版的**对照物都在**（可以逐功能对齐）；但也意味着**没有捷径**——手写一个能画 flowchart 的最小实现可能只要 600–800 行，代价是**和 Pi 的图长得不一样**（Pi 画的是 Unicode 方框艺术，不是 ASCII art） |
| **C 移植上游 Rust 原版**（`xai-org/grok-build` 的 `crates/codegen/xai-grok-markdown/src/mermaid.rs`） | 新增一个 mermaid 渲染模块（可整段移植或作为独立 crate vendor 进来） | **不可估**：源码**本机不可得**（未查到）；体积不可实测（本地 cargo registry 无候选） | 风险：**需要 owner 提供源码或放行联网**；`xai-org/grok-build` 的**许可未查**（未查到，需 owner 确认）——vendoring 第三方代码必须先确认许可。另外 Pi 自己也是通过 npm 依赖 `grok-mermaid`（TS 版）实现 C 的，**不是**直接用 Rust 原版，所以「对齐 Pi」在 C 下反而要**再移植一层** |

### 4.3 推荐

**推荐 A，但把 A 的定位说清楚：它只关掉「模式未接到 settings 键」，不关掉「真图渲染」。**
若 owner 想要真图，则走 **C**（移植上游 Rust 原版），**且需要 owner 先提供源码并确认许可**。

理由：

1. **A 的成本是 B/C 的零头，而它本身是一个真实的口径修复**。nini 的 `Settings` 里根本没有 `markdown` 段
   （`settings.rs:33`），Pi 有（`settings-manager.ts:66`）——这是一处**可核实的键表缺失**，补上它成本约 180 行。
2. **真图这件事的证据链刚刚才补齐**。此前台账写「Pi 的渲染实现无法从快照验证」；现在查清了：
   Pi 用的是 npm 包 `grok-mermaid` 0.2.2（安装包 `package.json:60`），它是一个**纯 TS、自带布局引擎**的实现。
   **这个发现改变了决策的形状**：不是「有没有 Pi 的实现可以对齐」，而是「**有一个明确的第三方实现可以参考或移植**」。
3. **C 是最接近 Pi 的路，但有一个硬前提**：Rust 原版不在本机（未查到），**许可未查**。
   在 owner 交出这两样之前，C 的工作量与体积都是**未知数**——不能拿它当承诺。
4. **B 不推荐**。手写一个「能画 flowchart」的 Rust 引擎，产出的是**另一种东西**：
   Pi 画的是 Unicode 方框艺术（`grok-mermaid` 有专门的 `width-data.ts` 993 行字符宽度表和 `ansi.ts`），
   自研版本会**看起来不一样**，而 nini 的定位是「Pi 的原味」。
   若只求「有图」，B 更便宜；若求「像 Pi」，B 注定对不齐。
5. **A 里 `final` 与 `streaming` 同义这件事必须写进文档**（见 §4.4），否则接完键就制造了一处新的口径失真。

### 4.4 拍板后怎么验收

- `crates/nini-tui/tests/` 下有 **1 条 oracle**：`settings.json` 的 `markdown.mermaid` 取三个不同值时，
  **渲染结果真的不同**（至少 `off` 与非 `off` 必须不同）。
- 若选 A：文档里写明「nini **不画真图**，mermaid 块始终是降级占位框；`markdown.mermaid` 的
  `final` 与 `streaming` 在 nini 里同义」。这条必须能被 grep 到。
- 若选 C：判据落在 `crates/nini-tui/tests/`——**一个 mermaid 块在转录里被画成图（不是占位块）**，
  且该图的形状与 `grok-mermaid` 对同一输入的输出**逐行一致**（可以拿安装包的 TS 实现做对照物，
  参照 `docs/plans/r9-syntax-highlighting.md` 的实测方法）。
- 若选 B：判据降级为「能画出图」，但**必须同时记录「与 Pi 的图不一致」**，
  并把它写进 `scripts/parity-gaps.json` 的 `B22.nini_state`（否则就是一次新的口径失真）。
- 无论选哪个：`B22` 的 `status` 不再是 `blocked-owner`；其 `exit_criterion` 里「等 owner 决策 R12」这句删掉。

### 4.5 如果 owner 什么都不做，代价是什么

- `B22` 永久 `blocked-owner`；台账里唯一一条**用户输入就能看见**的未决项（模型输出里带 mermaid 的用户会看到占位框）。
- nini 的 `settings.json` **永远没有 `markdown` 段**。用户从 Pi 抄一份 `settings.json` 过来，
  `markdown.mermaid` 被 serde 静默忽略（`Settings` 没有该字段，`#[serde(default)]` 之外是**直接丢弃**）——
  **用户改了设置却看不到任何变化，也看不到任何提示**。
- 那个刚刚查清的重要事实会**烂在这个文件里**：「Pi 的 mermaid 是 npm 包 `grok-mermaid`，Rust 原版在 `xai-org/grok-build`」。
  下一个人查 R12 时要**重新查一遍**（本机安装包路径、`grok-mermaid` 的 README、许可）——
  **信息已经查到，却没被沉淀成台账或决策，这就是白查**。
- 更实际的：`grok-mermaid` 的**许可与上游 Rust 仓库的可达性**是 owner 才能给的两样东西。
  不拍板 → 这两样永远不会被要到 → C 这条路永远停在门口。

---

## 5. R13 —— 主题槽怎么接线（哪些接、哪些清、哪些不接）

**台账**：`B24`（P3）。

### 5.1 事实

| 事实 | 证据 |
|---|---|
| nini 的主题槽共 **61 个** | `crates/nini-tui/src/theme.rs:131` `COLOR_NAMES` |
| **`COLOR_NAMES` 同时是主题文件写回的过滤器** | `crates/nini-tui/src/theme.rs:349` `colors.retain(\|k, _\| COLOR_NAMES.contains(&k.as_str()))` → **删任何槽都会静默丢弃用户 `theme.json` 里的对应值** |
| 与 Pi 的主题键表对比 | 本机同版本安装包 `dist/modes/interactive/theme/dark.json` 的 `colors` 共 **56** 键；nini **独有 6 个**：`background / code / foreground / header / info / link`；Pi **独有 1 个**：`scrollbarTrack`（61 = 56 − 1 + 6） |
| **零读取槽 = 27 个**（生产代码里没有任何槽名读取点） | **复算口径（2026-11-05，可复算）**：一个槽算「零读取」，当且仅当它的名字作为字符串字面量**在 `crates/nini-tui/src/**` 的生产代码里一次都不出现**（`theme.rs` 自身除外；**注释与 `#[cfg(test)]` 块都已剔除**——测试里断言一个字符串不等于生产代码读了这个槽）。命令见台账 `B24.nini_state`（同一段代码）。结果：**61 个槽，27 个零读取** = **9 个 `syntax*`**（B7 管）+ **18 个本条记的**：**7 个 `thinking*`** + **3 个 `customMessage*`** + **2 个**（`border`、`borderAccent`）+ **6 个 legacy**（`background / code / foreground / header / info / link`） |
| ✅ **台账 `B24` 的数字已在本轮对齐**（原先只有标题错） | 原先 `B24.title` 写「**16 个**槽定义了却读不到」，而 `nini_state` 的「**27 个**零读取」与分项 ①thinking×7 ②customMessage×3 ③border/borderAccent×2 ④legacy×6 = **18**（27 − 9 = 18）**本来就自洽**——错的只有标题那一个数字。2026-11-05 已按上面写明的口径复算：**27 零读 = 9 + 18**，台账标题改为「18 个」，并把口径与复算命令写进 `B24.nini_state`（此前口径只存在于读者脑子里，于是同一个问题被三个地方用三套算法数出三个数） |
| nini **已经有**边框 UI（输入框外框），可接 thinking 档位 | `crates/nini-tui/src/render.rs:1550-1600`：输入框外框颜色取 `theme.fg_style(if state.is_bash_mode() { "bashMode" } else { "borderMuted" })`（`:1570` 附近）→ **换档位色不需要新造 UI**。Pi 侧同理：`interactive-mode.js:1659` / `:3423` 把 thinking 档位接到 `editor.borderColor` |
| Pi 侧这些槽**都有真实读取点** | 安装包：`theme.js:242` `getThinkingBorderColor(level)` 按档取色 → `interactive-mode.js:1659/3423` 接编辑器边框；`customMessageBg/Text/Label` 在 `components/branch-summary-message.js:13/28/34`、`components/compaction-summary-message.js:13/29`；`border` 在 `components/dynamic-border.js:11`、`components/bordered-loader.js:12`、`interactive-mode.js:4985`；`borderAccent` 在 `components/tree-selector.js:673` |
| `keyHint` 那个「读了不存在的槽」的缺陷**已修** | `crates/nini-tui/src/render.rs:1458-1466`（注释写明 v0.16 之前读的是 `keyHint`，落到 `default_color` = 白+粗体）；现读 `dim`；有颜色级断言 `crates/nini-tui/tests/welcome_card.rs:128` |
| `customMessage` 在 nini **有数据层、无渲染层** | 数据层：`crates/nini-core/src/entries.rs:281` `CustomMessageEntry`、`:145` `CustomMessage`；渲染层：转录只处理 User/Assistant/ToolResult/BashExecution 四类，**没有** customMessage 的渲染入口（`crates/nini-tui/src/selectors/tree.rs:62` 只是提到） |
| legacy×6 现在**已经**没有任何用户可见效果 | 它们已被 `theme.rs:349` 的 `retain` 静默丢弃 → **删掉它们对用户零可见变化**，真正缺的是「显式提示」 |

### 5.2 选项

| 口径 | 要动哪些文件 | 工作量估计 | 后果与风险 |
|---|---|---|---|
| **A 只接 thinking×7**（按当前 thinking 档位给**已有的输入框外框**上色） | `crates/nini-tui/src/theme.rs`（档位→槽名映射 ≈ 10–20 行）、`crates/nini-tui/src/render.rs`（边框取色改一行分支 ≈ 10–20 行）、`crates/nini-tui/tests/` 新增 1 文件（≈ 50–80 行） | **3 文件 × ~100 行** | 风险：nini 的外框是**输入框**，Pi 的是**编辑器边框**——位置语义相近但不完全等价；`B24` 的 `nini_state` 说「nini 没有按档位着色的地方，接上得先有『要不要边框 UI』的产品决定」——**这个前提在 v0.16 已不成立**（边框 UI 已在 `render.rs:1550-1600`），选 A 等于**替 owner 回答了那个已经不成立的问题** |
| **B 接 thinking×7 + customMessage×3** | A 的 3 文件，再加 `crates/nini-tui/src/render.rs`（customMessage 渲染入口 ≈ 40–70 行）+ `crates/nini-core/src/entries.rs`（自定义消息的产生者 ≈ 30–60 行）+ 测试 | **5 文件 × ~250 行** | 风险：**nini 目前没有任何东西会产生 customMessage**。Pi 的 customMessage 来自 branch summary / compaction summary / skill invocation（安装包 `components/` 下三个文件）。nini 的 `/fork` 与压缩流程**要不要发一条自定义消息**，是产品决定；而且这条通道**被 R11 卡着**（扩展工具不发自定义消息，但内建流程可以发）——**先做 R11 的拍板更省事** |
| **C 清理 legacy×6 + 其余悬空槽**，并且**把静默丢弃改成显式提示** | `crates/nini-tui/src/theme.rs`（删槽 + `retain` 改成「未知键 → 收集起来 → 提示一次」，≈ 50–80 行）、`crates/nini-tui/tests/`（≈ 60–100 行） | **2 文件 × ~150 行** | 风险：**删槽会影响老用户的 `theme.json`**——正是 `theme.rs:349` 注释警告的那件事。若不做「显式提示」，删槽就是把用户配置**静默扔掉**（现状），若做了提示就是一次**用户可见的行为变更**（启动时多一行提示）。另外 legacy×6 现在**已经零可见效果**，删它们的**收益接近零**，成本却不为零 |
| **D 一个都不动** | 只改台账：`scripts/parity-gaps.json`（`B24`）+ `docs/GOALS.md` §4 | **2 文件 × ~15 行** | 风险：27 个悬空槽继续占着主题键表的位置；**未来每个新写的槽都会让人怀疑「这个是不是也没接线」**；而且 `COLOR_NAMES` 是写回过滤器——**槽表与实际可用槽长期不一致，等于一份会骗人的清单** |

### 5.3 推荐

**推荐 A（只接 thinking×7），并把 C 里的「显式提示」那一半单独拿出来做（不动删槽）。**

理由：

1. **thinking×7 是投入产出比最高的一项，且前提已经具备**。
   Pi 侧这 7 个槽的作用是「按 thinking 档位给编辑器边框上色」（安装包 `theme.js:242` + `interactive-mode.js:1659/3423`）；
   nini 侧**已经有输入框外框**（`render.rs:1550-1600`）**和一模一样的档位表**
   （`crates/nini-core/src/provider.rs:169` 的 `ALL = [off, minimal, low, medium, high, xhigh, max]`，与 Pi 一致）。
   差的就是「按档位换颜色」这一行分支。
2. **customMessage×3 现在接不了**。不是技术问题，是**没有生产者**：nini 没有任何流程会生成自定义消息。
   要接就得先决定「哪些流程发、发什么文案」——这是产品问题，不是接线问题。
   而且它和 R11 的话题重叠（Pi 的 customMessage 大量来自扩展），**先把 R11 定了再回来更省事**。
3. **legacy×6 建议**保留槽位、**但把静默丢弃改成显式提示**。
   理由：这 6 个槽现在**已经零可见效果**（被 `retain` 丢掉），删它们**省不下任何运行时**（它们只是字符串常量），
   却会让老用户的 `theme.json` 少几个键。**真正该修的是「静默」**：用户在自己的 `theme.json` 里写了 `foreground`，
   nini 悄悄扔掉且一声不吭——这是**一次未被承认的信息丢失**。
4. **`B24` 的 exit_criterion 说「接线前没有可断言的行为」**——这句话现在**部分过期**了：
   `thinking×7` 一旦接上，就**立刻有**可断言的行为（边框颜色随档位变），所以选 A 之后 `B24` 的判据可以写得很实。

### 5.4 拍板后怎么验收

- `crates/nini-tui/tests/` 下：**每个被接上的槽至少一条断言**（颜色或文本）。
  具体到 A：`thinkingOff` → `thinkingMax` 七档，输入框外框的颜色**各不相同**（可以断言到 `theme.fg_style("thinkingMax")` 之类的具体值）。
- `scripts/parity-gaps.json` 的 `B24.exit_criterion` 改成上面那条可判定的判据；`status` 不再是 `blocked-owner`。
- **legacy×6 若决定不删**：判据是「用户 `theme.json` 里的这 6 个键被**显式忽略并有提示**，而不是静默丢弃」
  （注意 `B24.exit_criterion` 原文已经这么写了——它预设的是「若决定删除」这条分支）。
- `docs/GOALS.md` §4 补上 R13 一行（**现状缺失**）。

### 5.5 如果 owner 什么都不做，代价是什么

- `B24` 永久 `blocked-owner`。它是 **P3**，但它是台账里**唯一一条「文档与实现不一致」的差距**：
  主题键表 61 个槽，**27 个没有任何读取点**——而主题键表是**用户会照着改的文件**（`theme.json` 的键名就是这 61 个）。
  用户改了 `thinkingHigh`，什么都不会发生，也不会有任何提示。
- 更实际的风险是**下一轮主题工作会踩坑**：任何人（包括 agent）看到 `COLOR_NAMES` 里 27 个槽没有读取点，
  会以为「这些槽是实现了但没接」或「这是待办」，而**两种理解都错**
  （thinking 是 Pi 有真实用途的，customMessage 是**没有生产者**的，legacy 是**已死的**）。
- `B24` 的 exit_criterion 里那句「接线前没有可断言的行为」会**继续误导**：它让人以为「接不上」，
  而实际是「thinking 现在就能接、只是没人拍板」。
- 台账数字失真的代价会扩散：`B24.title` 曾写「16 个」，而正文是「27 个零读取」、分项是 18（27 − 9 = 18）。**根因不是算错，是判据没写口径**——同一个问题被三个地方用三套算法数，读者无法复算。2026-11-05 已把口径与复算命令写进 `B24.nini_state`，标题改为「18 个」。
  **三处数字不一致**，且都在一份「决策台账」里。这与 `docs/GOALS.md` 反复强调的「手工数字会过期」是同一种病。

---

## 6. R14 —— 键位语义要不要跟 Pi 对齐

**台账**：`B25`（P2）。

### 6.1 事实

| 事实 | 证据 |
|---|---|
| nini 的默认键表 **40 条绑定 / 38 个动作** | `crates/nini-tui/src/keys.rs:226` `default_keymap()`（逐条数 `KeyBinding {` = 40；`action:` 去重 = 38；`Abort` 与 `ScrollToBottom` 各绑 2 次） |
| Pi 的键位 id 总量与 nini 的差距 | `src/ui/keybindings.ts` 的 `KEYBINDINGS` = **47** 个 id；`src/tui/keybindings.ts` 的 `TUI_KEYBINDINGS` = **47** 个 id；两者重叠 4 个 → 合并 **90** 个 id，其中 **80** 个有默认键（`scripts/keymap-parity.py` 的输出口径）。⚠️ 台账 `B25.pi_basis` 写「KEYBINDINGS（**90** 个 binding id）+ TUI_KEYBINDINGS（47）」——**90 是合并去重后的数**，单看 `KEYBINDINGS` 是 47（口径需订正） |
| 机器判定已经跑通 | `python3 scripts/keymap-parity.py` 退出码 **0**，输出：「Pi 有默认键的 binding id：80；已登记 80（map 46 + exempt 34）」、「Pi 用户可见的 /hotkeys 行：40；nini 键位行：40」 |
| **3 处语义分叉**（已登记，**不判红**） | `scripts/keymap-parity.json` 的 `divergences` 段（3 条）：<br>① `tui.editor.deleteToLineStart`：Pi=`delete-to-start`（Ctrl+U 删到行首），nini=`clear-input`（清空整个输入框）——**nini 里有 `KillToLineStart` 动作（`crates/nini-tui/src/keys.rs:140`）但没绑定**<br>② `app.clear`：Pi=`clear`（Ctrl+C 清空编辑器），nini=`interrupt`（Ctrl+C 中止回合；nini 退出走 Ctrl+D，`keys.rs:257`）<br>③ `app.thinking.toggle`：Pi=`thinking-toggle`（Ctrl+T 显隐 thinking 块），nini=`thinking-cycle`（Ctrl+T 提升 thinking 档位）——**nini 没有「显隐 thinking 块」这个动作** |
| **5 组「Pi 有、nini 无」= 6 个 binding id** | `scripts/keymap-parity.json` 的 `map` 段里 `nini: null` 的条目**实为 6 条**：`tui.editor.jumpForward`、`tui.editor.jumpBackward`、`tui.input.copy`、`app.suspend`、`app.message.copy`、`app.message.dequeue`。⚠️ 台账正文写「**5 个**」——按**功能组**数是 5（jumpForward+jumpBackward 算一组），按 **binding id** 数是 6。口径需统一 |
| Pi 侧这 6 个的默认键 | `app.suspend`=Ctrl+Z（`src/ui/keybindings.ts:96`）、`app.message.copy`=Ctrl+X（`:130`）、`app.message.dequeue`=Alt+Up（`:138`）、`tui.editor.jumpForward`=Ctrl+] / `jumpBackward`=Ctrl+Alt+]（`src/tui/keybindings.ts:106/110`）、`tui.input.copy`=Ctrl+C（`:146`） |
| nini 的剪贴板**只有写、没有读** | `crates/nini-tui/src/clipboard.rs` 只有 `copy()`（`:60`，走 `arboard`），无读接口 → `app.message.copy` / `tui.input.copy` 需要新增读能力 |
| 已钉住的一致部分 | `Home`/`End` = 滚到转录顶/底；行首/行尾 = `Ctrl+A`/`Ctrl+E`（Pi 是 `Ctrl+Home`/`Ctrl+End`）；按词移动两边都是 `Ctrl+Left`；撤销 Pi=`Ctrl+-`、nini=`Ctrl+/`（**唯一一处纯键名差异**） |

**复算**：`python3 scripts/keymap-parity.py`（退出码 0）；`python3 -c "import re;print(len(re.findall(r'action: ',open('crates/nini-tui/src/keys.rs').read()))))"`。

### 6.2 选项

| 口径 | 要动哪些文件 | 工作量估计 | 后果与风险 |
|---|---|---|---|
| **A 全对齐**：3 处分叉改语义 + 补 6 个缺失 binding | `crates/nini-tui/src/keys.rs`（键表：Ctrl+U 改绑已有的 `KillToLineStart` ≈ 5 行；补 6 个键 ≈ 30 行）、`crates/nini-tui/src/commands/dispatch.rs`（新增动作：thinking 显隐、suspend、message copy、message dequeue、input copy、jump×2 ≈ 120–200 行）、`crates/nini-tui/src/clipboard.rs`（**读**能力 ≈ 40–80 行）、`crates/nini-tui/src/state.rs`（输入框选区支持 ≈ 60–120 行，`tui.input.copy` 需要「选区」这个概念，nini 目前没有）、`scripts/keymap-parity.json`（map/divergences 改写）、测试 1–2 文件 | **6 文件 × ~400 行** | 风险：① **全是用户可见的键位变更**——Ctrl+U 从「清空输入框」变成「删到行首」，Ctrl+C 从「中止回合」变成「清空编辑器」（**这是 Pi 的行为**：先清空，再按才退出），Ctrl+T 从「提升档位」变成「显隐 thinking 块」；② `Ctrl+C` 的变更**影响最大**：nini 用户按 Ctrl+C 中止回合是高频操作，改成「先清空」会让人以为程序没反应；③ `app.suspend`（Ctrl+Z 挂起）需要真的把进程挂到后台并能恢复，nini 现在**没有这个能力**（`runtime.rs:1760` 的 suspend 是「挂起 TUI 去跑外部编辑器」，不是挂起进程） |
| **B 只对齐 3 处分叉，不补缺失的 6 个** | `crates/nini-tui/src/keys.rs`（Ctrl+U 改绑 ≈ 5 行）、`crates/nini-tui/src/commands/dispatch.rs`（新增「显隐 thinking 块」动作 ≈ 60–100 行；Ctrl+C 语义 ≈ 30–50 行）、`crates/nini-tui/src/state.rs`（thinking 块的显示开关 ≈ 30–60 行）、`scripts/keymap-parity.json`（3 条 divergence 清空）、测试 1 文件 | **4 文件 × ~180 行** | 风险：Ctrl+C 的语义变更仍在（**中止回合 → 清空编辑器**），这一条建议**单独考虑**：nini 的 `Abort` 也管 Esc，双绑定到 Ctrl+C 会让「中止」只剩 Esc 一个键；且 Pi 自己的 `app.clear` 是「先清空，第二次才退出」，nini 的 `Quit` 走 Ctrl+D —— 两边的「退出」手势不同，**照抄 Pi 需要把 Ctrl+D 也一起想清楚** |
| **C 只做「零争议的那一处」**：Ctrl+U 改绑到**已经存在但没绑定**的 `KillToLineStart`（`keys.rs:140`），其余 2 处分叉与 6 个缺失**全部登记为「有意差异」并写明理由** | `crates/nini-tui/src/keys.rs`（≈ 5 行）、`scripts/keymap-parity.json`（`divergences` 里 2 条写理由 + `map` 里 6 条 `nini: null` 写 `note`）、`docs/GOALS.md` §4 | **3 文件 × ~40 行** | 风险：`B25` 的 exit_criterion 说「要么按 Pi 改，要么在 `divergences` 里写明理由」——**C 完全满足判据**；但「有意差异」若不写理由就等于没写，所以**理由必须由 owner 给**，不能由实现者编 |
| **D 一个都不改** | 只改台账 | **2 文件 × ~10 行** | 风险：`B25` 是**唯一一条 `machine_checkable: true` 却被 `blocked-owner` 卡住**的差距——它的机器判据（两个脚本都退出 0）**已经达成**，卡住的是「语义要不要对齐」这个产品问题。挂着不动 = 台账里唯一的「判据已达成但状态未推进」的条目 |

### 6.3 推荐

**推荐 C（先做 Ctrl+U 那一条，其余登记理由），并请 owner 对 3 处分叉逐条给结论。**

理由：

1. **Ctrl+U 这一处是「零争议、零新概念」**。nini **已经有** `KillToLineStart` 这个动作
   （`crates/nini-tui/src/keys.rs:140`），只是没绑键；Pi 是 Ctrl+U 删到行首。**改绑 5 行**，
   语义完全对齐，且 nini 现有的「清空输入框」另有归属。
2. **另外 2 处分叉不是「改键」，是「改 nini 的交互模型」**。
   Ctrl+C：nini 用它中止回合，Pi 用它清空编辑器——**两者不能同时成立**；
   选 Pi 就要把「中止」挪到别处（Pi 是 Esc），并把「退出」手势一起重新想（nini 现在是 Ctrl+D）。
   Ctrl+T：nini 的 thinking 是**档位**（切换影响请求参数），Pi 的是**显隐**（只影响显示）。
   这两条都需要 owner 说清「nini 要 Pi 的哪一种」，**不该由实现者代选**。
3. **缺的 6 个 binding id 里有 3 个需要新能力，不是新绑定**：
   `app.suspend` 需要进程级挂起/恢复（nini 现在**没有**）；`app.message.copy` + `tui.input.copy` 需要
   **剪贴板读**（`clipboard.rs` 只有写）与**输入框选区**（nini 没有这个概念）。
   这些是**新功能**，不是 parity 对齐，且都会扩大 nini 的能力面——**需要单独的产品决定**。
4. **`B25` 的判据已经被满足了一大半**。`scripts/keymap-parity.py` 退出 0、
   `cargo test -p nini-tui --test keymap_parity` 是绿的——**「不漂移」已经做到了**。
   剩下的就是「语义要不要追平」这一个产品问题，把它**写进 `divergences` 的理由里**就能关账，
   **不必先做 C 里的改键**。若 owner 明确要「追平」，那就按 A/B 排期，而不是让它一直挂在 `blocked-owner` 上。

### 6.4 拍板后怎么验收

- `python3 scripts/keymap-parity.py` 退出 **0**（**已达成，不能退化**）。
- `cargo test -p nini-tui --test keymap_parity` 退出 **0**（5 条 oracle，真树上绿 + 四种改坏方式各自变红）。
- `scripts/keymap-parity.json` 的 `divergences` 段里，**每一条要么被改掉、要么有一句 owner 给的理由**；
  `map` 段里 6 条 `nini: null` 的条目**每条都有一句理由**（现在这 6 条是**光秃秃的 null**，
  这本身就是一处「登记了但没说为什么不」）。
- 若改键（Ctrl+U / Ctrl+T / Ctrl+C）：`crates/nini-tui/tests/` 下有断言锁住新行为，
  且 `crates/nini-tui/src/keys.rs` 与 `scripts/keymap-parity.json` **同一次提交**改（否则门禁会红）。
- `docs/GOALS.md` §4 补上 R14 一行（**现状缺失**）。

### 6.5 如果 owner 什么都不做，代价是什么

- `B25` 永久 `blocked-owner`——**它是台账里唯一一条 `machine_checkable: true` 却卡在 owner 手里的差距**。
  这会让「可判定比例 43/56」这个数字**永远停在 43**，
  而实际上这条的机器判据**早就绿了**（两个脚本都退出 0）。
- 台账与实现的**唯一一次「状态落后于事实」**会一直挂着：
  `scripts/keymap-parity.json` 已经把 3 处分叉和 6 条缺失**逐条列出来**了，
  却因为「要不要改」没人拍板而**一条都没落地**。下一个来改键位的人会遇到一个
  **登记完整、但完全没有裁决**的契约文件。
- 更实际的：`Ctrl+U` 这一处**已经具备改的对策**（动作 `KillToLineStart` 在 `keys.rs:140` 躺着没人绑），
  **成本 5 行**。不拍板 = 这 5 行永远不做。
- 而 `Ctrl+C` 的分歧会持续制造**用户困惑**：nini 文档/教程里若写「Ctrl+C 中止」，与 Pi 的用户预期相反；
  这类「靠文档化解」的分歧，每一份文档都要重复一遍，而**改 5 行键表只需要一次**。

---

## 7. 一页总表

| 决策 | 一句话问题 | 推荐 | 台账条目 | 主要工作量（文件 × 行） | 拍板后的第一条可判定判据 |
|---|---|---|---|---|---|
| **R5** | provider 覆盖面要不要追平 Pi 的 40 | **不按「40」追平**，按方言分层；只补用户真正常用的若干家（每家 ~90 行），OAuth/新方言明确不做 | `B9`、`C3` | 每家 ≈ 1.5 文件 × ~90 行；补 10 家 ≈ 15 文件 × ~900 行 | 脚本能算出 nini 支持的 provider 集合，并与 `builtinProviders()` 的 40 在 `SNAPSHOT.lock` 对账 |
| **R10** | `/clone` 改成「切到新分支」要不要 | **对齐 Pi**：`/clone` = fork-at-leaf 并切到新 session（复用 `cmd_fork`），**不带参数** | `B1` | 4 文件 × ~130 行 | 1 条 oracle：clone 后当前 session 已切换且原分支仍在 tree 中 |
| **R11** | 要不要做 Pi 兼容的扩展运行时 | **不做**（R6 已被证伪）：关账 + README 明写「扩展工具不在 parity 范围内」；工具能力三项另立条目 | `B21`、`C4` | 3 文件 × ~20 行（声明） | README 里有那句话，且可被测试 grep 到 |
| **R12** | mermaid 要不要引布局引擎画真图 | **先只接 `markdown.mermaid` 键**（~180 行）；真图等 owner 提供上游 Rust 源码与许可后走移植 | `B22` | 4 文件 × ~180 行（接键）；真图不可估 | `markdown.mermaid` 三个取值真的改变渲染结果 |
| **R13** | 主题槽怎么接线 | **只接 thinking×7** 到**已有的**输入框外框；customMessage 等 R11；legacy×6 保留但把静默丢弃改成显式提示 | `B24` | 3 文件 × ~100 行 | 七档 thinking 对应七种边框颜色，各有断言 |
| **R14** | 键位语义要不要跟 Pi 对齐 | **先改 Ctrl+U**（绑到已有的 `KillToLineStart`，5 行）；其余 2 处分叉与 6 个缺失由 owner 逐条给理由或另立功能项 | `B25` | 3 文件 × ~40 行 | `divergences` 里每条都有理由；`map` 里 6 条 `nini: null` 都有 note |

### 7.1 需要 owner 提供/确认的清单（查不到的事实）

以下 9 项**本文件查不到**，它们要么需要外部材料，要么是台账自身的口径矛盾：

| # | 事项 | 属于 | 现状 |
|---|---|---|---|
| 1 | `xai-org/grok-build` 的 Rust `crates/codegen/xai-grok-markdown/src/mermaid.rs` 源码与其**许可** | R12 | 本机与仓库**都不可得**；许可**未查** |
| 2 | Rust 侧 mermaid 布局引擎的**体积增量** | R12 | 本机 cargo registry 缓存 915 个 crate，**无任何** mermaid/graph/layout crate → **未实测** |
| 3 | JS 引擎 crate（rquickjs/boa 等）的可得性与体积 | R11 | 本机 registry **无** → 选项 C 的体积与工作量**无法离线估** |
| 4 | Pi 每家 provider 的**模型目录** | R5 | `references/pi-0.85.1/src/api/data/*.json` **不在快照** → 无法逐家对账 |
| 5 | Pi 内建工具到底 **7 个还是 8 个** | R14 / B21 | 快照内 **7 个**；本机安装包 **8 个**（多 `powershell`）→ 需 owner 确认台账用哪个口径 |
| 6 | `B1.exit_criterion` 里「cmd_clone **接受 session-id 参数**」与 Pi 实际（**无参数**）的冲突 | R10 | 需 owner 确认按哪个口径 |
| 7 | ~~`B24` 的数字：16 还是 18~~ | — | **2026-11-05 已解决**：根因是「零读取」没写口径。按写明的口径复算 = **27 零读 = 9 syntax* + 18**，台账标题已改为 18、口径与复算命令已写进 `B24.nini_state`，不再需要 owner 裁决 |
| 8 | ~~`B25` 的「5 个缺失」是 5 组还是 6 个 binding id~~ | — | **2026-11-05 已解决**：`map` 里 `nini: null` 实为 **6 条**，其中「跳字符」占 2 个 id（「跳上/跳下同一字符」在 Pi 是两个独立 binding id，nini 一个都没有）→ **5 组 / 6 个 id**。台账标题与正文已按此改写 |
| 9 | R5 的「用户真正常用哪几家」 | R5 | 无客观依据 → 需 owner 给名单，否则就是又一轮自选范围 |

### 7.2 台账自身还需要修的三处口径（与拍板并行，不额外花成本）

| 位置 | 现状 | 应为 | 状态 |
|---|---|---|---|
| `docs/GOALS.md` §4 决策台账 | 只有 `R4 / R5 / R9 / R10 / R6` 五行 | 补 `R11 / R12 / R13 / R14` 四行 | ✅ **已补**（2026-11-05） |
| `scripts/parity-gaps.json` `B25.pi_basis` | 「KEYBINDINGS（**90** 个 binding id）」 | `KEYBINDINGS` 实为 **47**；90 是与 `TUI_KEYBINDINGS`(47) 合并去重后的数（重叠 4） | ✅ **已改**（2026-11-05） |
| `docs/GOALS.md` §2 Provider 行 | 「Pi **0.84.2** 内置 40 个」 | 版本口径对齐 0.85.1（实测同为 40） | ✅ **已改**（2026-11-05）——「按 R5 的新结论改写」仍待拍板 |

**另有一处新发现（与拍板并行，不额外花成本）**：`/login` 与 `/logout` 的 provider 清单里
有 **`together`**（10 个），但 `crates/nini-cli/src/provider_factory.rs` 的 `build_provider`
**构造不出** `together` —— 它的 `other =>` 臂会打印 `unknown provider: together` 并
`exit(2)`。也就是说 `/login` 在**教用户用一个 nini 拒绝的 provider**。修法二选一：
① 把 `together` 从 `/login`/`/logout` 清单里去掉（**5 行**）；② 给它加一个走
`openai_compat` 的薄壳（**~90 行**，属 R5 的「方言分层」范围）。
