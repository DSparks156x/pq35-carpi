#!/bin/bash
# First boot of a card finished by the image tool (docs/IMAGE-TOOL.md) or `cargo xtask image`
# (DEPLOY.md section 7). cloud-init's runcmd runs it once, as root, from the boot partition's
# carpi/ folder, which holds either:
#
#   setup.json     the image tool's answers; the release is downloaded here, or
#   setup.env      CARPI_USER, CARPI_HAT, CARPI_TZ, with the bundle (*.tar.zst) beside it.
#
# Waits for the network (packages, and the release), profiles the OS, applies the answers
# (os/options.py), installs the bundle (the kiosk session is profile.sh's). Success leaves
# /var/lib/carpi-firstboot.done and cloud-init reboots (power_state in user-data). The log is
# /var/log/carpi-firstboot.log, copied to carpi/firstboot.log on the boot partition. On a
# failure there's no reboot: ssh in, fix, and re-run this script.
set -uo pipefail

D=$(cd "$(dirname "$0")" && pwd)
LOG=/var/log/carpi-firstboot.log
DONE=/var/lib/carpi-firstboot.done
WORK=/var/lib/carpi-firstboot
exec > >(tee -a "$LOG") 2>&1

say() { echo "firstboot: $*"; }
finish() {
  say "$1"
  cp "$LOG" "$D/firstboot.log" 2>/dev/null
  sync
}
fail() { finish "FAILED: $*"; exit 1; }

# Release discovery uses HTTPS, so wait for the system clock to be synchronized first. Keep this
# self-contained: firstboot.sh is also copied to the boot partition and run independently.
wait_for_clock_sync() {
  local timeout=${CARPI_CLOCK_SYNC_TIMEOUT:-180} interval=${CARPI_CLOCK_SYNC_INTERVAL:-3} waited=0
  if timedatectl show -p NTPSynchronized --value 2>/dev/null | grep -qx yes; then
    say "system clock is already synchronized"
    return 0
  fi

  say "enabling network time and starting the time synchronization service"
  timedatectl set-ntp true 2>/dev/null || true
  # timedatectl normally starts the configured NTP client; explicitly start timesyncd where present.
  if systemctl list-unit-files systemd-timesyncd.service --no-legend 2>/dev/null | grep -q systemd-timesyncd; then
    systemctl start systemd-timesyncd.service 2>/dev/null || true
  fi

  say "waiting up to ${timeout}s for the system clock to synchronize"
  while [ "$waited" -lt "$timeout" ]; do
    sleep "$interval"
    waited=$((waited + interval))
    if timedatectl show -p NTPSynchronized --value 2>/dev/null | grep -qx yes; then
      say "system clock synchronized after ${waited}s"
      return 0
    fi
  done
  say "system clock did not synchronize after ${timeout}s"
  say "check network access and NTP service (timedatectl status), then rerun firstboot"
  return 1
}

say "start $(date -Is)"
[ -e $DONE ] && { finish "already done ($DONE)"; exit 0; }

SETUP=
if [ -f "$D/setup.json" ]; then
  SETUP=$D/setup.json
  # The answers this script needs itself.
  eval "$(python3 - "$SETUP" <<'EOF'
import json, shlex, sys
s = json.load(open(sys.argv[1]))
r = s.get("release") or {}
for k, v in {
    "CARPI_USER": s.get("user", "pi"),
    "CARPI_TZ": s.get("timezone", ""),
    "CARPI_REPO": r.get("repo", "DSparks156x/rnse-carpi"),
    "CARPI_CHANNEL": r.get("channel", "stable"),
    "CARPI_VERSION": r.get("version") or "",
}.items():
    print(f"{k}={shlex.quote(str(v))}")
EOF
)" || fail "can't read setup.json"
  CARPI_HAT=none
elif [ -f "$D/setup.env" ]; then
  # shellcheck source=/dev/null
  . "$D/setup.env"
  : "${CARPI_USER:?}" "${CARPI_HAT:=carpihat}" "${CARPI_TZ:=}"
else
  fail "no setup.json or setup.env in $D"
fi

say "waiting for the network"
for i in $(seq 120); do
  getent hosts deb.debian.org >/dev/null && getent hosts github.com >/dev/null && break
  [ "$i" = 120 ] && fail "no network after 6 min (Wi-Fi in network-config?)"
  sleep 3
done

# The bundle: on the card, or the release asked for.
mkdir -p $WORK
BUNDLE=$(ls "$D"/*.tar.zst 2>/dev/null | head -1)
if [ -z "$BUNDLE" ]; then
  wait_for_clock_sync || fail "clock is not synchronized; HTTPS release lookup cannot be trusted"
  say "finding the ${CARPI_VERSION:-$CARPI_CHANNEL} release of $CARPI_REPO"
  URL=$(python3 - "$CARPI_REPO" "$CARPI_CHANNEL" "$CARPI_VERSION" <<'EOF'
import json, sys, urllib.request
repo, channel, version = sys.argv[1:4]
req = urllib.request.Request(
    f"https://api.github.com/repos/{repo}/releases?per_page=50",
    headers={"Accept": "application/vnd.github+json", "User-Agent": "carpi-firstboot"},
)
releases = json.load(urllib.request.urlopen(req, timeout=60))
# GitHub's list order need not follow publication time.
releases.sort(key=lambda r: r.get("published_at") or r.get("created_at") or "", reverse=True)
def ok(r):
    if r["draft"]:
        return False
    if version:
        return r["tag_name"] == version
    # Beta takes stable releases too.
    return not r["draft"] and (channel == "beta" or not r["prerelease"])
for r in releases:
    if ok(r):
        for a in r["assets"]:
            if a["name"].endswith("-arm64.tar.zst"):
                print(a["browser_download_url"])
                sys.exit(0)
sys.exit("no matching release with an arm64 bundle")
EOF
) || fail "no release to install"
  BUNDLE=$WORK/$(basename "$URL")
  say "downloading $(basename "$URL")"
  curl -fL --retry 5 -o "$BUNDLE" "$URL" || fail "download failed"
fi

say "unpacking $(basename "$BUNDLE")"
rm -rf $WORK/b && mkdir -p $WORK/b
tar --zstd -C $WORK/b -xf "$BUNDLE" share bin/carpi-update || fail "can't unpack (zstd?)"
S=$WORK/b/share
OS=$S/os
# A bundle older than its os/ scripts: the ones on the card.
[ -f "$OS/profile.sh" ] || OS=$D

say "profile"
bash "$OS/profile.sh" --user "$CARPI_USER" --hat "$CARPI_HAT" ${CARPI_TZ:+--tz "$CARPI_TZ"} \
  || fail "profile.sh"

if [ -n "$SETUP" ]; then
  say "options"
  python3 "$OS/options.py" --setup "$SETUP" --share "$S" --user "$CARPI_USER" --phase pre \
    || fail "options.py"
fi

# The user manager, for install.sh's systemctl --user (linger starts it at boot; start it now).
systemctl start "user@$(id -u "$CARPI_USER").service" || fail "user manager for $CARPI_USER"

UPD=/tmp/carpi-update
install -m 0755 $WORK/b/bin/carpi-update $UPD || fail "no carpi-update in the bundle"
# A newly embedded firstboot.sh can download an older published bundle. Only pass the new
# flag when both that bundle's installer and updater support it; legacy releases keep their
# original installation behavior until a release with deferred startup is available.
DEFER_START=()
INSTALL_HELP=$(bash "$OS/install.sh" --help) || fail "install.sh help"
UPDATER_HELP=$("$UPD" apply --help) || fail "carpi-update apply help"
if grep -q -- '--defer-start' <<< "$INSTALL_HELP" && grep -q -- '--defer-start' <<< "$UPDATER_HELP"; then
  DEFER_START=(--defer-start)
else
  say "legacy bundle: deferred startup unavailable; using its original installer"
fi
say "install $(basename "$BUNDLE")"
bash "$OS/install.sh" --user "$CARPI_USER" --bundle "$BUNDLE" --updater $UPD "${DEFER_START[@]}" || fail "install.sh"

if [ -n "$SETUP" ]; then
  python3 "$OS/options.py" --setup "$SETUP" --share "$S" --user "$CARPI_USER" --phase post \
    || fail "options.py post"
fi

# LIVI and wayvnc run in the kiosk session profile.sh set up (carpi-session.service, in place of
# lightdm autologin: raspi-config's B4 would turn lightdm back on).

rm -rf $WORK/b
touch $DONE
finish "done $(date -Is); rebooting"
