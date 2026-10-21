# nini

> A Pi-compatible coding agent, rewritten in Rust.

[![Status](https://img.shields.io/badge/status-v0.10.3-green)](https://github.com/jinxumi-oss/nini/releases)
[![License](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
[![Tests](https://img.shields.io/badge/tests-1502%20passing-green)](https://github.com/jinxumi-oss/nini/actions)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange)](https://www.rust-lang.org)

nini is a clean-room Rust reimplementation of [Pi](https://github.com/earendil-works/pi),
the AI coding agent by Mario Zechner. It reads the same `~/.pi/agent/`
configuration, produces the same session JSONL v4, and exposes the same
23 slash commands plus 5 of its own. Drop it in alongside Pi and your
existing skills, settings, and models.json keep working.

## Why nini?

- **Single 5.4 MB binary** instead of a Node.js toolchain
- **No license riders** — pure MIT, unlike forks carrying OpenAI/Anthropic
  redistribution restrictions
- **1502 unit + integration tests**, including property-based SSE fuzz and
  end-to-end wire-format tests that drive a real HTTP server
- **Built-in tools you can extend** — bash, read, write, edit, grep, find
- **Streaming TUI** with slash-command autocomplete

## Features

| Area | What works |
|---|---|
| **Providers** | Anthropic, OpenAI (Chat + Responses), OpenAI-compatible (any `/v1/chat/completions`), fixture (offline test) |
| **Default model** | `MiniMax-M3.1-Flash-Preview` — override with `--model` or `/model` |
| **Tools** | `bash` (process-tree kill, timeout, SIGTERM→SIGKILL), `read`, `write` (atomic), `edit` (exact-match), `grep` (regex + .gitignore), `find` (glob + .gitignore) |
| **TUI** | ratatui + crossterm, readline-style editing, kill ring, history recall, slash-command autocomplete dropdown, command palette (Ctrl+Shift+K), transcript search (Ctrl+F), external editor (Ctrl+G) |
| **Slash commands** | All 23 Pi builtin commands (`/model`, `/export`, `/new`, `/quit`, …), plus 5 nini-only ones (`/help`, `/status`, `/prompt`, `/debug`, `/editor`) |
| **Session** | JSONL v4 codec + legacy-v3 reader; reads `~/.pi/agent/skills/`, `settings.json`, `models.json` |
| **Compaction** | LLM-backed summary with deterministic fallback; auto-triggers on context overflow |
| **Hooks (v0.7)** | Pi-compatible 5-method `AgentLoopHooks` trait: steering / followup / transform_context / convert_to_llm / should_stop_after_turn. Tool before/after lifecycle hooks. BashOperations + ReadOperations for mockable backends. |

## Quick start

### Install

```bash
curl -fsSL https://raw.githubusercontent.com/jinxumi-oss/nini/main/install.sh | bash
```

The installer places `nini` (and an `nini` launcher for legacy-pi installs)
in `~/.local/bin`. Add it to your `PATH` if it isn't already.

### Run with no API key (fixture provider)

```bash
nini -p "echo hello"
nini demo "find TODOs and fix them"
```

The fixture provider returns scripted responses so you can exercise every
code path without spending a single token.

### Run with a real provider

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
nini --provider anthropic -p "refactor the auth middleware"

# Or via env
export NINI_PROVIDER=openai
export OPENAI_API_KEY="sk-..."
nini -p "summarize this codebase"

# OpenAI-compatible (vLLM, OpenRouter, Together, etc.)
export NINI_PROVIDER=openai-compat
export OPENAI_BASE_URL="http://localhost:8000/v1"
export OPENAI_API_KEY="local"
nini -p "what does this function do?"
```

### Interactive TUI

```bash
nini
```

You get a four-pane terminal UI: status bar, transcript, prompt editor, key
hints. Type `/` to summon the slash-command autocomplete. Press
`Ctrl+C` to abort, `Ctrl+D` to quit.

## CLI commands

| Command | Purpose |
|---|---|
| `nini` | Launch interactive TUI (requires TTY) |
| `nini -p "<task>"` | Single-shot print mode |
| `nini demo [task]` | Scripted autonomous demo (no API key) |
| `nini info` | Show loaded skills, settings, and provider |
| `nini --provider <name>` | Use a specific provider (`anthropic`, `openai`, `openai-responses`, `openai-compat`, `fixture`) |
| `nini --model <id>` | Override the default model |
| `nini --help` / `--version` | Help / version |

## TUI keybindings

| Key | Action |
|---|---|
| `Enter` | Send input |
| `Shift+Enter` | Newline (multi-line input) |
| `Esc` | Abort running / clear input / dismiss popup |
| `Ctrl+C` | Abort current operation |
| `Ctrl+D` | Quit nini (double-tap within 3s) |
| `Ctrl+L` | Switch model |
| `Ctrl+F` | Open transcript search |
| `Ctrl+G` | Open current input in external editor ($VISUAL/$EDITOR) |
| `Ctrl+Shift+K` | Open command palette |
| `F1` | Toggle short / extended footer |
| `Ctrl+A` / `Ctrl+E` | Beginning / end of line |
| `Home` / `End` | Jump transcript to oldest / newest (Pi parity) |
| `Ctrl+K` | Kill to end of line |
| `Ctrl+U` | Clear input |
| `Ctrl+W` | Kill word backward |
| `Alt+D` / `Alt+Y` | Kill word forward / yank-pop |
| `Ctrl+Y` / `Ctrl+/` | Yank / **undo** |
| `Ctrl+T` | Cycle thinking level |
| `Ctrl+P` / `Ctrl+Shift+P` | Cycle model next / previous |
| `Ctrl+O` | Toggle collapsed tool output |
| `Ctrl+V` | Paste image from clipboard |
| `PageUp` / `PageDown` / `Shift+End` | Scroll transcript up / down / to the latest |
| `Ctrl+←` / `Ctrl+→` | Jump word left / right |
| `↑` / `↓` | Move the caret between lines; at the first/last line, recall prompt history |
| `Tab` | Accept selected completion |

## Slash commands

All 23 commands in Pi's `builtin.json` (v0.85.1) are recognized, plus five
nini-only additions — `/help`, `/status`, `/prompt`, `/debug`, `/editor` —
which are **extensions, not parity**, and are excluded from the parity claim.
The commands marked **stub** output a "not yet implemented" message instead
of taking real action.
A flag or command that nini does not implement is **refused with a reason
naming it** — never accepted and dropped.

| Command | Argument | Status |
|---|---|---|
| `/settings` | — | real (selector) |
| `/model` | `<provider/model>` | partial (sets state, not persisted) |
| `/tree` | — | real (ASCII tree of the session) |
| `/thinking` | `<off\|minimal\|low\|medium\|high\|xhigh\|max>` | partial (validates, not persisted) |
| `/scoped-models` | — | stub (no scoping model) |
| `/export` | — | full (writes HTML to `~/.pi/agent/exports/`) |
| `/import` | `<path>` | real |
| `/share` | — | stub |
| `/copy` | — | partial (prints to stdout, no clipboard) |
| `/name` | `<name>` | partial (in-memory only, not persisted) |
| `/session` | — | partial (in-memory state, no session file) |
| `/changelog` | — | real |
| `/hotkeys` | — | full (**derived from the keymap at runtime**) |
| `/fork` | `[n]` / `at n` | real (writes a branch session file) |
| `/clone` | — | real (copies the session file) |
| `/trust` | — | partial (in-memory only) |
| `/login` | `<provider>` | lists providers + where to put the key |
| `/logout` | — | tells you what to unset (cannot mutate the parent shell) |
| `/new` | — | full (clears transcript) |
| `/compact` | — | real (session-layer compaction) |
| `/resume` | — | real (session picker) |
| `/reload` | — | full (re-reads skills) |
| `/quit` | — | full |

## Pi ecosystem compatibility

nini reads and writes the same file formats as Pi v0.85.1:

| What | Where |
|---|---|
| User-level settings | `~/.pi/agent/settings.json` |
| Project-level settings | `./.pi/settings.json` |
| User-level skills | `~/.pi/agent/skills/<name>/SKILL.md` |
| Project-level skills | `./.pi/skills/<name>/SKILL.md` |
| Models | `~/.pi/agent/models.json` and `./.pi/models.json` |
| Sessions | `~/.pi/agent/sessions/<project>/<timestamp>.jsonl` |
| Slash commands | identical names + argument hints |

You can `nini -p` in one terminal and `pi` in another — both write the
same session JSONL.

## Known limitations (as of v0.10.3)

Verified against this binary, not carried over from an older README.
The project's goal and its current status live in
[`docs/GOALS.md`](docs/GOALS.md) — it is the authority on what nini is
trying to be; this section is the user-facing summary of it.

- `/share` is the one command that returns "not yet implemented". The rest
  of the 23 Pi commands dispatch; `/tree`, `/fork` and `/settings` are real
- Compaction uses a deterministic local summary, not an LLM-based one
- `--continue-session` / `--session-id` / `--fork` on the command line are
  refused with a pointer to the TUI command; sessions themselves are
  persisted, and `/resume` and `/fork` work there
- No LLM-backed *branch* summary: `/fork` creates a real branch, but Pi's
  `navigateTree` (moving to another node inside one file, optionally
  summarising what was abandoned) is not implemented
- 5 of ~51 Pi providers implemented natively; configure the rest via `models.json`
- No OAuth / device-flow auth — env API keys only
- No extension runtime. Removed from scope deliberately: it was specified
  as a Rust cdylib while Pi loads TypeScript, so the two have no common
  execution model. See `docs/plans/v0.10-pi-parity-refactor.md` §26
- Prompt templates and AGENTS.md / CLAUDE.md context files are refused with
  a reason rather than silently ignored
- **No syntax highlighting.** Pi colours fenced code blocks (`highlightCode`)
  and does the same inside the write tool's diff; nini renders code blocks
  with the base style only. The nine `syntax*` theme slots exist and are read
  by nothing — they are reserved, not implemented

## Architecture

```
crates/
├── nini-core/   # Agent loop, provider trait, tool trait, session, skills,
│                 #   settings, compaction, message types
├── nini-ai/      # Anthropic + OpenAI + OpenAI-Responses + OpenAI-compat +
│                 #   fixture provider + SSE parser
├── nini-tools/   # bash, read, write, edit, grep, find
├── nini-session/ # JSONL v4 codec + legacy-v3 bridge
├── nini-tui/     # keys + state + render + runtime + slash commands + autocomplete
├── nini-cli/     # `nini` binary entry point
├── nini-conformance/  # Pi parity oracles — no product code, only
│                 #   statements about Pi that can go red
└── nini-ext/     # empty. Kept in the workspace with a header saying why;
                  #   its route was disproved (see the plan, §26)
```

The dependency direction is `nini-cli` → `nini-tui` → `nini-core` →
nothing else. `nini-ai` and `nini-tools` plug into `nini-core` via traits,
so adding a new provider or tool is a single-file change.

## Development

### Build & test

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
```

The test suite includes property-based fuzz (`proptest`) for the SSE
parser, end-to-end CLI tests that spawn the compiled binary, TUI
integration suites using `ratatui::backend::TestBackend` for
frame-level snapshot assertions, and a `nini-conformance` crate that
holds **only** Pi-parity oracles — executable statements about Pi 0.84.2
that a change to nini can falsify.

The rule for conformance tests: fixtures come from real Pi output, never
from nini's own. A test that round-trips nini against itself cannot catch
a wire-format bug, because the bug is symmetric — that is how a
`stop_reason` / `stopReason` casing difference survived 950 green tests.

Every fix in this release was verified by re-injecting the defect and
confirming the specific test goes red.

### Adding a new provider

1. Create `crates/nini-ai/src/<provider>.rs`
2. Implement the `nini_core::Provider` trait (just `name()` and `stream()`)
3. Translate SSE events into `nini_core::StreamEvent`
4. Register it in `nini-cli/src/main.rs::build_provider`
5. Add tests in `crates/nini-ai/src/<provider>.rs`

### Adding a new tool

1. Create `crates/nini-tools/src/<tool>.rs`
2. Implement `nini_core::tool::Tool` (just `name()`, `spec()`, and `execute()`)
3. Register it in `nini-cli/src/main.rs::build_tools`
4. Add tests alongside the implementation

## Acknowledgments

nini is a clean-room reimplementation. The reference specification was
extracted from:

- **[Pi](https://github.com/earendil-works/pi)** by **Mario Zechner** —
  the original TypeScript implementation. nini's session format,
  slash command surface, settings schema, and compaction algorithm
  all follow Pi v0.85.1's documented behavior.

- **Armin Ronacher** and the **Rust community** — for the rich ecosystem
  (tokio, ratatui, reqwest, serde) that makes a project like this
  practical to build in a few weeks.

- The Pi contributors and early users whose feedback shaped the spec
  we read while building this.

If Pi upstream ships a feature you need urgently, please file an
[issue](https://github.com/jinxumi-oss/nini/issues/new) — the priority list
mirrors the most-asked-for upstream features.

## License

MIT — see [LICENSE](LICENSE).

## Contributing

Contributions are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) for the
commit-message style, PR template, and the `good first issue` label.
For security issues, see [SECURITY.md](SECURITY.md).
