# DIS research for the Rust rewrite

Scope: read-only survey of `RNS-E-Hudiy/dis_client/`, `dis_emulator/`, `references/dis display references/`, DIS parts of `references/ICAN_rundown.md` and `references/PQ35_46_ICAN.dbc`, `clusterRE/*.txt`, plus a read-only grep of `RNSE hacking/CAN_PROTOCOL.md` and `R41_FEATURES.md` for the firmware ID remap (the repo itself has no record of it).

Paths below are relative to `RNS-E-Hudiy/` unless they start with `clusterRE/` or `RNSE hacking/`.

## 0. Things that change the picture

1. **All bench evidence comes from an A3 cluster, not the TT.** `clusterRE/dis_findings.txt:1-4` lists the target as **8P0920982G** (A3, VDO H15) on a 500 kbit/s SM2 bench bus with wake frame 0x575 FF×8. `dis_client/BENCH_RESULTS.txt:169-170` says the bench is 500 kbit/s and the car is 100 kbit/s. `clusterRE/` also holds an `8J0920990F__0370.sgo`, which is the TT cluster's firmware file, but no TT bench results exist. The only car-confirmed piece is the handshake fix (`NATIVE_HANDSHAKE_CONTROLLER.txt:55-57`); the latest controller has never run in the car (`BENCH_RESULTS.txt:228-231`).
2. **The repo does not handle the firmware's DIS redirect at all.** In `RNSE hacking/CAN_PROTOCOL.md:13-21` the firmware moves the RNS-E's own DIS traffic:
   - DDP transport 0x6C0 → **0x7B2** (RNS-E → Pi) and 0x6C1 → **0x7B3** (Pi → RNS-E).
   - Radio text 0x363 → **0x7B4** and 0x365 → **0x7B5**, sent every 800 ms.
   - Frame contents are stock. "The Pi forwards them to and from the real IDs."

   `R41_FEATURES.md:13-14,50-53` adds two conditions. Transport is remapped only when PCBBC "FIS-Control" is ON. Radio text is remapped only while TV is the source; in every other source the cluster gets the stock IDs. Version history (`CAN_PROTOCOL.md:364-366`): text redirect arrived in r29, IDs were renumbered in r31. A grep of the repo for 0x7B2–0x7B5 finds no forwarder. Today the Pi's own `dis_top_display_service.py` writes 0x363/0x365 and overwrites OEM writes. The firmware Drew calls "r21" in user docs is probably an internal r3x/r4x build (`CAN_PROTOCOL.md` lists r21 as the BC/BD + 0x461-keys release). This needs confirming (see questions).
3. **0x604 is not a DIS message.** `mNAV_1` (DBC `PQ35_46_ICAN.dbc:1334-1345`; `ICAN_rundown.md:816-842`) is NAV_On / mute / warning-tone / error bits, `NV1_NAV_Mode`, and six mode parameters. The DBC comment says the mode and parameter codes are in a "separate specification". Routing is "local (stays on infotainment)" and the cycle is 1000 ms (`dbc:8157`). It tells you whether the RNS-E's own nav is on. It carries no maneuver graphics. The old code never reads it. `config.json:276-277` mislabels 0x6C0/0x6C1 as `nav_status`/`media_status`.
4. **The white cluster accepts only the central window.** On the A3 bench, claims for centre+lower (0,27,64,61), full (0,0,64,88), top-only and top+centre all returned **53 C0**, which is not a grant. Only the central (0,27,64,48) claim worked (`clusterRE/ddp_top_region_bench_results.txt:15-25`). A full clip inside central ownership still drew inside the centre (`:44-53`). The top two lines are therefore reachable only through 0x363/0x365 (or the redirected text). The old `set_region` full/centre_lower/top_centre options (`dis_service.py:1512-1535`) do not work on a white cluster.

## 1. Transport and protocol facts

### Channel and IDs
- The nav ↔ cluster channel is a fixed TP2.0-style channel. DBC: `mTp_NSL_KOM` 0x6C0 (1728) from the head unit, `mTp_KOM_NSL` 0x6C1 (1729) from the cluster, both `VAGTP20_StatConnection=1` (`dbc:2001-2008, 9516-9555`). Rundown: `ICAN_rundown.md:1195-1215`.
- The DBC also defines a telephone channel: 0x6C2 `mTp_TM_KOM` → cluster and 0x6C3 `mTp_KOM_TM` ← cluster (`dbc:2004`, `ICAN_rundown.md:1219-1225`). The repo contradicts itself here:
  - `ddp_protocol.py:184-187` says "Telephone (0x6C5/0x6C4)".
  - `dis_tests/test_tel_ddp_final.py:24-29` uses 0x6C4/0x6C5 plus a 0x665 heartbeat.
  - `dis_tests/ddp_bridge.py:1-11,280-330` treats 0x6C2/0x6C3 as the RNS-E's remapped nav channel, apparently from an early firmware.

  Nothing shows a working phone channel to the cluster.
- Defaults: TX 0x6C0, RX 0x6C1, socketcan, 100 kbit/s (`ddp_protocol.py:180-188`).

### Framing (generic TP2 data, no length prefix)
- The first byte holds the opcode in the high nibble and the sequence in the low nibble:
  - `2n`: body frame, no ACK.
  - `0n`: continuation that requests an ACK (block boundary).
  - `1n`: end of message, requests an ACK.
  - `Bn`: ACK, where n is the next expected sequence.
  - `9n`: receiver not ready.
  - `Dn`, `Fn`: treated like 9n/Bn by the stock masks.
  - `A0/A1/A3/A8`: channel control.

  References: `flasher/vag_protocols/tp2.py:42-95`, `ddp_protocol.py:134-139, 753-816`, `references/dis display references/pcbbc.txt` "FRAME STRUCTURE".
- **The DDP message boundary is the 1n frame only.** There is no length prefix (`segment_message(..., length_prefixed=False)`, `ddp_protocol.py:1218-1220`).
- **The ACK block and the application message are separate limits.** The old code originally ended messages at the 42-byte ACK block. That corrupted commands; the fix is to use 0n continuation frames (`clusterRE/dis_findings.txt` "Confirmed framing root cause", ~l.150-170; `ddp_protocol.py:1201-1247`).
- The block-size cap is `min(15, max_unacked_frames=6, max_message_bytes//7=6, peer BS)`. Peer BS comes from byte 1 of A0/A1, 0x0F=15 (`ddp_transport.py` `PeerTransportParameters`; `ddp_protocol.py:1213-1217`). The bench ran block 15 without problems (`dis_findings.txt` cow campaign).

### Timing constants (old defaults, `ddp_protocol.py:50-71`)
| Constant | Value |
|---|---|
| Inter-frame gap | 2 ms. The bench used 0 ms and was fine at 500 k. |
| White post-message delay | 20 ms. The bench removed it: "~5.3/s, user confirmed cow animating correctly". |
| Receiver-not-ready (9n) wait | 100 ms. After it, replay the suffix from n. Max 5 not-ready retries. |
| Missing ACK | resend only the final ACK-request frame, max 2 times |
| Per-block ACK wait | 1000 ms (`ddp_protocol.py:1060`) |

- Any session or display-state change during a block makes the driver close the session. It does not guess what the peer received (`ddp_protocol.py:1040-1050`). The exception is a block that was already completely ACKed when 2E arrived (`:1011-1020`, commit c6ef5ce).
- **Keepalive:** if we opened the channel, send A3 every 2 s, even while PAUSED (`ddp_protocol.py:1746-1761`). The cluster replies A1 0F 8A FF 4A FF. If the cluster sends A3, reply A1 (`:774-778`).
- The A0/A1 parameter bytes are BS 0x0F, T1 0x8A, T3 0x4A. The bench decodes them as pacing ticks (`ddp_transport.py` `PeerTransportParameters`). The diagnostic TP2 code uses `A0 0F 8A FF 32 FF` instead (`tp2.py:18-19`).

### Session open
- White, active: TX `A0 0F 8A FF 4A FF`, RX `A1 0F 8A FF 4A FF` (`ddp_protocol.py:1266-1278`).
- White, passive: the cluster sends A0 first, we answer A1. Drain duplicate A0s first (`:1249-1264, 1318-1352`).
- On service start, send A8 once to clear zombie sessions, then wait 1 s (`dis_service.py:1442-1448`).
- If an A3 arrives from an old session, send A8 and reopen (`ddp_protocol.py:1390-1402`).
- Red cluster (legacy, untested on red hardware): it broadcasts `A0 07 00`. We reply `A1 0F` + A3, then exchange A3/A1 four times (`:1280-1316`).
- Setting DISCONNECTED sleeps 1.2 s inside `_set_state` (`ddp_protocol.py:278-279`). That is a hack.

### Application handshake (bench and car confirmed)
Sequence from `NATIVE_HANDSHAKE_CONTROLLER.txt:61-71` and `BENCH_RESULTS.txt:177-180`:

```
TX 15 01 01 02 00 00      session announcement
RX 00 01                  mode request (mode 1; mode 2 is unsupported -> reply then close)
TX 01 01 00               reply
TX 08                     capability query
RX 09 20 0B 50 08 0B 50 30 39 00 32 00     one 12-byte record on the A3 bench
TX 20 3B A0 00            setup (priority A0; 10 = "full")
RX 21 3B A0 00            or the native FFFF sentinel
TX 33                     release: initial state has no display intent
TX A3 / RX A1             liveness
-> READY
```

- **Capability format byte (`ddp_application.py:16-38`):**
  - 0x20 → command family **0x52** (mono, the white TT/A3 case).
  - 0x10 + company 03 → **0x7A** ("hicolor"; restricted support for a colour cluster).
  - Other 0x10 → 0x5A (no renderer).
  - The tail is opaque.
- The cluster may repeat `00 01` during discovery. Each one needs its own 01 reply (`NATIVE_HANDSHAKE_CONTROLLER.txt:114-130`). The previous controller reconnected unnecessarily on this.
- The cluster may also send `00 xx` / `08` / `14` / unknown even opcodes at runtime. The driver replies; for example, `14` gets `15 01 01 02 00 00` and invalidates setup (`ddp_protocol.py:515-571`).
- `0B rr cc [sc]` is an application **error**: rr=1 unsupported, 2 incompatible context, 3 invalid parameters (`ddp_application.py:90-110`). Recovery for reason 2 is `01 01 00`, then `08`, then `20` again (`ddp_protocol.py:1932-1971`). Recovery has a budget of 3 episodes per session.
  - `0B 03 57` was historically labelled "graphic ACK white". That label is wrong; it is an error (`ddp_protocol.py:104-107`, `clusterRE/graphics_application_status_findings.txt`).
  - `0B 03 55` showed up 14–513 ms *after* the transfer was ACKed, for empty 55 records. Errors are late and not correlated to a message.

### Who owns the screen: window status and arbitration
- **Claim:** `52 05 82 00 1B 40 30` (claim + clear, central 0,27,64,48) (`dis_service.py:280-361`). Transport ACK is *not* ownership. Wait up to 1 s for a `53 xx` status.
- **53 xx bitmask** (stock 805549D2, `ddp_application.py:71-87`), checked in this order:
  - bit 0x40 set → fault
  - value & 3 == 0 → unavailable
  - bit 0x80 set → **granted**
  - otherwise → available

  | Value | Meaning |
  |---|---|
  | 53 85 | granted (central) |
  | 53 8A | granted (full, red) |
  | 53 84 / 53 04 / 53 88 / 53 08 | busy / unavailable, for example a cluster warning or menu |
  | 53 05 / 53 0A | free |
  | 53 C0 | fault; seen for unsupported region claims |

  Families 5B/7B carry the same mask. `7B 05` is a special release/reclaim.
- **Observed sequence:** first claim gets `53 84`, then `53 05`, then `2E` (reinit request), we send `2F`, then we claim again and get `53 85` (`BENCH_RESULTS.txt:181-185`, `clusterRE/dis_findings.txt:33-45`). The cluster blocks the centre for about 30 s after wake with startup warnings (`bench_cluster/README.md` step 4).
- **The cluster arbitrates.** Its own warnings, menus and trip computer win, signalled as 53 busy. When it gives the window back, it sends 53 05 then 2E. The old code also treats a 2E/2F that arrives while we are not presenting as **"cluster selected our page"**. The driver chose the nav page in the cluster menu, so we resume (`dis_service.py:175-215`, `dis_display.py:1009-1021`).

  Nothing shows the cluster ranking nav vs phone vs radio itself. There is one DDP owner per channel. The radio lines (0x363/365) are a separate path, and the DBC says the radio must stop sending them when the "display is busy" or the phone-active bit is set (`dbc` CM_ 867/869, l.4993-5018).
- Release: `33` (`ddp_protocol.py:1421-1441`). Hard close: `A8`.
- The old service never releases on inactivity (`ENABLE_INACTIVITY_RELEASE=False`, `dis_service.py:136-144`).

### Measured throughput (A3 bench, 500 kbit/s; none of these are camera-verified LCD frame rates)
- One ACK block (≤15 frames) round-trips in about 9 ms. Hardware timestamps show a ~10 ms ACK cadence (`clusterRE/native_transport_performance_findings.txt`).
- 9n backpressure appears at high rates, about 1.5 per full native frame. Each one costs 100 ms plus a replay, and that dominates native frame time (0.49–0.62 s per frame).
- Logical (2×2-pixel) full-centre bitmap, row messages ≤104 B, ACK block 15, gap 0: **about 4 updates/s visually confirmed**. With packed window+rows+commit and the 20 ms delay removed: about 5.3–5.7/s. Native bulk writes: 8.3–10/s ACKed. At a 10 fps target with 424-byte frames (5 messages, 62 CAN frames), the cow was visually correct.
- Uncapped transfers (~16.7/s) teared and showed only "a few actual updates/s".
- Production native nav bitmap: 1366 B in 31 messages, transport ACK 0.258 s (`native_bitmap_profile.txt:13-16`).
- Application message size limits:
  - 49 B is the stock RNS-E budget.
  - 105 B is the verified white policy.
  - 156–195 B worked.
  - **208 B froze the display with transport ACKs still coming back**, and recovered without a reset.
  - A 22-row 55 command (LEN 0xB3) froze; LEN ≥ 0x80 is suspected (`dis_findings.txt` "2026-10-03 checkpoint" and "raster and line findings").
- Coalescing many bitmap windows into one ≤768-byte message froze the display. It is opt-in only (`dis_service.py:748-782`).
- **`39` is not an atomic present.** A no-commit probe still updated the screen, and the cluster shows intermediate passes (`native_bitmap_profile.txt:18-22`, `dis_findings.txt` "receiver flow" section).
- **Car at 100 kbit/s:** each 8-byte frame is about 1.1–1.3 ms on the wire versus about 0.25 ms at 500 k. A 424-byte frame (62 CAN frames) needs about 70–80 ms of raw bus time before ACK latency, and it shares the infotainment bus. Expect roughly 2–5 full-centre updates/s at best. This is an estimate; it has not been measured.

### Native code vs emulated behaviour
- `NATIVE_HANDSHAKE_CONTROLLER.txt` reconstructs the RNS-E 193 SH4 firmware's own controller from function addresses (8055A4EE etc.).
- Deliberate differences from native: retiring unsent old-mode setups, close/reopen when the mode changes after the first 20, and closing on initial mode 2 (`:118-126, 201-205`).
- The 32-entry queue and 10 s deadlines are host policy.
- The old 50-test transport suite fails 16 tests plus 1 error, before and after these changes (`:144-147`).

## 2. Display model (white cluster)
- **Coordinates:** the DIS uses red-era *logical* units, 64 wide × 88 tall. The centre window is (0,27,64,48) (`pcbbc.txt` REGION COMMAND).
  - The white cluster draws each logical unit as **2×2 physical pixels**, so the centre is **128×96 physical** (`native_bitmap.py:10-13`, `README.md:33-37`).
  - **52** region coordinates are absolute. **57** text and **63** lines are relative to the current clip (`dis_service.py:572-577`).
- **Primitives:**

  | Opcode | Form | Notes |
  |---|---|---|
  | **52 05 FL X Y W H** | region | FL 0x80 claim, 0x02 clear, 0x01 colour. Flag 0x03 (clear+colour) fills, used for inverted text backgrounds (`dis_service.py:519-528`). |
  | **57 LEN FL X Y chars…** | text | FL 0x20 centre; 0x0C font: 00 fixed, 04 proportional, 08 graphics; 0x03 colour mode: 00 black/transparent, 01 XOR, 10 normal opaque, 11 transparent. Code values: `FLAG_ITEM=0x06`, `0x26` centred, `0x22` fixed centred (`apps/base.py:8-12`). LEN = chars+3. Stock budget 49 B per record; at most 44 glyphs (`native_nav_glyphs.py:30-45`). |
  | **55 LEN MODE X Y raster…** | bitmap | MODE 00 erase, 01 XOR, 02 draw, 03 set. X must be 0 (draw at column 0 of the clip). 1 bpp, MSB first, byte-aligned rows. Multi-row wrapping inside one 55 is verified up to 8–12 rows; 22 rows froze (`dis_service.py:690-705`, `clusterRE/bitmap_flag_probe_findings.txt`). Upper mode bits: unknown. |
  | **63 04 OR X Y LEN** | line | OR 0x10 vertical, 0x20 horizontal. Physically V1 = 1 px × 2 px, H1 = 2 × 1, and lengths double. LEN 0 is a no-op. Max run used: 15. **Lines inherit SET or XOR from the preceding 55 mode.** A zero 8-byte `55 mode1` selector before every 63 stroke lets you address individual physical pixels; that is the whole "native 128×96" hack (`dis_findings.txt` "05:22", "05:35"; `native_bitmap.py`, `native_tiles.py`). A full native frame is about 2 KB, several times the logical bitmap. |
  | **39** | commit | Required, sent as its own message after the data (`dis_service.py:707-731`). Not atomic. |
  | **33 / 2E / 2F** | release / reinit request / reinit confirm | |
  | **69 / 7A / 83 / 5F / D1** | hicolor | 7A cluster family only (colour cluster). Not relevant to the TT. |

- **Fonts:** the cluster has three built-in fonts (fixed, proportional, graphics). The native cell is 12×14 physical pixels with measured advances (`references/dis display references/NativeWhiteFontCharts.json` layout and counts; `font_metrics.py`, `font_metrics_data.json`; images `NativeWhite*Font*.png`).
  - Text capacity is budgeted per 105-byte message (`font_metrics.py:57-69`).
  - The old code clears a text field before redrawing it, so a narrower string doesn't leave ghosts (`README.md:42-45`). Clearing with spaces (`0x65` × 11) is what the stock RNS-E does. `0xD7` clear-to-end-of-line is unverified on the 8J (`pcbbc.txt` NOTES).
- **Native nav glyphs:** the stock RNS-E draws its arrows and approach bar with the **graphics font through 57 records**. It does not use bitmaps (`pcbbc.txt` NOTES; `native_nav_glyphs.py:1-8`).
  - `stock_mono_catalog.json` (hash-pinned) holds the stock screen 4/5 records extracted from ROM: bar levels as 21 steps of `57 04 0A 39 xx yy` cells, and icons keyed by "source object" bytes (`stock_mono_frame.py`).
  - `native_nav_catalog.json` holds 139 mono and 129 hicolor maneuver placements.
  - These are reproducible, cheap (tens of bytes), and native-looking. They are the best nav-arrow path on the TT.
- **AUDSCII:** non-standard. a–p map to 0x01–0x10, q–z to 0x71–0x7A, and there is a CP1252 lookup table (`pcbbc.txt` table; `icons.py:11-69` `encode_audscii`; `dis_top_display_service.py:151-166`). 0x1F is the continuous-scroll gap. `WHEEL_CONTROL_GLYPH` is AUDSCII 0x15 (`icons.py:55`).
- **Top two lines:** `mRadio_1_neu` 0x363 holds characters 1–8 and `mRadio_2_neu` 0x365 holds characters 9–16. Each is a single 8-byte frame, one AUDSCII byte per character (`dbc:1564-1582`, `ICAN_rundown.md:192-226`).
  - DBC rules: send after KL15; on change, send immediately but no faster than every 50 ms; stop when the display is busy, `RA2_Display_disabled` is set, or the phone-active bit is set (`dbc:4993-5018`).
  - The old code writes 8 fixed cells and scrolls them itself (`dis_top_display_service.py:174-300`). It re-sends on a 100 ms heartbeat and a 1 s forced resend (`:56-70`).
- **Refresh and flicker:** the cluster shows partial passes, so a large repaint flickers or tears. Mitigations in the old code:
  - Use field-level updates, not full clears (`dis_display.py:1196-1235`).
  - Deliver nav icons as "deferred planes", one refinement plane per frame (`dis_display.py:1293-1300`, commit a2b6608).
  - Hold the last maneuver through route gaps instead of flashing "No Route" (`dis_display.py:584-605, 1241-1242`).
  - Tiles keep their base and corrections in one message (`native_bitmap_profile.txt` "tiles").

## 3. What the stock radio sends (with redirect) and what the old code does with it
- **Radio text:** with the redirect active, characters 1–8 arrive on 0x7B4 and 9–16 on 0x7B5, every 800 ms, **only while TV is the source**. In other sources the radio talks to the cluster directly on 0x363/0x365 (`R41_FEATURES.md:53`). The firmware notes expect the Pi to forward 7B4/7B5 → 363/365 and stop after a ~1.2 s gap.
  - **The old code captures none of this.** `dis_display.py:87-90` has the 0x363/0x365 subscription commented out.
  - The top service only *overwrites* OEM writes. It watches 0x363/0x365, freezes its own scroll, and fires 3 immediate copies of its snapshot for each OEM frame (`dis_top_display_service.py:583-610`).
  - It transmits only while 0x661 byte 3 == TV id 0x37 and the RNS-E is awake (`:497-503, 568-590`; commit 1cff7e9).
- **Graphics session:** with FIS-Control on, the RNS-E's DDP session is on 0x7B2/0x7B3. The only code that touched an RNS-E graphics session is `dis_tests/ddp_bridge.py`. It is a dumb bidirectional bridge with a decoder/logger, using 0x6C2/0x6C3 as the remapped IDs (`:1-11, 262-339`). Nothing in production captures, parses or replays the stock nav screen.
  - What exists offline: `stock_mono_frame.py` and `stock_nav_composition.py` compile ROM-derived stock nav screens from raw firmware selectors. A stock nav screen could be *re-synthesised* from the RNS-E's semantic state, but nothing currently gets that state off the RNS-E.
- **0x604 mNAV_1:** see §0.3. Possibly useful as "RNS-E nav guidance active". Mode codes are unknown.
- **0x661 mRadio_4:** `RA4_Show_Radio` is the display request bit; `RA4_Radio_Mode`/params give the source. The old code uses byte 3 == 0x37 for TV (`config.json` input_mappings.source).

## 4. Old app and page model
- **Process split:**
  - `dis_service.py` (1837 lines) owns DDP. It takes JSON draw commands over ZMQ PULL `dis_draw.ipc` and publishes `DIS_STATE`, `DRAW_ACK/NACK seq` and `DIS_REQUESTED gen` on `dis_status.ipc`.
  - `dis_display.py` (1407 lines) runs the apps and page policy and sends JSON frames.
  - `dis_top_display_service.py` (1302 lines) is a separate process for the top lines, with its own priority logic.
  - Three processes therefore decide what is shown, and they coordinate through status pub/sub (`dis_top_display_service.py:965-1031` reads the centre app to avoid duplicating content).
- **Apps** (`dis_display.py:41-49`): nav, media, phone, car_info (Readings: 8 configurable values in high-res mode, `apps/readings.py`), acceleration_test, coverart, easteregg (GIFs, regex match on track), openpilot. The menu, radio and settings apps were removed in bd1797f.
  - `applist` config defines the stalk cycle (`:52-56`). Nav is skipped without a route, phone without a call (`:336-407`).
- **Views:** an app's `get_view()` returns either a line dict (`line1..5` at y = 1, 11, 21, 31, 41) or a list of draw commands with `group`, `type` and `deferred` (`dis_display.py:24-25, 1237-1360`). The engine diffs per group by `str(items)` and sends one frame, then waits for ACK before the next. If no feedback arrives within 30 s it rebuilds (`:1162-1194`).
- **Priority resolver** (`dis_display.py:612-648`): phone overlay > nav auto-switch > brief cover art (5 s on a new track) > easter egg > user selection.
- **Nav policy** (`navigation_policy.py`): interrupt when the maneuver changes (5 s peek) or the route is within 500 m (approach), re-arm beyond 1000 m, suppress after a manual stalk press. There is a 3 s release grace for CarPlay route gaps (`dis_display.py:584`).
- **Claim policy:** `start_inactive`, `claim_on_nav`, `claim_on_phone` (context-only claims) and user pause/resume, all in `_check_nav_availability_pause` (`dis_display.py:489-582`). This is the most tangled logic in the module.
- **Top lines priority:** phone > nav > media > "No Media", ordered by applist. A source is skipped when the centre already shows it (`dis_top_display_service.py:965-1031`).

**Sound ideas worth keeping (as concepts, not code):**
- Frame sequence with ACK/NACK backpressure and dropping instead of queueing.
- Transport kept separate from application status.
- Per-field text updates with measured font metrics.
- ROM-derived stock glyph catalogs.
- Nav peek/approach hysteresis.
- Holding through route gaps.
- Invalidating the retained-pixel model on any ownership or session generation change.

**Patched areas, bugs and workarounds visible in code and logs:**
1. Most driver state is created lazily with `getattr(self, x, default)` instead of in `__init__` (all through `ddp_protocol.py`). There are several overlapping "generation" counters: state, application session, request, capability, error, presentation request.
2. `_set_state(DISCONNECTED)` sleeps 1.2 s and shuts down the bus inside a setter (`ddp_protocol.py:262-279`).
3. ACK-block vs message framing bug (fixed): messages were cut at 42 bytes (`dis_findings.txt`).
4. Batch flush indentation bug sent draws twice (fixed, `dis_findings.txt` "Offline batching correction").
5. `mode_flag` was not forwarded to `draw_bitmap` (fixed, `bitmap_flag_probe_findings.txt`).
6. `0B 03 57` was mislabelled as a success ACK (`ddp_protocol.py:104-107`).
7. Bitmap coalescing froze the display and is now opt-in (`dis_service.py:748`). Messages of 208 B or more and 22-row 55 commands froze the cluster while transport ACKs continued.
8. `set_region` full/top_centre/centre_lower are dead on white (`dis_service.py:1512-1535`). top_centre claims 88 and clips 75 (`ddp_top_region_review.txt:20-27`).
9. The top-line service fights OEM writes (100 ms heartbeat, 3 reactive copies). With the firmware redirect this is obsolete. Commit 1cff7e9 stopped writes while the RNS-E sleeps, because writing kept the bus awake.
10. The repeated-`00` handshake case caused unneeded reconnects. It was fixed, but not tested in the car (`BENCH_RESULTS.txt:192-199`).
11. 2E arriving during an ACKed claim dropped the session (fixed in c6ef5ce, `ddp_protocol.py:1011-1020`).
12. The DIS draw REQ socket to `vehicle_data` is recreated on timeout (`dis_display.py` ~l.920-935).
13. The stale DIS_STATE fallback in the top service assumes "not ready" after 5 s (`dis_top_display_service.py:967-971`).
14. The 7A hicolor path (69 symbols, frame-15 records) and the red-cluster path are big, untested on the TT, and not needed for the car.
15. 16 old transport tests fail plus 1 error; the native suite passes (`NATIVE_HANDSHAKE_CONTROLLER.txt:144-149`).
16. README to-dos: red DIS bitmap broken, top radio lines, more nav bitmaps (`dis_client/README.md:1-8`). FixList: "DIS service should report commit status… drop updates instead of backlog" (`FixList.md:16-19`).
17. Native 128×96 needs a selector before every stroke. It is expensive and slow at 100 k; the logical 64×48 path is the practical default.
18. 237 commits touch `dis_client/` (`git log -- dis_client`), many named "fix", "maybe fix claim?", "n+1 failed fixes".

## 5. Recommended abstractions for the Rust daemon

### Layer 1: `can` I/O
- A trait `CanPort { send(id, &[u8]); recv() -> Frame + hw timestamp }`.
- Implementations: socketcan, a mock bus for the emulator, and replay from `wire.jsonl` captures.
- One writer per ID. A global transmit inhibit for flashing, replacing `flasher.traffic.transmission_guard`.

### Layer 2: `tp2_fixed`, a sans-IO state machine per channel
```rust
struct Tp2Channel { role: Opener|Acceptor, tx_seq, rx_seq, block_cap, params }
fn on_frame(&mut self, f: &[u8], now) -> Vec<Out>   // Out = Tx(frame) | Delivered(msg) | Acked(msg_id) | Closed(reason)
fn submit(&mut self, msg: Bytes) -> MsgId            // one logical message, segmented 2n/0n/1n
fn poll(&mut self, now) -> Vec<Out>                  // keepalive A3 every 2 s, 100 ms 9n wait, ACK timeout 1 s
```
- Handles: ACK every 0n/1n received, 9n wait + suffix replay, missing-ACK resend of the last frame only, A3/A1, A8, bounded retries, sequence wrap.
- No sleeps inside; the reactor drives time. This makes timing testable with a fake clock.
- Instantiate one channel for the cluster (0x6C0/0x6C1). If the RNS-E session is terminated at the Pi, instantiate another for the RNS-E side (0x7B2/0x7B3) in the **acceptor/cluster role**.

### Layer 3: `ddp_session` (application control)
- Runs the handshake as an explicit enum state machine: announce → mode → caps → setup → release → ready.
- Answers 00/08/14 and unknown even requests.
- Parses 53/5B/7B into `WindowStatus{Granted, Available, Unavailable, Fault}` and handles 2E/2F, 0B errors and recovery budgets.
- Emits events: `Ready{caps}`, `Window(status)`, `ClusterRequestedContent`, `AppError{raw}`, `Lost`.
- API: `claim(Region::Central)`, `release()`, `submit_frame(seq, Vec<Message>) -> FrameResult`.
- Keep `generation: u64` as the **one** counter. It bumps on every ownership or session change and invalidates retained pixels and in-flight frames.
- Capability profile: only `Mono52 { center: 64x48 logical, 128x96 physical, msg_budget: 105 }`. The 7A/red variants stay unimplemented behind the trait.

### Layer 4: renderer (cluster-agnostic scene → DDP messages)
- **Scene model:** a retained tree of surfaces.
  - `Surface::Center` (128×96 physical, logical grid 64×48).
  - `Surface::TopLines([Cell8; 2])`.
- **Center scene:** a list of `Element`s with stable ids:
  - `Text{rect, font, align, invert, text}`
  - `Glyphs{x,y, font: Graphics, bytes}` (native nav and bar)
  - `Bitmap1bpp{rect logical}`
  - `NativeBitmap{rect physical}` (opt-in)
  - `Line`, `Fill`
- **Compiler:**
  - diffs the new scene against the *believed* cluster state, which is dropped on any generation change;
  - emits minimal 52 clip / 57 / 55 / 63 records;
  - packs ≤105 B messages (never a 55 LEN ≥ 0x80) with a separate 39;
  - computes the text-clear rect from measured font advances.
- Output is pure data (`Vec<Message>`), so it can be golden-tested without hardware.
- **Pacing:** at most one frame in flight. The newest scene replaces any pending one: latest wins, no backlog. Rate cap configurable, default ~4/s at 100 k. Large changes are split into progressive passes.

### Layer 5: sources and the arbiter
- Each source publishes a `Contribution { surface, priority, scene, sticky_until, interruptible, reason }` or withdraws it. Sources: stock radio, AA/CarPlay nav, media, readings, alerts, phone, eggs, cover art.
- The arbiter chooses **per surface**: centre and top lines are independent. The top lines can show nav while the centre shows readings.
- Suggested ranks:
  1. Cluster-owned. Not ours; observed as 53 busy. Never fight it.
  2. Alerts (short TTL).
  3. Phone, incoming or active call.
  4. Nav approach (<500 m) or a maneuver change (5 s peek).
  5. User-selected page (stalk): media / readings / AA nav idle / eggs.
  6. Stock radio passthrough, when TV is not the source or Pi content is empty.
- Rules: hysteresis and hold (nav 3 s grace), manual-select suppression, "cluster selected content" resume, and `start_inactive` / claim-on-context as policy flags. Each is a separate rule object, not inline `if` chains.
- **Stock radio as a source:**
  - Text: decode AUDSCII from 0x7B4/0x7B5 into a `TopLines` contribution. When it wins, forward the raw bytes to 0x363/0x365 at the radio's pace (no re-encode). On a ~1.2 s gap, withdraw.
  - Graphics, two options:
    - **(a) Pi acts as a cluster emulator to the RNS-E** on 0x7B2/0x7B3. It answers the handshake with the real cluster's 09 record, grants 53 85, and parses the RNS-E's 52/57/39 stream into a `Center` scene. That scene is then just another contribution, re-rendered through our single cluster session. This is the clean option.
    - **(b) Transparent bridge** while the radio wins (the ddp_bridge approach). Simpler, but it needs two sessions sharing one cluster channel, which breaks the sequence and ownership model.

### Emulator and bench harness
1. **Wire-level cluster simulator** in Rust, implementing the cluster's side of the same tp2 + ddp state machine:
   - It answers with the captured 09 record (`09 20 0B 50 08 0B 50 30 39 00 32 00`).
   - It keeps a 128×96 framebuffer that executes 52/55/57/63/39 with the measured ROP rules (63 inherits the 55 mode; 2×2 logical scaling) and the font tables from `NativeWhiteFontCharts.json` / `font_metrics_data.json`.
   - Scriptable faults: startup busy ~30 s (84 → 05 → 2E → 85), random 9n, A8, a 0B on bad records, 53 C0 for non-central claims, a "freeze" on messages over 195 B or 55 LEN ≥ 0x80, late 0B errors.
   - It also simulates the RNS-E side (0x7B2–0x7B5 traffic) for the stock-radio source.
   - The current `dis_emulator` is a JSON-command canvas (`emulator_service.py:316-360`, `templates/index.html:1221-1310`). It cannot test the transport and acknowledges frames immediately.
2. **Golden tests:** scene → messages → simulator framebuffer → PNG compared with reference images. Font glyph PNGs are in `references/dis display references/`.
3. **Replay tests:** feed the bench `wire.jsonl` captures (`dis_emulator/bench_cluster/runtime/*/service/wire.jsonl`) and the car handshake into the session state machine with a fake clock.
4. **Hardware bench:** the A3 cluster via J2534 (`bench_cluster/README.md`) plus the TT car at 100 k. Every hardware result needs eyes or a camera; transport ACK ≠ pixels.

### Hard constraints
- ICAN is fault-tolerant 100 kbit/s CAN shared with everything else. Measure busload; never flood. Don't keep the RNS-E or bus awake: no writes while KL15 is off or the RNS-E is asleep (commit 1cff7e9, `dis_service.py:1384-1413`).
- Only the central window (0,27,64,48 logical = 128×96 physical) can be claimed on the white cluster. The top lines are 2 × 8 AUDSCII cells via 0x363/0x365, ≥50 ms between changes, and must stop when the display is busy or the phone bit is set.
- One DDP owner per channel. Ownership is the 53 grant only, never a transport ACK. Respect busy and 2E/2F; claim again only after 2E/2F or a free status, with a slow retry (old code: 5 s).
- Application message ≤105 B (195 worked, 208 froze). 55 records with LEN ≥ 0x80 are unsafe. 57 text ≤44 glyphs / 49 B. 39 is a separate message and not atomic. ACK block ≤15 frames, with 0n continuations inside a message.
- Must handle 9n (100 ms wait + suffix replay), missing ACK (resend the last frame), 0B late errors (not frame-correlated), repeated 00 mode requests, A3 keepalive every 2 s, A8 from either side, and a ~30 s startup busy period.
- Any session or ownership change invalidates believed pixels. The next frame must be a full redraw.
- At most one frame in flight, latest wins, frames are droppable. No unbounded queues.
- AUDSCII encoding, with lowercase a–p at 0x01–0x10.
- Never transmit while flashing is active (shared inhibit with the flasher).
