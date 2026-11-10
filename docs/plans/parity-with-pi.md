# 对标 Pi 的功能实现 —— 优化计划 v3

> 建立：2026-10-18（v2）→ **v3：2026-10-23（重写，修 v2 的 13 处漏洞）**
> 目标：把「nini 对标 Pi 的功能实现」从**名字层面齐了**推进到**语义与行为层面可证齐**
> 权威：目标的表述在 [`../GOALS.md`](../GOALS.md)；本文件是执行细则。冲突以 GOALS 为准。
> 基准：格式层 `references/spec-v0.85.1/`（Pi v0.85.1 `d981de12`，19 文件带 sha256）；
> 行为层 **Pi 0.85.1**，源码快照锁在 `references/pi-0.85.1/`（411 文件 + `SNAPSHOT.lock` sha256）。
> 机器台账：**`scripts/parity-gaps.json`** 是差距的唯一数据源，本文件不再手工维护计数。

---

## 0. v3 为什么重写

v2（2026-10-18）的自我审查推翻了 v1 的三条论断，这本身是对的。但**v2 自己也带了 13 处漏洞**，
其中 5 处会直接让执行走偏。逐条修法见 [§10 修订记录](#10-v3-修订记录13-处漏洞的修法映射)。

v3 的三条硬变化：

1. **差距表不再写在文档里。** v2 的 §2 是 Markdown 表格，§5 的「A 类未闭 4 / B 类未闭 10 / 实测缺陷 1」
   是手工计数——而 v2 §9.4 刚刚判过「手工计数不可靠」的死刑。v3 把差距搬进
   `scripts/parity-gaps.json`，由 `scripts/parity-gap.py --json` 产出**所有**统计，
   文档只引用脚本输出，不复述数字。
2. **行为层对照物上锁，且引用改成符号形态。** v2 的 oracle 全部指向「本机 Pi」的
   `dist/*.js` 行号——实测 0.84.2 → 0.85.1 之间，90 处引用只有 11 处仍指向同一行，
   而**没有任何东西会红**。v3 把机制面源码快照进 `references/pi-0.85.1/`（411 文件 sha256），
   并把引用改成 `文件#符号`：含义被钉住，行号由脚本解析，符号消失即红灯。
3. **每条判据必须机器可判定。** v2 的 W2 判据含「逐条勾掉」、W1-A 含「tmux 实测」这类
   无法复算的项；v3 的每个 DoD 都写成**一条命令 + 一个退出码**。

---

## 1. 基准与分层（不可混用）

| 层 | 对照物 | 可验证方式 | 锁在哪 |
|---|---|---|---|
| 格式 | Pi v0.85.1（19 文件） | sha256 逐文件校验 | `references/spec-v0.85.1/SPEC.lock` |
| 行为 | **Pi v0.85.1**（411 个机制面文件） | 快照内解析 + 快照 sha256 | `references/pi-0.85.1/SNAPSHOT.lock` |

**一个目标版本 = 0.85.1，且是冻结的**（owner 指令：统一锚定一个版本，直到 owner 认为完全没有问题）。
冻结的机器判定在 `scripts/pi-target-lock.py`（并入 CI）；目标版本的唯一来源是
`references/TARGET.json`。换目标的顺序：**先差分 → 再重锚 → 最后才改目标**。
此前行为层只能对 0.84.2（本机装得下的那个），代价是
**90 处行号引用里只有 11 处在 0.85.1 里仍指向同一行**，且没有任何东西会红
（证据：`docs/pi-version-delta-0.84.2-to-0.85.1.md`）。

**硬规则**

1. 行为层的「Pi 是怎么做的」用**符号引用**：`文件#符号`，例如
   `agent-loop.ts#shouldStopAfterTurn`。含义被钉住，行号由脚本在目标版本里解析。
   `scripts/pi-oracle-lock.py --verify-refs` 逐条解析：**符号消失即退出 1**。
2. 行号引用（`agent-loop.ts:225`）只作为过渡保留，且必须仍能解析到**代码行**
   （不是注释行——注释会随代码移动，行为不会）。
3. **快照是行为层的唯一真相。** Pi 升级 → 快照失效 → `--check` 红 → 必须重录并重审 oracle。
   别名表只有一份（`scripts/pi_refs.py`）：0.85.1 里 `keybindings.ts` **同名不同物**
   （pi-tui 的 TUI 键位 vs coding-agent 的 app 键位），抄三份就会重演「同一份失真三份副本」。

> 已知的一处语义漂移（v3 记录在案）：`crates/nini-core/src/agent.rs` 中若干处
> `agent-loop.js:NNN` 实际写的是 `.ts` 的行号（npm 包不发 `.ts`，作者当时对着还原出的
> TS 源码抄行号）。快照已含 `.ts` 还原版（来自 `.js.map` 的 `sourcesContent`），
> v3 的 W0 负责把这些引用改写成 `.ts:` 形式并逐条复核。

---

## 2. 差距总表

**本节不列内容。** 差距的唯一数据源是 `scripts/parity-gaps.json`；
渲染与统计由 `scripts/parity-gap.py` 负责，校验由 `scripts/parity-gap.py --check-plan` 负责。

```console
$ python3 scripts/parity-gap.py --json      # 机读：总数、按类/状态/批次分布、逐条
$ python3 scripts/parity-gap.py              # 人读表格
$ python3 scripts/parity-gap.py --check-plan docs/plans/parity-with-pi.md   # 文档与台账对账
```

**当前规模**：以 `python3 scripts/parity-gap.py --json` 的输出为准（分类分布、状态分布、
机器可判定占比、被哪个 owner 决策阻塞的条数）。本文件**不手写这些数字**——
`scripts/plan-selfcheck.py` 的 S9 会因为文档里出现手写计数而失败。
需要看数字就跑脚本，需要看趋势就比 `docs/parity-snapshots/` 里的历史 JSON。

**分类口径（v3 修正）**

| 类 | 含义 | 处置 |
|---|---|---|
| A | 结构耦合，会损坏/丢失/错序数据 | 必须先于 B 类；每轮只减不增 |
| B | 功能缺失或语义不符 | 按批次推进 |
| C | 元问题（决定 A/B 会不会再漏） | W0/W5 |
| D | 小项 | 做的时候顺手，不单独立项 |

**与 v2 的差异**：v2 的 C 表有 C1/C1b/C2/C3 四行，v3 把 C1b（手工核对不可靠）并入 C1
（它本来就是 C1 的证据），并新增 **B10 = `/share` 仍是 stub**——v2 的 B 表里没有它，
而 README 承认它是唯一一条返回「尚未实现」的命令。**parity 声明不能把「命令名齐」当「语义齐」。**

### 2.1 工具的两层：原始自带 vs 扩展（**两件不同的工程**）

**这一节是 v3 新增的**，因为把两者混在一张待办表里是本项目反复犯的错
（10-05 的「缺 `ls` 作废」就是混层查错了目录）。

#### Pi 自己的划分（权威措辞来自 `dist/core/sdk.d.ts:25-47`）

```ts
noTools?: "all" | "builtin";
//  - "all": start with no tools enabled
//  - "builtin": disable the default built-in tools (read, bash, edit, write)
//    but keep extension/custom tools enabled
tools?: string[];        // "Otherwise it enables the default built-in tools
                         //  (read, bash, edit, write). Extension/custom tools
                         //  remain enabled unless noTools changes that default."
customTools?: ToolDefinition[];   // "in addition to built-in tools"
```

| 层 | Pi 的名字 | 内容（0.85.1） | 归属 | 性质 |
|---|---|---|---|---|
| **原始自带 · harness 层** | built-in | read, write, edit, bash | `pi-agent-core/dist/harness/tools/` | **函数**：进程内实现 + schema |
| **原始自带 · coding 层** | built-in | grep, find, ls（+ 0.85.1 的 powershell） | `pi-coding-agent/dist/core/tools/` | **函数** |
| **扩展层** | extension / custom | **Pi 默认一个都不带**（唯一内置扩展 llama.cpp 是隐藏的，且不注册工具） | 扩展运行时 `dist/core/extensions/` | **插件** |

Pi 的三个工厂函数把边界写死：`createCodingTools()` = [read, bash, edit, write]
（= Pi 说的 built-in 四件套）；`createReadOnlyTools()` = [read, grep, find, ls]；
`createAllTools()` = 全部。

#### 四条判据（区分两层的**可判定**依据）

| 判据 | 原始自带工具 | 扩展工具 |
|---|---|---|
| **谁发布** | 随二进制发布，升级即变 | 第三方插件，独立版本 |
| **谁进 system prompt** | **自动进**（`Available tools` 段），并且工具集决定 guideline（`system-prompt.js:53-60` 的 `hasBash/hasGrep/hasFind/hasLs/hasRead`） | **默认不进**——除非扩展自己提供 `promptSnippet` / `promptGuidelines` |
| **谁渲染** | 统一由 TUI 渲染（Pi 的 `ToolExecutionComponent`） | **可以自带** `renderCall` / `renderResult` / `renderShell` |
| **可否被关掉** | `tools` 白名单 / `excludeTools` / `noTools: "builtin"` | 默认不受 `noTools: "builtin"` 影响（只有 `noTools: "all"` 才关） |

#### nini 两侧的现状

| | 原始自带（T1） | 扩展（T2） |
|---|---|---|
| 工具集 | `nini-tools` 有 read/write/edit/bash/grep/find —— **缺 `ls`**（B11）；契约抄自 0.84.2，**未按 0.85.1 重捕获**（B13） | `nini-ext` 的 `register_tool(Arc<dyn Tool>)` —— **只能塞进一个函数式工具** |
| prompt 归属 | ✅ 已有 `Tool::system_prompt_contribution()`（snippet + guidelines，组装确定性排序） | ✅ 同上（同一个 trait） |
| 渲染归属 | 统一由 `nini-tui` 渲染 | ❌ **无按工具注册渲染器的入口**（`rich.rs` 按名字通用渲染） |
| 执行期反馈 | 无流式更新回调 | ❌ 无 `onUpdate`、无 `signal`（`ToolContext` 只有 cwd/两个预算） |

**结论**：nini 已经有「扩展工具」的**一半契约**（prompt 贡献），缺另一半
（渲染钩子、流式更新、signal）。所以「扩展运行时不可用」这句话不准确——
准确的说法是：**nini 的扩展只能贡献「一个函数式工具 + 一段 prompt」，
不能贡献「一个带自己外观与流式进度的工具」**。

#### 两条**独立**的工作流（不要混排期）

| 工作流 | 范围 | 退出判据 |
|---|---|---|
| **WF-BUILTIN** | 八个内置工具的**契约**（描述 + 参数 schema，按 0.85.1 重捕获）与**语义**（截断、超时、并发、变更队列） | **执行细则在 [`plans/tool-parity.md`](tool-parity.md)**（T1–T4；判据看板 `scripts/tool-parity-check.py`）。已闭：**B11**（`ls` 与工具集→prompt 联动）、**B13**（契约按 0.85.1 重捕获）、**B12**（截断语义统一为 50 KiB head/tail） |
| **WF-EXTENSION** | 扩展运行时：渲染钩子、流式更新、signal、受限采样 | 每项能力各有 1 条 oracle；在此之前 **README 明确声明「扩展工具不在 parity 范围内」** |

**WF-BUILTIN 是「高效、准确完成任务」的载体；WF-EXTENSION 是「生态」，成本高一个数量级。**

**v3 这一轮又新增了三条**（都是执行过程中实测发现的，不在 v2 的表里）：
**A5**（TUI 启动后不创建 session 文件，普通会话不落盘）、**A6**（中断后排队的消息
不会被自动投递）、**D2**（无 TTY 时的横幅硬编码 `nini v0.4.0`）。详见台账。

**D3**（Pi 的可点击链接 OSC 8）本轮**收尾**：它的退出判据有两个分支——① ratatui 支持
hyperlink：**被上游证据排除**（ratatui 0.29 / 0.30.2、ratatui-core 0.1.2、
crossterm 0.28 / 0.29 全部没有 hyperlink；且 cell 缓冲区会丢弃含控制字符的
grapheme、差分重绘只重画变化的 cell，承载不了有状态的 OSC 8 转义对）；
② README 的 known limitations 里明说「链接不可点击」：**已满足**
（`README.md` / `README.zh-CN.md` 各一条，oracle:
`crates/nini-tui/tests/hyperlink_limitation_documented.rs`）。一个分支走不通、
一个已达成，故关闭。

**A8**（长任务在半路被轮数上限截断）本轮**关掉**：nini 曾给一次 run 设 50 轮硬上限
（`MAX_TOOL_ITERATIONS = 50`），撞上就抛 `TooManyIterations` **终止整轮**——60 步的
任务死在第 50 步，transcript 里只有 49 条工具结果、**没有答案**（用户截图里的
`too many tool iterations (50)`）。Pi 0.85.1 **没有任何轮数上限**：
`references/pi-0.85.1/src/loop/agent-loop.ts` 的内外两层循环都没有计数器，
`references/pi-0.85.1/src/` 全目录 grep `maxIterations|max_turns|maxTurns|turnCount|
iterationCount` 零命中。现在默认**不限**（`max_iterations = 0`），上限改为可配置
（`--max-iterations N` / `settings.json` 的 `maxIterations`），无上限时由 1000 轮的
runaway 兜底防止真死循环；`max_follow_up_rounds` 同理（16 → 0 = 不限，Pi 的外层是
`while (true)`）。oracle：`crates/nini-conformance/tests/long_task_delivery.rs`
（3 条）+ `scripts/e2e-long-task.sh`（真二进制 + mock provider，nini 与真 Pi 0.85.1
跑同一个 60 步剧本，断言两边都交付、请求次数都是 61 且相等）。

**B14**（models.json 声明的 provider 用不了）本轮**关掉**：nini 此前只把
`models.json` 当**模型目录**——`baseUrl`/`apiKey` 因为 `ProviderConfig` 没有
camelCase 别名**连读都没读进来**（serde 整条丢弃），构造层只认十一个硬编码名字，
于是用户自己配的网关 provider 报 `unknown provider: my-gw`，而 Pi 照着文件里的
`baseUrl` 就去连了。现在：camelCase 别名读得进来（snake_case 旧拼法不回归）、Pi 的
配置值求值（`$VAR`/`${VAR}`/`!cmd`/`$$`/`$!`）有了、构造层先查**用户级**
models.json（命中即优先，11 个内置分支一行未改）、`baseUrl` 回退链 = models.json >
原分支的 `*_BASE_URL` 环境变量 > 内置默认常量 > 报 `"baseUrl" is required`，
`--list-models` 按 Pi 的格式在 stderr 打 `Warning: errors loading models.json:`。
oracle：`crates/nini-cli/tests/models_json_provider.rs`（真二进制 + wiremock，断言
请求打到文件里的 baseUrl 且带上文件里的 apiKey）+ `scripts/e2e-models-json-provider.sh`
（nini 与真 Pi 0.85.1 跑同一份文件，断言两边 mock 收到的 `Authorization` 字面相等，
缺 baseUrl / 缺 api / 缺凭证时两边指向同一个缺失项）。

**B16**（nini 的 bash 工具听 `$SHELL` 的，Pi 不听）本轮**关掉**：Pi 挑 shell 的规则
是 `getShellConfig`（`dist/utils/shell.js:92-102`，Unix 分支）——`/bin/bash` 存在就用
→ PATH 上的 `bash` → 兜底 `sh`，**从不问环境**。nini 原来读 `$SHELL`，于是
GitHub Actions 的 macOS runner 上（`$SHELL=/Users/runner/bin/bash`，那是 runner 的
包装脚本，不是能跑命令的 shell）**每一条**命令都失败，工具结果渲染成 `✗` 而不是
`✓`，`crates/nini-tui/tests/agent_wire_e2e.rs` 的
`submit_triggers_agent_and_renders_response` 因此变红（CI run 40829135270 macOS
失败）。现在 nini 用与 Pi 同一套规则；登录 shell 是 zsh 的用户拿到的也回到 Pi 给的
bash。oracle：`crates/nini-tools/src/bash.rs` 的
`the_shell_is_not_taken_from_the_environment`，以及在坏 `$SHELL` 下仍须绿的那条
TUI 测试（改回读 `$SHELL` 必须变红）。

**B15**（项目级 `<cwd>/.nini/models.json` 的 provider 配置**不被采用**）与
**D5**（Pi 的 oauth/headers/authHeader/model 级字段在 nini 侧无承载点、模型级
baseUrl/api/compat/cost/samplingParams/thinkingLevelMap 同理，以及同名内置 provider
命中 models.json 条目时 fallback 轮转不生效）本轮**明确不做并登记**：项目级文件
只用于展示（`--list-models`、`merge_user_overrides`），**不参与 provider 构造**——
一个用户 merely clone 的仓库不该能指使 nini 去连它选的主机、把凭证发过去，或执行
它写的 `!cmd`。Pi 0.85.1 根本没有项目级 models.json 这个概念，所以「与 Pi 一致」的
答案就是**不读**。

**B17**（nini 的工具并发度默认 8 且用户改不了，Pi 0.85.1 的并行分支是无上限的
`Promise.all`）本轮**关掉**：默认改成**不限**（0 = 不限），与 Pi 一致；同时把上限
旋钮**暴露**给用户——`--max-parallel-tools N` 与 settings.json 的 `maxParallelTools`
（0 = 不限，N > 0 = 最多 N 个并发）。这个旋钮是 **Pi 没有**的能力，属于 superset，
不是新的不一致。v0.8.9 当初立上限的量化依据（30 × 8MiB 缓冲）不成立：
`STREAM_CAPTURE_CAP` 是**单流**上限，生产每流只有 64KiB（每个子进程两条流，
stdout 与 stderr，30 条并发约 3.75MiB，且该预算随 `ctx.max_output_bytes` 可配），
且仓库里没有实测数据。对齐的范围只有**工具执行阶段的并发度**这一层：Pi 的
prepare 阶段仍是串行的，abort 的部分结果语义也不同（见 B19）。
oracle（**行为断言**，不是常量断言）：`crates/nini-core/tests/agent_integration.rs` 的
`the_default_batch_runs_all_calls_at_once`（K=20 个探针工具的 peak 必须 = 20），
`crates/nini-cli/tests/cli_flag_contract.rs` 的
`max_parallel_tools_is_accepted_rather_than_refused`（flag 被接受而不是被拒绝），
加 `scripts/e2e-parallel-tools.sh`（同一剧本，nini peak 与 Pi peak 相等**且都等于
K=20**，且 Pi 侧自己的两个独立证据——自报峰值与重叠区间数——互相一致）。

**B18**（Pi 的 `executionMode` / `toolExecution` **整批降级**在 nini 侧没有接线：
类型在、`batch_is_sequential` 判定也在，但生产路径恒 Parallel，也没有用户可配的
开关）与 **B19**（abort 的**部分结果**语义：Pi 保留已完成工具的结果，nini 明确丢弃
并补发 `[aborted]`，理由写在代码注释里）本轮**明确不做并登记**：B18 涉及工具注册
API 与配置项、改动面比并发度大，而并发度旋钮已经覆盖了「用户想降级」的需求；
B19 是 nini 的有意设计（发部分 `tool_results` 会破坏与下一次 LLM 调用的 API 契约）。
不限并发之后，一次中止最多同时取消 N 个在途 future（不再是 ≤8）：被取消的 `bash`
的**直接子进程**会被 `kill_on_drop` 杀掉
（`crates/nini-tools/src/procgroup.rs:130`），但它 spawn 出来的**子孙进程**不会
被整组杀掉——这个取舍写进 CHANGELOG 的「已知限制」。

**v0.15（批次级工具执行模式 + 台账盘点）**：本轮关掉了 **B18**——批次级
`toolExecution` 的入口补上了（CLI `--tool-execution`、settings 键 `tool_execution`
（`toolExecution` 亦作为别名认），
三扇装配门都带上），判定谓词与 Pi 逐字同构；工具级 executionMode 仍无承载主体
（nini 没有运行时扩展系统），新登记为 **B21**。另外三条是**对外说明**类的小修：
**B1a**（`/clone` 不再声明一个它不读的参数）、**B10a**（`/share` 说清本机不做
云端分享，并指向 `/export`）、**B15a**（README 说清项目级 models.json 不参与
provider 构造）。

同一次盘点把四条**伪差距**从「待修」改成了「不是差距」：**B4**（bash mode 早在
v0.11 就实现了，台账措辞过期）、**B10**（产品原则差异：nini 不做云端分享）、
**B15**（有意的安全决策）、**B19**（有意设计，理由写在代码注释里）。它们的状态是
`fake`——既不是 `fixed`（什么都没修），也不是 `open`（没什么可修）；剩下的真差距
在各自拆出来的子条目里（B4a / B10a / B15a）。另新登记 **B20**：串行模式下 nini
的工具事件时机与 Pi 不同（Pi 的 start 与 end 交错，nini 整批 start 先行）——修它
要新增事件类型并改 TUI 渲染，会牵动默认的并行路径，所以**登记而不半修**。
**D1** 仍然是盘点出来的唯一一条规模小、可直接做的真差距。

**这一轮对齐的范围只有「批次级」这一层**：判据是**两个** oracle。第一个是差分
oracle `scripts/e2e-tool-execution.sh`（nini 与真 Pi 0.85.1 跑同一个剧本，三条断言：
Pi 侧并发峰值 == 1、nini 侧并发峰值 == 1、**对照组** parallel 模式下两侧峰值
都 == K——没有第三条，「nini 永远峰值 1」也能骗过前两条）。

第二个是**黑盒端到端** oracle `scripts/e2e-tool-execution-cli.sh`：第一个 oracle 的
nini 侧是用**测试内的环境变量**切模式的，它根本没有经过 CLI —— 于是「用户敲的
`--tool-execution sequential` 真的让工具一个一个跑」一直没有端到端证据，而 v0.14
恰恰栽在「CLI flag 被丢弃时，全仓测试仍然全绿」上。第二个 oracle 跑**编译出来的
二进制**，用墙钟耗时判并发度（K 个各睡 S 秒的 bash 调用：串行 ≈ K×S，并发 ≈ S）。

它的阈值取**两端的中点** `MID = (K+1)×S/2`，而不是拍脑袋的倍数：真值是
`S+开销` 与 `K×S+开销`，中点让「机器慢 → 假红」与「变异 → 假绿」**同时**余量最大
（K=4、S=1s 时余量 1.5s，实测开销 ≈ 40ms）。`SLEEP` 有下限 0.5s：睡得太短，
进程开销就会淹没判据。第四条判据是「这一批**真的跑了 K 个工具**」——工具调用在
黑盒里看不见，工具**结果**看得见（mock 收到第二次请求时带着 K 条 `role="tool"`），
所以 mock 的日志记 `tool_results=N`，oracle 据此断言。

实测四条反向变异都会让它红：把 CLI flag 丢弃、去掉批次级降级判定、mock 不再记
`tool_results`、以及把阈值放宽成恒成立（最后一条由
`crates/nini-core/tests/oracle_criteria.rs` 抓住——**墙钟阈值也能被放宽到永不失败**，
所以它和差分 oracle 的判据一样需要外部守卫）。

**没有**对齐的两件事
都登记在台账里：**B21**（工具级 executionMode 的声明通道：nini 没有运行时扩展
系统）与 **B20**（串行模式下的事件时机：Pi 的 start 与 end 交错，nini 整批 start
先行）。README 与 CHANGELOG 这一轮也把话限定在这一层——上一版「与 Pi 0.85.1
对齐」写得太宽，被 reviewer 抓过。

---

## 3. 排序规则（与 GOALS 合并后的唯一版本）

v2 的排序规则（A→B）与 GOALS §3 的现行规则（按未验证路径数排序）**互相冲突**，
而 GOALS 明文规定「冲突本身就是需要修的缺陷」。v3 合并为：

1. **功能面按 A→B 排**（A 类结构缺陷优先，因为它会损坏数据）。
2. **同一类内按严重度 P0→P3**；同严重度内按**未验证路径数**降序（GOALS §3 的原规则，保留）。
3. **被 owner 决策阻塞的条目单列**（`blocked-owner`），不参与排序，因为它们的工作量由外部输入决定。
4. **被外部条件阻塞的条目单列**（`blocked-external`），并写明降级路径。
5. **盘点后看清「从来不是差距」的条目单列**（`fake`）：它不是待修的差距，而是有意的
   设计分歧、产品原则差异，或台账措辞过期。`fake` 既不是 `fixed`（什么都没修），
   也不是 `open`（没什么可修）——混进这两类都会让统计说谎。
6. **每轮开工前必须写下退出判据（可判定 = 一条命令 + 一个退出码）**（GOALS §3 硬性要求）。

> 排序的输入是 `scripts/parity-gap.py --json` 的 `by_round` / `by_severity` / `machine_checkable`，
> 不是记忆。**任何一轮结束都必须重跑该脚本，并把前后两个 JSON 存进 `docs/parity-snapshots/`**——
> 「A 类每轮只减不增、oracle 数每轮只增」这条单调性要求，需要有历史快照才能被检查。

---

## 4. 阶段

每个阶段的退出判据都是**可执行的布尔判定**。

### W0 —— 把「行号式 oracle」变成有锁的、可枚举的台账

**为什么先做**：v2 的 6 处参数提示差异、2 处假参数，全部是手工读代码发现的；
v2 的自我审查又证明了手工读会出错（§9）。v3 的 W0 已经有实物交付（见下），
剩下的工作是**迁移存量**与**补 Pi 侧可执行对照**。

**已交付（v3）**

| 产物 | 作用 | 验证命令 |
|---|---|---|
| `scripts/parity-gaps.json` | 差距的唯一数据源，12 个必填字段 | `python3 scripts/parity-gap.py --json` |
| `scripts/parity-gap.py` | 台账校验 + 机读统计 + 与本文件对账 | `python3 scripts/parity-gap.py --check-plan docs/plans/parity-with-pi.md` |
| `scripts/pi-line-refs.py` | 全量行号引用清单（数量以 `scripts/pi-line-refs.py --json` 的输出为准） | `python3 scripts/pi-line-refs.py --json` |
| `references/pi-0.85.1/` + `SNAPSHOT.lock` | Pi 0.85.1 行为层源码快照（411 文件 sha256） | `python3 scripts/pi-oracle-lock.py --check` |
| `scripts/pi-oracle-lock.py` | 快照校验 + 引用逐条解析 | `python3 scripts/pi-oracle-lock.py --verify-refs` |
| `crates/nini-conformance/fixtures/pi-0.85.1-session.jsonl` | **真 Pi 0.85.1 录制**的会话对照物（只洗路径，父子链原样）——**C5** 的对照物，取代 0.84.2 的旧录制 | `cargo test -p nini-conformance --test session_parity_0_85_1` |

**未交付（W0 剩余，退出判据如下）**

1. **把 `.js` 行号引用统一为 `.ts` 行号**（npm 包不发 `.ts`，作者当年对着还原源码抄行号）。
   退出判据：`python3 scripts/pi-oracle-lock.py --verify-refs` 退出 0 且 `drift=0`。
2. **补 Pi 侧可执行对照**：用 `scripts/mock-provider.py` 起本地 OpenAI-compat 端点，
   临时把 Pi 的 `models.json` 指过去，录制 A1（中断）与 A2（压缩）两个场景的 fixture。
   退出判据：`crates/nini-conformance/fixtures/pi-0.84.2-*.jsonl` 新增 ≥2 个文件，
   且 `python3 scripts/parity-gap.py --json` 中 A1/A2 的 `oracle` 字段不再是 `none`。
3. **录不到怎么办**：如果 mock provider 也无法触发（Pi 侧不可离线），**不得跳过 W0**，
   也不得降级为「手工核对」——按 §3 规则 4 标 `blocked-external`，并把该条差距的
   `exit_criterion` 改写为「快照行号校验通过 + 人工复核记录进 `docs/invariant-verification.md`」。
   **注意：这条分支不允许顺延 A1 的修复到 B 类之后**（v2 漏洞 1）。

### W1-A —— A1：中断与 spawn 解耦（P0）✅ **v3 已完成**

- 修法：`RunState` 新增 `agent_alive: bool`（与 `mode` 解耦），spawn 时置 true，
  终止路径（`Done` 事件 + watchdog）置 false；`mode` 回归「编辑器可否接受输入」本职。
- 附加：watchdog 用 `run_seq` 单调序号保证「迟到的 reap 不会解除新一轮的守卫」。
- 退出判据：
  1. `cargo test -p nini-tui --test esc_concurrency_probe` 退出 0（**不带 `--ignored`**，5 条）；
  2. `cargo test -p nini-tui` 退出 0（361 unit + 全部集成，0 failed）；
  3. 变异验证两条均红：删掉 `Done` 里的 `agent_alive = false` → `the_done_event_itself_clears_the_guard` 红；
     拆掉 watchdog 的复位 → `a_driver_that_never_says_done_still_reopens_the_editor` 红。

### W1-B —— A2 + A3：压缩异步化与 ESC handler 栈（P0→P1，高风险）

- A2：`is_compacting` 必须有赋值点；压缩期间的提交行为**以 Pi 实测为准**
  （`agent-session.js` 的抛错路径与 `queueCompactionMessage` 路径并存，**必须先录 Pi 行为再定**）。
- A3：引入 `enum EscapeTarget { Idle, Agent, Compacting, Retry, BranchSummary }`，对齐 Pi 的 handler 替换。
- **与 W1-A 的关系**：两者都改 Esc 路径与运行态。v2 把它们分批的理由（避免互相掩护回归）
  在 v3 仍然成立，但**必须同批落地、同一轮验证**，否则「哪一处引起的回归」不可判定。
- 退出判据：
  1. `runtime.rs` 里那个「压缩期间提交进队列」的分支**从不可达变为可达**，且有 1 条 oracle；
  2. 压缩中/重试中/分支摘要中按 Esc 的三种行为各有 1 条 oracle；
  3. 现有 compaction 测试零改动通过；
  4. `python3 scripts/parity-gap.py --json` 中 A2/A3 的 `status` 变为 `fixed`。

> **v0.11 补记（2026-10-26）**：**A2b**（压缩期的 Esc 只取消压缩）在 v0.11 关掉，
> 做法见 [`plans/next-parity.md`](next-parity.md) §2 N1；同时**发现并关掉**了 **A7**
> ——Esc 的中止信号**没有任何消费者**（详见 next-parity.md §4 的自审第 16 条）。

### W1-C —— A4：双队列拆分（P2）

依赖 A2。退出判据：`clearAllQueues` 的两个队列各有 1 条 oracle，ESC 还回时两者顺序与 Pi 一致。

> **v0.11 补记（2026-10-27）**：A4 **已关**。nini 此前只有一条
> `pending_next_turn_messages`，同时被三个用途争抢（steering 钩子 / 轮后自动投递 /
> Esc 退回编辑器），谁先谁拿 → 语义不确定。现在拆成
> `pending_steer_messages` 与 `pending_followup_messages`，按 Pi 路由
> （流式中 Enter→steer、**Alt+Enter**→followUp、压缩期同样两分），
> 四个消费点各归其位，Esc 把两个队列一起退回且 **steering 在前**。
> 做法与变异注入记录见 [`plans/next-parity.md`](next-parity.md) §2 N2。

### W2 —— B 类命令语义与交互形态（P1–P2）

B1 `/clone`、B1b `/export`、B2 `/trust`、B3 `/reload`、B6 模型搜索排序、B10 `/share`、D 类小项。
- 退出判据：
  1. `scripts/parity-gaps.json` 中 W2 的每条 `status` 变为 `fixed`；
  2. 每条**至少一条会红的 oracle**（不是「跑得通」）；
  3. `argument_hint` 与 `references/spec-v0.85.1/slash-commands/builtin.json` 逐条一致
     （可脚本复算，纳入 CI）。

> **v0.11 补记（2026-10-26）**：W2 里的 **B2**（`/trust` 弹选择器）、**B3**（`/reload` 的范围）、
> **B6**（模型搜索排序）已关；**B1**（`/clone`）仍被 owner 决策 **R10** 阻塞；
> **B10**（`/share`）与 D 类小项仍未做。

### W3 —— B4 bash mode + B5 double-ESC（P2）

动手前必须先确认 Pi 的 `isBashMode` 语义（**不要把它当成 `!cmd`**）。
退出判据：两条 oracle；且 `!cmd` 的既有行为不被改动。

> **v0.11 补记（2026-10-26）**：**B5**（double-ESC）已在 v0.11 关掉——
> 做法见 [`plans/next-parity.md`](next-parity.md) §2 N3。**B4**（bash mode）仍未做：
> 它是 Pi 的**独立交互形态**，必须先录 Pi 的行为再设计 nini 的形态。

### W4 —— B7 语法高亮 + B8 mermaid（P3，先决策再动手）

B7 需 owner 决策（R9：体积 +2–5 MB vs nini 的单文件卖点）。
退出判据：**若 owner 决定不做**，该条的关闭条件是「已明确拒绝并写进 README known limitations
+ 有 1 条 oracle 断言该拒绝行为」；若做，则 9 个 `syntax*` 槽至少 1 条 oracle。

### W5 —— C2/C3 治理

C2：任何计划文档的状态字段必须能在代码里指出对应符号。
**已闭合（2026-10-28）**：台账的 `nini_state` 一律只写**符号**（行号会漂，符号不会），
并由两处机器检查盯住——`crates/nini-core/tests/ledger_anchors.rs`（Rust 侧的第二实现）。判据：每条 gap 的
`nini_state` 至少有一个能解析的锚点（文件存在 + 符号在该文件里 grep 得到），
且两个实现都必须抓住「故意做坏」的样例。
C3：provider/成本层是排期问题，记录在案，不在代码层解决。

### v0.16 —— 关掉最后四条**可做**的真差距（2026-11-02）

这一轮没有新的功能面，只把台账里剩下的、**不依赖 owner 决策**的真差距关掉，
并把每一条变成**可判定的断言**。

| 条目 | 做了什么 | 判据 |
|---|---|---|
| **B4a** | `!cmd` / `!!cmd` 边跑边出、Esc 杀整棵进程树、不再卡 UI（`run_streaming` + `BashJob` + `EscapeTarget::Bash`） | `cargo test -p nini-tui --test b4a_bash_stream` 退出 0（9 条） |
| **B20** | 新增 `AgentEvent::ToolExecutionStart`：**串行**批次里 start/end 按工具交错，**并行**批次里所有 start 在任何 result 之前（与 `agent-loop.ts:431`/`:487` 逐条对齐）；TUI 用它区分「排队」与「正在执行」 | `cargo test -p nini-core --test tool_execution_events` 退出 0（5 条）+ `cargo test -p nini-tui --test b20_tool_execution_events` 退出 0（2 条） |
| **D1** | 三个「已就位但没接线」的符号接进运行时（终端标题 / tmux 键盘检查 / 资源诊断），并让 skills 的同名碰撞**落败者不再被静默丢弃** | `cargo test -p nini-tui --test d1_startup_wiring` 退出 0（9 条）+ `cargo test -p nini-core --test skill_collision_precedence` 退出 0（5 条） |
| **B8** | mermaid 围栏块 → **明确降级占位**（图种 + 「没画」+ 源码逐行保留）；`Off` 逐字节走回旧代码块路径 | `cargo test -p nini-tui --test b8_mermaid_block` 退出 0（10 条） |

**新登记 B22**：B8 按它自己的判据（「渲染为图**或**明确的降级占位」）关掉了，
但**真图**渲染仍未做——Pi 的 `components/mermaid.ts` 不在快照里（无从验证），
nini 也没有布局引擎。这一条单独登记，不含糊。

**顺带修掉**：`install.sh` 的 EXIT trap 吞掉真正的下载错误（并泄漏临时目录）；
`--skill` 的优先级与它自己的帮助文本相悖。

**本轮台账**：46 条，`open` 8 → 5，`fixed` 29 → 33，`fake` 4 → 5（新登记 B23）。
前后快照在
`docs/parity-snapshots/2026-11-01-v0.15-before-v0.16.json` 与
`docs/parity-snapshots/2026-11-02-v0.16-after.json`。

**新登记 B23**：`/export` 默认省略 thinking 内容（Pi 的导出保留）——那是有意的
产品决定（v0.8），但**导出器注释承诺的 `--include-thinking` flag 此前根本不存在**，
v0.16 把它做出来。这是跑 `gate.sh --full` 的导出对照时撞出来的。

**剩下的 8 条**（`open` 5 + `blocked-owner` 3）**都不是「还没人做」**：
B21 与 C4 缺一个**运行时扩展系统**（承载主体），B22 缺一个**布局引擎**，
C3/D5 与 B9 是 **provider 覆盖面与字段**（R5 待拍板），B1 是 `/clone` 的语义
（R10），B7 是语法高亮（R9）。每一缺什么写在台账的 `nini_state` 里。

**v0.17 这一轮（2026-11-05 ~ 11-06，随 v0.17.0 一起发布）**：

* **D5 的可实施部分**：`models.json` 里写的 provider/model 级字段此前**被 serde 直接丢掉**，现在接到请求上。
  载体落在 **provider 构造**上——`nini_core::provider::Request` 有 30 个构造点、**没有一个**用
  `..Default::default()`，其中 **20 个在 oracle/测试文件里**；给 `Request` 加字段会**打破 20 个
  oracle 的构造点**，违反「oracle 只能引用修复前后都存在的 API」。`Provider` trait 一行未改。
  三个 provider 共用一份 `HeaderOverrides::resolve`（不是三份实现各自对齐），合并顺序照 Pi：
  **provider 默认头 → provider 级 → model 级**，同名（大小写不敏感）后者覆盖。
  **两个 Pi 细节接住**：`authHeader` 在 Pi 的 schema 里是**布尔**（只认字符串会让那种文件整个被丢掉）、
  省略 `api` 时按 Pi 口径回退（自定义名字显式拒绝并点名 `api`）。
  **接线时发现的一个真坑**：注册表的 9 个「原生」条目不能当缺省 api——它的方言是 Pi 的
  `openai-responses`/`google-generative-ai`/`mistral-conversations`，而 nini 的 `openai`/`google`/
  `mistral` 说的是 `openai-completions`，照抄会让这三个 provider **建不起来** → 另有一张表。
* **R14 的最后一项**：`Alt+Up`（Pi 的键，nini 本来空着）→ 新动作 `DequeueMessages` → 复用既有的
  `return_queue_to_editor`（它已实现 Pi 的合并语义），反馈用 Pi 的原话。
* **顺带修掉一个静默坑**：R14 新增的三个动作没登记进 `keybindings_manager::parse_action` →
  用户在 `keybindings.json` 里写它们会被**静默忽略**（`parse_action` 返回 None，以为改绑了
  其实没变，且没有任何东西会告诉他）。已登记并加 oracle。
* **订正 r2a 的计划状态**：它 §0 的两个 P0 **早已被修掉**（真实 session 文件里 `parentId` 链式、
  `toolCall`/`toolResult` 行数 27–213；`/compact` 与自动压缩走 `session_compact` → `Session::compact`
  → `append_compaction`；resume 的转录由 `build_context_entries` 重建；A3 的阈值压缩走 session hook，
  `AgentEvent::Compacted` **保留**——从「空信号」变成「session 层压缩的可观察契约」）。该文档补了
  §5，把「**计划文档的 status 字段一旦写下就不会自己更新；读者会拿它当事实**」写成教训；
  `GOALS.md` §6 与 `parity-with-pi.md` §8 的台账行同步订正。
* **台账**：56 条，条数与 v0.16 之后相同（本轮改的是 D5 的 `nini_state`/`oracle` 与 B25 的映射，
  `map` 里 `nini: null` 从 4 降到 3，每条有 note）。D5 **收窄而不是关掉**：已交付的写进 `nini_state`
  与 `oracle`，**仍缺的**（`oauth`（不做，R4）、`modelOverrides`、model 级 `api`/`cost`/
  `samplingParams`/`thinkingLevelMap`、口径差异 ②）留在 `open`。
* **测试数 2043 → 2074**（本轮 +31）。
* 前后快照在 `docs/parity-snapshots/2026-11-05-v0.16-before-v0.17.json` 与
  `docs/parity-snapshots/2026-11-06-v0.17-after.json`。

### TUI 第一轮（v0.16.0 之后，随 v0.17.0 发布）—— 把「作者认为 Pi 长这样」变成「实测」

上一轮的结论是：nini 的 TUI **不缺功能，缺可判定的对照**。这一轮把三件事做掉。

| 交付 | 是什么 | 判据 |
|---|---|---|
| **键位语义层对照** | 屏幕级对照（`scripts/tui-oracle.py`）提取器不可用（§5.4），于是换一条路：**从源码取真相**——Pi 的 `KEYBINDINGS`（47 个 binding id）+ `TUI_KEYBINDINGS`（47 个，**去重后共 90 个**）逐条对 nini 的 `default_keymap()`。契约在 `scripts/keymap-parity.json`（唯一数据源），判定在 `scripts/keymap-parity.py` | `python3 scripts/keymap-parity.py` 退出 0（**新增门禁项**：gate 的链④ + CI 的 parity job）；`cargo test -p nini-tui --test keymap_parity` 退出 0（5 条，含**四种改坏方式各自变红**） |
| **五个浮层的帧级 oracle** | `model` / `settings` / `trust` / `help` / `palette` 此前没有帧级 oracle，`command_palette.rs` 全仓零测试引用 | `cargo test -p nini-tui --test selector_palette_frame --test selector_model_frame --test selector_settings_frame --test selector_trust_frame --test help_overlay_frame` 退出 0（22 条） |
| **两个真缺陷** | ① 浮层面板只清样式不清符号 → 转录从面板空行里透出文字（v0.5 时代的缺陷，注释一直声称已防住）；② 欢迎卡片读 `theme.fg_style("keyHint")`——**一个不存在的槽** → 落到默认色 = 白色 + 粗体 | 前者由 `the_panel_covers_the_transcript_it_sits_on` 钉住（去掉 `set_symbol` → 红）；后者由 `the_example_keys_use_a_theme_slot_that_exists` 钉住（**按颜色**断言，改回 `keyHint` → 红） |

**顺带**：R9 的体积估算被实测推翻（syntect **+0.63 MiB**，不是估的 +2–5 MB），
决策包见 [`r9-syntax-highlighting.md`](./r9-syntax-highlighting.md)；同时查清
**Pi 的终端确实做语法高亮**且用的正是那 9 个 `syntax*` 槽 → B7 是**真差距**。

**新登记**：**B24**（18 个主题槽悬空 + `keyHint` 读不存在的槽；legacy×6 是 nini
自创的已死 schema，删槽会**静默丢弃用户 theme.json 的值**，所以不是能顺手做的
清理）、**B25**（三处键位语义分叉：`Ctrl+T` 显隐 thinking 块 vs 档位 +、
`Ctrl+U` 删到行首 vs 清空输入框、`Ctrl+C` 清空/退出 vs 中止回合；另有 5 个 Pi
的键位 nini 没有）。**改键是用户可见的行为变更，本轮只登记不改**。

**再登记并关掉**：**B26**（`/resume` 重建 transcript 的三处失真：工具调用 / 工具结果 /
bash 执行全丢——`cmd_resume` 里那段 v0.8.9 之前的手写重建循环只取文本块，还把
`session_load` 刚建好的 live-context 副本**覆盖**成更差的纯文本副本；确认行
`(loaded session <id>)` 用 `push_assistant` 写进了 session 文件，于是每次 resume
积累一条假 assistant 消息，并在转录里粘连进最后一条 assistant 文本）。
判据：`cargo test -p nini-tui --test resume_rebuild` 退出 0（6 条，含两种变异各自变红）。

### provider/agent 层第一轮（v0.16.0 之后，随 v0.17.0 发布）—— 一次线上 400 把长任务打死

线上现象：用户的 provider 是讯飞星辰 MaaS（`~/.nini/agent/models.json` 的 `xfyun`，
`api = openai-completions`，baseUrl 以 `/v2` 结尾，模型 `spark-x2.6`）。跑一个长任务
（`cargo test` 跑了 218 秒、上下文累积很大）之后，nini 报
`[error] provider error: api error (400): request failed: 400 Bad Request` ——
**零信息**，任务直接死掉。

这一轮把这件事拆成**四个叠加的缺陷**（+ 两个附带修的），并逐条变成可判定的断言。

| 交付 | 是什么 | 判据 |
|---|---|---|
| **B27** | HTTP 错误响应体被整个丢弃：三个 provider 的非 2xx 分支只拼状态码，服务端原文与供应商错误码全丢 → 新增 `crates/nini-ai/src/error_body.rs`（有界读 body、提取结构化原文、保留错误码、剥 HTML、脱敏凭证）；`fallback` 的摘要不再二次丢弃原因 | `cargo test -p nini-ai --test error_body_wire` 退出 0（13 条）+ `error_body` 单元测试（22 条） |
| **B28** | HTTP 400 从不触发上下文溢出压缩重试：agent loop 有三条错误出口，**所有** provider 的非 2xx 都走第三条（`Some(Err(e))`），而它既不判溢出也不重试 → 抽出 `recover_from_overflow`，三条出口统一到同一套压缩重试语义；`overflow.rs` 新增**供应商错误码通道**（`OverflowSignals` / `VENDOR_ERROR_CODES` / `lookup_vendor_code` / `extract_vendor_error_code`） | `cargo test -p nini-core --test overflow_http_400` 退出 0（10 条）+ `overflow` 单元测试（12 条） |
| **B29** | 空文本 user 消息被序列化成 `content: ""`：讯飞回 400（code 10005）→ 按 Pi 0.85.1 的真实形状对齐（OpenAI 兼容发成 `" "`、Anthropic 丢弃空 user 轮、Responses 丢弃无 block 的 user 消息） | `cargo test -p nini-ai --test openai_empty_user_message` 退出 0（11 条） |
| **B30** | HTTP 路径不重试瞬时错误：429 / 5xx / 529 死在第一次 → `is_transient_status` / `is_transient_provider_error` / `begin_transient_retry`，状态码已知则状态码拍板，4xx 一律不重试 | `cargo test -p nini-core --test provider_http_retry` 退出 0（9 条） |

**关键事实（决定了 B28 的做法）**：讯飞的上下文超限 body 是
`{"error":{"code":10907,"message":"Bad Request"}}` —— **一个 overflow 词都没有**，
自然语言 pattern 的命中率是 **0%**。所以才必须有**结构化的供应商错误码通道**：
`10907` = 溢出（压缩重试），`10005`（参数非法）/ `10404`（模型不可用）= **不是**溢出
（压缩只会白丢历史）。

**B31 登记为伪差距（fake）**：这条错误码通道是 **nini 独有**的能力——Pi 的溢出判定
只有 stop_reason 与自然语言 pattern 两条通道，**Pi 同样识别不了讯飞的 10907**。
所以它不是「补 Pi 的欠账」，而是有意增强；登记在此是为了说清它**要人来养**
（厂商改码就得改表）。

**黑盒对照**（mock 讯飞服务器：第一次返回真实的 400 `{"error":{"code":10907,...}}`，
之后返回正常 SSE，跑编译出的二进制）：

| | 请求数 | 请求路径 | 结果 | 退出码 |
|---|---|---|---|---|
| 修复前 | 1 | `/v2/**v1**/chat/completions` | `api error (400): request failed: 400 Bad Request` | 1 |
| 修复后 | 2 | `/v2/chat/completions` | `OK`（自动压缩重试后交付） | 0 |

**顺带更正既有条目**：**A2** 此前写「**两条**溢出路径都发 Compacting 相位」——那是台账
写窄了：agent loop 共有**三条**错误出口，第三条（HTTP 错误出口）**从未**有压缩生命
周期，长任务在它上面直接死掉。A2 的标题与 oracle 随之更正（第三条出口由
`overflow_http_400.rs` 钉住）。

### provider/agent 层第二轮（同一轮未发布工作的独立审查）

上面那一轮写完之后，另有一个 agent 做了一次**独立审查**（它实际做了变异验证：把实现
改坏，看断言会不会红）。审查抓到了 2 个 Critical、9 个 Warning，其中两个 Critical 都是
**「本轮要修的 bug 被原样装回去，而测试仍然全绿」**——这类缺陷比没写测试更坏，因为它
让人以为这里有保护。

审查里有**两条**是真正的 Pi 对齐差距（Pi 有、nini 无），本轮关掉并登记：

| 交付 | 是什么 | 判据 |
|---|---|---|
| **B32** | 溢出恢复的一次性上限**永不复位**：Pi 在成功的 assistant 轮之后复位 `_overflowRecoveryAttempted`（`references/pi-0.85.1/src/session/agent-session.ts:697-699`），nini 只置位不复位 → 长任务里第二次上下文溢出就被「恢复失败」拒绝 | `cargo test -p nini-core --test overflow_recovery_parity` 退出 0（6 条）；变异注入（删掉复位点）→ 该文件变红 |
| **B33** | Anthropic 侧 **assistant 轮**的空白 text 未过滤：Pi 对 assistant 轮同样 `if (block.text.trim().length === 0) continue;` 且空则整轮丢弃（`references/pi-0.85.1/src/api/anthropic-messages.ts:1276-1330`），nini 只过滤 user 轮 → 会发出 `{"role":"assistant","content":[{"type":"text","text":""}]}` 这种 Anthropic 会拒的形状 | `cargo test -p nini-ai --test anthropic_blank_blocks` 退出 0（5 条）；变异注入（assistant 不参与过滤）→ 该文件变红 |

其余的审查项（W1–W9 里剩下的、以及端到端抓到的那次回归）是 **nini 自己的正确性问题**，
不是与 Pi 的差距，因此**不登记进台账**——台账记的是「与 Pi 的差距」，把 nini 的内部
健壮性改进塞进去会让这张表失去意义。它们的完整记录在 `CHANGELOG.md` 的「独立审查后的
修复」一节。

**留了一条没修**（如实记录，不假装关掉）：厂商错误码查表**不看厂商**——`10907` 对别的
厂商也可能被读成溢出。本轮不做「按厂商选表」这种过度设计，风险写进 CHANGELOG 的
「留给 owner 的」。

### v1.1 —— `/resume` 改成交互式会话选择器（B34）+ Pi 选择器四件套（B35）（2026-11-09）

`/resume` 此前是一堵文本墙：最多 50 行 `N: <uuid>.jsonl [38762 bytes, …]`，认不出是
哪个会话；而且这些行是 `CommandOutcome::Output`，运行时会用 `push_assistant` 把它们
写进 session 文件——清单被当成了助手说过的话。Pi 的 `/resume` 只开浮层、不打印清单
（`references/pi-0.85.1/src/ui/interactive-mode.ts:3094-3098`）。

| 交付 | 是什么 | 判据 |
|---|---|---|
| **B34** | `/resume` 开浮层，行内认得出是哪个会话（名字 → 首条用户消息 → `(no messages)`，附相对时间与项目目录名）；`/resume <query>` 改为**开浮层并预填 query**（口径 A，与 Pi 一致，旧行为是「打印匹配清单」）；命令不再返回任何 Output 行，清单不再进 session 文件 | `cargo test -p nini-tui --test resume_selector` 退出 0（12 条）；`cargo test -p nini-tui --test slash_command_e2e` 退出 0 |
| **B35** | Pi 选择器的四件套做掉：scope 切换（当前目录 / 全部，Tab）、排序模式（threaded / recent / relevance，Ctrl+S）、名字过滤（Ctrl+N）、路径列（Ctrl+P）、删除会话（Ctrl+D，要确认，当前会话不可删）、重命名会话（Ctrl+R，写成追加的 `session_info`，文件名不动）。行内容对齐 Pi（消息数 + 相对时间 + 项目目录），空态按 scope 与名字过滤给不同提示；`/resume <n>` 与选择器共用 `picker_order`（第 n 行 = 第 n 个会话，threaded 的树也算数） | `cargo test -p nini-tui --test resume_selector_pi` 退出 0（14 条）；`cargo test -p nini-tui --test resume_selector` 退出 0（12 条） |

**两处有意偏差**（写在这里，免得被当成 bug）：

1. **scope 默认是「全部」，不是 Pi 的「当前目录」。** 浮层的第 n 行必须与 `/resume n` 是
   同一个会话（B34 锁住的性质）；从「当前目录」起步会让行号与 `/resume n` 错位。
2. **Pi 的 `re:` 正则搜索没有实现。** nini-tui 没有 regex 依赖，为一个搜索模式加依赖
   不划算；这样的查询被当成普通词处理。

---

## 5. 门禁与验证口径

### 5.1 交付门禁

`scripts/gate.sh` 是唯一判定脚本，锁两条基线：**裸奔不变量数不得增加**、**测试数不得减少**。
本轮新增 `fix:` 提交会改变不变量总数与测试数，因此：

> **基线刷新规则**：先确认 `invariants_none` 未增加、测试数未减少，再用
> `bash scripts/gate.sh --update-baseline` 显式刷新，并把刷新前后的 JSON 记入
> `docs/parity-snapshots/`。**禁止修改 `scripts/gate.sh` 与 `scripts/invariants.py` 的判定逻辑**
> （它们是「门禁本身要可信」的一部分）。

### 5.2 每轮必须复算的口径（全部由脚本产出）

| 指标 | 命令 |
|---|---|
| 差距总数 / 按类 / 按状态 / 按批次 | `python3 scripts/parity-gap.py --json` |
| 机器可判定判据占比 | 同上（`machine_checkable` / `not_machine_checkable`） |
| 台账锚点（每条 gap 的 `nini_state` 是否指向真实存在的符号） | `cargo test -p nini-core --test ledger_anchors`（Rust；跑在标准测试门禁里） |
| 行号引用总数与分布 | `python3 scripts/pi-line-refs.py --json` |
| 快照一致性 + 行号解析 | `python3 scripts/pi-oracle-lock.py --check --verify-refs` |
| 裸奔不变量数 | `python3 scripts/invariants.py --json` |
| 测试数 | `cargo test --workspace --all-targets` |

**单调性要求**：A 类每轮只减不增；`machine_checkable` 每轮只增不减。
任一指标反向，说明这一轮引入了新的不确定性——**这比没修更糟**。
反向发生时必须在本文件 §9 记录，不得静默。

---

## 6. 明确不做（范围决策已定，不要重开）

| 项 | 理由 |
|---|---|
| 扩展运行时 | 已被证伪：Pi 加载 TS，`nini-ext` 是 `dlopen` cdylib，无共同执行模型 |
| OAuth / 设备流 | 产品决策；对应 API key 登录可另议 |
| 命令行 session 参数 | `--continue-session` 等已显式拒绝并指向 TUI 命令 |
| LLM 分支摘要 | 明确不做 |
| prompt 模板 / AGENTS.md | 明确拒绝并说明，不是静默忽略 |

---

## 7. 决策台账（owner 拍板，实现者不自行决定）

| # | 决策 | 现状 | 阻塞什么 |
|---|---|---|---|
| R4 | nini 是否自建凭据存储 | 待决（`6abadd9` 已整体回滚一次） | 整个 auth 层；`/login` 的 Pi 形态 |
| R5 | provider 是否补齐到 Pi 全集 | 待决。**v3 实测：Pi 0.84.2 内置 40 个，nini 原生 9 个实现（7 个品牌）**，v2 文档写的「51 / 5」两个数都是错的 | `nini-ai` 的方向；B9、**C3** |
| R9 | 语法高亮是否值得 +2–5 MB 体积 | **已拍板：本轮不做**；判据改成「9 个槽各能出对应颜色」。体积早已实测（syntect+fancy-regex +0.63 MiB / onig +0.90 / 自研 +0.003），但真正的大头是 **scope→槽映射**，且 **Pi 的高亮实现不在快照里**（`src/utils/` 整个目录缺失）→ 逐行对齐不可验证。见 `plans/r9-syntax-highlighting.md` §4 | B7 → fake |
| R10 | `/clone` 改为「切到新分支」是否可接受 | 待决。**Pi 的 `/clone` 没有参数**（`references/pi-0.85.1/src/ui/interactive-mode.ts:5186` 的 `handleCloneCommand(): Promise<void>`） | B1 |
| **R11** | 要不要做 Pi 兼容的**扩展运行时**、由谁承载 | 待决（R6 只证伪了 `nini-ext` 的技术路径；产品层面仍开放） | **B21**、**C4** |
| **R12** | mermaid **真图**渲染引不引布局引擎 | 待决（Pi 0.85.1 用 npm 包 `grok-mermaid` 0.2.2） | **B22** |
| **R13** | 主题槽接线策略：边框 UI / `customMessage` / legacy 槽删还是留 | 待决（复算：61 个槽，**27 个零读取** = 9 个 `syntax*` + **18 个**） | **B24** |
| **R14** | 键位语义与 Pi 对齐的口径 | 待决（改键是用户可见行为变更） | **B25** |
| — | clean-room 表述 vs 读 Pi 源码的方法论 | **已决（2026-11-07，改表述）**：README 改成「代码是从零用 Rust 写的，没有翻译或抄写 Pi 的任何一行 TypeScript」——去掉「不看 Pi 的源码」这个**与事实不符**的说法（仓库里固定着 Pi v0.85.1 的 422 个源码文件，340 条符号引用指向它） | 已闭合，见 §9.2 第 3 条 |

**R5 / R10 / R11 / R12 / R13 / R14 的一页纸决策材料**（事实 + 选项表 + 工作量 + 推荐 +
「owner 什么都不做的代价」）在 [`plans/owner-decision-packs.md`](./owner-decision-packs.md)；
R9 的在 [`plans/r9-syntax-highlighting.md`](./r9-syntax-highlighting.md)。

**2026-11-05：台账的 `status` 字段与自己的 `exit_criterion` 对齐**。此前 6 条条目
（B21/B22/B24/B25/C3/C4）的 `status` 写着 `open`，而它们**自己的判据就写着「要先有
owner 决策」**——字段与判据打架，读者无法判断哪条是真的能动手。已改挂决策号并转为
`blocked-owner`（C3→R5，B21/C4→R11，B22→R12，B24→R13，B25→R14）。**现在 open 只剩
D5 一条**，它的判据明写「在此之前本条保持 open」。

---

## 8. 与既有计划的关系

| 文档 | 处置 |
|---|---|
| 本文件 | **active**：功能面 parity 的唯一执行细则（v3） |
| `plans/input_interrupt_parity.md` | 本文件 W1 的展开；§8.8 状态字段已按实际修正（C2） |
| `plans/v0.10-pi-parity-refactor.md` | R1–R8 历史路线，**只读** |
| `plans/r2a-compaction-persistence.md` | **shipped**（2026-11-05 复核）：A1–A4 与 §0 的 P0.1/P0.2 全部交付；它曾长期被标成 `active/blocked`，而阻塞它的两个 P0 早已被修掉——计划文档的 status 字段不会自己更新，其读者会拿它当事实 |
| `plans/owner-decision-packs.md` | **active**：R5/R10/R11/R12/R13/R14 的一页纸决策材料（只提供事实与选项，不替 owner 决定） |
| `plans/v0.6*`–`v0.9.2*`、`tui-*.md` | frozen，不再追加 |

---

## 9. 已知不确定性（v3 更新）

**保留它们比删掉更有用**：这正是 W0 要解决的那类问题——读代码得出的结论会漂。

### 9.1 已被推翻（不再重开）

| # | 旧说法 | 实测 |
|---|---|---|
| 1 | 行为层 oracle = 0 | 66 条（`#[test]` + `#[tokio::test(...)]` 都要数） |
| 2 | 参数提示逐条一致，ARG 差异 0 处 | 6 处差异 |
| 3 | A1 会损坏 session JSONL | `O_APPEND` 单行写入原子，真实后果是**顺序交错 + TokenStats 混算 + 事件乱序** |
| 4 | Pi 有 51 个 provider、nini 有 5 个 | Pi 0.84.2 **40 个**、nini **9 个实现**；v3 已按实测写 |
| 5 | nini 侧 `cmd_export` 在 291 行、`cmd_clone` 在 727 行 | 实为 **300 / 863**（而且还会继续漂）；**C2 已闭合**：台账改为只写**符号**（`cmd_export` / `cmd_clone`），行号由 `crates/nini-core/tests/ledger_anchors.rs` 与 `crates/nini-core/tests/ledger_anchors.rs` 核对 |

### 9.2 仍未闭合（实现时必须验证，不得假设）

| # | 缺口 | 何时验证 |
|---|---|---|
| 1 | Pi 压缩期提交是抛错还是排队（两条路径都存在） | W1-B 动手前先录 Pi 行为 |
| 2 | 全部行号引用中，有多少是**语义漂移**（行号在范围内但内容已不是那件事） | W0 剩余项 1：`--verify-refs` 打印逐条内容供人工复核 |
| 3 | ~~README 的「clean-room 重写（……**不看 Pi 的源码**）」与本计划的方法论（读 Pi 源码并引用行号、录真 Pi fixture）**互相矛盾**~~ | **已闭合（2026-11-07，owner 拍板：改表述）**。两个口径的后果对比：① **改表述**（选它）——README 改成「代码是从零用 Rust 写的，**没有翻译或抄写 Pi 的任何一行 TypeScript**」。这是**事实**：nini 是 Rust，Pi 是 TypeScript，不可能「抄」；而「不看 Pi 的源码」**本来就是假的**——`references/pi-0.85.1/` 里固定着 422 个 Pi 源码文件，340 条符号引用指向它，CI 每次门禁重新解析。代价：丢掉一个听起来更响亮的说法。② **改方法论**（不选）——删掉源码快照、删掉全部符号引用、oracle 只能录真 Pi fixture。代价是**把项目最强的资产扔掉**：正因为固定了源码，才能证明「nini 说的 Pi 行为」不是猜的。新的表述同时成了**致敬**：README 的 Credits 一节写明「Pi v0.85.1 的源码被固定在本仓库里，340 条符号引用每次门禁重新解析」——把方法论从「藏起来的矛盾」变成**公开的致敬**。|
| 4 | synctact 引入后的真实体积增量 | W4（R9 决策依据） |

### 9.3 方法论结论（不变）

**「我读过了」不构成证据。** v3 把差距、计数、行号、快照哈希全部交给脚本；
人工只保留一件事：**把脚本解析出来的内容与自己的理解对上**——并把核对结果写进
`docs/invariant-verification.md`。

---

## 10. v3 修订记录（13 处漏洞的修法映射）

| # | 漏洞（v2） | v3 修法 | 落地位置 |
|---|---|---|---|
| 1 | W0 的 (c) 分支允许把 P0 降级到 B 类之后 | (c) 分支明令**不得顺延 A1**；改为 `blocked-external` + 快照校验降级路径 | §4 W0 未交付 3 |
| 2 | A2 判据依赖未闭合前提 | W1-B 判据第 4 条要求先产出 Pi 行为记录 | §4 W1-B |
| 3 | v1 已废弃的「fixture 为红」写法复活 | 判据拆成「Pi 侧 fixture 存在」+「nini 侧断言当前为红」两件事 | §4 W0 未交付 2 |
| 4 | §3 自定的台账脚本不存在，全部指标手工计数 | `scripts/parity-gaps.json` + `parity-gap.py` 交付，§2/§5 只引用脚本输出 | §2、§5.2 |
| 5 | 与 GOALS 的排序规则冲突、两个「唯一列表」 | §3 合并为唯一版本；GOALS 同步改写 | §3 + `docs/GOALS.md` |
| 6 | W1-A / W1-B 改同一状态机却分批 | 改为**同批落地、同一轮验证** | §4 W1-B |
| 7 | 「Pi 能否离线触发」被当二元问题 | 给出可行解：`scripts/mock-provider.py` + 临时 `models.json` 指向本地端点 | §4 W0 未交付 2 |
| 8 | 判据含不可复算的人工项（tmux 实测） | 人工项降级为辅助确认；主判据改为脚本判定 | §4 W1-A/W2 |
| 9 | W2 判据对 owner 阻塞项不可达 | 补「owner 决定不做」的关闭条件 | §4 W4 |
| 10 | 计划与 `gate.sh` 的交互缺失 | §5.1 写明基线刷新规则与禁令 | §5.1 |
| 11 | oracle 迁移成本低估（v2 只数了 `.js` 那一半） | 数字按实测改写，并给出 W0 剩余项的逐条清单 | §4 W0、§9.2 |
| 12 | 行为层对照物无版本锁 | `references/pi-0.85.1/` + `SNAPSHOT.lock` + `pi-oracle-lock.py` | §1、§4 W0 |
| 13 | `/share` stub 不在 B 表里；parity 声明把「名字齐」当「语义齐」 | 新增 B10；分类口径明写 | §2 |
