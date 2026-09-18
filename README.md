# nini

> A Pi-compatible coding agent, rewritten in Rust.

[![Status](https://img.shields.io/badge/status-v0.4--alpha-yellow)](https://github.com/jinxumi-oss/nini/releases)
[![License](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
[![Tests](https://img.shields.io/badge/tests-528%20passing-green)](https://github.com/jinxumi-oss/nini/actions)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange)](https://www.rust-lang.org)

nini is a clean-room Rust reimplementation of [Pi](https://github.com/earendil-works/pi),
the AI coding agent by Mario Zechner. It reads the same `~/.pi/agent/`
configuration, produces the same session JSONL v4, and exposes the same
23 slash commands. Drop it in alongside Pi and your existing skills,
settings, and models.json keep working.

## Why nini?

- **Single 4.3 MB binary** instead of a Node.js toolchain
- **No license riders** — pure MIT, unlike forks carrying OpenAI/Anthropic
  redistribution restrictions
- **528 unit + integration tests** including property-based SSE fuzz
- **Built-in tools you can extend** — bash, read, write, edit, grep, find
- **Streaming TUI** with slash-command autocomplete

## Features (v0.4.0)

| Area | What works |
|---|---|
| **Providers** | Anthropic, OpenAI (Chat + Responses), OpenAI-compatible (any `/v1/chat/completions`), fixture (offline test) |
| **Tools** | `bash` (process-tree kill, timeout, SIGTERM→SIGKILL), `read`, `write` (atomic), `edit` (exact-match), `grep` (regex + .gitignore), `find` (glob + .gitignore) |
| **TUI** | ratatui + crossterm, readline-style editing, kill ring, history recall, slash-command autocomplete dropdown |
| **Slash commands** | All 23 Pi-compatible commands (`/help`, `/model`, `/export`, `/new`, `/quit`, …) |
| **Session** | JSONL v4 codec + legacy-v3 reader; reads `~/.pi/agent/skills/`, `settings.json`, `models.json` |
| **Compaction** | Local-summary heuristic; auto-triggers on context overflow |

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
| `Ctrl+D` | Quit nini |
| `Ctrl+L` | Switch model |
| `F1` | Help |
| `Ctrl+A` / `Ctrl+E` | Beginning / end of line |
| `Ctrl+K` | Kill to end of line |
| `Ctrl+U` | Clear input |
| `Ctrl+W` | Kill word backward |
| `Ctrl+←` / `Ctrl+→` | Jump word left / right |
| `↑` / `↓` | Recall history (or navigate popup when one is open) |
| `Tab` | Accept selected completion |

## Slash commands

All 23 Pi-compatible commands are recognized. The ones marked **stub**
output a "not yet implemented" message instead of taking real action.

| Command | Argument | Status |
|---|---|---|
| `/settings` | — | stub |
| `/model` | `<provider/model>` | partial (sets state, not persisted) |
| `/tree` | — | stub |
| `/thinking` | `<off\|minimal\|low\|medium\|high\|xhigh\|max>` | partial (validates, not persisted) |
| `/scoped-models` | — | stub |
| `/export` | — | full (writes HTML to `~/.pi/agent/exports/`) |
| `/import` | — | stub |
| `/share` | — | stub |
| `/copy` | — | partial (prints to stdout, no clipboard) |
| `/name` | `<name>` | partial (in-memory only, not persisted) |
| `/session` | — | partial (in-memory state, no session file) |
| `/changelog` | — | stub |
| `/hotkeys` | — | full (keybinding list) |
| `/fork` | — | stub |
| `/clone` | — | stub |
| `/trust` | — | partial (in-memory only) |
| `/login` | `<provider>` | stub |
| `/logout` | — | stub |
| `/new` | — | full (clears transcript) |
| `/compact` | — | stub |
| `/resume` | — | stub |
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

## Known limitations (v0.4.0 alpha)

These are intentional gaps and will be filled in subsequent releases.
File an issue if you need any of them sooner.

- 12 slash commands are stubbed; 11 are partial (see table above)
- Compaction uses a deterministic local summary, not LLM-based
- Session persistence (`--continue` / `--session <id>`) is not wired into the TUI
- 5 of ~51 Pi providers implemented natively; configure the rest via `models.json`
- No OAuth / device-flow auth — env API keys only
- No extension runtime (deferred to v2)
- Model/thinking/name changes are in-memory only; not persisted to `settings.json`

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
└── nini-ext/     # extension runtime stub (deferred to v2)
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
parser, end-to-end CLI tests that spawn the compiled binary, and 6
TUI integration suites using `ratatui::backend::TestBackend` for
frame-level snapshot assertions.

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
