#!/usr/bin/env python3
"""Exercise the installer SDLink seed and preservation without root or systemd."""
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
source = (ROOT / "os/install.sh").read_text()
start = source.index('if [ ! -e "$AV/avsd.env" ]; then')
block = source[start:source.index('\nif [ ! -e /etc/systemd/system/avsd-governor.service', start)]

with tempfile.TemporaryDirectory() as directory:
    root = Path(directory)
    share = root / "share"
    (share / "sdlink/run").mkdir(parents=True)
    # Same generic defaults SDLink ships; rnse-carpi must select LIVI itself.
    (share / "sdlink/run/avsd.env").write_text(
        'GAIN=-3\n#APP_PROC=livi-core\n#CAP_SINK=avsd_out\n#CAP_SINK_CREATE=1\n'
    )
    avsd = root / "avsd"
    avsd.mkdir()
    script = '''set -eu
AV=$1 S=$2
put_missing() { [ -e "$2" ] || cp "$1" "$2"; }
''' + block + '''
. "$AV/avsd.env"
printf '%s|%s|%s|%s' "$APP_PROC" "$CAP_SINK" "$CAP_SINK_CREATE" "$GAIN"
'''

    def run():
        return subprocess.run(
            [os.environ.get("BASH", "bash"), "-s", "--", avsd.as_posix(), share.as_posix()],
            input=script, text=True, capture_output=True, check=True,
        ).stdout

    assert run() == 'livi-core|avsd_out|1|-3'
    path = avsd / "avsd.env"
    original = path.read_bytes()
    assert run() == 'livi-core|avsd_out|1|-3'
    assert path.read_bytes() == original
    path.write_text('APP_PROC=custom\nCAP_SINK=my_sink\nCAP_SINK_CREATE=0\nGAIN=-9\n')
    assert run() == 'custom|my_sink|0|-9'

print("SDLink fresh install selects LIVI; reruns preserve existing settings")
