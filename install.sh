#!/usr/bin/env sh
# context-brain installer
#
# Usage:
#   curl -sSL https://raw.githubusercontent.com/yarik1987k/contexa-brain-mcp/main/install.sh | sh
#
# Optional environment variables:
#   CB_VERSION   Specific version to install (default: latest)
#   CB_PREFIX    Install location (default: /usr/local/bin, falls back to ~/.local/bin if not writable)

set -eu

REPO="yarik1987k/contexa-brain-mcp"
BIN_NAME="context-brain"

err() { printf '\033[31merror:\033[0m %s\n' "$1" >&2; exit 1; }
log() { printf '\033[36m==>\033[0m %s\n' "$1"; }

# ── Detect OS + arch ────────────────────────────────────────────────────
uname_s=$(uname -s)
uname_m=$(uname -m)

case "$uname_s" in
  Darwin) os=apple-darwin ;;
  Linux)  os=unknown-linux-gnu ;;
  *) err "unsupported OS: $uname_s (only macOS and Linux are supported today)" ;;
esac

case "$uname_m" in
  arm64|aarch64) arch=aarch64 ;;
  x86_64|amd64)  arch=x86_64 ;;
  *) err "unsupported arch: $uname_m" ;;
esac

# Linux ARM is not yet built — fail early with a clear message.
if [ "$os" = "unknown-linux-gnu" ] && [ "$arch" = "aarch64" ]; then
  err "Linux aarch64 is not yet published. Build from source: git clone https://github.com/$REPO && cd contexa-brain-mcp && cargo build --release"
fi

target="${arch}-${os}"
log "Detected target: $target"

# ── Resolve version ─────────────────────────────────────────────────────
version="${CB_VERSION:-}"
if [ -z "$version" ]; then
  log "Resolving latest release..."
  version=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
    | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -1)
  [ -n "$version" ] || err "could not determine latest version. Set CB_VERSION=v0.1.0 manually."
fi
log "Installing version: $version"

# ── Pick install prefix ─────────────────────────────────────────────────
prefix="${CB_PREFIX:-/usr/local/bin}"
if [ ! -w "$(dirname "$prefix" 2>/dev/null || echo "$prefix")" ] && [ ! -w "$prefix" ]; then
  prefix="$HOME/.local/bin"
  log "/usr/local/bin not writable, using $prefix"
fi
mkdir -p "$prefix"

# ── Download + verify ───────────────────────────────────────────────────
tmpdir=$(mktemp -d 2>/dev/null || mktemp -d -t cb)
trap 'rm -rf "$tmpdir"' EXIT

tarball="context-brain-${target}.tar.gz"
url="https://github.com/$REPO/releases/download/${version}/${tarball}"
sha_url="${url}.sha256"

log "Downloading ${tarball}..."
curl -fSL "$url" -o "$tmpdir/$tarball"

log "Verifying checksum..."
curl -fSL "$sha_url" -o "$tmpdir/$tarball.sha256" || err "checksum file missing"
cd "$tmpdir"
# The .sha256 file contains "<hash>  <filename>"; awk out just the hash so
# the shasum check works regardless of pathname differences.
expected_sha=$(awk '{print $1}' "$tarball.sha256")
actual_sha=$(shasum -a 256 "$tarball" | awk '{print $1}')
[ "$expected_sha" = "$actual_sha" ] || err "checksum mismatch (expected $expected_sha, got $actual_sha)"
cd - >/dev/null

# ── Extract + install ───────────────────────────────────────────────────
log "Extracting..."
tar xzf "$tmpdir/$tarball" -C "$tmpdir"

log "Installing to $prefix/$BIN_NAME"
install -m 0755 "$tmpdir/$BIN_NAME" "$prefix/$BIN_NAME"

# ── Verify ─────────────────────────────────────────────────────────────
if ! command -v "$BIN_NAME" >/dev/null 2>&1; then
  echo
  echo "Installed to $prefix/$BIN_NAME but it is not on your PATH."
  echo "Add this to your shell profile:"
  echo "  export PATH=\"$prefix:\$PATH\""
fi

log "Done."
echo
echo "Add to .cursor/mcp.json or .mcp.json:"
echo '  {'
echo '    "mcpServers": {'
echo '      "context-brain": {'
echo "        \"command\": \"$prefix/$BIN_NAME\","
echo '        "args": ["serve", "--project", "."]'
echo '      }'
echo '    }'
echo '  }'
