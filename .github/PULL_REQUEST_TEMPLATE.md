---
name: Pull request
about: Contribute code, docs, or tests to nini
title: ""
labels: ""
assignees: ""
---

### What does this PR do?

One-paragraph summary. Reference any related issues with `#123`.

### Type of change

- [ ] Bug fix (non-breaking change that fixes an issue)
- [ ] New feature (non-breaking change that adds functionality)
- [ ] Breaking change (fix or feature that changes existing behavior)
- [ ] Documentation
- [ ] Refactor (no behavior change)

### How was this tested?

- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes
- [ ] `cargo test --workspace` passes (233 tests)
- [ ] Added new tests for the change

If you added a new tool or provider, list the integration tests you added.

### Checklist

- [ ] Commit messages follow `<type>(<scope>): <subject>` (see CONTRIBUTING.md)
- [ ] Public API items have doc comments (enforced by `missing_docs` lint)
- [ ] No new `unsafe` code (`#![forbid(unsafe_code)]` is workspace-wide)
- [ ] CLI behavior unchanged (or CHANGELOG.md updated)
- [ ] If this changes the session JSONL format or slash command surface,
      documented the compatibility implications

### Related issues

Link any related issues. Use `Fixes #123` to auto-close.

### Screenshots / recordings

If your change affects the TUI, attach a screenshot or short screen
recording. Without it, reviewers can only read the diff.
