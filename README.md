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

In LIVI, **Car Integration > A/V over SD > Read decoder** reads flash sector 96 and displays its CRC32
as the installed RNS-E SDLink image identity. Progress and errors appear on the same page; reading
the complete 128 KiB over CAN can take several minutes. This only reads the unit and does not compare
or update firmware. The CRC covers the whole sector, including unused bytes, so different installation
methods can produce different CRCs for the same decoder code. Keep the ignition on and the RNS-E awake during the read.

## License

PolyForm Noncommercial 1.0.0 ([LICENSE.md](LICENSE.md), [NOTICE](NOTICE)). `crates/carpi-proto` is MIT OR Apache-2.0 so the GPL LIVI fork can link it.
