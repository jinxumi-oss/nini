## v0.10.9 — the default model is one the operator actually has (2026-10-26)

1633 tests, all green. Three things that were quietly wrong, and one that
was asked for.

### The default model is `MiniMax-M2.7-highspeed`

It was `MiniMax-M3.1-Flash-Preview`. The new one is the model the
operator's own `models.json` declares under the gateway provider that
serves it, so the default is a model that exists on the other side of the
connection instead of one nini has to hope for.

The id is sent to the provider **as it is**: nini does not qualify it with
a provider prefix, because the provider is chosen separately
(`--provider` / `NINI_PROVIDER`) and a prefix would make the id a
*different* id to every server that is not the one it was written for.

The promise is pinned in three places that cannot drift apart silently:
the constant in the source, the `--help` text the binary prints, and
`--list-models` (`crates/nini-cli/tests/default_model.rs`). A default
model is a promise made to a user who types nothing; the previous default
(`test-model`) was replaced for exactly this reason — it named a model no
server had heard of, and the failure surfaced as a 404 at the first turn.

### nini can finally read the models a user's `models.json` declares

Pi writes `contextWindow`, `maxTokens` and `reasoning`. nini's
`ModelConfig` had `context_window` and nothing else, so serde **ignored
every key Pi had written** — a user's `models.json` parsed into an empty
model list. The symptom was silent and total: every model the user had
declared was invisible to nini, and the context window nini used was the
200 000 fallback rather than the number written in the file.

Three consequences, all now fixed:

1. **The models are known.** A model the user declares becomes a
   `ModelSpec` — with their id, their window and their output ceiling.
   The price is **not** carried over: `models.json` states no rates, and
   inventing them is the defect this project removed when it stopped
   fabricating costs. A user model therefore has a zero cost, and the
   status bar hides a zero-cost pill — "unknown" reads as absent, not as
   free.
2. **The `/model` selector offers them.** A catalogue that hides the
   models this installation can reach is a catalogue that lies about what
   is available.
3. **The context window is theirs.** `refresh_context_window` consults
   the catalog, then the user's declaration, then the settings fallback —
   in that order, because the catalog's numbers are measured, the user's
   are their own fact about their setup, and the settings value is a
   default nobody has adjusted for this model.

The dead `BUILTIN_CATALOG` — thirteen hand-written model ids with a
comment claiming they were the fallback for `--list-models`, read by
nothing — is **gone**. A second, hand-written list of models next to the
generated one is how two answers to one question get written down.

### `/model` searches the way Pi searches (B6)

Pi does not filter the model selector on the model id. It filters on
`getModelSelectorSearchText` (`model-search.js`) —

```text
`${provider} ${provider}/${id} ${provider} ${id}${name}`
```

— and ranks the survivors by `fuzzyFilter` (`fuzzy.js`) from
`@earendil-works/pi-tui`.

nini filtered on the **label** with a generic substring-then-fuzzy rule
and never saw the provider at all. The consequence was not a cosmetic
ordering difference: typing `openai/gpt-5` found
`openrouter/openai/gpt-5` — the proxy whose id merely *contains* the
query — and could not find `openai/gpt-5` at all, because that id was not
a prefix of the proxy's id. Pi's provider-first search text is precisely
the rule that puts the model the user meant on top.

The filter is now the selector's own (`SelectorState::state_filter`),
with nini's generic rule kept as the default for every other selector.

**The port was checked against Pi's own function, score for score**, and
that check found three defects:

| defect | symptom |
|---|---|
| the first matched character did not earn Pi's first `-5` (Pi starts `lastMatchIndex` at `-1`, so it *is* consecutive) | every score was too high by 5–30 points; the order could change |
| only one of Pi's two alphanumeric-swap patterns was implemented | `5gpt` found nothing where Pi finds `gpt5` |
| the catalogue handed Pi's template a **qualified** id (`openai/gpt-5`) where Pi's `item.id` is **bare** (`gpt-5`) | the search text was `openai openai/openai/gpt-5 …` — a text Pi never builds, and one whose scores are not Pi's |

The expectations in `crates/nini-tui/tests/pi_fuzzy_oracle.rs` are **Pi's
output**, produced by running `fuzzy.js` from `pi-tui` 0.85.1 and pinned
verbatim. A port that is 90 % faithful is a port whose remaining 10 % is
invisible, and the score is what decides the order.

### `/trust` without an argument asks a question

nini had a complete `TrustSelector` — and **nothing ever called it**. The
command layer answered the no-argument case by falling back to
`sub = "trusted"`: typing `/trust` *marked the working directory
trusted*, silently, with no question asked and no list shown.

Pi answers a bare `/trust` with `showTrustSelector()`
(`interactive-mode.ts#showTrustSelector`). So does nini now. The
argument forms (`/trust ask`, `/trust distrust`, `/trust list`,
`/trust clear`) are nini's own and keep working.

The oracle asserts the thing the runtime actually watches —
`AppState::run_state.status` becoming `open_selector:trust` — and, just
as importantly, that a bare `/trust` writes **no** decision.

### The session shape gets its first executable oracle (C1, first brick)

C1 says the behaviour layer had no executable Pi-side comparison: every
oracle was "a comment citing a line number, plus an assertion about
nini". The tool contracts and the truncation rules got their Pi-side
oracles in v0.10.8; this release adds the third — the **shape** of a
session.

The comparison is `crates/nini-conformance/fixtures/pi-0.85.1-session.jsonl`,
written by **real Pi 0.85.1** (recorded by `scripts/pi-session-record.sh`
against a mock provider, paths washed out). What is asserted is the
shape — entry types, roles, content-block types — and not the words: the
words are the mock's, and pinning them would pin the mock.

The shape matters because the shape *is* the context the model reads. An
assistant `toolCall` and its `toolResult` are two messages in Pi's
convention; if nini wrote them as one, the model would be reading a
different conversation on the next turn.

### Four citations that pointed at nothing, and two files the lock did not know about

The behaviour layer's citations are **symbols** (`file#symbol`), and
`scripts/pi-oracle-lock.py --verify-refs` resolves every one of them
inside the 0.85.1 snapshot — a symbol that disappears turns the run red.
Running it found **four** that did not resolve, and all four were wrong:

| citation | reality in 0.85.1 |
|---|---|
| `agent-session.ts#queueCompactionMessage` (×2) | the method lives in `interactive-mode.ts` |
| `interactive-mode.ts#handleSlashCommand` (×2) | there is no such method; the dispatch is inside `setupEditorSubmitHandler` |

A citation that points at a symbol which is not there is worse than a
line number that drifted: it looks like evidence and resolves to nothing.
All four are fixed, and the run now resolves **111 citations with
`drift=0` and `missing=0`**.

The snapshot check (`--check`) was red for a different reason: two files
sat in the snapshot directory that `SNAPSHOT.lock` did not describe —
`tool-contracts.json` and `truncate-oracle.json`, both dumps of Pi's own
runtime. A lock that does not describe everything in the directory cannot
detect tampering with it, so both are now recorded in the lock (with
their hashes), and the check reports **413 files, all matching**.

---

## v0.10.8 — the guard against a second agent was the editor's state (2026-10-23)

1600 tests, all green. The defect was measured on v0.10.2 and had been sitting
in the plan as a P0 with a written exit criterion that nobody had executed.

### Esc could start a second agent, and the session file kept the receipts

Reproduce the old one:

```console
cargo test -p nini-tui --test esc_concurrency_probe -- --ignored --nocapture
# → 2 spawns, 2 live
```

The path is ordinary use: type while the agent is streaming, press **Esc**,
press **Enter**. Pi cannot reach that state — `AgentSession.isIdle` is
`!_isAgentRunActive` (`agent-session.js:592`), a property of the *agent*,
and Esc only calls `agent.abort()` without touching it; a submission during
streaming is steered into the running turn, so a second agent is never
spawned.

nini had **one field doing both jobs**. `RunMode` answered "may the editor
take a submission?" *and* "is an agent running?" — and v0.10.1 (fixing the
real bug "after Esc I cannot type for four seconds") set it back to `Editing`
the instant Esc was pressed, which removed the only guard against a second
spawn. Two turns then wrote to one session file, shared one `TokenStats`, and
interleaved their events into one transcript.

### Two facts, two fields

`RunState` now carries `agent_alive: bool` — "has the spawned task finished
pushing events?" — next to `mode`, which keeps its own job. The guard in
`submit_user_input_inner` reads `agent_alive`; Esc still hands the editor
back immediately, because that is what the user asked for and it is what Pi
does.

A flag that is raised must also be lowered, and the lowering is where this
kind of fix usually leaks:

* the terminal `Done` event clears it;
* a **watchdog** clears it too, because `Done` is the *driver's* promise and
  the driver is a `dyn Fn` — a panic, an early return or a poisoned lock
  inside the task would otherwise leave the flag up forever, and the symptom
  of that is silence: nothing could ever be submitted again;
* the watchdog is armed with a monotonic `run_seq`, so a task that finishes a
  moment *after* its `Done` landed cannot disarm the guard of the next turn.

A submission that arrives while the agent is still alive is **steered into
the existing queue** rather than refused — refusing it silently threw away
what the user had typed.

### The receipt

`tests/esc_concurrency_probe.rs` is no longer `#[ignore]`d: a defect that was
fixed and then re-introduced has to fail a plain `cargo test`. It now has
seven tests — the original one, the four that guard the *other* direction
(`Done` clears the guard; a `Done` reopens the editor; an erroring turn
reopens it; a driver that never says `Done` still reopens it), and two that
the **review** below asked for. A guard that is never lowered is not a fix,
it is a mute button.

### The review found five things in the fix itself

An independent review of the change (`docs/reviews/parity-v3-review.md`)
found five critical problems — three of them **in the fix that was supposed
to close the hole**. All five were confirmed and closed, each with its own
mutation test:

1. **The watchdog's turn number was read outside the critical section**, so
   it could capture a *newer* turn's number and then disarm that newer
   turn — reopening the window A1 closed. The number is now read in the
   same critical section that raises the guard.
2. **A late `Done` from an interrupted turn cleared the guard of the turn
   running now.** The sink now knows which turn it belongs to, and a stale
   terminal event is ignored instead of acted upon.
3. **A queued message was written into the session twice** — once when it
   was queued and once when it was delivered — so the model saw it twice
   and `/resume` showed it twice. Queueing now writes to the transcript
   only; delivery writes to the session.
4. **The steer path required `mode == Editing`**, so a submission that
   reached the API while a turn was running (mode `Running`) was silently
   dropped — the text stayed in the input box and nothing was queued. The
   product's key handler queued it on its own, which is why only the API
   path lost it. A new test dug this out.
5. **Two of the new checks were vacuous** — a `--check-plan` that matched
   zero ids and passed anyway, and a self-check whose regular expression
   matched nothing. A check that cannot fail is decoration. The plan
   cross-check now works on the prose (every gap id the plan mentions must
   exist in the ledger; every gap the ledger calls fixed must be written
   down), and the self-check gained a rule that **every regex-based check
   must catch its own synthetic sample**.

Four mutations were injected to prove the assertions can go red: deleting
the `Done` reset, disarming the watchdog, writing to the session at queue
time, and restoring the old `mode == Editing` condition — each turns a
different test red.

### The end-to-end version, and why a unit test was not enough

`scripts/e2e-interrupt.sh` starts the **real binary** against the scripted
mock provider, interrupts a turn that is really streaming, submits again, and
then reads two artefacts instead of the screen:

1. the mock provider's request log — **no two requests may overlap**;
   overlap is the machine-checkable signature of a second agent starting
   while the first was still streaming;
2. the session JSONL — the second turn's text must not be written before the
   first turn's, and the timestamps must be monotonic.

It is not a decoration: with the fix reverted, the script fails with
*"request #1 and #2 overlap — a second agent started while the first was
still streaming"*.

### The target version is now **one** version (0.85.1)

Until now nini aimed at two Pi versions at once: the format layer was frozen
against the spec copied from **v0.85.1**, while the behaviour layer could only
be checked against the **v0.84.2** installed on this machine. That is not
"layering", it is **two targets** — and the same piece of code can be answered
two different ways.

The cost was measured, not guessed:

- **90 of the tree's line-number citations pointed at Pi source lines.**
  Between 0.84.2 and 0.85.1, **11 of them still pointed at the same line**;
  78 pointed at something else; 1 was out of range — and **nothing turned red**.
  A citation that silently points at the wrong statement is worse than no
  citation at all: it looks like evidence.
- The machine's Pi was upgraded to **0.85.1** (`pi --version` = 0.85.1; the
  four sibling packages moved with it). The one-time `migrateAuthToAuthJson`
  migration was a **no-op** here — `auth.json` already existed — and the
  user's `~/.pi/agent/auth.json` is byte-identical after the upgrade.
- Both versions were snapshotted into the repository while the diff was
  taken, then **the 0.84.2 snapshot was removed**: one installed Pi, one
  snapshot, one target. The diff's findings live on in
  `docs/pi-version-delta-0.84.2-to-0.85.1.md` — they are evidence, not
  clutter.
- Citations changed shape: **`file#symbol`** instead of `file:line`
  (`agent-loop.ts#shouldStopAfterTurn`). The meaning is pinned; the line
  number is resolved by `scripts/pi-oracle-lock.py --verify-refs` in the
  target snapshot. A symbol that disappears turns the run red; a symbol that
  moved is reported as movement. 101 citations resolve against 0.85.1 with
  zero drift.

### Three structural moves between the versions (they change what "parity" means)

1. **`pi-client` and `pi-protocol` were removed**, replaced by
   `@earendil-works/chord`. nini's counterpart for that layer was zero nodes
   to begin with — and now the layer has a different name too.
2. **Keybindings moved into `pi-tui`.** In 0.85.1 the coding agent *also* has
   a file called `keybindings.ts` (the app-level bindings). **Same name,
   different thing** — so the alias table lives in exactly one place
   (`scripts/pi_refs.py`); three copies would repeat the "same distortion in
   three copies" bug this project already paid for in its keymap.
3. **The search-hit styling moved from the coding-agent layer to the `pi-tui`
   layer.** nini copied that styling from 0.84.2's coding agent — the
   counterpart is no longer there. That is the price of not knowing whose
   responsibility a piece of code was when you copied it.

### The target version is frozen, and the freeze is machine-enforced

owner's standing directive (2026-10-23): *from now on, nini's parity work
anchors to **one** version, until I say there is no problem left with it.*

A rule that lives only in a conversation evaporates at the next session, and a
rule without a gate is not a rule. So it is now machine-checked
(`scripts/pi-target-lock.py`, wired into CI and into
`scripts/plan-selfcheck.py`):

- exactly **one** Pi snapshot under `references/` — two snapshots means two
  answers for the same piece of code;
- the snapshot, the format spec and the machine's `pi --version` must all be
  the **same** version;
- no source file may point at another Pi version;
- `docs/GOALS.md` and this plan must declare the same target;
- the freeze flag itself must be true — **releasing it is the owner's action,
  not the script's**.

The order for a future version change is written down in
`references/TARGET.json` and cannot be reversed: **diff first, re-anchor
second, change the target last.** Skipping the first step is exactly how the
0.84.2 round produced 78 citations that quietly pointed at the wrong
statements with nothing turning red.

### A plan for the built-in tools, with a board you can run

`docs/plans/tool-parity.md` is the execution detail for **WF-BUILTIN** — the
eight built-in tools (read/write/edit/bash/grep/find/ls/powershell) against
**Pi 0.85.1**, in four items:

| | item | criterion (one command, one exit code) |
|---|---|---|
| **T1** | re-capture every tool's **contract** (description + parameter schema) from 0.85.1 — nini's are transcribed from 0.84.2 and all seven differ | `python3 scripts/pi-tool-contracts.py --compare` |
| **T2** | add the missing **`ls` tool** (Pi's coding layer; `path` + `limit`, default 500) | `python3 scripts/tool-parity-check.py --quick` |
| **T3** | **tool set → system prompt**: Pi only lists a tool in "Available tools" when the caller gave it a one-line snippet, and the guidelines depend on which tools are enabled (`hasBash && !grep && !find && !ls` → "Use bash for file operations like ls, rg, find"). nini has neither rule, and its prompt is assembled in `nini-core` while Pi 0.85.1 assembles it in **`pi-ai`** — the layering is inverted there | `cargo test -p nini-core --test prompt_tool_linkage` |
| **T4** | **truncation semantics**: Pi has one `truncate.js` (50 KiB head/tail, 500-char grep lines); nini has per-tool implementations (bash 1 MiB / 2000 lines) | `cargo test -p nini-tools --test truncation_parity` |

Every item states how to make its own assertion go red, because a criterion
that cannot fail is decoration. The board starts red on purpose — it is the
baseline:

```
T1 ✗  7 个工具与 0.85.1 不一致：bash edit find grep ls read write
T2 ✗  crates/nini-tools/src/ls.rs 不存在
T3 ✗  测试不存在
T4 ✗  测试不存在
```

In CI the board runs as a **report** (`continue-on-error`) until the plan is
done; then it becomes a hard gate.

### `/export` now honours the path you give it

Pi's `/export` says *"Export session (HTML default, or specify path:
.html/.jsonl)"*. nini **declared that argument and then ignored it** — the
signature was literally `_args` — so the file always landed in the exports
directory no matter what you typed. Declaring a parameter the implementation
does not read is a dishonesty bug (T4), and the ledger had it as B1b.

`/export <path>.html` writes the transcript there; `/export <path>.jsonl`
writes the **session** (Pi's own JSONL format, first line the session
header); any other extension is refused **out loud** — *"use .html or
.jsonl"* — instead of quietly writing somewhere else. With no argument the
behaviour is unchanged (HTML into the exports directory).

### Esc finally means something different in different contexts

Pi does not have one Esc handler: it **replaces** the handler before entering
compaction, a retry or a branch summary (`autoCompactionEscapeHandler`,
`retryEscapeHandler` — see the EscapeTarget in `interactive-mode.ts`), and
puts the old one back when it leaves that context.

nini had one if-else chain (completion → search → pending quit → running →
clear), so "Esc during compaction" had no meaning of its own — it happened to
land in the "running" arm, which is close, but it was not *designed*.

There is now an explicit `EscapeTarget { Idle, Agent, Compacting, Retrying,
BranchSummary }`, derived from the agent's phase, and Esc dispatches on it:
Idle clears the input, while Compacting/Retrying/BranchSummary interrupt the
turn — so the text you are typing is never thrown away by an Esc in the
middle of a compaction.

The two dispatch points (the runtime's key handler and `apply_action`) used
to be **two independent if-else chains** — the same "one truth, several
copies" defect this project already paid for in its keymap. They now share
one decision function, `escape_action()`.

One difference from Pi remains: Pi cancels *only* the compaction and lets
the turn continue; nini interrupts the whole turn (its compaction is local
and takes milliseconds). Tracked as gap **A2b**.

### Context compaction has a lifecycle now — and the code that was dead is alive

nini detected a context overflow, compacted, and carried on — **silently**.
There was no phase, no flag, nothing. Which meant that
`runtime.rs`'s branch *"if compaction is in progress, queue the message
instead of spawning an agent"* was **unreachable dead code**: the flag it
tests (`run_state.is_compacting`) was never set by anything. Pi has an
explicit meaning there (`agent-session.ts#queueCompactionMessage`); nini had
a comment describing one.

The agent now emits a `Compacting` phase around the compaction and clears it
afterwards, on **both** overflow paths — the recoverable length stop *and*
the provider's `StreamEvent::Error`, which is the one that arrives in
production (providers map HTTP 400 to it). The TUI turns that phase into
`run_state.is_compacting`, so the queueing branch is live: a message typed
during compaction is queued and delivered with the next turn instead of
starting a second agent.

The phase is carried as a **stable machine tag** (`AgentPhase::tag()`), not
as the `Display` string. The TUI used to decide "are we compacting?" by
`starts_with("compacting")` — a display format that, if it ever changed,
would have silently disabled the whole mechanism with nothing going red.

An independent review of this change found three critical problems in it —
a queue that was dropped while the comment claimed it was handed back, a
`None` branch that never ended the lifecycle (the flag would have stayed
true forever, and the TUI would have queued every later message and run
none of them), and a missing reset when a run ended. All three are fixed,
each with an oracle and a mutation check.

What is **not** fixed yet: pressing Esc *during* a compaction should cancel
the compaction and let the turn continue — Pi swaps its Esc handler per
context. nini's Esc is still one if-else chain, so that is tracked
separately (gap **A2b**, part of the EscapeTarget work).

### A message you typed during an interrupt no longer disappears

The A1 fix made Esc hand the editor back immediately — which is right — but it
also created a hole: a message submitted in that window was **queued** (the
agent was still winding down, so a second agent must not start), and then
**nobody ever picked it up**. The agent loop polls for steering messages
*during* a turn; once the turn is over there is no poll left, so the message
sat in the queue forever. What the user typed was simply gone.

Pi solves this in its session layer: when a run settles, the session sends
the pending messages as follow-up turns. nini's equivalent is the runtime
loop, and it now does the same thing: when a turn ends (or the watchdog
reaps it) and messages are queued, the loop sends them as **new turns** —
one message per turn, in order, so the order is preserved and each message
gets its own turn, exactly like Pi's follow-up shape.

The user does not press Enter again. A message already shown in the
transcript (it was shown when it was queued) is written to the session but
**not displayed a second time**, and the number of automatic deliveries is
bounded so that "delivered → the user queues another → delivered again"
cannot loop forever.

### A conversation no longer disappears when you close the TUI

nini started, ran, and **never wrote a session file**. The footer said
`[no session]`, `/resume` listed nothing, and everything said in a session
was gone the moment the process exited — unless you happened to run `/new`
first. Pi creates a session file per conversation, under
`~/.pi/agent/sessions/<project>/`.

nini now does the same, with Pi's shape:
`~/.nini/agent/sessions/<project>/<timestamp>_<suffix>.jsonl`. The startup
path and `/new` share one helper (`paths::new_session_path`), so the files
sort the same way in `/resume`; the suffix keeps two sessions opened in the
same second from colliding (the second would have overwritten the first).
The file is written on the first flush, so starting and quitting immediately
does not litter empty files.

The footer now shows the session id instead of `[no session]`, and when
there is no agent directory at all, nini says so plainly rather than
letting the user believe the conversation was saved.

Also: the no-TTY banner said **`nini v0.4.0`** while the binary was 0.10.8.
It now reports the real version.

### The conformance fixture is now Pi 0.85.1's own output — and it proves the graph

The session fixtures were recorded from **Pi 0.84.2**, and their values were
scrubbed: every entry id was replaced with an 8-character hash *without*
carrying the change through to `parentId`. Seventeen of twenty-one parent
references dangled, so the fixture could prove **line-level schema** and
nothing else — not `get_path`, not compaction reconstruction, not branch
selection. One of its own tests pinned that limitation so it could not be
mistaken for a property of Pi.

`scripts/pi-session-record.sh` now records the real thing: it starts the
scripted mock provider, drives the **installed Pi 0.85.1** through a turn
that includes a tool call, and takes the session file Pi wrote. Only the
path is scrubbed; **ids, parent ids, timestamps and bodies stay verbatim**.

So the fixture now proves two things at once:

- every line Pi writes is readable by nini (`failed == 0`);
- **the parent chain resolves completely**, so it can be used to test
  everything that walks the tree;
- and it contains the full turn shape — user → assistant(`toolCall`) →
  `toolResult` → assistant(text) → user → assistant(text) — because **tool
  calls and tool results are recorded in Pi's sessions**. nini's own P0.1
  notes that nini *writes* without them; this proves nini cannot *read*
  without them either.

The old 0.84.2 fixtures stay, labelled as historical evidence in
`fixtures/README.md`. CI re-records the fixture and compares its **shape**
(entry types and role sequence) — ids and timestamps differ every time,
because a real Pi wrote them.

### The tools: seven of Pi's eight, with the contract checked against Pi's own runtime

The built-in tool set now matches **Pi 0.85.1** — `read`, `write`, `edit`,
`bash`, `grep`, `find`, `ls` — down to the tool descriptions, the parameter
schemas, and the prompt contributions. The only exception is `powershell`
(Windows-only in Pi), which nini does not ship.

**How the comparison is made.** Not by transcribing Pi's source: Pi's tool
descriptions are *template literals* (`Output is truncated to
${DEFAULT_MAX_BYTES / 1024}KB`), so reading the source yields the wrong
string, and comparing it against nini's Rust literals compares two *ways of
writing* rather than one contract. `scripts/pi-tool-dump.mjs` now **executes
Pi's own `createToolDefinition`** and freezes the runtime values — description,
parameter names/types/descriptions, `required`, `promptSnippet`,
`promptGuidelines` — into `references/pi-0.85.1/tool-contracts.json`.
`crates/nini-tools/tests/tool_contract_parity.rs` asserts nini against that
file, tool by tool.

**What the alignment exposed** — four behavioural differences that no schema
comparison would have caught:

- **`bash` had a 120-second default timeout.** Pi's contract says "optional, no
  default timeout". nini's cap would **kill a long build** — exactly the thing
  a user asks an agent to do. Now there is no default timeout; an explicit one
  is still bounded.
- **`read`'s `offset` was 0-indexed** while the contract (now Pi's) says
  1-indexed. A contract that lies is worse than an inconsistency: the model
  would read the wrong line. Both are now 1-indexed.
- **`grep` advertised three parameters** (`pattern`/`path`/`include`) while Pi
  has seven. The model never used what it was not shown — `glob`,
  `ignoreCase`, `literal`, `context` and `limit` are now implemented, and
  `include` is renamed to Pi's `glob`.
- **The output formats differed**: grep printed `path:line:content` where Pi
  prints `path:line: content` (with a space), and Pi truncates long lines at
  500 characters.

**The system prompt now follows Pi's rules**, which nini did not have at all:

- a tool appears in "Available tools" **only when it provides a one-line
  snippet** (Pi: `visibleTools = tools.filter(name => !!toolSnippets?.[name])`);
- the guidelines depend on **which tools are enabled** — Pi emits "Use bash
  for file operations like ls, rg, find" only when there is a shell but no
  grep/find/ls;
- two guidelines are always present ("Be concise in your responses", "Show
  file paths clearly when working with files");
- and the sections are assembled **exactly once** — the CLI used to build them
  and the agent built them again, so the model saw the same instructions twice.

**One truncation path instead of five.** Pi has a single `truncate.js`
(2000 lines *and* 50 KiB, whichever comes first; head never splits a line,
tail may keep half a line; single lines cut at 500 characters). nini had a
per-tool implementation with a 1 MiB budget — twenty times Pi's, so the same
command consumed twenty times the context. `crates/nini-tools/src/truncate.rs`
implements Pi's semantics, and `scripts/truncate-oracle.mjs` **executes Pi's
own `truncate.js`** to freeze the expected values
(`references/pi-0.85.1/truncate-oracle.json`) that
`crates/nini-tools/tests/truncation_parity.rs` asserts against. `bash` now
also spills the full output to a temp file and says so, because its contract
promises that ("If truncated, full output is saved to a temp file") — a
description that lies is a bug.

An independent review of the plan found five holes in it (the comparison was
heuristic, the ordering rationale was wrong, two checks could pass vacuously,
and the prompt was assembled twice); all five are closed, and the plan records
the corrections in `docs/plans/tool-parity.md` §3.5 and
`docs/reviews/tool-parity-review.md`.

### The tools are two different projects, and now they are labelled as such

Pi's own words (`dist/core/sdk.d.ts`): `noTools: "builtin"` disables "the
default built-in tools (**read, bash, edit, write**) but keep
**extension/custom tools** enabled", and `customTools` registers "in
addition to built-in tools".

- **Original built-in tools** — functions that ship with the binary:
  `read/write/edit/bash` (harness layer) and `grep/find/ls` + `powershell`
  (coding layer). nini has six of the eight, **missing `ls`**, and every
  description was captured from **0.84.2** and never re-captured.
- **Extension tools** — plugins registered by third parties. **Pi ships none
  by default** (its one built-in extension, llama.cpp, is hidden and registers
  no tool). The contract is a different animal: an extension tool can bring
  its own `renderCall`/`renderResult`, receive `onUpdate` and a `signal`, and
  contribute prompt fragments.

nini already has **half** of the extension contract — `Tool::system_prompt_contribution()`
is Pi's `promptSnippet`/`promptGuidelines` equivalent, assembled
deterministically. What it lacks is the other half: **no per-tool renderer
registry, no streaming update callback, no signal** in `ToolContext`. So the
honest statement is not "nini has no extension runtime" but: **a nini
extension can contribute a function-shaped tool and a prompt fragment, but not
a tool with its own appearance and streaming progress.**

`references/pi-0.85.1/tool-contracts.json` now holds Pi 0.85.1's tool
contracts, and `scripts/pi-tool-contracts.py --compare` reports where nini
still speaks 0.84.2 (all seven differ).

### The plan was rewritten, because the plan had holes

`docs/plans/parity-with-pi.md` is now **v3**. The v2 plan was audited and
found to have thirteen holes, five of which would have sent an executor
somewhere wrong (the most serious: its "Pi cannot be triggered offline"
branch allowed the P0 to be deferred behind P2 work, with no owner in the
loop). v3:

* moves the gap table out of Markdown into `scripts/parity-gaps.json`, so
  every count is produced by a script instead of remembered — the plan had
  convicted itself of "hand-counted numbers are unreliable" and then kept
  hand-counting;
* locks the behaviour layer: Pi 0.84.2's source is snapshotted into
  `references/pi-0.84.2/` with a sha256 lock, and
  `scripts/pi-oracle-lock.py --verify-refs` resolves every one of the **89**
  line-number references in the tree (v2's plan counted 31 — it had only
  counted the `.js` ones) and reports the ones that drifted;
* makes every exit criterion a command with an exit code, including the ones
  that used to be "tick the box" or "look at it in tmux";
* resolves the conflict with `docs/GOALS.md`, which had a second, different
  ordering rule and a second "only list".

`scripts/plan-selfcheck.py` checks that the plan and the ledger still agree
(9 checks, all green), and `scripts/parity-gap.py` refuses to report on a
ledger with an empty field.

### Two things this round found that the ledger did not know

* **A5 — the TUI does not create a session file.** The footer says
  `[no session]` and the conversation is gone after exit; only `/new` (or
  `/fork`) creates a file. Measured on v0.10.7. The README's known
  limitations claimed the opposite ("sessions themselves are persisted"), so
  both READMEs now say what the binary does. Tracked, not fixed — it is a
  product decision, and it is bigger than this round.
* **A6 — a queued message is not delivered after an interrupt.** Pi steers it
  into the running turn; nini only flushes the queue on the next spawn, so a
  message typed right after an Esc waits for another Enter.

The provider counts in the docs were wrong too: Pi 0.84.2 ships **40**
builtin providers (the plan said 51) and nini implements **9** (the plan said
5). Both READMEs and `docs/GOALS.md` now carry the measured numbers.

## v0.10.7 — the ASCII stand-in had to cover the whole box-drawing block (2026-10-23)

1550 tests, all green. v0.10.6 fixed the frame width; this completes it.

### One character was missing, and it was the one that showed

The ASCII stand-in for a terminal that draws ambiguous characters two
cells wide was a hand-written list. It had every box-drawing character
except `┤` (U+2524) — the right-hand junction of a table's separator
row. So every separator was one column wider than the rows above and
below it: the frame fitted, and the borders did not meet.

The list is now a **range rule** over the whole block: horizontal to
`-`, vertical to `|`, corners and junctions to `+`, block elements to
`#` and `.`. A character cannot be missed by being forgotten.

### The test that would have caught it

The frame test measured whether a row *fits* the terminal. It could not
see a frame that is the right width and still wrong, so it is joined by
two assertions:

* rows are grouped into bordered blocks, and every row of a block must
  have the same width as the first;
* the measurement ignores the trailing padding — the buffer is always
  the terminal's full width, so measuring it would say every row is
  exactly as wide as the terminal and the alignment question would
  never be asked.

Verified by injection: halving the rule segments again (the mistake this
change fixes) turns the assertion red with `a bordered block is not one
width`.

## v0.10.6 — the frame was drawn at twice the terminal's width, because of the box-drawing characters (2026-10-23)

1550 tests, all green. The report's second photo: every row shifted by a
different amount, and the horizontal rules cut in half.

### The frame was twice as wide as the terminal — because of the box-drawing characters

Every character nini draws as chrome is East-Asian **Ambiguous**: `─ │ ┌ ┐ └ ┘ ├ ┤ ┬ ┴ ┼`, the `░ ▓ █` context bar, `• ▸ ✓ ✗ ❯ ↑ ↓`, and the braille spinner. `unicode-width` — and most terminals — draw those one cell wide. A terminal configured for a Chinese, Japanese or Korean locale very often draws them **two**.

That is not cosmetic. A row of `cols` cells carrying *k* ambiguous characters is `cols + k` columns wide in such a terminal: the row overflows, the terminal folds it, and **every row below it moves down a line**. One rule is enough to shift the whole lower half of the screen — the transcript, the input box, the cursor. Measured on this repository's own TUI capture at 100 columns: **501 of 4920 rows overflow, the widest at 200 columns**. That is what the report's "乱码 + 下划线" looks like, and why each row in the screenshot was shifted by a different amount.

There is no capability query for this, so nini now **asks the terminal**: it writes a known number of `─` on a scratch row, sends a Device Status Report, and reads back where the cursor ended up (`crates/nini-tui/src/ambiguous_probe.rs`). Eight characters, one column each → narrow; sixteen → wide. The ladder is measure, then the locale, then the historical assumption; and `NINI_AMBIGUOUS_WIDTH=wide|narrow` overrides all of it, because a terminal that does not answer the probe (tmux does not) or does not follow its locale is a real thing.

In wide mode the frame is drawn with one-cell ASCII stand-ins — box drawing to `+-|`, bars to `#`, bullets to `*` — and every layout decision is sized with the widths the terminal actually uses, including the table frame's minimum (Pi's `3n + 1` becomes `7n + 2`). The assertion lives in `crates/nini-tui/tests/frame_fits_cjk_terminal.rs`: no row of the frame may exceed the terminal *when measured the way a CJK terminal measures it*, at 60/80/100/120 columns, for every kind of line nini can draw — prose, tables, quotes, code fences, tool calls, tool results, bash output, and the running status line with its spinner and context bar.

## v0.10.5 — a character cut in half by the network killed the whole turn (2026-10-22)

1548 tests, all green. The report is 「一遇到复杂任务/工具调用任务就乱码」, and
v0.10.4 fixed one cause of it. This is another one, and it is the one that
matches the report's shape: a *streaming* failure, in Chinese, while a task
is running.

### The parser decoded each chunk on its own

`SseParser::feed` decoded **each network chunk in isolation**:

```text
let s = std::str::from_utf8(chunk).map_err(|e| SseError::InvalidUtf8(e.to_string()))?;
```

A chunk boundary is wherever the network put it, and a Chinese character is
three bytes long — so one of every three of them lands on a boundary from
time to time. The chunk that holds the first byte of a character is not
valid UTF-8 on its own, and the decoder said so: `ProviderError::Sse`, the
turn died, and the screen kept a half-written answer and an error.

The bytes of a character that has not finished arriving are not an error —
they are a character in transit. The parser now holds them and prepends them
to the next chunk. A byte that can never be valid (0xFF) is still an error,
because buffering it would hide a broken stream.

### The failure is now deterministic, and it is a script

"Every so often" cannot be an acceptance criterion, so the mock provider
grew a `--split-bytes` flag: it writes the stream in N-byte pieces, the way
a real network does. `scripts/e2e-hostile-stream.sh` then drives the **real
TUI** against hostile content — Chinese, emoji, a table, a heading, a code
block — and asserts that the answer is *whole*, that the table's columns
line up, and that no replacement character (U+FFFD) reaches the screen.

It fails on the published v0.10.4 binary:

```text
FAIL
  - the stream died with `invalid utf-8` — a character was cut in half by a chunk boundary
  - the answer is not complete on screen
```

and passes on the fixed one. That is the difference between "I think this
is fixed" and "the failure is a script that goes red without the fix".

## v0.10.4 — the TUI, the library and the terminal were all writing to the same screen (2026-10-22)

1540 tests, all green. The report was 「一遇到工具调用任务就乱码」: a tool call makes the screen garble.
Three defects, all of them two writers fighting over one row.

中文：nini 是一个干净的 Pi agent，用 Rust 重写——作者力求保留 Pi agent 最初的原滋味。
本版修的正是"TUI、库和终端同时往同一块屏幕上写字"这件事。

### 1. The library wrote to the terminal the TUI was painting

#### Two texts, one row

The report was "一遇到工具调用任务就乱码" — a tool call makes the screen
garble. The cause is one line, printed on **every** tool call:

```text
[nini] trust: cwd '/home/jin/nini2' is at Ask level; defaulting to Allow (v0.8 will add interactive prompt)
```

`nini_core::project_trust::check` prints it, and the default trust
level is `Ask` (there is no `trust.json`), so every ordinary user saw
it on every call. It went to fd 2 — the alternate screen the TUI is
painting — and the repaint interleaved with it:

```text
 nini   ⠼[nini] trust: cwd '/home/jin/nini2' is at Ask level; d5f3ulting 6o Allow (v0.8 will add interactive prompt)
```

The `5` and the `3` are the TUI's own `ctx 3%` and `out 662` digits.
The escape-sequence capture shows the polluted characters carrying
`[38;2;102;102;102m` — the status bar's own dim style. It was never a
terminal encoding problem: two writers, one row.

#### The choke point

The TUI is the only thing that owns the terminal, so it is the only
place where this can be fixed. While the TUI runs, fd 2 is now a
**pipe**: `crates/nini-tui/src/terminal_guard.rs` dups the real stderr
aside, points fd 2 at the pipe, and reads it on a background thread.
Every line becomes a `TranscriptLine::Notice` in the transcript — dim,
prefixed with `⚠`, and unmistakably not model output. The TUI's own
writes (`/copy` without a clipboard, a settings write that failed) took
the same road.

Three properties the guard has to keep, each with an assertion:

* **Nothing is lost.** A line the reader could not deliver goes back to
  the real stderr when the guard is dropped.
* **Nothing deadlocks.** If the reader thread ever stops, fd 2 is
  handed back immediately — otherwise the pipe fills up and the next
  `eprintln!` wedges the whole process.
* **stdout is untouched.** The renderer writes to stdout; changing
  stdout's meaning would break the frame, not fix it.

#### The recurrence has a name now

`crates/nini-tui/tests/no_terminal_writes.rs` reads the sources of
`nini-tui` and `nini-core` and fails on any `print!`/`eprintln!` that
is not on a short whitelist with a stated reason. The whitelist pins a
**count** per file, not a line number, so a new write in a whitelisted
file breaks the test too: an exemption has to be re-decided, not
silently widened.

### 2. The scroll indicator was painted over the transcript

The `↑ N more above` hint was drawn on the same
row as the first line of the transcript, and the `↓ N more` hint on the
row of the last one.

#### The hint ate the text

`render.rs` painted the transcript list into the whole transcript area
and then drew the overflow indicators on top of it — the first row for
`↑ N more above`, the last row for `↓ N more`. The indicator is 17 cells
wide, so the beginning of the first visible line was gone:

```text
 ↑ 30 more above r: turn `AppState` into ratatui `Frame` drawing calls.
```

(the line was `//! Render layer: turn `AppState` into …`). The same
overwrite chopped the input box's border into fragments — the
"underlines" the report showed.

The indicators now get a row of their own, and the transcript gets one
row less. Which row is reserved depends on the mode: the autoscroll
hint sits on the first row (there is content above the fold), the
user-scroll hint on the last (there is content below it). The decision
is made against the *full* height first, so "how much is hidden" and
"how much room is left" never depend on each other.

Everything that counts rows moved with it: the visible height, the
hidden-above count, the items skipped from the bottom, the page size
PageUp scrolls by, and the scrollbar's viewport — the bar describes
the content area, because the reader scrolls the transcript, not the
hint.

### 3. The review found three new problems — one of them worse than the bug

An independent review (`nini-reviewer`) of the two fixes above found
that the root cause was right and the *edges* were not. Three of its
findings were serious enough to be fixed before shipping.

**The exit could hang.** The guard turned fd 2 into a pipe and its
`Drop` joined the reader thread — which only returns when every write
end of that pipe is closed. The external editor is spawned with
inherited stdio, and `$EDITOR=code` (without `--wait`), `emacsclient` or
`gvim -f` all leave a daemon holding the descriptor. So Ctrl+D never
returned the shell, on a terminal that had already left the alternate
screen. That is worse than the garbled screen it replaced.

The stop is now a signal, not an EOF, and the wait is bounded: a
self-pipe tells the reader to stop, a completion channel with a timeout
means a wedged reader is detached rather than waited on, and the editor
is not a pipe citizen at all — the guard is suspended while it runs, so
the editor's own diagnostics reach the user's terminal.

**The slot was claimed with a check-then-act.** Two threads could both
be halfway through installing the guard, and the second one would save
*the pipe* as "the original stderr". A failed thread-spawn left the
global slot pointing at a descriptor that was about to be closed: every
later install was refused, and the panic hook's `dup2` pointed fd 2 at
whatever that number meant by then. The slot is claimed with one
compare-and-swap now, and a failed install leaves nothing behind.

**The reader blocked on the render loop's lock.** It took the state
lock to push a notice, while the render loop held the same lock for
whole key handling. A writer filling the pipe could block the process.
`try_lock` now, and a line that cannot be delivered goes to the real
stderr at exit instead of waiting.

### 4. stdout was invisible to the test that guards the screen

The guard captures stderr. stdout is the screen the renderer paints on
and cannot be captured — so a stdout write during a TUI session is the
original defect, unfixed. And the test written to prevent a recurrence
could not see it: it grepped four macro names, treated `print!` and
`eprintln!` as the same thing, and scanned two crates. Measured: turning
one `eprintln!` into a `println!` in a whitelisted file left the test
green, and the bug was back.

stdout and stderr are judged separately now. stdout is a violation
everywhere except the one-shot commands (`nini info`, `nini demo`,
`--list-models`), which never open a TUI. stderr stays allowed in the
files that are listed, with the count pinned. Every crate is scanned,
`writeln!(io::stdout(), …)` is caught as well, a whitelist entry whose
file no longer writes is reported as a stale exemption, and a new crate
that nobody scans fails the test.

### 5. The copy fallback put a whole answer into the transcript

`/copy` without a clipboard used to print to stdout — the alternate
screen, which is how a copy became garbage. It became a transcript
notice instead, and then it became too right: the whole assistant
message went in. A 200 KB answer is 200 KB of rendered rows, all of it
indexed for search, all of it in the HTML export, for a fallback whose
job is to show the reader something they can copy from somewhere better.
The notice carries the head of the message and a pointer to the rest.

Two smaller corrections in the same pass: the `↓ N more` hint stayed on
screen when a stale scroll offset said there was more below while every
line was already visible (and it took a row the content could have used),
and the trust test that claimed a trusted project "never asks" only
asserted the decision, not the silence.

### 6. The second review found four things that were not true

The fixes above were reviewed again, independently, and the review found
that the *repair* had four defects of its own — none of them the
garbled screen, all of them the project cares about more: claims that
were not true.

**The copy fallback counted bytes it never copied.** It said "copied 3482
bytes to the transcript" — 3482 being the length of the *source*
message, while the transcript received a truncated copy of it. The
message now says what happened: the clipboard was unavailable, the
message is in the transcript above, the full text is in `/export`.

**A notice reached the HTML export unescaped.** A notice can carry
model output (the copy fallback puts the assistant's text in one), so a
`<script>` in a model answer became markup in the exported file. Notices
are escaped like every other content that reaches the export.

**A notice was clipped instead of wrapped.** It was built as one string
and drawn in a `List`, which clips rather than wraps — so the tail of a
long diagnostic was never on screen. A notice that stops mid-word looks
exactly like the corruption this whole change was about.

**The order that keeps lines from being lost had no assertion.** The
guard has to stop delivering to the transcript before the alternate
screen goes away; the call was once missing and nothing noticed. There
is now a test that reads `runtime.rs` and fails if the order is wrong.

Two smaller ones: the dropped-lines notice now says *how many* lines
were dropped (and says it even when the reader thread was detached and
could not say it itself), and the structural test now catches
`print ! ("…")` — Rust allows whitespace between a macro name and its
`!`, and the test had been blind to it. The crate-coverage test was
also rewritten: it compared a list with itself, so it could never fail,
and it now compares the scan with a list written out by hand.

### 7. Five more claims that were not true

The third review round (and one end-to-end run that turned out to be
*testing the wrong thing*) found five more:

* **The exported session carried live markup.** The export escaped what
  the user typed and what the model said, but not the tool arguments,
  the bash command line, or the provider's stop reason. A model asked
  to `echo '<script>…</script>'` produced a working `<script>` in the
  exported HTML, which the user then opens in a browser.
* **The trust warning was printed under the trust store's lock.** While
  the TUI runs, stderr is a pipe; a blocked print held the mutex every
  other tool call waits for, so the agent could look frozen with no way
  out. The decision is now taken under the lock and printed after it.
* **The queue died with the queue.** A diagnostic that the reader had
  read but the render loop had not yet drawn was dropped on exit: not
  in the transcript, not in the scrollback, gone.
* **The "N lines were dropped" warning appeared twice** — once from the
  reader, once from the guard. A warning that appears twice is a warning
  nobody reads.
* **The provider could fail silently.** A missing API key left the TUI
  talking to a fixture provider while the status bar said
  `(openai-compat) MiniMax-M3.1-Flash-Preview`; the user's message went
  in and nothing came back. The reason is now a transcript notice, and
  the status bar names the fallback.

Plus two that were not "claims" but plain costs: the guard's batch was
drained one line at a time (now one operation on the transcript per
drain, which is an upper bound rather than a measured speed-up — the
per-line form did **not** copy the transcript, because nothing else
held a clone of it at that moment); and the scrollbar's thumb was drawn
in rendered rows but hit-tested in transcript lines, so a press on the
visible thumb made it jump.

### 8. The notice queue is the guard's own

The reader thread used to take the render loop's state lock to push a
notice — with `try_lock`, so it could never block. That was right about
the blocking and wrong about everything else: `try_lock` turns every
collision into a **lost** notice. The line went to the residual, and the
residual is written to the real stderr when the guard goes away, so the
user saw the diagnostic in their scrollback at exit, or not at all,
instead of in the transcript. It happened on the very first tool call of
a session, because that is when the render loop is busiest.

The reader now hands the lines to a queue the guard owns, and the render
loop drains that queue on every tick. The reader never touches the
state lock: a writer that fills the pipe cannot block, and a line is
never lost because the render loop happened to hold the lock at that
instant.

### 9. The trust warning repeated once per tool call

Routing stderr into the transcript made a second defect visible: the
warning was a fact about the *tool call*, and `check()` runs before every
call, so a twenty-tool-call task produced twenty identical rows.

It is a fact about the **project's** trust posture, so it is now stated
once per cwd (`ask_level_warning`). The test that says so is red when the
dedupe is removed — it was verified that way, not assumed.

### 10. The last review found a claim of ours that was not true

The end-to-end run that finally *worked* (it had been testing nothing:
no API key in a non-interactive shell, so the provider never started)
found four more things, and one of them was a claim we had written
ourselves.

**The claim was false.** The comment said every `push_notice` deep-copied
the whole transcript — because the render loop's snapshot holds another
`Arc` clone — so a 200-line drain copied the transcript 200 times and
froze the UI. It does not: the snapshot from the previous iteration has
been dropped by the time the next drain runs, so the refcount is 1 and
`Arc::make_mut` copies nothing. The comment, the paragraph here, and a
structural test that locked the false premise are all corrected. A test
that locks a false premise is worse than no test — it sends the next
reader looking for a cost that was never paid.

**The exit printed the tail before the head.** The guard flushes two
buffers on the way out: the queue (read before the guard started
closing) and the residual (read after). The reader writes the residual
out when it stops, and the queue was flushed *after* it, so the older
lines were printed last — a diagnostic that arrives in pieces showed
its tail before its head. The queue is flushed before the reader is
told to stop.

**The width was measured in the wrong terminal.** The CJK fix covered
the warning sign in the prefix; the body has ambiguous characters of
its own (`→`, `…`, `✓`, `•`), and a library diagnostic is full of them,
so the row was still one column per ambiguous character too wide in a
CJK terminal and the tail was still cut.

**The fallback could be talked out of.** The provider's failure was
reported once, in a notice. The notice scrolls away, and `/model`
wrote the model name straight into the state — so the status bar went
back to looking completely normal while the app was still talking to
the fixture, and the user had no way to tell that no model would
answer. The reason now lives in the state, and every later model change
says it again.

And `ProjectTrustStore::check()` — the "decide and print" shape this
whole campaign removed — is test-only now. It was a public API waiting
for the next person to put the print back under the lock.

### The installer named a repository that had been renamed (2026-10-21)

v0.10.3 is published (tag + release + binary asset); this is what came
out of shipping it.

### The one-liner that could not install anything

`install.sh` said `REPO="jinxumi/nini"`. The repository is
`jinxumi-oss/nini`, so **every** download 404'd — including the ones for
releases whose binaries were published and sitting there waiting:

```text
$ curl -sIL https://github.com/jinxumi/nini/releases/download/v0.10.1/nini-x86_64-unknown-linux-gnu.tar.xz
HTTP/2 404
```

The header's own one-liner carried the same wrong slug, so the documented
command fetched a 404 and piped it into bash. The fix is one string; the
part that mattered is the **assertion** that did not exist: `install.sh`
is shell, the suite is Rust, and no test ever read the file. The 404 was
invisible from inside the repository — the tag existed, the release
existed, the asset existed, and the installer was the only broken link.
`crates/nini-cli/tests/installer_source.rs` now checks the script
against the world rather than against itself: the slug the releases are
published under, the one-liner in the header, and the asset naming
convention the published releases carry.

### The gate caught its own author

That fix is a `fix:` commit, so `scripts/invariants.py` counted it as a
new invariant — and with no assertion, the bare-invariant count went
**1 → 2** and `scripts/gate.sh` went red. Which is the gate working:
a fix without an assertion is a fix that can recur. The assertion above
brings the count back to 1.

## v0.10.3 — the audit that asked what a fix costs when it leaves no assertion (2026-10-19)

1488 tests, all green. The theme: **every parity claim in this file had a
fixture of its own making.** Two of them turned out to be wrong about the
real Pi, and the way they were wrong is now measurable.

### What a fix without an assertion costs

`scripts/invariants.py` walks all 90 `fix:` commits — each one a real
failure whose subject is already a decidable invariant — and asks whether
any test asserts on the fix. Of 90: 64 shipped their own test, 8 were
corroborated elsewhere, **18 have no assertion at all**. Regression
protection is 80%.

That was the opening position, and the campaign closed it at **1**: 67
ship their own test, 7 are corroborated elsewhere, 15 are now *declared*
(`//! invariant: <sha>`), and 1 is still bare — `78593581`, a second
clipboard entry point. `scripts/invariants.py` prints the tally and the
gate locks it at 1, so it can only go down.

That number is not the problem. The problem is that a test written inside
the same commit as the fix is **same-source**: it blocks regression, and
it cannot block the misunderstanding that produced the fix. `stop_reason`'s
casing and the 3 footer falsity copies both lived under green tests for
exactly that reason.

### The first oracle that could not have been written from the bug

`nini-ai`'s streaming `tool_call` had 4 unprotected defects of precisely
this shape — *args dropped on first chunk*, *inline-args emits no Stop*,
*single-shot and streaming share one Start*. `openai_wire_e2e.rs:18` says
its fixtures are **"hand-built"**, and a hand-built sample cannot catch a
wrong understanding: the sample and the parser share the one error.

So chain 2 was built. `scripts/capture-sse.py` proxies Pi to its provider
and writes each SSE event as it arrives — event granularity, because
`arguments` arrives in increments and the boundaries *are* the evidence.
What Pi actually sends, unedited:

```text
[0] delta is role only — content/tool_calls are absent, not null
[1] index:0 id name:"bash" arguments:"{"     <- a lone open brace
[2] index:0, no id, no name, arguments increment
[3] index:0, arguments:"}"
[4] finish_reason:"tool_calls", tool_calls null
[5] "choices":[] usage-only
```

Re-injecting the original bug turns exactly one of the 5 new assertions
red, with `input_json: Null`. An expectation corrected by that experiment:
nini's own `inline_args_on_start_chunk_emits_complete_stop` fails under
the same mutation, so these 5 are not "more tests". The real difference is
the **shape** being asserted — the existing sample puts a complete body in
the first chunk and its comment calls that "aiio-shape"; this provider
sends a lone `{` and streams the rest.

### TUI: record the screen, not my belief about it

`/hotkeys` printing `Home ?` was caught by looking at a screen. No test
caught it. Screen recording was impossible because model output varies —
so the upstream was replaced. Pi identifies providers by protocol, not by
who answers, which was verified rather than assumed: it drives
`scripts/mock-provider.py` and replies with the script verbatim. Same
script now feeds Pi and nini, so the difference between their screens is
the parity difference — deterministic, free, offline.

Four isolation traps, each measured: user skills leaked into the boot
screen (29 names — a snapshot, not an oracle), an empty trust store blocks
on a prompt, a reused tmux server inherits its old environment, and
`send-keys -l` corrupts input (`/hotkeys/hotkeys` one-shot, `0/ho`
char-by-char; `load-buffer` + `paste-buffer` is correct).

The screen then showed: Pi draws `/hotkeys` as a boxed table, nini emits
one flowing blob; and Pi binds `Ctrl+-` Undo, `Ctrl+Z`, `Shift+Tab`,
`Ctrl+X`, `Alt+Enter`, `Alt+Up`, and `/` `!` `!!`, none of which nini binds.

### Exports compared as data, because pixels were the wrong axis

Pi's `/export` is a self-contained HTML whose `entries` sit in a `<script>`
as base64(JSON) — `{header, entries[8], leafId, systemPrompt, tools}`.
nini's is `render_transcript_html(&[TranscriptLine])`: it renders the
structure away before writing. 279,049 bytes vs 815 is the symptom.

Compared on content preservation, with a real exit code: Pi keeps all six
things that happened, nini drops `thinking` (8 structured entries vs 0).

### Two flags accepted, documented, and dropped

`--export <path>` (`main.rs` had `export: _`) and `--tui-mode`
(`tui_mode: _`) were both advertised in `--help` and discarded.
Measured: `nini --export /tmp/x.html -p hello` exited 0, `/tmp/x.html` did
not exist, and the HTML landed in `~/.nini/agent/exports/`. `--tui-mode` is
a real Pi flag; nini ships only the regular layout. Both are now refused
with a reason.

The interesting part is why no test noticed. `cli_flag_contract.rs` is
**enumerative** — it lists the flags it cares about — so a flag added later
and dropped on the floor was never caught unless someone remembered to add
it to a list. It is now **coverage-based**: read the source, find every
field destructured as `_`, require that each advertised one is named in a
refusal. That test found `--tui-mode` the moment it existed.

### A gate, with a baseline that cannot silently regress

`scripts/gate.sh` — fmt, clippy, two build profiles, tests, and two
baseline locks: bare invariants (**1**, down from the 18 the campaign
opened on) and unwired signatures (51) may not increase. Full-green tests are not the same as correct, so the gate locks
the trend instead of trusting the count. It was self-tested by lowering the
baselines: both checks went red, exit code 1.

Details and the open items (`tui-oracle.py`'s extractor is not yet
trustworthy and is kept out of the gate) are in `docs/invariant-audit.md`.

### The tag, corrected before it left the machine (2026-10-21)

This release was audited before it was published, and the audit found
three things that would have shipped broken:

- **`Cargo.lock` still said 0.10.2.** A clean checkout of the tag could
  not build with `--locked` — `cargo metadata --locked` exited 101. The
  lockfile now carries 0.10.3, so the tag builds the version it names.
- **The `--full` version check could only fail.** It grepped the output
  of `nini --version` for the word "commit"; the binary prints
  `nini 0.10.3 - 34ddd5a +dirty (...)` and never that word. The check
  now matches the sha it was always meant to match.
- **The README contradicted itself.** The badge claimed 1488 tests, the
  body claimed 1436, and the version stamps said v0.10.2. All three are
  corrected, and the gate now reads the body's count as well — one
  number, read in both places, equal to the measurement.

Two further corrections to the gate: an unparsable test count used to be
a *skip*, which is how a check quietly stops existing; and a
`//! invariant:` declaration now has to be backed by
`docs/invariant-verification.md` — for each declared sha, the bug that
was injected and the assertion that turned red.

## v0.10.2 — the binary could not tell you what it was (2026-10-18)

1428 tests, all green. Two themes, both found by asking a question of the
artifact rather than of the code: *which commit is this?* and *can this
binary do what its tests say it can?*

### A user's hook panic killed the whole session

`catch_hook_panic` (`nini-core/src/agent_hooks.rs:312`) exists so that a
**misbehaving user-supplied hook** degrades to a default and logs instead
of taking the agent down; `nini-core/src/agent.rs:2022,2057` does the same
around tool calls. Under `[profile.release] panic = "abort"` — which is how
nini has been built since it shipped — none of that ran. The process died
mid-turn, and the hook's own bug became the user's lost work. Pi's hooks
are TypeScript and cannot abort the process, so this was a behaviour-layer
divergence too.

It stayed invisible because **cargo forces `panic = "unwind"` for test
targets**, `cargo test --release` included. Every hook-isolation test in
this repo passed, under both profiles, while the shipped binary had the
guard compiled out. Verified both ways, with the real profile:

```console
$ cargo run --release --example panic_probe -p nini-cli
recovered: the release profile unwinds          # exit 0

# with panic = "abort" restored:
$ cargo run --release --example panic_probe -p nini-cli
thread 'main' panicked at panic_probe.rs:26:58
                                              # exit 134, no "recovered"
```

`examples/` is the only Cargo target that is built by the real profile
*and* is not a test harness, which is why the probe lives there and is now
a CI step. Cost of unwinding: 6 054 472 → 6 716 744 bytes (+11%).

### Four ways one number said four different things

`nini 0.10.1` was on its own an unanswerable question: the commit could be
nine behind the working tree, the release artifact could not name itself at
all, and `target/release` and `target/debug` were different programs
(`panic = "abort"`, overflow checks off, LTO on) claiming the same identity.
All four are now decidable:

```console
$ nini --version
nini 0.10.2 - 8c8a009 +dirty (2026-10-18 00:53:23 UTC) [release]
```

- `crates/nini-cli/build.rs` derives commit, timestamp, dirty and profile
  from git. The function's doc comment already promised "embedded by
  `build.rs`"; there was no `build.rs`, so CI's release artifact shipped
  `unknown (unknown)` and nothing noticed.
- `[profile]` is in the line because the two profiles genuinely are two
  programs. CI's `cli-smoke` now fails if it is missing.
- `rerun-if-changed` now covers `Cargo.toml`, after flipping `panic`
  relinked the binary while `--version` still reported the previous
  build's timestamp.
- `scripts/build-release.sh` builds, refuses to proceed if `--version`
  cannot name a commit, then smokes `info` / `demo` / `-p`.

### Also in this release

- **Six places where the TUI said one thing and did another** — the footer
  advertising keys the keymap does not have, search indexing 60 characters
  of each line, search hits that were never painted, `Up`/`Down` deleting
  the prompt, no scroll-to-top, and a thinking block that was a string
  rather than Markdown. Unreleased entries below keep the detail.
- A blockquote body carrying Pi's `mdQuote` + italic.
- One flake fixed rather than worked around: the bash timeout assertion was
  losing to scheduler noise at 2.5 s; the regression it guards takes 6 s.

### Upgrade note

Nothing to do. `~/.pi/agent/` config, session JSONL, and the slash-command
surface are unchanged.

## v0.10.2 · detail — thinking blocks and blockquotes render as what they are (2026-10-18)

Ten new tests; **1418 → 1428**. The last two items of the 10-17
audit list that had been implemented but not written up. Both are the
same shape of defect: **the palette and the header table promised a
rendering that the code did not perform.**

### A thinking block was a string

Pi renders each run of thinking blocks through a full `Markdown`
(`assistant-message.ts:147`), with
`fg("thinkingText") + italic` as the default style. nini split the
accumulated text on `\n` and handed the strings to the transcript, so
inside a reasoning block the model saw:

```text
  💭 - weigh the options
  💭 - **this part matters**
```

— raw list markers and raw emphasis delimiters, which models emit in
reasoning constantly. `thinkingText` was defined in the palette and
read by **nothing**; the block was drawn with `dim`.

`rich::render_thinking_block` now builds the Markdown first and applies
the style at the **outer** layer, so inline formatting sits on top of
`thinkingText` the way it does in Pi. The `  💭 ` gutter and the
continuation indent are nini's own and stay — Pi's label belongs to the
collapsed state only. The old `wrap_thinking_block` (v0.8.4, hard wrap
at 180 cells) is gone with it: wrapping is the renderer's job now, and
hard-wrapping text before it reaches the Markdown layer is what made
lists and fences unrenderable in the first place.

Pinned by `thinking_markdown.rs` (4 oracles).

### A blockquote body carried no style

`markdown.rs`'s own header table claimed the quote body uses `mdQuote`
+ italic. The implementation painted only the `│ ` border with
`mdQuoteBorder` and left the text with whatever inline formatting it
had — so `**bold**` inside a quote rendered bold, where Pi renders it
in the quote colour with bold on top.

The fix is applied at flush time rather than in `base_style`, because
Pi wraps the **finished** line (`quoteStyle(line)`,
`markdown.ts:557`). Patching the parser's base style instead is what
would leave the nested case wrong — which is precisely the case the
claim is about.

Pinned by `blockquote_style.rs` (6 oracles).

### What is still open from the same audit

One item survives, and it is the largest: **no syntax highlighting.**
Pi has `highlightCode` and uses it in the write tool's diff too. All
nine `syntax*` slots in the palette are dead; `theme.rs` says "reserved
for future synctact integration" and the README still does not say so.

## v0.10.2 · detail — Home jumps to the top, like Pi (2026-10-17)

Six new tests; **1412 → 1418**.

### The wiring, and why it is not a style choice

Pi binds `home` to `tui.altScreen.top` — "Scroll viewport to top"
(`pi-tui/src/keybindings.ts:208`) — and `end` to
`tui.altScreen.bottom`. That the editor *also* lists `"home"` for
`cursorLineStart` (`:186`) looks like a conflict, and it is one, resolved
by dispatch order rather than by intent:

```ts
// tui.ts:336
private inputListeners = new Set<TUIInputListener>();
// tui.ts:836
for (const listener of this.inputListeners) { … if (result?.consume) return; }
```

A `Set` iterates in insertion order, and `tui-alt-screen.ts:224` registers
the viewport's `handleViewportInput` at construction, before any child
component. `scrollToTop()` returns `{ consume: true }` (`:634`). So in
the full-screen layout **`home` reaches the transcript first, and the
editor's `cursorLineStart` is unreachable.**

nini had this exactly backwards, and had no top-jump at all:
`grep -rn ScrollToTop` over the workspace returned nothing. A user who had
scrolled back through hundreds of lines had one way forward: `PageUp`,
once per screenful — where `PageUp` counts `TranscriptLine`s and a single
one can render across fifty rows.

`Home` → `ScrollToTop`, `End` → `ScrollToBottom`.

### What the editor got instead

`MoveLineStart` / `MoveLineEnd` keep their real bindings: `Ctrl+A`, and
newly `Ctrl+E` — Pi's second key for each
(`["end", "ctrl+end", "ctrl+e"]`, `keybindings.ts:186`). Without this the
change would have traded one broken key for a missing one, which is what
P0-5's first draft did by redefining `move_to_start`.

### Two dispatch points, one of which dropped half the actions

`apply_action` is `pub` and its scrolling match was **entirely empty**:

```rust
KeyAction::ScrollUp | KeyAction::ScrollDown | KeyAction::ScrollToBottom => {}
// "this branch exists so the second key-dispatch site is exhaustive"
```

It has no production caller — only tests — but that is exactly how P0-5's
`Up`/`Down` stayed broken for three releases while tests passed, and how
the keymap ended up in three divergent copies the same day. The scrolling
is now one `scroll_transcript` / `scroll_to_top` / `scroll_to_bottom`
triple, called from both dispatch points.

### And a fourth copy, in the test suite

`tests/tui_e2e.rs` had `drive()` — commented "the same logic
`runtime::handle_key` uses, but inlined here". It had drifted three ways
while every test stayed green:

- `MoveUp` / `MoveDown` still called `recall_history(±1)` — the P0-5
  defect, so this file *could not* have caught it;
- `ScrollUp` / `ScrollDown` / `ScrollToBottom` were `{}`;
- `ScrollToTop` did not exist, so adding it did not compile.

`drive()` now delegates to `runtime::apply_action`. All 736 `nini-tui`
tests pass unchanged, which is the useful part: the expectations were
already right, the copy was just not the code under test. This is the
`nini-conformance` argument — "every test round-tripped nini against
itself" — applied to the test harness itself.

## v0.10.2 · detail — Up and Down could delete your prompt (2026-10-17)

Seven new tests; **1405 → 1412**. The last P0 from the 10-17 audit.

`Up` and `Down` were **only** a history browser. Both call sites did
`state.input.recall_history(±1)` unconditionally, so a single keypress
replaced the entire prompt buffer with an earlier message.

### Why that was destructive and not merely wrong

nini's renderer draws the input as several rows — `render_prompt` splits
on `\n` and paints a `❯` gutter per line — while `InputBuffer` had no row
concept at all: one byte offset, `move_to_start` / `move_to_end` meaning
"whole buffer", no way to ask which row the caret is on. The user *saw*
three rows, pressed Up expecting the caret to rise, and the draft was
gone. Rendering and editing disagreed about what a row is.

### Pi's ladder, implemented

`editor.ts:833`:

```text
  Up   : first row && (empty || browsing history || column 0)
            -> navigateHistory(-1)
         first row                        -> moveToLineStart()
         otherwise                        -> moveCursor(-1, 0)

  Down : browsing history && last row    -> navigateHistory(1)
         last row                         -> moveToLineEnd()
         otherwise                        -> moveCursor(1, 0)
```

The asymmetry is the point: **history is the fallback, not the action.**

Rows are *visual* rows — soft-wrapped, not just `\n`-separated — because
that is what Pi's `buildVisualLineMap` uses and what nini's own
`prompt_cursor_geometry` already computes for cursor placement
(`row = explicit_rows + column_in_line / width`). A caret that only
understood `\n` would disagree with the pixel it is drawn on for any
prompt wider than the box — the §22.4 family: the feature exists, the
arithmetic does not match the thing it describes.

All the geometry is in **display columns**, not bytes, or a CJK glyph
would put the caret mid-character and make Up jump sideways.

### The width dependency, which Pi has too

Vertical motion needs the prompt width, and the renderer is the only
place it exists. So `render_prompt` publishes it, mirroring Pi's
`this.lastWidth` — a second `AtomicUsize` next to the existing
`page_lines`, because `render_frame` takes `&AppState` and interior
mutability is the only route. It is published **unconditionally**: the key
handler must not depend on whether a cursor happened to be drawn.

`width == 0` (no frame yet) reads as "rows are `\n`-separated", so every
single-line prompt behaves exactly as it did before.

### One function, not two call sites

`handle_key` and `apply_action` each carried their own copy of this
match, and I fixed only the first — the tests went on failing because
`apply_action` was still calling `recall_history`. That is exactly how the
keymap ended up in three divergent copies (footer, `/hotkeys`, README)
earlier the same day, so the ladder is now a single
`move_prompt_vertically` both call.

`move_to_line_start` / `move_to_line_end` were added rather than
redefining `move_to_start` / `move_to_end`: the latter still mean "whole
buffer" and still back `Ctrl+A`, and silently changing `Ctrl+A` for every
existing user is not a fix, it is a second surprise.

### The oracle, and two wrong expected values

The first draft called `InputBuffer::move_visual_up` and set
`ui_state.prompt_inner_width` — neither exists yet, so it could not build
against the unfixed code. **A compile error is not an oracle**, and this
is the fourth time this project has lost a day to it. Everything now goes
through `runtime::apply_action` (same signature on both sides) and
establishes the width the way the product does, by rendering a frame.

Then two hand-written expected values were wrong and the implementation
was right:

- "a round trip restores the column" — no. Pi clamps to the target row's
  width and keeps nothing across rows; moving up into `line one` (8 cols)
  from column 10 leaves the caret at 8, and it comes back down as 8.
- The corrected draft then hand-computed the resulting byte offset and
  got *that* wrong too. It now asserts `(row, column)` from
  `visual_position` — the same "又一条我写错的期望值" the plan already logs
  twice (§19.6, §20.7).

A test whose expected value is derived by hand from a fixture is a test
that will be "corrected" to agree with whatever the code happens to do.

## v0.10.2 · detail — search found things and showed you none of them (2026-10-17)

Second half of the Ctrl+F fix in the entry below. Four new tests;
**1401 → 1405**.

Search was fully wired on the control side: `update_search_query` filled
`SearchState::matches` with line indices, the bar drew the `2/2` counter,
`n`/`N` moved `current`. Neither half of the *display* existed.

**The hits were not painted.** A TestBackend snapshot of a search with two
hits contains exactly one style across every transcript row:
`bg=Reset fg=Reset m=NONE`. Pi builds two closures at
`interactive-mode.ts:349` — `underline + bg(searchMatchBg) +
fg(searchMatchText)` for a hit, `bold + inverse` on top for the current one
— and hands both to the scroll view. nini's `searchMatchBg` and
`searchMatchText` were palette entries with no reader: the same dead-slot
shape as `mdCodeBlockBorder` (§32) and `thinkingText` (below).

**The viewport did not follow.** `search_next` moved `current` and wrote
nothing else, while `autoscroll` kept the transcript pinned to the tail.
Measured: a hit at line 60 of 121 in a 20-row frame, viewport showing
48–59, and pressing `n` left the screen byte-identical.

So the feature told you it had found two things and then displayed zero of
them.

### The fix

`SearchHighlight::{None, Hit, Current}` is threaded into
`items_for_line` and applied per `Span`. Two constraints shaped that:
ratatui resolves `span > line > item`, and every renderer in that function
sets a per-span foreground, so a `ListItem::style` would colour the
background and be ignored for the text; and `ListItem::content` is
`pub(crate)` in ratatui 0.29, so a post-pass over finished items is not
expressible — the paint has to happen while the `Line` is still ours.
That is why `items_for_line` now builds `Vec<Line>` and wraps at the end.

`reveal_search_match` sets `scroll_offset = total - 1 - idx` and clears
`autoscroll` (which otherwise takes the tail-aligned branch and ignores
`scroll_offset` entirely). It deliberately always scrolls rather than only
when off-screen: the renderer does not report visibility back into state,
and re-deriving it would duplicate the item-count cache the renderer owns.
`end_search` now calls `reset_view()`, because leaving `autoscroll` off
would strand the user scrolled into the past after they closed the search.

### Two more false greens, same family

The first draft of these tests passed for the wrong reason, twice over:

- `push_assistant` **appends to the current turn's line**
  (`push_assistant_raw` merges into the previous `AssistantText` until the
  turn breaks), so a 121-line fixture built from it was a **one-line**
  transcript. The "hit is 60 lines up" test was green on a fixture that
  contained one line.
- The viewport scanner matched `NEEDLE` in the **search bar's own query
  echo**, so "the hit must be on screen" was satisfied by the query text
  the user had just typed.

That is the same shape as the three `/hotkeys` tests: an assertion
satisfied by something else on screen. Four tests, three fixtures, one
lesson — a test that cannot distinguish the thing under test from its
neighbour is not covering it.

### And an unrelated flaky found while verifying

`bash::tests::timeout_does_not_sit_out_the_full_grace_period` failed one
full-workspace run out of four, then passed five times in isolation. It
asserts wall-clock `elapsed < 2500 ms` for work that takes ~1800 ms
unloaded, and a full run schedules ~87 test binaries. The bound is now
4000 ms — still far below the 6000 ms the pre-v0.8.9 behaviour needed, so
the regression it guards is still caught loudly.

## v0.10.2 · detail — four places where the TUI said one thing and did another (2026-10-17)

Found by an enumeration audit of `nini-tui` against the installed Pi
(0.84.2) source, not by user report. Six new tests; **1395 → 1401**.
All six were written to fail on the unfixed code first.

### The shape of it

The same defect lived in **three independent copies of the keybinding
table** — the footer's four branches, `/hotkeys`, and the README — and
all three disagreed with `default_keymap()` and with each other. Two of
the three advertised keys that **do not exist at all**:

| advertised | footer said | `default_keymap()` |
|---|---|---|
| `Ctrl+Z` | `undo` | **no such binding** (undo is `Ctrl+/`) |
| `Alt+Backspace` | `kill-word` | **no such binding** (`Ctrl+W` is) |
| `Ctrl+C` | `quit` | bound to `Abort` — which, with no popup open, **clears the input buffer** |

The third is worse than a dead key. The footer promised an exit and
delivered the destruction of half-typed input: the footer, the README,
and the implementation were three different claims about one key, and
the user's most likely reading of them was the one that lost data.

This is §35's family — a hint and an implementation that never met — one
version later, and it had grown a second and third copy meanwhile.

### The three tests that were hiding it

Fixing the footer turned three green tests red, and all three were false
positives rather than regressions:

```rust
let _ = dispatch(&mut state, &mut settings, CommandId::Hotkeys, "");
// ... then assert the frame contains "Ctrl+C"
```

`CommandOutcome::Output` reaches the transcript only because the runtime
pushes it; with the result discarded, nothing was ever added. The
assertion was satisfied by the **key hint bar**, not by the command. The
footer stopped lying about `Ctrl+C`, the three tests went red, and what
was underneath was a suite that had never checked a single command's
output reaching the screen. Two defects were covering for each other.

### Ctrl+F could not search a real answer

`line_summary_text_impl` is the *only* input `update_search_query` matches
against, and it truncated every assistant / thinking / tool-result line to
60 characters — under a doc comment calling it "suitable for transcript
search". Measured on an 88-character message:

```text
索引含 NEEDLE_AT_70 ? false
原文含 NEEDLE_AT_70 ? true
```

Model answers are routinely longer than 60 characters, so this was not an
edge case: `Ctrl+F` could not reach the text a user could see on screen.
The truncation is gone; the cost is one `to_lowercase()` + `contains()`
per keystroke over the line's own text.

### And a flake I had already located

`nini_core::logging` is a process-global `AtomicBool`, and `/debug` is one
of the 28 entries in `REGISTRY`. `dispatch_is_total_over_all_commands`
exists to call every command, so it wrote that global while
`debug_command_moves_the_real_logging_switch` read it as its baseline —
cargo runs those threads in parallel, and the test failed about one run
in ten. Fixed on both sides: `DEBUG_LOCK` in the test binary, and
`cmd_debug` now derives its new state from the global switch rather than
from the per-`AppState` field that could disagree with it. The second
change also fixes a real bug — after `--verbose` the CLI sets the global
while the field stays at its default, so the first `/debug` would have
switched logging *off*.

### Oracles

`tests/footer_binding_truth.rs` (2) and `tests/search_index_coverage.rs`
(4), plus `footer_quitting_mode_names_the_two_keys_that_actually_work`,
which replaces a test that had pinned the string `force-quit` — an action
that exists nowhere in the codebase.

The first draft of the footer oracle reported eleven unbound keys instead
of two, because I had guessed the modifier bit values instead of reading
them (`CTRL` is `0b010`, not `1`). A green test that cannot go red proves
nothing; neither does a red one for the wrong reason.

### `/hotkeys` is now derived, not written

It reads `default_keymap()` and labels every binding, so it cannot claim a
key the TUI does not have. First draft had two bugs the tests caught
immediately: characters rendered lowercase (`Ctrl+c`), and modifiers
compared with `==` instead of `contains`, which would have printed
`Ctrl+Shift+P` as a bare `Shift+P`.

### Still open from the same audit

Recorded here because this list is the audit's own record. Five of the
six items it named have since been fixed and each now has an oracle
that was red before the fix:

| Item | Fixed in |
|---|---|
| Search hits are invisible (`searchMatchBg`/`searchMatchText` defined, read by nothing; `search_next` did not move the viewport) | `feat(tui): six places where the TUI said one thing and did another` — `search_match_visible.rs` |
| Up/Down overwrite what you are typing (`InputBuffer` had no line concept, so `recall_history(-1)` ate the buffer) | same commit — `prompt_vertical_motion.rs` (visual-row geometry added) |
| There is no scroll-to-top (`grep -rn ScrollToTop` was empty; Pi binds `Home`) | same commit — `transcript_scroll_to_top.rs` |
| Blockquote bodies carry no style | `feat(tui): a blockquote body carries Pi's quote style` |
| Thinking text is not markdown | `feat(tui): six places where the TUI said one thing and did another` — `rich::render_thinking_block` |

**One item is still open, and it is the largest:**

- **No syntax highlighting.** Pi has `highlightCode` and uses it in the
  write tool's diff too. All nine `syntax*` slots in the palette are dead;
  `theme.rs` says "reserved for future synctect integration" and the README
  still does not say so.

## v0.10.2 · detail — an interrupt used to cost you your next four seconds (2026-10-16)

Reported as: "while nini is running, pressing Esc means I cannot type". Measured
in tmux against a provider that takes eight seconds to answer:

```text
  t≈ 2s after Esc   status "aborted"   footer "Enter retry  Esc clear"
  t≈ 6s after Esc   status "idle"      footer back to normal
  typing at t≈2s: discarded
  typing at t≈16s: works
```

### The cause: a state nothing returned from

`KeyAction::Abort` set `RunMode::Aborted`. Every input path gates on
`mode == Editing` — `handle_key`'s insert branch, `submit_user_input`, the
editor's own handlers — and nothing in the codebase ever moved *out* of
`Aborted` except the agent stream's `Done`, which arrives when the aborted
run winds down on its own. So from the keystroke after Esc until that
`Done`, every key was dropped with no message, no echo, no status change.

### Pi has no such state

```js
this.defaultEditor.onEscape = () => {
    if (this.session.isStreaming) {
        this.restoreQueuedMessagesToEditor({ abort: true });
    } else if (this.session.isBashRunning) { this.session.abortBash(); }
    else if (this.isBashMode) { this.editor.setText(""); this.isBashMode = false; }
    ...
};
```

`interactive-mode.js:2219`. The interrupt is synchronous and the editor is
usable the instant you press the key — no waiting for a stream to finish.

`RunMode::Aborted` is deleted. An abort returns to `Editing` and keeps
`run_state.status = "aborted"`, so the user still sees that the turn was cut
short.

### The footer promised a key that did nothing

`RunMode::Aborted` rendered `Enter retry  Esc clear`. There is no `Aborted`
branch anywhere in the submit path — `submit_user_input` returns unless the
mode is `Editing` — so `Enter` did nothing. The hint is gone, and the footer
offers `send`, which is true again.

### Oracles

`crates/nini-tui/tests/interrupt_editing_e2e.rs`. Stashing the source and
keeping the tests: **1 passed, 2 failed** —

```
an interrupt must not park the TUI in a state no key handler accepts
input in — every insert and submit is gated on Editing
```

They name `RunMode::Editing` and nothing else. `RunMode::Aborted` no longer
exists, so a test referencing it would fail to *build* against the parent —
and a test that cannot build is not an oracle, which this session has now
got wrong twice for the better.

### Still short of Pi

While a turn is *running*, nini still refuses input; Pi accepts it and queues
it (`restoreQueuedMessagesToEditor` is that queue). That gap is unchanged and
is not addressed here.

## v0.10.2 · detail — a thematic break was sixty columns of guess (2026-10-16)

The turn-divider fix above did not cover this report. "Long horizontal lines
late in a long task" had **two** sources, and the second one only shows up
when the model uses `---` — which it does constantly between sections of a
long answer, and which no scripted fixture ever produced.

`markdown.rs:466`:

```rust
// Use the full terminal width — but we don't know it here, so use a
// generous default. ratatui's wrap will clip anyway.
let rule = "\u{2500}".repeat(60);
```

The comment's premise was false. The caller *did* know: `rich.rs` holds
`max_width` and passes it to every other constructor in the same match arm.
`render_markdown(s, theme)` simply never received it.

### Pi's rule

`@mariozechner/pi-tui` `dist/components/markdown.js`, `case "hr"`:

```js
lines.push(this.theme.hr("─".repeat(Math.min(width, 80))));
```

Both halves matter. `width` because a rule that ignores it is not a rule,
it is a constant; and the **cap at 80** because that is what stops a rule
becoming a full-width band on a wide terminal.

### Why it hid below 60 columns

`rich.rs` truncates finished lines to `max_width`, so a 60-column rule
became a 40-column rule on a 40-column terminal. The old code was correct
below 60 and wrong above it — which is why the defect only appeared on the
terminals people actually use, and why it survived a look at narrow ones.

tmux, 100 columns, real binary, before → after:

```
line  4: ─= 60      line  4: ─= 80      ← min(100, 80)
line 37: ─= 89      line 87: ─= 89      ← prompt border, unchanged
line 39: ─= 98      line 89: ─= 98
```

### The fix

`render_markdown` takes the width, `MdState` keeps it, and `on_rule` uses
`min(width, 80)`. With no width — only reachable from tests or a degenerate
area — the rule is the shortest one that still reads as a rule (1), rather
than sixty columns of claim about a screen this code cannot see.

### The oracles, and two mistakes in reaching them

`crates/nini-tui/tests/markdown_hr_width.rs`. Stashing the source and
keeping the tests: **1 passed, 3 failed** — `expected the minimal rule when
no width is known, got 60`, and `each rule must follow min(width, 80), not
a constant: [60, 60]`.

They call `render_assistant_message`, not `render_markdown`, and the reason
is worth writing down: the first version called `render_markdown` directly
and **did not compile against the old code**, because that function is what
gained the parameter. A test that cannot build proves nothing — the same
trap the turn-separator tests fell into earlier in this same session.
`render_assistant_message(text, theme, max_width)` had its third parameter
on both sides of the fix.

## v0.10.2 · detail — a turn boundary is a blank line, not a rule (2026-10-16)

Reported as "the TUI still draws long horizontal lines". Reproduced in tmux
across four terminal widths, and the cause was a single line of code that
had been there since the first public release.

### The rule was `min(60, terminal width)`

`rich::render_divider` built `"─".repeat(60)` — sixty hard-coded
box-drawing characters, drawn after every turn and after the output of
nearly every slash command (`push_divider` has 20+ call sites). It knows
nothing about width, so what reaches the screen is the terminal clipping
it:

| terminal width | on-screen length | measured in |
|---|---|---|
| 200 | 60 — stranded mid-screen | tmux |
| 100 | 60 | tmux |
| 50 | 50 | tmux |
| 40 | 40 — a full-width rule | tmux |

The 40-column case is the report. At narrow widths it stops being a divider
and becomes a line across the screen.

### Pi draws no such line

Every message boundary in Pi is a `Spacer(1)`: `assistant-message.js:74`,
`:121`, `:131`, `:139`, `:144`, `bash-execution.js:30`,
`branch-summary-message.js:30`, `compaction-summary-message.js:31`. Not
one draws a rule. The single rule Pi does draw is sized from the real
width — `dynamic-border.js:18`, `"─".repeat(Math.max(1, width))` — which
is the prompt's own border, and which nini still draws correctly.

So this was never a rendering bug. It was an element nini invented, with a
length chosen once and never revisited, and with `render_divider_is_sixty_dashes`
asserting that 60 — a test that gave the defect a specification.

### The fix

`render_divider` is now `turn_separator`, and it returns one blank row.
`TranscriptLine::Divider` stays, and so do all 20-odd `push_divider` call
sites and the tests that count transcript lines: what a turn boundary *is*
has not changed, only how it is drawn. The separator is a single space
rather than an empty string — ratatui measures an empty `RLine` as zero
rows tall, which would have deleted the gap the rule was standing in for,
and the test that checks this fails if it regresses.

HTML export keeps `<hr class="divider">`; its stylesheet already carries
`margin:1em 0`, which is the HTML spelling of the same gap.

### The oracles, and their falsification

`crates/nini-tui/tests/turn_separator_e2e.rs`, stashing the source and
keeping the tests: **0 passed, 2 failed**.

```
at width 40, turn boundaries added 80 rule(s) to the frame
```

Two rules of forty — the defect, counted.

They compare a frame with turn boundaries against a baseline frame with an
empty transcript, rather than counting from a fixed row, because the
prompt's border is a legitimate `─` line and hard-coding a coordinate
would encode a layout guess.

One existing test, `transcript_width_e2e::clear_while_scrolled_up_resets_the_viewport`,
used "the divider is still visible" as its evidence that the renderer
works. That evidence died with the rule, so the probe is now a line with
content. The test's purpose is unchanged; its witness was the defect.

## v0.10.1 — one lock, eleven silent answers (2026-10-16)

### Release bookkeeping, stated plainly

The `v0.10.0` **tag points at `be783e9`**, which is where the GitHub Release
was created. The CI diagnosis quoted in the v0.10 section below (the
`RUST_BACKTRACE` step) landed afterwards, in `7009e55`, so that text is not
in the tag — it is in this release. The v0.10.0 assets were built from
`be783e9`, which is the state that text describes. The tag was not moved:
it is published, it carries a binary, and rewriting it is a destructive act
that belongs to the owner, not to the next commit.

The v0.10 release left `main` red: the `7009e55` run failed on
`ubuntu-latest` in `a3_seed::the_seed_carries_tool_calls_and_results`, with
the seed coming back as `[]`. The macOS leg passed that run, and had
SIGSEGV'd the run before it — so the note in v0.10 ("macOS is not green,
ubuntu passes") described a state that no longer exists and now describes
the reverse.

The failure was not a flaky test. It was a `try_lock`.

Tests: **1357 → 1360**.

### The category: contention encoded as data loss

`Session` lived in an `Arc<tokio::sync::Mutex<_>>`, and all eleven access
points took it with `try_lock`. Each one had invented its own answer to
"another thread holds it", and not one of them told anyone:

| Access point | What a busy lock meant |
|---|---|
| `session_context_messages` | an empty seed — the model is asked the next question with no history |
| `session_append_message` | the agent turn is never recorded |
| `session_push_message` | the user/assistant message is never recorded |
| `session_compact` | compaction silently does not happen |
| `transcript_from_context` | the transcript stops matching the session |
| `session_flush` | this write is skipped, and nothing logs that |
| `/tree`, branch summary | an empty tree; a status line stuck on "computing…" |
| `/fork` ×2 | "(fork: the session is busy — try again in a moment)" |

None of those is a state the product has. A lock held for microseconds by
another reader is not an error condition, and every one of those answers was
indistinguishable from the honest one: an empty session.

The contention was self-inflicted. `session_flush` holds the lock across
`write_to_file` — a full serialise of every entry plus an atomic rename —
and every turn end flushes. Measured on the pre-fix code, a 200-turn loop
read **198 empty seeds**: the flush thread owned the lock for the length of
the write and every reader looked straight past it.

### Why the lock was the wrong primitive

`tokio::sync::Mutex` exists for a guard that lives across `.await`. The
file's own comment claimed "`session` is held across `.await` points in the
agent task, so it needs the async mutex" — and that claim is false. Every
one of the eleven sites is a short synchronous section in an ordinary `fn`,
and the workspace contains exactly one `.lock().await`:
`nini-tools/mutation_queue`, a different crate and a different lock.

A mutex you cannot `.await` across is an invitation to `try_lock`, and
`try_lock` is where every silent failure above came from. The lock is now
`std::sync::Mutex`, poisoning recovered the way `lock_state` already
recovers it (`unwrap_or_else(|e| e.into_inner())`) — a panic while holding
the session lock leaves the session intact, and refusing to read it again
would turn a recoverable panic into total data loss.

### Fix — the flush thread no longer holds the lock across the write

Making readers wait is only correct if the wait is short. It is: the lock
is now held for one `Session::clone` and released, and the serialise and the
rename happen outside it. `Session` is `Clone`, so this costs one copy of
what was about to be written anyway.

What lands on disk is the session as of the flush call — what the previous
code wrote, minus the window in which every reader was blind. The write
error path moved from `eprintln!` to `tracing::error!`; a dropped session
write is invisible (the transcript shows the turn, a restart does not),
and `eprintln!` reached nobody unless a terminal happened to be attached to
the thread that lost the race. v0.10 wired `tracing`; this is the first
consumer on a path where losing it costs data.

`/fork`'s two "the session is busy" branches are gone. They could only ever
have been a lie told to a user mid-command.

### The oracles, and how they were falsified

Three tests in `crates/nini-tui/tests/session_lock.rs`, each written to fail
on the old code:

| Test | Fix | Property |
|---|---|---|
| `a_contended_read_yields_the_context_not_an_empty_seed` | the lock | a busy session is waited for, not reported as empty |
| `a_contended_append_is_still_recorded` | the lock | a message written under contention is still recorded |
| `flushing_does_not_make_the_seed_disappear` | the flush | 200 turn-ends, zero empty seeds |

`git stash` of the source, tests kept: **0 passed, 3 failed**, and the
assertion messages are the defects themselves. Two things were wrong on the
way to that, and both are recorded in the test file:

- The first version called `session.lock()`, which does not compile against
  `tokio::sync::Mutex` — so on the old code it *failed to build* rather than
  failing its assertions. A test that cannot run is not an oracle.
- The second version handed the lock to a spawned thread and read
  immediately. The holder had often not been scheduled yet, so a test of
  contention never contended and **passed on the code it was written to
  catch**. The holder now signals once the lock is actually held.

## v0.10 — Three chains that were built and never connected (2026-10-15)

A long task used to break in one specific shape: it would run, cross
the context window, and die. Not with an error — with the history
intact, nothing compacted, and a retry that could not succeed. The
cause was not one bug but a **category**: nini accumulates working
code that is never connected to anything, and the suite is green
because every test exercises the piece rather than the assembly.

Tests: **1280 → 1357**.

| Area | Change |
|---|---|
| **Compaction** | Pi's `_checkCompaction` runs after *every* assistant message; nini's ran between submissions. A run of 50 tool turns compacted **zero** times. Restored, routed through the session hook. |
| **Overflow** | The detector's error-message branch was dead — the production call passed `None`, and real 400s arrive on `StreamEvent::Error`, a different branch that treated them as fatal. Rewritten to Pi's three cases. |
| **Recovery** | No `_overflowRecoveryAttempted` equivalent: a failed compaction `continue`d with the same oversized context until `max_iterations`. Bounded at one, with an actionable message. |
| **Context bar** | `set_status_bar_usage` had no caller, so the status bar could only draw `ctx 0%`. `Usage` also dropped the cache counters Pi includes. Both wired. |
| **Project trust** | `/trust` persisted a decision and nothing ever read it during a run. A directory marked untrusted ran `bash`, `write` and `edit` anyway. Enforced through `BeforeExecute`. |
| **`-p` / `demo`** | Both called `build_tools()` directly, ignoring `--tools` / `--exclude-tools` / `--no-tools` **and** the trust gate. A restriction that holds in the TUI and evaporates in `-p` is not a restriction. |
| **`/fork`** | Copied transcript lines into a local variable and printed a count. No session, no file, no branch — so `/tree` had nothing to draw. Now `createBranchedSession`. |
| **CLI flags** | 16 accepted, described in `--help`, dropped. 5 now work; 11 refused with a reason naming the flag. |
| **Logging** | `tracing` initialised nowhere; `/debug` flipped a flag with no consumers. There was no logging at all. |
| **Themes** | `SettingsManager::theme()` returned `Theme::default()` and the runtime never read the settings at all, so `--use-theme`, `/settings` and the theme watcher all resolved to a hard-coded "dark". |
| **`/reload`** | Counted the skills on disk and discarded the count — the prompt is fixed per run, so nothing was reloaded. Now re-reads settings and names what cannot change. |

### Known issue in this release — macOS CI is not green

The `test (macos-latest)` job fails intermittently with **SIGSEGV** in
`nini-tui --test agent_wire_e2e`. `test (ubuntu-latest)` passes.

This predates the release: the v0.9.1 run of the same workflow failed on
macOS too (in a different test). It could not be reproduced on Linux,
including pinned to two cores, so the fix is not in this release and the
log says nothing about where it dies — a segfault gives no Rust backtrace.

What changed here is the diagnosis: the job now sets `RUST_BACKTRACE=1`
and names the job. Until it is reproduced, the honest statement is that
nini's macOS build is not continuously verified, and that is a real gap in
the release rather than a footnote to it.

> **Corrected by the `Unreleased` section above.** The `7009e55` run had
> macOS passing and `ubuntu` failing. The macOS SIGSEGV is still
> un-reproduced; the ubuntu failure had a cause, and it was in nini.

### Fix — a long run never compacted (Pi `agent-session.ts:1098` Case 2)

`agent.rs:956-975` had removed the in-run threshold check, on the
correct observation that a self-compaction never reached the session.
The follow-through was wrong: compaction moved to a check that runs
*between submissions*, and a single run never reaches it. A run of
50 tool turns therefore crossed the window with compaction disabled
for its entire life.

The check is back, routed through the session hook R2a built, so the
fold is persisted and the model's context and the session agree:

```rust
if let Some(result) = self.maybe_compact_at_threshold() {
    yield AgentEvent::Compacted { … };
    continue;   // the request below is built from the compacted history
}
```

Gated on a hook being present. Without one there is nowhere to persist
a summary, and an Agent that folds its own history records nothing —
the exact divergence A3 was right to object to.

### Fix — overflow detection could not see a real overflow

`agent.rs:1146` called `is_context_overflow(&stop_reason, None, …)`.
The `None` was the error message, so the only branch that matches a
real HTTP 400 (`overflow.rs:18-31`) never ran in the product. The
providers report the failure on `StreamEvent::Error`
(`nini-ai/src/openai.rs:418`), which reached a different arm that
classifies it as non-retryable (`agent.rs:216`) and ends the run.

`overflow.rs` is now Pi's three cases
(`pi-ai/src/utils/overflow.ts:134-162`): error-message patterns with
the non-overflow exclusions first (a throttled request must never
discard real history), silent overflow on a *successful* response
whose input exceeds the window, and the length-stop-with-no-output
case. The `Error` arm consults it before deciding fatality.

`is_recoverable_length` also gained Pi's `desiredMaxOutput > 0` guard
(`overflow.ts:172`) — without it an unset `max_tokens` resolved to a
`0 < 0` comparison that happened to be false for the wrong reason.

### Fix — recovery is bounded and says why

Pi allows one compact-and-retry
(`agent-session.ts:329/2001`) and, on the second failure, emits:
*"Context overflow recovery failed after one compact-and-retry
attempt. Try reducing context or switching to a larger-context
model."*

nini had no bound: `agent.rs:1179` `continue`d with an identical
oversized history, which overflowed again, until `max_iterations`
(50) tripped and the user saw `TooManyIterations` — a message that
says nothing about context.

The turn that hit the ceiling is now dropped whole — no assistant
message, no tool calls, no `turn_end` — and the request is rebuilt
from the compacted history, which is Pi's `willRetry`.

One deliberate deviation from Pi's ordering: a `length` stop that
*carried tool calls* is not compacted. Pi refuses that batch on
purpose (`agent-loop.ts:122`: *"its arguments may be truncated"*), and
compacting instead would run tools whose arguments were cut
mid-string. The next turn's threshold check does the compressing.

### Fix — the status bar's context bar (`set_status_bar_usage`)

The writer, the two fields and the renderer (`render.rs:390-405`,
`ctx 42% [████░░░░]`) all existed. Nothing called the writer, so
`context_used` stayed at its initialiser for the life of the process.
`AgentEventLite::Usage` also carried `(input, output)` and dropped the
cache counters, so the number could not have been right once wired:
Pi budgets with `totalTokens || input + output + cacheRead + cacheWrite`
(`compaction/compaction.ts:146`), and on a provider with prompt
caching the cached read is most of the context.

Context used is the **latest** turn, not a running total — the next
request carries the last context plus the answer to it.

### Fix — `/trust` now decides something

`ProjectTrustStore` has been fully implemented since v0.9.2 (atomic
0600 writes, per-cwd map, four levels, `check()`), `/trust` persists
and reads it back — and `check()` had **zero production callers**. The
`BeforeExecute` surface that exists to host such a gate had no
production implementation either, though the agent loop consults it
(`agent.rs:2019`).

So a security control displayed a decision and enforced nothing, which
converts "I marked this directory unsafe" into a belief. The gate
rides `BeforeExecute`, is keyed on the **session** directory rather
than the argument path (otherwise "untrust this repo" evaporates the
moment a call reads one file from elsewhere), and tells the user how
to change it when it denies.

`Ask` — the default, and the level a user who has never run `/trust`
is at — still allows. Turning a first release of this gate into a
prompt would break every existing session on upgrade.

### Fix — `-p` and `demo` had no tool policy at all

`nini -p "…"` is what CI and every script calls. It built its tool set
with `tool_registry::build_tools()` directly, so `--tools`,
`--exclude-tools`, `--no-tools` and `--no-builtin-tools` were accepted,
documented in `--help`, and applied to nothing — and the trust gate was
absent with it. All three entry points now share `build_run_tools`.

### Fix — `/fork` was a message, not a branch

`cmd_fork` copied `transcript_state.lines[..cut_at]` into a local
variable, appended the text `[fork] branch cut at user msg #N` to the
current transcript, and reported how many entries it had kept. Nothing
else happened. The code said so in a comment: *"We don't actually
create a new JSONL file here."*

So the branch could not be resumed (no file), `/tree` had no second
leaf to draw (v0.9.2's gap list: *"/fork creates no branches"*), and
the 585 lines of `branch_summary.rs` had no caller.

Pi writes a new session file whose entries are the retained path,
re-chained, under a header naming its parent
(`session-manager.ts:1412-1489`), then replaces the live runtime with
it (`agent-session-runtime.ts:318-325`). That is what happens now.

Two details that are not cosmetic:

- **`LabelEntry` re-chaining.** A label is a real tree node, so
  dropping one without re-linking orphans everything beneath it and
  `get_path()` returns a one-entry branch — which compaction, `/tree`
  and `/fork` all walk.
- **`session_id` was a second, unrelated id.** `session_begin()`
  minted a `Uuid::now_v7()` while the session carried `u_{v4}` in its
  header, so the number in the status bar and the number in the file
  were different values for the same conversation, and a fork's
  `parentSession` reference named something nothing could resolve.
  `session_load` already read the id off the header; only this path
  invented one.

Both of Pi's positions are supported, because they are different
actions: `/fork 2` branches *before* the second question and hands its
text back (Pi's default, which returns `selectedText` for the editor),
`/fork at 2` branches *at* it and keeps it.

The four `/fork` tests asserted the old behaviour — one of them
literally `assert_eq!(lines.len(), before + 2)` for "the echo +
divider", pinning the marker line that was *not* a fork. They now
assert what the command is for: a new file, the retained prefix, a
parent link, and a live state that became the branch.

### Fix — a theme system where the theme was the missing part

871 lines of `theme.rs`: the built-ins, `load_from_file`, `parse`,
`merge`, the full Pi palette, serde. `--use-theme` writing the field.
`/settings` offering a picker. A watcher reloading on file change.

And `SettingsManager::theme()`:

```rust
pub fn theme(&mut self) -> crate::theme::Theme {
    crate::theme::Theme::default()      // the name is never read
}
```

Worse, `runtime.rs` did not use `SettingsManager` at all — it built
`SettingsManager::default()`, whose theme field is the literal `"dark"`.
So the theme a user picked in `/settings`, the theme named by
`--use-theme`, and the theme in `settings.json` all stopped there, while
the **watcher** correctly re-read the real file and changed the colours
mid-session to something startup had never agreed with.

Resolution follows Pi (`settings-manager.ts:737-740`,
`package-manager.ts:2367`): `dark` / `light` are built-ins; a value
containing a `/` is a **path** (which is how a theme becomes shareable
rather than something every user copies); anything else names a file in
the agent's `themes/` directory. The rules live in `Theme::resolve`, which
both startup and the watcher call, so the two cannot disagree.

An unresolvable name falls back to dark **and says so on stderr**.

`--theme-file`, refused in the previous entry with the reason *"custom
themes are not implemented"*, is honoured again — that reason was wrong.

### Fix — `/reload` reported a reload that did not happen

It counted the skills on disk and threw the count away. The skills a
session carries live in the system prompt built at startup, so counting
them changed nothing the model can see.

It now re-reads the settings file, re-derives the context window (a
catalogue lookup, so a settings edit can move it), reports what changed
(`dark → light`), and says plainly that skills need a restart. The model
is deliberately *not* switched: `RunConfig` is cloned once per run, so
changing it here would leave the status bar and the provider disagreeing
until the next prompt — the class of bug R1e fixed for `/model`.

### Fix — flags that were accepted and dropped

Sixteen flags were destructured as `_` in `main.rs`. The comment above
the destructure said it plainly: *"these were all `_: _` … i.e. accepted,
documented in `--help`, and then dropped on the floor."*

**Five now work**, each against Pi's definition rather than a guess:

| Flag | Behaviour | Pi |
|---|---|---|
| `--system-prompt` | Replaces the base; skills, tool descriptions and the cwd hint still land on top | `system-prompt.ts:46-68` |
| `--append-system-prompt` | Appended to whatever base is in force, after it | `system-prompt.ts:41,49-51` |
| `--skill` | **Overrides** discovery, as its help says. Accepts a skills root, a single skill directory, or a `SKILL.md` | — |
| `--thinking` | Reaches `RunConfig::reasoning`, and beats the settings file | `main.ts:487` |
| `--verbose` | Starts the log before the first event | — |

**Eleven are now refused individually.** The old blanket message said
*"TUI session persistence is not implemented yet"* about `--offline`,
`--theme-file` and `--no-context-files` among others — none of which
has anything to do with sessions. A wrong reason is only marginally
better than silence, and worse once the user has learned to distrust it.
Each refusal now states what the flag would have done and what to use
instead.

`--skill`'s "overrides" is the operative word: appending to the
discovered set would make the flag unable to *narrow* anything, which
is the only reason to reach for it. A path that is not there is an
error rather than an empty skill set that looks like "I have no
skills installed".

### Fix — there was no logging

`/debug` flipped `ui_state.debug_logging` and printed a confirmation;
nothing read the field. `tracing-subscriber` was a declared, unused
dependency. A user who hit a problem had one documented diagnostic
action, and it produced no output.

`/debug` and `--verbose` now move one process-wide switch
(`nini_core::logging`). The subscriber is installed once, at DEBUG, and
the switch gates whether the call sites emit fine-grained events — a
`reload`-based filter would have to be threaded through every
construction site to stay reachable from a slash command, to change a
single predicate.

Output goes to `~/.nini/agent/log`, the location `/debug`'s help text
has always named. Initialised once in `main` before any entry point:
it lived in `run_tui`, so `nini -p` and `nini demo` — the entry points
CI uses — installed no subscriber at all. Same shape as the tool-policy
gap above: wired for one entry point, invisible for the others.

---

## v0.9.2 — Things that told you they worked when they didn't

> **Never released.** These fixes are in the v0.10 release above; tagging a
> v0.9.2 for them would have produced two releases of the same code, and the
> numbers below (950 tests) describe a state that never shipped.

The v0.8.9 and v0.9.1 releases fixed crashes. This one fixes a
different class of problem: **nini reporting success it had not
achieved.** Three of the four items below shipped through a fully
green 937-test suite with zero clippy warnings.

Tests: **937 → 950**.

---

### Fix — the TUI never told the model about your skills (P0-1)

`app.rs` built the system prompt with a hardcoded empty string where
the skills block belonged:

```rust
settings_to_system_prompt(&settings, &cwd, &shared_tools, "")
//                                                     ^ this
```

Meanwhile `settings_to_system_prompt` already accepted a
`skills_prompt` argument and appended it, `format_skills_for_prompt`
already rendered the Pi-style `<available_skills>` XML, and the status
bar told the user **"loaded 5 skills — type to begin"**. The only
caller that ever passed a real value was the scripted `nini demo` path.

So: interactive nini advertised your skills to you, and told the model
nothing. Every skill-driven workflow — the reason to run nini instead
of `cat`-ing a file at an LLM — silently did nothing.

This survived 937 green tests because the assembly lived inside
`run_tui`, an `async fn` behind a TTY that **no test can call**. There
was no seam. The fix extracts `build_tui_run_config()` so the prompt
is testable, then injects the real block.

The new tests were validated by re-injecting the bug and confirming
they fail: three go red, three that don't depend on the fix stay green.

`--no-skills` was wired in the same change. It was also being dropped
in `main.rs` (`no_skills: _`) while `--help` advertised it, and a new
always-on feature with no working opt-out isn't shippable.

### Fix — /copy claimed the clipboard and touched nothing (P0-4)

`clipboard::copy` was a seven-line stub whose body was `Ok(())`. Since
`cmd_copy` treated `Ok(())` as success, `/copy` printed

```
(copied 812 bytes to clipboard)
```

and the clipboard stayed empty. The stdout fallback further down the
call site was **unreachable** — it only ran on `Err`, and `Err` could
never happen.

`arboard` was already a dependency (`image_paste.rs` uses it for
images), so the real implementation cost nothing. On a headless
machine it now correctly reports the failure and prints the text:

```
(copied 15 bytes to stdout — no clipboard: clipboard unavailable:
 Unknown error while interacting with the clipboard: X11 server
 connection timed out because it was unreachable)
```

The lesson is now enforced in the module docs: **never report success
we cannot verify.** The dead `read_image` stub went with it.

### Fix — /logout removed nothing while reporting a tally (P0-5)

`/logout` called `std::env::var_os(env).is_some()`, counted the hits,
and announced `credentials unset in this session: ANTHROPIC_API_KEY`.
Nothing was ever unset.

It now hands over the exact command instead:

```
nini cannot unset credentials in your shell (1 found).
Run this in the shell that launched nini, then restart nini:

  unset ANTHROPIC_API_KEY
```

`std::env::remove_var` would have been the one-liner, and is the wrong
answer: the TUI is a multi-threaded tokio runtime, and mutating the
environment while another thread reads it is undefined behaviour. This
crate is edition 2021, so the compiler will *not* warn you — on
edition 2024 the same code stops compiling. A comment now says so.

A real bug fell out of this too: `openai`, `openai-responses` and
`openai-compat` all map to `OPENAI_API_KEY`, so a bare `/logout`
listed it three times. Deduplicated, with a regression test.

---

## Known gaps carried into v0.9.2 (unchanged)

Phase 0 of the plan (`docs/plans/v0.9.2-honesty-and-deadcode-sweep.md`)
audited every user-visible promise in the codebase against its
implementation. The following are **still broken** and are the next
targets:

| # | Gap | State |
|---|---|---|
| P0-2 | `ProjectTrustStore::load`/`save`/`get`/`clear` are all no-ops and `check()` has **zero call sites** — project trust is entirely unenforced. Worse, `default_path()` returns a CWD-relative `.pi/agent/trust.json` while the selector writes `~/.pi/agent/trust.json`. | ✅ v0.10：store 自 v0.9.2 起已真实实现；本轮把 `check()` 接进 `BeforeExecute` 执行闸门（`nini-core/src/trust_gate.rs`） |
| P0-3 | 16 further CLI flags are dropped in `main.rs` and advertised in `--help`. 6 others are warned about; 13 work. | ⬜ |
| P1-1 | `/debug` toggles a flag with no consumers; `tracing` is declared in four crates and initialised nowhere. There is no logging. | ⬜ |
| P1-2 | Theme hot-reload does not exist. `notify` is a declared, unused dependency. | ⬜ |
| P1-3 | `/tree` is broken in three layers, and would still render a straight line once two of them were fixed, because `/fork` creates no branches. | ⬜ |
| P1-4 | `/reload` counts skills and discards the result. | ⬜ |
| P1-7 | `set_status_bar_usage` is dead, so `context_window` is always 0 — the status-bar context bar and `/status`'s context line never render. The test at `status_lines.rs:73` sets the field by hand, which is why it is green. | ✅ v0.10：`Usage` 事件（含 cache 计数）经 setter 写入 context bar |
| — | `/share` returns "not yet implemented". | ⬜ |

Roughly 1,400 lines of unreferenced code remain (`nini-ext`,
`branch_summary.rs`, `operations.rs`, 25 dead `pub fn`, 7 dead
dependencies). None is deleted yet: v0.8.4 and v0.9.0 both deleted
something that turned out to be a regression and had to restore it.

## v0.9.1 — Two panics that would have ended your session (2026-10-05)

The last two items on the v0.8.9 plan. Both are about **what happens
after something goes wrong**, which is precisely what the previous
release never tested.

Tests: **929 → 937**.

---

### Fix — one panic used to kill the TUI permanently (P2-8)

The runtime had 29 `shared.lock().unwrap()` calls. A `Mutex` is poisoned
when a thread panics while holding it, and every later `lock()` on a
poisoned mutex returns `Err`. So `unwrap()` converts **one** panic into
a permanent condition: the next frame's first `lock()` panics again, and
again, for the rest of the process. The user is left with a frozen or
blank terminal and no way out short of killing nini.

That is a much worse outcome than the panic that started it — and it was
not hypothetical for this codebase. v0.8.9 and v0.9.0 shipped fixes for
**three panics reachable from ordinary input**: an integer overflow in
`read`, a UTF-8 boundary panic in bash output truncation, and a
concurrent-agent indexing panic. Any of them would have ended the
session, and the only symptom would have been "nini froze".

A poisoned lock means "some state may be inconsistent", not "the state
is unreadable". The TUI re-derives nearly everything it draws from
`AppState`'s own fields; the failure mode of proceeding is a transient
glitch, while the failure mode of panicking is a dead application. So
`lock_state()` recovers the guard and carries on.

Applied to all 29 sites in `runtime.rs` plus the global event bus — a
poisoned *global* is worse still, because it outlives every session and
would make the next run of nini panic on its first event. Production
code is now free of `lock().unwrap()`; the remaining occurrences are all
inside `#[cfg(test)]` blocks.

### Fix — small terminals no longer render nonsense (P2-12)

The chrome was a fixed 6 rows (2 footer + 3 prompt + 1 hint) before the
transcript got anything. Below that, ratatui squeezed the overflow into
whatever was left. A 20x5 terminal rendered:

```text
> hello idle       m          ← the user's message and the status bar, same line
┌ ─ input ─ ───────┐
 Enter  send  …     ← and the input row is gone
```

Nothing panicked — ratatui clipped — but it no longer read as an
application, and "make your terminal bigger" is not advice you can give
someone in a 60-column split pane or an SSH window on a phone.

Degradation is now explicit and ordered: drop the hint bar, then the
footer's second row, then the prompt's border. The transcript and the
prompt are last to go, because they are the two things you cannot work
without.

Two details the tests caught while building it:

- `render_footer` had a `height < 2` fallback that *invented* a row
  (`tmp.height = 1`) and drew the stats line on top of the transcript —
  that is where `> hello idle m` came from. Zero rows now means zero rows.
- `Borders::ALL` on the prompt costs two rows, so at the 2-row height the
  layout hands us on a short terminal, the frame consumed the whole
  widget and the input line was pushed out. Below 3 rows the border is
  dropped and the text drawn bare: an unframed prompt you can type into
  beats a handsome empty box.

---

### Tests +8

`crates/nini-tui/tests/lock_poison_e2e.rs` (5): the guard is usable after
poisoning, the render path survives three consecutive frames, a
simulated runtime tick survives, the global bus survives, and state
written *before* the panic is still intact afterwards.

Verified red-then-green: restoring `lock().unwrap()` reproduces
`called \`Result::unwrap()\` on an \`Err\` value: PoisonError { .. }` and
fails 4 of the 5.

`transcript_width_e2e.rs` (3): degenerate sizes (10x4, 5x3, 20x5, 40x3,
8x2, 3x1) never panic or overflow; the prompt survives at usable widths;
the transcript keeps at least one row. Verified red-then-green too.

## v0.9.0 — Honour `--api-key`, and make the default model real (2026-10-05)

A v0.8.9 post-release acceptance pass, released as **0.9.0** rather than a patch: the default model changes, which is a behaviour change. Cargo also rejects a four-part semver, so `0.8.9.1` was never on
the table. Two things it turned up that the green suite could not: a
documented flag that did nothing, and a default model that was a placeholder.

Tests: **920 → 929**.

---

### Fix — `--api-key` was silently ignored

`main.rs` destructured the flag as `api_key: _` and dropped it.
`provider_factory` read `OPENAI_API_KEY` straight from the environment
and never heard about the flag, so `--help`'s *"API key (overrides env
vars)"* was false in both directions:

```console
$ nini --provider openai-compat --api-key deliberately-wrong ...   # env correct
  → succeeded                                                        # flag ignored

$ nini --provider openai-compat --api-key <correct> ...             # env unset
  → Error: OPENAI_API_KEY required for openai-compat                # flag ignored
```

`--api-key` now overrides the environment variable, and
`resolve_api_key()` distinguishes three cases with three different
messages: explicit key, environment key, and neither.

### Fix — an empty API key failed silently

`std::env::var` returns `Ok("")` for a variable that is set but blank, so
`--api-key ""` (or an exported-but-empty `OPENAI_API_KEY`) sailed past the
old `.context(...)` check and built a provider that sent
`Authorization: Bearer ` — a **401 with no diagnostic anywhere in the
TUI**, just two dividers and silence. An empty key is a configuration
mistake and now says so:

```console
$ nini --provider openai-compat --api-key "" ...
  Error: --api-key was passed but is empty. Pass a real key, or unset it
         and set OPENAI_API_KEY instead.

$ OPENAI_API_KEY="   " nini --provider openai-compat ...
  Error: OPENAI_API_KEY is set but empty. Unset it, or pass --api-key <key>.
```

The same run showed that an *invalid* key was reported properly
(`[error] provider error: api error (401)`), so the two failure shapes
were inconsistent — same root cause, opposite symptoms.

### Fix — session flags are now refused out loud, not ignored

`--continue-session`, `--session-id`, `--no-session`, `--name`, `--fork`
and `--session-dir` were also `_: _`. TUI session persistence is a
known, README-declared gap, so they cannot be implemented here — but
silently accepting a flag is worse than refusing it. They now print
what is being ignored and why.

### Change — the default model is a real one

`--model` defaulted to `test-model`, a placeholder that only means
anything to the `fixture` provider. `nini --provider openai-compat`
without `--model` therefore sent a model id no server has heard of.

The default is now **`MiniMax-M3.1-Flash-Preview`** — real, fast and
cheap, which is what you want as a default because an agent's first
token matters more than its fifth. Override with `--model` or `/model`.

### Fix — `delta.reasoning_content` was dropped

Switching the default exposed a latent parsing gap. MiniMax's reasoning
models disagree about where the thinking goes, *intermittently on the
same model and prompt*: `MiniMax-M3` inlines it in `delta.content`
inside `<think>` tags, while `MiniMax-M3.1-Flash-Preview` sends it as
`delta.reasoning_content`. serde ignores unknown fields, so the
reasoning parsed successfully and was then thrown away — the user saw an
answer with no thinking block, the same failure shape as the v0.8.9 P0-4
text loss one layer down.

`OpenAiDelta` now carries the field and routes it to `ThinkingDelta`.
Both kinds of text go into the same queue in arrival order, and neither
returns early — the v0.8.4 lesson is that returning on one kind of text
drops the other.

### Fix — 529 is retryable

The first run on the new default model failed with
`api error (529)`. That is MiniMax's gateway reporting a saturated
model, and it is exactly as transient as a 503 — but `is_retryable_error`
did not list it, so the run failed outright when a retry would have
succeeded. It is now covered.

---

### Verified end to end

Against `https://minnimax.chat/v1` with the new default model, in the
real TUI:

```text
 nini   idle                                     (openai-compat) MiniMax-M3.1-Flash-Preview
> 用一句话说明 Rust 的所有权
────────────────────────────────────────────────────────────
  💭 The user asks in Chinese: "用一句话说明 Rust 的所有权" …
  💭
  💭 Just answer concisely in Chinese, one sentence.
Rust 的所有权是一套编译期规则：每个值有且仅有一个所有者，所有者离开
作用域时值被释放，而赋值或传参会…
```

## v0.8.9 — What the transcript was hiding (2026-10-05)

A release driven by a single question: *why does the UI feel heavy after a
big task?* The answer turned out to be four separate P0s, three of which
only a real end-to-end run could find — 920 tests were green while the
agent could not say a word, could not pass a tool argument, and could not
kill a process.

Tests: **812 → 920** (+108, every one a regression guard with a verified
red-then-green cycle).

---

### P0 — assistant text was 100% dropped on the OpenAI path

`nini --provider openai-compat --model MiniMax-M3 -p "..."` printed
**nothing**: no output, exit code 0, no error message anywhere. `curl`
against the same endpoint returned a perfectly good answer.

v0.8.4 had fixed a real bug — a chunk carrying both a `</think>` close and
a tool-call start lost the tool call, because the text handler did
`return Some(fragment)` and skipped the `tool_calls` loop — by stashing the
fragments in `State::fragments_pending` instead. **The drain that stash was
waiting for was never written.** The field had one writer and no reader,
and `TextDelta` / `ThinkingDelta` were never constructed anywhere in
`openai.rs`.

```text
$ grep -n fragments_pending crates/nini-ai/src/openai.rs
144:    fragments_pending: Vec<ThinkFragment>,            # declaration
364:                        state.fragments_pending = frags;   # only writer
```

Every test either used the `fixture` provider (which never touches SSE
parsing) or hand-built `SseEvent` values and asserted on **tool-call**
events — because tool calls were what v0.8.4 fixed. Nobody asserted that
`TextDelta` comes out of a real wire payload.

The fix drains one fragment per `translate_sse` call, *after* the
tool_calls handling (v0.8.4's ordering must survive), plus a `finalize`
flush so the last sentence of an answer isn't lost, plus
`ThinkTagFilter::flush()` for streams that end mid-`<think>`.

**Also found by the new wire tests, in the same file:**

- **Duplicate `ToolCallStop`.** The single-shot inline-args path emitted a
  Stop and left the entry in `tool_calls`, so `finalize` emitted a second
  one for the same id — two transcript lines, `tool_results` written twice.
- **Usage was permanently zero.** `finalize(&mut state, None)` hard-coded
  `None` and discarded the parsed `chunk.usage`, so the status bar's token
  and cost counters read 0 for every OpenAI-compatible provider no matter
  what the model actually spent.

### P0 — interleaved parallel tool calls lost their arguments

Driving the real TUI with *"read these four files in parallel"*:

```text
▸ read {"path":"target1.rs"}
▸ read {"path":"target2.rs"}
▸ read {"path":"target3.rs"}
▸ read {"path":"target4.rs"}
✗ read Took 0ms
  invalid arguments: invalid type: null, expected struct ReadArgs
```

The agent accumulated tool calls in a **single `current_call` slot**: every
`ToolCallStart` pushed the previous call out with its `input_json` still
empty, and every `ToolCallStop` discarded whatever did not match. That is
correct for a strictly serial `Start→Stop→Start→Stop` sequence — which is
exactly what `fixture` emits, and therefore exactly what every test in the
repo exercised. Real models interleave, and the result was most tool calls
reaching the tool layer as `Value::Null`.

The transcript still looked perfect, because v0.8.8 made the TUI sink
track each line by id independently — so the arguments rendered
beautifully while the tools received nothing. Two bugs hiding each other.

Now accumulated in a `HashMap` keyed by id, with stragglers flushed (and
logged) if a Stop never arrives.

### P0 — the bash tool never actually killed the process tree

`kill_process_tree` passed `child.id()` to `killpg()` — but `BashTool`
never called `setsid(2)`, so the child was not a process-group leader and
the group it named did not exist:

```text
child pid=550789
child's real pgid (/proc/550789/stat field 5) = 550786   # the parent's group
killpg(child_pid=550789) target group exists? false      # ESRCH, swallowed
```

The tool description promises *"on timeout the process tree is killed"*.
On the path the LLM actually uses, that was false. And because the
process-group kill was dead, every timeout also sat out the full grace
period: a `timeout: 1` call really cost **6177 ms** (1 s + 5000 ms grace,
after which the child was finally reaped by `kill_on_drop`).

The root cause was two divergent bash implementations — `bash_runner.rs`
(hook path) had the `pre_exec(setsid)`, `bash.rs` (tool path) had a comment
claiming it did and no code to match, with zero test overlap. Both now go
through one `procgroup` module, and `KILL_GRACE_MS` is 500 instead of 5000.

### P0 — a CJK bash result panicked the whole agent

`truncate()` cut the byte budget with a raw `&kept[..half]`:

```text
panicked at src/bash.rs: end byte index 524288 is not a char
boundary; it is inside '文' (bytes 524286..524289 of string)
```

This is the same bug class v0.8.4 fixed twice (`render_tool_call`,
`render_bash_execution`) — `truncate` was missed both times. It lives on
the tool-execution path, so a >1 MiB Chinese bash result unwound the entire
agent. 812 tests were green because every fixture was ASCII.

### P0 — `read` overflowed, or silently lied

`limit` defaulted to `usize::MAX`, so `offset + limit` overflowed. Debug
builds (what `cargo test` runs) panicked with `attempt to add with
overflow`; release wrapped to `offset - 1` and the tool **returned an empty
string**, so the model concluded the file had no content past the offset.
A model passing `offset` without `limit` is completely ordinary, and it
could take the agent down.

### P0 — bash output was buffered in full before truncation

Both readers were unbounded `read_to_end`, so the 1 MiB budget was a
post-hoc cosmetic number. `yes | head -c 64MB` reported
`original_bytes=67108864` and `original_lines=6100806` while returning
22 KB — the peak held the 64 MB, a second 64 MB `from_utf8_lossy` copy,
and ~6.1 M fat pointers for the `lines().collect()` inside `truncate`.

Now capped *while streaming* (draining past the cap so the child never
blocks on a full pipe) with honest totals, and `truncate` is a single pass
on character boundaries. `read` also got a real ceiling, checked against
metadata *before* loading.

---

### P1 — the transcript lied about what it cut, then got 200× faster

**Rendering (`P1-3` … `P1-6`).** Only `tool_result` had a width budget.
Everything else was handed to ratatui's `List`, which does **not** wrap —
it cuts at the screen edge with no marker. A 210-character user message
rendered as `> STARTAAAA…` with the tail simply gone; `render_tool_call`'s
flat 120-**byte** budget put its `…` at column 120, i.e. *off-screen*, so
the truncation marker was itself truncated away. The unit test "covering"
it asserted `joined.contains('…')` on the returned string, so it stayed
green while the screen showed a bare hard cut.

Every renderer now takes a cell budget; `truncate_cells` /
`truncate_cells_head` are the shared primitives (the latter for paths and
model ids, where the tail is the informative end). The truncation
indicator now reports what was *actually* dropped, in the unit it was
dropped in — it was subtracting a 2 KB budget that v0.8.7 had already
stopped applying, so it said nothing when 1460 cells were hidden and said
"2 KB" when 4.7 KB was. The footer no longer overflows (it measured with
`chars().count()`, and appended the right half past the terminal edge when
the halves did not fit — pushing the model name, of all things, off
screen). Shrinking the transcript (compaction, `/clear`, `/new`, session
reload) resets the scroll viewport, which previously left a completely
blank transcript with a meaningless `↓ 59 more`.

**Performance (`P1-7`).** *"After finishing a complex task, typing in the
prompt felt sluggish and the TUI felt like it was holding a lot of
resources."* The renderer needed each line's row count and got it by
rendering **every line, fully, on every frame** — then rendering the
visible ones a second time.

```text
                       before      after    speedup
   16 lines /   8 KB    5.94 ms    1.67 ms      3.6x
  307 lines / 164 KB   90.65 ms    1.87 ms     48x
 1227 lines / 661 KB  361.53 ms    1.82 ms    199x
```

A linear **0.545 ms/KB** with no intercept; a 16 ms budget is 60 fps, so
~29 KB saturated a frame. The cost was *independent of what was on screen*:
34 visible rows, 1227 lines rendered, 1193 discarded. Now cached on
`(width, content fingerprint)`, which is self-invalidating — a streamed
`AssistantText` grows, its length changes, only that line is re-counted.

---

### P2 — durability, cancellation, resource bounds

- **Session JSONL** (`P2-9`): one malformed line used to make the *entire*
  session unreadable — a hand-edit, a disk that filled mid-append, a
  truncated copy, a field from a newer nini. Now salvaged line by line,
  with the damage reported in the transcript rather than only in a log
  (a silent tolerance is its own kind of lie).
- **Abort** (`P2-11`): the abort path returned without emitting
  `ToolResult`, so every in-flight tool call stayed pending in the
  transcript for the rest of the session. The test for it asserts the
  **invariant** (every `ToolCallStart` id appears as a `ToolResult` id) —
  the pre-existing test only checked that the task ended, which is why it
  never noticed.
- **Parallel tool execution** (`P2-10`): batch size was whatever the model
  emitted. 30 `bash` calls meant 30 process trees at once, multiplying
  against the memory ceiling above. Now a worker pool bounded by
  `RunConfig::max_parallel_tools` (default 8; `0` = unlimited).
- **CI** (`P3-14`): the clippy job's `-D` flags were cancelled by `-A`
  flags for the same lints later in the same command — it had been
  allowing everything. The fmt job had been red since the repo was
  created. Both now gate for real.

---

### Testing discipline (v0.8.9)

Four bugs in this release could not be caught by the tests that existed,
and the pattern is consistent: **they all lived on code paths the
`fixture` provider never takes.** Fixture turns are serial, well-formed,
non-truncating, and never interleave.

So v0.8.9 adds two layers that had been missing entirely:

- `crates/nini-ai/tests/openai_wire_e2e.rs` — 14 tests through the real
  `stream()`: real HTTP (wiremock), real SSE bytes, real `translate_sse`,
  real `finalize`.
- `crates/nini-tui/tests/transcript_width_e2e.rs` (14) and
  `transcript_perf_e2e.rs` (8) — `TestBackend` cell-level assertions, not
  string-level ones.

And a rule, learned the hard way three times this release: **assert the
property that broke, not an adjacent symptom.** Removing the text drain
still produced every character in the right order, just at the wrong
*moment*; counting events couldn't tell it apart, so it took an ordering
assertion. Performance needed a *ratio*, not an absolute millisecond
count, or it would go flaky on slow CI.

---

## v0.8.8 — Tool identity preservation (2026-10-03)

Driving real LLM tasks after v0.8.7 revealed 3 additional bugs where
tool call identity was lost between the agent and the TUI:

**Bug fix — parallel tool calls keep distinct args**

`AgentSink::push(ToolCallStop)` previously updated the LAST
`TranscriptLine::ToolCall` unconditionally. For N parallel tool calls
in one assistant turn, each Stop event overwrote the LAST line — so
only the final call's args survived in the transcript and earlier
calls' args were silently lost. Fix: `TranscriptLine::ToolCall` and
`TranscriptLine::ToolResult` gain an `id` field; the sink looks up the
target line by id (reverse-walked from the end) and updates in place.
Falls back to pushing a new line if no matching id is found (handles
out-of-order events from buggy providers).

**Bug fix — tool results display tool name**

`render_tool_result` previously emitted just `✓ ` or `✗ ` followed by
a duration pill — no tool name. For N parallel tool results the user
couldn't tell which `✓ Took Nms` block belonged to which call. Fix:
`AgentEvent::ToolResult` carries the tool name end-to-end (agent →
cli driver → sink → `TranscriptLine::ToolResult.name` →
`render_tool_result`). Renders as `✓ bash (4ms)` / `✗ read (0ms)`.

**Bug fix — `--model` flag now propagates to the status bar**

`crates/nini-tui/src/runtime.rs:214` hardcoded `AppState::new("test-model")`,
silently dropping the user's `--model MiniMax-M3` (or any other) flag
at the TUI layer. The CLI-side `cfg.model` was already plumbed to
`RunConfig.model` and used for the actual LLM call, but the status
bar always displayed `test-model`. Fix: `run()` takes a new
`initial_model: &str` parameter; `cli::app::run_tui_bootstrap` passes
`&cfg.model`. Closes a sister issue to v0.7.2 ux-report #7 (model
selector not synced).

**Breaking — `TranscriptLine::ToolCall/ToolResult` add `id`/`name`**
fields

Session JSONL v4 codec handles arbitrary enum fields; old session
files reload with the new fields populated as `""`. In-memory
transcripts round-trip cleanly through `Arc::make_mut` clones.

**Tests**: 836 → 842 (+6 e2e regression in
`crates/nini-tui/tests/tool_identity_e2e.rs`):
  * `parallel_tool_calls_keep_distinct_args` — 3 parallel reads, each
    preserves its own path
  * `tool_result_renders_tool_name` — `✓ bash` appears in result header
  * `failing_tool_result_renders_tool_name` — `✗ read` on errors
  * `parallel_tool_results_keep_distinct_names` — bash/read/grep names
    distinct in mixed parallel runs
  * `tool_call_stop_unknown_id_falls_back` — out-of-order Stop
    without matching Start doesn't panic
  * `initial_model_arg_propagates_to_status_bar` — status bar shows
    `--model` argument

All existing tests pass unchanged. `cargo clippy` clean.

---

## v0.8.7 — TUI transcript continuity + Pi collapse parity (2026-10-03)

**Bug fix — transcript overflow no longer clips tool-call headers**

When the model emits parallel tool calls with substantial bodies
(typical: 8× `read` of large files), the previous TUI showed
tool-result body fragments without their `▸ <name>{…}` headers — the
user could no longer tell which tool produced which body fragment
("窗口工具调用输出不连续,会断开"). Root cause: `render_transcript` used
`lines.len()` (TranscriptLine count) for autoscroll math, but each
TranscriptLine expands to 1..N ListItems at render time. When items
> visible_height (~18 at 80x24), ratatui's List widget clipped the
bottom — but the latest tool-call *header* sat right above its result
body, so the header scrolled off the top while the body stayed
visible. Fix:
  * Render all items into a `Vec<ListItem>`, then truncate to
    `visible_height` keeping the tail (autoscroll) or the user-scroll
    window. Truncation is in **item space**, not line space, so it
    respects the actual rendered row count.
  * Per-line item counts (`items_for_line` helper) drive both the
    truncation decision and the new overflow indicator count.
  * User-scroll path translates `scroll_offset` (TranscriptLine
    count) to an item-space offset, keeping the existing keybindings
    (`PageUp`/`PageDown`/`j`/`k`) but with item-space-respected view.

**Enhancement — `↑ N more above` indicator in autoscroll mode**

When autoscroll has hidden items above (the common case during /
after a burst of parallel tool calls), show `↑ N more above` at the
top of the transcript. Mirrors the existing user-scroll `↓ N more`
indicator at the bottom-right — same coverage of the corner cell.
Tells the user they can scroll up to see hidden content.

**Cleanup — single combined truncation indicator for tool results**

Old behavior emitted TWO consecutive truncation rows when a tool
result exceeded both the per-line (200 bytes) and total (2 KB)
budgets: `…(N more lines)` then `…(truncated, N more bytes)`. New
behavior emits ONE row: `…(N more lines, M KB hidden)`. Single
visual cue, less noise.

**Bug fix — `…` truncation indicator stays visible**

`render_tool_result` now accepts `max_width: Option<usize>` and the
TUI threads `area.width` through. Per-line budget is
`max_width - 2 - 1` cells (subtracting the 2-space indent and 1 cell
for the `…` indicator itself). Truncation uses display-cell width
(via `unicode-width`) instead of byte length, so CJK content (2
cells per char) packs the visible area without wasting width.

**Tests**: 829 → 836 (+7 e2e regression tests in
`crates/nini-tui/tests/transcript_overflow_e2e.rs`):
  * 8 parallel reads don't bottom-clip the latest tool call header
  * `↑ N more above` appears when overflow
  * single-tool control flow shows no indicator
  * 200x50 keeps call→result adjacency + body rows
  * CJK args don't panic and respect visible_height
  * long single-line result shows visible `…` indicator
  * single combined truncation row replaces the old double row

All existing tests pass unchanged. `cargo clippy` clean.

---

## v0.8.6 — Wire up tool self-descriptions + length-stop safety (2026-09-30)

**Bug fix — system prompt now exposes tool snippets + guidelines**

Every tool (`bash`/`read`/`edit`/`find`/`grep`) has implemented
`system_prompt_contribution()` since v0.7.1, and `nini-core` has had
`build_system_prompt_with_contributions()` since then too. None of
this was ever wired up. The CLI was using a hardcoded
"Available tools: bash, read, write, edit, grep, find" string with
no snippets or guidelines — the model had no idea each tool's
distinguishing characteristics (e.g. "bash is default for any shell
operation" vs "read is for whole files", "edit is for targeted
patches", "find is for glob enumeration"). This release wires it up.

`write` was missing its `system_prompt_contribution()` (added now).
Output is now sorted alphabetically by tool name to fix the
HashMap-random-iteration flakiness in tests + LLM cache hit rate.

New system prompt section (sorted, alphabetical):

    ## Tool self-descriptions
    - bash: Run shell commands (ls, grep, cat, cargo test, ...).
    - edit: Apply a targeted text replacement to a file.
    - find: Enumerate files matching a glob (e.g. `*.rs`).
    - grep: Regex search file CONTENTS (not file names).
    - read: Read a file's contents into your context.
    - write: Write a file atomically (temp file + rename).

    ## Tool usage guidelines
    - bash: Default for any shell operation; pick this unless ...
    - bash: Set `timeout` for anything that could run unboundedly.
    - bash: Confirm with the user before destructive commands ...
    - edit: `old_text` must match EXACTLY ONCE — include enough ...
    - read: Always read a file before editing it — never edit blind.
    - write: For whole-file rewrites use write, not edit.

**Bug fix — reject tool_calls on length stop**

When the LLM hits its output_tokens limit, the assistant message
may end mid-tool-call-JSON. The current code falls through to the
tool execution loop with truncated args (`serde_json::from_str`
returns `Value::Null`, the tool runs with empty args, the user sees
a cryptic "tool error: invalid args"). This release matches Pi's
`failToolCallsFromTruncatedMessage`: when stop_reason == "length"
AND tool_calls is non-empty, emit synthetic `[reject] tool call
truncated by output token limit` ToolResults, push them to messages,
and let the next LLM turn recover.

The existing auto-compaction-on-length-stop path is preserved
(shrinks context before next turn); only the tool execution is
skipped.

**Enhancement — skill prompt includes `<location>` for model loading**

Previously, `format_skills_for_prompt` emitted markdown bullets
`- name: description` with no file path. The model knew a skill
existed but had no way to load it via `read`. Now uses Pi-style
`<available_skills>` XML with `<location>` (the path to SKILL.md).
Models can now actually load skill content.

**Tests**: 822 → 826 (+4 new regression tests across 3 PRs).
All existing tests pass unchanged. `cargo clippy` clean.

---

## v0.8.5 — TUI hang on parallel reads + AppState clone perf (2026-09-30)

**Bug fix — TUI unresponsive during / after parallel tool execution**

Reproducer: ask the agent to read 14 docs in one turn. The TUI would
appear frozen: status bar stuck on the last frame, key presses
ignored, no spinner animation. Root cause was a silent no-op in the
v0.7.4 wake-on-event mechanism (Notify wired to the wrong handle)
combined with an O(transcript-size) `AppState::clone()` on every
runtime loop iteration. Both fixed.

  * **Watch-channel wake-up (`runtime.rs`)** — replaced v0.7.4's
    per-iteration `Arc<Notify>` (which `submit_user_input` created
    independently and never connected to the runtime loop's
    `select!`) with a single shared `tokio::sync::watch::Sender<bool>`
    owned by the loop. Every agent task spawned during one session
    sends on the same channel; the loop drains coalesced changes in
    `<5ms` (well under the 50ms tick). Regression test
    `sink_push_wakes_watcher_within_5ms` pins the timing contract.
  * **Single lock per `AgentSink::push`** — was two locks
    (`lock → release → lock → release`) which opened a race window
    where concurrent pushes could interleave `PhaseChanged` and the
    body of an event, causing `status` to lag the transcript.
  * **`AppState::clone()` is O(1)** — wrapped
    `transcript_state: TranscriptState` in `Arc<TranscriptState>`.
    With 14 tool results each carrying ~25 KB of file content, the
    old code did ~350 KB of memcpy per render-frame (~3.5 s/s at
    20 FPS). Now it's an Arc bump. Mutation sites use
    `Arc::make_mut(&mut self.transcript_state)` which, since the
    outer `Mutex` gives unique access, never triggers the CoW
    branch. Regression test
    `appstate_clone_is_cheap_regardless_of_transcript_size` asserts
    100 clones of a 350 KB transcript take <50 ms.

- **Parallel tool execution** — replaced the serial
  `for tc in tool_calls` loop in `agent.rs` with a
  `FuturesUnordered<...>` pool. The model often emits N parallel
  tool_calls in one turn (e.g. 14 reads); before this change, nini
  ran them serially even though the provider API treats them as
  independent. Now they execute concurrently — 8 × 100ms sleeps
  finish in ~100ms (was ~800ms).
  - **Slot-ordering invariant preserved**: tool_results in the
    message sent to the next LLM call are in the SAME order as
    tool_calls in the preceding assistant message. Anthropic's API
    requires this; breaking it 400-errors the next turn. Verified
    by `parallel_tool_results_in_slot_order_for_llm_message`.
  - **Abort semantics preserved**: firing `abort.abort()` while
    futures are mid-flight cancels the whole agent run (returns
    `Aborted` event). We never emit partial tool_results — that
    would break the API contract. Verified by
    `abort_during_parallel_tool_returns_aborted_event`.
  - **Per-future abort check**: each spawned future checks abort at
    its first await point so a pre-aborted batch doesn't begin
    running tool bodies at all.

**Out of scope (next iterations)**
- Truncating `ToolResult.content` at push time (would also need
  `/export` HTML to either accept truncated output or read from a
  separate `raw_content` field)
- Capping `transcript_state.lines` length
- Replacing `auto_link` with a regex-based scan (10× faster on
  large tool results)

**Tests**: 816 → 824 (+5 new regression tests in `agent_integration`
+ `agent_wire_e2e`). All other tests pass unchanged. `cargo clippy`
clean across workspace.

---

## v0.8.4 — Pi TUI parity + AI streaming protocol fixes (2026-09-28)

**AI streaming protocol — correctness**

The single-shot and streaming code paths through `nini-ai` had drifted
apart, dropping tool_call args on the first chunk, swallowing agent
errors, and emitting `MessageStop` inconsistently. All paths now share
one canonical lifecycle:

  * `9c9783e` single-shot + streaming tool_call share one `Start`, drain
    only when complete (no partial-args leakage)
  * `7a71f5b` `aiio` inline-args tool_call now emits both `Start` + `Stop`
  * `c3944d1` tool_call args dropped on first chunk + agent errors
    swallowed — both fixed
  * `8b4ff90` openai `stream()` must emit `MessageStop` at end of stream
    (closes loop with TUI renderer)

**TUI multi-byte UTF-8 safety — zero panics**

Five regressions were panicking on CJK / emoji / accented input by
byte-slicing at non-char-boundary indices. All replaced with
`UnicodeWidthStr` for display width and `chars().take()` / proper
`char_indices()` slicing:

  * `7f7f24c` eliminate every remaining byte-slicing panic in `tui` + `core`
  * `3a31fe9` `prompt_template` description no longer panics on multi-byte
  * `30db289` `render_tool_call` no longer panics on multi-byte args
  * `0f8232d` `UnicodeWidthStr` for emoji and CJK display width

**TUI UX (Pi parity polish)**

  * `dda5f51` v0.8.4 UX improvements — Pi-parity refinements
  * `8744ceb` complete wiring: `StopReason` + welcome card + narrow hints
  * `ffb3d96` transcript autoscroll
  * `82df71c` transcript scroll indicator (fixes "ommand." truncation on
    long thinking — ux-001)
  * `0233372` restore `/hotkeys` command output (v0.8 regression)
  * `0afb740` update e2e tests for v0.8.4 TUI changes

**Provider configuration**

  * `24950d4` honor `{PROVIDER}_BASE_URL` env var for openai / deepseek /
    groq / mistral / cohere (previously only Anthropic picked up the
    override)

**Tests**

  * `ee105b2` regression coverage for v0.8.4 thinking + tool-call +
    provider-error paths
  * Total: **816 passing** (v0.8.3: 760, +56)

---

## v0.8.3 — commands.rs拆分计划 (2026-09-25)

`crates/nini-tui/src/commands.rs` (1833 LOC, 含1个1200 LOC的`dispatch()`
函数处理28个slash命令) 拆为7个子模块:

  * **mod.rs** (25 LOC) — pub use facade (保留外部API)
  * **registry.rs** (351 LOC) — CommandId + REGISTRY (28 entries) + by_name + complete
  * **result.rs** (98 LOC) — CommandOutcome + CommandResult
  * **parse.rs** (64 LOC) — parse() input → (CommandId, String)
  * **status_lines.rs** (71 LOC) — build_status_lines() (for /status)
  * **html_export.rs** (110 LOC) — render_transcript_html() (for /export)
  * **dispatch.rs** (1237 LOC) — dispatch() + entry_legacy_message

外部API不变 — lib.rs的`pub use commands::{...}` 通过新mod.rs facade 重导出。

**附带修复**:

  * 为 CommandOutcome + CommandResult 添加 `PartialEq` (之前缺失, 阻塞了下游测试)
  * `html_escape()` 现在用于 User / Assistant / ToolCall 文本
    (之前是 /export 的轻度 XSS 隐患)
  * REGISTRY 顺序: ScopedModels 移到 Status 之前 (恢复 v0.8 之前的顺序, slash_command_e2e 测试依赖此顺序)

测试: **739 → 760** (+21 净增).
  * registry: 8 tests
  * result: 4 tests
  * parse: 6 tests
  * status_lines: 3 tests
  * html_export: 3 tests

**v0.8.4 候选**: dispatch.rs 拆 28 match arm 为 cmd_xxx fn (per-command 单测).

## v0.8.2 — main.rs拆分计划 (2026-09-25)

main.rs (1230 LOC的"上帝函数")按职责拆分为6个子模块:

  * **prompt_setup.rs** (99 LOC) — model_for_cfg + system prompt
  * **provider_factory.rs** (276 LOC) — build_provider + parse_scripted_turns
  * **tool_registry.rs** (133 LOC) — build_tools + filter_tools
  * **info.rs** (50 LOC) — `nini info` 子命令
  * **demo.rs** (279 LOC) — run_demo + run_print + fixtures
  * **app.rs** (220 LOC) — AppConfig + run_tui + AgentDriver闭包

main() body: **314 → 87 LOC** (72% reduction).

**新增单元测试** (10个):

  * prompt_setup: model_for_cfg 3 paths + system_prompt 内容 (6 tests)
  * provider_factory: parse_scripted_turns 5 个 variants (6 tests)
  * tool_registry: filter_tools 6 个组合 (6 tests)
  * demo: find_first_file_with_todo (2 tests)
  * info: smoke test (1 test)

main.rs 从0测试覆盖 → 现在每个sub-module可独立单元测试。

**Fix**: v0.8.2附带修复demo_fix_todos_turns()fixture bug——
原single-turn fixture导致agent循环50次 (ProgrammedProvider
循环队列)。改为2-turn fixture后正常退出。

总测试: **723 → 739** (+16).
外部API不变: `nini -p`、`nini demo`、`nini info`、`nini` (TUI)
行为完全相同。
## v0.8.1 — AppState split into 5 sub-structs (2026-09-25)

Internal refactor. **No external API change** — all method calls
(`state.push_user`, etc.) work unchanged. Only field-access
syntax changed (e.g., `state.tokens.input` →
`state.run_state.tokens.input`).

**Why**: AppState grew to 38 pub fields, 70 pub methods in
v0.8.0-pre1. Adding any new field required touching 6+ files.
Splitting by concern reduces future surface area.

**New layout** (6 fields, down from 38):
  * `state.input`               — InputBuffer (edit buffer)
  * `state.transcript_state`    — TranscriptState (lines + scroll)
  * `state.run_state`           — RunState (mode + stats + abort)
  * `state.model_state`         — ModelState (LLM config)
  * `state.session_state`       — SessionState (persistence + cwd)
  * `state.ui_state`            — UiState (overlays + theme + debug)

**Migration**: ~315 field-access sites across 11 files.
Pure sed pipeline was too brittle (chained multi-line patterns,
multiple local names, cascading substitutions, same field names
on different types). Final implementation: ~10 sed rounds +
targeted Python scripts.

**Tests**: 723 pass / 0 fail (was 714). Added 9 sub-struct unit
tests proving each sub-struct can be tested independently.

**Out of scope** (next iterations):
  * Lock-free reads (would require changing SharedState type)
  * Splitting main.rs (separate refactor)
  * Splitting commands.rs (lower priority)

## v0.8.0-pre1 — Pi-parity TUI improvements (2026-09-25)

**Tool selection (the user-reported bug)**

The model used to pick `find` for "list files" prompts because
nini's `find` description read "Find files by glob pattern" —
matching the word "find" too well. Fixed by:

  * `find` description now says **"NOT for listing a directory's
    contents — use bash (`ls`) for that."** (Pi-style cross-ref).
  * 5 tool snippets shortened to 4–10 words (was 24-word
    paragraphs describing mechanics).
  * Each tool's `system_prompt_contribution.guidelines` now
    cross-references its sibling tools ("For finding files by
    NAME use find", "For whole-file rewrites use write").
  * Regression suite: 4 tests lock in bash/grep/find/tokens.

**TUI status bar (Pi parity)**

  * `(provider) model` prefix — `(anthropic) MiniMax-M3`.
    Lets users tell at a glance which backend is active.
  * `• thinking-level` indicator (when set).
  * Tool calls show **`Took 1.23s`** / **`Took 850ms`** pill
    on the result line. New `duration_ms` field plumbed
    through `ToolOutput` → `AgentEvent` → `AgentEventLite`
    → transcript.

**Reasoning display**

`<think>...</think>` content is no longer stripped (the
v0.7.4 choice). It now renders with a 💭 prefix and
dim/italic style so users can follow the model's reasoning
without it dominating the transcript. Stored in the
transcript but NOT in the session log — replays don't
re-emit reasoning. Pi-style.

**Tests**: 714 pass / 0 fail (was 698).

**End-to-end verified** with real MiniMax-M3 LLM:
  * "List files in src/" → model picks `bash` (was `find`)
  * "Find files containing TODO" → `grep`
  * "List all .rs files" → `find`
  * "Search 'provider' in *.rs" → `grep` with `include`

# Changelog

All notable changes to nini will be documented here. The format is
based on [Keep a Changelog](https://keepachangelog.com/), and this
project follows [Semantic Versioning](https://semver.org/).

## [0.7.4] - 2026-09-24

One commit on top of v0.7.3. Closes the 3 remaining medium
follow-up issues from the end-to-end UX test documented at
`docs/ux-reports/v0.7.2-end-to-end.md`.

### Bug fixes

**TUI didn't redraw between agent events**

- Symptom: status bar always showed `idle` even when the agent
  was actively streaming. Cause: the runtime loop only redraws
  on the 50ms tick, on user input, or on agent-finished. Fast
  agents that completed in <50ms never showed `working` or
  `Working` — user only saw the final state.
- Fix: `AgentSink` now carries an `Arc<Notify>`. Every `push`
  call notifies the runtime's `select!` loop, which falls through
  to re-snapshot and re-render. New `_ = sink_notify.notified()`
  branch added in `tokio::select!`.

**`<think>...</think>` reasoning blocks leaked to transcript**

- Symptom: MiniMax-M3 and similar reasoning-capable models emit
  their `thinking` inside the same `delta.content` as the
  answer, wrapped in `<think>...</think>` tags. Without
  stripping, the user sees the model's internal monologue
  before the real answer.
- Fix: `ThinkTagFilter` (new struct in `nini-ai/src/openai.rs`)
  walks text chunks in order and emits only the content OUTSIDE
  `<think>...</think>` blocks. Tags may span chunk boundaries
  (e.g., chunk N ends with `<` and chunk N+1 starts with
  `mm:think>`), so the filter holds back text from the last `<`
  to the end of the chunk as a potential partial-tag buffer.
- 10 new unit tests cover: pass-through, full block stripping,
  tag at start/end, newlines inside tag, multiple blocks,
  unclosed tag across chunks (both opening and closing), empty
  input, and a realistic MiniMax-M3-shaped stream.

**Status bar always showed `test-model` even when `--model` was set**

- Symptom: `nini test-model | ~/nini | ...` regardless of CLI flag
  or `settings.json` content.
- Cause: the bootstrap function in `nini-cli/src/main.rs` had a
  logic bug — it read `s.provider` first and assigned it to
  `state.model`, then read `s.model`. The intended order is:
  1. CLI `--model` flag
  2. `settings.json` model field
  3. `settings.json` provider field (fallback)
  4. `"test-model"` (last resort)
- Fix: reordered the bootstrap priority list to match the
  intended order.

### Test coverage

- **689 tests passing** (was 679 in v0.7.3). +10 from
  `ThinkTagFilter` tests.
- All 689 tests green across 14 suites.

### Compatibility

- No public API breakage.
- Tests in `crates/nini-tui/tests/agent_wire_e2e.rs` and
  `crates/nini-tui/tests/interactive_e2e.rs` updated to pass the
  new notify arg to `AgentSink::new` (10 occurrences).

## [0.7.3] - 2026-09-24

One commit on top of v0.7.2. Closes the architectural purity
gap identified in the v0.7 plan review: `convert_to_llm` was
identity at the agent loop because the actual 7→3 (nini's
10→4) conversion was duplicated in 3 crates AND had a real
behavioral bug.

### Bug fixes

**ToolResult messages were DROPPED on session reload** (v0.6.1
→ v0.7.0 regression)

- The legacy read direction used `_ => None` for every
  non-User/Assistant `entries::AgentMessage` variant, so
  `ToolResult` messages stored in session files vanished
  after reload. **Effect**: any agent loop that saved a
  session and reloaded it lost all tool results — the model
  no longer knew what its tools had returned.
- **Fix**: introduced `nini_core::conversion` as the single
  chokepoint for `SessionEntry::Message → provider::Message`.
  The new chokepoint emits `Role::Tool` (preserving
  `tool_use_id` + `is_error`) per the wiki 7→3 table.

**Assistant messages were stored as `Custom("assistant")`** in
session files

- `nini_session::convert_to_pi_message` was using the wrong
  `entries::AgentMessage` variant (`pi::Custom` with a
  discriminator string) instead of the dedicated
  `pi::Assistant(AssistantMessage)` variant that has existed
  since v0.6.x.
- **Fix**: write direction now maps `Role::Assistant →
  pi::Assistant` and `Role::Tool → pi::ToolResult`. The
  read direction (new chokepoint) handles the inverse
  mapping correctly.

### Added

- **`nini_core::conversion`** — single chokepoint for
  `entries::AgentMessage → provider::Message` conversion.
  Three public functions:
  - `session_entry_to_llm_message(&SessionEntry) -> Option<Message>`
  - `default_session_to_llm(&[AgentMessage]) -> Vec<Message>`
  - `session_message_to_llm(&AgentMessage) -> Option<Message>`
- `nini_tui::commands::entry_legacy_message` and
  `nini_cli::main::cli_entry_legacy` delegate to the
  chokepoint (was 28 + 25 lines of per-variant match; now 9 +
  8 lines of delegation).
- `push_then_read_round_trip` regression test in
  `nini-session` — any future change that breaks the
  write→read round-trip (User / Assistant / ToolResult) fails
  loudly.

### Test coverage

- **665 tests passing** (was 663 in v0.7.0). 20 new
  conversion unit tests + 1 round-trip integration test.

### Compatibility

- Session files written by v0.7.0 with the wrong variants
  (`Custom("assistant")`, `Custom("toolResult")`) still
  round-trip — the new chokepoint handles `Custom` in the
  read direction per the wiki (custom → user).
- No public API surface change.

## [0.7.2] - 2026-09-24

One commit on top of v0.7.1. Closes Pi hook #9 — the last
remaining Pi reference architecture piece.

### Added

**Hook #9 — `system_prompt_contribution`**

- New `ToolSystemPrompt { snippet, guidelines }` struct in
  `nini_core::tool`. Default = empty; convenience
  `Self::empty()` for tools that want to opt in with no content.
- `Tool::system_prompt_contribution(&self) -> Option<ToolSystemPrompt>`
  trait method (default `None`).
- `build_system_prompt_with_contributions(base, registry)` —
  walks every tool, collects contributions, formats Pi-style
  sections appended to the base prompt.
- `build_system_prompt(config, registry)` chokepoint in
  `nini-core/src/agent.rs` — the agent loop calls this once per
  request, replacing `config.system` with the augmented
  prompt before `build_request`.
- The 5 built-in tools (Bash / Read / Edit / Find / Grep) each
  contribute a snippet + 2–3 guidelines.

### Test coverage

- **675 tests passing** (was 665 in v0.7.1). 10 new tests
  (9 unit + 1 integration) cover: empty base + no tools →
  None; populated base + tools → appended; multiple tools each
  get their own line; empty snippet / empty guidelines
  silently skipped; default None vs explicit `Some(empty())`
  both behave as no-op; end-to-end integration test verifies
  the augmented system prompt reaches the request builder.

### Compatibility

- No public API breakage. `ToolSystemPrompt` is additive;
  existing tools continue to compile and behave identically
  unless they override `system_prompt_contribution`.

### Reference architecture status

Pi Agent Loop architecture parity: **23 / 23 = 100%**.

## [0.7.1] - 2026-09-24

One commit on top of v0.7.0. Closes the architectural purity
gap identified in the v0.7 plan review: `convert_to_llm` was
identity at the agent loop because the actual 7→3 (nini's
10→4) conversion was duplicated in 3 crates AND had a real
behavioral bug.

### Bug fixes

**ToolResult messages were DROPPED on session reload** (v0.6.1
→ v0.7.0 regression)

- The legacy read direction used `_ => None` for every
  non-User/Assistant `entries::AgentMessage` variant, so
  `ToolResult` messages stored in session files vanished
  after reload. **Effect**: any agent loop that saved a
  session and reloaded it lost all tool results — the model
  no longer knew what its tools had returned.
- **Fix**: introduced `nini_core::conversion` as the single
  chokepoint for `SessionEntry::Message → provider::Message`.
  The new chokepoint emits `Role::Tool` (preserving
  `tool_use_id` + `is_error`) per the wiki 7→3 table.

**Assistant messages were stored as `Custom("assistant")`** in
session files

- `nini_session::convert_to_pi_message` was using the wrong
  `entries::AgentMessage` variant (`pi::Custom` with a
  discriminator string) instead of the dedicated
  `pi::Assistant(AssistantMessage)` variant that has existed
  since v0.6.x.
- **Fix**: write direction now maps `Role::Assistant →
  pi::Assistant` and `Role::Tool → pi::ToolResult`. The
  read direction (new chokepoint) handles the inverse
  mapping correctly.

### Added

- **`nini_core::conversion`** — single chokepoint for
  `entries::AgentMessage → provider::Message` conversion.
  Three public functions:
  * `session_entry_to_llm_message(&SessionEntry) -> Option<Message>`
  * `default_session_to_llm(&[AgentMessage]) -> Vec<Message>`
  * `session_message_to_llm(&AgentMessage) -> Option<Message>`
- `nini_tui::commands::entry_legacy_message` and
  `nini_cli::main::cli_entry_legacy` delegate to the
  chokepoint (was 28 + 25 lines of per-variant match; now 9 +
  8 lines of delegation).
- `push_then_read_round_trip` regression test in
  `nini-session` — any future change that breaks the
  write→read round-trip (User / Assistant / ToolResult) fails
  loudly.

### Test coverage

- **665 tests passing** (was 663 in v0.7.0). 20 new
  conversion unit tests + 1 round-trip integration test.
- Net additions: 1 session round-trip + 1 conversion
  round-trip = +21 new tests across 2 crates.

### Compatibility

- Session files written by v0.7.0 with the wrong variants
  (`Custom("assistant")`, `Custom("toolResult")`) still
  round-trip — the new chokepoint handles `Custom` in the
  read direction per the wiki (custom → user).
- No public API surface change.

## [0.7.0] - 2026-09-23

Five commits on top of v0.6.1, taking nini from "feature-complete
but hardcoded" to "Pi-compatible hook architecture". The headline:
**every agent-loop extension point is now a trait method with a
default implementation, so in-process Rust extensions can shape
agent behavior without forking nini-core**.

This is the release described in `docs/plans/v0.7-hook-parity.md`.
Every existing call site stays byte-equivalent: `Agent::new`,
`Tool::execute`, `provider::stream`, etc. — all unchanged
signatures, all unchanged behavior unless the caller opts into a
custom hook.

### Added

**v0.7 M1 — Steering / Follow-up message queues**
(`AgentLoopHooks` trait, 2 methods)

- New `crates/nini-core/src/agent_hooks.rs` defines
  `AgentLoopHooks` with two hook methods (default impl = v0.6.1
  no-op behavior):
  * `get_steering_messages(&[Message]) -> Vec<Message>` —
 invoked
    once per loop iteration after tool execution, before the
    next LLM call. Pi contract: lets the user redirect the
    agent mid-run (e.g. "actually, check X first").
  * `get_followup_messages(&[Message]) -> Vec<Message>` —
 invoked
    once per `Agent::run` call when the model emits no further
    tool calls. Pi contract: lets the user schedule work
    after the current task (e.g. Alt+Enter "then run cargo
    test").
- `Agent::with_hooks(...)` constructor + `Agent::new(...)`
  installed with `NoopHooks` by default (no behavior change
  for existing callers).
- `catch_hook_panic` + `catch_hook_panic_async` defensive
  wrappers — a panicking hook never crashes the agent loop;
  the fallback is `R::default()` + a warn to stderr.

**v0.7 M2 — Tool lifecycle hooks (before / after execute)**

- `Tool` trait gained two methods with default `None`:
  * `fn before(&self) -> Option<Box<dyn BeforeExecute>>`
  * `fn after(&self) -> Option<Box<dyn AfterExecute>>`
- New `BeforeExecute` / `AfterExecute` traits in
  `nini_core::tool`. `BeforeExecute::run` may substitute args
  (`Ok(Some(replaced))`), pass through (`Ok(None)`), or deny
  the call (`Err(permission_denied)`). `AfterExecute::run`
  may rewrite the `ToolOutput` (for redaction / truncation /
  extra details).
- `WrappedTool` newtype + public `ArcBoxAdapterBefore` /
  `ArcBoxAdapterAfterTool` adapters let extensions wrap an
  existing tool without rewriting it.
- `ProjectTrustStore::check(&ToolCall) -> BinaryDecision` —
  per-cwd decision collapses the 4-state `TrustLevel` into a
  2-state `BinaryDecision` (Allow / Deny). The `Ask` level
  collapses to `Allow` with a warn log; interactive prompting
  is deferred to v0.8 (needs cross-task async signaling).
- `panic_msg` helper for extracting human-readable strings
  from `Box<dyn Any>` payloads (used by `catch_unwind` on
  panicking async hooks via `FutureExt::catch_unwind`).

**v0.7 M3a — Internal-only message variants**

- `entries::AgentMessage` gained 3 variants per the wiki's
  7-variant model:
  * `Notification(NotificationMessage { kind, data, timestamp })`
  * `UiMessage(UiMessage { component, props, timestamp })`
  * `AppMessage(AppMessage { source, payload, timestamp })`
- Wire format matches pi JSONL v4 (`role: notification` /
  `uiMessage` / `appMessage`).
- `AgentMessage::is_internal_only()` predicate returns `true`
  for these 3 variants — used by `convert_to_llm` (M3b) to
  filter them from the LLM view.
- `filter_internal_only_messages` skeleton in `agent_hooks.rs`
  documents the hook point (M3a's filter lives at the
  entries→provider boundary, not at the provider layer).

**v0.7 M3b — Context hooks**

- `AgentLoopHooks` trait grew from 2 to 5 methods:
  * `transform_context(&[Message]) -> TransformResult` —
    non-mutating view of messages for the LLM. Returns
    `{ messages, dropped_count, reason }` for observability.
  * `convert_to_llm(&[Message]) -> Vec<Message>` — last
    filter before the LLM sees the messages. Default =
    identity at the provider layer (the real filtering
    happens at the entries→provider boundary in callers).
  * `should_stop_after_turn(&[Message]) -> bool` — soft-stop
    hook. Returns `true` to end the run early. Default =
    `false`. Coexists with the hard `max_iterations` cap
    (both fire; whichever trips first wins).
- `TransformResult` struct + `Default` impl (for
  panic-safety fallback).
- All 3 hooks are wired into `Agent::run` and panic-safe via
  `catch_hook_panic`.

**v0.7 M5b — Operations traits + BashRunner cross-crate
migration**

- New `crates/nini-tools/src/operations.rs` (320 lines):
  * `BashOperations` trait — `exec` / `which` / `kill_pg`
  * `ReadOperations` trait — `read` / `stat_size`
  * `ExecOutcome` struct mirrors the wire shape of the
    previous `nini-tui::BashResult`
  * `DefaultBashOperations` (delegates to migrated
    `BashRunner`) + `DefaultReadOperations` (uses
    `std::fs`)
  * `bash_which` helper
- `crates/nini-tui/src/bash_runner.rs` **moved** to
  `crates/nini-tools/src/bash_runner.rs` via `git mv`. The
  runner had no TUI dependencies, so the move is
  pure-organization. `nini-tui::runtime.rs` updated to use
  `nini_tools::bash_runner::BashRunner`.
- 6 existing BashRunner tests + 11 new Operations tests
  pass under the new location.

### Compatibility

- **No public API breakage.** `Agent::new`, `Tool::execute`,
  `provider::stream`, every CLI flag, every slash command —
  all unchanged. v0.7.0 is a pure-additive release.
- `Tool::before` / `Tool::after` default to `None`; existing
  tool implementations (BashTool, ReadTool, EditTool,
  FindTool, GrepTool) compile and behave identically to
  v0.6.1.
- `ProjectTrustStore`'s public API (`load`, `get`, `set`,
  `save`, `clear`, `default_path`) is unchanged; new
  methods (`check`, `get_for_cwd`, `to_binary`) are
  additive.
- Session JSONL files written by v0.6.x read back unchanged
  into v0.7.0. The 3 new variants use distinct `role`
  tags; old variants keep their existing shape.

### Test coverage

- **644 tests passing** (was 580 in v0.6.1). 14 test suites,
  all green on `cargo test --workspace`.
- Net additions by milestone:
  * M1: +14 (steering / followup / noop integration)
  * M2: +14 (project_trust 9 + tool lifecycle 5)
  * M3a: +10 (entries 8 + agent_hooks skeleton 2)
  * M3b: +15 (agent_hooks 6 + context_hook_tests 9)
  * M5b: +11 (operations 11)
- All new hooks have at least one regression test
  confirming v0.6.1 behavior is preserved when the default
  hooks (`NoopHooks`) are used.

### Known gaps (intentional, for v0.8+)

- Mouse support (click, double-click, wheel) — ratatui
  `EnableMouseCapture` is enabled but `MouseEventKind` dispatch
  is not yet wired.
- OSC 52 image paste path (Kitty / iTerm2). Currently
  `arboard` only.
- OAuth / device-flow auth (env API keys only).
- ~46 of ~51 Pi providers not yet wired (have 5 builtin:
  anthropic, openai, openai-responses, openai-compat, fixture).
- Extension stable C ABI — currently nini-ext is in-process
  Rust only.
- TrustStore "Ask" level needs cross-task async signaling;
  deferred to v0.8 alongside the C ABI bridge.
- Parallel tool execution (M4 from the plan) — file-path
  dependency detection unresolved; deferred to v0.8.
- Concrete `before()` implementations on Bash / Read / Edit
  that call `TrustStore::check` — blocked on `ToolContext`
  learning to carry the trust store.

### Architectural change (the headline)

v0.6.1's `Agent::run` was ~333 lines with hardcoded compaction,
retry, and tool execution. v0.7.0 keeps the same control flow
but layers 5 new extension points on top, each backed by a
trait method with a sensible default. The next release can
swap any one of those defaults without touching the others —
the textbook "open-closed principle" payoff of the Pi design
philosophy.

### Credits

Reference architecture: `docs/spec-v0.85.1` (sourced from the
wiki `concepts/pi-agent-loop-architecture.md`). Plan source:
`docs/plans/v0.7-hook-parity.md`.

## [0.6.1] - 2026-09-23

## [0.6.1] - 2026-09-23

One commit on top of v0.6.0 closing the last known P0 stub:
the `/editor` slash command and `Ctrl+G` are now wired to a real
external editor dance.

### Added

**F020 — External editor (Ctrl+G / `/editor`)**

- New `crates/nini-tui/src/editor.rs` (290 lines): `resolve_editor()`
  with `$VISUAL` → `$EDITOR` → `nano` → `vi` → `notepad` fallback
  chain and a tiny `which()` helper; `temp_path()` for per-process
  unique file paths; `edit_in_external_editor(initial)` writes
  initial to a temp file with a 2-line header comment (stripped
  on read-back), spawns the editor synchronously, reads the file
  back, returns `Some(new_text)` / `None` (no change) / `Err`.
- New `KeyAction::OpenExternalEditor`, bound to `Ctrl+G`
  (readline / bash / Pi muscle memory).
- `AppState.pending_external_editor: bool` flag that the run
  loop polls on a dedicated 25 ms tick; the dance itself runs
  via `handle_external_editor_dance()` which:
  1. Snapshots the input and clears the flag.
  2. `LeaveAlternateScreen` + `disable_raw_mode` + show cursor.
  3. Spawns the editor synchronously (run loop is blocked but
     the TUI is suspended so no UI updates are expected).
  4. `enable_raw_mode` + `EnterAlternateScreen` + hide cursor.
  5. Applies the result: `InputBuffer::replace_whole()` for
     changes, status hint for no-changes, error line for
     spawn failure.
  6. `terminal.clear()` to wipe any stale editor output.
- `InputBuffer::replace_whole(new_text)` — pushes an undo
  snapshot before swapping, so Ctrl+Z restores the pre-edit
  buffer. No-op when text is unchanged (avoids polluting the
  undo stack on round-trip-no-change).
- The previously dead `/editor` slash command now actually
  works: it sets `state.pending_external_editor = true`
  instead of writing a dead status string.

### Test coverage

- **580 tests passing** (was 569 in v0.6.0). All 14 suites green.
- New tests in `nini-tui::editor`:
  `temp_path_is_unique_per_call`,
  `strip_header_drops_only_header`,
  `strip_header_preserves_content_without_header`,
  `strip_header_handles_empty_buffer`,
  `which_finds_absolute_paths`,
  `which_returns_none_for_nonexistent`,
  `resolve_editor_finds_something`,
  `edit_in_external_editor_spawns_and_reads_back`,
  `replace_whole_noop_when_unchanged`,
  `replace_whole_swaps_and_pushes_undo`.
- New keymap test in `nini-tui::keys`:
  `ctrl_g_opens_external_editor`.

### Known gaps (intentional, for v0.7+)

- Mouse support (click, double-click, wheel) — ratatui
  `EnableMouseCapture` is enabled but `MouseEventKind` dispatch
  is not yet wired (`F017`).
- OSC 52 image paste path (Kitty / iTerm2). Currently
  `arboard` only (`F018`).
- OAuth / device-flow auth (env API keys only).
- ~46 of ~51 Pi providers not yet wired (have 5 builtin:
  anthropic, openai, openai-responses, openai-compat, fixture).
- Extension stable C ABI (the v1 host invokes
  `nini_ext_activate` but extensions register commands directly
  through the API rather than returning a typed Rust object).

## [0.6.0] - 2026-09-23

Four commits on top of 0.5.0. The headline is "users can actually
finish a workflow now": end-to-end tmux survey of v0.5 surfaced 18
UX issues that blocked basic flows; this release closes the 9 P0/P1
gaps, lands the 5 P2 polish items, and adds 3 of the v0.6 P3
"beyond Pi" features (command palette, transcript search, verified
undo / kill ring).

### Milestone 1 — basic workflows unblocked

**Bash runner** (was a 25-line stub)

- Real `BashRunner` using `std::process::Command` + thread + mpsc +
  `setsid` + process-group kill (`SIGTERM` → `SIGKILL`) on timeout.
  Unix takes the full path; Windows falls back to best-effort.
- `TranscriptLine::BashExecution` now carries an explicit `stderr`
  field, rendered in error color with a `stderr:` header so the
  three streams (stdout / stderr / exit) stay distinguishable.
- Timed-out runs surface the partial output plus a `timed out`
  suffix instead of swallowing stdout.
- 6 new unit tests (stdout / stderr / exit / timeout / no-timeout /
  max-clamp).

**`@` file completion** (was 8-line stub)

- `extract_at_prefix_pair` walks backward to the most recent `@`
  with a whitespace boundary and skips past `/` so `@src/foo` works.
- `search_files_as_struct` uses `walkdir` (4 levels) + prefix
  filter + `SKIP_DIRS` short-circuit (`.git`, `node_modules`,
  `target`, …).
- `CompletionCache` (5 s TTL, `Arc<Mutex>`) avoids re-scanning on
  every keystroke.
- Capped at `MAX_RESULTS = 50` with a "refine to narrow" hint.
- 10 new unit tests, including the email-`@`-rejection and
  end-of-line cursor cases.

**`Ctrl+L` opens the ModelSelector** (was just a status string)

- Wired through `open_selector:model` flag; the runtime spawns
  `ModelSelector` exactly like `/model`.

**Status bar — cwd / git / cost / context%**

- `nini_core::git::git_branch()` (wrapping `git rev-parse`) and
  `set_status_bar_metadata` plumb the start of 5-state status bar.
- `AgentEventLite::Usage(input, output, cost)` surfaces
  `ProviderUsage.cost.total` as `cost_usd` so the renderer can
  format it.
- "Loaded N skills" banner moved out of the transcript into the
  status toast so it no longer collides with selector overlays.

**Slash command coverage** (REGISTRY: 23 → 28)

- `/help` — opens the help overlay (see M2).
- `/debug` — toggles `debug_logging` state.
- `/status` — prints session id / cwd / git branch / tokens /
  cost / context%.
- `/editor` — sets `open_editor:true` for the runtime to spawn
  `$VISUAL` / `$EDITOR`.

**`/resume` scans session subdirectories** (was always "No
sessions found" on real layouts)

- Replaced `read_dir(base)` with `walkdir` at `max_depth = 4`
  over the whole `~/.pi/agent/sessions/` tree.
- Sorts: cwd-name subdir first, then by mtime descending.
- Capped at 50 entries with a `refine with /resume <query>` hint.

**Slash completion popup is scrollable**

- `CompletionPopup` gained `scroll_offset` + `max_visible` (default
  8) and `select_up` / `select_down` call `scroll_into_view`.
- Only `[scroll_offset..scroll_offset + max_visible]` rows render.
- Title shows `N/total ↕` when the popup overflows.
- `complete()` on empty query returns ALL 28 commands (was capped at
  8, hiding 20 of them).

**Single-Enter submit when completion matches**

- `KeyAction::Submit` checks the popup:
  - 1 candidate OR exact prefix → submit immediately.
  - multiple with `argument_hint` → apply + space (do not submit).
  - multiple without hint → apply + submit.
- Tab still does pure accept for power users.

**Selector panel no longer corrupts layout**

- `selector_area` exactly equals the transcript slice
  (`area.height - status(1) - prompt(3) - footer(1) - padding(1)`).
- `SelectorPanel` fills its inner area with `bg_style` and `Clear`
  so prior transcript content doesn't bleed through (the
  `i│put` artifact).
- Removed the in-transcript banner — it overlapped with the
  selector border.

### Milestone 2 — Pi parity polish

**Unified footer**

- F1 toggles `help_extended`; the bottom footer swaps between a
  5-row short footer and a 13-row exhaustive keymap dump.
- Footer toggling does NOT push transcript lines (was a bug in the
  earlier iteration that littered the visible history).

**`Ctrl+D` double-tap confirm**

- `state.pending_quit` is set on the first press; the status bar
  shows a "press Ctrl+D again to quit" prompt.
- Second press within 3 s actually exits; otherwise the flag clears
  and normal operation resumes.

**`!!cmd` private marker**

- `submit_user_input` distinguishes `!cmd` (regular bash run, sent
  to agent) from `!!cmd` (runs and pushes to transcript but is
  NOT injected into agent context). Useful for sensitive commands
  that you don't want logged into the conversation history.

**Selector cleanup**

- `clear_status_after_selector` wipes stale `open_selector:*` and
  `switch model` strings when a selector closes, so the status bar
  doesn't get stuck on a stale prompt.

**Real `/help` overlay**

- New `help_overlay` module wraps `SelectorPanel` to list all 28
  commands with descriptions, argument hints, and fuzzy filter.
- `SelectorState` trait gained `state_query` / `state_set_query`;
  `state.close_help` lets F1 also clear the active selector.

### Milestone 3 — beyond Pi

**F015 — Command palette (Ctrl+Shift+K)**

- VSCode-style single input + fuzzy search across all 28 slash
  commands plus 2 meta actions ("Clear transcript", "Quit nini").
- New `crates/nini-tui/src/command_palette.rs` implements
  `SelectorState` and reuses the `SelectorPanel` widget.
- Dispatch is prefix-based: `cmd:/<name>` → `commands::dispatch`;
  `action:clear` / `action:exit` → direct state mutation.
- Fixed an upstream bug in `handle_selector_key` where
  `visible` (filtered indices) and `state_selected()` (items
  index) were mixed up; filtering + ↑↓ now navigates correctly.
- Keymap note — Ctrl+K **stays** as `KillToLineEnd` (readline /
  bash / Pi muscle memory). Palette lives on **Ctrl+Shift+K**;
  the Shift modifier avoids breaking either the muscle memory for
  Ctrl+K or the existing Ctrl+Shift+P (CycleModelPrev).

**F016 — Verified undo + kill ring end-to-end**

- `KeyAction::Undo / Yank / YankPop / KillWordBackward /
  KillWordForward` all wired through the runtime; previously
  `kill_to_line_end` and `kill_word_backward` pushed undo
  snapshots AFTER truncation, which broke Ctrl+Z roundtrip.
- Reorder: kill ops now snapshot BEFORE truncating so undo
  restores the deleted text.
- 6 new unit tests covering `kill_word_backward`,
  `kill_word_forward`, yank restores, undo restores, and the
  full kill+undo roundtrip.

**F019 — Transcript full-text search (Ctrl+F)**

- New `KeyAction::OpenSearch` bound to `Ctrl+F`. Original
  attempt to bind plain `/` collided with empty-input
  slash-command autocomplete, so the key was switched.
- `SearchState` on `AppState` tracks query + match indices +
  cursor. Printable chars update the query in search mode (not
  the input buffer).
- `n` / `N` jump to next / previous match with wrap-around.
- Backspace pops one char from the query.
- `Esc` exits search.
- `render_search_bar` shows query + `N/total` counter + hint.
- Layout fix (commit `d4070d0`): extend to 5 rows when search
  is active; ratatui keeps chunk indices stable so chunks[2..4]
  are the same regardless of search state. Earlier code
  hard-coded different indices and pushed the prompt up by a row.

### Test coverage

- **569 tests passing** (was 532 in v0.5.0). 10 consecutive
  `cargo test --workspace` runs all green.
- Test breakdown: 36 nini-ai + 6 nini-cli lib + 21 cli e2e +
  110 nini-core + 3 agent integration + 9 nini-ext + 4
  extension integration + 38 nini-tools + 173 nini-tui +
  15 agent wire e2e + 37 autocomplete e2e + 9 compaction e2e +
  31 interactive e2e + 52 slash command e2e + 24 tui e2e.

### Known gaps (intentional, for v0.7+)

- Mouse support (click, double-click, wheel) — ratatui
  `EnableMouseCapture` is enabled but `MouseEventKind` dispatch
  is not yet wired (`F017`).
- OSC 52 image paste path (Kitty / iTerm2). Currently
  `arboard` only (`F018`).
- Ctrl+G external editor (stubbed; would spawn `$VISUAL` /
  `$EDITOR` / `nano`).
- OAuth / device-flow auth (env API keys only).
- ~46 of ~51 Pi providers not yet wired (have 5 builtin:
  anthropic, openai, openai-responses, openai-compat, fixture).
- Extension stable C ABI (the v1 host invokes
  `nini_ext_activate` but extensions register commands directly
  through the API rather than returning a typed Rust object).

## [0.5.0] - 2026-09-22

Eight sessions of work on top of 0.4.0, taking nini from a public
skeleton to a feature-complete Pi-compatible coding agent.

### Added

**Transcript rendering**

- Full markdown rendering for assistant messages: ATX headings
  (h1-h6), bullet/ordered/task lists, blockquotes, fenced code
  blocks (with language tag), inline `code`, **bold**, *italic* /
  _italic_, `[label](url)` links, soft-wrap at 100 chars.
- ANSI-strip pass before markdown so tool-output escapes don't leak
  through the renderer.
- OSC 8 hyperlink auto-linking (URLs in assistant text become
  clickable on supported terminals).

**Themes**

- Theme system with 15 named color slots (background/foreground/dim/
  muted/borderMuted/accent/success/warning/error/info/code/link/text/
  toolPendingBg/header).
- Two built-in palettes (`Theme::dark()` Pi-style, `Theme::light()`)
  with real hex colors instead of the previous stub.
- JSON theme load via `Theme::load_from_file` / `Theme::parse`.
  Accepts `#RGB`, `#RRGGBB`, `#RRGGBBAA`, and ANSI color names.
- `theme.fg_style(slot)` adds BOLD for emphatic slots (accent,
  success, error, warning, info, header).

**Selectors** — all 6 wired up

- `ModelSelector` lists the 10-entry builtin catalog from
  `ModelRuntime::with_defaults`, tags the current model as
  `is_current`.
- `SessionSelector` scans `~/.pi/agent/sessions/*.jsonl`, sorted
  newest-first.
- `ThinkingSelector` — 5 levels (off/minimal/low/medium/high).
- `TrustSelector` — 3 levels (ask/trusted/never) tied to the
  project's `ProjectTrustStore`.
- `SettingsSelector` — 3 categories (model/theme/thinking), each
  showing the current value.
- `TreeSelector` — visualizes a `&[SessionEntry]` slice with
  per-entry icons (◇ branch, ◆ compaction, › user, • assistant, etc.).
- `SelectorState::state_items` returns fresh items every call so the
  runtime can re-render after every key press.
- `SelectorPanel` is a real ratatui `Widget` — border, title bar,
  filter prompt, scrollable list with selection highlight.
- Word-bounded fuzzy filter (subsequence match within a single
  whitespace-delimited word) so typing "oai" matches "openai" but
  NOT "claude opus 4.7".

**Slash commands**

- `/resume <filter>` — substring filter on session filename; surfaces
  mtime + size for each entry; emits "(no sessions matching ...)"
  on a fully-filtered list.
- `/resume 99` — proper `CommandResult::error` with "Invalid index
  ... Available: 1-N" instead of silently falling through to list.
- 23 slash commands total (was 24; the registry was already complete
  before 0.5.0).

**Editor**

- Ctrl+O toggle collapsed on tool output / tool call / bash
  execution. `AppState::toggle_collapsed(idx)` flips the flag;
  `AppState::collapse_all()` folds every collapsible line.
- Collapsed lines render only their header + a dim "▸ Ctrl+O to
  expand" hint.

**Compaction**

- `BRANCH_SUMMARY_PROMPT` — 6-section prompt (Goal / Constraints /
  Progress / Key Decisions / Next Steps / Output Format) modelled
  after pi-mono.
- `summarize_branch_with_llm(entries, provider, model, ...)` —
  streams an LLM-backed summary, falls back to deterministic
  heuristic on any provider error.
- `summarize_branch(entries)` — offline fallback that extracts
  file operations and user-message counts.
- `serialize_entries_for_prompt` — flattens entries to
  `[NNNN] role body` lines, dropping oldest entries when the running
  total exceeds 24 KB.
- `format_file_operations` — `<read-files>`/`<modified-files>` XML
  block for prompt input (matches pi-mono).

**Settings**

- `SettingsManager` is real (was a 49-line stub). Loads from
  `~/.pi/agent/settings.json`, writes back on `flush()`.
- `Settings` struct has `context_window` (200k default), `reserve_tokens`
  (16k), `keep_recent_tokens` (20k) — used by the auto-compaction
  trigger.
- `Settings::model_name`, `set_default_model`, `set_default_thinking_level`
  — all wired through to the inner struct + JSON serialization.

**Auto-compaction trigger**

- `AppState::should_auto_compact(&Settings) -> bool` — cheap
  `(chars / 4)` heuristic that returns true when the transcript
  exceeds `context_window - reserve_tokens`.
- `AppState::auto_compact_local() -> usize` — deterministic local
  compaction: fold the older half into a `[CONTEXT SUMMARY]` block
  at the head.
- Wired into `submit_user_input`: before each user prompt, if the
  trigger fires, the runtime folds the prefix and pushes a
  "[auto-compact] folded N entries before next turn" line.

**Extensions** (nini-ext now usable)

- `ExtensionLoader::discover(dir)` — walks `dir` for shared-library
  suffixes (`*.so` / `*.dylib` / `*.dll` / `*.ext` / `*.nini_ext`),
  strips the conventional `lib` prefix.
- `ExtensionLoader::load_all(dir, api)` — opens each library with
  `libloading`, looks up the `nini_ext_activate` symbol, and calls
  it. Extensions that fail to load are silently skipped (pi parity).
- `RuntimeApi` is the concrete `ExtensionAPI` implementation,
  thread-safe (`Arc<Mutex<RegisteredState>>` underneath).
- 6 `ExtensionAPI` methods: `register_command`, `register_tool`,
  `send_user_message`, `get_active_model`, `get_current_cwd`,
  `set_status`.
- Each loaded library is kept alive via an `Arc<libloading::Library>`
  held in a `LoadedHandle`.

**Misc**

- Save image with atomic counter + microseconds in
  `save_image_to_temp` (was: `pid + millis` — racy under parallel
  test execution).
- `Entry::default()` added (was missing).
- `AgentMessage = provider::Message` type alias, `entries::EntryType`
  enum, etc. — several internal types reshaped from the 0.4.0
  stub shape.

### Test coverage

- **532 tests passing** (was 233 in 0.4.0) — 10 consecutive
  `cargo test --workspace` runs all green.
- Test breakdown: 36 nini-ai + 6 nini-cli lib + 21 cli e2e +
  108 nini-core + 3 agent integration + 4 nini-ext + 38 nini-tools +
  138 nini-tui + 15 agent wire e2e + 37 autocomplete e2e + 9
  compaction e2e + 31 interactive e2e + 52 slash command e2e + 24
  tui e2e.

### Known gaps (intentional, for v0.6+)

- Ctrl+G external editor (stubbed; would spawn `$VISUAL` /
  `$EDITOR` / `nano`).
- OAuth / device-flow auth (env API keys only).
- ~46 of ~51 Pi providers not yet wired (have 5 builtin: anthropic,
  openai, openai-responses, openai-compat, fixture).
- LLM-based compaction prompt in addition to the heuristic fallback
  (the agent loop calls `compact_history` with `inline_llm_summary`,
  but the prompt is still the v1 one).
- Extension stable C ABI (the v1 host invokes `nini_ext_activate`
  but extensions register commands directly through the API rather
  than returning a typed Rust object).

## [0.4.0] - 2026-09-12

Initial public release. Not announced on any external channel.

[0.7.4]: https://github.com/jinxumi-oss/nini/compare/v0.7.3...v0.7.4
[0.7.3]: https://github.com/jinxumi-oss/nini/compare/v0.7.2...v0.7.3
[0.7.2]: https://github.com/jinxumi-oss/nini/compare/v0.7.1...v0.7.2
[0.7.1]: https://github.com/jinxumi-oss/nini/compare/v0.7.0...v0.7.1
[0.7.0]: https://github.com/jinxumi-oss/nini/compare/v0.6.1...v0.7.0
[0.6.1]: https://github.com/jinxumi-oss/nini/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/jinxumi-oss/nini/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/jinxumi-oss/nini/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/jinxumi-oss/nini/releases/tag/v0.4.0
