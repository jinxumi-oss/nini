# Security policy

## Supported versions

| Version | Supported |
|---|---|
| latest (unreleased `main`) | yes |
| `v0.4.x` | yes |
| `< v0.4` | no |

We do not backport security fixes to older releases.

## Reporting a vulnerability

Please **do not** open a public GitHub issue for security-sensitive bugs.

Report privately via GitHub's [private vulnerability reporting][gh-private]
on the nini repository. This routes the report to maintainers privately
without disclosing it publicly.

[gh-private]: https://github.com/jinxumi/nini/security/advisories/new

When reporting, include:

- A clear description of the vulnerability
- Steps to reproduce (or a minimal test case)
- Affected versions (commit hash if possible)
- Your assessment of impact and severity

You should receive an acknowledgment within 72 hours. We aim to publish
a fix or advisory within 14 days for critical issues, longer for less
severe ones. We will coordinate disclosure timing with you.

## What we won't do

- We will not threaten or pursue legal action against researchers who
  follow this policy
- We will not require you to sign an NDA before reporting
- We will not publicly disclose your identity without your consent

## Threat model

nini is a developer tool that:

- Executes commands on your machine via the `bash` tool (sandboxed by
  timeout and process-tree kill; not a security boundary)
- Reads files anywhere on disk (with `read`, `grep`, `find`)
- Edits files anywhere (with `write`, `edit`)
- Sends conversation content and tool results to the configured LLM
  provider

The threat model assumes:

- The user's shell and filesystem are trusted
- The LLM provider may be hostile or compromised
- The agent's prompt may contain prompt-injection from untrusted file
  contents

nini does NOT defend against:

- Prompt injection from files the model is asked to read (Pi upstream has
  the same limitation)
- A compromised LLM provider exfiltrating local data via tool calls
- Local privilege escalation by a process the `bash` tool spawned

## Workspace policy

The `#![forbid(unsafe_code)]` lint is set workspace-wide. All code is
written in safe Rust. The supply chain is managed via `cargo` with
locked versions (`Cargo.lock` is committed).

We do not currently run `cargo audit` or `cargo deny` in CI; that is
on the v0.5 roadmap.
