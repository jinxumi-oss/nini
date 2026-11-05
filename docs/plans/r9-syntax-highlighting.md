# R9 决策包 —— 语法高亮值不值那点体积

> 建立：2026-11-02（v0.16.0 之后的第一轮 TUI 工作）
> 作用：把台账 **B7**（语法高亮）背后的决策 **R9** 从「一个估的数字」变成
> 「一份有实测数字的判断书」。
> 权威：决策本身属 owner（`docs/GOALS.md` §7 的决策台账）；本文件只提供**证据**
> 与**选项**，不替 owner 拍板。
> 机器数据：`scripts/parity-gap.py --json`（台账）、`scripts/parity-gap.py`（人读表）。
>
> **2026-11-05 更新**：owner 已拍板 **R9 本轮不做**（结论见 §4.4 与 §6）。
> 本轮补了三件当初没测的事（§4：槽位契约 / 工作量落点 / 可验证边界）；
> §3 的体积实测数字与方法论**原样保留**——它们是对的，只是不构成做不做的决定性理由。

---

## 1. 决策是什么

`docs/GOALS.md` §7 的原始措辞：

| 决策号 | 问题 | 状态 |
|---|---|---|
| **R9** | 语法高亮是否值得 **+2–5 MB** 体积（synctect vs 自研） | 待决 |

它阻塞台账 **B7**（Pi 有语法高亮，nini 没有）。

**这一轮把那个数字推翻了**：实测的主流方案是 **+0.63 MiB**，不是 2–5 MB。
决策的形状因此变了——它不再是「体积 vs 单文件卖点」的取舍，而是「**0.6 MiB
买多少东西**」。

---

## 2. 事实：Pi 的终端**确实**做语法高亮

这一点此前是**未知的**（快照子集里没有渲染器），本轮查清了：

| 证据 | 位置 | 说明 |
|---|---|---|
| TUI 启动时加载语法库 | `references/pi-0.85.1/src/ui/interactive-mode.ts:110`（`import { loadAllHighlightLanguages } from "../../utils/syntax-highlight.ts"`）、`:1008-1013` | 高亮在**终端**里做，不是在浏览器里 |
| 高亮引擎 = highlight.js | 本机同版本安装包 `…/@earendil-works/pi-coding-agent/dist/utils/syntax-highlight.js:1-23`（`highlight.js/lib/core.js` + 22 种语言） | **来源是安装包不是快照**：快照子集不含该文件，但安装包与快照同为 0.85.1 |
| **那 9 个槽的真实读取点** | `dist/modes/interactive/theme/theme.js:800-819`：`keyword: (s)=>t.fg("syntaxKeyword",s)`、`comment: …syntaxComment`、`string: …syntaxString`、`operator: …syntaxOperator` … | Pi 用的正是 `syntax*` 这 9 个槽 |
| 终端代码块着色 | `theme.js:838-856`（`highlightCode()`）；markdown 渲染器 `dist/bundle/chunks/chunk-JVUZSMYM.js:583`（`case "code": … this.theme.highlightCode(token.text, token.lang)`） | 代码围栏按语言上色 |
| 工具输出也着色 | `dist/core/tools/renderers/read.js:12,92`、`write.js:10,20,28,45,97` | read/write 的文件内容也高亮 |

**结论：B7 是真差距，不是伪差距。** nini 的 9 个槽在 Pi 那边「有主」，在 nini
这边悬空。

### nini 这 9 个槽的来历

```
$ git log -S 'syntaxComment' --oneline -- crates/
13e3f55 feat(tui): align visual rendering with Pi agent (v0.8.3)
    - Syntax (9): reserved for synctect integration
```

**照抄 Pi 的主题键表时一并抄来的**，注释写明「reserved for syntect
integration」，至今**没有读取点**（`crates/nini-tui/src/theme.rs:175-183` 定义，
`:500-505` 给默认 ANSI 色，`:643-651`/`:718-726` 给 dark/light 的 hex）。

### Pi 的 HTML 导出：高亮在**浏览器端**

`dist/core/export-html/index.js:94-116` 把 `marked.min.js` 与
`highlight.min.js` **内联进导出的 HTML**，由浏览器执行（`template.js:1616-1625`
的 `hljs.highlight(...)` 都在浏览器里）。这正是 `scripts/export-oracle.py` 那句
「九成是 Pi 自带的 marked + highlight.js」的机制来源。

---

## 3. 实测：体积到底涨多少

方法（**在 /tmp 的副本里做，仓库的 `target/` 与工作树全程未污染**）：

1. `tar --exclude=./target --exclude=./.git` 复制仓库到 `/tmp/nini-size-probe`；
2. `cargo build --release --offline -p nini-cli --bin nini`（profile 与交付一致：
   `opt-level="z"`, `lto=true`, `codegen-units=1`, `strip=true`）；
3. 在 `crates/nini-tui/src/` 加一个**真的调用高亮 API** 的探针（防 LTO 把它裁掉），
   从 `nini-cli/src/main.rs` 调它，再量 `target/release/nini` 的字节数。

| 候选 | crate / 版本 | 新增 crate | 二进制 | **Δ vs baseline** |
|---|---|---|---|---|
| baseline（无高亮） | — | 0 | **6 954 856 B（6.63 MiB）** | — |
| **A** `syntect` + fancy-regex | syntect 5.3.0（`default-features=false, features=["default-fancy"]`） | **14**（syntect, bincode, plist, quick-xml, yaml-rust, fancy-regex, linked-hash-map, time*, deranged, num-conv, powerfmt） | 7 620 128 B | **+665 272 B = +0.63 MiB** |
| **B** `syntect` + onig | syntect 5.3.0（默认 onig） | **15**（+onig, onig_sys, pkg-config） | 7 895 200 B | **+940 344 B = +0.90 MiB** |
| **C** 自研（复用已在树内的 `regex`） | regex 1.x（已在依赖树里） | **0** | 6 958 008 B | **+3 152 B = +0.003 MiB** |

构建耗时：baseline 冷编译 **1m02s**；A 增量重编 42 s；B 48 s（含 onig_sys 的
C 编译）；C 41 s。

**`syntect` 关掉 `default-syntaxes` 之后连 `SyntaxSet`/`easy` API 都不存在**
（`syntect-5.3.0/src/easy.rs` 被 cfg 掉），**必须外挂语法 dump 文件** → 破坏
「单文件」卖点，所以它没被列为候选。

未测：`tree-sitter-*`（需联网，且体积远超预算）、`two-face` 等未进本地
registry cache（离线不可构建）。

> **R9 的「+2–5 MB」高估了 3–8 倍。**

---

## 4. 本轮复核：三件决策包当时没测的事

本节是 2026-11-05 的复核。§1–§3 回答的是「**体积多大**」；下面三件事回答的是
「**槽位对不对得上**」「**工作量落在哪**」「**做完能验证到什么程度**」。

### 4.1 槽位契约成立：Pi 的 9 个槽名与 nini 的 `COLOR_NAMES` 完全一致

| 事实 | 证据 |
|---|---|
| Pi 侧只有这 9 个 `syntax*` 槽 | 本机同版本安装包 `…/@earendil-works/pi-coding-agent/dist/modes/interactive/theme/theme.js`：`grep -o "syntax[A-Z][a-zA-Z]*"` 去重 = **9 个**：`syntaxComment` `syntaxFunction` `syntaxKeyword` `syntaxNumber` `syntaxOperator` `syntaxPunctuation` `syntaxString` `syntaxType` `syntaxVariable`（**来源是安装包不是快照**——见 §4.3） |
| nini 侧也是这 9 个 | `crates/nini-tui/src/theme.rs` 的 `COLOR_NAMES`（`// === Syntax (9)` 段）：**同样 9 个名字**，不多不少 |
| 结论 | 两边**集合完全相同** → 「把高亮输出落到 Pi 的这 9 个槽上」在**键名层面没有障碍**。§2 说 B7 是真差距，指的就是这件事 |

### 4.2 工作量的大头不在体积，在 **scope → 槽**的映射表

- §3 的三个数字（**+0.63 MiB / +0.90 MiB / +0.003 MiB**）**只覆盖体积**：它们回答的是
  「引 syntect 之后二进制变大多少」，**没有**回答「高亮要写多少行代码」。
- syntect **自带主题**、输出的是**它自己的颜色**（TextMate 语法的 scope → 颜色）。
  要落到 Pi 的那 9 个槽上，必须**写一张 scope → 槽的映射表**：把 syntect 的 scope 名
  （`entity.name.function`、`string.quoted`、`constant.numeric` 那一族）**逐条**归到
  Pi 的 9 个槽之一。**这张表就是主要工作量。**
- 参考量：Pi 自己那张表是 **20 行 scope → 颜色槽**的映射
  （安装包 `theme.js:800-819`：19 行落进那 9 个 `syntax*` 槽，`meta` 落进 `muted`）。
  Pi 用的是 **highlight.js** 的 scope 名（`keyword` / `comment` / `string` / `number` …），
  syntect 用的是 **TextMate** 的 scope 名——**两套词表不是一一对应**，映射要一条条做，
  而且**每换一套语法/主题就要重做一遍**。
- 所以：**体积已知，工作量未知**。把「+0.63 MiB」当成这件事的全部成本，
  是把**第二半**当成了全部。

### 4.3 可验证边界：Pi 的高亮实现不在快照里，「逐行对齐 Pi」在仓库内**不可验证**

| 事实 | 证据 |
|---|---|
| 快照里 Pi 从 `../../utils/syntax-highlight.ts` import | `references/pi-0.85.1/src/ui/interactive-mode.ts:110` `import { loadAllHighlightLanguages } from "../../utils/syntax-highlight.ts";`（调用点 `:1010`） |
| 快照的 `src/` 里**没有 `utils/`** | `references/pi-0.85.1/src/` 只有 `api / core / ext / loop / provider / session / tools-t1 / tools-t2 / tui / ui` 十个目录，**没有 `utils`** |
| 结论 | Pi 的高亮实现（`utils/syntax-highlight.ts`）与那张 scope 映射表（`theme.js:800-819`）**都只在安装包里**。所以 nini 若引 syntect，**能对齐的只有「槽位契约」这一层**（9 个槽各能出对应颜色）；**「与 Pi 的着色输出逐行一致」在仓库内无法验证**——对照物不在 `references/` 里 |

这不是 nini 的缺陷，是**快照是子集**的既有后果（与 `docs/plans/owner-decision-packs.md`
§0.1b 写的是同一件事）。

### 4.4 结论（owner 已拍板，2026-11-05）

1. **本轮不引依赖**：不引 syntect，也不引任何高亮引擎。§3 的数字保留存档——
   它说明体积**不是**拦路虎，但体积从来不是这一轮的决定性理由。
2. **台账 B7 的判据改精确**：不是「与 Pi 逐字符/逐行一致」（在仓库内不可验证，§4.3），
   而是「**9 个 `syntax*` 槽各能出对应颜色**」（§4.1）。
3. **将来若要做，工作量的大头是 scope → 槽的映射表**，不是体积；体积侧已经有数
   （+0.63 MiB）。

---

## 5. 三个选项（2026-11-02 当时未决，**留痕**）

| 选项 | 做法 | 代价 | 影响面 | 台账怎么变 |
|---|---|---|---|---|
| **A 不做** | 删掉 9 个悬空槽（或保留并标注 reserved），README 写明「不做语法高亮」 | 0 | 主题槽 61 → 52；**注意**：`COLOR_NAMES` 同时是主题文件写回的过滤器（`theme.rs:349` 的 `colors.retain(...)`），删槽会**静默丢弃**用户 `theme.json` 里的对应值 | B7 归 **fake**（产品原则差异：Pi 用 JS 高亮、nini 不引 JS 引擎），**R9 关闭** |
| **B 做（完整对齐 Pi）** | 引入 syntect（+fancy-regex），接 markdown 代码块 + read/write 输出，映射到 9 个 `syntax*` 槽 | **+0.63 MiB**（onig 版 +0.90）、+14 crate、编译 +~40 s；语法质量与 Pi 的 hljs 不同（syntect 是 TextMate 语法） | `crates/nini-tui/src/markdown.rs`（CodeBlock 分支）、read/write 渲染器、`theme.rs` 读取点 | B7 → **fixed**，但需先决 R9 |
| **C 折中** | 只在 `/export` 的 HTML 端高亮：把 `highlight.min.js`（约 120 KB）内联进 `crates/nini-tui/src/commands/html_export.rs` 生成的 HTML（照抄 Pi 的 `core/export-html/index.js:94-116` 机制），终端 TUI 不变 | 二进制 +~0.12 MiB（内联 vendor JS）；nini 的 HTML 目前是自研的「内联 minimal」markdown，要接 marked 才有代码块 | 只动导出路径；终端零风险 | B7 拆两半：导出端 → fixed；终端端 → 保留 open 或降级 fake |

---

## 6. 结论（owner 已拍板，2026-11-05）

**拍板结果：R9 本轮不做（选项 A 的口径，但槽不删、只标注）。**

- 不引 syntect、不引任何高亮引擎；9 个 `syntax*` 槽**保留**（删槽会静默丢弃用户
  `theme.json` 里的对应值，见 `crates/nini-tui/src/theme.rs` 的 `colors.retain(...)`）。
- 台账 B7 的判据改成「**9 个 `syntax*` 槽各能出对应颜色**」（理由见 §4.1–§4.4）。
- 将来若要做：**先写 scope → 槽的映射表**（§4.2），不是先解决体积（体积已有数，§3）。

**原推荐（2026-11-02 的倾向，留痕，owner 未采纳）**：

从工程事实出发，倾向 **B（做）**，理由三条：

1. **体积不再是拦路虎**。+0.63 MiB 对 6.63 MiB 的二进制是 **+9.5%**，而
   nini 的卖点是「单文件、无运行时」——那 0.63 MiB 是**编译进去的**，不引入
   Node、不引入 daemon、不引入第二个可执行文件。「单文件」没有被破坏。
2. **B7 是真差距**，而且是**用户看得见**的：代码块的着色在终端里一眼就能比出
   来。A 选项等于把一条真差距**永久记为伪差距**。
3. **C 方案不解决主要问题**：Pi 的高亮主要发生在**终端**里（导出只是顺带），
   C 只补了顺带的那一半。

但**决策仍是 owner 的**，而且决策前还需要回答两个问题（本文件给不出答案）：

- **语法质量**：syntect 是 TextMate 语法，Pi 是 highlight.js——**同一段代码两边
  的着色不会完全一样**。nini 的定位是「Pi 的原味」，这个差异要不要接受？
- **语言集**：Pi 加载 22 种语言；syntect 的默认语法集是 179 种（要按 Pi 的
  列表裁剪），还是全给？

**在 owner 拍板前，实现者可以做的**：把 9 个悬空槽的状态**写清楚**
（照抄 Pi 的主题键表 ≠ 支持这个功能），别让主题表看起来像承诺了什么。

---

## 7. 怎么复算

```console
$ python3 scripts/parity-gap.py --json | python3 -c 'import json,sys; d=json.load(sys.stdin); print([g["id"] for g in d["gaps"] if g["status"]=="blocked-owner"])'
['B1', 'B7', 'B9']
```

B7 的 `exit_criterion` 写着：「先由脚本给出『引入 synctect 前后的二进制体积
增量』数字，owner 拍板 R9 后 crates/nini-tui/tests/ 下 9 个 syntax* 槽至少有
1 条 oracle 通过」——**第一半已经完成**（§3 的实测数字），第二半等 owner。

**2026-11-05**：owner 已拍板（§4.4 / §6）。**第二半的判据随之改精确**：不再是
「至少有 1 条 oracle 通过」，而是「**9 个 `syntax*` 槽各能出对应颜色**」——
且这一层**在仓库内可验证**（槽名两边一致，§4.1），不像「与 Pi 逐行对齐」那样
对照物在快照之外（§4.3）。台账 `scripts/parity-gaps.json` 的 `B7` 侧由 owner 更新。
