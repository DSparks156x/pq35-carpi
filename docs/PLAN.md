# pq35-carpi foundation plan

Status: **draft, not yet agreed.** No code until Drew signs this off.

pq35-carpi replaces everything `RNS-E-Hudiy` does, built around a LIVI fork instead of Hudiy. The old repo is reference only, for what's proven on the car. Nothing gets ported.

Target: PQ35/46 cars on ICAN (TT 8J, A3 8P, R8, Q3 should mostly work as written), RNS-E 193 on Drew's SD-link firmware (R56B lineage), Pi 5, Android Auto. CAN IDs and car facts live in one place, not scattered through code, but we don't make every knob configurable.

Research notes behind this plan are in [research/](research/), with file:line refs into the old repo. Key facts are repeated here where a design decision rests on them.

---

## 1. Repos and processes

Three repos:

| Repo | What | License |
|---|---|---|
| `pq35-carpi` (this) | Car daemon, tools, simulators, install. Submodules the other two. | Noncommercial (see Q1), except `carpi-proto` |
| LIVI fork | AA/CarPlay head unit plus our Slint pages and a bridge module. Submodule at `livi/`. | GPL-3.0 + LIVI attribution (not ours to restrict) |
| sdlink | Encoder, avlink, Teensy firmware, RNS-E decoder sectors. Submodule at `sdlink/`. | Drew's call |

Flashing drivers (Haldex, EPS, etc.) stay their own separately maintained thing. They get a bus lease from the daemon (section 2.10) and nothing else.

Processes on the Pi:

- **`carpid`**: the car daemon. One process, one owner of `can0`, async (tokio) with one task per subsystem. Must run fine with LIVI stopped.
- **LIVI fork**: `livi-core`, `livi-ui`, compositor, helpers, as upstream. Our additions: a `carpi` bridge in core and our pages in the UI. Both talk to `carpid` over its local API.
- **sdlink chain**: wlcap, encoder, avlink, as today. LIVI runs nested in labwc on NOOP-1 so the chain works unchanged.
- **`carpi`**: a CLI for debugging and bench work (dump values, watch frames, inject keys, drive the DIS, take a bus lease).

Why one daemon: the old stack had 13 units, about 18 ZMQ endpoints, flag files for locks, and five processes opening their own CAN sockets around the transmit gate. One process gives one transmit gate, one bus budget, one state store and one clock. A crash restarts the whole daemon through systemd, which takes well under a second.

### 1.1 Crate layout (this repo)

Protocol crates are **sans-IO**: state machines that take `(now, input)` and return outputs and the next deadline. No sleeps, no sockets inside. The daemon wires them to real I/O; tests wire them to simulators and virtual time.

```
crates/
  carpi-can        Frame types, CanPort trait (SocketCAN, replay, sim), rx timestamps,
                   the TX gate and bus budget
  carpi-signals    Signal table generated from the DBC + overlay; decode with validity
  carpi-state      Value store: named values, samples with age/validity, watches, derived values
  carpi-nm         NM ring observer and bus-state machine
  carpi-power      Power backends (CarPiHat GPIO, hat MCU) and shutdown sequencing
  carpi-input      Raw key decoders, gesture recogniser, router, sinks
  carpi-rnse       RNS-E firmware commands (0x7B0, 0x602, ...) with re-assert on RNS-E reset
  carpi-light      Light model: one input set, outputs for panel, LIVI night, AA night
  carpi-tp2        TP2.0 data link (sequence, ACK, blocks, keepalive), shared by KWP and DDP
  carpi-kwp        KWP2000 client, request classes
  carpi-diag       Value catalog, channel manager, subscription scheduler, exclusive modes
  carpi-dis        DDP session, scene model, renderer, surfaces, arbiter, sources, stock-radio emulation
  carpi-proto      Local API message types (MIT/Apache so the GPL fork can link it)
  carpi-api        Local API server
  carpi-sim        Simulators: NM ring, RNS-E, cluster DDP peer, gateway + KWP modules
  carpid           The daemon binary: config, wiring, supervision
  carpi-cli        The `carpi` tool
tools/
  dbcgen           DBC + overlay -> checked-in Rust signal table
hat/               Hat MCU firmware, later phase
livi/              submodule (fork)
sdlink/            submodule
```

## 2. Core abstractions

### 2.1 CAN port and TX gate

- `CanPort` trait: `recv() -> (Frame, Instant)`, `send(Frame)`. Implementations: SocketCAN, file replay (candump format), in-process sim bus.
- **Nothing sends a frame except through the gate.** Every send carries a `TxClass`:
  - `Keepalive` (0x602 TV presence)
  - `Override` (0x7B0 firmware commands)
  - `DisplayTop` (0x363/0x365), `DisplayDdp` (0x6C0 session)
  - `Diag` (TP2 0x200, 0x300+)
  - `Extra` (0x67A exhaust, Haldex mode, Drew's own add-ons)
  - `Lease` (an external tool holds the bus)
- The gate asks the bus-state machine (2.3) whether the class may send now. Refused frames are dropped and counted, never queued across a sleep.
- **Bus budget**: the gate tracks bus load per class over a sliding window. At 100 kbit/s, DDP and TP2 fight for the same ~1000 frames/s ceiling, so the DIS renderer and the diag scheduler both ask the budget before planning work. Budget shares are config, not code.
- Kernel listen-only is a backstop only. The old code toggled it by taking `can0` down and up, which blinded RX for ~5 s each time. We don't do that.
- A rolling raw-frame recorder (last N minutes in RAM, dump on request or on fault) is part of the CAN layer, not a later tool. Most open questions below are answered by captures.

### 2.2 Signals and state

**Signal table.** Generated at build time by `tools/dbcgen` from `PQ35_46_ICAN.dbc`, filtered to an allowlist of IDs the car actually sends (41 on the TT), plus a hand-written TOML overlay for what the DBC lacks or gets wrong:

- real DLCs (0x2C1 is 5 on the car, DBC says 6; 0x436 is 6, DBC says 8)
- timeouts (the DBC has none; default 3x cycle time)
- invalid raw values (pattern: all-ones = error, all-ones-1 = init; speed 32708/32725/32742; wheel pulses 1021–1023)
- qualifier bits (`valid_when`/`invalid_when`, e.g. steering `LW1_Int_Status == 0`), and the 30 "Alt" bits that flag stale gateway repeats
- counters and checksums (XOR on 0x151, 0x531, 0x5B5; CRC8 on 0x3C3, poly unknown)
- which signals need Kl15

The generated table is checked in, so builds don't need the tool. Car-family differences are an overlay file per car, not code.

**Sample.** Every value in the system is

```rust
struct Sample<T> {
    value: Option<T>,
    status: Status,   // Valid | Init | Error | Stale | NotPresent
    at: Instant,      // when it was received/computed
    source: Source,   // Ican(id) | Diag(module, group, field) | Derived | Bridge
}
```

Staleness is computed at read time from the signal's timeout, so a value can't look fresh because nobody updated it. Consumers get `Sample`, never a bare number.

**Store.** One value store keyed by catalog names (dotted, e.g. `vehicle.speed`, `engine.boost.actual`). ICAN signals, diag values, derived values and bridge values (AA nav, media, calls) all live there. Subscribers get a `watch` per value or a batched stream at a requested rate. Derived values (light model, context) are small pure functions registered against their inputs.

### 2.3 Bus-state machine and power

Facts it rests on: OSEK NM ring with two members on the TT, gateway 0x42B and RNS-E 0x436, each sending about every 93 ms. Byte 0 = successor (ID − 0x420); byte 1 bits 0–2 Ring/Alive/LimpHome, bit 4 SleepInd, bit 5 SleepAck. Kl15 on 0x2C3/0x575. The Pi is not a ring member and never sends NM frames.

States:

```
Silent ──any frame──▶ Waking ──ring stable──▶ Awake ──SleepInd on all members──▶ SleepPending
   ▲                                              ▲                                   │
   │                                              └────── ring active again ──────────┤
   └──── no frames for T_silent (or SleepAck) ◀──────────────────────────────────────┘
```

- `Awake` has sub-flags, not more states: `kl15`, `rnse_alive` (0x436 fresh, SleepInd 0), `ring_ok`.
- Gate policy per class:
  - `Keepalive`, `Override`, `DisplayTop`: only while `rnse_alive`.
  - `DisplayDdp`, `Diag`: only with `kl15`.
  - `Extra`: per add-on, default `kl15`.
  - Nothing in `SleepPending` or `Silent`. One frame wakes every ICAN node.
- **The Pi never holds the bus awake** (Drew, 2026-11-07). If 0x602 itself keeps the RNS-E from raising SleepInd, the 0x602 sender must stop at Kl15 off, not at SleepInd. That's the first capture to make.
- `Silent` for T_silent → power backend gets `ShutdownRequested`. CAN is the shutdown trigger; the amp-wake GPIO becomes a secondary input.

**Power backend trait**: `ready_for_poweroff(delay)`, `wake_reason()`, `heartbeat()`.
- `CarPiHat`: GPIO 25 latch via `gpio-poweroff` overlay (already in config.txt), GPIO 12 amp wake as an extra signal, `systemctl poweroff` on request.
- `HatMcu`: UART link. Pi sends heartbeat and "halting"; MCU sends bus state, wake reason and its own view of Kl15. The MCU owns 5 V, listens on CAN to wake, re-powers the Pi if wake returns during halt, and rides through cranking. MFi chip sits on the hat's I²C for LIVI's CarPlay; the daemon doesn't touch it.

Drew's note: the hat changes how 5 V is switched and adds a status link. The bus-state machine and gate are identical with either backend.

### 2.4 Input pipeline

```
raw frames ─▶ decoders ─▶ KeyEvent{key, Down|Repeat|Up, t} ─▶ gestures ─▶ router ─▶ sinks
```

- **Decoders**:
  - Wheel 0x5C3 byte 1 (0B up, 0C down, 08 click, 1C MODE, 2A PTT, 06/07 volume, 00 release; the TT gateway turns a second MODE press into 01).
  - Faceplate and TV-panel 0x461 (`37 30 SS K1 K2 00`, 01 pressed ~110 ms repeat, 04 released ×5).
  - Knob as signed detents.
- **Gestures**: short, long, extended, double, with per-key timing in config. Timings start from the old code's numbers (wheel long = 3 repeats, 0x461 long 5 frames / extended 30) and get replaced by captured ones.
- **Router**: decides per gesture from context in the store: `rnse.source` (TV or not), `livi.connected`, `focus` (LIVI, DIS page, our pages). Rules:
  - Nothing routes unless the RNS-E is on TV (Drew: wheel keys ignored outside TV; firmware can be changed if needed).
  - DIS focus (entered by a wheel gesture) sends up/down/click to the DIS.
  - Otherwise keys go to LIVI.
  - A small set of system gestures (shutdown, reboot, save capture) maps to daemon actions.
- **Sinks**:
  - LIVI through a uinput keyboard with a fixed keymap. LIVI maps keys to AA rotary/keycodes and drives its own menus, so our Slint pages get focus navigation for free.
  - The DIS controller.
  - Daemon actions.
  - RNS-E commands.
- Long press is resolved in the daemon and sent as a distinct key, because LIVI has no long press.

### 2.5 RNS-E commands

`carpi-rnse` owns the firmware protocol (`RNSE hacking/CAN_PROTOCOL.md`, close enough for now; the car runs R56B-equivalent):
- 0x7B0 `BA` source label, `BB` LCD brightness, `BE`/`BF` key/source/volume/status, `BC`/`BD` ADC (analog path only).
- 0x602 TV presence, if the firmware still needs it (Q8).

It keeps a **desired state** and re-asserts it whenever the RNS-E comes back (0x436 reappears or a status poll shows a reset). The old code lost every setting on an RNS-E reboot.

**`B7` is never sent.** On the flash-over-CAN firmware lineage `B7 00`–`03` overwrite flash descriptor slots.

### 2.6 Light model

One derived value, `light`, from:
- 0x635 `DI1_Display`/`DI1_Sensor`
- `KO1_Lichtsensor`
- terminal 58t
- headlight state

Output `{ambient, lights_on, night (with hysteresis), panel_level}`. Three consumers read it: `carpi-rnse` (BB level), the bridge (LIVI dark mode, AA night sensor), and the DIS (its own contrast if the cluster allows). Nothing else decides day or night.

### 2.7 DIS

The old DIS was three processes and a pile of if-chains, touched by 237 commits. The rebuild has five layers, each testable alone.

1. **Transport.** `carpi-tp2` data link on the fixed 0x6C0/0x6C1 channel: data frames 2n/0n/1n, ACK Bn, receiver-not-ready 9n (wait, resend), keepalive A3 ~2 s, close A8.
2. **DDP session.**
   - Handshake: 15 → 00/01 → 08/09 → 20/21 → 33.
   - Claim `52 05 82 00 1B 40 30`; it's owned only on a `53 85` grant.
   - Cluster arbitration: busy 53 84 → free 53 05 → reinit 2E → confirm 2F → reclaim.
   - Output is events (`Ready`, `Busy`, `Lost`, `Error`) and a **generation counter** that bumps on any session or ownership change, so the renderer can never draw into a stale session.
   - Hard limits:
     - application message ≤105 B (195 worked, 208 froze the display while ACKs kept coming)
     - no `55` records with LEN ≥ 0x80
     - `39` commit is a separate message and isn't atomic.
3. **Surfaces.**
   - `Centre`: the white cluster only grants (0,27,64,48) logical = 128×96 physical.
   - `TopLines`: 2×8 chars over 0x363/0x365, separate from DDP.
   - A capability profile describes each; red/other clusters are another profile behind the same trait.
4. **Scene and renderer.**
   - Sources produce a retained scene per surface: text (cluster font id), 1-bit bitmap, built-in glyph, line, rect.
   - The renderer diffs the new scene against what the cluster holds and emits the smallest command batch within the limits and the bus budget.
   - Logical 64×48 by default, native 128×96 where it matters (nav icons).
   - The renderer also produces a PNG of what it believes is on the cluster, for tests and for the help site.
5. **Arbiter and sources.**
   - Each source posts an `Offer { surface, class, priority, min_dwell, expires }` with its scene.
   - Classes: `Alert` > `NavManeuver` (near a turn) > `Call` > `UserPage` > `Media` > `StockRadio` > `Idle`.
   - The arbiter picks a winner per surface independently, from a declared rule table: auto-switch to nav within N m of a maneuver, return to the previous page T s after, user page selection via DIS focus.
   - Sources:
     - AA nav from the bridge: maneuver, distance, road, lanes once the fork parses them.
     - Media and call from the bridge.
     - Readings pages: catalog values, using the diag scheduler.
     - Alerts.
     - Stock radio (below).

**Stock radio source.** With FIS-Control on, the RNS-E's DDP session moves to 0x7B2/0x7B3 and its radio text to 0x7B4/0x7B5 (sent only while TV is the source, every 800 ms). `carpi-dis` plays the cluster toward the RNS-E on 0x7B2/0x7B3: it runs the peer side of the same transport and session, accepts its draw commands, and turns them into a scene. The stock radio and nav then compete in the arbiter like any other source. 0x604 is RNS-E nav status, not DIS content; it feeds context.

### 2.8 Diagnostics

Facts: there's very little on-car evidence, and only the engine is measured. TP2 setup on 0x200 `01 C0 00 10 00 03 01` → 0x201 `D0`. Parameters BS 15, T1 100 ms, T3 5–10 ms. Opening a channel plus `10 89` costs ~70 ms of bus time. One `21 xx` is ~48 ms, mostly ECU latency. The estimated ceiling is ~18–20 group reads/s per channel. Parallel channels across modules are untested.

- **Channel manager**: one state machine per module (Closed, Opening, Open, Grace, Failed with backoff), timer-driven, interleaving requests across open channels so one slow module never stalls the rest. Max concurrent channels is config, starting at 2 until measured.
- **KWP client**: every request carries a class, `ReadOnly | Volatile | Persistent | Programming`. Anything above `ReadOnly` needs an exclusive lease. A module profile can deny polling outright: **ABS 0x03 is denied** (polling it causes an ESP fault and upsets the Haldex).
- **Catalog**: each value has one name and a list of providers (`ican:ID:signal`, `diag:module:group:field`) with costs. Formula decoding uses the ECU's formula byte; meaning comes from group/position per part number.
- **Scheduler**: one scheduler.
  - Clients subscribe `{value, period, priority, lease}`.
  - It resolves each value to its cheapest provider (ICAN first), packs groups into a set cover, and plans with EDF plus priority within the diag share of the bus budget.
  - Each subscription reports `requested`, `granted`, `achieved Hz` and a degradation reason.
  - Leases expire unless renewed (pages renew while visible).
  - Channels close after a grace period with no demand.
- **Exclusive modes**: `VcdsPause` (stop all diag TX), `Exclusive(job)` (one client owns diag), `BusLease` (2.10).
  - Another tester is detected passively when we see 0x200 setups we didn't send, and we go to `VcdsPause` (Q12).
- **Jobs**: install list, DTC read/clear, coding, adaptation. These are typed jobs with progress and cancel, not ad hoc requests.
- **ME7Logger-style dynamic identifiers**: a side path. It depends on Drew's ECU (2.0 TFSI is MED9/MED17, not ME7) and lives behind the same provider interface if it happens.

### 2.9 Local API

`carpi-api`, types in `carpi-proto`. One Unix socket, length-prefixed messages (JSON while the shape settles; the framing allows a switch to CBOR later). LIVI fork and daemon move in lockstep through the submodule, so no cross-version compatibility is needed (Drew), but the protocol carries a version and refuses mismatches.

- `values.subscribe` / `values.batch`: rate, priority, lease; replies carry `Sample`s and granted rates.
- `bridge.push`: from LIVI. AA nav state, media, calls, phone status, connection, focus.
- `input.event`: semantic events in either direction (pages can ask for raw knob, etc.).
- `rnse.*`, `dis.*`, `light.*`: commands and state.
- `jobs.*`: start, progress, cancel (DTCs, install list, coding, captures, sdlink updates).
- `config.get/set` with revisions.
- `bus.lease`: take or release the bus for an external tool.
- `events`: power state, alerts, faults.

Same-user peer check on the socket. No TCP listener by default (see Q11 for the phone portal).

### 2.10 Bus lease for external tools

A flashing tool asks for `bus.lease(reason, max_duration)`. The daemon then:
1. drains DIS and diag, closing channels and the DDP session cleanly
2. stops every TX class
3. replies with a token

The tool uses its own SocketCAN socket. The daemon keeps reading and recording. When the lease is released or expires, everything comes back. That covers Drew's separately maintained flashers without porting them. Leases need `kl15` and the bus must be awake; they never hold the bus awake.

### 2.11 LIVI bridge and fork points

Decision: the fork connects to `carpid`. Drew leaned towards the daemon being a LIVI client; this recommends the reverse, for three reasons:

- His license goal: the daemon must not link GPL crates.
- The pages need the daemon API anyway.
- The fork's bridge sees typed internal state that LIVI's socket protocol doesn't expose (calls, lanes).

To confirm, see Q2.

Fork changes, kept in separate crates or files with one-line hooks so rebases stay cheap:

| Change | Where | Upstream? |
|---|---|---|
| `livi-carpi` bridge: subscribe curated values, push nav/media/calls/phone/connection, apply night mode | new crate in core, hook in core startup | no |
| Fill more AA sensors from bridge values: doors, outside temp, gear, lights; dead reckoning (steering 0x3C3, wheel ticks 0x5B5) later | `aa_telemetry.rs` | doors/compass yes, DR maybe |
| Consume `Report::Calls` into state | `projection/aa.rs`, `state.rs` | likely |
| Parse lanes and next steps | `livi-aa-stack` nav | likely (state fields exist for CarPlay) |
| Our pages | new `livi-carpi-ui` crate, one hook in the `shell.rs` tab enum | no |
| Car actions / vendor extension (e.g. AA "car alerts" opens fault page) | later, needs RE | unlikely |
| Headless compositor output to the encoder | contingency only | maybe |

Upstream PRs follow LIVI's rules: issue first, no Claude co-author trailer (`Assisted-by:` instead), CLA.

Don't set `LIVI_KIOSK=1`: it makes LIVI power the host off itself. Use the config's kiosk setting for fullscreen. LIVI's telemetry server on :4000 listens on all interfaces, including its AP; firewall it to localhost or disable it if the bridge replaces it.

### 2.12 Pages

Slint pages inside the fork (Drew), in `livi-carpi-ui`, using LIVI's widgets and theme. They talk only to `carpid` over the API.

- **Data**: dashboards (engine, transmission, AWD) and readings.
- **Diagnostics**: install list, DTCs, measuring groups, coding.
- **Logs**: recorded sessions, CAN captures.
- **Car**: RNS-E source label, brightness, light model, DIS pages, power, services.
- **SD link**: status and updates.

Knob navigation comes from LIVI's own focus handling. 800x480 vs visible 800x400 is Q10.

### 2.13 Video

Phase 1: unchanged. LIVI runs nested in labwc on NOOP-1, and sdlink's wlcap/encoder/avlink carry it. sdlink is a submodule, so this repo installs and updates the Pi-side chain and pushes matching Teensy firmware and RNS-E decoder sectors, with sdlink's boot checksum guard. Audio sinks must be renamed away from Hudiy's.

Contingency, only if latency or CPU says so: the fork's compositor renders offscreen and pipes XRGB frames plus damage straight into the encoder's stdin contract.

## 3. Scope items mapped

| Scope | Built on |
|---|---|
| Power, bus sleep | 2.1 gate, 2.3 bus-state machine, power backend |
| Input | 2.4 pipeline, uinput into LIVI, context from store |
| DIS | 2.1 budget, 2.7 layers, sources from store and bridge |
| Radio overrides | 2.5 desired-state client, 2.6 light model |
| Vehicle state | 2.2 signals and store; curated set to AA via bridge (2.11) |
| Diagnostics | 2.8 on 2.1 gate and budget; values into the same store |
| LIVI | 2.9 API, 2.11 fork points, 2.12 pages |
| Video | 2.13, sdlink submodule |
| Drew's extras (Haldex mode, exhaust valve, ...) | an `extras` module per item, using `Extra` TX class, catalog values and a page; no special paths |

## 4. Testing

- **Sans-IO unit tests** for every protocol crate, on virtual time. That covers TP2 link, DDP session, KWP, NM machine, gesture recogniser, arbiter and scheduler.
- **Simulators** (`carpi-sim`), wire-level, scriptable faults:
  - Cluster DDP peer: startup busy, random 9n, 53 C0 for non-central claims, freeze over 195 B, late errors.
  - RNS-E: NM member, 0x461 keys, 0x7B0 responder, DDP sender on 0x7B2.
  - Gateway plus KWP modules with measured latencies.
  - An NM ring that goes to sleep.
- **Replay**: candump files through the full daemon with TX captured; assert on decoded state and on what would have been sent. `references/canlog.txt` is the first fixture. Every capture Drew makes becomes one.
- **DIS golden renders**: scene → PNG and scene → byte stream, both snapshot-tested.
- **Property tests**: the gate never sends in `SleepPending`/`Silent`; the renderer never emits a message over the limit; the scheduler never polls a denied module.
- **Bench**: Drew's A3 cluster bench (500 kbit/s) for DIS, then the car at 100 kbit/s. Bench numbers are labelled as bench.
- **CI**: `cargo test` and clippy on x86-64, cross-build for aarch64. Pi-only bits (SocketCAN, GPIO, uinput) sit behind traits so CI never needs hardware.

## 5. Build order

Each step ends with something running on the car or bench, read-only first.

0. **Captures and skeleton.** Workspace, CI, `dbcgen`, recorder. Captures on the car:
   - ignition off to bus silence, with and without the Pi sending
   - knob detents and direction
   - MODE/PTT/click holds
   - a short drive log
1. **Read-only daemon.** CAN port, signals, store, NM observer, `carpi` CLI. Runs beside the old stack with zero TX.
2. **LIVI trial.** Stock LIVI on the microSD (BOOT_ORDER=0xf461, Drew runs the EEPROM write), nested in labwc, through sdlink. Answers HEVC, memory and latency questions.
3. **Gate, bus-state machine, power.** CAN-triggered shutdown on the CarPiHat. First TX: 0x602 and 0x7B0 through `carpi-rnse`, light model.
4. **Input.** Pipeline into LIVI over uinput. Old stack off from here on the trial card.
5. **Fork, part 1.** Submodule, `livi-carpi` bridge, calls, lanes, extra sensors.
6. **DIS.** TP2 link and DDP session against the simulator, then bench, then car. Renderer, arbiter, nav/media/call/readings sources.
7. **Diagnostics.** KWP, channel manager, catalog, scheduler, VCDS pause, bus lease. Measure channel concurrency and rates on the car.
8. **Pages.** `livi-carpi-ui`: Data, Diagnostics, Logs, Car, SD link.
9. **Stock radio DIS source.** Cluster role on 0x7B2/0x7B3.
10. **Extras and hat.** Haldex mode, exhaust valve, hat MCU firmware and `HatMcu` backend. Headless-encoder contingency if step 2 showed the need.

## 6. Open questions

Answered so far (2026-11-07):

- **Repo**: `pq35-carpi`, sibling of `RNS-E-Hudiy`, local only.
- **Audience**: works as written on A3/TT/R8/Q3; CAN IDs in one place; no blanket configurability. Others fork for distant relatives.
- **RNS-E firmware**: not r21 any more. The SD-link firmware (about R56B) has similar commands. `RNSE hacking/CAN_PROTOCOL.md` is close enough for now. Firmware can be changed to suit.
- **Hat**: plan for it; it only changes how 5 V is switched plus a status link. Sleep and TX rules are the same.
- **Pages**: Slint, inside the LIVI fork, kept rational for rebases. No webviews.
- **LIVI fork**: separate repo, submodule here, kept in lockstep.
- **Bridge direction**: Drew unsure; leaned towards the daemon as a LIVI client. This plan recommends the fork connecting to the daemon (2.11). Confirm: **Q2**.
- **Licensing**: Drew wants "mildly commercially restrictive". See **Q1**.
- **Flashing**: Drew's flash drivers stay separate (bus lease). Haldex mode and exhaust are his extras, design freely. sdlink is a separate repo submoduled here; this repo pushes Pi-side updates and matching Teensy/RNS-E sectors.
- **Stock DIS**: yes, the stock radio's centre screen is an arbiter source.
- **Wheel keys**: ignore when the RNS-E isn't on TV.
- **Keep awake**: the Pi never holds the bus awake after ignition off.

Still open:

1. **License for pq35-carpi.** Proposal: PolyForm Noncommercial 1.0.0 (free for personal use, commercial use needs Drew's permission). `carpi-proto` is MIT/Apache, because the GPL fork links it. Code in the fork is GPL whatever we want, so keep page logic thin there. OK?
2. **Bridge direction**: fork connects to the daemon (recommended), or the daemon is a client of LIVI's socket?
3. Does 0x602 TV presence keep the RNS-E from sleeping? Needs the capture in step 0.
4. Knob: frames per detent and which direction is clockwise? Hold behaviour of MODE and PTT? Capture.
5. Which transceiver does the CarPiHat use on ICAN? Is it fault-tolerant low-speed?
6. Engine ECU part number and software (`1A 9B`)? That decides whether dynamic identifiers are worth it.
7. Has two-channel TP2 (e.g. engine + Haldex) ever run at once? OK to measure in step 7?
8. Does the R56B-era firmware still need the Pi's 0x602 frame, or does it fill it itself (r30+ did after ~2.2 s)?
9. Is FIS-Control on in the car's firmware, so the RNS-E's DDP session goes to 0x7B2/0x7B3?
10. Visible RNS-E area: 800x480 or 800x400? Pages are sized to it.
11. Phone file access (logs, captures out; firmware in) over LIVI's AP: worth a small HTTP portal in the daemon, or drop it for now?
12. VCDS: auto-pause when another tester is detected, or manual only?
13. Which DIS pages survive: readings, acceleration test, openpilot, the eggs?
14. Does LIVI's own AP and Bluetooth replace the NetworkManager hotspot setup on the Pi?
15. Does your phone negotiate HEVC with LIVI? (Pi 5 has no H.264 hardware decode.) The trial answers it.
16. Is the visible TT odometer about 238,600 km? Sanity check for the 0x65D decode.
