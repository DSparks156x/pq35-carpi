#!/bin/bash
# First install of a bundle on a profiled Pi (os/profile.sh first). DEPLOY.md section 7.
#
#   sudo os/install.sh --bundle FILE [--updater PATH] [--user NAME]
#
# Sets up what a bundle can't do for itself, then installs it with carpi-update (the same apply a
# release takes): the default carpid.toml, the user units, the A/V-over-SD chain's folder and its
# governor unit, Move to NVMe's root unit and script, wayvnc's config. --updater: a carpi-update binary, for a device with none
# installed yet (`cargo xtask image` puts one next to the bundle). Idempotent: anything already
# there is left alone, so it never overwrites user config or a unit someone edited.
set -euo pipefail

BUNDLE=
UPDATER=
USERNAME=${SUDO_USER:-}
while [ $# -gt 0 ]; do
  case $1 in
    --bundle) BUNDLE=$2; shift 2 ;;
    --updater) UPDATER=$2; shift 2 ;;
    --user) USERNAME=$2; shift 2 ;;
    -h|--help) sed -n '2,10p' "$0"; exit 0 ;;
    *) echo "install: unknown argument: $1" >&2; exit 1 ;;
  esac
done

say() { echo "install: $*"; }
die() { echo "install: $*" >&2; exit 1; }

[ "$(id -u)" = 0 ] || die "run as root (sudo)"
[ -n "$BUNDLE" ] && [ -f "$BUNDLE" ] || die "no bundle: pass --bundle FILE"
[ -n "$USERNAME" ] || die "no user: run with sudo from the car user, or pass --user"
[ -f /etc/carpi-profile ] || die "not profiled: run os/profile.sh first"
command -v zstd >/dev/null || die "zstd isn't installed (os/profile.sh installs it)"
UID_=$(id -u "$USERNAME")
GROUP=$(id -gn "$USERNAME")
HOME_=$(getent passwd "$USERNAME" | cut -d: -f6)
CUR=/opt/carpi/current
UNITS=$HOME_/.config/systemd/user
[ -n "$UPDATER" ] || UPDATER=$CUR/bin/carpi-update
[ -x "$UPDATER" ] || die "no carpi-update at $UPDATER: pass --updater"

# as_user CMD...: run as the car user in their systemd session (linger keeps it up).
as_user() {
  sudo -u "$USERNAME" env XDG_RUNTIME_DIR=/run/user/$UID_ \
    DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$UID_/bus HOME="$HOME_" "$@"
}
# put_missing SRC DST [MODE]: copy as the car user's file, only if DST doesn't exist.
put_missing() {
  [ -f "$1" ] || return 0
  [ -e "$2" ] && return 0
  say "writing $2"
  install -D -o "$USERNAME" -g "$GROUP" -m "${3:-0644}" "$1" "$2"
}

for _ in $(seq 30); do [ -S /run/user/$UID_/bus ] && break; sleep 1; done
[ -S /run/user/$UID_/bus ] || die "no user session bus for $USERNAME (is linger on?)"

# 1. The bundle's share/: config and units go in before the apply, whose restart starts them.
SHARE=$(mktemp -d)
trap 'rm -rf "$SHARE"' EXIT
tar --zstd -C "$SHARE" -xf "$BUNDLE" share
S=$SHARE/share
put_missing "$S/config/carpid.toml" "$HOME_/.config/carpi/carpid.toml"
for u in "$S"/systemd/user/*.service; do put_missing "$u" "$UNITS/$(basename "$u")"; done
put_missing "$S/sdlink/run/avsd-chain.service" "$UNITS/avsd-chain.service"

# 2. A/V over SD: ~/avsd/current -> the release, the env file if there's none, log folders.
AV=$HOME_/avsd
install -d -o "$USERNAME" -g "$GROUP" "$AV" "$AV/current" "$AV/logs" "$AV/clips"
for l in "bin:$CUR/bin" "chain:$CUR/share/sdlink/run"; do
  n=${l%%:*}
  [ -e "$AV/current/$n" ] || [ -L "$AV/current/$n" ] && continue
  say "linking $AV/current/$n"
  ln -s "${l#*:}" "$AV/current/$n"
  chown -h "$USERNAME:$GROUP" "$AV/current/$n"
done
put_missing "$S/sdlink/run/avsd.env" "$AV/avsd.env"
if [ ! -e /etc/systemd/system/avsd-governor.service ] && [ -f "$S/sdlink/run/avsd-governor.service" ]; then
  say "installing avsd-governor.service"
  install -m 0644 "$S/sdlink/run/avsd-governor.service" /etc/systemd/system/
  systemctl daemon-reload
  systemctl enable avsd-governor.service
fi

# 2b. Move to NVMe: the root unit carpid may start (polkit rule from os/profile.sh) and its script.
#     The script goes where only root can change it, since root runs it.
if [ -f "$S/os/move-nvme.sh" ] && ! cmp -s "$S/os/move-nvme.sh" /usr/local/lib/carpi/move-nvme.sh; then
  say "installing /usr/local/lib/carpi/move-nvme.sh"
  install -D -o root -g root -m 0755 "$S/os/move-nvme.sh" /usr/local/lib/carpi/move-nvme.sh
fi
if [ ! -e /etc/systemd/system/carpi-move-nvme.service ] && [ -f "$S/systemd/system/carpi-move-nvme.service" ]; then
  say "installing carpi-move-nvme.service"
  install -m 0644 "$S/systemd/system/carpi-move-nvme.service" /etc/systemd/system/
  systemctl daemon-reload
fi

# 3. wayvnc on the desktop session: PAM login, RSA-AES with its own key.
VNC=$HOME_/.config/wayvnc
if command -v wayvnc >/dev/null && [ ! -e "$VNC/config" ]; then
  say "writing $VNC/config"
  install -d -o "$USERNAME" -g "$GROUP" -m 0700 "$VNC"
  as_user openssl genrsa -traditional -out "$VNC/rsa_key.pem" 2048 2>/dev/null
  as_user chmod 600 "$VNC/rsa_key.pem"
  printf 'address=0.0.0.0\nport=5900\nenable_auth=true\nenable_pam=true\nrsa_private_key_file=%s\n' \
    "$VNC/rsa_key.pem" > "$VNC/config"
  chown "$USERNAME:$GROUP" "$VNC/config"
fi

# 4. Enable the units (before the apply, so its restart and the next boot both start them).
as_user systemctl --user daemon-reload
for u in carpid.service livi.service avsd-chain.service wayvnc.service; do
  [ -e "$UNITS/$u" ] || continue
  as_user systemctl --user is-enabled -q "$u" 2>/dev/null && continue
  say "enabling $u"
  as_user systemctl --user enable "$u"
done

# 5. The bundle. A first install has nothing to roll back to: a failed health check (no desktop
#    session yet, say) leaves it installed, and the next boot brings everything up.
install -d -o "$USERNAME" -g "$GROUP" /opt/carpi
say "applying $(basename "$BUNDLE")"
as_user "$UPDATER" apply --file "$BUNDLE" --allow-unsigned 2>&1 | while IFS= read -r l; do say "  $l"; done || say "apply reported a problem; see above"
[ -L $CUR ] || die "carpi-update didn't install anything"
say "done: $(readlink $CUR)"
