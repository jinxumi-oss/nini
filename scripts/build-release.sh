#!/usr/bin/env bash
#
# Build the binary that ships, and check that it can identify itself.
#
# Why this exists: nini's claims (test count, parity surface) are verified
# *against a specific binary*, and `~/.local/bin/nini` is **a copy of the
# release artifact**, not a symlink into `target/`. A stale
# `target/release/nini` is therefore a correctness problem, not a nuisance
# — it was once 8 fixes behind the working tree while `--version` still
# said the right commit number, because the identity is only as good as
# the build that embedded it.
#
# The same trap, one level up: rebuilding `target/release/nini` does **not**
# update the binary the user's shell runs. The v1.0.0 release shipped a tag
# and a tarball while `~/.local/bin/nini` still held a v0.16.0 build from
# weeks earlier — the release flow had no install step, and the symlink
# assumption above is what hid it. `--install` is that step.
#
# `crates/nini-cli/build.rs` fills in commit / time / dirty from git, so
# no environment variables are needed. Set NINI_BUILD_COMMIT to override
# (vendored builds), or SOURCE_DATE_EPOCH for a reproducible timestamp.
#
# Usage: scripts/build-release.sh [--package] [--install] [--check]

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

# --- flags --------------------------------------------------------------
do_package=0
do_install=0
do_check=0
for arg in "$@"; do
  case "$arg" in
    --package) do_package=1 ;;
    --install) do_install=1 ;;
    --check) do_check=1 ;;
    *)
      echo "unknown flag: $arg" >&2
      echo "usage: scripts/build-release.sh [--package] [--install] [--check]" >&2
      exit 2
      ;;
  esac
done

HOST_TRIPLE="$(rustc -vV | awk '/^host:/{print $2}')"
ARCHIVE="nini-${HOST_TRIPLE}.tar.xz"

if [[ "$do_package" -eq 1 ]]; then
  echo
  echo "==> package release/${ARCHIVE}"
  mkdir -p release
  tar -cJf "release/${ARCHIVE}" -C "$(dirname "$BIN")" nini
  # The sidecar hashes the binary by the name it has *inside the archive*,
  # so `install.sh` can verify it in the extracted directory (the archive
  # holds exactly one top-level file: `nini`).
  ( cd "$(dirname "$BIN")" && sha256sum nini ) > "release/${ARCHIVE}.sha256"
  echo "packaged: release/${ARCHIVE} (+ .sha256)"
fi

if [[ "$do_install" -eq 1 ]]; then
  echo
  echo "==> install into the user environment"
  archive="release/${ARCHIVE}"
  if [[ ! -f "$archive" ]]; then
    echo "ERROR: release/${ARCHIVE} not found — package it first (--package)" >&2
    exit 1
  fi
  install_dir="${NINI_INSTALL_DIR:-$HOME/.local/bin}"
  mkdir -p "$install_dir"
  tmp_dir="$(mktemp -d)"
  trap 'rm -rf "$tmp_dir"' EXIT
  tar -xJf "$archive" -C "$tmp_dir"
  cp "release/${ARCHIVE}.sha256" "$tmp_dir/"
  ( cd "$tmp_dir" && sha256sum -c "${ARCHIVE}.sha256" )
  # Atomic replace (temp file + rename): a running `nini` keeps its old
  # inode, so a live session is never broken — and writing the file in
  # place would fail with ETXTBSY while it is executing.
  tmp_bin="$(mktemp "$install_dir/.nini.XXXXXX")"
  install -m 0755 "$tmp_dir/nini" "$tmp_bin"
  mv -f "$tmp_bin" "$install_dir/nini"
  echo "installed: $install_dir/nini ($("$install_dir/nini" --version))"
  # If the source moved on after this release was packaged, say so: what
  # is installed is the *released* binary, not the newest build.
  embedded="$("$install_dir/nini" --version | awk '{print $4}')"
  if [[ -n "$embedded" ]] && git rev-parse --verify -q "$embedded" >/dev/null; then
    if ! git diff --quiet "$embedded" HEAD -- crates Cargo.toml Cargo.lock; then
      echo "NOTE: the code changed after this release was packaged" >&2
      echo "      (packaged at $embedded) — re-run with --package to publish the newer code" >&2
    fi
  fi
fi

if [[ "$do_check" -eq 1 ]]; then
  echo
  echo "==> README test count vs binary"
  readme_count=$(grep -oP 'badge/tests-\K[0-9]+' README.md | head -1)
  echo "README claims $readme_count; verify with: cargo test --workspace | grep -oP 'ok\. \K[0-9]+(?= passed)' | paste -sd+ | bc"
fi
