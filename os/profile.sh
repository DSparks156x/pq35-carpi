#!/bin/bash
# Turn a fresh Raspberry Pi OS Trixie into a carpi device. DEPLOY.md section 6.
#
#   sudo os/profile.sh [--hat carpihat|cowcarhat] [--user NAME] [--dry-run]
#
# Idempotent: every step checks first and only changes what's missing. Safe to re-run.
# Never touches user config (~/.config/carpi, ~/.config/LIVI, ...).
# Bump PROFILE_VERSION when a step is added that bundles may rely on; bundles name the minimum
# they need (manifest.json min_os_profile, docs/BUNDLE.md).
set -euo pipefail

PROFILE_VERSION=1

HAT=carpihat
USERNAME=${SUDO_USER:-}
DRY=0
while [ $# -gt 0 ]; do
  case $1 in
    --hat) HAT=$2; shift 2 ;;
    --user) USERNAME=$2; shift 2 ;;
    --dry-run) DRY=1; shift ;;
    -h|--help) sed -n '2,9p' "$0"; exit 0 ;;
    *) echo "profile: unknown argument: $1" >&2; exit 1 ;;
  esac
done

say() { echo "profile: $*"; }
die() { echo "profile: $*" >&2; exit 1; }
# do_ CMD...: run it, or print it with --dry-run.
do_() { if [ $DRY = 1 ]; then echo "  would run: $*"; else "$@"; fi; }
REBOOT=0

[ $DRY = 1 ] || [ "$(id -u)" = 0 ] || die "run as root (sudo)"
. /etc/os-release
[ "${VERSION_CODENAME:-}" = trixie ] || die "needs Trixie, this is ${PRETTY_NAME:-unknown}"
[ -n "$USERNAME" ] || die "no user: run with sudo from the car user, or pass --user"
id "$USERNAME" >/dev/null 2>&1 || die "no such user: $USERNAME"

# config.txt lines per hat.
case $HAT in
  carpihat)
    HAT_LINES=(
      "dtparam=spi=on"
      "dtoverlay=mcp2515-can0,oscillator=8000000,interrupt=23"
      "gpio=25=op,dh"
      "dtoverlay=gpio-poweroff,gpiopin=25,active_low"
      "usb_max_current_enable=1"
    )
    ;;
  cowcarhat)
    die "cowcarhat: lines not defined yet"
    ;;
  *) die "unknown hat: $HAT (carpihat, cowcarhat)" ;;
esac

# Runtime only; nothing is built on the device.
PACKAGES=(
  # LIVI: native CarPlay and wireless Android Auto (livi/README.md)
  bluez libspa-0.2-bluetooth hostapd dnsmasq-base iw rfkill avahi-daemon avahi-utils pulseaudio-utils fuse3
  # libraries LIVI links (the -dev packages it builds against, minus -dev)
  libssl3t64 libwayland-client0 libwayland-server0 libxkbcommon0 libfontconfig1
  libudev1 libinput10 libseat1 libgbm1 libgstreamer1.0-0 libgstreamer-plugins-base1.0-0
  # livi-gst-host elements: appsrc/videoconvert/playbin3 (base), gl*, v4l2*dec/vp8/vp9/jpeg (good),
  # h26xparse/waylandsink/v4l2sl*/va* (bad), avdec_* (libav), pulsesink
  gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-libav
  gstreamer1.0-gl gstreamer1.0-pulseaudio
  # carpid and the bench
  can-utils evtest wayvnc
)

# 1. Packages
missing=()
for p in "${PACKAGES[@]}"; do
  dpkg-query -W -f='${Status}' "$p" 2>/dev/null | grep -q 'install ok installed' || missing+=("$p")
done
if [ ${#missing[@]} -gt 0 ]; then
  say "installing ${missing[*]}"
  do_ apt-get update
  do_ env DEBIAN_FRONTEND=noninteractive apt-get install -y "${missing[@]}"
else
  say "packages ok"
fi

# 2. config.txt: append missing lines under [all], in one marked block.
CONFIG=/boot/firmware/config.txt
[ -f $CONFIG ] || die "$CONFIG not found"
add=()
for l in "${HAT_LINES[@]}"; do
  grep -qxF "$l" $CONFIG || add+=("$l")
done
if [ ${#add[@]} -gt 0 ]; then
  say "config.txt: adding ${add[*]}"
  if [ $DRY = 0 ]; then
    cp -p $CONFIG $CONFIG.carpi-bak
    printf '\n[all]\n# carpi profile %s (%s)\n' $PROFILE_VERSION "$HAT" >> $CONFIG
    printf '%s\n' "${add[@]}" >> $CONFIG
  fi
  REBOOT=1
else
  say "config.txt ok"
fi

# put_file PATH CONTENT: write only if different. Returns 0 if it changed.
put_file() {
  if [ -f "$1" ] && [ "$(cat "$1")" = "$2" ]; then return 1; fi
  say "writing $1"
  [ $DRY = 1 ] || { mkdir -p "$(dirname "$1")"; printf '%s\n' "$2" > "$1"; }
}

# 3. CAN on can0 through systemd-networkd. Not required for online: no hat, no boot stall.
if put_file /etc/systemd/network/80-can0.network "[Match]
Name=can0

[Link]
RequiredForOnline=no

[CAN]
BitRate=100000
RestartSec=100ms"; then
  if systemctl is-active -q systemd-networkd; then do_ networkctl reload; fi
fi
if ! systemctl is-enabled -q systemd-networkd 2>/dev/null; then
  say "enabling systemd-networkd"; do_ systemctl enable --now systemd-networkd
fi

# 4. uinput: module at boot, input group may open it (same rule as deploy/99-carpi-uinput.rules).
if put_file /etc/modules-load.d/uinput.conf "uinput"; then do_ modprobe uinput; fi
if put_file /etc/udev/rules.d/99-carpi-uinput.rules \
  'KERNEL=="uinput", GROUP="input", MODE="0660", OPTIONS+="static_node=uinput"'; then
  do_ udevadm control --reload
  do_ udevadm trigger --name-match=uinput || true
fi
if ! id -nG "$USERNAME" | tr ' ' '\n' | grep -qx input; then
  say "adding $USERNAME to input"; do_ usermod -aG input "$USERNAME"; REBOOT=1
fi

# 5. Lingering: user services (carpid, LIVI) start at boot without a login.
if [ ! -e "/var/lib/systemd/linger/$USERNAME" ]; then
  say "enabling linger for $USERNAME"; do_ loginctl enable-linger "$USERNAME"
fi

# 6. /opt/carpi: releases/<version> and the current symlink, owned by the car user (carpi-update runs as it).
GROUP=$(id -gn "$USERNAME")
for d in /opt/carpi /opt/carpi/releases; do
  if [ ! -d $d ]; then
    say "creating $d"; do_ install -d -o "$USERNAME" -g "$GROUP" -m 0755 $d
  elif [ "$(stat -c %U:%G $d)" != "$USERNAME:$GROUP" ]; then
    say "chown $d"; do_ chown "$USERNAME:$GROUP" $d
  fi
done

# 7. Record the profile.
put_file /etc/carpi-profile "PROFILE_VERSION=$PROFILE_VERSION
HAT=$HAT" || true

say "done, profile $PROFILE_VERSION, hat $HAT, user $USERNAME"
[ $REBOOT = 0 ] || say "reboot for config.txt / group changes"
