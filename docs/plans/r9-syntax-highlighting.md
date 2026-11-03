# R9 决策包 —— 语法高亮值不值那点体积

> 建立：2026-11-02（v0.16.0 之后的第一轮 TUI 工作）
> 作用：把台账 **B7**（语法高亮）背后的决策 **R9** 从「一个估的数字」变成
> 「一份有实测数字的判断书」。
> 权威：决策本身属 owner（`docs/GOALS.md` §7 的决策台账）；本文件只提供**证据**
> 与**选项**，不替 owner 拍板。
> 机器数据：`scripts/parity-gap.py --json`（台账）、`scripts/parity-gap.py`（人读表）。

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

## 4. 三个选项（**不是决定**）

| 选项 | 做法 | 代价 | 影响面 | 台账怎么变 |
|---|---|---|---|---|
| **A 不做** | 删掉 9 个悬空槽（或保留并标注 reserved），README 写明「不做语法高亮」 | 0 | 主题槽 61 → 52；**注意**：`COLOR_NAMES` 同时是主题文件写回的过滤器（`theme.rs:349` 的 `colors.retain(...)`），删槽会**静默丢弃**用户 `theme.json` 里的对应值 | B7 归 **fake**（产品原则差异：Pi 用 JS 高亮、nini 不引 JS 引擎），**R9 关闭** |
| **B 做（完整对齐 Pi）** | 引入 syntect（+fancy-regex），接 markdown 代码块 + read/write 输出，映射到 9 个 `syntax*` 槽 | **+0.63 MiB**（onig 版 +0.90）、+14 crate、编译 +~40 s；语法质量与 Pi 的 hljs 不同（syntect 是 TextMate 语法） | `crates/nini-tui/src/markdown.rs`（CodeBlock 分支）、read/write 渲染器、`theme.rs` 读取点 | B7 → **fixed**，但需先决 R9 |
| **C 折中** | 只在 `/export` 的 HTML 端高亮：把 `highlight.min.js`（约 120 KB）内联进 `crates/nini-tui/src/commands/html_export.rs` 生成的 HTML（照抄 Pi 的 `core/export-html/index.js:94-116` 机制），终端 TUI 不变 | 二进制 +~0.12 MiB（内联 vendor JS）；nini 的 HTML 目前是自研的「内联 minimal」markdown，要接 marked 才有代码块 | 只动导出路径；终端零风险 | B7 拆两半：导出端 → fixed；终端端 → 保留 open 或降级 fake |

---

## 5. 建议（**实现者不自行决定**）

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

## 6. 怎么复算

```console
$ python3 scripts/parity-gap.py --json | python3 -c 'import json,sys; d=json.load(sys.stdin); print([g["id"] for g in d["gaps"] if g["status"]=="blocked-owner"])'
['B1', 'B7', 'B9']
```

B7 的 `exit_criterion` 写着：「先由脚本给出『引入 synctect 前后的二进制体积
增量』数字，owner 拍板 R9 后 crates/nini-tui/tests/ 下 9 个 syntax* 槽至少有
1 条 oracle 通过」——**第一半已经完成**（§3 的实测数字），第二半等 owner。
