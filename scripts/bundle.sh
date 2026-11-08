#!/bin/bash
# Assemble a carpi bundle from built outputs. Format: docs/BUNDLE.md.
#
#   scripts/bundle.sh [--version V] [--channel stable|beta|dev] [--out DIR]
#                     [--pq-bin DIR] [--teensy HEX [--teensy-version STR]] [--key FILE]
#
# Expects, relative to the repo root:
#   target/release/{carpid,carpi[,carpi-update]}   (--pq-bin to point elsewhere)
#   livi/out/bin/{LIVI,livi-core,livi-helperd,livi-gst-host,livi-compositor}   (cargo xtask build-native)
#   sdlink/bin/{rnv3enc,avlink,wlcap}   (make in sdlink)
#   sdlink/pi/run/*, deploy/*
# Teensy firmware is optional and only included with --teensy (CI doesn't build PlatformIO yet).
# --key signs manifest.json with minisign; the key must be unencrypted (minisign -G -W).
# Needs: git, python3, sha256sum, tar, zstd, minisign (only with --key).
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
die() { echo "bundle: $*" >&2; exit 1; }

VERSION= CHANNEL=dev OUT=$ROOT/dist PQBIN=$ROOT/target/release TEENSY= TEENSY_VERSION= KEY=
while [ $# -gt 0 ]; do
  case $1 in
    --version) VERSION=$2; shift 2 ;;
    --channel) CHANNEL=$2; shift 2 ;;
    --out) OUT=$2; shift 2 ;;
    --pq-bin) PQBIN=$2; shift 2 ;;
    --teensy) TEENSY=$2; shift 2 ;;
    --teensy-version) TEENSY_VERSION=$2; shift 2 ;;
    --key) KEY=$2; shift 2 ;;
    -h|--help) sed -n '2,14p' "$0"; exit 0 ;;
    *) die "unknown argument: $1" ;;
  esac
done

case $CHANNEL in stable|beta|dev) ;; *) die "channel must be stable, beta or dev" ;; esac
[ -n "$VERSION" ] || VERSION=dev-$(git -C "$ROOT" rev-parse --short=7 HEAD)
[[ $VERSION =~ ^[A-Za-z0-9._-]+$ ]] || die "version '$VERSION' has characters outside [A-Za-z0-9._-]"

commit() { git -C "$1" rev-parse HEAD 2>/dev/null || die "$1: not a git checkout (submodules fetched?)"; }
PQ_COMMIT=$(commit "$ROOT")
LIVI_COMMIT=$(commit "$ROOT/livi")
SDLINK_COMMIT=$(commit "$ROOT/sdlink")
EPOCH=$(git -C "$ROOT" log -1 --format=%ct HEAD)
DATE=$(TZ=UTC0 git -C "$ROOT" log -1 --format=%cd --date=format-local:%Y-%m-%dT%H:%M:%SZ HEAD)
MIN_PROFILE=$(sed -n 's/^PROFILE_VERSION=\([0-9][0-9]*\)$/\1/p' "$ROOT/os/profile.sh")
[ -n "$MIN_PROFILE" ] || die "no PROFILE_VERSION in os/profile.sh"

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
STAGE=$TMP/stage LIST=$TMP/list
mkdir "$STAGE"

# put SRC DEST MODE: copy into the stage; binaries must be aarch64 ELF (e_machine 0xB7).
put() {
  [ -f "$1" ] || die "missing $1"
  if [ "$3" = 0755 ] && [[ $2 == bin/* ]]; then
    [ "$(od -An -tx1 -j18 -N2 "$1" | tr -d ' \n')" = b700 ] || die "$1 is not an aarch64 ELF"
  fi
  install -D -m "$3" "$1" "$STAGE/$2"
}

put "$PQBIN/carpid" bin/carpid 0755
put "$PQBIN/carpi" bin/carpi 0755
[ -f "$PQBIN/carpi-update" ] && put "$PQBIN/carpi-update" bin/carpi-update 0755

for b in LIVI livi-core livi-helperd livi-gst-host livi-compositor; do
  put "$ROOT/livi/out/bin/$b" "bin/$b" 0755
done
for b in rnv3enc avlink wlcap; do
  put "$ROOT/sdlink/bin/$b" "bin/$b" 0755
done
for f in "$ROOT"/sdlink/pi/run/*; do
  n=$(basename "$f")
  case $n in *.sh) m=0755 ;; *) m=0644 ;; esac
  put "$f" "share/sdlink/run/$n" $m
done

put "$ROOT/deploy/carpid.toml" share/config/carpid.toml 0644
put "$ROOT/deploy/carpid.service" share/systemd/user/carpid.service 0644
put "$ROOT/deploy/99-carpi-uinput.rules" share/udev/99-carpi-uinput.rules 0644

FIRMWARE=()
if [ -n "$TEENSY" ]; then
  if [ -z "$TEENSY_VERSION" ]; then
    TEENSY_VERSION=$(sed -n 's/^#define FW_VERSION "\(.*\)"$/\1/p' "$ROOT/sdlink/teensy/sdemu/config.h")
    [ -n "$TEENSY_VERSION" ] || die "no FW_VERSION in sdlink/teensy/sdemu/config.h; pass --teensy-version"
  fi
  put "$TEENSY" firmware/teensy/sdemu.hex 0644
  FIRMWARE=(teensy firmware/teensy/sdemu.hex "$TEENSY_VERSION")
fi

# Manifest. Paths are sorted bytewise (LC_ALL=C) so the order is the same everywhere.
(cd "$STAGE" && find . -type f | sed 's|^\./||' | LC_ALL=C sort) > "$LIST"
VERSION=$VERSION CHANNEL=$CHANNEL DATE=$DATE MIN_PROFILE=$MIN_PROFILE \
PQ_COMMIT=$PQ_COMMIT LIVI_COMMIT=$LIVI_COMMIT SDLINK_COMMIT=$SDLINK_COMMIT \
python3 -I - "$STAGE" "$LIST" "${FIRMWARE[@]}" <<'PY'
import hashlib, json, os, sys
stage, listfile, fw = sys.argv[1], sys.argv[2], sys.argv[3:]
e = os.environ
files = []
for path in open(listfile).read().splitlines():
    if not path:
        continue
    full = os.path.join(stage, path)
    h = hashlib.sha256()
    with open(full, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    files.append({"path": path, "sha256": h.hexdigest(), "size": os.path.getsize(full),
                  "mode": "%04o" % (os.stat(full).st_mode & 0o777)})
manifest = {
    "format_version": 1,
    "version": e["VERSION"],
    "channel": e["CHANNEL"],
    "arch": "arm64",
    "date": e["DATE"],
    "commits": {"pq35-carpi": e["PQ_COMMIT"], "livi": e["LIVI_COMMIT"], "sdlink": e["SDLINK_COMMIT"]},
    "min_os_profile": int(e["MIN_PROFILE"]),
    "files": files,
    "firmware": [{"target": fw[i], "path": fw[i + 1], "version": fw[i + 2]} for i in range(0, len(fw), 3)],
}
with open(os.path.join(stage, "manifest.json"), "w", newline="\n") as f:
    json.dump(manifest, f, indent=2)
    f.write("\n")
PY

HEAD_FILES=(manifest.json)
if [ -n "$KEY" ]; then
  minisign -S -s "$KEY" -m "$STAGE/manifest.json" -x "$STAGE/manifest.json.minisig" \
    -t "carpi $VERSION $CHANNEL" </dev/null >/dev/null
  HEAD_FILES+=(manifest.json.minisig)
fi

mkdir -p "$OUT"
NAME=carpi-$VERSION-arm64
TAR=$OUT/$NAME.tar.zst
# Manifest first, then files in manifest order; fixed owner and mtime so the tar depends only on its inputs.
cat <(printf '%s\n' "${HEAD_FILES[@]}") "$LIST" |
  tar -C "$STAGE" --no-recursion --owner=0 --group=0 --numeric-owner --mtime="@$EPOCH" \
      --format=gnu -cf - -T - |
  zstd -q -19 -T0 -f -o "$TAR"
for f in "${HEAD_FILES[@]}"; do cp "$STAGE/$f" "$OUT/$f"; done
[ -n "$KEY" ] || rm -f "$OUT/manifest.json.minisig"

echo "bundle: $TAR ($(du -h "$TAR" | cut -f1), $(wc -l < "$LIST") files${KEY:+, signed})"
