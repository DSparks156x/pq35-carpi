#!/bin/bash
# Turn a fresh Raspberry Pi OS Trixie into a carpi device. DEPLOY.md section 6.
#
#   sudo os/profile.sh [--hat carpihat|cowcarhat|none] [--user NAME] [--tz ZONE] [--dry-run] [--no-packages]
#
# --no-packages: report missing packages instead of installing them (a Pi with no internet).
# --tz (or CARPI_TZ=...): the Pi's timezone, e.g. America/Chicago. The car clock is local time and
# carpid sets the Pi clock from it, so the two must agree. Without it the timezone is left alone.
# Idempotent: every step checks first and only changes what's missing. Safe to re-run.
# Never touches user config (~/.config/carpi, ~/.config/LIVI, ...).
# Bump PROFILE_VERSION when a step is added that bundles may rely on; bundles name the minimum
# they need (manifest.json min_os_profile, docs/BUNDLE.md).
set -euo pipefail

PROFILE_VERSION=1

HAT=carpihat
USERNAME=${SUDO_USER:-}
TZONE=${CARPI_TZ:-}
DRY=0
NOPKG=0
while [ $# -gt 0 ]; do
  case $1 in
    --hat) HAT=$2; shift 2 ;;
    --user) USERNAME=$2; shift 2 ;;
    --tz) TZONE=$2; shift 2 ;;
    --dry-run) DRY=1; shift ;;
    --no-packages) NOPKG=1; shift ;;
    -h|--help) sed -n '2,14p' "$0"; exit 0 ;;
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
  # The image tool's options.py writes config.txt itself.
  none) HAT_LINES=() ;;
  *) die "unknown hat: $HAT (carpihat, cowcarhat, none)" ;;
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
  # carpid and the bench; polkitd for carpid's clock rule
  can-utils evtest wayvnc polkitd
  # os/install.sh unpacks a bundle's share/ with tar --zstd
  zstd
  # Move to NVMe (os/move-nvme.sh)
  rsync fdisk dosfstools rpi-eeprom
)

# 1. Packages
missing=()
for p in "${PACKAGES[@]}"; do
  dpkg-query -W -f='${Status}' "$p" 2>/dev/null | grep -q 'install ok installed' || missing+=("$p")
done
if [ ${#missing[@]} -gt 0 ] && [ $NOPKG = 1 ]; then
  say "missing ${missing[*]}: not installed (--no-packages)"
elif [ ${#missing[@]} -gt 0 ]; then
  say "installing ${missing[*]}"
  do_ apt-get update
  do_ env DEBIAN_FRONTEND=noninteractive apt-get install -y "${missing[@]}"
else
  say "packages ok"
fi

# 2. config.txt: append missing lines under [all], in one marked block.
CONFIG=/boot/firmware/config.txt
[ -f $CONFIG ] || die "$CONFIG not found"
# The overlays may be folded into one DTB (step 10): edit the plain file, fold again at the end.
HERE=$(cd "$(dirname "$0")" && pwd)
DTMERGE=/usr/local/sbin/carpi-dt-merge
if [ -f "$HERE/dt-merge.sh" ] && ! cmp -s "$HERE/dt-merge.sh" $DTMERGE; then
  say "installing $DTMERGE"; do_ install -m 0755 "$HERE/dt-merge.sh" $DTMERGE
fi
CONFIG_BEFORE=$(cat $CONFIG)
[ $DRY = 1 ] || [ ! -x $DTMERGE ] || $DTMERGE undo
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
# The mcp2515's default queue is 10 frames; a DIS burst overflows it (ENOBUFS). Applies from
# the next boot; set it now too.
if put_file /etc/systemd/network/80-can0.link "[Match]
OriginalName=can0

[Link]
TransmitQueueLength=100"; then
  do_ ip link set can0 txqueuelen 100 2>/dev/null || true
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

# 7. Journal on disk, capped, so a capture after a crash still has the minutes before it.
#    The car user reads every unit's journal through the adm group. Synced every 10 s: the car
#    can cut power without a shutdown, and journald's default 5 min loses the whole drive.
if put_file /etc/systemd/journald.conf.d/carpi.conf "[Journal]
Storage=persistent
SystemMaxUse=200M
SyncIntervalSec=10s"; then
  do_ systemctl restart systemd-journald
fi
if ! id -nG "$USERNAME" | tr ' ' '\n' | grep -qx adm; then
  say "adding $USERNAME to adm"; do_ usermod -aG adm "$USERNAME"; REBOOT=1
fi

# 8. carpid sets the clock from the car's while network time hasn't synced (timedated; needs
#    set-ntp too, since timedated refuses set-time while NTP is on), and the timezone from
#    the settings page.
put_file /etc/polkit-1/rules.d/50-carpi-time.rules "// carpid: set the clock from the car's, and the timezone (crates/carpid/src/clock.rs).
polkit.addRule(function(action, subject) {
  if ((action.id == \"org.freedesktop.timedate1.set-time\" ||
       action.id == \"org.freedesktop.timedate1.set-ntp\" ||
       action.id == \"org.freedesktop.timedate1.set-timezone\") && subject.user == \"$USERNAME\") {
    return polkit.Result.YES;
  }
});" || true

# 8b. carpid saves, tests and scans Wi-Fi networks for updates (docs/PAGES.md 5b). It runs as a
#     lingering user service, not an active login, so polkit asks for a password without this.
put_file /etc/polkit-1/rules.d/50-carpi-wifi.rules "// carpid: Wi-Fi networks for updates (crates/carpid/src/wifi.rs).
polkit.addRule(function(action, subject) {
  if ((action.id == \"org.freedesktop.NetworkManager.settings.modify.system\" ||
       action.id == \"org.freedesktop.NetworkManager.network-control\" ||
       action.id == \"org.freedesktop.NetworkManager.wifi.scan\") && subject.user == \"$USERNAME\") {
    return polkit.Result.YES;
  }
});" || true

# 8c. carpid starts Move to NVMe (os/move-nvme.sh, run as root by carpi-move-nvme.service, which
#     os/install.sh installs). This unit only, start only.
put_file /etc/polkit-1/rules.d/50-carpi-nvme.rules "// carpid: Move to NVMe (crates/carpid/src/storage.rs).
polkit.addRule(function(action, subject) {
  if (action.id == \"org.freedesktop.systemd1.manage-units\" &&
      action.lookup(\"unit\") == \"carpi-move-nvme.service\" &&
      action.lookup(\"verb\") == \"start\" && subject.user == \"$USERNAME\") {
    return polkit.Result.YES;
  }
});" || true

# 9. Timezone, when asked for.
if [ -n "$TZONE" ]; then
  [ -f "/usr/share/zoneinfo/$TZONE" ] || die "unknown timezone: $TZONE"
  if [ "$(timedatectl show -p Timezone --value 2>/dev/null)" != "$TZONE" ]; then
    say "timezone $TZONE"; do_ timedatectl set-timezone "$TZONE"
  else
    say "timezone ok"
  fi
fi

# 10. Boot time (docs/BOOT.md). Power-on to LIVI paging the phone; measure with tools/boot-measure.sh.
#     Firmware: no camera/DSI probing, no initramfs (NVMe, ext4 and PCIe are built into the kernel),
#     an uncompressed kernel (no gunzip; a kernel hook keeps the copy current), no HAT EEPROM read.
for kv in camera_auto_detect=0 display_auto_detect=0 auto_initramfs=0; do
  k=${kv%=*}
  if grep -q "^$k=" $CONFIG && ! grep -qx "$kv" $CONFIG; then
    say "config.txt: $kv"; [ $DRY = 1 ] || sed -i "s/^$k=.*/$kv/" $CONFIG; REBOOT=1
  fi
done
RAWHOOK=/etc/kernel/postinst.d/zz-carpi-raw-kernel
if put_file $RAWHOOK '#!/bin/sh
# carpi (os/profile.sh): the firmware loads an uncompressed copy of the kernel, ~0.3 s faster.
# Keeps kernel_2712_raw.img in step with kernel_2712.img; drops the kernel= line if that fails.
# Also folds the overlays again (carpi-dt-merge): a kernel update brings new DTBs and overlays.
C=/boot/firmware/config.txt
if gunzip -c /boot/firmware/kernel_2712.img > /boot/firmware/kernel_2712_raw.img.tmp 2>/dev/null; then
  mv /boot/firmware/kernel_2712_raw.img.tmp /boot/firmware/kernel_2712_raw.img
else
  rm -f /boot/firmware/kernel_2712_raw.img.tmp
  sed -i "/^kernel=kernel_2712_raw.img$/d" $C
fi
[ -x /usr/local/sbin/carpi-dt-merge ] && /usr/local/sbin/carpi-dt-merge merge
exit 0'; then
  do_ chmod 755 $RAWHOOK
fi
if ! grep -qx 'kernel=kernel_2712_raw.img' $CONFIG && [ -f /boot/firmware/kernel_2712.img ]; then
  say "config.txt: uncompressed kernel"
  if [ $DRY = 0 ]; then
    $RAWHOOK
    [ -f /boot/firmware/kernel_2712_raw.img ] && \
      printf '\n[all]\n# carpi boot time\nkernel=kernel_2712_raw.img\nforce_eeprom_read=0\n' >> $CONFIG
  fi
  REBOOT=1
fi
#     Kernel command line: black from power-on to LIVI. No splash (plymouth), console off the screen,
#     no logo, no cursor.
CMDLINE=/boot/firmware/cmdline.txt
if [ -f $CMDLINE ] && ! grep -q 'vt.global_cursor_default=0' $CMDLINE; then
  say "cmdline.txt: black boot"
  [ $DRY = 1 ] || sed -i -e 's/ splash\b//; s/ plymouth.ignore-serial-consoles\b//; s/ console=tty1\b/ console=tty3/' \
    -e 's/$/ loglevel=3 logo.nologo vt.global_cursor_default=0 plymouth.enable=0/' -e 's/  */ /g' $CMDLINE
  REBOOT=1
fi
#     NetworkManager runs "netplan generate" at every start (~0.7 s each, up to three times) and
#     everything waited for NM. Wi-Fi profiles netplan made (cloud-init's first-boot Wi-Fi) become
#     NM keyfiles; new ones are keyfiles already. A wrapper skips generate when there's no YAML.
for y in /etc/netplan/*.yaml; do
  [ -e "$y" ] || continue
  say "netplan: $y -> NetworkManager keyfiles"
  if [ $DRY = 0 ]; then
    for f in /run/NetworkManager/system-connections/netplan-*.nmconnection; do
      [ -e "$f" ] || continue
      dst=/etc/NetworkManager/system-connections/$(basename "$f")
      [ -e "$dst" ] || install -m 600 "$f" "$dst"
    done
    mkdir -p /var/lib/carpi/netplan && mv "$y" /var/lib/carpi/netplan/
  fi
done
if ! dpkg-divert --list /usr/sbin/netplan 2>/dev/null | grep -q netplan.distrib && [ -e /usr/sbin/netplan ]; then
  say "netplan: wrapper"; do_ dpkg-divert --local --rename --divert /usr/sbin/netplan.distrib --add /usr/sbin/netplan
fi
[ -e /usr/sbin/netplan.distrib ] || [ $DRY = 1 ] && put_file /usr/sbin/netplan '#!/bin/sh
# carpi (os/profile.sh; dpkg-divert, the real CLI is /usr/sbin/netplan.distrib).
# NetworkManager runs "netplan generate" at every start: Python, generate, udevadm and
# "systemctl daemon-reload", ~0.7 s each. With no YAML (/lib/netplan only says
# "renderer: NetworkManager") and no netplan-made NM profiles there is nothing to generate.
if [ "$*" = "generate" ] \
   && ! ls /etc/netplan/*.yaml /run/netplan/*.yaml >/dev/null 2>&1 \
   && ! ls /run/NetworkManager/system-connections/netplan-* >/dev/null 2>&1; then
  exit 0
fi
exec /usr/sbin/netplan.distrib "$@"' && do_ chmod 755 /usr/sbin/netplan
#     Logins (the car user's manager: carpid, LIVI) don't wait for NetworkManager: LIVI runs its own
#     access point. A copy of the packaged unit without network.target.
if [ -f /usr/lib/systemd/system/systemd-user-sessions.service ]; then
  put_file /etc/systemd/system/systemd-user-sessions.service "# carpi (os/profile.sh): the packaged unit without After=network.target.
$(sed '/^After=/s/ *network.target//' /usr/lib/systemd/system/systemd-user-sessions.service)" && REBOOT=1
fi
#     cloud-init only runs the first boot (os/firstboot.sh); after that it's ~0.7 s of every boot.
if [ -d /etc/cloud ] && [ ! -e /etc/cloud/cloud-init.disabled ]; then
  say "cloud-init: off after this boot"; do_ touch /etc/cloud/cloud-init.disabled
fi
#     Units the car doesn't use: X11/DSI checks, ssh-file switch (ssh is on), binfmt, printing, NFS,
#     and the wait-online units (nothing needs network-online; they hold the boot for a minute).
MASK=(rp1-test.service sshswitch.service glamor-test.service systemd-binfmt.service
  proc-sys-fs-binfmt_misc.automount NetworkManager-wait-online.service systemd-networkd-wait-online.service)
OFF=(cups.service cups.socket cups.path cups-browsed.service rpcbind.service rpcbind.socket
  nfs-blkmap.service nfs-client.target)
for u in "${MASK[@]}"; do
  [ "$(systemctl is-enabled "$u" 2>/dev/null)" = masked ] && continue
  systemctl cat "$u" >/dev/null 2>&1 || continue
  say "masking $u"; do_ systemctl mask "$u"
done
for u in "${OFF[@]}"; do
  systemctl is-enabled -q "$u" 2>/dev/null || continue
  say "disabling $u"; do_ systemctl disable "$u"
done
#     Kiosk session: labwc on tty7 for the car user straight from systemd, in place of lightdm
#     autologin and the desktop (panel, file manager, wallpaper). Black until LIVI draws. A desktop
#     can come later (on leaving LIVI); lightdm stays installed:
#       systemctl disable carpi-session && systemctl enable --force lightdm
put_file /etc/carpi/labwc/autostart '# carpi kiosk session (os/profile.sh): only what LIVI needs. No panel, file manager or wallpaper.
# avsd sets the mode of its headless screen too; this has it right from the start.
wlr-randr --output NOOP-1 --custom-mode 800x480@60Hz >/dev/null 2>&1 &' || true
put_file /etc/carpi/labwc/rc.xml '<?xml version="1.0"?>
<!-- carpi kiosk session (os/profile.sh) -->
<labwc_config>
  <core><decoration>server</decoration></core>
</labwc_config>' || true
put_file /etc/carpi/labwc/environment '# carpi kiosk session (os/profile.sh)
XKB_DEFAULT_MODEL=pc105
XKB_DEFAULT_LAYOUT=us
# With no display plugged in, a headless output: the screen avsd sends to the RNS-E.
LABWC_FALLBACK_OUTPUT=NOOP-1' || true
put_file /etc/pam.d/carpi-session '#%PAM-1.0
# carpi-session.service (os/profile.sh). systemd runs account + session only; the session part is
# /etc/pam.d/lightdm-autologin'"'"'s, so logind registers an active wayland session on seat0, VT 7.
@include common-account
session  required pam_env.so readenv=1
session  required pam_env.so readenv=1 envfile=/etc/default/locale
session  [success=ok ignore=ignore module_unknown=ignore default=bad] pam_selinux.so close
session  required pam_limits.so
session  required pam_loginuid.so
@include common-session
session  [success=ok ignore=ignore module_unknown=ignore default=bad] pam_selinux.so open' || true
if put_file /etc/systemd/system/carpi-session.service "# carpi kiosk session (os/profile.sh): labwc on tty7 for the car user, straight from systemd.
# Replaces lightdm autologin and the desktop: black from power-on until LIVI draws.
# Back to the desktop: systemctl disable carpi-session && systemctl enable --force lightdm
[Unit]
Description=carpi kiosk session (labwc on tty7)
After=systemd-user-sessions.service dev-dri-card0.device dev-dri-renderD128.device getty@tty7.service
Wants=dev-dri-card0.device dev-dri-renderD128.device
Conflicts=getty@tty7.service

[Service]
Type=simple
User=$USERNAME
PAMName=carpi-session
WorkingDirectory=~
TTYPath=/dev/tty7
TTYReset=yes
TTYVHangup=yes
TTYVTDisallocate=yes
StandardInput=tty-fail
StandardOutput=journal
StandardError=journal
UtmpIdentifier=tty7
UtmpMode=user
Environment=XDG_SESSION_TYPE=wayland XDG_SESSION_CLASS=user XDG_SEAT=seat0 XDG_VTNR=7
Environment=XDG_SESSION_DESKTOP=carpi XDG_CURRENT_DESKTOP=wlroots WLR_DRM_FORCE_LIBLIFTOFF=1
# logind only makes the session active once its VT is in front
ExecStartPre=+/usr/bin/chvt 7
ExecStart=/usr/bin/labwc -C /etc/carpi/labwc
Restart=always
RestartSec=1

[Install]
Alias=display-manager.service"; then
  do_ systemctl daemon-reload; REBOOT=1
fi
if [ "$(readlink /etc/systemd/system/display-manager.service 2>/dev/null)" != /etc/systemd/system/carpi-session.service ]; then
  say "kiosk session in place of lightdm"
  systemctl is-enabled -q lightdm.service 2>/dev/null && do_ systemctl disable lightdm.service
  do_ systemctl enable --force carpi-session.service
  do_ systemctl set-default graphical.target
  REBOOT=1
fi

# 11. Record the profile.
put_file /etc/carpi-profile "PROFILE_VERSION=$PROFILE_VERSION
HAT=$HAT" || true

# 12. Fold config.txt's overlays into one DTB (os/dt-merge.sh, ~0.2 s of firmware). Last, after every
#     config.txt edit above. os/options.py undoes and folds again around its own edits.
if [ $DRY = 0 ] && [ -x $DTMERGE ]; then
  $DTMERGE merge
  [ "$(cat $CONFIG)" = "$CONFIG_BEFORE" ] || REBOOT=1
fi

say "done, profile $PROFILE_VERSION, hat $HAT, user $USERNAME"
[ $REBOOT = 0 ] || say "reboot for config.txt / group changes"
