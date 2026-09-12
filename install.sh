#!/usr/bin/env bash
# nini installer
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/jinxumi/nini/main/install.sh | bash
#
# Environment variables:
#   NINI_INSTALL_DIR   where to put the binary (default: ~/.local/bin)
#   NINI_VERSION       tag to install (default: latest)
#   NINI_NO_MODIFY_PATH  set to anything to skip PATH instructions

set -euo pipefail

REPO="jinxumi/nini"
INSTALL_DIR="${NINI_INSTALL_DIR:-$HOME/.local/bin}"
BINARY_NAME="nini"
LEGACY_LINK="nini"  # for users who already have a `nini` on PATH

# --- helpers --------------------------------------------------------------

err() { printf "nini-install: %s\n" "$*" >&2; }
log() { printf "nini-install: %s\n" "$*"; }

have_cmd() { command -v "$1" >/dev/null 2>&1; }

# --- platform detection ---------------------------------------------------

detect_target() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"

  case "$os" in
    Linux)  os="unknown-linux-gnu" ;;
    Darwin) os="apple-darwin" ;;
    *) err "unsupported OS: $os (nini currently ships Linux + macOS binaries)"; exit 1 ;;
  esac

  case "$arch" in
    x86_64|amd64)  arch="x86_64" ;;
    aarch64|arm64) arch="aarch64" ;;
    *) err "unsupported architecture: $arch"; exit 1 ;;
  esac

  echo "${arch}-${os}"
}

# --- version resolution --------------------------------------------------

resolve_version() {
  if [ -n "${NINI_VERSION:-}" ]; then
    echo "$NINI_VERSION"
    return
  fi
  if have_cmd curl; then
    curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
      | sed -n 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1
  elif have_cmd wget; then
    wget -qO- "https://api.github.com/repos/${REPO}/releases/latest" \
      | sed -n 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1
  else
    err "need either curl or wget to fetch the latest version"
    exit 1
  fi
}

# --- download + verify ---------------------------------------------------

download_and_install() {
  local version="$1" target="$2"
  local archive="${BINARY_NAME}-${target}.tar.xz"
  local url="https://github.com/${REPO}/releases/download/${version}/${archive}"
  local tmpdir
  tmpdir="$(mktemp -d)"
  trap 'rm -rf "$tmpdir"' EXIT

  log "downloading ${archive} (${version})"
  if have_cmd curl; then
    curl -fsSL -o "${tmpdir}/${archive}" "$url"
  else
    wget -qO "${tmpdir}/${archive}" "$url"
  fi

  log "extracting"
  tar -xJf "${tmpdir}/${archive}" -C "$tmpdir"

  # Verify checksum if available (nini releases publish .sha256 sidecars)
  if [ -f "${tmpdir}/${archive}.sha256" ] && have_cmd sha256sum; then
    (cd "$tmpdir" && sha256sum -c "${archive}.sha256") >/dev/null
    log "checksum verified"
  fi

  mkdir -p "$INSTALL_DIR"
  install -m 0755 "${tmpdir}/${BINARY_NAME}" "${INSTALL_DIR}/${BINARY_NAME}"

  # Create a legacy `nini` link only if no existing `nini` is on PATH
  # (mirrors Pi's installer behavior for compatibility).
  if [ "$BINARY_NAME" != "$LEGACY_LINK" ] && ! command -v "$LEGACY_LINK" >/dev/null 2>&1; then
    ln -sf "${INSTALL_DIR}/${BINARY_NAME}" "${INSTALL_DIR}/${LEGACY_LINK}"
    log "created ${LEGACY_LINK} symlink"
  fi

  trap - EXIT
  rm -rf "$tmpdir"
}

# --- PATH -----------------------------------------------------------------

print_path_instructions() {
  if [ -n "${NINI_NO_MODIFY_PATH:-}" ]; then
    return
  fi
  case ":$PATH:" in
    *":${INSTALL_DIR}:"*) return ;;
  esac
  err "${INSTALL_DIR} is not on your PATH"
  err "  add this to your shell profile (~/.bashrc, ~/.zshrc, ...):"
  err "    export PATH=\"\$HOME/.local/bin:\$PATH\""
}

# --- main -----------------------------------------------------------------

main() {
  if [ -w "$INSTALL_DIR" ] || mkdir -p "$INSTALL_DIR" 2>/dev/null; then
    :
  else
    err "cannot write to $INSTALL_DIR (set NINI_INSTALL_DIR or run with sudo)"
    exit 1
  fi

  local target version
  target="$(detect_target)"
  version="$(resolve_version)"

  if [ -z "$version" ]; then
    err "could not resolve latest version"
    exit 1
  fi

  download_and_install "$version" "$target"

  log "installed nini ${version} to ${INSTALL_DIR}/${BINARY_NAME}"
  print_path_instructions
  log "run 'nini -p \"echo hello\"' to verify the install"
}

main "$@"
