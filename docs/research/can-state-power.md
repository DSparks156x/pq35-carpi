# ICAN signal layer, vehicle state, power and bus sleep — research notes

Scope: Drew's TT 8J (PQ35), RNS-E 193 on r21, Pi 5 + CarPiHat Pro 5, repo `RNS-E-Hudiy` @ `testing` (d5383be).
Read-only research. Paths are relative to the repo root unless absolute. "Guess" = old code or doc asserts it without on-car evidence I could find.

Evidence base:
- `references/canlog.txt` — 3041 frames, no timestamps. 0x470 (50 ms) has 214 frames, so the log is ~10.7 s, ignition on (0x2C3 = 07), engine off (charge lamp on), car parked, lights off.
- `references/PQ35_46_ICAN.dbc` — BusNet PQ export, 2008, 100 kbit/s, 235 messages, 2084 signals.
- `references/ICAN_rundown.md` — auto-translated DBC dump of the 41 IDs in canlog.txt. Its "Log Frequency" counts are frame counts in that ~10.7 s log, not rates.
- Analysis script (scratch, not in repo): `C:\Users\raccoon\AppData\Local\Temp\claude\0d19c1a42222ca8b\66e23859-655e-46b7-b48c-43762d830db5\scratchpad\dbc.py` (parses BO_/SG_/BA_ attributes).

---

## 1. Signal model

### 1.1 IDs seen on this car (canlog.txt), with DBC cycle and observed DLC

All senders are Gateway_PQ35 unless noted. Cycle = DBC `GenMsgCycleTime` (ms). Frame counts in a ~10.7 s log match the cycle within ~1 frame unless flagged.

| ID | DBC name | Cycle | DLC DBC/car | What it is |
|---|---|---|---|---|
| 0x151 | mAirbag_1 | 100 (fast 20) | 4/4 | crash bits, counter + XOR checksum |
| 0x2C1 | mLSM_1 | 100 | **6/5** | stalk/light switch module: indicators, high beam, wipers, MFA buttons |
| 0x2C3 | mZAS_Status | 100 | 1/1 | ignition switch: S, 15, X, 50, P (bits 0-4). Log: `07` |
| 0x351 | mGateway_1 | 100 | 8/8 | GW1 speed (0.01 km/h), reverse light, dated bits. Log: all zero |
| 0x359 | mGW_Bremse_Getriebe | 100 | 8/8 | GWB speed, path pulses, ABS/ESP flags, selector, dated bits |
| 0x35B | mGW_Motor | 100 | 8/8 | RPM (0.25 rpm), coolant, brake/clutch switches, fuel counter |
| 0x363/0x365 | mRadio_1_neu/2_neu | IfActive (fast 800) | 8/8 | DIS top lines. Sent by RNS-E or the Pi (see TX list) |
| 0x3C3 | mLenkwinkel_1 | 100 | 8/8 | steering angle, rate, status, counter, CRC8 + checksum |
| 0x3E1 | mClima_1 | 100 | 8/8 | outside temp, AC pressure, blower |
| 0x3E3 | mClima_2 | 800 | 8/8 | cabin temp, solar, set temp |
| 0x42B | mNM_Gateway_I | NM | 6/6 | OSEK NM, gateway. Log: `16 01 00 00 00 00` |
| 0x436 | NWM_RNS | NM | **8/6** | OSEK NM, RNS-E. Log: `0B 01 00 00 00 00` |
| 0x470 | mBSG_Kombi | 50 | 8/8 | doors, hood, boot, lamps, 58d/58t dimming, rest voltage |
| 0x523 | mGW_KOM_INF | 200 | 8/8 | locking/alarm, displayed km/h |
| 0x527 | mGW_Kombi | 200 | 8/8 | speed, tyre circumference, outside temp filtered/raw |
| 0x531 | mLicht_1_alt | 50 | 4/4 | lights (side/dipped/main/fog/reverse/indicators/brake/DRL), counter + XOR |
| 0x555 | mMotor7 | 100 | 8/8 | boost, oil temp, `MO7_Sleep_Ind` |
| 0x557 | mKD_Error | 500 | 8/8 | per-module DTC-present bits |
| 0x571 | mBSG_2 | 500 | 6/6 | battery voltage, load shedding, `BS2_aus_Infotainment` |
| 0x575 | mBSG_3 | 200 (delay 20) | 4/4 | terminals S/15/X/50/P from BCM, battery state. **66 frames vs 53 expected**: on-change sends on top of the cycle |
| 0x58C | mMotor_10 | 100 | 8/8 | start-stop, manifold pressure |
| 0x5B5 | mBremse_10 | 200 | 8/8 | 4x wheel path pulses + direction, counter + XOR |
| 0x5C3 | mMFL_Cmd_ | 500 | 2/2 | steering wheel buttons |
| 0x604 | mNAV_1 | 1000 | 8/8 | sender RNS_300_NF (the RNS-E here) |
| 0x60E | mEinheiten | 1000 | 2/2 | cluster unit settings. Log `C7 00`: miles, °F, gallons |
| 0x621 | mKombi_K1 | 100 | 7/7 | fuel L, handbrake, display brightness, light sensor |
| 0x623 | mKombi_K2 | 1000 | 8/8 | cluster clock (time sync source) |
| 0x627 | mKombi_K4 | 500 | 8/8 | display request/ack |
| 0x635 | mDimmung | 200 | 3/3 | DI1_Display / DI1_Schalter / DI1_Sensor. Log `25 00 23` |
| 0x651 | mSysteminfo_1 | 100 | 8/8 | `SY1_Sleep_Infotainment`, `SY1_Sleep_Komfort`, diag flags |
| 0x653 | mGateway_3 | 500 | 3/3 | country/language/engine type |
| 0x655 | mSollverbau_neu | 500 | 8/8 | coded installation list |
| 0x658 | mBEM_02 | 100 | 8/8 | battery SOC, generator, shutdown stages |
| 0x65D | mDiagnose_1 | 1000 | 8/8 | odometer km (20 bit), date/time |
| 0x65F | mFzg_Ident | 200 | 8/8 | VIN, **multiplexed** (FI1_MUX 0..2) |
| 0x661 | mRadio_4 | 1000 | 8/8 | sender is the RNS-E here (DBC says Radio_2DIN). Log `83 01 12 37`: 0x37 = TV source |
| 0x6C0/0x6C1/0x6C2 | mTp_* | — | 8 / var | TP2.0 channels (DDP to cluster). 0x6C2 shows `A0 0F 8A FF 4A FF`/`A8` |
| 0x6DB | BAP_Clima | — | var | BAP |

Not seen in the log but used by old code: 0x2A1 mNavig_1 (yaw), 0x529/0x52B/0x629/0x62B/0x62D (MFA trip), 0x545/0x54B (park assist), 0x5E1 (Clima_3), 0x461 (r21 MMI keys), 0x602 (TV, Pi-sent), 0x7B0/0x7B6 (r21 bridge), 0x679/0x6DA/0x67A (custom Haldex/exhaust). They may only appear in other states (driving, nav on, TV mode), or never on a TT. **The log is one 10 s parked snapshot; a driving log is needed before trusting any rate or ID list.**

### 1.2 DBC attribute model (what a signal definition can be built from)

Per message (BA_ BO_): `GenMsgCycleTime` (175 of 227 set), `GenMsgCycleTimeFast` (30), `GenMsgSendType` (Cyclic / IfActive / NoMsgSendType), `GenMsgDelayTime` (min gap between event sends, e.g. 0x575 = 20 ms), `NmMessage` (1 only for the ring IDs), `Timeoutzeit` (defined, never set — so **no per-message timeout comes from the DBC; it must be derived from the cycle**).

Per signal (BA_ SG_, ~2031 each): `GenSigFuncType`, `GenSigStartValue`, `Fehlerwert`, `GenSigInactiveValue`, `GenSigSwitchedByIgnition`, `NWM-WakeupAllowed` (never 1). Plus `VAL_` tables (799) carry named error/init values.

`GenSigFuncType` counts across the whole DBC:
- `DatedBit` 30 — the "Alt"/"alt"/"_alt" obsolete flags (`GW1_FhzgGeschw_alt`, `GWB_Alt_*`, `GWM_Alt_*`, `GWI_alt_*`, `GWK_Alt_*`, `GW3_Alt_*`, `KKO_alt_mBSG_Kombi`, `DN1_alt_*`). Start value 1 = "outdated". 1 means the gateway has not received the source frame recently and is repeating a stale/substitute value.
- `MsgCounter` 4 — `AB1_MessageZaehler` (0x151), `B10_Zaehler` (0x5B5), `LW1_Zaehler` (0x3C3), `LIA_Zaehler` (0x531).
- `XOR-2` 3 — `AB1_Pruefsumme`, `B10_Checksumme`, `LIA_Pruefsumme`. Verified on the log: XOR of all other data bytes (0x5B5 `0E 01 … 0F`: 01^0F = 0E; 0x531 `00 A0 40 E0`: 00^A0^40 = E0).
- `CRC-8` 1 — `LW1_CRC8CHK` (0x3C3 byte 6). Polynomial not in the DBC. 0x3C3 byte 7 (`LW1_Pruefsumme`) is not a plain XOR of bytes 0-6 (checked on one frame). Unknown.
- `SW-Kl15` 2 — `ZS1_ZAS_Kl_15` (0x2C3 bit 1), `BS3_Klemme_15` (0x575 bit 1). These are the canonical terminal-15 sources.
- `Constant` 2.
- `GenSigSwitchedByIgnition` = 1 on 165 signals: only meaningful with Kl15 on.

Error/init conventions (VAL_ names, German): `Fehler` (error), `Init`/`INIT`/`Initwert`/`Init_PQ25_35_46`, `Unterspannung` (undervoltage), `Sensorfehler`, `ungueltig` (invalid), `nicht_verfuegbar`, `Ersatzwert` (substitute). Typical raw patterns:
- n-bit unsigned: all-ones = error (`255`, `127`, `1023`, `63`, `15`, `31`, `7`), all-ones-minus-one = INIT (`254`, `14`). E.g. `DI1_Sensor`/`KO1_Lichtsensor` 254 INIT / 255 Fehler; `BEM_Ladezustand` 14 Init / 15 Fehler.
- 15-bit speeds: 32708 Init, 32725 Undervoltage, 32742 Sensor error (GWB/GW1 speed, start value 32708).
- 10-bit wheel pulses: 1021 Init, 1022 Undervoltage, 1023 Sensor error (start 1021).
- RPM 16-bit: 65280 (0xFF00) Fehler.
- Separate qualifier bits: `*_def` (58d/58s/58t not OK), `*_Fehler`, `QB_*` ("1 = replacement/init/error value"), `LW1_Int_Status` (0 ok, 1 no init, 2 sporadic, 3 permanent), `LW1_ID` 128 = calibrated.
- 0 is sometimes "not installed" (`MO7_Oeltemperatur` 0 not installed, 1 Init, 255 Fehler).
- Sign-magnitude, not two's complement: steering angle/rate magnitude + separate sign bit (`LW1_Vorzeichen` bit 15, `LW1_Geschw_Vorzeichen` bit 31), `MO7_Gradient_Vorz`.
- Multiplexing: only 0x65F (FI1_MUX, VIN in 3 pages, page 0 = "Geheimnis" secret bytes) and 0x3A2 PSD_02 (not on this car). 82 mux signals total.
- All signals are Intel (little-endian) — no Motorola signals in this DBC.

DBC vs car mismatches already seen: 0x2C1 DLC 6→5, 0x436 DLC 8→6, 0x661 sender is RNS-E, 0x363/0x365 sender "Vector__XXX". `GWK_AussenTemp_Fehler` VAL table maps both 0 and 1 to "OK" (rundown line for 0x527) — the DBC itself is not trustworthy on qualifier polarity. `GWB_Info_Waehlhebel` reads 8 ("Pos P") on the log frame; the TT may be manual — treat as invalid unless the car is S tronic (question).

### 1.3 What a signal definition needs (proposal)

Per message: id, dlc expected (car-observed, not DBC), cycle_ms, send type, timeout_ms (e.g. 3× cycle with a floor, or 2.5× for 50/100 ms frames), counter location + width, checksum kind + location, mux selector if any, "dated bits" in the frame and which signals each one guards.
Per signal: start, length, Intel, factor, offset, unit, signed-ness model (unsigned / sign-magnitude with sign bit), named enum, invalid raw set (error/init/undervoltage), valid raw max, qualifier bits that invalidate it (`invalid_when bit=1`) or must be set (`valid_when`, e.g. steering `LW1_Int_Status == 0`, tyre circumference `GWK_Reifenumfang_empf == 1`), "needs Kl15" flag, freshness = message timeout.
State output per signal: `Value(v) | Invalid(reason) | Stale | NeverSeen` — never a default number.

The old `vehicle_data/ican_definitions.py` already does most of this by hand: `invalid_raw`, `invalid_bits`, `valid_when`, `valid_raw_max`, `sign_bit`, `frame_length`, `stale_after_ms = max(1000, 3 × cycle)` (`vehicle_data/ican_definitions.py:16-37`). It ignores counters/checksums and Kl15 gating. Good reference for semantics, not to port.

### 1.4 Codegen vs hand-curated

- DBC: 235 msgs / 2084 signals. Seen on this car: 41 IDs carrying ~779 DBC signals. Old code: `vehicle_data` defines 249 values on 26 ICAN IDs + 2 custom Haldex IDs (`vehicle_data/ican_definitions.py`; count via SIGNALS); `can_base_function.py` hand-parses 0x2C3, 0x470, 0x42B/0x436/0x420-0x43F, 0x635, 0x623; DIS uses 0x351/0x60E/0x2C1/0x5C3; dark mode uses 0x635.
- Recommendation: **generate a typed table from the DBC (build-time script → Rust source checked in), filtered by an allowlist of car-observed IDs, plus a small hand-written overlay** for what the DBC gets wrong or lacks: car DLCs, timeouts, qualifier polarity, which dated bit guards which signal, checksum/CRC algorithms, Kl15 gating, sign-magnitude pairs. Full runtime DBC parsing is not worth it; full hand-curation of 700+ signals is error-prone (the DBC has 2031 `Fehlerwert`/start values to carry). The DBC is a 2008 generic PQ35/46 file: it lists messages the TT never sends, and its DLCs are wrong for 2 of 41 IDs.

---

## 2. OSEK NM on ICAN, bus sleep, transmit gating

### 2.1 What the DBC says

- `NmType "VAG-OSEK"`, `NmBaseAddress 1024 (0x400)`, `NmMessageCount 64` → NM range 0x400-0x43F; every defined NM node is 0x426-0x43D. `GenNWMSleepTime 4000` ms. No TTyp/TMax/TError/TWaitBusSleep in the DBC.
- `NmStationAddress`: Gateway_PQ35 43 (0x2B → ID 0x42B), RNS_300_NF/RNS_MFD2/RNS_midline 54 (0x36 → 0x436), Radio 57 (0x439), Booster_AMP 51 (0x433), MDI 53, TV_hybrid 55 (0x437), UHV phone 58 (0x43A), Rearview 41, Tastatur 38, Standheizung 39, GSM 40, DAB 47, Kompass 52, SBS 60, Telematik 61.
- NM frame layout (same for 0x42B 6-byte and 0x436 8-byte):
  - byte 0: `Receiver` = logical successor address. On the car: gateway sends `16` (= 0x436-0x420), RNS-E sends `0B` (= 0x42B-0x420). So byte 0 is "NM ID − 0x420".
  - byte 1 (bits 8-15): bit 8 `CmdRing`, bit 9 `CmdAlive`, bit 10 `CmdLimpHome`, bit 12 `SleepInd`, bit 13 `SleepAck`.
  - Gateway bytes 2-5: wake causes (Kl30 reset bit 20, overrun timer 21, NWake 22, CAN 23, wake line 24, comfort CAN 25, infotainment CAN 26, Kl15 30, diag CAN 31, LIN 32/33).
  - RNS-E bytes 2-7 (DBC 8 bytes, car sends 6): `Per_WakeUp` (bits 16-20: 0 functional, 1 CAN, 2 P wake, 3 KL15_HW, 4 KL30 reset, 5 ON button, 6 phone mute, 7 RTC, 8 eject, 9 CD insert), `Fkt_WakeUp` (21-23: 0 P, 1 dimming, 2 Comm Req, 3 phone mute), `Klemme_15` (24), `Diagnose` (25), `MindestAktivZeit` (26), causes 4-8 (27-31: Radio BAP, TP20 RNA, TP20 radio), `Nachlauf_1..8` overrun (32-39), timeout error, comp. protection, transport mode, `Abschaltst_aktiv` (46), single-wire (47).
- Related sleep signals outside NM: `SY1_Sleep_Infotainment` / `SY1_Sleep_Komfort` (0x651 bits 4/2, "Bus rest"), `MO7_Sleep_Ind` (0x555 bit 46), `BS2_aus_Infotainment` / `BS2_Warn_Infotainment` (0x571 bits 30/41: BCM load shedding of infotainment), `BEM_02_Abschaltstufen` (0x658).

### 2.2 What the car shows

- Ring has exactly two members: gateway 0x42B and RNS-E 0x436. 115 + 114 frames in ~10.7 s → each node transmits about every 93 ms; ring messages (`CmdRing`), no SleepInd, no alive/limp-home. Nothing else in 0x400-0x43F.
- The Pi is not a ring member and must not become one.
- No log exists in the repo of the ignition-off → sleep sequence (SleepInd/SleepAck order, how long after Kl15 off, whether the RNS-E holds the ring awake while the Pi transmits). **This is the single most important capture to make before writing the sleep state machine.**

### 2.3 What the old code does

`rns-e_can/can_base_function.py`:
- Kl15 / key: 0x2C3 byte 0 bit 0 = "key in lock" (DBC: terminal S), bit 1 = Kl15 (`:543-607`). 0x575 is not used for power (only `vehicle_data` reads it).
- NM: subscribes to 0x420-0x43F (`:492-497`, `:802-806`); SleepInd = byte 1 bit 4, SleepAck = byte 1 bit 5 (`:504-506`), matching the DBC. Tracks every member's last seen time + SleepInd (`:117-120`, `:518`).
- `is_radio_active()`: 0x436 seen < 5 s ago and its SleepInd = 0 (`:139-148`). This gates brightness, ADC and the DIS top-line writer.
- `is_bus_ready_to_sleep()` (`:150-177`): false while Kl15 = 1; true if ignition has been off ≥ `fallback_seconds` (900), or a SleepAck was seen, or every member seen in the last 5 s shows SleepInd (true also when no member is seen).
- On SleepAck with Kl15 ≠ 1 → listen-only immediately (`:523-531`). Otherwise when ready-to-sleep holds, wait `delay_seconds` (10) then listen-only (`:891-920`). Any member with SleepInd = 0 cancels and leaves listen-only (`:519-522`, `:911-920`).
- Listen-only is set by `sudo ip link set can0 down; … type can listen-only on|off; … up` (`:324-348`).
- TX gating: `listen_only_inhibits_tx()` = listen-only on, desired, switching, or pending (`:643-650`). Only the base service's own senders check it (0x602 TV, 0x7B0 brightness/ADC). DIS top lines check `radio_active`; DIS DDP and TP2 check Kl15 via POWER_STATUS; Haldex/exhaust check only the flashing guard. `can_handler.py` forwards everything it is given, guarded only by Flashing Mode (`rns-e_can/can_handler.py:249-253`), with 0x602 exempt even from that (`:78-79`).
- POWER_STATUS publish (ZMQ, `:408-428`) every 1 s and on change: kl15, kls, bus_active, radio_active, doors, wake_signal.

Weak points / guesswork in the old design:
- The 0x602 TV frame is sent every 500 ms whenever listen-only is not pending (`:748-757`). FixList says traffic keeps the RNS-E awake (`FixList.md:7`). If the RNS-E never raises SleepInd while 0x602 arrives, "all members SleepInd" never becomes true and the 900 s fallback is what actually ends it. Not verified either way.
- `ip link down/up` makes `can_handler`'s python-can socket error out; it then sleeps 5 s before reopening (`can_handler.py:337-342`, `:307-311`), so RX is blind for ~5 s on every mode switch, including the wake switch. The bus is also owned by systemd-networkd (`install.sh:618-627`, BitRate 100K, RestartSec 100ms) — unknown whether networkd re-applies settings and drops listen-only.
- Several processes open their own SocketCAN sockets and transmit without going through the gate (DDP `dis_client/ddp_protocol.py:692-713`, TP2 `tp2/tp2_protocol.py:104-113`, AVSD `avsd/app/can_link.py:51-61`, flasher). Listen-only at the kernel level is the only thing that really stops them; the app-level gate is advisory.
- 0x635 "lights on" test differs between services (see §4).
- NM range comment says 0x420+addr; DBC says base 0x400. Harmless today.

### 2.4 Complete Pi TX list (repo-wide, runtime code; tests/emulators excluded)

| ID | Payload | Rate | Sender | Gate |
|---|---|---|---|---|
| 0x602 (mTV_A_1) | `features.tv_simulation.payload`, config `0112302020202020` (default `0912302020202020`) | every 500 ms, always while awake | `can_base_function.py:748-757` via can_handler | listen-only pending/on; exempt from Flashing Mode (`can_handler.py:78-79`) |
| 0x7B0 | `B7 lvl` dial 0-10, `BB lcd` 0-100 (0 = native), `BA src` 0-2 (r21/"r38" bridge, `rns-e_can/rnse_control.py:42-53`) | once per change, re-armed on RNS-E wake (`rnse_control.py:324-361`) | `can_base_function.py:654-664` | listen-only, Flashing Mode, radio_active |
| 0x7B0 | `BC reg val` ADC write, `BD` dump (`hudiy_manager/rnse_adc_protocol.py:219,238`) | on user action | can_base_function | same + radio_active (`:689-693`) |
| 0x7B0 | B6/B7/B8/B9/… RAM/flash ops (AVSD) | on user action | `avsd/app/can_link.py`, `avsd/app/rnse_flash.py` — own raw socket | refuses if can0 listen-only (`can_link.py:41-46`) |
| 0x363 / 0x365 | DIS top lines, 8 chars each | 10 Hz heartbeat while owning (`dis_client/dis_top_display_service.py:57,424-440`) | via can_handler | radio_active from POWER_STATUS (`:488-507`), Flashing Mode |
| 0x6C0 (default; 0x6C4 phone variant) | TP2.0/DDP to cluster: A0/A1 setup, A3 keep-alive every ~2 s (`ddp_protocol.py:1757-1759`), data, B-acks | while a DIS session is open | `dis_client/ddp_protocol.py:692-713` — own socket | Flashing Mode; dis_service runs on kl15 or bus_active (`dis_service.py:1396`) |
| 0x200 + 0x300-0x306 | TP2.0 channel setup to modules, KWP requests, `3E` tester present | while diagnostics are subscribed | `tp2/tp2_worker.py`, `tp2/tp2_protocol.py:104-113` — own socket | Kl15 and user-enabled (`tp2_worker.py:393-418`) |
| 0x67A | exhaust valve command, 3× 20 ms per command | on user action | `rns-e_can/haldex_manager.py:430-450`, `hudiy_dataview/exhaust_valve.py:17` | Flashing Mode only |
| 0x307 / various | openpilot TP2 receiver | disabled (`config.json` `openpilot.enabled=false`) | `tp2/openpilot_receiver.py` | — |
| flasher IDs | KWP2000/TP2 flashing of Haldex/EPS | on user action | `flasher/` — private path, exempt from the guard | — |

The Pi never transmits NM frames, 0x2C3/0x575, or anything in 0x400-0x43F.

### 2.5 Facts the new bus-sleep state machine and TX gate should rest on

1. Terminal state comes from two sources: 0x2C3 (ignition switch, 100 ms) and 0x575 (BCM terminals, 200 ms + on-change). Treat Kl15 as known only while one of them is fresh; stale = unknown, not off.
2. Ring membership is learned, not assumed: any ID 0x400-0x43F seen recently. On this car: {0x42B, 0x436}.
3. Bus is "awake" if any NM frame or any cyclic frame arrived within ~TError (use ≥1 s; measure). Bus is "going to sleep" once a SleepAck is seen with no later SleepInd = 0. Bus is "asleep" after silence ≥ the observed TWaitBusSleep (DBC hint: 4000 ms; measure).
4. The RNS-E is "awake" while 0x436 is fresh and its SleepInd = 0. That is the only gate needed for 0x7B0 / 0x363 / 0x365.
5. TX gate must be one choke point in the daemon (nothing else opens a TX socket), with reasons: `KL15_OFF`, `SLEEP_INDICATED`, `SLEEP_ACKED`, `BUS_SILENT`, `FLASHING`, `RADIO_ASLEEP`. Per-ID policy: which IDs may be sent with Kl15 off (probably none except deliberate RNS-E commands while 0x436 is awake).
6. After SleepAck or silence the daemon must send nothing at all, because one frame wakes every ICAN node (and through the gateway possibly more). Kernel listen-only is a backstop, not the mechanism; switching it should not cost RX (open the socket after the mode change, or use a CAN controller that supports the change without down/up).
7. The Pi must decide whether it is allowed to keep the RNS-E awake (0x602) after Kl15 off. That is a policy choice for Drew, not a protocol fact.

---

## 3. Power

### 3.1 Hardware as configured

- CarPiHat Pro 5 (`pi-setup/README.md:16`, `pi-setup/preexisting.md:50-52,137`):
  - `gpio=25=op,dh` — GPIO 25 driven high at firmware boot = 5 V latch held.
  - `dtoverlay=gpio-poweroff,gpiopin=25,active_low` — after halt the kernel drops GPIO 25, the hat cuts 5 V (`pi-setup/files/boot/firmware/config.txt:21-23`).
  - `dtoverlay=mcp2515-can0,oscillator=8000000,interrupt=23` — MCP2515, 8 MHz, IRQ GPIO 23, SPI (`config.txt:26`). (`example_config.txt:91` and `install.sh:581` default to 12 MHz/GPIO 25 — upstream values, wrong for this hat.)
  - Ignition-sense input on GPIO 12. Drew wires the RNS-E amplifier wake (remote) output to it, not real Kl15 (`README.md:48`, `oldREADME.md:3,5,16`). The RNS-E raises amp wake on unlock/door open, so the Pi boots before ignition.
  - No HAT EEPROM (`force_eeprom_read=0`, `pi-setup/boot-tuning.md:37`). `PSU_MAX_CURRENT=5000` in EEPROM because the Pi can't see the supply (`preexisting.md:32`).
  - Memory note: the hat's 5 V is weak and noisy, and it has no CAN-woken MCU.
- `can0`: systemd-networkd, 100 kbit/s, `RestartSec=100ms` (`install.sh:618-627`). ICAN is fault-tolerant low-speed CAN (memory note) — which transceiver the hat really has is a question.

### 3.2 Shutdown path today (`config.json:213-243`, `can_base_function.py:609-641,857-976`)

- `gpio_shutdown`: enabled, pin 12, `active_low: true`, delay 60 s. gpiozero `Button(pull_up=None, active_state=False)`; "pressed" = pin low = wake **off**. Wake off for 60 s → `sudo shutdown -h now`. Wake back on cancels.
- `auto_shutdown` on Kl15 off: disabled (delay 900 s when on). It refuses to shut down while the wake line is active (`:183-203`).
- Listen-only: enabled, delay 10 s, fallback 900 s after Kl15 off.
- systemd stop timeout 10 s (`pi-setup/files/etc/systemd/system.conf.d/99-boottuning-stop-timeout.conf`); measured shutdown ~5 s (`pi-setup/boot-tuning.md:174`); power-on → Hudiy ~6-7 s app / ~12.8 s from power (memory note).
- Old ignition watcher `carpihat.service` (GPIO 12, 10 s) is disabled (`preexisting.md:137`).
- The chain that has to work: Kl15 off → Pi stops TX → RNS-E sleeps → amp wake drops → GPIO 12 low 60 s → halt → GPIO 25 low → hat cuts 5 V. Any Pi frame on ICAN keeps the RNS-E (and so the Pi) alive (`oldREADME.md:5`, `FixList.md:7`).

Gaps: nothing handles wake returning during the ~5 s halt (does the hat re-power, or does the Pi stay dead until the next wake edge?). Cranking (Kl50) dips on a weak 5 V rail are not handled. The `active_low` name is inverted relative to what it means ("wake present = pin high").

### 3.3 What a future hat MCU (CAN-aware, owns power) must do

- Own the 12 V → 5 V switch for the Pi, and wake on: ICAN activity (FT transceiver WAKE/INH, e.g. TJA1055-class), amp-wake line, optional Kl15 wire.
- Listen to ICAN only (never transmit, or have TX physically disabled when asleep) and track: 0x2C3/0x575 Kl15/S, NM ring (0x42B/0x436 SleepInd/SleepAck, silence), 0x470 doors, 0x523 unlock.
- Tell the Pi (UART/I²C/GPIO) why it is on and what the car state is, and accept "ready to power off" plus a requested off-delay.
- Pi shutdown handshake: Pi requests off → MCU waits for halt signal (GPIO 25-style) or a timeout → cuts power. If wake returns mid-halt: finish the cut, wait ~1 s, re-power.
- Hard timeouts: max on-time with Kl15 off and bus asleep (battery protection), watchdog on a hung Pi, low-battery cutoff (read 0x571 `BS2_U_BATT`, `BS2_aus_Infotainment`, 0x658 shutdown stage, plus its own ADC).
- Ride through cranking (Kl50 in 0x2C3/0x575): hold-up for the Pi or a clean brownout reset.
- Keep its own quiescent current within VAG's budget (question: what does Drew want, e.g. < 1 mA).

---

## 4. Light model

DBC sources:
- 0x635 mDimmung (200 ms): `DI1_Display` bits 0-6 % (58d, display backlight), bit 7 `_def`; `DI1_Schalter` bits 8-14 % (58s, switch illumination, follows the dimmer wheel and goes non-zero with lights on), bit 15 `_def`; `DI1_Sensor` bits 16-23 (254 INIT, 255 error). Log: `25 00 23` = display 37 %, switch 0, sensor 35.
- 0x621 mKombi_K1 (100 ms): `KO1_Bel_Displ` bits 40-46 % (127 = no substitute), `KO1_Sta_Displ` bit 47, `KO1_Lichtsensor` bits 48-55 (254/255), `KO1_Fernlicht` bit 37. Log: 37 % and 35 — same numbers as 0x635.
- 0x470 mBSG_Kombi (50 ms): `BSK_Display` bits 16-22 %, `BSK_Klemme_58t` bits 24-30 % (Log 0x3A = 58 %), `_def` bits; `BSK_Standlicht` 40, `BSK_Parklicht_*` 41/42, `BSK_Abblendlicht` 43, `BSK_Nebellicht` 44, `BSK_Nebelschlusslicht` 61, `BSK_Fernlicht` 62, `BSK_Tagfahrlicht` 63; indicators bits 0-3; reverse bit 5.
- 0x531 mLicht_1_alt (50 ms, counter + XOR): side 0, dipped 1, main 2, fog 3, rear fog 4, reverse 5, parking L/R 6/7, indicator lamps 8/9, hazard 11, brake 14, DRL 15, indicator telltales 16/17, cornering 18.
- 0x2C1 mLSM_1: stalk inputs (main beam switch 3, flasher 2, parking 5/6), not lamp state.
- 0x2C3/0x575 bit 4: terminal P (parking light position).

Old code:
- `can_base_function.py:430-447`: night = 0x635 byte 1 > 0 (`DI1_Schalter` including its `_def` bit — an error frame reads as night).
- `hudiy_client/dark_mode_api.py:245-254`: same byte, but bit 7 set or raw 127 = invalid. Night follows at once, day only after 4 s of "off" with ignition on, counted from ignition-on (`:257-300`), because 58 drops with ignition off.
- `vehicle_data/ican_definitions.py:249-303` exposes 0x621/0x470/0x635 brightness/sensor values; nothing uses them for control.
- So "night" today = "dimmer/switch illumination non-zero" ≈ side lights or headlights on. Ambient-light based night (sensor) is not used. Whether `DI1_Schalter` is non-zero with DRL/auto-lights is unknown.

RNS-E panel brightness (r21, via the Pi): CAN 0x7B0, `B7 <0-10>` = brightness dial level, `BB <0-100>` = LCD override (0 restores native: cluster dimming + dial; effective minimum 6), `BA <0-2>` = source label/icon (0 Hudiy, 1 CarPlay, 2 Android Auto) (`rns-e_can/rnse_control.py:26-53,300`). Replies/ADC on 0x7B6. The module docstring says "r38 firmware protocol" (`rnse_control.py:3`) while docs say r21 — naming question. Config: auto day/night dial 10/1 and LCD 100/6, both disabled; manual 10 / LCD 0 (`config.json:6-22`). Which CAN signal the stock RNS-E uses for its own dimming (0x635 DI1_Display? 0x470 58t?) is not in the repo.

---

## 5. Signals worth feeding Android Auto sensors

| AA sensor | Source (ID, signal, scale) | Rate | Validity notes |
|---|---|---|---|
| Speed | 0x359 `GWB_FzgGeschw` 9\|15 ×0.01 km/h | 100 ms | invalid 32708/32725/32742; dated `GWB_Alt_FzgGeschw` bit 0; `GWB_Geschw_Ersatz` bit 55 = substitute. Alt: 0x351 `GW1_FzgGeschw`, 0x527 `GWK_FzgGeschw`, 0x523 `KB1_angez_kmh` (cluster-indicated, ×0.32, 1020 err) |
| RPM | 0x35B `GWM_Motordrehzahl` 8\|16 ×0.25 rpm | 100 ms | 65280 error; dated `GWM_Alt_1_Motor` |
| Odometer | 0x65D `DN1_KM_Stand` 8\|20 km | 1000 ms | dated `DN1_alt_Kilometerstand` bit 62. Log decodes to 238,619 km — confirm against the cluster |
| Fuel level | 0x621 `KO1_Tankinhalt` 24\|7 L, `KO1_Tankwarnung` bit 31 (low fuel) | 100 ms | 127 error; cluster estimate, last value held with ignition off. No range signal seen (MFA 0x629.. not in log) |
| Parking brake | 0x621 `KO1_Handbremse` bit 5 | 100 ms | log shows 1 (applied) |
| Gear | reverse from 0x351 `GW1_Rueckfahrlicht` bit 1 / 0x470 bit 5 / 0x531 bit 5; selector 0x359 `GWB_Info_Waehlhebel` 60\|4 only if S tronic | 50-100 ms | log reads selector = 8 with the car parked — check gearbox type |
| Night mode | 0x635 `DI1_Schalter` > 0 (with `_def` = 0), or sensor 0x635 `DI1_Sensor` / 0x621 `KO1_Lichtsensor` with hysteresis | 200 ms | §4 |
| Lights | headlight: 0x531 bits 0/1/2 (or 0x470 40/43/62); indicators: 0x531 16/17 or 0x470 0/1; hazard: 0x531 11 / 0x470 3 | 50 ms | 0x531 has counter + XOR |
| Doors | 0x470 `BSK_FT/BT/HL/HR_geoeffnet` bits 8-11, hood 12, boot 13 (`HD_Hauptraste`) | 50 ms | old code: `can_base_function.py:450-490` |
| Environment | outside temp 0x527 `GWK_AussenTemp_gefiltert` 40\|8 ×0.5 −50 °C | 200 ms | 255 error. Alt 0x3E1 `CL1_AussenTemp` (100 ms) |
| Driving status | derived from speed | — | AA restricts UI when moving |
| Dead reckoning: steering | 0x3C3 `LW1_Lenkradwinkel` 0\|15 ×0.04375° + sign bit 15 | 100 ms | valid only `LW1_Int_Status` = 0, `LW1_ID` = 128; counter bits 44-47, CRC8 byte 6 |
| Dead reckoning: wheel ticks | 0x5B5 `B10_Wegimp_VL/VR/HL/HR` 10-bit rolling counters, direction bits 60-63 | 200 ms | 1021-1023 invalid; `QB_*` qualifiers bits 12-15, 56-59; counter + XOR. Pulses/rev from 0x359 `GWB_Impulszahl` 40\|6, tyre circumference 0x527 28\|12 (valid when bit 4) |
| Yaw / gyro | 0x2A1 `NV1_Gierrate` (mNavig_1, sent by the navigation unit) | 250 ms | not seen in the log — does the 193 send it? |
| HVAC (optional) | 0x3E3 `CL2_Solltemperatur`, `CL2_InnenTemp` | 800 ms | 254/255 |
| Battery (diag) | 0x571 `BS2_U_BATT` ×0.05+5 V | 500 ms | 255 error |
| GPS | none on ICAN from the RNS-E observed (0x5A3 mNAVPOS not in log) | — | phone GPS |

---

## 6. Short guesswork list (old code)

- 0x2C3 bit 0 called "key in lock sensor" — DBC: terminal S. Close enough, but 0x575 is the BCM's own terminal state and is ignored.
- Bus-sleep readiness when no NM member is visible counts as "ready" (`can_base_function.py:175-177`) — a broken RX path looks like a sleeping bus.
- 5 s freshness for NM and radio_active is arbitrary.
- 0x635 night threshold and invalid handling differ between services.
- The 900 s fallback and 10 s listen-only delay are tuning guesses (`FixList.md:2-7` asks "what is ideal?").
- `active_low` for GPIO 12 means the opposite of its name.
- `vehicle_data` stale-after = max(1 s, 3× cycle) — reasonable, untested.
