# carpi-dis: cluster display driver design

Status: draft for Drew. No code. Replaces PLAN.md 2.7 layers 3–4 ("surfaces", "scene and renderer").

Path prefixes: `dc/` = `RNS-E-Hudiy/dis_client/`, `cRE/` = `clusterRE/`, `ref/` = `RNS-E-Hudiy/references/dis display references/`.
Tags: **[M500]** measured on the A3 bench at 500 kbit/s, direct to the cluster. **[V]** visually confirmed by Drew. **[E100]** estimate for the car, not measured. **[G]** guess.

## 0. The short version

- The cluster is a **text-and-glyph terminal with a slow raster side door**. Text, ROM glyphs, lines and clears cost 6–30 bytes each. A full-centre bitmap costs ~630 B. A native-res image costs 1.4–20 KB.
- So there is no framebuffer, no scene and no differ. **Every screen is hand-authored**: an ordered list of cluster commands with slots for variable data, plus that screen's own rules for what to re-send when a slot changes.
- Each screen gets something usable up in the first message or two: icon first, then text. Slow detail work (high-res passes) runs in a cancellable **refine lane**, interleaved with the **update lane** (distance, scrolling), which always goes first. A few reusable helpers (number field, scroll text, quick-then-refine icon, glyph cells) cover the common cases. There's no engine.
- The shared layer under the screens is thin: TP2 link, DDP session, command encoder, message packer, text metrics, frame pacer. The arbiter picks which screen owns the centre and what goes on the top lines.
- In the car the Pi is on ICAN (100 kbit/s), the cluster is on its own 500 kbit/s bus, and the gateway sits between them. Every DDP byte crosses ICAN, which is shared, and every ACK crosses the gateway twice. Bench **throughput** numbers don't carry over. Bench **cluster behaviour** (limits, ROP rules, pacing needs) does.

## 1. Cost model

### 1.1 The path

```
Pi ──ICAN 100k (shared)──▶ gateway ──cluster bus 500k──▶ cluster
   ◀── ACK Bn / 9n / 53 ──        ◀──
```

- One 8-byte CAN frame on ICAN is ~1.1–1.35 ms with stuffing. A TP2 ACK (1 data byte) is ~0.5 ms. **[E100]**
- TP2 carries 7 application bytes per frame. App bytes B → `ceil(B/7)` data frames, plus one ACK per ≤15-frame block (`dc/ddp_protocol.py:1213-1217`; block 15 ran fine **[M500]** `cRE/dis_findings.txt:206-209`).
- Bench ACK round trip is ~9 ms per block, with ACKs on a ~10 ms grid in hardware timestamps **[M500]** (`cRE/native_transport_performance_findings.txt:19-20, 47-50`). The 10 ms grid is probably the cluster's task tick **[G]**, so it should carry over.
- **Car ACK round trip [E100]** = ICAN time of the block + gateway forward + cluster tick (~10 ms) + gateway forward back + ICAN time of the ACK. For a full 15-frame block that's ~18 ms of ICAN plus 10–15 ms of latency, so roughly 30 ms per block versus 9 ms on the bench. Gateway forwarding latency is unknown (Q2).
- Gateway direction: 100k in, 500k out, so the gateway drains faster than we fill it. Bursts of 15 frames shouldn't queue there unless the cluster bus is busy **[G]**.
- The TP2 receiver-wait penalty is fixed: each `9n` costs 100 ms plus a replay (`cRE/native_transport_performance_findings.txt:11-18`; J2819 T_Wait, `:75-88`). That doesn't change with bitrate.
- Two currencies matter:
  1. **ICAN time**: shared with every other ICAN node and with our diag. Budgeted by the PLAN 2.1 gate.
  2. **Cluster render work**: native strokes sent too fast corrupt the picture (5 ms message gap corrupted tiles, 10 and 20 ms were clean **[M500][V]**, `cRE/nav_sweep_checkpoint.txt:79-81`), and heavy line work draws `9n` (`cRE/native_transport_performance_findings.txt:66-73`). The cluster's work rate is the same in the car. At 100k we feed it more slowly, so render-work limits should bite *less* often than on the bench **[G]**.

### 1.2 Commands

Coordinates: logical 64×88, white centre = (0,27,64,48), drawn 2×2 → 128×96 px (`dc/native_bitmap.py:10-13`). `52` is absolute. `57` and `63` are relative to the current clip (`dc/dis_service.py:572-577`). Only the central claim is granted; others get `53 C0` **[M500]** (`cRE/ddp_top_region_bench_results.txt:15-25`).

| Command | Bytes | ICAN frames [E100] | What we know | Artefacts / limits |
|---|---|---|---|---|
| **57 text**, n chars, fixed/prop font | 5+n | 10-char field: 2 | Cheap, renders instantly at bench speeds **[M500][V]** | Opaque mode (`..10`) paints only its own advance, so a narrower string leaves a tail (`dc/README.md:42-45`). Keep records ≤49 B (stock budget, `dc/dis_service.py:629-648`). |
| **57 graphics-font glyph run** (flags 0A opaque / 0B transparent) | 5+k | 1 per glyph cell | Stock nav: arrows and approach bar are graphics-font glyphs, not bitmaps (`ref/pcbbc.txt:150-160`; `dc/native_nav_glyphs.py:1-8`) **[V]** | ≤44 glyphs per record (`dc/native_nav_glyphs.py:44`). 118 usable glyphs, 12 px advance (`dc/font_metrics_data.json`). |
| **63 line**, H or V | 6 | 1 | Logical lines up to 60 long confirmed (`cRE/dis_findings.txt:104-106`) **[V]**. Physically V1 = 1×2 px, H1 = 2×1 px. LEN 0 is a no-op (`:465-468`) **[V]**. | **Inherits SET/XOR from the last 55 mode** (`:588-596`) **[V]**. A stray XOR-mode 55 turns later lines into XOR. |
| **52 clip + clear/fill** | 7 (+7 to restore the full clip) | 1–2 | Cheap. Flag 02 clears, 03 fills (inverse background, `dc/dis_service.py:519-528`) | Clear-then-draw is visible because nothing is atomic: there's a blank flash between the passes **[G, consistent with V]**. |
| **55 bitmap, logical** | 5 + rows·⌈w/8⌉ per record, plus 14 for clip/restore | 32×38 arrow ≈186 B ≈ 27 frames | X must be 0 inside its clip. MSB-first. Multi-row records verified up to 12 rows **[V]** (`cRE/dis_findings.txt:482-488`; `dc/bitmap_rows_profile.txt:22-26`) | **LEN ≥ 0x80 froze it** (22 rows, LEN B3) **[V]** (`cRE/dis_findings.txt:517-524`). Width not a multiple of 8 → one row per record (`dc/dis_service.py:690-705`). |
| **Full-centre logical bitmap** 64×48 | 625–639 | ~90–93 + 6–7 ACKs ≈ **110–125 ms ICAN + ~200 ms latency** | 4–5.7 updates/s **[M500][V]** (`cRE/dis_findings.txt:284-293, 333-335`). Uncapped 16.7/s tore and only showed "a few" updates **[V]** (`:532-536`). | Partial passes are visible. In the car: ~2–3/s at best, and each one eats ICAN **[E100]**. |
| **Native-res strokes** (zero `55 mode1` selector + `63` per stroke) | 12 per stroke | 72×72 nav icon: 1366 B, 31 msgs ≈ 196 frames ≈ **0.25 s ICAN + ≥0.3 s gaps** | All 16 2×2 patterns, cow and apes confirmed **[V]** (`cRE/dis_findings.txt:616-656`). Icon ACK in 0.26 s at gap 0, 0.81 s at 10 ms, 1.04 s at 20 ms **[M500]**. | Selector needed before *every* stroke. Group priming, LEN3 empty selector and bulk 63 all failed (`cRE/native_performance_checkpoint.txt:18-21, 62-66`). Needs ≥10 ms between messages. Full-screen refresh visible on icon change (`cRE/nav_sweep_checkpoint.txt:82`). Apes: 20 KB, 3–5 s per frame. |
| **39 commit** | 1 | 1 (+1 ACK) | Stock sends it as a separate message (`dc/dis_service.py:707-709`). Inline 39 worked on the bench **[V]** (`cRE/dis_findings.txt:334-335`). | **Not atomic.** A no-commit probe still updated the screen **[V]** (`:385-386`). |
| **Top lines 0x363/0x365** | 8 + 8, plain CAN | 2 | One AUDSCII byte per cell (`dbc:1564-1582`) | Change no faster than every 50 ms. Stop when the display is busy or the phone bit is set (`dbc:4993-5018`). |

**Message limits** (application message = 2n…1n, independent of the ACK block):
- ≤105 B: verified policy **[V]**.
- 156–195 B: worked **[V]**.
- 208 B: froze the display while ACKs kept coming, and recovered without a reset **[V]** (`cRE/dis_findings.txt:270-279`).
- Coalescing several windows into ≤768 B froze it (`:228-233`).
- 195 B gives no throughput gain over 105 B (`cRE/native_transport_performance_findings.txt:52-59`; `cRE/bitmap_budget_experiments.txt:4-24`). **We use 105 B.**
- 84 B messages mixing raster and strokes: shifted border, corruption **[V]** (`cRE/native_performance_checkpoint.txt:112-113`). The packer must keep records whole and never split a selector from its stroke.

### 1.3 Tricks from the old code worth keeping

- **ROM glyph catalogs.** `dc/stock_mono_catalog.json` (hash-pinned) and `dc/native_nav_catalog.json` (139 mono placements).
  - A whole stock nav screen is **135 B in 4 messages** (≈20 ICAN frames): clip, 5 bar cells, blanked road line, road text, icon clip+clear, 10 glyphs (`dc/stock_mono_frame.py:78-87`, measured by compiling one).
  - A bar level change touches one `57 04 0A 39 yy gg` cell: **6 B, one frame**.
  - Icon glyphs on one row sit 6 logical apart, so they could merge into one run (24 B → 9 B) **[G, untested]**.
- **Fixed-advance digits.** Native proportional digits are all 10 px and fixed-font digits 12 px (`dc/font_metrics_data.json`). A right-aligned number field padded with figure-width blanks never needs a clear. The proportional space is 4 px, so pad with fixed-font spaces or the 12 px 0x65 blank the stock uses (`ref/pcbbc.txt:123-126`).
- **Measured text metrics** in physical px for all three fonts (`dc/font_metrics.py`; capture method `cRE/nav_sweep_checkpoint.txt:86-91`). Use them for fitting and tail clears, not for pixels.
- **Tail-only clears** (`dc/text_render.py:40-72`): clear only when the old footprint sticks out past the new one.
- **Row-grouped 55 records** (≤12 rows, LEN < 0x80), packed whole into ≤105 B messages: 625 B per full centre is the minimum (`cRE/full_bitmap_cost_review.txt:27-42`).
- **Sticky clip.** Leaving the full-centre clip set between frames saves 14 B. It must be re-primed after any session change (`cRE/dis_findings.txt:334-336`).
- **Deferred refinement.** The base goes out first and native refinement planes follow one per frame, so text stays live (`dc/dis_display.py:1293-1300`).
- **Hold through route gaps** instead of flashing "No Route" (`dc/dis_display.py:584-605`).
- **Not worth keeping:** coalescing across windows (froze), 22-row records (froze), 84 B packing, XOR deltas for native images (no reliable gain, `cRE/native_performance_checkpoint.txt:28-33`).

### 1.4 What carries from bench to car

| Carries (cluster-side) | Doesn't carry (path-side) |
|---|---|
| Size limits: 105/195/208, LEN < 0x80, ≤44 glyphs | Updates/s, ACK block 9 ms, frame times |
| ROP latch, selector rules, 2×2 scaling, clip rules | Gap 0 being "free" (at 100k each frame costs ICAN for everyone) |
| Native strokes need ≥10 ms message gap | Native image timings (car adds ~0.25 s of ICAN per icon) |
| 9n behaviour and 100 ms wait | 9n *rate* (should be lower in the car **[G]**) |
| Handshake and 53/2E/2F arbitration (car-confirmed handshake, `dc/NATIVE_HANDSHAKE_CONTROLLER.txt:55-57`) | Anything the gateway does: latency, filtering, buffering |

## 2. Driver design

### 2.1 Transport and session (keep PLAN 2.7 layers 1–2)

- The PLAN holds up: TP2 link (2n/0n/1n, Bn, 9n wait 100 ms + suffix replay, A3 every 2 s, A8), DDP handshake 15→00/01→08/09→20/21→33, claim `52 05 82 00 1B 40 30`, owned only on `53 85`, busy→free→2E→2F→reclaim, one generation counter.
- Additions:
  - **Per-message gap**, set by the screen per message class. Text and glyphs: 0. Native strokes: 10 ms.
  - **Adaptive pacing**: after a 9n, raise the message gap for the next N frames, then decay it.
  - **ACK timeout** sized for the gateway path (keep 1 s; measure the real figure).
  - **Session register file**: current clip, last 55 mode (ROP latch), "full clip sticky", all invalidated on a generation bump. The encoder reads and writes these; screens never guess them.
- The capability record from `09` is kept raw. The stock-radio peer (2.6) replays it to the RNS-E.

### 2.2 Thin shared layer (no scene, no differ)

- **Encoder**: typed constructors for the cluster's own commands. `text(x,y,font,align,ink,audscii)`, `glyphs(x,y,flags,bytes)`, `line(x,y,H|V,len)`, `clip(rect)`, `clear(rect)`, `fill(rect)`, `bitmap_rows(rect,mode,rows)`, `native_strokes(...)` (quarantined), `commit()`. Each one validates the hard limits when it's built: 57 ≤44 glyphs and ≤49 B; 55 LEN < 0x80, X = 0, width rules; coordinates inside the claimed centre.
- **Latch tracker**: before any `63`, emits a SET selector if the ROP latch isn't SET (or isn't known). Before any relative command, emits the clip it needs if the clip register differs. This is the only "smart" thing in the shared layer, and it's mechanical.
- **Packer**: whole records into messages ≤105 B in the screen's order. It never reorders and never splits a record or a selector/stroke pair. A message boundary can be forced (`barrier`) where a screen wants a visible step.
- **Text metrics**: `measure(font, bytes)`, `fit(...)`, `pad_to(width)` with a blank-cell table per font, AUDSCII encoding.
- **Frame pacer**: one frame in flight. A frame is ≤N messages plus 39. Latest wins: screens mark slots dirty rather than queueing frames. Before a frame goes out, the pacer asks the bus budget (PLAN 2.1) for its ICAN frames.
- **Cost estimator**: exact, from the packed bytes: data frames, ACK blocks, messages, gap time, so ICAN ms and wall ms. Screens use it to choose between their own ladder steps (2.4).

### 2.3 Screens: hand-authored command lists

A screen is a Rust module with:
- `layout`: fixed rects for each slot, chosen by hand against metrics.
- `enter(state) -> Frame`: the full paint, in a deliberate order.
- `update(old, new) -> Frame`: per-slot rules saying what to re-send. Unchanged slots send nothing.
- `ladder`: the screen's own cheaper variants under budget pressure.
- On `enter`, `update` and an alert-overlay restore, the screen gets the session's register state. On a generation bump the pacer calls `enter` again; screens hold no belief across sessions.

#### Two lanes per screen: update and refine

Drew's pattern: get something usable up in the first message or two, then refine as bandwidth allows, with text updates always first.

- **Update lane**: cheap slot re-sends (text runs, glyph cells, lines). Latest value wins. Always served first in every frame.
- **Refine lane**: slow detail work (native high-res passes, big bitmaps), cut into **slices** of ≤N whole messages. A slice fills only the budget left over after the update lane, with its own message gap (10 ms for native strokes).
- Each refine job carries the **slot version** it was built for. When the slot changes (new maneuver), the job is cancelled at the next slice boundary. Slices are whole messages, so a cancel never leaves a split record. The job's own fast base gets redrawn by the update lane, which covers any half-refined pixels.
- Per screen, a small priority table orders the slots in the update lane and says whether refine may run at all (e.g. no refining while a distance countdown is under 100 m).
- A generation bump drops all refine jobs. The next `enter` restarts them.

#### Toolkit (small, reusable, not an engine)

- `NumberField`: right-aligned run in a fixed rect, padded with figure-width blanks. A change re-sends its own run only. Never clears.
- `ScrollText`: a fitted window over a long string, one padded run per step. Speed and pause-at-ends are parameters. Under pressure it slows and then stops.
- `QuickThenRefineIcon`: a fast base (ROM glyph clone or logical bitmap), then an optional native refine job sliced into the refine lane. A glyph base never refines.
- `GlyphCells`: a set of graphics-font cells at fixed anchors that re-sends only changed cells (approach bar, lane arrows).
- `InverseLabel`: fill + black text for focus or alerts, with its restore.

Each helper takes explicit rects from the screen. Screens still own the order. Lane displays fit the same model: a `GlyphCells` row of lane-arrow glyphs (if the graphics font has usable ones) or small logical bitmaps per lane, updated per cell.

#### Example A: nav maneuver (Drew's order)

Layout (logical, relative to the centre): icon (0,0,54,40); bar cells x=57, y=3/10/17/24/31; distance value (40,8,16×9), unit (40,17); road line y=41, full width; optional lanes row y=33 (later).

`enter`. Nothing is atomic, so the clear comes first and the icon is the first thing on screen:
1. Clear full centre + full clip (14 B).
2. **Icon, fast**: a ROM graphics-glyph clone of the stock arrow (≈60–130 B, done; no refine), or for unmapped maneuvers a 32×38 logical bitmap (~186 B). Usually 1–2 messages.
3. **Text**: distance value and unit (`NumberField`, ≈15 B), road (`ScrollText`, ≈25 B).
4. Bar: 5 opaque glyph cells (30 B).
5. `39`.

Total ≈150–250 B ≈ 25–36 ICAN frames ≈ 35–50 ms of ICAN **[E100]**. Then:

6. **Refine (bitmap icons only, opt-in)**: the native 72×72 pass, ~1.4 KB as ~31 messages, sliced ≤3 messages per frame at a 10 ms gap. That's ~2–4 s at the DIS share **[E100]**, interleaved with the updates.

`update` rules (update lane priority: distance > bar > road scroll > icon change):
- **Distance changed**: `NumberField` run. **8–10 B, 2 frames.** The unit re-sends only when it changes.
- **Bar level changed**: changed cells only. Usually **6 B, 1 frame**.
- **Maneuver changed**: cancel any refine job → icon clip + clear icon rect + new fast icon + full clip. Bar, distance and road are untouched, so no full-screen flash (the old native path flashed, `cRE/nav_sweep_checkpoint.txt:82`). Then start a new refine job if it's a bitmap.
- **Road changed or scroll step**: one padded run (~25 B), 2 steps/s by default.
- **Route gap < 3 s**: send nothing.

Ladder under pressure: pause refine first → slow scroll → quantise distance more coarsely when far → bar at 2× steps → last resort, a text arrow label.

#### Example B: readings grid (2×4)

Layout: title row y=0 (inverse when focused); 8 value cells at y = 12, 21, 30, 39 in two columns of 32 logical; each column has a 6-wide icon gutter and a value field 20 wide, right-aligned.

`enter`:
1. Clear centre + full clip.
2. Static decor: icons as small logical bitmaps (8×8: 7 + 13 + 7 B each, or one clip per column with all 4 icons as rows of one bitmap: ~7 + 5 + 32 + 7 B).
3. Optional separator `63` V line at x=32 (6 B, after a SET latch).
4. Title text.
5. Values, top to bottom.
6. `39`.

`update` rules:
- **Value changed**: one text run, right-aligned in its 20-wide field, left-padded with figure-width blanks so the footprint is constant. **≈10 B, 2 frames.** Never clear.
- If a value goes wider than its field, show `--` (old rule, `dc/apps/readings.py:250-256`). Never clip into a neighbour.
- Units are static decor and aren't re-sent.
- **Focus moved**: two runs. The old title or cell goes back to normal (fill clear + text) and the new one is drawn inverse (fill + black text, 19+n B).
- Values are re-sent no faster than their scheduler rate, and the screen caps itself (e.g. 4 frames/s). At ~10 B per value that's ~3 kbit/s of ICAN at full tilt **[E100]**.

Ladder: lower the per-value rate, then update only half the cells per frame (round-robin).

#### Other screens (same pattern, sketched)

| Screen | Enter cost | Hot updates |
|---|---|---|
| Media | 3 text lines + play glyph + progress line ≈ 80 B | Scroll step 25 B. Progress: add a 6 B line segment, clear the tail on seek. |
| Call | Name, number/state, glyph ≈ 60 B | Timer once per second: one 5-char run, 10 B |
| Alert overlay | Fill banner + black text ≈ 40 B | On dismiss the owning screen repaints the slots whose rects intersect the banner (each screen declares slot rects, so this is a lookup, not a differ) |
| Cover art | Logical 64×48 one-shot, 7 messages ≈ 630 B; native only opt-in | None. Never animated in the car. |
| Stock radio mirror | Replays the RNS-E's records (2.6) | Forwards its records |

### 2.4 Budget and graceful degradation

- The DIS share of ICAN is config in the PLAN 2.1 gate. Proposal: 20% ≈ 200 frames/s ≈ 1.4 KB/s of application bytes.
- The pacer offers each frame tick (e.g. every 100 ms) the bytes available. The screen builds the frame from its dirty slots in its own priority order: nav distance > bar > maneuver > scroll. Anything that doesn't fit stays dirty with the newest value. Nothing queues.
- The update lane is served first. The refine lane gets only what's left, so a 1.4 KB native icon can never block a 10 B distance tick. Refine slices are ≤3 messages per frame and cancellable by slot version.
- Page switches cost one `enter` (~100–220 B for text screens). The arbiter's min dwell stops flapping.
- Fallback order across a screen: native bitmap → logical bitmap → ROM glyph → text label (e.g. "←300m").

### 2.5 Arbiter

- Keep the PLAN's per-surface rule table: Alert > NavManeuver > Call > UserPage > Media > StockRadio > Idle, with min dwell, nav approach/peek hysteresis (`dc/navigation_policy.py`), the 3 s route-gap hold and manual-select suppression.
- The cluster outranks everything. `53 84` pauses us. `53 05` + `2E` triggers reclaim and a full `enter`. A 2E/2F while idle means "cluster menu selected our page" (`dc/dis_service.py:175-215`).
- The arbiter returns a **screen id**, not content. Screens are switched, never blended. The one exception is the alert overlay, which the owning screen knows how to restore.
- **Top lines** are a separate surface:
  - The Pi writes 0x363/0x365 only while the RNS-E is on TV, `rnse_alive`, Kl15 on and the cluster isn't busy/phone (`dbc:4993-5018`).
  - On other sources the firmware sends them itself (`RNSE hacking/R41_FEATURES.md:53`), so the Pi is silent.
  - Sources: the radio text from 0x7B4/0x7B5, forwarded byte-exact, or our own 16-cell strings.
  - At most one change per 50 ms and no heartbeat spam. The old 100 ms resend and 3 reactive copies (`dc/dis_top_display_service.py:56-70, 583-610`) existed only to fight the OEM writes, which the redirect removes.
  - Skip any top-line content the centre already shows.

### 2.6 Stock radio as the cluster's peer on 0x7B2/0x7B3

- `carpi-dis` runs the cluster side: TP2 acceptor, `15`→`00 01`, `08`→`09 <real cluster's record>`, `20`→`21` echo, keepalive replies.
- **Option A: busy/free.** We tell the RNS-E `53 84` while it isn't winning, and `53 05` + `2E` when it wins, so it repaints itself. No shadow state. But it depends on how the RNS-E behaves when held busy (unknown), and every switch-in costs an RNS-E round trip.
- **Option B: always grant + shadow (recommended).** Always grant `53 85`. Keep the RNS-E's retained record list: its screens are ≤~20 records at fixed anchors, and a `52` clear drops records anchored inside the rect. When it wins: clear, replay the list, then forward new records live through our session, re-packed and latch-tracked. Switching is instant and independent of RNS-E timing. Keep `2E` as the escape hatch to force a full repaint when the shadow is in doubt (after a daemon restart).
- Its records are already cluster-native (stock 49 B budget), so the mirror costs about what the RNS-E itself would.
- 0x604 feeds context only (is RNS-E guidance active), not content.

### 2.7 Alternatives rejected

- **Retained scene + differ** (PLAN draft): it hides the cost and order decisions that make the cluster look right, and it invites full-raster fallbacks. Rejected per Drew.
- **Framebuffer + dirty-rect bitmap**: correct for any content, but a single 10 B distance tick becomes ≥100 B of bitmap, and full-centre updates use ~50% of the DIS share at 2/s. Cover art only.
- **Native 128×96 as the default**: ~4× the bytes of logical, needs 10 ms gaps, and flashes on change. Opt-in, static content only.
- **195 B messages**: no gain, more risk. Stay at 105.

## 3. Testing

- **Wire-level cluster simulator** (`carpi-sim`):
  - TP2 cluster side with a **path model**: ICAN 100k frame times, gateway delay (parameter, default 2 ms each way **[G]** until measured), cluster 10 ms tick, BS 15. Run with a 500k direct preset to reproduce bench numbers.
  - Application: handshake with the captured `09` record; claim rules (centre only, else `53 C0`); scripted startup busy 84→05→2E→85; menu-busy injection; 2E/2F page selection.
  - Renderer: 128×96 px, real fonts from `ref/NativeWhiteFontCharts.json`, 52/55/57/63/39 semantics including the ROP latch, clip-relative coordinates and 2×2 scaling. It applies each message as it arrives and **records every intermediate framebuffer**.
  - Faults:
    - message >195 B → freeze (the driver must never reach it);
    - 55 LEN ≥ 0x80 → freeze/partial;
    - 57 >44 glyphs → reject;
    - native strokes with <10 ms gap → corruption flag;
    - render-work queue overflow → `9n`;
    - random 9n, A8, repeated `00 01`, late `0B 03 xx` 14–513 ms after the transfer.
- **Golden tests per screen** (fixture state → frame):
  - Byte-stream snapshot.
  - Final PNG.
  - **Flicker metric**: pixels that pass through a value matching neither the before nor the after frame. Distance, bar and value updates must score 0.
  - Cost assertion: e.g. distance tick ≤2 ICAN frames, nav enter ≤35, readings value ≤2.
- **Refine tests**: on virtual time, a maneuver change mid-refine cancels at the next slice boundary; distance ticks keep their latency (≤1 frame tick) while a refine job runs; the final PNG after cancel + new icon matches the golden.
- **Property tests**: the encoder and packer never emit an over-limit record or message, never split a selector/stroke, never draw `63` with an unknown latch, and never send while not granted.
- **Replay**: bench `wire.jsonl` captures (`dis_emulator/bench_cluster/runtime/*/service/wire.jsonl`) through the session machine on virtual time; AA nav traces through arbiter + screens with bus share measured on the 100k path model.
- **Bench** (A3, 500k direct):
  1. Session, claim, release, busy recovery against carpid.
  2. One primitive palette page photographed against its golden.
  3. Each screen's enter and update sequences, photographed.
  4. A **pacing sweep with an artificial 100k + delay shim** in the bench owner (rate-limit TX to 100k frame times, add delay to ACKs) to rehearse the car path.
  5. Native icon at 0/5/10/20 ms gaps, re-confirmed.
- **Car**:
  1. Passive capture with FIS-Control off: the TT `09` record, stock screen bytes and pacing, gateway latency (timestamp 0x6C0 out vs 0x6C1 ACK in on ICAN).
  2. FIS-Control on, pure forward bridge to prove the redirect.
  3. carpid owning the centre: measure ACK round trip, 9n rate and DIS share of ICAN under driving load, with eyes or camera on each screen.
  4. Then the stock-radio peer.

## 4. Questions only Drew can answer

1. Is the TT 8J centre also limited to (0,27,64,48) with the same `09` record? Can I take one passive FIS-Control-off capture to get it, plus the stock screen bytes?
2. Can the bench cluster be wired through a real gateway (or a TT/A3 gateway on the bench) so the 100k + gateway path is measured rather than shimmed?
3. What share of ICAN can the DIS take? (Proposal: 20%. Diag competes for the rest.)
4. Native 128×96: keep it at all in the car? It's opt-in nav icons and cover art only, and costs ~0.5–1 s per icon change at 100k **[E100]**.
5. Nav arrows: are ROM glyphs (stock look) as default, with 32×38 logical bitmaps for unmapped maneuvers, OK? Or do you want the Google-style native icons back?
6. Stock radio mirror: is the RNS-E's own nav used at all in the TT, or is it AA only? That decides whether the 0x7B2 peer is worth building early, and whether option B (always grant + shadow) is acceptable.
7. Top lines on TV: forward the radio's text, or show our media/nav text?
8. Long text: marquee scroll, page-flip, or truncate with an ellipsis? At what speed?
9. Is a brief blank on page switch acceptable? Text updates won't flash, but a full `enter` clears first.
10. Which screens survive (PLAN Q13): readings, acceleration test, openpilot, cover art, eggs?
