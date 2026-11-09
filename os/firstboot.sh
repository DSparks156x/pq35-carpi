#!/bin/bash
# First boot of a card made with `cargo xtask image` (DEPLOY.md section 7). cloud-init's runcmd
# runs it once, as root, from the boot partition's carpi/ folder:
#
#   setup.env      CARPI_USER, CARPI_HAT, CARPI_TZ
#   profile.sh     os/profile.sh, from the bundle
#   install.sh     os/install.sh, from the bundle
#   carpi-update   from the bundle, for the first install
#   *.tar.zst      the bundle
#
# Waits for the network (the profile installs packages), profiles the OS, installs the bundle,
# turns on desktop autologin. Success leaves /var/lib/carpi-firstboot.done, and cloud-init
# reboots (power_state in user-data). The log is /var/log/carpi-firstboot.log, copied to
# carpi/firstboot.log on the boot partition so it can be read from a PC. On a failure there's no
# reboot: ssh in, fix, and re-run this script.
set -uo pipefail

D=$(cd "$(dirname "$0")" && pwd)
LOG=/var/log/carpi-firstboot.log
DONE=/var/lib/carpi-firstboot.done
exec > >(tee -a "$LOG") 2>&1

say() { echo "firstboot: $*"; }
finish() {
  say "$1"
  cp "$LOG" "$D/firstboot.log" 2>/dev/null
  sync
}
fail() { finish "FAILED: $*"; exit 1; }

say "start $(date -Is)"
[ -e $DONE ] && { finish "already done ($DONE)"; exit 0; }
[ -f "$D/setup.env" ] || fail "no $D/setup.env"
# shellcheck source=/dev/null
. "$D/setup.env"
: "${CARPI_USER:?}" "${CARPI_HAT:=carpihat}" "${CARPI_TZ:=}"
BUNDLE=$(ls "$D"/*.tar.zst 2>/dev/null | head -1)
[ -n "$BUNDLE" ] || fail "no bundle (*.tar.zst) in $D"

# The network: the profile's packages come from apt.
say "waiting for the network"
for i in $(seq 120); do
  getent hosts deb.debian.org >/dev/null && getent hosts archive.raspberrypi.com >/dev/null && break
  [ "$i" = 120 ] && fail "no network after 6 min (Wi-Fi in network-config?)"
  sleep 3
done

say "profile"
bash "$D/profile.sh" --user "$CARPI_USER" --hat "$CARPI_HAT" ${CARPI_TZ:+--tz "$CARPI_TZ"} \
  || fail "profile.sh"

# The user manager, for install.sh's systemctl --user (linger starts it at boot; start it now).
systemctl start "user@$(id -u "$CARPI_USER").service" || fail "user manager for $CARPI_USER"

UPD=/tmp/carpi-update
install -m 0755 "$D/carpi-update" $UPD || fail "no carpi-update in $D"
say "install $(basename "$BUNDLE")"
bash "$D/install.sh" --user "$CARPI_USER" --bundle "$BUNDLE" --updater $UPD || fail "install.sh"

# LIVI and wayvnc run in the desktop session: log the car user in at boot.
if command -v raspi-config >/dev/null; then
  say "desktop autologin"
  raspi-config nonint do_boot_behaviour B4 || say "raspi-config B4 failed; set autologin by hand"
fi

touch $DONE
finish "done $(date -Is); rebooting"
