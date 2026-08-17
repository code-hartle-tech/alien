#!/usr/bin/env bash
# Build a reviewed source release from the current committed public surface.
#
#   ./mkrelease.sh                 source tarball only
#   ./mkrelease.sh --vendor        source tarball with Cargo dependencies
#   ./mkrelease.sh --native        add native binaries for this host
#   ./mkrelease.sh --static        add x86_64 Linux musl CLI/TUI/daemon
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
DIST=${ALIEN_DIST:-"$ROOT/dist"}
VENDOR=0
NATIVE=0
STATIC=0

for arg in "$@"; do
  case "$arg" in
    --vendor) VENDOR=1 ;;
    --native) NATIVE=1 ;;
    --static) STATIC=1 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

cd "$ROOT"
PUBLIC_ITEMS=(.github .gitattributes assets code packaging docs README.md CONTRIBUTING.md SECURITY.md LICENSE NOTICE flake.nix)
if [ -n "$(git status --porcelain --untracked-files=all -- "${PUBLIC_ITEMS[@]}")" ]; then
  echo "public surface is dirty; commit the reviewed snapshot first" >&2
  git status --short --untracked-files=all -- "${PUBLIC_ITEMS[@]}" >&2
  exit 1
fi

VERSION=$(git show HEAD:code/Cargo.toml | awk -F'"' '/^version = / { print $2; exit }')
NAME="alien-$VERSION"
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
mkdir -p "$STAGE/$NAME" "$DIST"

echo "==> staging committed $NAME"
git archive --format=tar HEAD -- "${PUBLIC_ITEMS[@]}" | tar -x -C "$STAGE/$NAME"
rm -f "$STAGE/$NAME/code/tools/mirror-to-github.sh"

if [ "$VENDOR" = 1 ]; then
  echo "==> vendoring locked Cargo dependencies"
  (
    cd "$STAGE/$NAME/code"
    mkdir -p .cargo
    cargo vendor --locked ../vendor > .cargo/config.toml
  )
fi

echo "==> guarding public source"
PRIVATE_PATTERNS='/Users/[a-z]+|/home/[a-z]+/|192\.168\.|10\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}|172\.(1[6-9]|2[0-9]|3[01])\.'
if grep -rIn -E "$PRIVATE_PATTERNS" "$STAGE/$NAME" --exclude=Cargo.lock --exclude-dir=vendor 2>/dev/null; then
  echo "refusing to package private infrastructure references" >&2
  exit 1
fi
forbidden=$(find "$STAGE/$NAME" \
  -path "$STAGE/$NAME/vendor" -prune -o \
  \( -name '*.exe' -o -name '*.dll' -o -name '*.il' -o -name '*.cs' \) \
  -print -quit)
if [ -n "$forbidden" ]; then
  echo "$forbidden" >&2
  echo "refusing to package first-party binary or decompiler artifacts" >&2
  exit 1
fi
metadata=$(find "$STAGE/$NAME" \( -name '._*' -o -name '.DS_Store' \) -print -quit)
if [ -n "$metadata" ]; then
  echo "$metadata" >&2
  echo "refusing to package filesystem metadata artifacts" >&2
  exit 1
fi

echo "==> source tarball"
tar_metadata_args=()
tar_version=$(tar --version 2>&1 || true)
if [[ "$tar_version" == *"bsdtar"* ]]; then
  tar_metadata_args+=(--no-xattrs --no-acls --no-fflags --no-mac-metadata)
elif [[ "$tar_version" == *"GNU tar"* ]]; then
  tar_metadata_args+=(--no-xattrs --no-acls --no-selinux)
fi
if [ "${#tar_metadata_args[@]}" -gt 0 ]; then
  COPYFILE_DISABLE=1 tar "${tar_metadata_args[@]}" \
    -C "$STAGE" -czf "$DIST/$NAME.tar.gz" "$NAME"
else
  COPYFILE_DISABLE=1 tar -C "$STAGE" -czf "$DIST/$NAME.tar.gz" "$NAME"
fi
(
  cd "$DIST"
  shasum -a 256 "$NAME.tar.gz" > "$NAME.tar.gz.sha256"
)

if [ "$NATIVE" = 1 ]; then
  HOST_OS=$(uname -s | tr '[:upper:]' '[:lower:]')
  HOST_ARCH=$(uname -m)
  echo "==> native $HOST_OS-$HOST_ARCH binaries"
  (
    cd "$STAGE/$NAME/code"
    cargo build --release --workspace --locked
  )
  for binary in alien alien-tui alien-gui alien-daemon alien-cooling; do
    install -Dm755 "$STAGE/$NAME/code/target/release/$binary" \
      "$DIST/$binary-$VERSION-$HOST_OS-$HOST_ARCH"
  done
fi

if [ "$STATIC" = 1 ]; then
  if [ "$(uname -s)" != Linux ] || [ "$(uname -m)" != x86_64 ]; then
    echo "--static requires an x86_64 Linux builder" >&2
    exit 1
  fi
  rustup target add x86_64-unknown-linux-musl
  (
    cd "$STAGE/$NAME/code"
    cargo build --release --locked --target x86_64-unknown-linux-musl \
      -p alien-cli -p alien-tui -p alien-daemon -p alien-cooling
  )
  for binary in alien alien-tui alien-daemon alien-cooling; do
    install -Dm755 "$STAGE/$NAME/code/target/x86_64-unknown-linux-musl/release/$binary" \
      "$DIST/$binary-$VERSION-linux-x86_64-static"
  done
fi

if [ "$NATIVE" = 1 ] || [ "$STATIC" = 1 ]; then
  # Binary artifacts travel with every applicable license and attribution.
  install -Dm644 "$STAGE/$NAME/LICENSE" "$DIST/LICENSE-$VERSION.txt"
  install -Dm644 "$STAGE/$NAME/NOTICE" "$DIST/NOTICE-$VERSION.txt"
  install -Dm644 "$STAGE/$NAME/code/alien-gui/assets/fonts/Archivo-OFL.txt" \
    "$DIST/Archivo-OFL-$VERSION.txt"
  install -Dm644 "$STAGE/$NAME/code/alien-gui/assets/fonts/IBMPlex-OFL.txt" \
    "$DIST/IBMPlex-OFL-$VERSION.txt"
fi

(
  cd "$DIST"
  artifacts=("$NAME.tar.gz")
  for file in alien-*"-$VERSION-"* "LICENSE-$VERSION.txt" \
    "NOTICE-$VERSION.txt" "Archivo-OFL-$VERSION.txt" \
    "IBMPlex-OFL-$VERSION.txt"; do
    [ -f "$file" ] && artifacts+=("$file")
  done
  shasum -a 256 "${artifacts[@]}" > "SHA256SUMS-$VERSION"
)

echo "==> release artifacts"
find "$DIST" -maxdepth 1 -type f -name "*$VERSION*" -print | sort
