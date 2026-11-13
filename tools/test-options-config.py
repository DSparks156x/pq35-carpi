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

# A profile may disable auto-detect globally; the image's Leave it choice re-enables it
# in the managed [all] block. Re-running replaces the block instead of duplicating it.
profile_config = "# profile\n[all]\ndisplay_auto_detect=0\ndtparam=audio=on\n"
with tempfile.TemporaryDirectory() as directory:
    config = Path(directory) / "config.txt"
    config.write_text(profile_config)
    with patch.object(options, "CONFIG", str(config)), patch.object(options, "dt_merge"):
        options.apply_config_txt({"video": None})
        text = config.read_text()
        assert "display_auto_detect=0" in text
        assert "[all]\n# >>> carpi options" in text
        assert text.count("display_auto_detect=1") == 1
        assert text.index("display_auto_detect=1") > text.index("display_auto_detect=0")
        options.apply_config_txt({"video": None})
        assert config.read_text().count("display_auto_detect=1") == 1
        options.apply_config_txt({"video": "edid480p"})
        assert "display_auto_detect=1" not in config.read_text()

assert "display_auto_detect=1" in options.config_lines({"video": None})
assert "display_auto_detect=1" not in options.config_lines({"video": "sdlink"})
print("Leave it display auto-detect override, rerun and explicit-mode checks passed")

# Exercise real deployment defaults and preserve the chosen diagnostic routing.
defaults = (ROOT / "deploy/carpid.toml").read_text()
for can, expected in [
    ({}, None),
    ({"ican_is_diag": True}, "ican"),
    ({"usb_diag": True}, "candiag"),
    ({"ican_is_diag": True, "usb_diag": True}, "candiag"),
    ({"ican_is_diag": True, "usb_ican": True}, "ican"),
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
writes.clear()
with patch.object(options, "write", side_effect=lambda p, text: writes.update({p: text})):
    options.apply_usb_can({"can": {"usb_ican": True}})
assert "Driver=gs_usb\n" in writes["/etc/systemd/network/70-carpi-usb-ican.link"]
assert "Name=ican\n" in writes["/etc/systemd/network/70-carpi-usb-ican.link"]
assert "BitRate=100000\n" in writes["/etc/systemd/network/80-carpi-usb-ican.network"]
assert "/etc/systemd/network/70-carpi-ican.link" not in writes, "preserved profile MCP251x link"

for conflicting_can in (
    {"usb_diag": True, "usb_ican": True},
    {"usb_ican": True, "mcp2515": {"oscillator": 8000000, "interrupt": 23}},
):
    writes.clear()
    try:
        options.validate_setup({"can": conflicting_can})
    except ValueError:
        pass
    else:
        raise AssertionError(f"accepted conflicting CAN setup: {conflicting_can}")
    with patch.object(options, "write", side_effect=lambda p, text: writes.update({p: text})):
        try:
            options.apply_usb_can({"can": conflicting_can})
        except ValueError:
            pass
        else:
            raise AssertionError(f"applied conflicting CAN setup: {conflicting_can}")
    assert not writes, "conflicting setup wrote CAN network files"

print("stable CAN names, USB roles, diag sharing and conflicting-role rejection checks passed")
