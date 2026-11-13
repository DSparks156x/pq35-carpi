# rnse-carpi
A full stack to run Android Auto/Carplay and cleanly integrate into the CanBus of an old RNS-E equipped pq35 audi. ie, a Mk2 TT or an A3 8P. Including a full DIS display driver (top lines and center) for white clusters (and probably red clusters still), and pulls basically any data possible from ICAN directly. also has a tp2/kwp2000 diag driver to grab measuring blocks off ECU/TCU/Haldex and display. oh. and diagnostics. 
Setup helper at https://cowpowersystems.com/carpi
## Status

`carpid` listens on the configured CAN interfaces, decodes ICAN into a value store, watches the NM ring, keeps a rolling capture and answers a local API. It also:

- keeps the RNS-E source label and panel brightness (`BA`/`BB` on 0x7B0), re-sent when the RNS-E comes back, through the TX gate;
- runs one light model for night mode and panel brightness, including curves, learned points and settings for ambient sources;
- turns wheel and faceplate keys into LIVI keys (uinput) while the RNS-E is on TV;
- drives the cluster display (`carpi-dis`): nav from the LIVI bridge and a 2x4 readings page, with DIS keys picking the page, plus the 0x363/0x365 top lines while on TV;
- runs diagnostics (`carpi-diag`) on the `diag` bus: API subscriptions to catalog values become polled group reads, plus install list and DTC read/clear jobs (clear needs `confirm: true`);
- serves the phone portal (`carpi-portal`) on `:5003/files` when `[portal]` is in the config;
- takes typed settings over the API (TX, shutdown, light model, DIS, key bindings, portal PIN), saves them to `carpid.toml` with a backup and applies them live;
- requests shutdown when the bus has been silent for 60 s with Kl15 off.

Sending, the cluster display and powering off are **off by default** (`[tx] enabled`, `[dis] enabled`, `[power] shutdown` in `deploy/carpid.toml`); until then the daemon logs what it would have done. Every frame goes through the TX gate.

DIS and diagnostics passed a functional car test on 2026-11-09, as did SDLink video/audio. Ignition, wake/sleep and shutdown remain unconfirmed. Diagnostic rates/concurrency and longer driving sessions need additional coverage. LIVI's Data, Diagnostics and Car Integration pages are implemented.

Current acceptance work and remaining features: [ROADMAP.md](docs/ROADMAP.md).

## Build

```
cargo test --workspace
cargo build --release -p carpid -p carpi-cli
```

Releases are built by GitHub Actions (`.github/workflows/release.yml`), not on the Pi: an arm64 Debian Trixie build of carpid, carpi, LIVI (`livi/`) and the sdlink Pi side (`sdlink/`), packed by `scripts/bundle.sh` into one bundle ([docs/BUNDLE.md](docs/BUNDLE.md)). A push to `main` makes a beta pre-release, a `vX.Y.Z` tag a stable release, anything else a workflow artifact. Clone with `--recurse-submodules`. Design: [docs/DEPLOY.md](docs/DEPLOY.md).

For a fresh install, flash Raspberry Pi OS Trixie 64-bit with the desktop, then use the [setup page](https://cowpowersystems.com/carpi/). First boot downloads the selected release and applies the answers automatically; see [IMAGE-TOOL.md](docs/IMAGE-TOOL.md). The next acceptance run is to verify the complete process without manual repairs. Developer deployment is covered in [DEPLOY.md](docs/DEPLOY.md).

## Use

```
carpi bus                 # ring state and members
carpi values light.       # live values, by prefix
carpi capture             # dump the last 10 minutes of raw frames
carpi replay some.log     # run a candump file through the decoders offline
```

`carpid --replay some.log --realtime` plays a capture as if it were the bus.

**Car Integration > Haldex mode switching** enables the optional RNSE-HUDIY five-mode controls in **Diagnostics > 22 AWD (Haldex)**: Stock, Performance, Competition, Snow Drift and Full Coupling. It is off by default. Enabling it requires HaldexRE firmware with the custom `3B F0` diagnostic handler (7716 or later); the feature probes support before sending a mode write. A switch is confirmed by querying the ECU after its debounce, and the last confirmed mode is saved in `[haldex]` in `carpid.toml` and restored on startup when enabled. A fresh installation has no saved mode to restore. Disabling the toggle hides the controls and stops mode switching and restore traffic; the saved setting remains available if enabled again. All requests use the existing diagnostics channel and TX gate.

In LIVI, **Car Integration > A/V over SD > SDLink firmware** (also under Software Update) shows the RNS-E SDLink module (sector 107) and the Teensy firmware: what is installed, what this release bundles, and whether it's newer. **Check versions** reads the module header and firmware table over CAN (read only) and asks the Teensy its version. **Install** writes the bundled image after a second press to confirm:

- RNS-E: ignition on, engine off. carpid stops the A/V chain, takes the CAN bus, stages the image over CAN (about a minute), commits it (the RNS-E erases sector 107, programs it and restarts, about 25 s), then reads the sector back and checks its CRC, the restart count and that the module runs. Before the commit nothing is written; a failed check stops there. Procedure: sdlink `docs/PI_UPDATER.md`.
- Teensy: carpid stops the chain, reboots the Teensy into its bootloader, flashes it and checks the version it reports.

**Advanced: Flash Sector** (SDLink Firmware): writes a portal upload `[prefix]sector_<N>.bin` (128 KiB) to RNS-E sector N. Two presses. Same stage/commit/verify as the install; no content checks. Sectors 32-511 (the unit refuses its loader below 0x400000; carpid's gate matches).

The release CI builds both images from sdlink (`.github/workflows/release.yml`, job `firmware`). **Read decoder** on the A/V over SD page still reads the whole sector 107 CRC32 (several minutes).

## License

PolyForm Noncommercial 1.0.0 ([LICENSE.md](LICENSE.md), [NOTICE](NOTICE)). `crates/carpi-proto` is MIT OR Apache-2.0 so the GPL LIVI fork can link it.
