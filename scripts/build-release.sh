#!/usr/bin/env bash
#
# Build the binary that ships, and check that it can identify itself.
#
# Why this exists: nini's claims (test count, parity surface) are verified
# *against a specific binary*, and `~/.local/bin/nini` is a symlink into
# `target/`. A stale `target/release/nini` is therefore a correctness
# problem, not a nuisance — it was once 8 fixes behind the working tree
# while `--version` still said the right commit number, because the
# identity is only as good as the build that embedded it.
#
# `crates/nini-cli/build.rs` fills in commit / time / dirty from git, so
# no environment variables are needed. Set NINI_BUILD_COMMIT to override
# (vendored builds), or SOURCE_DATE_EPOCH for a reproducible timestamp.
#
# Usage: scripts/build-release.sh [--check]

set -euo pipefail

cd "$(dirname "$0")/.."

echo "==> cargo build --release --locked"
cargo build --release --locked

BIN=target/release/nini

echo
echo "==> $("$BIN" --version)"
version_line="$("$BIN" --version)"

# A release binary that cannot name its source is exactly the failure this
# script exists to catch, so it is a hard error rather than a warning.
case "$version_line" in
  *unknown*)
    err() { echo "ERROR: $*" >&2; exit 1; }
    err "build identity is 'unknown' — build.rs could not reach git, or the build was vendored without .git"
    ;;
esac

if git diff --quiet --ignore-submodules HEAD -- 2>/dev/null && git diff --cached --quiet 2>/dev/null; then
  :
else
  echo "NOTE: the working tree is dirty; the binary above says so (+dirty)."
fi

echo
echo "==> smoke"
"$BIN" info    > /dev/null
"$BIN" demo    > /dev/null
"$BIN" -p "smoke test" --provider fixture < /dev/null > /dev/null
echo "ok: $version_line"

if [[ "${1:-}" == "--check" ]]; then
  echo
  echo "==> README test count vs binary"
  readme_count=$(grep -oP 'badge/tests-\K[0-9]+' README.md | head -1)
  echo "README claims $readme_count; verify with: cargo test --workspace | grep -oP 'ok\. \K[0-9]+(?= passed)' | paste -sd+ | bc"
fi
