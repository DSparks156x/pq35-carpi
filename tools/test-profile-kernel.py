#!/usr/bin/env python3
"""Run the actual raw-kernel profile step and postinst hook against a fake boot partition."""
import os
from pathlib import Path
import subprocess


ROOT = Path(__file__).resolve().parents[1]
profile = (ROOT / "os/profile.sh").read_text()
start = profile.index("RAWHOOK=")
block = profile[start:profile.index("#     Kernel command line:", start)]
# Only redirect system paths; execute the production shell logic unchanged.
for original, replacement in (
    ("/etc/kernel/postinst.d/zz-carpi-raw-kernel", "${TEST_ROOT}/hook"),
    ("/boot/firmware", "${TEST_ROOT}/boot"),
    ("/proc/device-tree/compatible", "${TEST_ROOT}/compatible"),
    ("/usr/local/sbin/carpi-dt-merge", "${TEST_ROOT}/dt-merge"),
):
    block = block.replace(original, replacement)

harness = r'''set -euo pipefail
base=$(mktemp -d)
trap 'rm -rf "$base"' EXIT
say() { echo "$*"; }
do_() { if [ "$DRY" = 0 ]; then "$@"; fi; }
put_file() {
  [ ! -f "$1" ] || [ "$(cat "$1")" != "$2" ] || return 1
  [ "$DRY" = 1 ] || printf '%s\n' "$2" > "$1"
}
new_case() {
  export TEST_ROOT=$base/$1
  mkdir -p "$TEST_ROOT/boot"
  CONFIG=$TEST_ROOT/boot/config.txt DRY=0 REBOOT=0
  printf '[all]\n' > "$CONFIG"
  printf 'test kernel\n' | gzip > "$TEST_ROOT/boot/kernel_2712.img"
  printf 'raspberrypi,4-model-b\000brcm,bcm2711\000' > "$TEST_ROOT/compatible"
}
supported() { printf '%s\000brcm,bcm2712\000' "$1" > "$TEST_ROOT/compatible"; }
selected() { grep -qx 'kernel=kernel_2712_raw.img' "$CONFIG"; }
apply_profile() {
''' + block + r'''
}

# Pi OS ships the 2712 image on a Pi 4 too: its presence must not select it.
new_case pi4
apply_profile
! selected
[ ! -e "$TEST_ROOT/boot/kernel_2712_raw.img" ]
[ "$REBOOT" = 0 ]
# Re-running repairs the exact legacy override without deleting other settings.
printf '# carpi boot time\nkernel=kernel_2712_raw.img\nforce_eeprom_read=0\n' >> "$CONFIG"
apply_profile
! selected
grep -qx 'force_eeprom_read=0' "$CONFIG"
[ "$REBOOT" = 1 ]
# The installed postinst hook independently repairs an incompatible selection too.
printf 'kernel=kernel_2712_raw.img\n' >> "$CONFIG"
"$RAWHOOK"
! selected

for board in raspberrypi,5-model-b raspberrypi,500 raspberrypi,compute-module-5; do
  new_case "${board##*,}"
  supported "$board"
  apply_profile
  selected
  cmp "$TEST_ROOT/boot/kernel_2712_raw.img" <(printf 'test kernel\n')
  [ "$REBOOT" = 1 ]
  REBOOT=0
  cp "$CONFIG" "$TEST_ROOT/before"
  apply_profile
  cmp "$CONFIG" "$TEST_ROOT/before"
  [ "$REBOOT" = 0 ]
  # Kernel updates refresh the selected copy, but never enable an unselected one.
  printf 'updated kernel\n' | gzip > "$TEST_ROOT/boot/kernel_2712.img"
  "$RAWHOOK"
  cmp "$TEST_ROOT/boot/kernel_2712_raw.img" <(printf 'updated kernel\n')
  sed -i '/^kernel=kernel_2712_raw.img$/d' "$CONFIG"
  "$RAWHOOK"
  ! selected
done

# Any custom or potentially included selection must retain precedence, even if
# a legacy generated override followed it in an [all] block.
for setting in 'kernel=my-kernel.img' '  kernel = custom.img' 'os_prefix=custom/' 'include custom.txt'; do
  new_case custom
  supported raspberrypi,5-model-b
  printf '%s\nkernel=kernel_2712_raw.img\n' "$setting" >> "$CONFIG"
  apply_profile
  ! selected
  grep -qxF "$setting" "$CONFIG"
  [ ! -e "$TEST_ROOT/boot/kernel_2712_raw.img" ]
done

# A failed refresh must remove the selection even when an old raw copy remains.
new_case corrupt
supported raspberrypi,5-model-b
apply_profile
printf 'invalid gzip\n' > "$TEST_ROOT/boot/kernel_2712.img"
"$RAWHOOK"
! selected
[ ! -e "$TEST_ROOT/boot/kernel_2712_raw.img.tmp" ]
apply_profile
! selected

new_case missing_kernel
supported raspberrypi,5-model-b
rm "$TEST_ROOT/boot/kernel_2712.img"
apply_profile
! selected

new_case unknown_board
rm "$TEST_ROOT/compatible"
printf 'kernel=kernel_2712_raw.img\n' >> "$CONFIG"
apply_profile
! selected

new_case dry_run
supported raspberrypi,5-model-b
DRY=1
cp "$CONFIG" "$TEST_ROOT/before"
apply_profile
cmp "$CONFIG" "$TEST_ROOT/before"
[ ! -e "$TEST_ROOT/hook" ]
echo 'raw kernel: board guard, legacy repair, custom config, updates, failures and dry run passed'
'''
result = subprocess.run([os.environ.get("BASH", "bash"), "-s"], input=harness,
                        text=True, capture_output=True)
assert result.returncode == 0, f"exit {result.returncode}\n{result.stdout}{result.stderr}"
print(result.stdout.rstrip().splitlines()[-1])
