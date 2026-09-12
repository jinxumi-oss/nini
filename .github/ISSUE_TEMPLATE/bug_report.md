---
name: Bug report
about: Something is broken or behaving unexpectedly
title: ""
labels: bug
assignees: ""
---

### Describe the bug

A clear and concise description of what the bug is.

### Reproduction

Steps to reproduce the behavior:

1. `nini --provider ... -p "..."` (or interactive TUI steps)
2. Observe: ...
3. Expect: ...

### Expected behavior

What you expected to happen.

### Actual behavior

What actually happened. Include the full output (especially stderr and
any panic / assertion failure messages).

### Environment

- nini version: (`nini --version`)
- OS + arch: (e.g., `linux/x86_64`, `macos/aarch64`, `windows/amd64`)
- Terminal: (e.g., `gnome-terminal`, `wezterm`, `tmux`)
- Provider: (`anthropic`, `openai`, etc.)
- Model: (e.g., `claude-opus-4-7`, `gpt-5`)
- Install method: (release binary, `cargo install`, built from source)

### Logs

If applicable, run with `RUST_LOG=debug nini -p "..." 2>&1 | tee /tmp/nini.log`
and attach the relevant portion (redact any API keys).

### Possible cause

If you have a hunch about what's going wrong, share it. Otherwise omit.

### Additional context

Anything else that might help — screenshots, related issues, etc.
