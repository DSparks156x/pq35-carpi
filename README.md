# pq35-carpi

Car daemon for PQ35/46 cars with an RNS-E and a Pi, built around a LIVI fork. Replaces RNS-E-Hudiy.

The plan is [docs/PLAN.md](docs/PLAN.md); the cluster display design is [docs/DIS.md](docs/DIS.md).

## Status

`carpid` listens on the configured CAN interfaces, decodes ICAN into a value store, watches the NM ring, keeps a rolling capture and answers a local API. It also:

- keeps the RNS-E source label and panel brightness (`BA`/`BB` on 0x7B0), re-sent when the RNS-E comes back, through the TX gate;
- runs one light model for night mode and panel brightness;
- turns wheel and faceplate keys into LIVI keys (uinput) while the RNS-E is on TV;
- requests shutdown when the bus has been silent for 60 s with Kl15 off.

Sending and powering off are **off by default** (`[tx] enabled`, `[power] shutdown` in `deploy/carpid.toml`); until then the daemon logs what it would have done.

Built but not yet in the daemon: TP2/KWP diagnostics (`carpi-tp2`, `carpi-kwp`, simulator in `carpi-sim`). In progress: the cluster display (`carpi-dis`) and the LIVI fork bridge.

## Build

```
cargo test --workspace
cargo build --release -p carpid -p carpi-cli
```

On the Pi, build natively and copy `target/release/carpid` and `target/release/carpi` to `~/.local/bin`, then install `deploy/carpid.service` and `deploy/carpid.toml` (instructions in the unit file).

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
