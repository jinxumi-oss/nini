# Security policy

## Supported versions

The release line is `v1.0.0` (latest). Only the latest release is supported;
we do not backport security fixes to older releases.

## Reporting a vulnerability

Please **do not** open a public GitHub issue for security-sensitive bugs.

Report privately via GitHub's [private vulnerability reporting][gh-private]
on the nini repository. This routes the report to maintainers privately
without disclosing it publicly.

[gh-private]: https://github.com/jinxumi-oss/nini/security/advisories/new

When reporting, include:

- A clear description of the vulnerability
- Steps to reproduce (or a minimal test case)
- Affected versions (commit hash if possible)
- Your assessment of impact and severity

You should receive an acknowledgment within 72 hours. We aim to publish
a fix or advisory within 14 days for critical issues, longer for less
severe ones. We will coordinate disclosure timing with you.

## What we won't do

- We will not threaten or pursue legal action against researchers who follow this policy
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

## Verification that backs this policy

The security-related claims above are backed by checks that run in CI and in
the release flow — not by a human review alone. They are listed so the
boundaries are auditable:

- **Dependencies are locked.** `Cargo.lock` is committed, and `publish = false`
  in the workspace, so there is no crates.io entry point for a future release
  to pull into. Nothing is fetched from the network at build time beyond the
  locked versions.
- **Release artifacts are identity-checked before they ship.**
  `scripts/build-release.sh` builds with `--release --locked`, refuses to run
  when the binary cannot resolve its own commit / time identity, runs a smoke
  check (`info`, `demo`, fixture-provider run), and the tarball is packaged as
  `nini-${HOST_TRIPLE}.tar.xz`; each release ships a `.sha256` sidecar that
  can be verified against the archive.
- **CI gates the code that ships.** `cargo fmt` and `clippy -D warnings` are
  enforced; the full suite runs on ubuntu and macos in debug and under the
  release profile, plus a CLI smoke and a version-identity check that a shipped
  binary must name its source.
- **No un-used unsafe is allowed to ship.** The workspace lints set
  `unsafe_code = "allow"` with `unused_unsafe = "allow"`, so an `unsafe` block
  that is never used is not a shippable change. (There is intentionally no
  `forbid(unsafe_code)` across the workspace — that would be a stronger claim
  than what gates in here.)
- **Unlisted boundaries stay boundaries.** The threat model above states
  exactly what ini does not defend against. A security fix only closes a
  documented boundary; boundaries not listed here are not closed by this
  repo, and so stay boundaries.

## Workspace policy

Supply-chain tooling (`cargo audit`, `cargo deny`) is **not** currently wired
into CI. The honest statement is "not wired in yet", not "not needed"; adding
them must happen with a baseline first, so they don't gate on a baseline that
doesn't exist.
