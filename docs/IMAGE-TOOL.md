# Image tool (plan, Drew 2026-11-08)

A tool that turns a stock Raspberry Pi OS Trixie card into a carpi: it asks how this car is set up, writes that onto the card's boot partition, and first boot does the rest. It grows out of `cargo xtask image` (DEPLOY.md section 7) into its own repo. First use: install on Drew's NVMe, cut the boot time back to Hudiy's, then fold the boot-time work back into the tool.

## Shape (Drew: multi-platform, a web app would be "glorious")

- **A web page**, built from `rnse-carpi-image` by Netlify and served at `cowpowersystems.com/carpi/`. Nothing to install, every OS.
- **Flashing stays with Raspberry Pi Imager** (stock Trixie 64-bit with the desktop, no customisation; Lite is not supported yet). The page then writes its few files onto the card's boot partition: in Chrome and Edge straight onto the mounted drive (File System Access API); elsewhere it hands over a zip to copy onto it.
- **The page carries no bundle.** First boot downloads the chosen release (stable, beta or an exact version) from rnse-carpi's GitHub releases: it already needs the internet for packages. So the page stays small and never fights GitHub's download rules.
- **Presets**: the answers save as a file (`my-tt.json`) to load next time.
- **What it writes**: cloud-init `user-data`, `network-config`, `meta-data`, and `carpi/`: `setup.json` (the answers), `firstboot.sh`. firstboot fetches the bundle, then runs `profile.sh` and `install.sh` from it with the answers.
- The repo's Action also builds a command-line version of the same writer (from `xtask image`) for scripting.

## Questions it asks

### CAN bus
- **SPI MCP2515 hat**: CarPiHat preset (8 MHz, interrupt 23, `spi-bcm2835`, power latch `gpio-poweroff` on GPIO 25), or custom oscillator and interrupt pin. Role: ICAN (default); ICAN + diag on the same bus (default no).
- **USB CAN as diag**: candleLight / gs_usb adapters. A udev rule names it, networkd brings it up at 500 kbit/s, carpid gets `[bus.diag]`.
- **Nothing**.

### Video
1. **480i60 EDID** for an HDMI-to-analog converter (`Rpi_RNSE_800x480i_EDID.bin`).
2. **480p60 EDID** (`Rpi_RNSE_800x480p60_EDID.bin`, 800x480 progressive, 32 MHz): needs Drew's RNS-E firmware; the page says so.
3. **SDLink**: A/V over SD; video and audio together.
4. **Leave it**.

EDIDs go to `/lib/firmware/edid/` and `drm.edid_firmware=HDMI-A-1:edid/<file>` in `cmdline.txt` (KMS).
SDLink and both 480 EDID choices seed LIVI's main display and phone projection at 800x480.
New installs open the main display fullscreen; existing LIVI settings are preserved.

### Audio
1. **USB DAC**: made the default output.
2. **DAC hats**: CarPiHat Pro (`hifiberry-dacplus-std`, onboard audio off, `vc4-kms-v3d,noaudio`), HiFiBerry DAC+ / DAC2, InnoMaker HiFi DAC.
3. **SDLink**: goes with SDLink video.
4. **Leave it**.

For a new install, `install.sh` selects LIVI in `~/avsd/avsd.env`:
`APP_PROC=livi-core`, `CAP_SINK=avsd_out`, and `CAP_SINK_CREATE=1`.
SDLink's generic template defaults to Hudiy; copying it unchanged leaves the chain waiting
for a process this system does not run. Existing environment files are preserved.

### Hat extras
- **RTC**: CarPiHat's clock (`i2c-rtc,ds1307`; DS3231 on v0.3 boards), so the time is right before the car's clock arrives.

### DIS
On or off from the start (`[dis] enabled`); still a setting afterwards.

### Phone
- LIVI's hotspot name and password, so the phone pairs on first boot.

### The Pi
- User name, password (hashed in the page; VNC and the console need one; ssh is key-only), ssh public key.
- Hostname, time zone (the car clock is local).
- Wi-Fi for first boot (it needs the internet), typed or as a netplan file.
- Bundle: stable, beta or an exact version.

### Touch screen
Not asked: detected on the fly. The on-screen keyboard defaults to touch when a touch screen is attached.

## NVMe and post-setup (Drew: most people have no M.2 USB enclosure)

The page finishes an SD card. Everything after first boot happens on the head unit, in a **Setup** screen at the top of Car Integration that shows until "Done, hide this" (saved in `carpi-ui.json`; Advanced has "Show Setup again"). Its rows open the existing screens, and back from them returns to Setup: Time zone, Wi-Fi networks, Phone connection (wireless Android Auto / CarPlay), Cluster display, Remote access (VNC at startup), and Move to NVMe. Phone connection shows the configured wireless state and opens LIVI's Connections settings; Back returns to Setup. Each shows its current state.

- **Move to NVMe**: in Setup and in Advanced, only when an NVMe drive is fitted and `/` isn't on it. The screen shows the drive, where the system runs from and how much it copies, warns that the NVMe is erased, and starts on the second push. It wipes `/dev/nvme0n1` (MBR like Pi OS: 512 MB FAT32 boot, ext4 root filling the rest), rsyncs `/` and `/boot/firmware` across, puts the new PARTUUIDs in the copy's `cmdline.txt` and `fstab`, sets `BOOT_ORDER=0xf416` (NVMe, SD, USB; plus `PCIE_PROBE=1` when config.txt has `dtparam=pciex1`), and reboots. Everything set up so far comes along. The SD's partitions are left alone: with it in, the Pi boots the NVMe; without the NVMe it boots the SD. Re-running starts over. It refuses when `/` is on the NVMe, there's no NVMe, it's smaller than what's in use plus 10 %, or it's over 2 TB.
- How: `os/move-nvme.sh`, run as root by `carpi-move-nvme.service` (installed by `install.sh`, never enabled). carpid starts it through a polkit rule from the profile (`Request::Storage`, `move_to_nvme`) and relays the progress file the script writes (`/run/carpi-move-nvme/progress`) as job events; a run that stops without a result fails the job and points at `journalctl -u carpi-move-nvme`. The hotspot stays up; the phone can't be used meanwhile and the screen says so. `sudo os/move-nvme.sh --no-reboot` does the same by hand.
- Tested on the Pi: SD-to-NVMe copy and reboot succeeded on 2026-11-09. The UI initially reported a false failure because `systemctl is-active` exits nonzero while the oneshot is activating. The daemon now queries `ActiveState` directly; regression coverage checks an activating copy and a genuinely failed service.

## Remote access (Car Integration tab)

A Remote access screen: the file portal's address; Start VNC now; VNC at startup (on by default for setup; off makes boot faster). ssh is always on. carpid starts, stops, enables and disables the wayvnc user unit through the settings API.

## Order

1. Firstboot that fetches the bundle itself and reads `setup.json`; the options as profile/install steps.
2. The web page, its preset files and writer; the Action that publishes it (and the CLI).
3. Remote access screen (rnse-carpi + fork). Done.
4. Setup screen and Move to NVMe. Copy and reboot verified on a Pi; progress reporting corrected after that run.
5. Drew's NVMe through that path; grind the boot time; the trimming becomes a profile step and an option.

## Reference

- CarPiHat: github.com/SamT-J/CarPiHat/wiki/Quick-Start-Guide. Ignition sense GPIO 12, illumination 13, reverse 7 (shares CS1: `spi0-cs,cs1_pin=24` if it fights CAN), outputs 27 and 22.
