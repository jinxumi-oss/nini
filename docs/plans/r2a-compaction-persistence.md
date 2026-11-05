# R2a — compaction persistence: ground truth, gap, and the fork

Status: **全部交付（2026-11-05 复核）**：A1–A4 与 §0 的 P0.1 / P0.2 均已落地。

本文档 §0 的 P0 描述与 §3 的「Still open」表**已过期**——写它们的时候属实，
后来被修掉了，但状态字段没有跟着更新。逐条证据与更正见 §5。

当时的状态（2026-10-07 写下，已过期，留痕）：

> Status: **A1 shipped** (`c249d9b`). A2 blocked by P0 below.

Baseline: 1019 tests pass, clippy 0, fmt clean.（2026-10-07 的基线，未随之更新）

---

## 0. P0 — two defects that outrank compaction

Found while scoping Option A. Both proven by execution, not by reading.

### P0.1 Tool calls and tool results are never persisted

A full turn — user message, tool call, assistant text, tool result — leaves
**two** entries in the session file: the user text and the assistant text.
The `toolCall` and `toolResult` blocks are absent entirely.

Evidence:

- `state.rs:1516` `push_tool_result` calls only `push_tool_result_raw` and
  carries the comment *"Tool results are part of the turn — session_append is
  handled on TurnEnd"*. Nothing handles them.
- `runtime.rs:110` `AgentSink` holds `state` and `notify` and **no session
  handle at all**, so it cannot persist anything on TurnEnd either.
- `runtime.rs:241` calls the `_raw` variant, documented as transcript-only.
- `session_append` has exactly two call sites (`state.rs:1426`, `:1432`),
  both plain text.
- No production code constructs a `ToolUse` or `ToolResult` `SessionEntry`.

Consequence: on `/resume` or restart the model sees user turns and assistant
prose with the entire tool history gone.

**后来的结论（2026-11-05 复核）：已修。** 提交 `69d2d9e`
（*fix(tui): the agent's half of a conversation reaches the session (P0.1)*，
2026-10-07 17:49）。

- 写入路径：`crates/nini-tui/src/state.rs:2306` `session_append_tool_result`、
  `:2377` `session_append_bash_execution`；生产调用点在
  `crates/nini-tui/src/runtime.rs:428`（工具结果）与
  `crates/nini-tui/src/state.rs:2923`（`!cmd`）。
- 验证：`crates/nini-tui/tests/p01_persist.rs`，6 个用例全过（含
  `a_turn_with_a_tool_call_round_trips_in_pis_shape`、
  `the_persisted_turn_survives_a_restart`）。
- 真实 session 文件复核：`~/.nini/agent/sessions/nini/` 下
  `20261104-024403_1d1613c2e.jsonl` 有 27 行 `toolCall`、15 行 `toolResult`；
  `20261104-012913_1be10e894.jsonl` 有 213 行 `toolCall`、115 行 `toolResult`
  （`grep -c 'toolCall' <file>`）。

复算命令：

```bash
grep -c 'toolCall' ~/.nini/agent/sessions/nini/20261104-024403_1d1613c2e.jsonl
grep -c 'toolResult' ~/.nini/agent/sessions/nini/20261104-012913_1be10e894.jsonl
```

### P0.2 Every entry has `parentId: null`, so there is no tree

A session with 4 entries yields a `get_path()` of length **1**.

`Session::get_path` finds the leaf as "an entry no other entry parents to".
With `parentId: null` on everything, every entry qualifies, and the newest
wins. The path is a single entry.

Consequence: `get_path`, tree/branch selection, `/resume` leaf choice, and
Pi's `buildContextEntries` are all non-functional on any session nini has
ever written. `firstKeptEntryId` refers to a tree that does not exist.

**后来的结论（2026-11-05 复核）：已修。** 提交 `1dbf9d1`
（*fix(session): a session is a tree, so the tree-shaped work can run (P0.2)*，
2026-10-07 18:25）。

- 写入路径：`crates/nini-session/src/lib.rs:394` `append_chained`——在
  `:396` 调 `entry.set_parent_id(parent)`，父节点取当前 leaf，Pi 语义。
  TUI 侧的调用点是 `crates/nini-tui/src/state.rs:2415`。
- 旧文件的回填：`crates/nini-session/src/lib.rs:527` `link_flat_entries`
  （在读入时按文件顺序补 null 的 `parentId`，不改写已有真实父节点），
  调用点 `:751`。
- 验证：`crates/nini-tui/tests/p02_tree.rs`，5 个用例全过（含
  `a_new_session_is_written_as_a_chain`、
  `a_flat_legacy_file_is_linked_in_file_order`）。
- 真实 session 文件复核：`~/.nini/agent/sessions/nini/20261104-024403_1d1613c2e.jsonl`
  里 `parentId` 是链式的——第一条 `parentId: null`，第二条指向第一条的 id，
  其余逐条指向前一条；无悬挂父节点。

复算命令：

```bash
python3 - <<'EOF'
import json
f = '/home/jin/.nini/agent/sessions/nini/20261104-024403_1d1613c2e.jsonl'
msgs = [json.loads(l) for l in open(f) if json.loads(l)['type'] == 'message']
print('root parentId:', msgs[0]['parentId'])
print('chained:', all(msgs[i]['parentId'] == msgs[i-1]['id'] for i in range(1, len(msgs))))
EOF
```

**These outrank compaction.** A compaction that walks a tree needs the tree.

---

## 1. Pi 0.84.2 ground truth

Source: `/home/jin/pi-src-reconstructed/pi-coding-agent/src/core/session-manager.ts`
(Pi 0.84.2 recovered from its own sourcemaps).

### 1.1 On-disk entry

```ts
export interface CompactionEntry<T = unknown> extends SessionEntryBase {
	type: "compaction";
	summary: string;
	firstKeptEntryId: string;   // REQUIRED — not optional
	tokensBefore: number;
	details?: T;
	usage?: Usage;
	fromHook?: boolean;
}
```

### 1.2 Write

```ts
appendCompaction(summary, firstKeptEntryId, tokensBefore, details?, fromHook?, usage?): string {
	const entry = {
		type: "compaction",
		id: generateId(this.byId),
		parentId: this.leafId,          // <-- chained to the leaf
		timestamp: new Date().toISOString(),
		summary, firstKeptEntryId, tokensBefore, details, usage, fromHook,
	};
	this._appendEntry(entry);
	return entry.id;
}
```

### 1.3 Read

```ts
// buildContextEntries, session-manager.ts:415
const compaction = last compaction entry on the leaf path
if (!compaction) return path
contextEntries = [compaction]
scan path[0..compactionIdx] FORWARD; start collecting once
  entry.id === compaction.firstKeptEntryId          // flag set BEFORE push
contextEntries.push(...path.slice(compactionIdx + 1))
```

Live context = `[compaction] + (tail from firstKeptEntryId) + (everything after)`.

---

## 2. What shipped (A1)

- `Session::append_compaction` — mirrors Pi's writer, parents at the leaf.
- `build_context_entries` — mirrors Pi's reader.
- Oracle `a_compaction.rs`, 7 tests. Four injected regressions each fail
  distinct tests (first-compaction-wins / kept-entry dropped / parented at
  root / `firstKeptEntryId` left null).

The algorithm is Pi-equivalent and verified.

**更正（2026-11-05 复核）**：原文写「**It is currently unreachable in
production**, because it consumes a tree that nini does not write (P0.2).」
——**当时**属实：写下这句话时 nini 写的 session 确实没有树。P0.2 修掉之后，
它就可达了；而 A2（`/compact` 与自动压缩接到 session 层）与 A4（resume 时
从 `build_context_entries` 重建转录）随即把生产路径挂到了它上面。今天
`/compact` 写出的就是一条真的 CompactionEntry，`build_context_entries`
读回来的也正是它。

验证：`crates/nini-tui/tests/a2_compact.rs` 的
`compact_writes_a_real_compaction_entry` 与
`the_transcript_is_rebuilt_from_the_session_context`（实测 5/5 通过）。

---

## 3. 当时的 Still open 与今天的归宿

下表的「当时的描述」一栏是**原文照录**（2026-10-07），留痕用；状态以
2026-11-05 的复核为准。

| id | 当时的描述 | 今天的状态 | 证据 |
|---|---|---|---|
| P0.1 | Persist tool calls and tool results | **已交付**（`69d2d9e`, 2026-10-07 17:49） | `crates/nini-tui/src/state.rs:2306` `session_append_tool_result`、`:2377` `session_append_bash_execution`；调用点 `crates/nini-tui/src/runtime.rs:428`。测试 `crates/nini-tui/tests/p01_persist.rs` 6/6 通过。真实 session 文件有 27–213 行 `toolCall`（见 §5） |
| P0.2 | Chain `parentId` so `get_path` returns the real history | **已交付**（`1dbf9d1`, 2026-10-07 18:25） | `crates/nini-session/src/lib.rs:394` `append_chained` → `:396` `set_parent_id`；旧文件回填 `:527` `link_flat_entries`。测试 `crates/nini-tui/tests/p02_tree.rs` 5/5 通过。真实 session 文件的 `parentId` 已链式 |
| A2 | Delete `auto_compact_local` and `cmd_compact`'s transcript slicing; route `/compact` and auto-compact through the session layer | **已交付**（`d20fb61`, 2026-10-07 20:09） | 全仓已无 `fn auto_compact_local`（`grep -rn "fn auto_compact_local" crates/` → 0）；`crates/nini-tui/src/state.rs:3144` 的注释明写「Replaces `auto_compact_local`」；`crates/nini-tui/src/commands/dispatch.rs:1276` `cmd_compact` → `:1285` `state.session_compact(keep)` → `crates/nini-tui/src/state.rs:3151` → `crates/nini-session/src/lib.rs:347` `Session::compact`（写出真的 CompactionEntry）。自动压缩路径 `crates/nini-tui/src/runtime.rs:3549`（`g.session_compact(keep)`；该文件正在被并行修改，行号会漂移，符号名 `session_compact` 更稳）。测试 `crates/nini-tui/tests/a2_compact.rs` 5/5 通过 |
| A3 | Move the Agent's in-loop trigger (`agent.rs:893`, `:1136`) out to the driver; drop `AgentEvent::Compacted` | **已交付，但后半句被更好的设计取代**（`8cc4f2f` + `d1e4b2a`, 2026-10-07 22:17 / 23:05） | 阈值压缩走 `crates/nini-core/src/agent.rs:1511` `maybe_compact_at_threshold()` → `:1440` `compact_history_via_hook()`（session hook，不是就地改内存）；`:1751` 的注释明写「the threshold compaction that used to live here is gone」。**`AgentEvent::Compacted` 保留**（`agent.rs:170`）：它从「没人消费的空信号」变成了「session 层压缩发生了」的可观察契约，被 4 个测试文件断言（`crates/nini-core/tests/` 下的 `compaction_lifecycle.rs`、`overflow_http_400.rs`、`overflow_recovery_parity.rs`、`a3b_overflow.rs`；另有 `crates/nini-tui/tests/a3_seed.rs`、`compaction_e2e.rs` 在注释里记录了这一取舍）。hook 的生产装配在 `crates/nini-cli/src/app.rs:683` `set_compaction_hook` → `:691` `g.session_compact(keep)` |
| A4 | Rebuild the transcript from `build_context_entries` on resume | **已交付**（`6683f36`, 2026-10-08 09:09） | `crates/nini-tui/src/state.rs:3171` `transcript_from_context()` → `:3176` `nini_session::build_context_entries(&guard)` → `render_context_entries(&context)`；resume 路径 `crates/nini-tui/src/state.rs:2484`（`session_load`）。测试 `crates/nini-tui/tests/a4_resume.rs` 4/4 通过（含 `loading_shows_the_live_context_not_the_raw_entries`） |

### A2's two fakes（当时的描述，已被 `d20fb61` 修掉）

`state.rs:1627 auto_compact_local` and `dispatch.rs:936 cmd_compact` both fold
**transcript lines** at `total / 2` — no token budget, no session write, no
effect on what the model sees. `cmd_compact`'s comment claims it "matches the
algorithm used by the auto-compaction path", which is false: auto uses a
token estimate, then the same `/2`.

> **后来的结论（2026-11-05 复核）**：两个 fake 都不存在了。
> `auto_compact_local` 这个函数已从全仓删除，`cmd_compact` 现在走
> `state.session_compact(keep)`，切点来自 token 预算，结果是一条真的
> CompactionEntry。原文点名的两个坐标（`state.rs:1627`、`dispatch.rs:936`）
> 已随其它改动漂移，不再指向那两个函数。

---

## 4. Order

### 当时的顺序

P0.1 → P0.2 → A2 → A3 → A4.

P0.2 changes `get_path` semantics for every existing session, so it needs a
migration decision: nini sessions written with flat `parentId: null` cannot be
distinguished from branched ones except by inspecting for any non-null parent.

### 实际执行顺序（2026-11-05 复核）

实际的提交顺序与上表**一致**，而且是**同一个轮次里连续做完的**——不是一个
「别的轮次先做掉 P0、后来者才补 A2/A3/A4」：

| 时刻（2026-10-07/08） | 提交 | 内容 |
|---|---|---|
| 14:39 | `c249d9b` | A1：session 层写 compaction（`append_compaction` + `build_context_entries`） |
| 14:46 | `9f49c67` | **本文件的 §0**：发现 P0.1 / P0.2，并写下当时的状态字段 |
| 17:49 | `69d2d9e` | P0.1 |
| 18:25 | `1dbf9d1` | P0.2 |
| 20:09 | `d20fb61` | A2 |
| 22:17 | `8cc4f2f` | A3 |
| 23:05 | `d1e4b2a` | A3b：overflow 压缩也走 session |
| 2026-10-08 09:09 | `6683f36` | A4 |

（时间取自 `git log --format='%h %ad %s' --date=format:'%Y-%m-%d %H:%M'`。）

P0.2 的迁移问题也有了答案：**读入时回填，不改写磁盘**。
`crates/nini-session/src/lib.rs:527` `link_flat_entries` 按文件顺序把 null 的
`parentId` 补成前一条的 id，且**不改写已经存在的真实父节点**——因此对真正的
Pi 文件（只有根是 null）是无操作。
---

## 5. 2026-11-05 复核：这份文档的状态字段曾经与事实不符

### 5.1 缺陷是什么

本文档在 2026-10-07 14:39（`c249d9b`，A1）之后 7 分钟写下 §0 的两个 P0
（`9f49c67`），并在同一批里把头部状态写成「A1 shipped，A2 blocked by P0
below」，把 §3 写成一张「Still open」表，把 §2 结尾写成「production 里不可达」。

这些描述**在写下时全部属实**。问题是：**它们写下之后就不再更新了**。同一个
轮次里，P0.1、P0.2、A2、A3、A3b、A4 就陆续落地（见 §4 的提交表），而本文档
的状态字段停在 14:46。

于是一个从未写错、只是**过期**的文档，在 2026-11-05 之前一直对读者说
「A2 blocked by P0」、「production 里不可达」。§4 最后的那个 P0 迁移问题也
早就有了答案（读入时回填），而文档还在问。

**这不是「写错了」，是「不再维护了」。** 两者在读者身上的后果一样。

### 5.2 实测证据（2026-11-05）

#### P0.1：工具调用/工具结果现在真的落盘了

- 代码：`crates/nini-tui/src/state.rs:2306` `session_append_tool_result`、
  `:2377` `session_append_bash_execution`；生产调用点
  `crates/nini-tui/src/runtime.rs:428`（工具结果）与
  `crates/nini-tui/src/state.rs:2923`（`!cmd`）。
- 提交：`69d2d9e`（2026-10-07 17:49）。
- 测试：`crates/nini-tui/tests/p01_persist.rs`，6/6 通过（实测）。
- **真实 session 文件**（`~/.nini/agent/sessions/nini/`，nini 自己写的目录）：

  | 文件 | `toolCall` 行 | `toolResult` 行 |
  |---|---|---|
  | `20261104-024403_1d1613c2e.jsonl` | 27 | 15 |
  | `20261103-234324_1806dce57.jsonl` | 115 | 60 |
  | `20261104-051849_1ec0762fe.jsonl` | 162 | 80 |
  | `20261104-012913_1be10e894.jsonl` | 213 | 115 |

  复算：`grep -c 'toolCall' ~/.nini/agent/sessions/nini/20261104-024403_1d1613c2e.jsonl`

  注：`~/.nini/agent/sessions/` 顶层的那些 `.jsonl` 是 Pi 侧导出的会话
  （`"type":"message"` + `role: user/assistant`），不是 nini 写的路径；
  nini 的会话在 `sessions/nini/` 子目录下（按 cwd 分目录）。

#### P0.2：`parentId` 现在是链式的

- 代码：`crates/nini-session/src/lib.rs:394` `append_chained` → `:396`
  `entry.set_parent_id(parent)`（父节点 = 当前 leaf，Pi 语义）；TUI 侧调用点
  `crates/nini-tui/src/state.rs:2415`。
- 旧文件回填：`crates/nini-session/src/lib.rs:527` `link_flat_entries`，
  调用点 `:751`。
- 提交：`1dbf9d1`（2026-10-07 18:25）。
- 测试：`crates/nini-tui/tests/p02_tree.rs`，5/5 通过（实测）。
- **真实 session 文件**：`20261104-024403_1d1613c2e.jsonl` 的 34 条 message
  实测——第一条 `parentId: null`，其余 33 条逐条指向前一条（线性链，无悬挂
  父节点）。

#### A2：`/compact` 与自动压缩写的是真的 CompactionEntry

- `auto_compact_local` **全仓已无**：`grep -rn "fn auto_compact_local" crates/`
  → 0 命中。`crates/nini-tui/src/state.rs:3144` 的注释明写「Replaces
  `auto_compact_local`」。
- 链路：`crates/nini-tui/src/commands/dispatch.rs:1276` `cmd_compact` →
  `:1285` `state.session_compact(keep)` → `crates/nini-tui/src/state.rs:3151`
  → `crates/nini-session/src/lib.rs:347` `Session::compact` → `:374`
  `append_compaction`（写出真的 `SessionEntry::Compaction`）。
- 自动压缩路径同样接到 `session_compact`：`crates/nini-tui/src/runtime.rs:3549`
  （附近的注释明写它「used to call `auto_compact_local`」；该文件正在被并行
  修改，行号会漂移，符号名 `session_compact` 更稳）。
- 提交：`d20fb61`（2026-10-07 20:09）。
- 测试：`crates/nini-tui/tests/a2_compact.rs`，5/5 通过（实测），
  含 `compact_writes_a_real_compaction_entry`、
  `first_kept_entry_id_is_the_entry_the_cut_landed_on`。

#### A3：阈值压缩走 session hook；`AgentEvent::Compacted` 被更好的设计取代

- `crates/nini-core/src/agent.rs:1511` `maybe_compact_at_threshold()` →
  `:1522` `compact_history_via_hook()`（`:1440`）——**不是**就地改内存，而是
  调 session hook。
- `:1751` 附近的注释明写「the threshold compaction that used to live here is
  gone」。
- hook 的生产装配：`crates/nini-cli/src/app.rs:683` `set_compaction_hook` →
  `:691` `g.session_compact(keep)`（真的落 session）。
- 提交：`8cc4f2f`（A3，2026-10-07 22:17）+ `d1e4b2a`（A3b，overflow 压缩同样
  走 session，23:05）。
- **`AgentEvent::Compacted` 保留**（`agent.rs:170`）。这是 §3 那一行
  「drop `AgentEvent::Compacted`」被**更好的设计取代**的地方：它不再是
  「没人消费的空信号」，而是「session 层发生了一次压缩」的可观察契约，
  被 4 个测试文件断言：

  | 测试文件 | 断言点 |
  |---|---|
  | `crates/nini-core/tests/compaction_lifecycle.rs` | `:97`、`:103`、`:106`（相位顺序：Compacting 必须早于 Compacted） |
  | `crates/nini-core/tests/overflow_http_400.rs` | `:288`、`:348`、`:497`、`:554` |
  | `crates/nini-core/tests/overflow_recovery_parity.rs` | `:238` |
  | `crates/nini-core/tests/a3b_overflow.rs` | `:184`、`:224` |

  另有 `crates/nini-tui/tests/a3_seed.rs:11` 与
  `crates/nini-tui/tests/compaction_e2e.rs:195` 在注释里记录了这一取舍的来龙去脉。

  **需复核**：这 4 个文件里断言 `AgentEvent::Compacted` 的用例本次**未实跑**
  （只核对了源码坐标与调用点）；其余几条（A2 / P0.1 / P0.2 / A4）已实跑通过。

#### A4：resume 的转录从 `build_context_entries` 重建

- `crates/nini-tui/src/state.rs:3171` `transcript_from_context()` →
  `:3176` `nini_session::build_context_entries(&guard)`（实现在
  `crates/nini-session/src/lib.rs:221`）→ `render_context_entries(&context)`
  （`crates/nini-tui/src/state.rs:39`）——正是 A4 要求的来源。
- resume 路径：`crates/nini-tui/src/state.rs:2484`（`session_load`）。
- 提交：`6683f36`（2026-10-08 09:09）。
- 测试：`crates/nini-tui/tests/a4_resume.rs`，4/4 通过（实测），含
  `loading_shows_the_live_context_not_the_raw_entries`。

### 5.3 教训

**计划文档的 status 字段一旦写下就不会自己更新；它的读者会拿它当事实。**

具体到这份文档：§0 的 P0 是**当时**的真实发现，留着有价值（它记录了 nini
为什么会写出没有树的 session、为什么会丢掉工具历史）；但把它写成
「Still open」并放进头部 status，就是让一句**已经失效的结论**占据了读者的
第一眼。一份写着「A2 blocked」的计划，会让读者以为 A2 还没做，于是不去做，
或者重复做。

规则（从这次事故里落下来的）：

1. **状态字段必须能被复核**。写「已交付」就要能指到提交或能跑的验证命令；
   写「未交付」也要能指到证据坐标。
2. **发现一个 P0 就要在同一份文档里记下它的归宿**，哪怕结论是「已修」——
   否则下一轮又要重新发现一遍。
3. **发现 ≠ 结论**。§0 是发现（留痕），§3 是结论（会过期）。两者混在一张
   表里，就会连着结论一起烂掉。

### 5.4 `docs/GOALS.md` §6 台账那一行该怎么写

现在这一行是：

> | `plans/r2a-compaction-persistence.md` | active/blocked | A1 已发布，A2 被其 §0 的 P0 阻塞 |

建议改成（状态从 `active/blocked` 改为已关闭，并注明 §0/§3 曾过期）：

> | `plans/r2a-compaction-persistence.md` | **done** | **A1–A4 与 §0 的 P0.1/P0.2 全部交付**（2026-10-07 一轮做完，2026-11-05 复核）；该文档 §0 的 P0 与 §3 的「Still open」表**曾与事实不符**，已在本文档 §5 留痕并更正 |

注意：台账那一行本身也是状态字段，同样的坑适用于它——所以它必须指向
**可复核的东西**（本文档 §5），而不是「已交付」三个字。
