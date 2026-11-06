#!/bin/sh
# Installs desl from its GitHub releases.
#
#   curl -fsSL https://raw.githubusercontent.com/iluxav/desl/main/install.sh | sh
#
# DESL_VERSION=v0.1.0      a release to install (default: the latest)
# DESL_INSTALL_DIR=/dir    where desl goes (default: ~/.local/bin)
# DESL_DOWNLOAD_BASE=url   where the release files are (a mirror, or a test)
#
# Never needs sudo. Says what it will do before it does it, and checks the download's SHA-256.

set -eu

REPO="iluxav/desl"
VERSION="${DESL_VERSION:-latest}"
INSTALL_DIR="${DESL_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
fail() {
    printf 'desl install: %s\n' "$*" >&2
    exit 1
}

case "$(uname -s)" in
    Linux) os="unknown-linux-gnu" ;;
    Darwin) os="apple-darwin" ;;
    *) fail "desl runs on Linux and macOS; this is $(uname -s). Build it from source with cargo instead." ;;
esac
case "$(uname -m)" in
    x86_64 | amd64) arch="x86_64" ;;
    aarch64 | arm64) arch="aarch64" ;;
    *) fail "there is no desl build for $(uname -m). Build it from source with cargo instead." ;;
esac
target="$arch-$os"
asset="desl-$target.tar.gz"
if [ -n "${DESL_DOWNLOAD_BASE:-}" ]; then
    base="$DESL_DOWNLOAD_BASE"
elif [ "$VERSION" = "latest" ]; then
    base="https://github.com/$REPO/releases/latest/download"
else
    base="https://github.com/$REPO/releases/download/$VERSION"
fi

if command -v curl >/dev/null 2>&1; then
    fetch() { curl -fsSL "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -q "$1" -O "$2"; }
else
    fail "this needs curl or wget to download desl."
fi
if command -v sha256sum >/dev/null 2>&1; then
    sha() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
    sha() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
    fail "this needs sha256sum or shasum to check the download."
fi

say "desl install will:"
say "  download $base/$asset"
say "  check its SHA-256 against $asset.sha256"
say "  put desl in $INSTALL_DIR"
say ""

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

fetch "$base/$asset" "$tmp/$asset" || fail "could not download $asset ($VERSION). Check the version at https://github.com/$REPO/releases."
fetch "$base/$asset.sha256" "$tmp/$asset.sha256" || fail "could not download $asset.sha256, so the download cannot be checked."
want="$(cut -d' ' -f1 <"$tmp/$asset.sha256")"
got="$(sha "$tmp/$asset")"
[ "$want" = "$got" ] || fail "the download's SHA-256 does not match the release's ($got, not $want). Nothing was installed; try again."

tar -xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$INSTALL_DIR"
# Replace in one step, so a running desl keeps its own file until it ends.
cp "$tmp/desl-$target/desl" "$INSTALL_DIR/desl.new"
chmod 755 "$INSTALL_DIR/desl.new"
mv -f "$INSTALL_DIR/desl.new" "$INSTALL_DIR/desl"

say "Installed $("$INSTALL_DIR/desl" --version 2>/dev/null || echo desl) in $INSTALL_DIR."
case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *) say "Note: $INSTALL_DIR is not on your PATH. Add it, for example: export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
esac
if "$INSTALL_DIR/desl" status --json 2>/dev/null | grep -q '"running":true'; then
    say "A desl daemon of the old build is running. \`desl daemon restart\` moves to the new one (its agents end; every thread is kept)."
fi
say ""
say "Run desl inside a repository. You need claude or codex on your PATH, logged in."
