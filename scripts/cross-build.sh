#!/bin/bash
# Build everything for the Pi on this machine (x86_64 + Docker), then bundle.
#
#   scripts/cross-build.sh [--only pq|livi|sdlink] [--no-bundle]
#
# Output: dist/carpi-dev-<sha>-arm64.tar.zst (the same bundle CI makes, unsigned).
# Deploy it:  cargo xtask deploy --file dist/<bundle>
#
# Caches live in Docker volumes (carpi-cargo, carpi-rustup, carpi-target), so
# only changed crates rebuild. `docker volume rm carpi-target` forces a clean build.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
ONLY= BUNDLE=1
while [ $# -gt 0 ]; do
  case $1 in
    --only) ONLY=$2; shift 2 ;;
    --no-bundle) BUNDLE=0; shift ;;
    -h|--help) sed -n '2,10p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 1 ;;
  esac
done

[ -f "$ROOT/livi/native/Cargo.toml" ] || { echo "livi/ submodule missing: git submodule update --init" >&2; exit 1; }
[ -f "$ROOT/sdlink/Makefile" ] || { echo "sdlink/ submodule missing: git submodule update --init" >&2; exit 1; }

image=carpi-cross
docker build -q -t "$image" -f "$ROOT/scripts/cross/Containerfile" "$ROOT/scripts/cross" >/dev/null

# Git Bash on Windows rewrites /paths in arguments; docker wants a Windows path for the bind mount.
src=$ROOT
if command -v cygpath >/dev/null; then src=$(cygpath -w "$ROOT"); export MSYS_NO_PATHCONV=1; fi

docker run --rm \
  -v "$src:/src" \
  -v carpi-cargo:/cargo/registry \
  -v carpi-git:/cargo/git \
  -v carpi-rustup:/rustup \
  -v carpi-target:/target \
  -e ONLY="$ONLY" -e BUNDLE="$BUNDLE" \
  -w /src \
  "$image" bash -c '
    set -euo pipefail
    T=aarch64-unknown-linux-gnu
    want() { [ -z "$ONLY" ] || [ "$ONLY" = "$1" ]; }

    if want pq; then
      echo "== rnse-carpi"
      rustup toolchain install stable --profile minimal >/dev/null
      rustup target add --toolchain stable $T >/dev/null
      CARGO_TARGET_DIR=/target/pq cargo +stable build --release --locked --target $T \
        -p carpid -p carpi-cli -p carpi-update
    fi

    if want livi; then
      echo "== LIVI"
      cd /src/livi
      rustup toolchain install >/dev/null   # the pinned one in livi/rust-toolchain.toml
      rustup target add $T >/dev/null
      cd native
      CARGO_TARGET_DIR=/target/livi cargo xtask build-native --arch arm64
      cd /src
    fi

    if want sdlink; then
      echo "== sdlink"
      make -C /src/sdlink -s CC=aarch64-linux-gnu-gcc ENCARCH=-mcpu=cortex-a76 clean all
    fi

    if [ "$BUNDLE" = 1 ]; then
      echo "== bundle"
      bash /src/scripts/bundle.sh --pq-bin /target/pq/$T/release --out /src/dist
    fi
  '
