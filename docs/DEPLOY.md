# Build, release, update

Status: **draft for Drew.** Sections 1, 2 and 6 are written (`docs/BUNDLE.md`, `scripts/bundle.sh`, `.github/workflows/release.yml`, `os/profile.sh`) but have not had a CI run yet.

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

## 4. The Update button: network dance

LIVI owns `wlan0` as the hotspot. To fetch:

1. The UI asks carpid for `update.check`, which tells the user the phone link drops for the duration.
2. carpid tells the fork over the bridge to suspend the AP. LIVI's AP teardown already hands `wlan0` back to NetworkManager.
3. NetworkManager joins the first known client network: saved profiles for home Wi-Fi and the phone's hotspot, set up on the Car page.
4. Fetch the release list, download the bundle, verify it.
5. Rejoin: carpid tells the fork to restore the AP, whether or not the fetch worked. A hard timeout of about 3 minutes restores the AP no matter what.
6. Apply (section 3) runs offline, after the AP is back.

If no known network appears within about 30 s, restore the AP and report it. A downloaded but unapplied bundle can be applied later without fetching again.

## 5. Direct deploy from the PC

`carpi deploy` (an xtask in pq35-carpi) for development:

- **Source** is one of:
  - a CI artifact for any branch or PR (`gh run download`), the default;
  - a local bundle, where only carpid and carpi are cross-built on Windows with `cargo zigbuild --target aarch64-unknown-linux-gnu.2.41`. That's pure Rust, so no sysroot is needed. LIVI and sdlink come from the latest CI artifact, since they need Trixie's libraries.
- **Delivery**: rsync over SSH to the Pi through LIVI's hotspot, then `carpi-update apply --file`. It's the same apply path, rollback included.
- **Tags**: dev bundles are versioned `dev-<sha>`, so the UI shows what's actually installed.

## 6. OS profile

The base image isn't in the bundle. It's a versioned script in pq35-carpi, `os/profile.sh`, that turns a fresh Pi OS Trixie into a carpi device:
- packages;
- config.txt lines (hat-specific: CarPiHat vs Drew's hat);
- networkd CAN config;
- udev rules;
- lingering;
- later, the NVMe boot-time trimming.

The bundle manifest names the minimum profile version it needs. The script is what I did by hand to the trial card, written down.

Run it as `sudo os/profile.sh [--hat carpihat|cowcarhat] [--dry-run]`; CarPiHat is the default and Drew's hat has no lines yet. It records `PROFILE_VERSION` and the hat in `/etc/carpi-profile`, and also creates `/opt/carpi/releases` owned by the car user. The CAN network file sets `RequiredForOnline=no` so a car without the hat doesn't stall boot in wait-online.

## Decisions (Drew, 2026-11-07)

- `DSparks156x/pq35-carpi` on GitHub, public.
- Beta = every push to `main`; stable = `vX.Y.Z` tags.
- No speed gating on updates.
- Teensy firmware updates behind their own confirmation.
- RNS-E updates are not done from the Pi yet. The custom RNS-E firmware is a full CD update today. Updating the sdlink parts from the Pi needs the RNS-E side re-architected for partial updates first; that's sdlink work, outside this repo.
