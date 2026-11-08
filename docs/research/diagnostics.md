# Diagnostics research: TP2.0 / KWP2000 / values / scheduler / flashing

Source: RNS-E-Hudiy, branch `testing` (read-only). Paths are repo-relative. "Proven" means there is on-car evidence (logs, working feature Drew uses); "coded" means the code does it but no car evidence was found; "guessed" means the code itself says unconfirmed.

Evidence base for on-car behaviour is thin: the only raw TP2 traces are `references/logs/2026-06-17/tp2_worker_{1,2}.log` (older worker code with verbose frame logging, engine 0x01 only). The current scheduler/broker README says outright: "No capacity or extra-field semantics have been confirmed on vehicle hardware by this implementation" (`vehicle_data/README.md:146-150`).

---

## 1. TP2.0 facts

### Topology
- One SocketCAN interface. Drew's car: diagnostics on ICAN (100 kbit/s, fault-tolerant) and the gateway routes TP2 to the other buses. `tp2/tp2_protocol.py:92` opens `bitrate=100000`; `config.json` `interfaces.can.diagnostic = can0` (same as infotainment). `tp2/config_can.sh` supports a second USB adapter `diagcan` (500k) with `-d` "for cars where ICAN refuses diagnostics". Not used on Drew's car.
- Each worker session opens its **own** python-can socket on the same interface (`tp2_protocol.py:90-95`, `tp2_worker.py:362-365`). The flasher opens yet another socket (`flasher/socketcan_device.py:39`). Every socket sees all ICAN traffic and filters in software (`flasher/vag_protocols/tp2.py:237-245`).

### Channel setup (proven on engine 0x01)
From `references/logs/2026-06-17/tp2_worker_2.log:15-30`:
```
TX 200: 01 C0 00 10 00 03 01      dest=0x01, C0 setup, "my RX = 0x300" (00 03), RX-ID-invalid flag 0x10 on ECU side, app type 01
RX 201: 00 D0 00 03 40 07 01      D0 positive, tester RX 0x300, ECU RX 0x740   (reply ID = 0x200 + dest)
TX 740: A0 0F 8A FF 32 FF         params request: BS=15, T1=100 ms, T3=5 ms
RX 300: A1 0F 8A FF 4A FF         ECU: BS=15, T1=100 ms, T3=10 ms
TX 740: 10 00 02 10 89 / RX: 50 89   StartDiagnosticSession 0x89
TX 740: A3 / RX: A1 ...           channel test (keepalive)
```
- Setup payload builder: `flasher/vag_protocols/tp2.py:268-272`; open: `:274-297` (waits for 7-byte `D0` on `0x200+module`, decodes `<xBHHB`). Negative replies D6..D8 are only detected as "not D0" (sweep in `tp2/module_survey.py:150-173` records them).
- Timing byte decode: top 2 bits = unit (0.1/1/10/100 ms), low 6 = count (`tp2.py:30-34`). The old log printed "T1=1380ms, T3=7.4ms" — that was a decode bug in old code; the bytes mean T1 100 ms, T3 10 ms.
- Our A0 always requests BS 15 / T1 100 / T3 5 (`tp2.py:17-19`); we then adopt the ECU's A1 values for our own pacing (`tp2.py:257-266`), with T3 floor 1 ms (`:112`).
- Known channel IDs: engine 0x01 -> ECU RX 0x740 (log). Haldex 0x0A application -> 0x764; anything else means the Haldex is in its loader (`flasher/controllers/haldex_gen4/protocol.py:227-229,253,305`). Tester RX IDs: worker pool 0x300-0x306 (`tp2_worker.py:108`), 0x307 reserved for Openpilot (disabled, `tp2/openpilot_receiver.py:23-24`).
- Gateway is TP2 0x1F. Installation list (below) gives VCDS address -> TP2 ID. Memory note: 09 -> 0x20 (BCM), 46 -> 0x21 (KSG, coded but doesn't answer), 04 -> 0x13 (coded, absent), RNS-E answers as 56/0x52 and 37/0x5B; 10/57/77 never answer.
- AWD: code uses TP2 0x0A everywhere (`rns-e_can/haldex_manager.py:56`, `vehicle_data/consumer_definitions.py:18-20` flags that "22 vs 0x0A" was never reconciled in code; the install list should hold the answer).
- Wakeup broadcast `200: 01 23 ...` exists only as a test script (`tp2/wakeup_test.py`); never needed in production.

### Channel open cost (proven, engine)
- Wire: D0 reply ~12 ms after C0; A1 ~12-16 ms after A0; `10 89` -> `50 89` ~38 ms. Open + session ~65-70 ms of bus time.
- Wall time in the old Python worker: 80-190 ms from "Connecting" to first TX (bus open + logging), ~250-340 ms to "Connected" including session + A3 (`tp2_worker_2.log:15-30, 47-62`).
- Current worker: failed connects back off 5 s (`tp2_worker.py:350-352`) and close the socket (`:380-384`).

### Keepalive / idle
- A3 channel test, expects A1 (or 0x93) back (`tp2.py:419-444`). `maybe_send_keep_alive` rate-limits A3 to once per `keepalive_interval = 1.0 s` (`tp2.py:184, 414-417`). The old worker sent A3 after every group read (visible in the log); current code sends at most 1/s.
- KWP TesterPresent `3E` every 2 s when a session is held but idle (`tp2_worker.py:770-773, 939-941, 958-960`). No evidence a module actually needs 3E as well as A3; it's belt and braces.
- 0x9x frames (receiver-not-ready) extend the deadline +1 s, hard cap 30 s (`tp2.py:339-340, 386-388`).
- A8 from peer: reply A8, raise ConnectionError (`tp2.py:446-453`). A3 from peer: answer A1 (`:344-345, 394-395`).
- Length word bit 15 is a flag some ECUs set (airbag, steering wheel, engine pending replies); length is the low 15 bits (`tp2.py:145-147`). Proven enough that someone wrote it down.
- Idle policy: slow-only (>=60 s periods) sessions disconnect between reads; any period < 60 s keeps the channel open (`tp2_worker.py:765-776`). A one-off KWP job holds the channel 2 s for a follow-up (`:343-344`).

### Concurrency limits
- Max concurrent channels the gateway tolerates: **unknown, never measured**. The 7-ID pool is self-imposed. The worker is one thread that round-robins sessions; channels are open concurrently but used sequentially (`tp2_worker.py:723-1035`). A module that times out (T1 timeout 2 s, `tp2_protocol.py:71`) stalls every other module for up to 2 s+.
- Survey sweep of 0x01-0x7F at 0.25 s/ID, closing half-open channels with A8 (`module_survey.py:150-173`) — gateway coped with it.

### Measured group read rates (proven, engine, old code)
From `tp2_worker_2.log` (my analysis):
- Group 3 alone: 9.2 reads/s. Five groups {3,20,102,106,115} round-robin: 9.6 reads/s total, i.e. ~1.9 Hz each. Mean request 83-91 ms, p95 ~160 ms.
- Wire breakdown of one `21 03` (log lines ~"1C 00 02 21 03"): ACK B_ +8 ms, first response frame +23-31 ms, 4 frames ~6 ms apart, our ACK; request-to-complete ~48 ms. 26-byte payload = 8 fields x 3 bytes (engine returns 8 fields per group, fields 5-8 often `0x0000`).
- So the wire floor is ~50 ms/group => ~18-20 groups/s per channel if Python overhead and per-read A3 are removed. That number is an estimate. Current code's real rate: unmeasured (tool exists: `tools/inspect_measuring_groups.py --as-fast`).
- Other modules: no rate data at all.

### Errors seen and recovery
- On car (logs): module 0x00 subscribed by DataView (a pseudo-module for CAN values) -> "No response to connection request" every 5 s forever (`2026-05-29/tp2_worker_1.log`). Openpilot 0x0C setup refused repeatedly (`2026-06-17/tp2_worker_3.log`). No KWP NRCs or TP2 timeouts appear in the engine trace.
- Recovery coded (not observed): per group 3 failures -> 30 s cooldown (`tp2_worker.py:1027-1031`); 10 consecutive session errors -> disconnect + reconnect (`:1009-1016`); exceptions on transport -> A3 test, else drop (`:1017-1025`). The broker adds its own exponential group cooldown 0.5 s..30 s, 30 s for non-transient NRC 11/12/31 (`vehicle_data/broker.py:211-235`, `tp2/group_scheduler.py:130-132`). Two cooldown layers that don't know about each other.
- Sequence mismatch -> ValueError; duplicate of last ACKed frame is re-ACKed (`tp2.py:135-141`).
- Flashing mode: every worker send goes through `transmission_guard` (flock); inhibited send raises and marks disconnected (`tp2_protocol.py:104-119`).
- **Hard constraint**: polling ABS 0x03 "triggers an ESP fault and interferes with Haldex operation on this vehicle" (`references/HALDEX_LOGGING_SPEC.md` ~line 103-107). Nothing in the code blocks it.

### KWP services
| Service | Use | Status |
|---|---|---|
| `10 89` | session on every connect (`tp2_worker.py:369`) | proven (engine, Haldex) |
| `10 84` / `10 85` | Haldex readout privileged / programming | proven by Drew's flashes (README: 180+) |
| `10 86` | probe only (`module_probe.py:39`) | coded, results not in repo |
| `21 nn` | measuring group | proven (engine). AWD/trans groups used by DataView; no traces |
| `18 02 FF 00` -> `18 00 FF 00` -> `13` -> OBD `03` | read DTCs with fallbacks (`tp2_worker.py:815-838`) | coded; which modules need which fallback: unknown in repo |
| `12 00 04 hi lo` | freeze frame per DTC (`:891-897`) | coded; "VCDS uses" comment; decoder PIDs guessed (`tp2_coding.py:421-487`) |
| `14 FF 00` | clear DTCs, then auto re-read (`:796-812`) | coded |
| `1A 9B/91/80/86/9A` | identification (`module_survey.py:52`) | proven via survey |
| `1A 9C` | flash status (Haldex) | proven |
| `1A 9F` | gateway installation list (`tp2_worker.py:261-280`, `installation_list.py:54-68`) | proven: 1 header byte, 4-byte records [VCDS addr, TP2 ID, bus, status] until 0xFF; status bit0 = coded proven; 0x08 "unreachable", 0x04 "fault" guessed |
| `22 F187/F189/F18A/F191/F197/0600` | new-style ident for modules that NRC 11 every 1A (PQ BCM) (`module_survey.py:57, 205-227`) | proven for BCM |
| `22 0610..0671, 064F/0655/0651` | BCM LIN sub-controller coding/part (`module_probe.py:78-88`) | coded |
| `3E` | tester present | coded |
| `27 01/02`, `27 03/04` | Haldex app / readout security (add32 0xBAFB / 0x762B) | proven |
| `3B F0 [b0 b1]` | custom Haldex firmware mode set/query (7716+ images; stock answers `7F 3B 12`) (`haldex_manager.py:56-64, 90-107`) | proven on Drew's firmware; **never send the set before a query proves support** — on older images it reaches the stock coding write (`:546-548`) |
| Coding **write** | not implemented anywhere for diagnostics | — |
| `2C` / `23` | none for engine. `23` exists in `kwp.py:231-234` and the PQ reader allowlist; used for EPS probe only | not tried on engine |
- KWP client: 0x78 pending loop up to 30 s / 30 times (`kwp.py:89-103, 134-142`); optional busy-retry on NRC 0x21 per profile; strict echo/length checks per profile.
- DTC status bytes are decoded with **UDS ISO 14229 bit meanings** (`tp2_coding.py:490-511`). VAG KWP status is different. Treat as wrong.
- Raw passthrough `KWP_REQUEST`/`KWP_RESULT` job API (`tp2_worker.py:282-345`) — any client can send any KWP to any module. The Haldex mode manager uses it.

---

## 2. Value catalog

- Size: **466 canonical values, 629 providers** (`vehicle_data/CATALOG.md:5`). Engine reference coverage: 328/328 labelled field occurrences across 96 groups of the owner's "Measuring Groups - Engine - 01.pdf" (`vehicle_data/README.md:7-11`). Plus transmission 0x02 groups 8/11/12/13/14/16/19 and AWD 0x0A groups 1/3/5 (`consumer_definitions.py`), plus ICAN signals from `PQ35_46_ICAN.dbc` and custom Haldex telemetry 0x6DA/0x679 (`ican_definitions.py`).
- Naming: dotted lowercase namespaces `engine.*`, `transmission.*`, `awd.*`, `vehicle.*`, `ambient.*`, `climate.*`... e.g. `engine.boost.actual_absolute`, `engine.knock.cylinder3.sensor_voltage`. Provider IDs: `diag:{module:02X}:{group}:{block}[:suffix]` (block is **one-based**) and `ican:{canid:03X}:{DBC signal}[:suffix]` (`catalog.py:24, 36, 190-192, 213-215`).
- Unification: one value has a list of providers of both kinds; ICAN providers are inserted first (`catalog.py:47`). Repeated occurrences of a quantity across groups are alternate providers (engine RPM has 44 group locations, `catalog.py:50-53`).
- Entry fields: `id, label, unit, type (number|string|status|bitfield), note, unit_policy ('reported' = take the ECU's unit)`. Provider fields: `verified, estimated, stale_after_ms` (diag 2000, ICAN max(1000, 3x DBC cycle) `ican_definitions.py:17-36`), `reference`.
- Decoding diag: measuring-block triples `[formula, a, b]` -> `TP2Coding.decode_value` (~70 formula types, `tp2/tp2_coding.py:100-418`; table "stolen from OpenHaldex"). Unknown formula -> `"0xAABB"`, unit `Type_N`. Catalog then checks formula is in `_KNOWN_FORMULAS` (`catalog.py:302`), type matches, and converts units through a small alias/conversion table or rejects with `unit_mismatch` (`catalog.py:305-354`). So **meaning comes from group/position; the ECU's formula byte supplies scale and unit**.
- Decoding ICAN: little-endian start/length/factor/offset, sentinel raws, invalid bits, `valid_raw_max`, sign bits, qualifier guards (`valid_when`), enum `choices`; truncated frame invalidates all providers of that frame (`catalog.py:238-287`).
- Trust gates: `verified=False` needs `allow_unverified`; `estimated=True` needs `allow_estimated` (e.g. altitude-derived atmospheric pressure, `catalog.py:103-105`; group 11 fields 5-7 unverified, `:121-126`; MO7 boost unverified, `:106-108`).
- Weak spots: meaning of fields 5-8 unknown; several "same quantity?" questions deliberately left as separate values (README "Semantic boundaries"). All engine mappings are from a PDF, not from Drew's ECU part number ("no ECU part number is required", README:4-5) — groups differ by ECU software.

### How values reach UIs today
ICAN frames -> `can_handler` ZMQ `can_stream.ipc` -> `vehicle_data.service` (launched by `hudiy_dataview/can_service.py`) -> broker -> `HUDIY_VALUES` on `vehicle_data_stream.ipc` per client_id -> `hudiy_dataview/value_bridge.py` (per browser sid, SYNC_VALUES heartbeat 5 s) -> Socket.IO `values_batch`. TP2 values: worker PUB `HUDIY_DIAG` -> broker. `hudiy_dataview/data_logger.py` subscribes directly to raw CAN + `tp2_stream` (bypasses the broker). A legacy "module 0" fake-group stream is still published for old consumers (`vehicle_data/service.py:120-162`).

---

## 3. Scheduler

Two layers in two processes:

**A. `tp2/group_scheduler.py` inside the worker (per module)**
- Interest = union over clients of {normal groups -> period 0 = as fast as possible, low groups -> 60 s, `group_periods_ms`} with fastest-wins (`:38-50`). Clients lease by re-SYNCing within 15 s (`tp2_worker.py:205`).
- `select` round-robins due groups so an unlimited group can't starve others (`:52-63`). `started` sets next_due = start + period; `completed` skips missed deadlines (no catch-up bursts) (`:65-79`).
- `snapshot` reports requested period, count, last success timestamp, last request duration, achieved Hz over last 32 successes including idle age, overrun flag (`:81-104`). Exposed in worker `STATUS.sessions[].group_stats`.
- `group_observation` turns responses into a payload or a typed failure (negative_response with NRC, malformed, transport; transient unless NRC 11/12/31) (`:107-138`) published on `HUDIY_DIAG_OBSERVATION`.

**B. `vehicle_data/broker.py` (value level)**
- Clients `SYNC_VALUES` a complete set with optional `rate_hz`/`period_ms`, `source` pin (`auto|diag|ican|provider id`), trust flags; lease 15 s (`:47-98, 109-113`). Defaults 2 Hz diag, 10 Hz ICAN delivery cap (README:22-24).
- Plan (`:326-425`): per request, use a healthy ICAN provider if any (with hysteresis: after a passive failure, need 2 good samples and 0.5 s before switching back, `:332-340`); otherwise collect diag candidates; weighted greedy set cover over (module, group) with cost = measured request duration / period, discounted if another TP2 client already reads that group fast enough (`:361-384`); redundancy removal pass.
- Plan is pushed to the worker as one SYNC client `vehicle_data` with `group_periods_ms`, every 5 s or on change (`service.py:35-117`). STATUS every 2 s feeds back external groups and stats (`broker.py:237-314`).
- Samples carry `status ok|stale|invalid|unavailable|paused`, `quality{valid,fresh,verified,estimated,reason}`, source, host acquisition `timestamp`, `age_ms`, `max_age_ms` (= max(3x period, stale_after)), `sample_sequence` (`:437-463`). Delivery: on change of status/source, or new acquisition when due (`:465-484`).

**Good:** monotonic deadlines, no catch-up, explicit failure observations, achieved-rate/overrun reporting, ICAN-first with hysteresis, shared reads across clients, provenance on every sample, fail-closed timestamps.

**Patched / weak:**
- Split brain: the broker plans groups, the worker schedules them, and they reconcile through 5 s SYNC + 2 s STATUS polling. Cooldowns duplicated (worker 3x->30 s, broker exponential). Legacy direct-group clients coexist and the broker tries to account for them as "external" demand.
- No packing across rate classes beyond greedy cover; no global budget: the worker never tells clients "you asked 2 Hz x 10 groups, you'll get 0.9". Overrun is only visible after the fact.
- Single-threaded worker; one slow module blocks all.
- `plan()` runs on every 10 ms service loop iteration (`service.py:284`).
- DataView SYNCs module 0 (a CAN pseudo-module) to the worker, which tries to open TP2 channel 0x00 forever (log evidence).
- Nothing knows ABS 0x03 is forbidden.

**VCDS pause / tester conflict:** manual only. User presses "Toggle Diagnostics" (`TOGGLE`/`SET_ENABLED`, `tp2_worker.py:592-629`; help-site guide line 260). Ignition off also stops it (`POWER_STATUS kl15`, `:386-427`). There is **no detection of a foreign tester** (e.g. watching for C0 setups on 0x200 from someone else, unexpected traffic on our channel IDs, or 0x7xx IDs in use). If VCDS and the Pi both talk, nothing notices.

**Exclusive ownership (used by flashing, survey, probe):** `QUIESCE{owner token}` -> worker sets `diagnostic_owner`, main loop closes all sessions and sets `quiescent`, reply only after acknowledgement (10 s) (`tp2_worker.py:441-460, 707-716`). Other commands then refused except STATUS/SYNC/READ_INSTALL_LIST (which only queue) (`:472-473`). `RELEASE{owner, recovery_required}`. Haldex manager has the same handshake and additionally drains the can_handler send queue with a BARRIER message and an ack socket (`haldex_manager.py:691-730`). A persistent `~/.hudiy/haldex_recovery_required.json` makes both services start owned by "recovery-required".

---

## 4. Flashing and readout (Haldex Gen4, module 0x0A)

Protocol: TP2.0 + KWP2000 through the gateway on ICAN, from the Pi's own socket (`flasher/socketcan_device.py`), 2 s timeouts.

**Flash** (`flasher/engine.py:589-685`, policy `controllers/haldex_gen4/protocol.py:224-270`):
1. Offline: image exactly 320 KiB, all four Layer-1 block checksums valid, anti-brick patch sites stock-or-patched, whole-block range; patches applied in memory; loader 0xC5 sum computed (`flasher/README.md:92-147`).
2. Connect (5 tries), `1A 9B` + `1A 9C`, require part 0BR/0AY/0BS and application channel (0x764).
3. `10 89`, `27 01` seed -> key = seed + 0x0000BAFB, `27 02`, `10 85` (expects `50 85 01`; timeout treated as ambiguous).
4. Reconnect up to 15 tries; verify channel is not 0x764 (i.e. loader). Loader security `27 01` must return 11 22 33 44, send `27 02 11 22 EE 3F`.
5. `34` RequestDownload addr/size -> block limit; chunk = min(240, limit-1) rounded to 4.
6. Point of no return: `31 C4 start end BCDdate toolid` erase, poll `33 C4` until `73 C4 00` (0x23 = busy, reconnect on lost channel) (`engine.py:537-560`).
7. `36` per chunk, **never retried** (ambiguous); `37`.
8. `31 C5 start end sum16`, poll `33 C5`.
9. Commit: `20` then `82`; StopCommunication ACK soft-resets; not proof of boot.
10. Close, reconnect, ident again; must be application (0x764, not bootloader, sane flash status) (`protocol.py:230-233`).
- Strict KWP profile: exact session/routine/key responses, single-byte positives for 20/36/37/82 (`protocol.py:58-74`).
- Recovery: `--recovery` skips app steps when already in loader. BSL UART scripts if worse.

**Readout** (`flasher/readout.py:51-183`, `protocol.py:115-135`): must be the running application (0x764), `1A 9B`, `10 89`, `27 03` seed + 0x762B -> `27 04` (expects `67 04 34`), `10 84`, then `35 addr 00 size` / `36` x N (200-byte blocks, strict) / `37` over 4 KiB windows, fsync per window. Cleanup `20`, `10 89`, ident again and compare. Strict request allowlist (`engine.py:184-196`). Sector checksums verified after.

**Exclusive bus control** (`hudiy_dataview/app.py:981-1040, 1193-1259`; `flasher/traffic.py`; `references/FLASHING_MODE.md`):
1. `flashing_operation()` flock (prevents toggling Flashing Mode off mid-flash).
2. `DiagnosticOwnership.acquire`: QUIESCE Haldex manager (drains CAN send queue) then TP2 worker, each must ack quiescence with the same token; process-local lock against a second operation.
3. `set_flashing_mode(True)`: persistent `~/.hudiy/flashing_mode.json` (fsync file+dir). Every application `bus.send` (can_handler, DIS/DDP, TP2 worker, Openpilot, Haldex manager, exhaust valve) holds the same flock and drops the frame when enabled. Corrupt state = inhibited.
4. Recovery marker written before any CAN work; cleared only on verified success. Mode stays ON after the flash until the user turns it off.
- **What stops:** DIS top display and DDP session (socket closed), all TP2 polling and the Openpilot receiver, Haldex KWP mode sets/queries, exhaust valve 0x67A, other can_send producers. **What keeps going:** TV presence 0x602 (explicit exemption so the RNS-E TV input survives), passive RX/logging, Hudiy. **NM:** the Pi never transmits OSEK NM (0x420-0x43F); it only observes the ring (`rns-e_can/can_base_function.py:112-142, 492-535`). So nothing NM-related needs stopping; the open question is whether the bus stays awake for a long flash (README line 71-73 lists this as unaccepted bench work).
- Doc admits: "No physical CAN/J2534 testing was performed while implementing this feature" (`FLASHING_MODE.md:75`). Drew's 180+ flashes were mostly with a J2534 clone, on the bench (`flasher/README.md:6-10`).

---

## 5. Recommended abstractions for the Rust daemon

### Hard constraints
1. One physical CAN interface, ICAN 100 kbit/s, shared with DIS/DDP, NM observation, Haldex telemetry at 50 Hz, and everything else. Diagnostics gets a slice of 100k, not the bus.
2. The gateway is in the path. Unknown: max concurrent channels, whether it serialises channels internally, behaviour when two testers use the same tester RX ID.
3. Wire floor ~50 ms per group read on the engine; ECU first-byte latency 23-40 ms dominates. Rate budget per channel ~15-20 req/s at best. Per-module, not per-bus, if channels interleave well (unproven).
4. Never touch ABS 0x03 by default. Never send `3B F0 b0 b1` to a Haldex that hasn't answered the `3B F0` query. Never retry `36`. Never enter `10 85` outside the flash state machine.
5. A tester on the OBD port (VCDS) is a peer on the gateway; today only manual pause.
6. Flashing needs: all other app TX stopped and *acknowledged* stopped; persistent, crash-safe state; recovery mode survives reboot.
7. Meaning of measuring-block fields depends on ECU software; group layouts in the catalog came from a PDF.

### TP2 transport / channel manager
- One task owns the CAN socket for all diagnostics (and ideally the whole ICAN TX path, with a priority queue), demuxing by CAN ID. No per-session sockets.
- `ChannelManager` holds a tester-ID pool, a per-module `Channel` state machine (`Closed -> Opening -> Parameters -> Session -> Ready -> Closing`, plus `Backoff(until)`), and timers for A3 (1 s) and 3E. Channel open/close are scheduled operations with known cost (~70 ms), so the scheduler can decide to keep a channel warm.
- Grace period: keep a channel open N seconds after its last consumer leaves (today: 2 s hold for KWP jobs; slow-only sessions close immediately).
- Interleave requests on different channels (async, one outstanding request per channel) instead of the worker's sequential loop. Configurable `max_open_channels` (start at 2-3) until Drew measures the gateway.
- Typed errors: SetupRefused(D6/D7/D8), SetupTimeout, AckTimeout, PeerDisconnect(A8), SequenceError, Busy(0x9x), with per-module backoff in one place.
- Keep `segment`/`reassemble` as pure functions (they're reusable for DDP-style fixed channels, `tp2.py:3-7`).

### KWP client
- `async fn request(channel, bytes, profile) -> Result<Positive, Kwp>`; handles 0x78 pending with a deadline, optional busy-retry per service, profile validation (echo, exact responses). Port the profile idea from `kwp.py:89-103`.
- A request classifier: `ReadOnly | Volatile (2C define, 3B to RAM handler) | Persistent (coding write, 14 clear) | Programming (10 85/34/31/36/37)`. The API refuses anything above ReadOnly unless the caller holds the matching exclusive mode. Today `KWP_REQUEST` lets anyone send anything.
- Per-module dialect cache from probing (1A vs 22 ident, 18 02 vs 18 00 vs 13 DTC), filled from the installation list + one probe.

### Value catalog and source resolution
- Keep the shape: `Value{id, label, unit, kind}` + `Provider::{Ican{frame, signal...}, Group{module, group, field}, Dyn{module, address, len, scale}, Derived}` with `verified/estimated`, `stale_after`. Static data (generated from DBC + group tables at build time or loaded from TOML).
- Key the group tables by ECU identity (part number + SW version from `1A 9B`) so a group layout is only trusted on matching software. Fields 5-8 included.
- Resolution order: healthy ICAN -> already-scheduled group -> cheapest new group -> unavailable. Keep the passive-fallback hysteresis.

### Subscription / lease scheduler
- Single in-process scheduler (no split between broker and worker). Inputs: `Subscription{client, value, period, priority, max_staleness, source_policy, lease}`.
- Per channel, a token budget derived from measured request cost (EWMA of duration per (module, group)). Pack values into groups (set cover) per rate class; then schedule groups EDF with priority classes; drop lowest priority first when over budget, and say so.
- Lease renewal as a cheap heartbeat; leases expire -> group dropped -> channel enters grace.
- DTC reads, install list, identification and one-off KWP are jobs in the same queue with priorities (interactive > live values > background).

### Exclusive modes
- `ExclusiveLease{owner, kind: VcdsPause | Flashing | Survey | Coding, scope: Bus | Module(id)}`. Acquire = stop scheduling, finish or abort in-flight request, send A8 on all affected channels, confirm, then grant. Persist Flashing/Recovery to disk (fsync) and restore at boot before any TX.
- Flashing scope must also stop non-diagnostic ICAN TX (DIS/DDP, valve, Haldex mode) except an allowlist (0x602). If the new daemon owns all ICAN TX, this is one gate instead of six flocks.
- VCDS: add passive detection — C0 setup requests on 0x200 whose tester RX ID isn't ours, or traffic on a channel ID we didn't open — and auto-enter `VcdsPause` (with a user override), auto-release after N seconds of silence.

### API: achieved rate and staleness
- Every sample: value, unit, source provider, acquisition time (host, monotonic + wall), age, sequence, status (ok/stale/invalid/unavailable/paused/denied), reason.
- Per subscription: requested period, granted period (what the scheduler committed to), achieved Hz (rolling), p95 age, last error, and a "degraded because" (budget, cooldown, exclusive mode, module absent).
- Per channel/module: open/closed, request cost EWMA, utilisation %, error counts.
- Push a subscription-status event whenever the granted period changes, not just samples.

### Side topic: faster engine reads (0x2C / 0x23)
- Nothing in the repo has tried it on the engine. `kwp.py` has `read_memory_by_address`; `PQMemoryReader` already does 23/35 reads with an allowlist on Haldex/EPS.
- ME7Logger style: `2C F0 03 ...` (defineByMemoryAddress, RAM address+len pairs) once, then `21 F0` returns all defined bytes in one response. Gain is density, not latency: one ~50 ms request can carry dozens of raw RAM variables at full resolution instead of 8 scaled fields, so maybe 15-20 Hz for a whole custom set vs ~2 Hz per group with 5 groups.
- Needs: ECU family must support 2C (ME7 does; the TT 8J's engine ECU family is unknown in the repo — 2.0 TFSI would be MED9.1/MED17, which is not ME7), an address map for Drew's exact SW version (from a flash dump + definition), possibly security access. RAM-only definitions are volatile, but treat as `Volatile` class and require explicit opt-in.
- Probe safely first: identify ECU (`1A 9B`), then test `2C` support with a single small define + `21 F0`, read-only otherwise.

### Things to port as-is (they were hard-won)
- Timing byte decode; length bit-15 flag; re-ACK duplicate final frame; 0x9x deadline extension; A8 reply etiquette.
- Haldex flash/readout sequences, strict profiles, no-retry TransferData, recovery marker, fresh-ident verification.
- Install-list parser and the module notes (BCM vs KSG, absent coded modules, RNS-E double address).
- Survey/probe matrix as a read-only "learn the car" tool.
