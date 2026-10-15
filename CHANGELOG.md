## v0.10 — Three chains that were built and never connected (UNRELEASED)

A long task used to break in one specific shape: it would run, cross
the context window, and die. Not with an error — with the history
intact, nothing compacted, and a retry that could not succeed. The
cause was not one bug but a **category**: nini accumulates working
code that is never connected to anything, and the suite is green
because every test exercises the piece rather than the assembly.

Tests: **1280 → 1341**.

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

## v0.9.2 — Things that told you they worked when they didn't (UNRELEASED)

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
