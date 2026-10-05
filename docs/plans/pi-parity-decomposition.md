# nini ↔ Pi 对标拆解（基于代码关系知识图谱）

> **日期**：2026-10-05
> **基准**：本机实装 Pi **0.84.2**（`@earendil-works/pi-coding-agent` + `pi-agent-core` 等 7 包）
> **方法**：Pi 的 TypeScript 源码从 `dist/*.js.map` 的 `sourcesContent` **完整还原**（487 文件 / 3.6 MB / 零缺失），
> 两侧用 `understand` skill 的确定性脚本（`scan-project` / `extract-import-map` / `extract-structure`）建图，
> 再按**职责**对齐，不按名字对齐。

图谱落盘：

| 侧 | 路径 | 规模 |
|---|---|---|
| nini | `.understand-anything/knowledge-graph.json` | 1720 节点 / 2197 边（115 Rust 文件） |
| Pi | `~/pi-agent-graph/knowledge-graph.json` | 2702 节点 / 5234 边（485 TS 文件） |

---

## 1. 子系统级对标

| Pi 子系统 | 节点/边 | nini 对应 | 节点/边 | 比值 |
|---|---|---|---|---|
| `pi-ai` | 838 / 2626 | `nini-ai` | 178 / 366 | **0.21x** 🔴 |
| `pi-coding-agent` | 1189 / 3495 | `nini-cli` (+部分 tui) | 873 / 2101 | 0.73x 🟠 |
| `pi-agent-core` | 328 / 900 | `nini-core`+`session`+`tools` | 436 / 928 | 1.33x 🟢 |
| `pi-tui` | 253 / 698 | `nini-tui` | 809 / 1938 | **3.20x** ⚠️ |
| `pi-client`/`protocol`/`telemetry` | 93 / — | **无** | 0 | **0x** 🔴 |

**总计：nini 1720 节点 = Pi 的 0.64x**

### 1.1 最反直觉的一条：投入方向反了

**`nini-tui` 是 `pi-tui` 的 3.2 倍，而 `nini-ai` 只有 `pi-ai` 的 0.21 倍。**

也就是说：nini 把工程量压在**终端渲染层**，却严重欠配**模型/Provider 层**。对一个 agent 来说，
后者才是产品本身 —— TUI 再漂亮，模型接不上、登录不了、协议覆盖不了，都不成立。

这一条推翻了我此前所有基于"用户可见假象"的优先级排序（`/copy` 假成功、`/logout` 假移除等）。
那些是真 bug，但**量级远小于此**。

---

## 2. `nini-ai` 缺口分解（最大缺口）

Pi 的 `pi-ai` 内部是**两层**结构，nini 是**一层**：

```
Pi pi-ai/src/
├── api/          353 节点 · 20 个线路协议(dialect)
├── providers/    177 节点 · ~37 个品牌，每个带 .models.ts 目录
├── auth/         130 节点 · 其中 auth/oauth 占 111
└── utils/        104 节点

nini-ai/src/      15 个平铺文件，无子目录
```

### 2.1 缺口一：线路协议层缺失 🔴

Pi 的 `api/` 20 个 dialect，**每一个都是独立的 wire format**：

```
anthropic-messages        openai-responses        openai-completions
bedrock-converse-stream   google-generative-ai   google-vertex
mistral-conversations     openai-codex-responses  azure-openai-responses
constrained-sampling      transform-messages      openai-prompt-cache
cloudflare               openrouter-images       pi-messages        …
```

nini 把**品牌**和**协议**混为一谈：`openai.rs` 一个文件同时承担 OpenAI 品牌 + responses 协议 + completions 协议。
后果是加一个走 OpenAI 协议的新品牌（如 `github-copilot`、`opencode`、`kimi-coding`）必须复制整个文件，
而 Pi 只需新增一个 `.models.ts` 声明 + 复用已有 dialect。

### 2.2 缺口二：Auth / OAuth 整层为零 🔴

`auth/` 130 节点（`auth/oauth` 独占 111）在 nini 中**完全没有对应物**。
`/login` 只能打印 env var 存在性检查 —— 不是"简化版"，是**这一层不存在**。

### 2.3 缺口三：模型目录层为零 🟠

Pi 每个品牌都有 `<brand>.models.ts`（37 个）。nini 只能从 `~/.pi/agent/models.json` 读，
**没有内置目录**，所以 `nini --list-models` 依赖用户是否写好了 models.json。

---

## 3. `pi-client` / `pi-protocol` / `pi-telemetry`：93 节点，nini 为 0

远程 client、协议层、遥测。`nini-ext` 是空的扩展运行时，不构成对应物。

---

## 4. 与前序调研结论的关系（哪些被推翻）

| 前序结论 | 现在的判定 |
|---|---|
| "P0-1 skills 注入" | ✅ 仍是真 bug，但**量级属于 P2**（单点字段缺失） |
| "`/copy` `/logout` 假成功" | ✅ 仍是真 bug，**仍是 P1**（用户直接受损） |
| "项目信任 check() 零调用" | ✅ 仍是真 bug，属 `pi-coding-agent` 0.73x 那部分 |
| "agent loop 缺 L1 外层" | ✅ **被图谱提升为最高优先级之一** —— `pi-agent-core` 是 nini 唯一超过 1x 的子系统，但缺口在最核心的循环语义上 |
| "`nini-ext` 是死代码，删掉" | ⚠️ **再次确认是错的**。Pi 有完整扩展运行时（`pi-coding-agent` 内），nini 的是空壳。**该接线，不该删** |
| "`branch_summary` 448 行死代码" | ⚠️ 同上 —— Pi 有 `branch-summarization`，nini 实现后从不调用。**该接线** |
| "G1 缺 `ls` 工具" | ❌ **作废**。0.84.2 的 `harness/tools/` 里没有 `ls`，`createLsTool` 搜不到。上一轮我是拿 0.85.1 的 spec 目录提的要求，**对着本地不存在的目标** |
| "G2 缺 file-mutation-queue" | ✅ 成立（0.84.2 有 `file-mutation-queue.js`） |

---

## 5. 重构建议（按图谱证据排序）

| 序 | 动作 | 依据 | 规模 |
|---|---|---|---|
| **R1** | **agent loop 补 L1 外层**（follow-up 触发新 turn，而非塞进历史当死上下文） | `pi-agent-core` 核心语义，nini 唯一 >1x 的子系统里的核心缺口 | 中 |
| **R2** | **`nini-ai` 拆两层**：`api/`（dialect）+ `providers/`（brand） | 0.21x 最大缺口；拆开后加品牌从"复制文件"降为"加声明" | 大 |
| **R3** | **接 `branch_summary` + `operations` 到 agent loop** | 已有 780 行实现，从未调用。Pi 有对应物 | 小（接线） |
| **R4** | **建 auth 层**（至少 env-key 表 + 可插拔 credential provider，为 OAuth 留位） | `auth/` 130 节点为零 | 中 |
| **R5** | **建模型内置目录**（`<brand>.models.ts` 等价物） | 37 个 vs 0 | 中 |
| **R6** | **`nini-ext` 接线到扩展运行时** | Pi 有，nini 是空壳 | 大 |
| **R7** | session serde 对齐（`stopReason`/`thinkingLevel` camelCase + `active_tools_change`） | 339 个真实 session 丢 48% | 小 |
| **R8** | TUI 层**暂不投入** | 已 3.2x，边际收益最低 | — |

**注意 R8 是反直觉的一条**：按直觉应该继续打磨 TUI（那是 nini 唯一超过 Pi 的地方），
但图谱说明那里已经过剩，而 `nini-ai` 只有 0.21x。

---

## 6. 本文档自身的已知局限

1. 节点数是**代理指标**，不是功能等价度。3.2x 不代表 nini-tui 更好，只代表更碎/更多。
2. `nini-tui` 在上表中被 `pi-tui` 和 `pi-coding-agent` 两次计入，比值需按单次口径重算才严格。
3. 本轮**未**逐函数比对 `pi-coding-agent`（0.73x）的内部构成，只知总量缺口。
4. 结论基于 **0.84.2**。若目标改为 0.85.1，第 2.1 节的 dialect 清单需重取。
