#!/usr/bin/env python3
"""Exercise firstboot's lookup with deliberately unordered API results."""
import contextlib
import io
import json
from pathlib import Path
from unittest.mock import patch

source = (Path(__file__).resolve().parents[1] / "os/firstboot.sh").read_text()
lookup = source.split('import json, sys, urllib.request\n', 1)[1].split('\nEOF', 1)[0]
lookup = 'import json, sys, urllib.request\n' + lookup


def release(tag, published, prerelease=True, draft=False, assets=True):
    return dict(tag_name=tag, published_at=published, prerelease=prerelease,
                draft=draft, assets=[dict(name=f"carpi-{tag}-arm64.tar.zst",
                                         browser_download_url=tag)] if assets else [])


releases = [release("old", "2026-11-09T09:00:00Z"),
            release("new", "2026-11-09T11:00:00Z"),
            release("draft", "2026-11-09T14:00:00Z", draft=True),
            release("notes", "2026-11-09T15:00:00Z", assets=False),
            release("stable", "2026-11-09T08:00:00Z", prerelease=False)]


def select(channel, version=""):
    output = io.StringIO()
    with patch("sys.argv", ["lookup", "owner/repo", channel, version]), \
            patch("urllib.request.urlopen", return_value=io.StringIO(json.dumps(releases))), \
            contextlib.redirect_stdout(output):
        try:
            exec(compile(lookup, "firstboot-release", "exec"), {})
        except SystemExit as done:
            assert done.code == 0, done
    return output.getvalue().strip()


assert select("beta") == "new"
assert select("stable") == "stable"
assert select("beta", "old") == "old"
print("firstboot releases: ordering, channels, drafts, assets and pin passed")
