# Contributing to nini

Thanks for your interest in contributing! nini is a young project and
every contribution — bug reports, docs, code — helps it grow.

## Code of conduct

We follow the [Contributor Covenant v2.1](CODE_OF_CONDUCT.md). Be
respectful. Assume good faith. Don't be a jerk.

## Reporting bugs

Open a [GitHub issue](https://github.com/jinxumi/nini/issues/new?template=bug_report.md).
Include:

- nini version (`nini --version`)
- OS and terminal (for TUI bugs)
- Provider (`--provider`)
- Reproduction steps
- Expected vs actual behavior
- Relevant log output (run with `RUST_LOG=debug`)

## Requesting features

Use the [feature request template](https://github.com/jinxumi/nini/issues/new?template=feature_request.md).
Look for the `good first issue` label — those are scoped, well-defined
tasks suitable for first-time contributors.

## Submitting code

### Workflow

1. **Fork** the repository.
2. **Branch** off `main` with a descriptive name:
   - `feat/compaction-llm` for new features
   - `fix/grep-overlapping-matches` for bug fixes
   - `docs/cli-quickstart` for documentation
3. **Implement** with tests. All new code must include unit tests.
   Public API changes need e2e tests.
4. **Verify locally**:
   ```bash
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   ```
5. **Push** to your fork and open a Pull Request against `main`.

### Pull request checklist

The PR template will guide you. The essentials:

- [ ] Tests cover the change (unit + integration where appropriate)
- [ ] `cargo fmt`, `cargo clippy`, `cargo test` all pass locally
- [ ] Commit message follows the convention below
- [ ] Docs updated (README, doc comments, or `docs/`)
- [ ] No new unsafe code (`#![forbid(unsafe_code)]` is workspace-wide)

### Commit message convention

We follow the same pattern as Pi upstream:

```
<type>(<scope>): <subject>

<body>

<footer>
```

**Types**: `feat`, `fix`, `docs`, `refactor`, `test`, `chore`, `perf`
**Scope** (optional): `core`, `ai`, `tui`, `tools`, `session`, `cli`
**Subject**: imperative mood, ≤ 72 chars, no trailing period

Examples:
```
feat(tui): add slash-command autocomplete dropdown
fix(bash): reap grandchild processes on timeout
docs: add architecture diagram to README
```

### Code style

- `rustfmt` is authoritative (run `cargo fmt`)
- `clippy` warnings are errors in CI
- Prefer `&str` over `String` in function signatures when ownership isn't needed
- Add doc comments to all `pub` items (the workspace lints enforce this)
- New providers and tools must implement the documented traits

## Project structure

See [`README.md`](README.md#architecture) for the crate layout. The
short version:

- `crates/nini-core/` — types and traits, no I/O
- `crates/nini-ai/` — provider implementations (depend on `nini-core`)
- `crates/nini-tools/` — built-in tools
- `crates/nini-session/` — JSONL codec
- `crates/nini-tui/` — terminal UI
- `crates/nini-cli/` — `nini` binary

## Setting up a development environment

```bash
git clone https://github.com/jinxumi/nini
cd nini
cargo test --workspace          # ~30 seconds, all 233 tests
cargo clippy --workspace --all-targets
cargo build --release            # 4.3 MB optimized binary
```

## Release process

Maintainers run `cargo dist` to produce platform binaries. Tags follow
`vMAJOR.MINOR.PATCH` (e.g., `v0.5.0`). CHANGELOG entries are required
for any user-visible change.
