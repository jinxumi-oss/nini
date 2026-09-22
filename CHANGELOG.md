# Changelog

All notable changes to nini will be documented here. The format is
based on [Keep a Changelog](https://keepachangelog.com/), and this
project follows [Semantic Versioning](https://semver.org/).

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

[0.5.0]: https://github.com/jinxumi-oss/nini/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/jinxumi-oss/nini/releases/tag/v0.4.0
