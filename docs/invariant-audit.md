# 链③ 不变量保护审计（2026-10-18）

> 工具：`scripts/invariants.py`（可复算，`--json` 机读，`--fail-on-none` 可进 gate）
> 输入：仓库全部 `fix:` 提交 = 每条一次真实故障 = 一条可判定的祈使句不变量
> 性质：这是 GOALS T3/T4 的执行细则，不是新政策

---

## 0. 一句话结论

**回归保护率 80%，但对照验证覆盖率接近 0。** 前者不是 nini 的问题，
后者才是"改了很多轮还是有问题"的机制。

---

## 1. 名词：为什么"回归保护"不等于"正确"

nini 的缺陷分两种死法：

| 死法 | 机制 | 谁能挡 |
|---|---|---|
| **回归** | 修好的代码被后来改坏 | 断言即可（同源测试就够） |
| **当初就理解错了** | 实现与断言出自同一次误解，一起绿 | **只能靠外部对照 oracle** |

`stop_reason` 大小写、footer 的 3 份失真副本，都是第二类。
1428 个绿测试对第一类有效，对第二类**结构性无效**。

所以"回归保护率高"这件事**不构成交付证据**，必须与对照覆盖率分开报。

---

## 2. 实测结果（90 条 `fix:` 提交）

| 档位 | 条数 | 含义 |
|---|---|---|
| SELF_TEST | 64 | 修复自带测试——**同源**，能挡回归，挡不住误解 |
| CORROBORATED | 8 | 无自带测试，但其他测试以修复点专有词元命中 |
| WEAK | 0 | 证据不足（本轮无） |
| **NONE** | **18** | 无任何断言保护 |

**回归保护率 = 72/90 = 80.0%**

### 2.1 对照验证覆盖（链②）

| 层 | fixture 来源 | 数量 |
|---|---|---|
| session 格式 | 真 Pi 0.84.2 录制 | **2**（`pi-0.84.2-fresh/session.jsonl`） |
| TUI 渲染 | — | **0**（GOALS T1 分层所致） |
| agent loop | — | **0** |
| **provider SSE / tool_call 线协议** | **无** | 见 §4 |

真 Pi 产出的 fixture 全部只被 `nini-conformance` 的 5 个文件引用。

---

## 3. 对上一轮论断的纠正（数据推翻，必须记录）

- **我此前从提交前缀统计得出"补验证从未发生（test 提交仅 9 条）"——错。**
  64/90 条 fix 提交自己就带了测试；测试被打包进 `fix(tui):` 这类提交里，
  前缀统计看不见。真实缺口不是"不写测试"，是"写的测试与实现同源"。
- **我曾怀疑 `theme_watcher` 缺陷无回归保护——错。** 它有 5 个单元测试，
  `theme_wiring.rs` 还做到了 settings.json → runtime → 状态栏的端到端。
- **我曾怀疑断言只停在状态层——错。** 22 个测试文件用 `TestBackend`，
  60 处帧断言，6 个走 tmux 真终端。

**因此：nini 的验证分层（单元 / 帧 / 真终端）是扎实的，缺的是第三根轴——
所有帧断言都是"我认为 Pi 长这样"写的，没有一条是"Pi 实际长这样"录下来的。**

---

## 4. 最高优先发现：nini-ai 流式 tool_call 语义整簇无对照

NONE 的 18 条里，**4 条集中在 `nini-ai` 的流式 tool_call 语义**：

| 提交 | 缺陷 |
|---|---|
| `c3944d13` | 首个 chunk 的 tool_call args 被丢弃，且 agent 错误被吞 |
| `7a71f5b3` | aiio inline-args tool_call 只发 Start 不发 Stop |
| `9c9783e3` | single-shot 与 streaming 两条路径各发一次 Start，只在完成时 drain |
| `3bf4815d` | fallback 全部耗尽时的诊断信息误导 |

而 `crates/nini-ai/tests/openai_wire_e2e.rs:18` 在自己的注释里写着：

> `fixture` provider (which never touches SSE parsing) or **hand-built**

**即：nini-ai 的线协议测试 fixture 是手搓的。** 上面 4 条全部属于 §1 的
第二类死法——"我以为 SSE 长这样"的理解错误，self-built fixture 必然放行。

这同时解释了 GOALS §2 记的那个死结：`nini-ai` 自 10-12 起零提交、
R5（51 provider）待决——**这一侧一直在被自己的 fixture 自我确认，
所以既不敢扩范围，也发现不了自己的错。**

**这是链②的最高优先目标，且技术上可做**：pi 就在本机
（`/home/jin/.local/bin/pi` → pi 0.84.2），跑一次即可录到真实 SSE 流。

### 4.1 链② 第一批已交付：抓包器 + 真 fixture + 5 条 oracle（2026-10-18）

**工具（可复用，非一次性）**

| 文件 | 作用 |
|---|---|
| `scripts/capture-sse.py` | 在 Pi 与 provider 之间做流式透传，**逐 SSE 事件**落盘 |
| `scripts/capture-pi-sse.sh` | 编排：起代理 → 临时 `PI_CODING_AGENT_DIR` → 跑真 Pi → 停代理 |

零污染：不改 `~/.pi/agent`，只把临时 `models.json` 的 `baseUrl` 指向代理，
`auth.json` 用符号链接。**任何 `Authorization` / `api-key` / `cookie` 头一律
不落盘**（`safe_headers()`）。落盘粒度是单个 SSE `data:` 事件，因为
tool_call 的 `arguments` 是跨事件增量拼接的，**事件边界就是那 4 条缺陷的证据**。

**真 fixture**：`crates/nini-ai/tests/fixtures/pi-0.84.2-openai-inline-args-toolcall.sse`
（未经编辑的原始字节）。**Oracle**：`crates/nini-ai/tests/pi_wire_oracle.rs`，5 条，走真实 `stream()`。

### 真实 Pi 的 tool_call 形态（全部断言的依据）

```text
[0] delta 只有 role            —— content / tool_calls 字段不存在
[1] index:0 id name:"bash" arguments:"{"          ← inline-args：name 与首段 args 同一事件
[2] index:0        arguments 增量                  ← 无 id 无 name，必须继承 [1]
[3] index:0        arguments:"}"                   ← 一个 JSON 被拆到 3 个事件
[4] finish_reason:"tool_calls"，tool_calls 为 null
[5] "choices":[] 的 usage-only 事件                ← choices[0] 直接索引会 panic
```

### 变异验证：oracle 真的会红

注入 `c3944d13` 的原始 bug（`input_json: initial_args` → `String::new()`）：

```
5 条 oracle 中第 4 条 FAILED，症状 input_json: Null —— 其余 4 条仍绿
还原后全量 73+8+18+8+5+9 全绿，git diff 干净
```

### 必须纠正的预期：既有测试**也**抓到了同一个变异

`openai::tool_call_inline_args_tests::inline_args_on_start_chunk_emits_complete_stop`
在同一变异下同样失败。**所以链② 的价值不是"多几条测试"——那 5 条里至少一条是冗余的。**

真正的差异在别处，而且更严重：

| | 既有测试 | 链② oracle |
|---|---|---|
| 首块 `arguments` | `"{\"command\":\"ls /tmp\"}"`（**完整 body**） | `"{"`（**真实抓包**） |
| 注释自称 | "aiio-shape ... a single chunk carries ... the full arguments" | 无自称，字节来自真 Pi |
| 覆盖路径 | single-shot inline | **streaming 增量** |
| 形态来源 | **假设**，且被标注成事实 | **本机实测** |

即：既有测试守卫的形态**不是本机这条链路的真实形态**。
而 `9c9783e3`（"single-shot 与 streaming 两条路径共用一次 Start"）
正是这个假设的历史产物——它把两种形态当成一种。

**因此链② 的作用不是"多写测试"，而是让手搓形态无法再冒充真流。**
这条差异可被任何人证伪：直接 `diff` 真 fixture 与 `openai.rs` 里的 JSON 字面量。

---

## 5. 链② 第二批：TUI 录屏对照（2026-10-18）

### 5.1 为什么 TUI 终于可录了：脚本化上游

TUI 的对照 oracle 原本不可行，因为模型输出不确定。本轮解掉的办法是**把上游换掉**：

| 事实 | 依据 |
|---|---|
| Pi 只认协议不认上游 | `models.json` 里一个 provider 就是 `{api, baseUrl, apiKey, models}` |
| 本机实测 `api` 取值 | `openai-completions` / `anthropic-messages` |
| **实测 Pi 接受自定义上游** | 用 `scripts/mock-provider.py` 起本地假 provider，Pi 输出与脚本逐字一致 |

于是 `scripts/mock-provider.py` 同一份脚本同时喂给 Pi 和 nini，两边画面差异
就是对照差异 —— **确定性、零 token、可离线复算**。

mock 的形态刻意与真流一致：`tool_call` 的 `arguments` 按 ~12 字节切块、
首块只带 `{`、usage 是 `choices:[]` 的独立事件。mock 若发完整 body，
它就验证不了任何东西。

### 5.2 隔离与保真：四个坑（每个都是实测出来的）

| 坑 | 现象 | 处理 |
|---|---|---|
| 用户 skills 泄入启动画面 | 录出 29 个 skill 名，**取决于这台机器装了什么**——那是快照不是 oracle | 换 `HOME` 到临时目录，整个 `~/` 隔离 |
| 信任库为空 | 弹 `Trust project folder?` 选框，footer 永不出现 | 预置 `trust.json` |
| tmux server 复用 | 复用遗留 server（继承旧环境）时行为不同 | 独立 socket `-L nini-cap` |
| **输入注入失真** | `send-keys -l` 一次性 → `/hotkeys/hotkeys`；逐字符 → `0/ho` | `load-buffer` + `paste-buffer -p` |

另：`sleep` 快照不是 oracle，本工具用**连续两次快照一致**判定稳定。

**一个反直觉的实测结论**：命令模式（`/` 开头）下 bracketed paste 会把首字符
替换成 `0`，而**普通输入完全干净**（`abc`、长句均正确）。Pi 与 nini **在这一点
上表现一致**，所以它不是 nini 的缺陷——**这正是同条件对照的价值**：不追求解释
每个像素，只比同条件下的差异。规避方式：命令分两步注入（`/` → `hotkeys`）。

### 5.3 已录到的真实差异（人工确认，来自录屏而非推测）

golden 在 `fixtures/tui/captures/`（110×52，隔离 HOME，mock 上游）。

**差异 1：渲染形态**

```
Pi   │ Ctrl+O        │ Toggle tool output expansion     │   ← 框线表格
nini Key bindings (nini TUI)39 bindings — derived from the keymap  Enter send input
     Shift+Enter insert newline  Esc abort the run ...          ← 挤成一整段
```

**差异 2：键位语义**（Pi 面板有、nini 面板无）

| 键 | Pi 的语义 | nini |
|---|---|---|
| `Ctrl+-` | Undo | 用 `Ctrl+/`，键位不同 |
| `Ctrl+Z` | Suspend to background | 无 |
| `Shift+Tab` | Cycle thinking level | 用 `Ctrl+T` / `Shift+Ctrl+T` |
| `Esc` | Cancel autocomplete **/ abort streaming** | 仅 abort the run，缺 autocomplete cancel |
| `Ctrl+X` | Copy last assistant message | 无 |
| `Alt+Enter` | Queue follow-up message | 无 |
| `Alt+Up` | Restore queued messages | 无 |
| `/` `!` `!!` | Slash commands / Run bash | 面板里无 |

这为 `GOALS.md` “剩余项 5：键位与 Pi 的语义差异” 补上了**实证清单**（此前只是声明）。

### 5.4 未达标的部分（必须声明）

`scripts/tui-oracle.py` **提取器噪声过大，当前不可作为断言**：它会把表头边框
`├───┼───┤` 配成键位，描述换行后错位配对，110 列下 Pi 侧只提到 11 个键（真实约 25 个）。

**因此：录屏链路可靠，提取链路未达标。** 下一轮两条修法：把 COLS 提到 Pi
表格不换行的宽度并收紧列边界；或绕开屏幕 OCR——用 `/export` 的结构化导出
（Pi 与 nini 都有）。在提取器可信之前，它只能当人工辅助，**不进 gate**。

---

## 5. NONE 18 条的分类（待办清单已生成）

| 类别 | 条数 | 处理 |
|---|---|---|
| 产品缺陷（有 src 改动） | 13 | 进待办队列，按 §4 优先级排序 |
| 测试基建 | 3 | 环境无关性 / footer 断言编码了开发机 home / 锁粒度 |
| 构建·文档 | 2 | `--version` 自报身份（已有 CI 门禁）、UX 报告 |

产品缺陷里按 crate 分：`nini-tui` 5、`nini-ai` 4、`nini-cli` 2、`nini-core` 1、`nini-tools` 1。

---

## 6. 审计工具本身的两个坑（已修，留档以防重犯）

1. **语料必须含 src 内的 `#[cfg(test)]` 模块。** 只扫 `crates/*/tests/`
   会把 `theme_watcher.rs` 的 5 个单元测试误判为未保护。
2. **取标识符前必须剥离注释与字符串。** 否则 `ensures` / `badge` 这类
   注释里的英文散文会被当成代码标识符，diff 与语料必然相交，
   脚本输出 **100% 保护率**——一个比没有审计更危险的结论。

证据门槛（两条件同时满足才算证据）：该词元出现在 ≤2 条 fix 提交的改动里
（全局专一度），且出现在 ≤3 个测试文件里。只用后一个门槛时，
`null` / `digit` / `relaxed` 会造成假阳性。
