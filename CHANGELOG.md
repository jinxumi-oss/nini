# Changelog

All notable changes to nini will be documented here. The format is
based on [Keep a Changelog](https://keepachangelog.com/), and this
project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Slash-command autocomplete dropdown (`/` triggers fuzzy match panel,
  `↑↓`/`Tab`/`Esc` to navigate)
- Compaction algorithm (cut-point heuristic + local summary; auto-fires
  on context overflow)
- Skills + settings + models.json loaders (Pi-compatible)
- 23/23 slash commands recognized; 9 fully implemented, 14 stubbed
- TUI (ratatui + crossterm) with Pi-compatible keybindings
- 6 built-in tools: bash, read, write, edit, grep, find
- 5 LLM providers: anthropic, openai, openai-responses, openai-compat,
  fixture (offline)

### Test coverage

- 233 tests passing (unit + integration + e2e + property fuzz)
- 6 TUI integration suites with `ratatui::TestBackend` snapshot tests
- CLI e2e tests that spawn the compiled binary
- 500-case proptest fuzz on the SSE parser

### Known gaps (intentional, for v0.5+)

- Stubbed slash commands: `/tree`, `/fork`, `/clone`, `/import`,
  `/share`, `/login`, `/logout`, `/changelog`, `/resume`,
  `/scoped-models`, `/settings`, `/compact`
- No session persistence (`--continue` / `--session` not wired)
- No OAuth / device-flow auth (env API keys only)
- LLM-based summarization (currently uses deterministic heuristic)
- 5 of ~51 Pi providers implemented natively

## [0.4.0] - 2026-09-12

Initial public release. Not announced on any external channel.

[Unreleased]: https://github.com/jinxumi-oss/nini/compare/main...HEAD
[0.4.0]: https://github.com/jinxumi-oss/nini/releases/tag/v0.4.0
