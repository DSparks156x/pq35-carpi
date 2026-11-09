#!/usr/bin/env python3
"""Exercise firstboot's clock gate with mocked system commands; never touches system time."""
from pathlib import Path
import subprocess
import os


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "os" / "firstboot.sh"
source = SCRIPT.read_text()
start = source.index("wait_for_clock_sync() {")
end = source.index("\n}\n", start) + 3
function = source[start:end]


def run(mode: str) -> subprocess.CompletedProcess[str]:
    harness = f"""
calls=0
ntp_enabled=0
start_calls=0
sleep_calls=0
timedatectl() {{
  case "$*" in
    'show -p NTPSynchronized --value')
      calls=$((calls + 1))
      if [ {mode!r} = already ] || {{ [ {mode!r} = delayed ] && [ "$sleep_calls" -ge 2 ]; }}; then echo yes; else echo no; fi
      ;;
    'set-ntp true') ntp_enabled=$((ntp_enabled + 1));;
  esac
}}
systemctl() {{
  case "$*" in
    'list-unit-files systemd-timesyncd.service --no-legend') echo systemd-timesyncd.service enabled;;
    'start systemd-timesyncd.service') start_calls=$((start_calls + 1));;
  esac
}}
sleep() {{ sleep_calls=$((sleep_calls + 1)); }}
say() {{ printf '%s\\n' "$*"; }}
{function}
CARPI_CLOCK_SYNC_TIMEOUT=6 CARPI_CLOCK_SYNC_INTERVAL=3 wait_for_clock_sync
result=$?
printf 'RESULT=%s NTP=%s START=%s SLEEPS=%s\\n' "$result" "$ntp_enabled" "$start_calls" "$sleep_calls"
exit "$result"
"""
    return subprocess.run([os.environ.get("BASH", "bash"), "-s"], input=harness, text=True, capture_output=True)


already = run("already")
assert already.returncode == 0, already.stdout + already.stderr
assert "RESULT=0 NTP=0 START=0 SLEEPS=0" in already.stdout

delayed = run("delayed")
assert delayed.returncode == 0, delayed.stdout + delayed.stderr
assert "RESULT=0 NTP=1 START=1 SLEEPS=2" in delayed.stdout

timeout = run("timeout")
assert timeout.returncode == 1, timeout.stdout + timeout.stderr
assert "did not synchronize after 6s" in timeout.stdout
assert "check network access and NTP service" in timeout.stdout
assert "RESULT=1 NTP=1 START=1 SLEEPS=2" in timeout.stdout

print("firstboot clock gate: already synced, delayed sync, and timeout passed")
