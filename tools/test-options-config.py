#!/usr/bin/env python3
"""Check first-install display defaults and preservation without touching the device."""
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import types
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.modules.setdefault("pwd", types.ModuleType("pwd"))
spec = importlib.util.spec_from_file_location("options", ROOT / "os/options.py")
options = importlib.util.module_from_spec(spec)
spec.loader.exec_module(options)

with tempfile.TemporaryDirectory() as directory, patch.object(options.os, "chown", create=True):
    root = Path(directory)
    (root / "config").mkdir()
    (root / "config/carpid.toml").write_text((ROOT / "deploy/carpid.toml").read_text())
    for video in ("sdlink", "edid480i", "edid480p", None):
        user = types.SimpleNamespace(pw_dir=str(root / str(video)), pw_uid=0, pw_gid=0)
        options.apply_user_config({"video": video}, str(root), user)
        path = Path(user.pw_dir) / ".config/LIVI/config.json"
        config = json.loads(path.read_text())
        assert config["kiosk"] == {"main": True, "cluster": False, "aux": False}
        if video:
            assert tuple(config[k] for k in (
                "mainScreenWidth", "mainScreenHeight", "projectionWidth", "projectionHeight"
            )) == (800, 480, 800, 480), video
        else:
            assert "mainScreenWidth" not in config and "projectionWidth" not in config
        config["mainScreenWidth"] = 999
        config["kiosk"]["main"] = False
        path.write_text(json.dumps(config))
        before = path.read_bytes()
        options.apply_user_config({"video": video}, str(root), user)
        assert path.read_bytes() == before, "rerun overwrote existing settings"

print("480 display defaults, fullscreen, leave-it and existing-settings checks passed")
