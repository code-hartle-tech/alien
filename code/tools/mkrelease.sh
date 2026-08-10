#!/usr/bin/env bash
# Build the release artefacts: source tarball and static single binaries.
#
#   ./mkrelease.sh              source tar.gz + native binaries
#   ./mkrelease.sh --vendor     tarball with crates vendored (offline builds)
#   ./mkrelease.sh --static     musl binaries that run on any glibc vintage
#
# Output lands in dist/.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
CODE="$ROOT/code"
DIST="$ROOT/dist"
VERSION=$(grep -m1 '^version' "$CODE/Cargo.toml" | cut -d'"' -f2)
NAME="alien-$VERSION"

VENDOR=0
STATIC=0
for a in "$@"; do
  case "$a" in
    --vendor) VENDOR=1 ;;
    --static) STATIC=1 ;;
    *) echo "unknown option: $a" >&2; exit 2 ;;
  esac
done

mkdir -p "$DIST"
rm -rf "${DIST:?}/$NAME"
mkdir -p "$DIST/$NAME"

echo "==> staging $NAME"
# Explicit list rather than "everything except": a published tarball is exactly
# the wrong place to discover that an exclude pattern missed something. In
# particular research/ holds decompilation output and must never ship, and
# wiki/ is the private realm.
for item in code packaging docs README.md LICENSE NOTICE flake.nix; do
  [ -e "$ROOT/$item" ] || continue
  cp -R "$ROOT/$item" "$DIST/$NAME/"
done
rm -rf "$DIST/$NAME/code/target"

if [ "$VENDOR" = 1 ]; then
  echo "==> vendoring crates (offline builds: Debian buildds, flathub, snapcraft)"
  ( cd "$DIST/$NAME/code"
    mkdir -p .cargo
    cargo vendor ../vendor > .cargo/config.toml )
fi

echo "==> guarding against leaks"
# The tarball is published. A grep is not a security boundary, but it does
# catch the specific mistakes this project could plausibly make: a workstation
# path, a LAN address, or a machine hostname pasted into a comment.
LEAKS=0
if grep -rIn -E '/Users/[a-z]+|/home/[a-z]+/Projects|192\.168\.[0-9]+\.[0-9]+|10\.[0-9]+\.[0-9]+\.[0-9]+' \
     "$DIST/$NAME" --exclude-dir=vendor 2>/dev/null | grep -v 'index.crates.io'; then
  echo "!! the lines above look like workstation paths or LAN addresses" >&2
  LEAKS=1
fi
if [ "$LEAKS" = 1 ]; then
  echo "refusing to build a tarball with those in it" >&2
  exit 1
fi

echo "==> source tarball"
tar -C "$DIST" -czf "$DIST/$NAME.tar.gz" "$NAME"
( cd "$DIST" && sha256sum "$NAME.tar.gz" > "$NAME.tar.gz.sha256" )

echo "==> binaries"
if [ "$STATIC" = 1 ]; then
  # musl: one binary that runs on any distribution regardless of glibc
  # version. The GUI is excluded on purpose — it dlopens libGL and
  # libwayland-client, so a "static" GUI would still fail on a machine without
  # them and would only be misleading.
  rustup target add x86_64-unknown-linux-musl 2>/dev/null || true
  ( cd "$CODE" && cargo build --release --target x86_64-unknown-linux-musl \
      -p alien-cli -p alien-tui -p alien-daemon )
  BIN="$CODE/target/x86_64-unknown-linux-musl/release"
  for b in alien alien-tui alien-daemon; do
    install -Dm755 "$BIN/$b" "$DIST/$b-$VERSION-x86_64-static"
  done
else
  ( cd "$CODE" && cargo build --release --workspace )
  for b in alien alien-tui alien-gui alien-daemon; do
    install -Dm755 "$CODE/target/release/$b" "$DIST/$b-$VERSION-x86_64"
  done
fi

( cd "$DIST" && sha256sum alien*-"$VERSION"-x86_64* > "SHA256SUMS-$VERSION" 2>/dev/null || true )

echo
echo "dist/:"
ls -1 "$DIST" | sed 's/^/  /'
