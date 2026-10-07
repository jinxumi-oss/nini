# R2a — compaction persistence: ground truth, gap, and the fork

Status: **A1 shipped** (`c249d9b`). A2 blocked by P0 below.
Baseline: 1019 tests pass, clippy 0, fmt clean.

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

### P0.2 Every entry has `parentId: null`, so there is no tree

A session with 4 entries yields a `get_path()` of length **1**.

`Session::get_path` finds the leaf as "an entry no other entry parents to".
With `parentId: null` on everything, every entry qualifies, and the newest
wins. The path is a single entry.

Consequence: `get_path`, tree/branch selection, `/resume` leaf choice, and
Pi's `buildContextEntries` are all non-functional on any session nini has
ever written. `firstKeptEntryId` refers to a tree that does not exist.

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

The algorithm is Pi-equivalent and verified. **It is currently unreachable in
production**, because it consumes a tree that nini does not write (P0.2).

---

## 3. Still open

| id | Item |
|---|---|
| P0.1 | Persist tool calls and tool results |
| P0.2 | Chain `parentId` so `get_path` returns the real history |
| A2 | Delete `auto_compact_local` and `cmd_compact`'s transcript slicing; route `/compact` and auto-compact through the session layer |
| A3 | Move the Agent's in-loop trigger (`agent.rs:893`, `:1136`) out to the driver; drop `AgentEvent::Compacted` |
| A4 | Rebuild the transcript from `build_context_entries` on resume |

### A2's two fakes

`state.rs:1627 auto_compact_local` and `dispatch.rs:936 cmd_compact` both fold
**transcript lines** at `total / 2` — no token budget, no session write, no
effect on what the model sees. `cmd_compact`'s comment claims it "matches the
algorithm used by the auto-compaction path", which is false: auto uses a
token estimate, then the same `/2`.

---

## 4. Order

P0.1 → P0.2 → A2 → A3 → A4.

P0.2 changes `get_path` semantics for every existing session, so it needs a
migration decision: nini sessions written with flat `parentId: null` cannot be
distinguished from branched ones except by inspecting for any non-null parent.