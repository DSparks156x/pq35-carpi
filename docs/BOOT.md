# Boot time

Target: power-on to LIVI paging the phone (its access point is up and the phone gets the page) under 10 s. Measured on a Pi 5 booting from an NVMe (SK Hynix 256 GB on an M.2 ribbon board), Trixie, LIVI with wireless Android Auto, no display plugged in (avsd's headless screen).

Measure after a boot with [tools/boot-measure.sh](../tools/boot-measure.sh) on the Pi: firmware hand-off (`vclog`), then the uptime of each stage. Power-on to X = firmware + X. Compare boots from the same sitting; the drive varies between days.

## Results (2026-11-08)

| | firmware | kernel | LIVI starts | LIVI on screen | paging | power-on → screen | power-on → paging |
|---|---|---|---|---|---|---|---|
| Before (lightdm desktop) | 7.93 s | 1.28 s | 7.43 s | 8.15 s | 9.7 s | 16.1 s | 17.6 s |
| Firmware + command line | 5.90 s | 0.74 s | 6.9 s | 7.6 s | 9.6 s | 13.5 s | 15.5 s |
| netplan, user sessions, cloud-init, units | 5.90 s | 0.74 s | 3.65 s | 4.4 s | 6.6 s | 10.3 s | 12.5 s |
| LIVI's AP at boot, three LIVI hooks, kiosk session | 5.90 s | 0.74 s | 3.5 s | 4.15 s | 4.25 s | 10.05 s | 10.15 s |
| Overlays in one DTB | 5.67 s | 0.74 s | 3.5 s | 4.15 s | 4.26 s | 9.83 s | **9.93-9.99 s** |

## What's done, and where

All of it is in `os/profile.sh` step 10 and 12 unless noted, so a fresh install gets it.

**Firmware** (`config.txt`): `camera_auto_detect=0`, `display_auto_detect=0` (no I2C probing for a CSI camera or DSI panel), `auto_initramfs=0` (NVMe, ext4 and PCIe are built into the kernel; the 22 MB initramfs took 1.1 s to read), `kernel=kernel_2712_raw.img` (an uncompressed copy: no gunzip; `/etc/kernel/postinst.d/zz-carpi-raw-kernel` keeps it current and drops the line if it can't), `force_eeprom_read=0`. EEPROM `DISABLE_HDMI=1` was already set.

**Overlays folded into one DTB** ([os/dt-merge.sh](../os/dt-merge.sh), installed as `carpi-dt-merge`): the firmware applied each overlay at boot, ~50 ms each. `merge` builds `carpi-merged.dtb` from the base DTB and the active `dtoverlay=`/`dtparam=` lines with `dtmerge`, comments those lines with `#merged# ` and adds `device_tree=carpi-merged.dtb` in a marked block. `undo` puts config.txt back. Edit config.txt only between the two: profile.sh and options.py do; the kernel hook merges again after an update. Anything it can't fold (another board, overlays in a conditional section, a refused overlay) leaves config.txt plain. The firmware still applies `bcm2712d0` itself. Checked: the live device tree is the same as with the overlays, apart from phandle numbers.

**Kernel command line**: no splash (`plymouth.enable=0`), console on tty3, `loglevel=3 logo.nologo vt.global_cursor_default=0`: black from power-on until LIVI draws.

**NetworkManager**: it ran `netplan generate` (Python, udevadm, `systemctl daemon-reload`) up to three times at every start, ~2 s, and everything waited for it. The netplan Wi-Fi profiles cloud-init made become NM keyfiles (the YAML goes to `/var/lib/carpi/netplan`); new ones (the Updates page) are keyfiles already. `/usr/sbin/netplan` is dpkg-diverted to a wrapper that skips a bare `generate` when there's no YAML.

**User sessions don't wait for NetworkManager**: `/etc/systemd/system/systemd-user-sessions.service` is the packaged unit without `After=network.target`. LIVI runs its own access point.

**cloud-init** is off after the first boot (`/etc/cloud/cloud-init.disabled`): it was ~0.7 s on the critical path.

**Units off**: masked `rp1-test`, `sshswitch`, `glamor-test`, `systemd-binfmt`, the wait-online units (nothing needs `network-online`; they held the boot a minute); disabled CUPS, rpcbind, NFS.

**Kiosk session** (`carpi-session.service`, `Alias=display-manager.service`): labwc on tty7 for the car user, started by systemd with a PAM session (`/etc/pam.d/carpi-session`, the session part of lightdm-autologin) so logind has an active wayland session on seat0. Its config is `/etc/carpi/labwc`: no panel, file manager, wallpaper or Xsession scripts. `LABWC_FALLBACK_OUTPUT=NOOP-1` gives avsd its headless screen when nothing is plugged in. lightdm stays installed: `systemctl disable carpi-session && systemctl enable --force lightdm` brings the desktop back. firstboot.sh no longer turns on lightdm autologin.

**LIVI**:
- `deploy/livi.service` (and wayvnc's) wait for the wayland socket in 0.05 s steps, not 1 s.
- `wifiDedicatedInterface` on (options.py seeds it): `livi-wifi-ap.service` is enabled, so the access point comes up at boot alongside everything else instead of after LIVI.
- Three hooks in the fork (`livi/`, branch `carpi`):
  - `wifi_ap.rs`: `systemctl enable/disable` only when the state differs (each one reloads systemd, 0.9 s at boot).
  - `wifi_ap.rs`: restart the AP only when LIVI just installed a newer helper. Its "older helper" check compares the helper's mtime with the wall-clock boot time, which is wrong at boot in a car (no network time yet), so it restarted the AP at every boot: 1.3 s.
  - `iap2-mfi/linux.rs`: no `/dev/i2c-N`, no MFi chip, at once (it spun its 2 s probe timeout, and the BlueZ profiles waited for it).
- The hotspot country: LIVI defaults to DE. options.py seeds `country` from the setup page's country (now asked in every network mode).

## Where the ~9.9 s go now

| | |
|---|---|
| 0-4.0 s | bootloader, DRAM training (fixed cost) |
| 4.0-4.3 s | voltage calibration (fixed) |
| 4.3-4.7 s | DTB, `bcm2712d0`, cmdline |
| 4.7-5.7 s | reading the 25 MB kernel off the NVMe (~25 MB/s) |
| +0.74 s | kernel |
| +2.4 s uptime | sysinit |
| +2.55 s | labwc starts (its socket at ~3.45 s: EGL, headless output, ~0.5 s of PAM waiting for the user manager) |
| +3.5 s | LIVI starts; UI at 4.15 s |
| +2.8 s | AP unit starts; up at ~4.3 s (claim 0.6 s, hostapd 0.85 s) |
| +4.26 s | paging |

## Not done / ideas
- **Boot partition clusters**: the firmware reads the kernel at ~25 MB/s from FAT32 with 4 KB clusters. FAT16 with 32 KB clusters may read faster. Needs a reformat of the boot partition (back it up; the SD card is the rescue). Not tried.
- **AP bring-up** (LIVI helper): ~0.6 s claiming wlan0, ~0.85 s to the AP being up (the 40 MHz neighbour scan is part of it; `noscan=1` in the hostapd config would skip it).
- **labwc**: ~0.5 s of EGL and output setup before its socket; Xwayland and desktop-entry loading are small.
- **Desktop on leaving LIVI**: start pcmanfm/panel when LIVI quits.
- The swap file (`rpi-swap`, 2 GB on the NVMe) could be zram only (`/etc/rpi/swap.conf Mechanism=zram`); not on the critical path.
