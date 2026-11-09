#!/usr/bin/env python3
"""Exercise deferred installer/firstboot wiring without root, services, or a real bundle."""
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
INSTALL = (ROOT / "os/install.sh").read_text()
FIRSTBOOT = (ROOT / "os/firstboot.sh").read_text()


def run(script, directory, *args, **env):
    return subprocess.run(
        [os.environ.get("BASH", "bash"), "-s", "--", str(directory), *args],
        input=script, text=True, capture_output=True,
        env={**os.environ, **{k: str(v) for k, v in env.items()}},
    )


with tempfile.TemporaryDirectory() as directory:
    root = Path(directory)
    updater = root / "updater"
    updater.write_text('#!/bin/bash\n[ "${UPD_HELP_EXIT:-0}" = 0 ] || exit 7\n'
                       'echo "${UPD_HELP:---defer-start}"\n')
    updater.chmod(0o755)

    # Exercise the real parser and real apply pipeline, preserving a preexisting current
    # pointer even on failure: its existence alone must never turn failure into success.
    parser = INSTALL[INSTALL.index("BUNDLE=\n"):INSTALL.index("\nsay()")]
    apply = INSTALL[INSTALL.index("# 5. The bundle."):]
    harness = '''set -euo pipefail
base=$1; shift
''' + parser + '''
say() { echo "$*"; }
die() { echo "$*" >&2; exit 1; }
as_user() { printf '%s\\n' "$@" > "$base/args"; echo 'updater result'; return "$APPLY_EXIT"; }
install() { :; }
CUR=$base/current GROUP=test
''' + apply
    (root / "current").symlink_to("releases/1.0.0")
    for deferred in (False, True):
        args = ["--bundle", "bundle.tar.zst", "--updater", "updater", "--user", "test"]
        if deferred:
            args.append("--defer-start")
        success = run(harness, root, *args, APPLY_EXIT=0)
        assert success.returncode == 0, success.stdout + success.stderr
        forwarded = (root / "args").read_text().splitlines()
        assert ("--defer-start" in forwarded) == deferred, forwarded
        assert "--file" in forwarded and "--allow-unsigned" in forwarded
        assert ("pending boot (health not checked)" in success.stdout) == deferred
        failure = run(harness, root, *args, APPLY_EXIT=42)
        assert failure.returncode != 0, failure.stdout + failure.stderr
        assert "apply failed" in failure.stderr
        assert "done:" not in failure.stdout and "staged:" not in failure.stdout

    # Check the installer's early guard, including dangling current and unsupported updater.
    guard_start = INSTALL.index('if [ ${#DEFER_START[@]} -gt 0 ]; then')
    guard = INSTALL[guard_start:INSTALL.index('\n# as_user CMD', guard_start)]
    preflight = '''set -euo pipefail
CUR=$1/current UPDATER=$1/updater DEFER_START=(--defer-start)
die() { echo "$*" >&2; exit 1; }
''' + guard
    existing = run(preflight, root)
    assert existing.returncode != 0 and "current already exists" in existing.stderr
    (root / "current").unlink()
    unsupported = run(preflight, root, UPD_HELP="legacy apply help")
    assert unsupported.returncode != 0 and "does not support" in unsupported.stderr
    assert run(preflight, root).returncode == 0

    # Run the actual firstboot tail through its DONE write. Mock only external commands.
    tail = FIRSTBOOT[FIRSTBOOT.index("# A newly embedded firstboot.sh"):]
    boot_harness = '''set -uo pipefail
WORK=$1 OS=$1 UPD=$1/updater DONE=$1/done BUNDLE=bundle CARPI_USER=pi SETUP=setup S=share
say() { echo "$*"; }
fail() { echo "FAILED: $*" >&2; exit 1; }
finish() { echo "$*"; }
bash() {
  if [ "$2" = --help ]; then echo "$INSTALL_HELP_TEXT"; return "${INSTALL_HELP_EXIT:-0}"; fi
  printf '%s\\n' "$@" > "$WORK/args"
  return "$APPLY_EXIT"
}
python3() { return "$POST_EXIT"; }
''' + tail
    for installer_new, updater_new in ((True, True), (True, False), (False, True), (False, False)):
        result = run(boot_harness, root, APPLY_EXIT=0, POST_EXIT=0,
                     INSTALL_HELP_TEXT="--defer-start" if installer_new else "legacy install help",
                     UPD_HELP="--defer-start" if updater_new else "legacy apply help")
        assert result.returncode == 0, result.stdout + result.stderr
        assert (root / "done").exists()
        forwarded = (root / "args").read_text().splitlines()
        assert ("--defer-start" in forwarded) == (installer_new and updater_new), forwarded
        (root / "done").unlink()

    for failure in ({"APPLY_EXIT": 42}, {"POST_EXIT": 42},
                    {"UPD_HELP_EXIT": 7}, {"INSTALL_HELP_EXIT": 7}):
        env = dict(APPLY_EXIT=0, POST_EXIT=0, INSTALL_HELP_TEXT="--defer-start")
        env.update(failure)
        result = run(boot_harness, root, **env)
        assert result.returncode != 0, result.stdout + result.stderr
        assert not (root / "done").exists()
        assert "rebooting" not in result.stdout

assert "condition: test -e /var/lib/carpi-firstboot.done" in (ROOT / "xtask/src/image.rs").read_text()
print("deferred startup: flags, fatal apply errors, fresh-only guard, legacy bundles, and DONE gate passed")
