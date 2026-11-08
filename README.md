# pq35-carpi

Car daemon for PQ35/46 cars with an RNS-E and a Pi, built around a LIVI fork. Replaces RNS-E-Hudiy.

The plan is [docs/PLAN.md](docs/PLAN.md); the cluster display design is [docs/DIS.md](docs/DIS.md).

## Status

`carpid` listens on the configured CAN interfaces, decodes ICAN into a value store, watches the NM ring, keeps a rolling capture and answers a local API. It also:

- keeps the RNS-E source label and panel brightness (`BA`/`BB` on 0x7B0), re-sent when the RNS-E comes back, through the TX gate;
- runs one light model for night mode and panel brightness (the PLAN 2.6 model, curve and learning are not built yet; its settings and `light.ambient`, `light.panel_pct`, sun-load values are);
- turns wheel and faceplate keys into LIVI keys (uinput) while the RNS-E is on TV;
- drives the cluster display (`carpi-dis`): nav from the LIVI bridge and a 2x4 readings page, with DIS keys picking the page, plus the 0x363/0x365 top lines while on TV;
- runs diagnostics (`carpi-diag`) on the `diag` bus: API subscriptions to catalog values become polled group reads, plus install list and DTC read/clear jobs (clear needs `confirm: true`);
- serves the phone portal (`carpi-portal`) on `:5003/files` when `[portal]` is in the config;
- takes typed settings over the API (TX, shutdown, light model, DIS, key bindings, portal PIN), saves them to `carpid.toml` with a backup and applies them live;
- requests shutdown when the bus has been silent for 60 s with Kl15 off.

Sending, the cluster display and powering off are **off by default** (`[tx] enabled`, `[dis] enabled`, `[power] shutdown` in `deploy/carpid.toml`); until then the daemon logs what it would have done. Every frame goes through the TX gate.

Untested on the car: the DIS session and screens in carpid (simulator only), diag polling rates and channel concurrency. In progress: the LIVI fork's pages.

## Build

```
cargo test --workspace
cargo build --release -p carpid -p carpi-cli
```

Releases are built by GitHub Actions (`.github/workflows/release.yml`), not on the Pi: an arm64 Debian Trixie build of carpid, carpi, LIVI (`livi/`) and the sdlink Pi side (`sdlink/`), packed by `scripts/bundle.sh` into one bundle ([docs/BUNDLE.md](docs/BUNDLE.md)). A push to `main` makes a beta pre-release, a `vX.Y.Z` tag a stable release, anything else a workflow artifact. Clone with `--recurse-submodules`. Design: [docs/DEPLOY.md](docs/DEPLOY.md).

A fresh Pi OS Trixie card is set up with `sudo os/profile.sh`.

By hand, for now: build natively on the Pi and copy `target/release/carpid` and `target/release/carpi` to `~/.local/bin`, then install `deploy/carpid.service` and `deploy/carpid.toml` (instructions in the unit file).

## Use

```
carpi bus                 # ring state and members
carpi values light.       # live values, by prefix
carpi capture             # dump the last 10 minutes of raw frames
carpi replay some.log     # run a candump file through the decoders offline
```

`carpid --replay some.log --realtime` plays a capture as if it were the bus.

## License

PolyForm Noncommercial 1.0.0 ([LICENSE.md](LICENSE.md), [NOTICE](NOTICE)). `crates/carpi-proto` is MIT OR Apache-2.0 so the GPL LIVI fork can link it.
