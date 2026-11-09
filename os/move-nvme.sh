#!/bin/bash
# Copy the running system onto the NVMe and start from it. docs/IMAGE-TOOL.md, NVMe and post-setup.
#
#   sudo os/move-nvme.sh [--no-reboot]
#
# carpi-move-nvme.service runs it (installed as /usr/local/lib/carpi/move-nvme.sh), started by
# carpid from the head unit's Move to NVMe screen through a polkit rule (os/profile.sh).
# Wipes /dev/nvme0n1: a FAT32 boot partition and an ext4 root filling the rest (MBR, like Pi OS).
# Copies / and /boot/firmware across, points the copy's cmdline.txt and fstab at the new
# partitions, sets the boot order to NVMe first with the SD as the fallback (BOOT_ORDER=0xf416),
# and restarts once all of that worked. Never touches the SD's partitions. Safe to re-run: it
# starts over on the NVMe.
#
# Progress for carpid (crates/carpid/src/storage.rs), one line each:
#   start|<unix time>   stage|<name>|<message>   progress|<stage>|<done>|<total>
#   finished|ok|<message>   finished|fail|<message>
set -euo pipefail

DISK=/dev/nvme0n1
P1=${DISK}p1
P2=${DISK}p2
MNT=/mnt/carpi-nvme
PROGRESS=${CARPI_MOVE_PROGRESS:-/run/carpi-move-nvme/progress}
BOOT_MB=512
REBOOT=1
case ${1:-} in
  --no-reboot) REBOOT=0 ;;
  -h|--help) sed -n '2,16p' "$0"; exit 0 ;;
  "") ;;
  *) echo "move-nvme: unknown argument: $1" >&2; exit 1 ;;
esac

STAGE=start
FINISHED=0
mkdir -p "$(dirname "$PROGRESS")"
printf 'start|%s\n' "$(date +%s)" > "$PROGRESS"

# put LINE: one progress line, also to the journal.
put() { printf '%s\n' "$1" >> "$PROGRESS"; echo "move-nvme: $1"; }
stage() { STAGE=$1; put "stage|$1|$2"; }
# Unmounting writes the NVMe's caches out; a global sync can wait on unrelated mounts.
cleanup() {
  umount -R "$MNT" 2>/dev/null || true
}
fail() {
  put "finished|fail|$1"
  FINISHED=1
  cleanup
  exit 1
}
# Anything that stops the script without fail() still ends the job.
on_exit() {
  local rc=$?
  if [ $FINISHED = 0 ]; then
    put "finished|fail|Stopped while ${STAGE} (exit $rc); see journalctl -u carpi-move-nvme"
    cleanup
  fi
}
trap on_exit EXIT

[ "$(id -u)" = 0 ] || fail "Needs root"

# 1. Checks.
stage check "Checking the drives"
ROOT_SRC=$(findmnt -n -o SOURCE /) || fail "Can't tell where / is"
BOOT_SRC=$(findmnt -n -o SOURCE /boot/firmware) || fail "/boot/firmware isn't mounted"
case $ROOT_SRC in
  /dev/nvme*) fail "Already running from the NVMe" ;;
esac
[ -b "$DISK" ] || fail "No NVMe drive found"
for t in rsync sfdisk mkfs.vfat mkfs.ext4 blkid rpi-eeprom-config; do
  command -v $t >/dev/null || fail "$t isn't installed (os/profile.sh installs it)"
done

# Anything of the NVMe's still mounted (a last run, or the desktop's automount) comes off.
cleanup
while read -r dev mp; do
  case $dev in
    "$DISK"*) umount "$mp" 2>/dev/null || fail "The NVMe is in use at $mp" ;;
  esac
done < <(findmnt -rn -o SOURCE,TARGET)
while read -r dev _; do
  case $dev in
    "$DISK"*) swapoff "$dev" || fail "The NVMe holds swap in use" ;;
  esac
done < <(tail -n +2 /proc/swaps)

SIZE=$(blockdev --getsize64 "$DISK")
USED=$(df -B1 --output=used / /boot/firmware | tail -n +2 | awk '{s += $1} END {print s}')
NEED=$(( USED + USED / 10 + BOOT_MB * 1024 * 1024 ))
[ "$SIZE" -ge "$NEED" ] ||
  fail "The NVMe ($(( SIZE / 1000000000 )) GB) is smaller than what's in use ($(( NEED / 1000000000 )) GB with room)"
# An MBR table addresses 2 TiB.
[ "$SIZE" -le $(( 2 * 1024 * 1024 * 1024 * 1024 )) ] || fail "NVMe drives over 2 TB aren't supported yet"

# 2. Partitions: boot, then root filling the rest.
stage partition "Erasing the NVMe and making partitions"
wipefs -a -q "$DISK" || fail "Couldn't wipe the NVMe"
printf 'label: dos\n,%sM,c\n,,83\n' "$BOOT_MB" |
  sfdisk -q --wipe always --wipe-partitions always "$DISK" >/dev/null || fail "Couldn't partition the NVMe"
udevadm settle || true
for _ in $(seq 20); do [ -b "$P1" ] && [ -b "$P2" ] && break; sleep 0.5; done
[ -b "$P1" ] && [ -b "$P2" ] || fail "The new partitions didn't show up"

stage format "Formatting the NVMe"
mkfs.vfat -F 32 -n bootfs "$P1" >/dev/null || fail "Couldn't format the boot partition"
mkfs.ext4 -F -q -L rootfs "$P2" || fail "Couldn't format the root partition"

mkdir -p "$MNT"
mount "$P2" "$MNT" || fail "Couldn't mount the new root"

# 3. Copy. -x stays on /, so /proc, /sys, /dev, /run and /boot/firmware come over as empty
#    folders; the excludes are for anything mounted under them that -x can't see as separate.
stage copy "Copying the system"
put "progress|copy|0|100"
last=0
set +e
rsync -aHAXx --numeric-ids --info=progress2 --no-inc-recursive \
  --exclude='/proc/*' --exclude='/sys/*' --exclude='/dev/*' --exclude='/run/*' \
  --exclude='/tmp/*' --exclude='/mnt/*' --exclude='/media/*' --exclude='/lost+found' \
  / "$MNT/" 2>/tmp/carpi-move-nvme.err | tr '\r' '\n' | while read -r line; do
    if [[ $line =~ \ ([0-9]{1,3})%\  ]]; then
      pct=${BASH_REMATCH[1]}
      if [ "$pct" != "$last" ]; then put "progress|copy|$pct|100"; last=$pct; fi
    fi
  done
rc=${PIPESTATUS[0]}
set -e
# 24: files vanished while copying, normal on a running system.
if [ "$rc" != 0 ] && [ "$rc" != 24 ]; then
  fail "Copying failed (rsync $rc): $(tail -n 1 /tmp/carpi-move-nvme.err)"
fi

# The boot partition goes on after the root copy, so rsync never sets owners on FAT.
stage boot "Copying the boot files"
mkdir -p "$MNT/boot/firmware"
mount "$P1" "$MNT/boot/firmware" || fail "Couldn't mount the new boot partition"
rsync -rt --modify-window=2 /boot/firmware/ "$MNT/boot/firmware/" || fail "Couldn't copy the boot files"

# 4. The copy starts from its own partitions.
stage ids "Pointing the copy at the NVMe"
NEW_BOOT=$(blkid -s PARTUUID -o value "$P1")
NEW_ROOT=$(blkid -s PARTUUID -o value "$P2")
OLD_ROOT=$(blkid -s PARTUUID -o value "$ROOT_SRC" || true)
OLD_BOOT=$(blkid -s PARTUUID -o value "$BOOT_SRC" || true)
[ -n "$NEW_BOOT" ] && [ -n "$NEW_ROOT" ] || fail "The new partitions have no IDs"
[ "$NEW_ROOT" != "$OLD_ROOT" ] && [ "$NEW_BOOT" != "$OLD_BOOT" ] ||
  fail "The NVMe's partition IDs match the SD's"

CMDLINE=$MNT/boot/firmware/cmdline.txt
[ -f "$CMDLINE" ] || fail "No cmdline.txt on the copy"
sed -i -E "s#(^| )root=[^ ]+#\1root=PARTUUID=$NEW_ROOT#" "$CMDLINE"
grep -q "root=PARTUUID=$NEW_ROOT" "$CMDLINE" || fail "cmdline.txt has no root= to change"

FSTAB=$MNT/etc/fstab
awk -v r="PARTUUID=$NEW_ROOT" -v b="PARTUUID=$NEW_BOOT" '
  /^[[:space:]]*#/ || NF < 2 { print; next }
  $2 == "/" { $1 = r }
  $2 == "/boot/firmware" || $2 == "/boot" { $1 = b }
  { print }' "$FSTAB" > "$FSTAB.new" || fail "Couldn't rewrite fstab"
grep -q "^PARTUUID=${NEW_ROOT}[[:space:]]" "$FSTAB.new" || fail "fstab has no / line to change"
mv "$FSTAB.new" "$FSTAB"

umount -R "$MNT" || fail "Couldn't unmount the NVMe"

# 5. Boot order: NVMe, then SD, then USB. Applied by the bootloader on the next start.
stage eeprom "Setting the Pi to start from the NVMe"
CONF=$(mktemp)
rpi-eeprom-config > "$CONF" || fail "Couldn't read the boot settings"
WANT=("BOOT_ORDER=0xf416")
# A non-HAT+ NVMe adapter needs the bootloader to probe PCIe (config.txt turns it on for Linux).
if grep -qE '^[[:space:]]*dtparam=(pciex1|nvme)' /boot/firmware/config.txt; then
  WANT+=("PCIE_PROBE=1")
fi
changed=0
for w in "${WANT[@]}"; do
  key=${w%%=*}
  if ! grep -qx "$w" "$CONF"; then
    sed -i "/^$key=/d" "$CONF"
    echo "$w" >> "$CONF"
    changed=1
  fi
done
if [ $changed = 1 ]; then
  rpi-eeprom-config --apply "$CONF" >/dev/null || fail "Couldn't change the boot order"
fi
rm -f "$CONF"

if [ $REBOOT = 1 ]; then
  put "finished|ok|Copied. Restarting from the NVMe"
  FINISHED=1
  sleep 5
  systemctl reboot
else
  put "finished|ok|Copied. Restart to run from the NVMe"
  FINISHED=1
fi
