#!/bin/bash
# One line of boot timing after a boot, on the Pi: firmware hand-off, systemd, and the uptime of
# each stage up to LIVI's UI and LIVI paging the phone (docs/BOOT.md). Power-on -> X = fw + X.
# Target: power-on -> paging under 10 s. Run as the car user (sudo for vclog).
#   bash boot-measure.sh            # this boot
fw=$(sudo vclog -m 2>/dev/null | grep -o 'Starting OS [0-9]*' | tail -1 | grep -o '[0-9]*$')
at() { # uptime (s) when a unit became active
  local u=$1; shift
  local v; v=$(systemctl "$@" show -p ActiveEnterTimestampMonotonic --value "$u" 2>/dev/null)
  [ -n "$v" ] && [ "$v" != 0 ] && printf '%.2f' "$(echo "$v/1000000" | bc -l)" || printf -- -
}
proc() { # uptime (s) when the first process matching $1 started
  local p; p=$(pgrep -o "$1") || { printf -- -; return; }
  awk -v t="$(getconf CLK_TCK)" '{printf "%.2f", $22/t}' "/proc/$p/stat"
}
logt() { # uptime (s) of the first journal line matching $2 from user unit $1
  journalctl -b --user -u "$1" -o short-monotonic --no-pager 2>/dev/null \
    | grep -m1 -- "$2" | sed -E 's/^\[ *([0-9.]+)\].*/\1/' | xargs printf '%.2f' 2>/dev/null || printf -- -
}
ui=$(logt livi.service 'LIVI connected')
# First page that reached the helper (a page before the helper's sockets are up fails: "not sent").
ap=$(journalctl -b --user -u livi.service -o short-monotonic --no-pager 2>/dev/null \
  | grep -E '\[devices\] (pages |paging list not sent)' \
  | awk '/pages /{if (t != "") {print t; exit} t=$2; sub(/\]/,"",t); next} /not sent/{t=""} END{if (t != "") print t}' \
  | head -1 | xargs printf '%.2f' 2>/dev/null || printf -- -)
printf 'fw=%s ' "$(printf '%.2f' "$(echo "${fw:-0}/1000" | bc -l)")"
printf 'kernel+user: %s | ' "$(systemd-analyze 2>/dev/null | head -1 | sed -E 's/.* in //; s/ = .*//')"
printf 'sysinit=%s nm=%s lightdm=%s user=%s labwc=%s livi=%s compositor=%s ui=%s ' \
  "$(at sysinit.target)" "$(at NetworkManager.service)" "$(at lightdm.service)" \
  "$(at user@1000.service)" "$(proc '^labwc$')" "$(proc '^livi-core$')" "$(proc '^livi-composito')" "$ui"
printf 'paging=%s ' "$ap"
[ "$ui" != - ] && [ -n "$fw" ] && printf '| power-on->ui=%.2f' "$(echo "$fw/1000+$ui" | bc -l)"
[ "$ap" != - ] && [ -n "$fw" ] && printf ' power-on->paging=%.2f (target <12)' "$(echo "$fw/1000+$ap" | bc -l)"
echo
awk '{printf "nvme reads=%d MB=%.0f read_ms=%d\n",$1,$3*512/1e6,$4}' /sys/block/nvme0n1/stat 2>/dev/null
systemctl --failed --no-legend --plain | head
