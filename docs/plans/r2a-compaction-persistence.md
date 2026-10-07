# R2a — compaction persistence: ground truth, gap, and the fork

Status: **investigated, not implemented.** `edit` contract shipped as `965fe84`.
Baseline at time of writing: 1012 tests pass, clippy 0, fmt clean.

---

## 1. Pi 0.84.2 ground truth

Source: `/home/jin/pi-src-reconstructed/pi-coding-agent/src/core/session-manager.ts`
(recovered from sourcemaps; 495 files, zero missing).

### 1.1 On-disk entry

```ts
// session-manager.ts:69
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

`SessionEntryBase` contributes `id`, `parentId`, `timestamp`.

### 1.2 Write

```ts
// session-manager.ts:1097
appendCompaction(summary, firstKeptEntryId, tokensBefore, details?, fromHook?, usage?): string {
	const entry: CompactionEntry<T> = {
		type: "compaction",
		id: generateId(this.byId),
		parentId: this.leafId,
		timestamp: new Date().toISOString(),
		summary, firstKeptEntryId, tokensBefore, details, usage, fromHook,
	};
	this._appendEntry(entry);
	return entry.id;
}
```

Note `firstKeptEntryId` is a **plain required string**. nini models it as
`Option<String>`. That is a divergence, though a permissive one: Pi's reader
would break on a `null`, so nini must never write `null` there.

### 1.3 Read — the reconstruction algorithm

```ts
// session-manager.ts:415  buildContextEntries
const path = buildSessionPath(entries, leafId, byId);
let compaction = null;
for (const entry of path) if (entry.type === "compaction") compaction = entry;
if (!compaction) return path;

const compactionIdx = path.findIndex((e) => e.id === compaction.id);
const contextEntries = [compaction];
let foundFirstKept = false;
for (let i = 0; i < compactionIdx; i++) {          // scan FORWARD
	const entry = path[i];
	if (entry.id === compaction.firstKeptEntryId) foundFirstKept = true;
	if (foundFirstKept) contextEntries.push(entry);
}
contextEntries.push(...path.slice(compactionIdx + 1));
return contextEntries;
```

**Semantics:** the live context is
`[compaction] + (pre-compaction tail starting at firstKeptEntryId) + (everything after the compaction)`.

The compaction entry becomes a model message via
`createCompactionSummaryMessage(summary, tokensBefore, timestamp)` →
`{ role: "compactionSummary", summary, tokensBefore, timestamp }`.

`firstKeptEntryId` is therefore **load-bearing**: without it, resume cannot know
where the retained tail begins, and the whole pre-compaction history is either
duplicated or lost.

---

## 2. nini's verified state — six breaks in one chain

Every line below was read from the working tree, not inferred.

| # | Break | Evidence |
|---|---|---|
| 1 | `AgentEvent::Compacted` is emitted twice and **consumed by nobody** | `agent.rs:939`, `agent.rs:1161`; grep for `Compacted` across `nini-tui/`, `nini-cli/`, `nini-session/` returns **zero** matches |
| 2 | `AgentEventLite` has **no `Compacted` variant** | full enum at `runtime.rs:67` |
| 3 | `make_compaction_entry` has **no production caller** | only `lib.rs:45` re-export and `compaction.rs:1221` test |
| 4 | `first_kept_entry_id` hardcoded `None` in production | `nini-session/src/lib.rs:707`, `nini-core/src/lib.rs:232` |
| 5 | The Agent has **no stable message identity** | `compact_history` synthesises `format!("msg_{i}")` from a positional index (`agent.rs:718`) |
| 6 | Agent messages and session entries are **not positionally aligned** | session entries are appended at **TurnEnd**, one per turn — `state.rs:1525` "Tool results are part of the turn — session_append is handled on TurnEnd" |

Break 6 is load-bearing. `compact_history` returns `keep_from`, an **index into
the Agent's `Vec<Message>`**. The session stores **one entry per turn**. There
is no mechanical map from one to the other, so `first_kept_entry_id` could not
be computed even in principle from what the Agent currently publishes.

This is not laziness in the original implementation. It is a structural
consequence of the split of responsibilities, and it is why the field is `None`.

### What nini already gets right

The in-memory splice is already Pi-equivalent:

```rust
// agent.rs — compact_history
new_history = [summary_message] ++ messages[prefix_len..]
```

which is the same set Pi's `buildContextEntries` produces. The defect is purely
in **persistence**, not in runtime behaviour.

---

## 3. The fork

Pi compacts **in the session layer** (`agent-session.ts:1878` →
`sessionManager.appendCompaction`), so the component that compacts owns the
entries and therefore knows the ids. nini compacts **in the Agent loop** and the
session layer only ever sees turn-level entries.

Closing the chain therefore requires an architecture decision.

### Option A — move compaction orchestration to the session layer (Pi's way)

The session owns entries and ids; compaction becomes a session operation the
TUI invokes, like `/compact`.

- Cost: the auto-compaction triggers currently inside the loop
  (`agent.rs:893` threshold, `agent.rs:1136` overflow) must move, or become
  requests the session services. Large blast radius.
- Benefit: it is Pi's actual architecture; the identity problem disappears
  permanently rather than being papered over.

### Option B — persist at turn granularity, offset in `details`

Write the `CompactionEntry` with `first_kept_entry_id` pointing at the **turn
entry** holding the first retained message, and record the intra-turn offset in
`details`.

- Cost: small. Read side must honour the offset.
- Cost: introduces a reconstruction rule Pi does not have. `details` is an
  extension point Pi itself documents, so the wire stays valid — but a
  Pi-produced session would reconstruct slightly differently from a nini one
  for the same logical state.

### Option C — give the Agent stable ids

Mint a uuid per Agent message at creation, carry it into the session entry, and
use it as `first_kept_entry_id`.

- Cost: medium. Touches message creation sites.
- Benefit: `firstKeptEntryId` means exactly what Pi's means. No second
  reconstruction rule. Wire-clean.

---

## 4. Recommendation

**Option C now, Option A eventually.**

C is the only option that makes `first_keptEntryId` semantically identical to
Pi's without relocating the auto-compaction triggers, which are entangled with
overflow handling inside the loop. A remains the target architecture and can be
adopted later without invalidating C's ids.

**B is rejected**: it trades a structural problem for a permanent semantic
divergence, which is the opposite of what this project is for.

The user owns this decision: it determines the shape of the next substantial
chunk of work, and building the wrong one costs more than not building it.

---

## 5. Work common to all three options

Safe to build regardless of the fork, and a strict prerequisite for any of them:

1. `AgentEventLite::Compacted { summary, tokens_before }` + the mapping from
   `AgentEvent::Compacted`. Without it the TUI cannot observe the event at all.
2. `Session::append_compaction(...) -> String` in `nini-session`, mirroring
   `appendCompaction` — real id, `parent_id = leaf`, returns the new id.
3. `compact_history` stops minting `msg_{i}` from a positional index.
4. `first_kept_entry_id: None` removed from both production sites; the type
   becomes non-`Option` to match Pi, or is asserted non-null on write.

Each step is independently testable and none of them commits to A, B or C.
