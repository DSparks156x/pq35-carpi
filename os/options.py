#!/usr/bin/env python3
"""The image tool's answers (setup.json) applied to a profiled Pi. docs/IMAGE-TOOL.md.

    sudo os/options.py --setup setup.json --share DIR --user pi --phase pre|post

pre (before install.sh): config.txt, cmdline.txt (EDID), the USB CAN adapter's network
files, carpid.toml and LIVI's config.json when they don't exist yet, the USB DAC rule.
post (after install.sh): which user units run (VNC, the A/V-over-SD chain).

Only what setup.json asks for is touched; every choice has a "leave it" answer. User
config that already exists is never overwritten. Safe to re-run.
"""

import argparse
import json
import os
import pwd
import re
import shutil
import subprocess
import sys

CONFIG = "/boot/firmware/config.txt"
CMDLINE = "/boot/firmware/cmdline.txt"
BEGIN = "# >>> carpi options (os/options.py; edits between these lines are replaced)"
END = "# <<< carpi options"

# DAC hats: their config.txt lines. All turn the Pi's own audio off.
DACS = {
    "carpihat-pro": ["dtoverlay=hifiberry-dacplus-std"],
    "hifiberry-dacplus": ["dtoverlay=hifiberry-dacplus"],
    "innomaker": ["dtoverlay=allo-boss-dac-pcm512x-audio"],
}
EDIDS = {"edid480i": "rnse-800x480i.bin", "edid480p": "rnse-800x480p60.bin"}

USB_DAC_RULE = """# carpi: a USB DAC is the default output (os/options.py).
monitor.alsa.rules = [
  {
    matches = [ { node.name = "~alsa_output.usb-.*" } ]
    actions = { update-props = { priority.session = 2000, priority.driver = 2000 } }
  }
]
"""


def say(msg):
    print(f"options: {msg}", flush=True)


def validate_setup(s):
    """Reject mutually exclusive USB CAN roles before any setup files are changed."""
    can = s.get("can") or {}
    if can.get("usb_diag") and can.get("usb_ican"):
        raise ValueError("can.usb_diag and can.usb_ican cannot both be enabled")
    if can.get("usb_ican") and can.get("mcp2515"):
        raise ValueError("can.usb_ican and can.mcp2515 both claim the ican interface")


def read(path):
    with open(path, encoding="utf-8") as f:
        return f.read()


def write(path, text, owner=None, mode=0o644):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    tmp = path + ".carpi-tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        f.write(text)
    os.chmod(tmp, mode)
    if owner:
        os.chown(tmp, owner.pw_uid, owner.pw_gid)
    os.replace(tmp, path)
    say(f"wrote {path}")


def config_lines(s):
    """config.txt lines for the hat, RTC, DAC and display choice."""
    out = []
    can = s.get("can") or {}
    spi = can.get("mcp2515")
    if spi:
        out += [
            "dtparam=spi=on",
            f"dtoverlay=mcp2515-can0,oscillator={int(spi['oscillator'])},interrupt={int(spi['interrupt'])}",
        ]
    hat = s.get("hat") or {}
    if hat.get("power_latch_gpio") is not None:
        g = int(hat["power_latch_gpio"])
        out += [f"gpio={g}=op,dh", f"dtoverlay=gpio-poweroff,gpiopin={g},active_low"]
    if hat.get("usb_max_current"):
        out.append("usb_max_current_enable=1")
    rtc = hat.get("rtc")
    if rtc in ("ds1307", "ds3231"):
        out += ["dtparam=i2c_arm=on", f"dtoverlay=i2c-rtc,{rtc}"]
    dac = DACS.get(s.get("audio"))
    if dac:
        out += dac
    if s.get("video") is None:
        # A profile may disable auto-detect. Restore it for the explicit Leave it choice.
        out.append("display_auto_detect=1")
    return out


DT_MERGE = "/usr/local/sbin/carpi-dt-merge"


def dt_merge(action):
    """os/dt-merge.sh (profile.sh installs it): edit config.txt plain, fold the overlays after."""
    if os.access(DT_MERGE, os.X_OK):
        subprocess.run([DT_MERGE, action], check=False)


def apply_config_txt(s):
    dt_merge("undo")
    try:
        edit_config_txt(s)
    finally:
        dt_merge("merge")


def edit_config_txt(s):
    text = read(CONFIG)
    # Our block, replaced whole.
    text = re.sub(re.escape(BEGIN) + r".*?" + re.escape(END) + r"\n?", "", text, flags=re.S)
    lines = config_lines(s)
    if s.get("audio") in DACS:
        # The DAC replaces the Pi's own audio, HDMI's included.
        text = re.sub(r"^(dtparam=audio=on)\s*$", r"#\1  # carpi: a DAC hat", text, flags=re.M)
        text = re.sub(r"^dtoverlay=vc4-kms-v3d\s*$", "dtoverlay=vc4-kms-v3d,noaudio", text, flags=re.M)
    if lines:
        text = text.rstrip("\n") + "\n\n[all]\n" + BEGIN + "\n" + "\n".join(lines) + "\n" + END + "\n"
    if text != read(CONFIG):
        shutil.copy2(CONFIG, CONFIG + ".carpi-bak")
        write(CONFIG, text)


def apply_edid(s, share):
    video = s.get("video")
    cmd = read(CMDLINE).strip()
    cmd = re.sub(r"\s*drm\.edid_firmware=\S+", "", cmd)
    name = EDIDS.get(video)
    if name:
        src = os.path.join(share, "os", "edid", name)
        dst = os.path.join("/lib/firmware/edid", name)
        os.makedirs(os.path.dirname(dst), exist_ok=True)
        shutil.copy2(src, dst)
        say(f"EDID {name}")
        cmd += f" drm.edid_firmware=HDMI-A-1:edid/{name}"
    if cmd + "\n" != read(CMDLINE):
        write(CMDLINE, cmd + "\n")


def apply_usb_can(s):
    can = s.get("can") or {}
    validate_setup(s)
    if can.get("usb_ican"):
        name, bitrate = "ican", 100000
    elif can.get("usb_diag"):
        name, bitrate = "candiag", 500000
    else:
        return
    link = (
        "/etc/systemd/network/70-carpi-usb-ican.link"
        if name == "ican"
        else "/etc/systemd/network/70-carpi-candiag.link"
    )
    net = (
        "/etc/systemd/network/80-carpi-usb-ican.network"
        if name == "ican"
        else "/etc/systemd/network/80-carpi-candiag.network"
    )
    write(link, f"[Match]\nDriver=gs_usb\n\n[Link]\nName={name}\nTransmitQueueLength=100\n")
    write(
        net,
        f"[Match]\nName={name}\n\n[CAN]\nBitRate={bitrate}\n\n[Link]\nRequiredForOnline=no\n",
    )


def edit_section(text, section, key, value):
    """Sets `key = value` in `[section]` of a TOML text we know the shape of."""
    pat = re.compile(r"(^\[" + re.escape(section) + r"\]\n(?:(?!^\[).*\n)*?)" + r"^" + re.escape(key) + r"\s*=.*$", re.M)
    if pat.search(text):
        return pat.sub(lambda m: m.group(1) + f"{key} = {value}", text, count=1)
    head = re.compile(r"^\[" + re.escape(section) + r"\]\n", re.M)
    if head.search(text):
        return head.sub(lambda m: m.group(0) + f"{key} = {value}\n", text, count=1)
    return text.rstrip("\n") + f"\n\n[{section}]\n{key} = {value}\n"


def drop_section(text, section):
    return re.sub(r"(?:^#.*\n)*^\[" + re.escape(section) + r"\]\n(?:(?!^\[).*\n?)*", "", text, flags=re.M)


def carpid_toml(s, default_text):
    t = default_text
    can = s.get("can") or {}
    t = edit_section(t, "bus.ican", "iface", '"ican"')
    if can.get("usb_diag"):
        t = edit_section(t, "bus.diag", "iface", '"candiag"')
    elif can.get("ican_is_diag"):
        t = edit_section(t, "bus.diag", "iface", '"ican"')
    else:
        t = drop_section(t, "bus.diag")
    t = edit_section(t, "dis", "enabled", "true" if s.get("dis", True) else "false")
    if s.get("car_mode", True):
        # Send to the car and power off when the bus sleeps: what a car install wants.
        t = edit_section(t, "tx", "enabled", "true")
        t = edit_section(t, "power", "shutdown", "true")
    release = s.get("release") or {}
    channel = release.get("channel")
    if channel in ("stable", "beta"):
        t = edit_section(t, "update", "channel", f'"{channel}"')
    # The repo the card installed from keeps the updates coming (a fork's, say).
    repo = release.get("repo")
    if isinstance(repo, str) and re.fullmatch(r"[A-Za-z0-9-]+/[A-Za-z0-9._-]+", repo):
        t = edit_section(t, "update", "repo", f'"{repo}"')
    return t


def apply_user_config(s, share, user):
    home = user.pw_dir
    cfg = os.path.join(home, ".config/carpi/carpid.toml")
    if not os.path.exists(cfg):
        write(cfg, carpid_toml(s, read(os.path.join(share, "config/carpid.toml"))), owner=user)
        os.chown(os.path.dirname(cfg), user.pw_uid, user.pw_gid)
    livi = os.path.join(home, ".config/LIVI/config.json")
    if not os.path.exists(livi):
        # wifiDedicatedInterface: LIVI's access point is a boot unit, up before LIVI starts, so the
        # phone is paged sooner (docs/BOOT.md). LIVI's own default is DE for the hotspot country.
        seed = {
            "bindings": {"knobLeft": "PageUp", "knobRight": "PageDown"},
            "wifiDedicatedInterface": True,
            "kiosk": {"main": True, "cluster": False, "aux": False},
        }
        if s.get("video") in ("sdlink", "edid480i", "edid480p"):
            seed.update(
                mainScreenWidth=800, mainScreenHeight=480,
                projectionWidth=800, projectionHeight=480,
            )
        if re.fullmatch(r"[A-Z]{2}", s.get("country") or ""):
            seed["country"] = s["country"]
        hot = s.get("hotspot") or {}
        if hot.get("name"):
            seed["carName"] = hot["name"]
        if hot.get("password"):
            seed["wifiPassword"] = hot["password"]
        write(livi, json.dumps(seed, indent=2) + "\n", owner=user, mode=0o600)
        os.chown(os.path.dirname(livi), user.pw_uid, user.pw_gid)
    if s.get("audio") == "usb":
        rule = os.path.join(home, ".config/wireplumber/wireplumber.conf.d/51-carpi-usb-dac.conf")
        write(rule, USB_DAC_RULE, owner=user)


def user_systemctl(user, *args):
    env = dict(os.environ, XDG_RUNTIME_DIR=f"/run/user/{user.pw_uid}")
    return subprocess.run(
        ["sudo", "-u", user.pw_name, "env", f"XDG_RUNTIME_DIR=/run/user/{user.pw_uid}", "systemctl", "--user", *args],
        env=env,
        check=False,
    ).returncode


def apply_units(s, user):
    vnc = s.get("vnc_at_boot", True)
    user_systemctl(user, "enable" if vnc else "disable", "wayvnc.service")
    say(f"VNC at startup: {'on' if vnc else 'off'}")
    sdlink = s.get("video") == "sdlink" or s.get("audio") == "sdlink"
    user_systemctl(user, "enable" if sdlink else "disable", "avsd-chain.service")
    subprocess.run(["systemctl", "enable" if sdlink else "disable", "avsd-governor.service"], check=False)
    say(f"A/V over SD: {'on' if sdlink else 'off'}")


def scrub(path, s):
    """The hotspot password has reached LIVI's config; it leaves the boot partition."""
    hot = s.get("hotspot") or {}
    if hot.get("password"):
        hot["password"] = ""
        with open(path, "w", encoding="utf-8") as f:
            json.dump(s, f, indent=2)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--setup", required=True)
    ap.add_argument("--share", required=True, help="the bundle's share/ folder")
    ap.add_argument("--user", required=True)
    ap.add_argument("--phase", choices=["pre", "post"], required=True)
    a = ap.parse_args()
    if os.geteuid() != 0:
        sys.exit("options: run as root")
    s = json.loads(read(a.setup))
    if s.get("version") != 1:
        sys.exit(f"options: setup.json version {s.get('version')!r}, this knows 1")
    try:
        validate_setup(s)
    except ValueError as e:
        sys.exit(f"options: {e}")
    user = pwd.getpwnam(a.user)
    if a.phase == "pre":
        apply_config_txt(s)
        apply_edid(s, a.share)
        apply_usb_can(s)
        apply_user_config(s, a.share, user)
        scrub(a.setup, s)
    else:
        apply_units(s, user)
    say(f"{a.phase} done")


if __name__ == "__main__":
    main()
