#!/bin/bash
# carpi-dt-merge: fold config.txt's device tree overlays into one DTB, so the firmware loads one file
# instead of applying each overlay at boot (~0.2 s on a Pi 5; docs/BOOT.md). Installed by os/profile.sh
# as /usr/local/sbin/carpi-dt-merge.
#
#   carpi-dt-merge merge   config.txt as written -> carpi-merged.dtb, the folded lines get "#merged# "
#   carpi-dt-merge undo    back to plain config.txt (the overlays applied by the firmware again)
#
# Edit config.txt only after `undo` (os/profile.sh and os/options.py do), then `merge` again.
# Kernel updates re-merge (a kernel postinst hook), since they bring new base DTBs and overlays.
# Anything it doesn't understand (another board, a conditional section with overlays in it, an
# overlay or parameter dtmerge refuses) leaves config.txt plain: slower, never broken.
set -uo pipefail

BOOT=${CARPI_BOOT:-/boot/firmware}
C=$BOOT/config.txt
OV=$BOOT/overlays
OUT=carpi-merged.dtb
BASE=bcm2712-rpi-5-b.dtb
MARK='#merged# '
BEGIN='# >>> carpi-dt-merge'
END='# <<< carpi-dt-merge'

say() { echo "dt-merge: $*"; }

undo() {
  [ -f "$C" ] || return 0
  grep -qE "^$MARK|^$BEGIN" "$C" || return 0
  sed -i -e "s/^$MARK//" -e "/^$BEGIN\$/,/^$END\$/d" "$C"
  # the blank line before our block
  sed -i -e '${/^$/d}' "$C"
  say "config.txt plain again"
}

# overlay NAME -> the file the firmware would load on a Pi 5 (overlay_map.dtb), or fail.
overlay_file() {
  local n=$1 map=$OV/overlay_map.dtb m
  if [ -f "$map" ] && fdtget "$map" "/$n" >/dev/null 2>&1; then
    if fdtget -l "$map" "/$n" >/dev/null 2>&1 && fdtget -p "$map" "/$n" 2>/dev/null | grep -qx bcm2712; then
      m=$(fdtget -t s "$map" "/$n" bcm2712 2>/dev/null)
      [ -n "$m" ] && n=$m
    elif fdtget -p "$map" "/$n" 2>/dev/null | grep -qE '^bcm27'; then
      return 1   # mapped for other chips only: not for a Pi 5
    fi
  fi
  [ -f "$OV/$n.dtbo" ] || return 1
  echo "$OV/$n.dtbo"
}

merge() {
  undo
  [ -f "$C" ] || { say "no $C"; return 0; }
  if ! tr -d '\0' < /proc/device-tree/compatible 2>/dev/null | grep -q 'raspberrypi,5-model-b'; then
    say "not a Pi 5: left alone"; return 0
  fi
  command -v dtmerge >/dev/null && command -v fdtget >/dev/null || { say "no dtmerge/fdtget: left alone"; return 0; }
  [ -f "$BOOT/$BASE" ] || { say "no $BASE: left alone"; return 0; }
  if grep -qE '^\s*(device_tree|os_prefix|overlay_prefix)=' "$C"; then
    say "config.txt sets its own device_tree/prefix: left alone"; return 0
  fi

  tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
  cp "$BOOT/$BASE" "$tmp/cur.dtb"
  local active=1 n=0 ovl="" ovf="" ovp=() folded=() line sect
  # step OVERLAY_FILE PARAMS...: one dtmerge on cur.dtb ("-" = base parameters). try: only see if it takes.
  step() { dtmerge "$tmp/cur.dtb" "$tmp/next.dtb" "$@" >/dev/null 2>&1 && mv "$tmp/next.dtb" "$tmp/cur.dtb"; }
  try() { dtmerge "$tmp/cur.dtb" "$tmp/try.dtb" "$@" >/dev/null 2>&1; }
  flush() {  # the pending overlay with its parameters
    [ -n "$ovf" ] || return 0
    step "$ovf" "${ovp[@]}" || { say "dtmerge refused $ovl ${ovp[*]}"; return 1; }
    ovf=""; ovl=""; ovp=()
  }
  while IFS= read -r line || [ -n "$line" ]; do
    n=$((n + 1))
    line=${line%%#*}; line=$(echo "$line" | sed 's/[[:space:]]*$//; s/^[[:space:]]*//')
    [ -n "$line" ] || continue
    if [[ $line =~ ^\[(.*)\]$ ]]; then
      sect=${BASH_REMATCH[1]}
      case $sect in
        all|pi5) active=1 ;;
        cm4|cm5|pi0|pi0w|pi02|pi1|pi2|pi3|pi3+|pi4|pi400|pi500|cm4s|none|tryboot|board-type=*) active=0 ;;
        *) active=2 ;;   # conditional (HDMI, EDID, gpio...): can't fold what may not apply
      esac
      continue
    fi
    case $line in
      dtoverlay=*|dtparam=*) ;;
      *) continue ;;
    esac
    [ $active = 0 ] && continue
    [ $active = 2 ] && { say "line $n is in a conditional section: left alone"; return 0; }
    case $line in
      dtoverlay=)
        flush || return 0 ;;
      dtoverlay=*)
        flush || return 0
        IFS=, read -ra parts <<< "${line#dtoverlay=}"
        ovl=${parts[0]}; ovp=("${parts[@]:1}")
        ovf=$(overlay_file "$ovl") || { say "overlay $ovl: no file for a Pi 5: left alone"; return 0; }
        folded+=("$n") ;;
      dtparam=*)
        IFS=, read -ra parts <<< "${line#dtparam=}"
        if [ -n "$ovf" ] && try "$ovf" "${ovp[@]}" "${parts[@]}"; then
          # the pending overlay takes it (firmware: dtparam after dtoverlay belongs to the overlay)
          ovp+=("${parts[@]}")
        else
          flush || return 0
          # a base parameter; one the base doesn't know is ignored, as the firmware does
          step - "${parts[@]}" || say "base parameter ${parts[*]} unknown: ignored, as the firmware does"
        fi
        folded+=("$n") ;;
    esac
  done < "$C"
  flush || return 0
  [ ${#folded[@]} -gt 0 ] || { say "no overlays: nothing to merge"; return 0; }

  install -m 0755 "$tmp/cur.dtb" "$BOOT/$OUT.tmp" && mv "$BOOT/$OUT.tmp" "$BOOT/$OUT" || { say "can't write $OUT"; return 0; }
  local expr="" i
  for i in "${folded[@]}"; do expr+="${i}s/^/$MARK/;"; done
  sed -i "$expr" "$C"
  printf '\n%s\n[all]\n# The overlays marked %s are in this file (os/dt-merge.sh); undo: carpi-dt-merge undo\ndevice_tree=%s\n%s\n' \
    "$BEGIN" "${MARK% }" "$OUT" "$END" >> "$C"
  sync
  say "${#folded[@]} lines folded into $OUT"
}

case ${1:-} in
  merge) merge ;;
  undo) undo ;;
  *) sed -n '2,13p' "$0"; exit 2 ;;
esac
