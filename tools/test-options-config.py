#!/usr/bin/env python3
"""Check first-install display defaults and preservation without touching the device."""
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import types
import tomllib
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

# Exercise real deployment defaults and preserve the chosen diagnostic routing.
defaults = (ROOT / "deploy/carpid.toml").read_text()
for can, expected in [
    ({}, None),
    ({"ican_is_diag": True}, "ican"),
    ({"usb_diag": True}, "candiag"),
    ({"ican_is_diag": True, "usb_diag": True}, "candiag"),
]:
    config = tomllib.loads(options.carpid_toml({"can": can}, defaults))
    assert config["bus"]["ican"]["iface"] == "ican"
    assert config["bus"].get("diag", {}).get("iface") == expected

writes = {}
with patch.object(options, "write", side_effect=lambda p, text: writes.update({p: text})):
    options.apply_usb_can({"can": {"usb_diag": True}})
assert "Driver=gs_usb\n" in writes["/etc/systemd/network/70-carpi-candiag.link"]
assert "Name=candiag\n" in writes["/etc/systemd/network/70-carpi-candiag.link"]
assert "BitRate=500000\n" in writes["/etc/systemd/network/80-carpi-candiag.network"]
print("stable CAN names, disabled/shared/separate diag and USB priority checks passed")
