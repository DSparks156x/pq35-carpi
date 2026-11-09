# Build, release, update

Status: **draft for Drew.** Sections 1, 2 and 6 are written (`docs/BUNDLE.md`, `scripts/bundle.sh`, `.github/workflows/release.yml`, `os/profile.sh`) but have not had a CI run yet. Sections 3 to 5 are written (`crates/carpi-update`, carpid's update jobs, the fork's AP hook, `xtask`) and tested off the Pi only.

Goals (Drew, 2026-11-07):
- No building on the Pi and none on Windows. GitHub Actions builds, with caches kept so builds are quick.
- A direct deploy from the PC for development.
- Stable and beta channels.
- Forking for another car is easy: a fork builds and ships its own releases with no edits.
- One **Update** button in LIVI updates everything: LIVI, carpid, and the sdlink Pi side, Teensy firmware and RNS-E decoder sectors.
- The button drops the hotspot, joins a normal Wi-Fi to fetch, then brings the hotspot back.

## 1. One bundle, one version

pq35-carpi is the superproject. It pins the LIVI fork (`livi/`) and sdlink (`sdlink/`) as submodules, so one pq35-carpi commit names exactly one set of everything. CI turns that commit into a single **bundle**:

```
carpi-<version>-arm64.tar.zst
  manifest.json      versions + commits of pq35-carpi, LIVI, sdlink; channel;
                     sha256 of every file; minimum OS profile
  bin/               carpid, carpi, LIVI binaries, sdlink Pi-side binaries
  firmware/          Teensy image, RNS-E decoder sector(s), each with its own
                     version and checksum guard (from sdlink)
  share/             systemd units, default configs (never overwrite user
                     config), udev rules
manifest.json.minisig
```

The exact layout and `manifest.json` schema are in [BUNDLE.md](BUNDLE.md). Differences from the sketch above: `manifest.json` and its `.minisig` sit at the top of the tarball and are also published next to it, so a device can check a release before downloading it; `firmware/` only has the Teensy image when one is passed in, since CI doesn't run PlatformIO yet, and no RNS-E sectors (see Decisions).

The device only ever talks to one repo's releases, so a fork of pq35-carpi is a complete product.

## 2. CI (GitHub Actions)

- **Runner**: `ubuntu-24.04-arm` (native arm64; free for public repos). Builds run inside a `debian:trixie` container so glibc and library versions match Pi OS Trixie.
- **Caches**:
  - `Swatinem/rust-cache`, keyed on each workspace's `Cargo.lock` and toolchain. pq35-carpi and LIVI are separate workspaces with separate caches; LIVI's pinned beta toolchain is part of its key.
  - The apt package cache.
  - sccache backed by the GitHub Actions cache, so unchanged crates don't rebuild even when a lockfile moves.
- **Jobs**:
  1. Test: the existing x86 fmt, clippy and test run.
  2. Build bundle on arm64.
  3. Sign the manifest. The minisign key lives in repo secrets (`MINISIGN_KEY`, made with `minisign -G -W`), and each fork uses its own. Without it the bundle is built unsigned, with a warning.
  4. Publish.
- **What triggers what**:

  | Event | Result |
  |---|---|
  | Push to `main` | Beta release, a GitHub pre-release tagged `beta-<date>-<sha>`; the newest 5 are kept |
  | Tag `vX.Y.Z` | Stable release |
  | Any branch or PR | Bundle uploaded as a workflow artifact only, for direct deploy |

- **Forks**: workflows use `${{ github.repository }}` everywhere and nothing is hardcoded. Each fork sets its own signing secret, and the device config names the fork's repo.

## 3. Device side: `carpi-update`

A separate small binary, not part of carpid, because it has to replace carpid. carpid exposes it as an API job, so the LIVI page shows progress.

- **Layout**: each bundle unpacks into `/opt/carpi/releases/<version>/`, and `/opt/carpi/current` is a symlink to the active one. Two are kept, current and previous. Services run from `current`.
- **Apply**:
  1. Verify the signature and every checksum.
  2. Unpack.
  3. Check the OS profile; refuse with a clear message if the OS needs something the bundle doesn't have.
  4. Swap the symlink.
  5. Restart the services.
  6. Health check: carpid answers, LIVI core is up, frames flow. If the check fails within 180 s, swap back and restart. This is the same pattern as sdlink's guard.
- **Firmware**: the Teensy image is flashed only when its version changed, behind its own confirmation in the UI. RNS-E sectors are not updated from the Pi until sdlink supports partial updates (see Decisions).
- **User config is never touched**: config files live in `~/.config/carpi`, `~/.config/LIVI` and so on. New defaults ship in `share/` for reference. No migrations unless Drew asks.
- **Source**: `[update] repo = "DSparks156x/pq35-carpi"` and `channel = "stable" | "beta"`. Pointing at a fork is one line.

As built: `carpi-update check | fetch [--force] | apply [--file F] [--allow-unsigned] [--reuse-current] | rollback | status`, with `--root` (default `/opt/carpi`) and `--profile` (default `/etc/carpi-profile`) for tests. It reads `[update]` and `[api]` from carpid's config:

```toml
[update]
repo = "DSparks156x/pq35-carpi"
channel = "stable"                          # or "beta" (beta also takes stable releases)
# pubkey = "/etc/carpi/minisign.pub"        # bundles must be signed by it when it exists
# restart_user = ["carpid.service"]         # systemctl --user restart
# restart_system = ["livi-kiosk.service"]   # sudo -n systemctl restart
# check_frames = false                      # health check also wants CAN frames
# updater = "/opt/carpi/current/bin/carpi-update"   # carpid only
```

- Progress is one JSON `JobEvent` (carpi-proto) per line on stdout, ending with `finished`.
- `fetch` checks the release's own `manifest.json` (signature, OS profile) before downloading the tarball, then verifies the tarball in one pass and keeps it in `/opt/carpi/downloads/<version>/`. `apply` without `--file` installs that.
- `/opt/carpi/state.json` holds the downloaded version and how the last apply ended (`ok`, `rolled_back`, `failed`); `status` reports it, so the result of an apply survives carpid's restart.
- An unsigned `--file` needs `--allow-unsigned` once the device has a key. No key: checksums only, with a note in the progress.
- Firmware in the manifest is reported, not flashed.

## 4. The Update button: network dance

LIVI owns `wlan0` as the hotspot. To fetch:

1. The UI asks carpid for `update.check`, which tells the user the phone link drops for the duration.
2. carpid tells the fork over the bridge to suspend the AP. LIVI's AP teardown already hands `wlan0` back to NetworkManager.
3. NetworkManager joins the first known client network: saved profiles for home Wi-Fi and the phone's hotspot, set up on the Car page.
4. Fetch the release list, download the bundle, verify it.
5. Rejoin: carpid tells the fork to restore the AP, whether or not the fetch worked. A hard timeout of about 3 minutes restores the AP no matter what.
6. Apply (section 3) runs offline, after the AP is back.

If no known network appears within about 30 s, restore the AP and report it. A downloaded but unapplied bundle can be applied later without fetching again.

As built:
- API (carpi-proto): `Request::Update { action: check | fetch | apply | status, and_apply }` replies `JobStarted { job }`, then unsolicited `Job { job, event }` until `finished`. One job at a time. `and_apply` with `fetch` is the Update button: fetch, AP back, apply.
- Bridge, carpid to LIVI: the fork sends `Request::Commands` once; carpid sends unsolicited `Response::Command { id, command: Ap { ap: suspend | resume } }` and the fork answers `Request::CommandDone { id, error }`. carpid waits 30 s per command.
- If NetworkManager already reports `connected` (Ethernet, say), the AP stays up.
- The fork stops and starts `livi-wifi-ap.service` through LIVI's own sudo rule, and only starts it on resume if it ran before. It resumes by itself when carpid's connection drops or after 4 minutes.
- `apply` runs as `systemd-run --user --unit=carpi-update-apply`, outside carpid's cgroup, because it restarts carpid. The UI loses the progress there and reads the result with `status`.

## 5. Direct deploy from the PC

`carpi deploy` (an xtask in pq35-carpi) for development:

- **Source** is one of:
  - a CI artifact for any branch or PR (`gh run download`), the default;
  - a local bundle, where only carpid and carpi are cross-built on Windows with `cargo zigbuild --target aarch64-unknown-linux-gnu.2.41`. That's pure Rust, so no sysroot is needed. LIVI and sdlink come from the latest CI artifact, since they need Trixie's libraries.
- **Delivery**: rsync over SSH to the Pi through LIVI's hotspot, then `carpi-update apply --file`. It's the same apply path, rollback included.
- **Tags**: dev bundles are versioned `dev-<sha>`, so the UI shows what's actually installed.

As built, `cargo xtask deploy`:
- `--from-ci <branch|run-id>` (default: the current branch): `gh run download` of the newest successful `release.yml` run's bundle artifact.
- `--file <bundle>`: a local bundle.
- `--carpid-only`: `cargo zigbuild` of carpid and carpi, then a dev bundle `dev-<sha7>[-dirty]` holding only those two and a manifest made from the Pi's installed one. `carpi-update apply --reuse-current` takes the other files from the current release and checks them against that manifest. Without zig or cargo-zigbuild it says how to install them.
- `--host` (default `carpi`), `--updater` (default `/opt/carpi/current/bin/carpi-update`).
- Delivery is scp to the Pi's home, then `apply --file ... --allow-unsigned` over ssh, printing the progress. scp instead of rsync: Windows has no rsync.

## 6. OS profile

The base image isn't in the bundle. It's a versioned script in pq35-carpi, `os/profile.sh`, that turns a fresh Pi OS Trixie into a carpi device:
- packages;
- config.txt lines (hat-specific: CarPiHat vs Drew's hat);
- networkd CAN config;
- udev rules;
- lingering;
- the journal on disk, capped at 200 MB, and the car user in `adm` (captures read every unit's journal);
- a polkit rule so carpid can set the clock from the car's (timedated `set-time` and `set-ntp`);
- a polkit rule so carpid can start Move to NVMe (`carpi-move-nvme.service`, start only), and rsync, fdisk, dosfstools and rpi-eeprom for it;
- the timezone, with `--tz` (or `CARPI_TZ`). It must match the car clock, which is local time;
- zstd, for `os/install.sh`;
- zstd, for `os/install.sh`;
- later, the NVMe boot-time trimming.

The bundle manifest names the minimum profile version it needs. The script is what I did by hand to the trial card, written down.

Run it as `sudo os/profile.sh [--hat carpihat|cowcarhat] [--tz Area/City] [--dry-run]`; CarPiHat is the default and Drew's hat has no lines yet. It records `PROFILE_VERSION` and the hat in `/etc/carpi-profile`, and also creates `/opt/carpi/releases` owned by the car user. The CAN network file sets `RequiredForOnline=no` so a car without the hat doesn't stall boot in wait-online.

## 7. New card: `cargo xtask image`

A stock Raspberry Pi OS Trixie image that sets itself up as a carpi on first boot.

```
cargo xtask image --image 2026-xx-xx-raspios-trixie-arm64.img.xz --tz America/Chicago
cargo xtask image --boot E:\ --tz America/Chicago      # a card or NVMe already flashed with the stock image
```

- **Asks** for the Wi-Fi name and password (first boot needs the internet for packages). `--edit` opens the network-config in an editor before it's written; `--network-config FILE` uses a netplan file as it is; `--no-wifi` is wired only. `--password` asks for a login password (VNC and the console need one); ssh is key-only, with the `carpi` alias's key (or `--ssh-key`).
- **Bundle**: `--bundle FILE`, `--from-ci BRANCH|RUN-ID`, default the CI bundle of the current branch.
- **Writes** cloud-init's `user-data`, `network-config` and `meta-data` and a `carpi/` folder (bundle, carpi-update, `firstboot.sh`, `profile.sh`, `install.sh`, `setup.env`) into the boot partition. From an image it writes a new `<name>-carpi.img`; flash that with Imager and no OS customisation.
- **First boot**: cloud-init makes the user and joins the Wi-Fi, then `os/firstboot.sh` waits for the network, runs `profile.sh`, then `install.sh`, turns on desktop autologin and reboots. Its log is `/var/log/carpi-firstboot.log`, also copied to `carpi/firstboot.log` on the boot partition. On a failure it doesn't reboot: ssh in, fix, re-run it.
- **`os/install.sh --bundle F [--updater P]`**: the first install, also usable on any profiled Pi. Puts in what's missing and nothing else: `carpid.toml`, the user units (carpid, LIVI, the A/V chain, wayvnc), `~/avsd`, the governor unit, Move to NVMe's root unit and its script (`/usr/local/lib/carpi/move-nvme.sh`, root-owned, refreshed when the bundle's differs), wayvnc's config. Enables them, then `carpi-update apply`. LIVI installs its own hotspot unit and udev rule when it runs.
- **Not done yet** (after the boot-time work): the Lite image with LIVI's kiosk instead of the desktop session, the boot-time trimming, LIVI's default knob bindings, an offline first boot. Untested on a real card so far.

## Decisions (Drew, 2026-11-07)

- `DSparks156x/pq35-carpi` on GitHub, public.
- Beta = every push to `main`; stable = `vX.Y.Z` tags.
- No speed gating on updates.
- Teensy firmware updates behind their own confirmation.
- RNS-E updates are not done from the Pi yet. The custom RNS-E firmware is a full CD update today. Updating the sdlink parts from the Pi needs the RNS-E side re-architected for partial updates first; that's sdlink work, outside this repo.
