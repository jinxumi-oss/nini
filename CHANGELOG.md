# Changelog

All notable changes to nini will be documented here. The format is
based on [Keep a Changelog](https://keepachangelog.com/), and this
project follows [Semantic Versioning](https://semver.org/).

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

[0.6.0]: https://github.com/jinxumi-oss/nini/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/jinxumi-oss/nini/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/jinxumi-oss/nini/releases/tag/v0.4.0
