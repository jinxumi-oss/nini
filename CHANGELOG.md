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
