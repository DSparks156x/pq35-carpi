# Pages, logs and the portal

Plan for the next pass over the head-unit pages, captures, data logging and the phone portal. It replaces PLAN 2.12's page list where they differ.

## 1. Head-unit UI rules

Every page has to work with only the RNS-E controls. Touch is secondary.

| Control | Does |
|---|---|
| Knob turn | Move focus. The list scrolls so the focused row stays in the middle third. |
| Knob push | Activate: toggle, open, pick. |
| Prev/Next long (D-pad left/right) | Change the focused value in place: cycle a choice, step a number. On a tab bar, switch tabs. |
| RETURN | Back one level. RETURN long goes home. |

- One header per page: the title and, where a page has them, tabs. No developer notes, file paths or raw keys like `ignition.zas.x`. Raw keys only appear on a value's details view.
- Focus is always obvious: a filled highlight, not a thin ring.
- Simple values are edited where they sit, with the D-pad. A sub-screen only opens for picking from a long list.
- Nothing needs typing. Anything that does (custom labels, names) is done in the portal.
- Rows are at least 56 px, with text at least 22 px; about 6 rows fit. The theme follows `light.night`.
- Empty states get one line saying why ("not seen on the bus since start"), not a wall of `–`. Pickers hide values the car has never sent, behind a "Show unavailable" row.
- Disabled controls are hidden, not greyed out, unless their being there explains something.

## 2. Slot editor

One component, used three ways: Data page tiles, DIS readings pages and data-log channel sets. A *layout* is a list of pages, and each page has a fixed number of slots.

- **Edit view**: draws the page as it really looks, with slot outlines.
  - Knob: moves between slots.
  - Push: opens the picker for that slot.
  - D-pad: moves the focused slot's value left or right, to reorder.
- **Picker**: two levels, never more.
  - **Level 1**: Empty, Recent (the last 6 picked), then the categories. Each category row shows how many of its values the car is sending.
  - **Level 2**: one category, split into sub-groups under headers. Each row shows the label, the live reading and a small `diag` tag if the value costs polling.
    - D-pad: jumps to the previous or next category without backing out.
    - Push: assigns the value.
    - The value already in the slot starts focused.
  - **Order**: curated, not alphabetical. Actual comes before specified, FL FR RL RR, cylinders 1–4, the everyday values first. Unknown keys go last, alphabetically.
  - **Hidden by default**: values the car has never sent, behind a "Show unavailable" row at the end.
  - **Where the split lives**: carpid's catalog (`group` and sub-group per value), so the Data page, the DIS editor, the data-log editor and the portal all share it.

  | Category | Sub-groups (in order) |
  |---|---|
  | Engine | Basics (rpm, load, throttle, pedal, coolant, oil, intake) · Boost (actual, specified, N75, atmospheric, air mass) · Fuel (rail actual/specified, injection, lambda actual/specified) · Timing (ignition timing, retard cyl 1–4) |
  | Driving | Speed (speed, odometer) · Steering (angle, rate) · Brakes (handbrake, EPB) · Wheels (pulses FL–RR, reverse FL–RR, axle pulses, pulses/rev) |
  | Haldex | Temps (oil, plates) · Control (pressure, torque est., valve %, valve current) · Supply |
  | Fuel & trip | Fuel level, fuel low, outside temp; trip values when they exist |
  | Electrical | Battery (battery, ECU terminal 30) · Ignition (key in, 15, X, 50, P) · ZAS (S, 15, X, 50) |
  | Body | Doors FL–RR, bonnet |
  | Lights | Exterior (side, low, high, DRL, fogs, turns, hazards, reverse) · Dimming (display, 58t, 58s, cluster, gateway, sensors) · Light model (ambient, night, panel %, target, trim) |
  | RNS-E | On, mode, on TV |
  | Phone & nav | Next turn distance, ETA, road, track, artist, call state (bridge values; DIS and data logs only) |
  | Other | Anything carpid reports that the catalog doesn't place, by key prefix |
- **Pages**: add, remove and reorder are rows at the end of the edit view. Names come from a fixed list (Engine, Boost, Haldex, Electrical, Trip, Custom 1–4); custom names are set in the portal.
- **Storage**: `carpid` owns layouts, through the settings API. `carpi-ui.json` goes away.

| Use | Slots per page | Extra per slot |
|---|---|---|
| Data page | 16 (4×4), or 4 / 9 big | none |
| DIS readings | per screen type: 1 big, 2 or 4 lines | short label (catalog default, portal edit), decimals |
| Data log set | one list, no pages, no fixed cap | rate class; the editor shows the estimated rate and diag load |

## 3. Debug logs

Called "capture" in code (`carpid/src/capture.rs`, `logs.capture_minutes`); every user-facing label says "Debug logs" / "Collect debug logs". One collection saves the **last 5 minutes** of everything.

- **Triggers**:
  - Prev extended (exists), action "Collect debug logs".
  - Collect debug logs in Car Integration > Debug logs.
  - A button in the portal.
  - Automatic, only for crashes and bad faults (Drew): a LIVI crash (`carpi-capture@.service`), a carpid crash (found at the next start; journal only), CAN bus-off, interface gone or an error-passive storm, an update rollback. At most one per reason per 10 min and 6 an hour.
- **Contents**:
  - The CAN ring of every bus, as candump.
  - The journal for the same window, across all units: carpid, LIVI, sdlink/avsd, wayvnc, the kernel, NetworkManager/hostapd and bluetooth.
  - A snapshot of the value store.
  - The config files and the release manifest.
  - `info.txt`: trigger, Pi time, car clock, odometer, versions.
- **Layout**: `~/logs/<YYYY-MM-DD>/<NN>/`. NN starts at 01 each day. Data-log runs share the same day numbering, so a day reads 01 capture, 02 datalog, 03 capture.
- **Time**: the Pi clock is local-correct before the folder is named. The RTC holds it, and carpid sets it from the car clock (0x65D) when the two differ by more than 2 min.
- **Journal**: journald goes persistent, capped at 200 MB, so the window is there after a crash. The capture copies its window out as text straight away.
- **Retention**: logs use at most 25% of the disk, and the oldest days go first. A starred # is never deleted.

## 4. Data logging

See 5c, Logs. Storage: one pool with debug logs (`logs.max_pct`).

## 5. Portal

A rewrite of the page. Built for a phone first, served by the existing axum server, with no external assets.

- **Home**:
  - Status card.
  - A Capture now button.
  - A Days list: newest first, with the count and size of each day.
- **Day view**: each # with its time, kind (capture or datalog), trigger, size and star. Buttons: **Download day (.zip)**, **Download #NN (.zip)**, and per file.
- **Zips**: streamed while being built, not built in a temp file first. Text files are deflated; candump and CSV too.
- **Data log retrieval**: from a run, pick columns and a time range, then download a trimmed CSV. The default is all of it.
- **Setup tab**:
  - Config upload.
  - Firmware upload.
  - Wi-Fi networks (`/wifi`): the saved list (name, open or password, connected), Add (name, password, hidden) and Forget (with a confirm page). PIN when one is set. No scan or test: both drop the hotspot the phone is on, so the page points to the head unit.
  - Text entry for custom page names and DIS labels.
  - Later: flashing jobs, which log into a # like everything else.
- Bug report stays: today's tar.gz, rebuilt as "zip of the latest capture plus status".

## 5a. Car Integration settings (Drew, 2026-11-08)

All our settings live in one **Car Integration** tab at the bottom of LIVI's own Settings. Our separate Settings page and the Car tab go away.

- **Look**: native LIVI rows, built from our Rust code through small hooks in `livi-ui` (see the LIVI settings study). LIVI lists have no section captions, so grouping is by sub-page.
- **Organisation**: by what you're doing, then by source. Levels are fine when they make sense.
- **Rows**:
  - Every row does something on a press, and choices and steps also change with the D-pad in place.
  - Labels are short nouns ("Auto shutdown", "Fullscreen cover time"), never sentences. No hint rows.
  - A row that opens a page shows nothing, or a live status of that page ("Waiting for the cluster", "Night, 40 %", "Update available"). Never a setting inside it.
  - Two-push confirms read "Confirm <verb>".

### Tree

```
Car Integration
├─ Flashing mode                tx.flashing_mode (persistent CAN send inhibit)
├─ Status ›                     live: bus, ignition, RNS-E source, phone, cluster, carpid version
│  ├─ All values ›              every store value with its age
│  └─ Buses ›                   ring members, frames, gate refusals
├─ Cluster display ›            status: Showing … / Waiting for the cluster / Off
│  ├─ Enabled                   dis.enabled
│  ├─ Auto claim                dis.claim (on = always, off = cluster menu only)
│  ├─ Start page                Readings / Accel / Media      dis.default_page
│  ├─ Readings ›                named pages: add, rename, reorder, remove
│  │    └─ <page> ›             Name, values (slot editor + picker); per value: decimals, unit, icon
│  ├─ Accel ›                   Line 1–4
│  ├─ Navigation ›
│  │    Auto claim / Auto release   dis.claim_nav, release_nav
│  │    Style                   Fancy / Stock                 dis.nav_style
│  │    Turn distance           dis.nav_distance
│  │    Trip info               dis.nav_pocket; ETA / Time left / Distance left
│  │    Lanes                   Off / Replace road / Alternate dis.nav_lanes
│  │    Units                   Phone / Metric / Imperial     dis.units
│  │    Clock                   Phone / 12 h / 24 h           dis.clock
│  ├─ Calls ›                   Auto claim / Auto release
│  └─ Media ›                   Auto claim / Auto release / Layout (Text / Cover + text) /
│                               Fullscreen cover time (Off, 2–15 s)
├─ Screen & lighting ›          status: Day/Night, panel %
│  ├─ Auto brightness, Brightness now, Light sensor, Curve, Learned points, Scroll trim
│  ├─ Brightness curve ›        the plot and point editor
│  └─ Night mode ›              Source, Night below, Night from / Day from, Location
├─ Controls ›
│  ├─ Steering wheel ›          per key ›: Short / Long → action (choice; push opens the list)
│  ├─ Faceplate ›               per key ›: Short / Long / Held → action
│  ├─ Long press                ms                            input.long_ms
│  ├─ Extended hold             s                             input.extended_s
│  ├─ On-screen keyboard, Key order, Show passwords
│  └─ Key bindings              Reset to defaults
├─ Debug logs ›                status: N today
│  ├─ Collect debug logs
│  ├─ Length                    1–30 min                      logs.capture_minutes
│  ├─ Log storage               5–75 % (debug + data logs)    logs.max_pct
│  └─ Collected ›
├─ Power & clock ›
│  ├─ Auto shutdown             power.shutdown
│  ├─ Shutdown delay            power.silent_s (15–600 s)
│  ├─ Radio hold                power.radio_hold_min (0–120, 0 = off)
│  ├─ Clock from car            clock.set_from_car
│  └─ Time zone ›               clock.timezone
├─ A/V over SD ›                status: Not set up / Update available
│  ├─ RNS-E firmware, SDLink protocol (info)
│  ├─ SDLink firmware ›         status: Update available
│  ├─ Legacy decoder CRC
│  ├─ Stream status, Audio gain
│  ├─ Picture ›                 LINKCAP, MAXVID, KEYINT, DITHER, palette snap ... with ranges
│  └─ Restart stream
├─ Remote access ›              status: VNC on
│  ├─ Portal                    address (info)
│  ├─ PIN ›, Clear PIN, PIN for downloads   portal.protect_downloads
│  ├─ SSH                       Always on (info)
│  └─ VNC, VNC at boot
└─ Advanced ›
   ├─ Move to NVMe ›, Show Setup
   ├─ CAN transmit              tx.enabled (off = log only)
   └─ Key input                 input.uinput
```

Setup (first start, hidden by Finish setup; Advanced > Show Setup brings it back): Time zone ›, Wi-Fi networks ›, Phone connection ›, Cluster display ›, Remote access ›, Move to NVMe ›.

The bundled updater replaces LIVI's **Settings > System > Software Update** screen.
It shows the installed version, last update result (this session's attempt, else the recorded apply), Stable/Beta channel, Install,
and Wi-Fi networks. Installing asks for confirmation before dropping the hotspot.

The updater and **A/V over SD** also open the same **SDLink firmware** screen. It
keeps the RNS-E module identity separate from the negotiated link protocol and
shows installed and bundled firmware for both the RNS-E and Teensy. A newer bundle
is an optional update offer; an older RNS-E may keep streaming through the
encoder's negotiated compatibility mode. carpid supplies the comparison (build
history, stream formats, firmware table); the screen never orders version strings.
**Check versions** probes both (read only); **Install** needs a second press (Confirm install) and
runs carpid's install job (sdlink `docs/PI_UPDATER.md`). The sector 107 CRC read on
A/V over SD stays a diagnostic, not a version or compatibility check.
**Advanced: Flash Sector**: listed when the portal holds `[prefix]sector_<N>.bin`
uploads. Two presses; writes the file to RNS-E sector N (`FirmwareAction::FlashSector`).
No content checks.

### API additions (carpi-proto `Settings` / `SettingsPatch`)

- `flashing_mode`: saved as `[tx] flashing_mode`. It blocks all daemon CAN sends, drops queued
  frames and cancels retries, while passive RX continues. Turning it off restores the independent
  `tx.enabled` preference. A toast announces changes/startup, and an amber banner stays over all
  main screens, including projection, while on. Lightweight `system.flashing_mode` readings keep
  the banner current with the settings pages hidden. Separate CAN tools are outside this setting;
  a send already handed to the OS cannot be recalled.

- `logs { capture_minutes, max_pct }`
- `clock { set_from_car, timezone }`
- `power { shutdown, silent_s, radio_hold_min }`, which replaces the flat `power_shutdown`
- `input { uinput, long_ms, extended_s }`
- `portal { protect_downloads, address }`
- `update { channel, version }`
- `avsd { values, ranges }`, read from and written to `~/avsd/avsd.env` with a chain restart
- the `dis` fields already landed, including `pages`

All of these apply live, or are listed in `restart_needed`.

## 5b. Keyboards and Wi-Fi networks (Drew, 2026-11-08)

LIVI has no on-screen keyboard and no way to join a Wi-Fi network. The update needs a known network to fall back to when the hotspot drops (DEPLOY.md section 4), so both come together. Mockup: claude.ai/artifact/4bVjmpo5F5qB1tH94u4ZR3 ("keebs look good").

### Keyboards (fork, livi-ui)

- **One overlay, two layouts.** A sheet over whatever asked for text: the field on top, keys below. LIVI's own text fields (its Settings' text screens: car name, Wi-Fi password) open it when their screen opens and when the field is pushed (keys_ui's `StartEditing`); Done commits through LIVI's own text commit, so LIVI's checks still apply (a turned-down value brings the keyboard back with a toast saying why). Our fields open it too.
- **Knob layout**: letters in rows of 10, alphabetical by default (QWERTY in the setting), a last row of ⇧ (one capital), 123/#+, space, ⌫, Show/Hide, Done. Knob walks every key in reading order and wraps; Up/Down jump a row keeping the column; push types; Prev/Next held move the cursor; Return deletes; Return held closes without saving. Keys are tappable too.
- **Touch layout**: phone-style QWERTY, ~64 px keys, 123 page, Show/Hide, an action key named for the job (Join, Save). Faceplate Return still deletes.
- **Setting**: Car Integration, Controls: On-screen keyboard: Auto / Knob / Touch / Off. Auto (default) opens the touch layout while a touch screen is attached to the main display (LIVI's display list, updated as touch devices come and go), else the knob one. Off: our fields still open the knob layout for a real keyboard to type into; LIVI's fields work as upstream. Knob keyboard letters: Alphabetical / QWERTY. Show passwords while typing.
- **Who uses it first**: Wi-Fi password and name, accel test lines (typed with units), readings page names, portal PIN (instead of the digit screen), LIVI's hotspot password.

### Wi-Fi networks (carpid + fork)

- **carpid owns it**, through NetworkManager (`nmcli`), the same way it reads `nmcli general` for the update. API: `Request::Wifi { action: list | add | forget | test }`.
  - list: saved client networks (name, security, autoconnect, last used) and whether one is connected.
  - add: name, password (or open), hidden or not. Saved as a NetworkManager profile with autoconnect, so the update's step 3 finds it.
  - forget: deletes the profile.
  - test: the update's dance without the download: hotspot down, join, report, hotspot back.
- **The one radio**: `wlan0` is the hotspot while LIVI runs, so the Pi can't join a network (or scan properly) with it up. Adding saves; joining happens in the update or in test.
- **Scan** (Drew): Add a network drops the hotspot about 10 s to list nearby networks, then brings it back; the phone link drops meanwhile, and the screen says so before it starts. A name can still be typed for a hidden network. Action `scan` in the API.
- **Profile**: a polkit rule so the car user (a lingering session, not an "active" one) may change NetworkManager's system connections. Passwords live in the profile as NetworkManager keeps them (root-only files).
- **UI** (Drew): Settings > System > Software Update > Wi-Fi networks. Saved networks, each with Forget and Test; Add a network scans, then opens the keyboard for the password (or the name, for a hidden one).
- **Phone portal** (done): Setup, Wi-Fi networks. The same list, add and forget, through carpid's `wifi` module (a hook, so carpi-portal doesn't depend on carpid). Checked on the server too: name 1-32 bytes, password 8-63 characters or empty for open. Passwords are never logged or sent back in a page. Scan and Test stay on the head unit.

### Order

1. carpid Wi-Fi API and the polkit rule, with tests against a fake nmcli.
2. The keyboard overlay in the fork, both layouts, and the setting; LIVI's own text fields use it.
3. Wi-Fi networks screen and portal form.
4. Typed accel lines, page names and the PIN move to the keyboard.

## 5c. Data page (Drew, 2026-11-11)

Mockup: claude.ai/artifact/MuNUKGJRVd6pYSFr2JYzW7. Replaces the Data and Diagnostics pages and Car Integration's Logging.

```
Data                        top level, tabs: Panels · Logs · Faults
├─ Panels                   panel pages + page dots; Scratch last
├─ Logs                     Start/Stop, Mark, Profile, Profiles ›, Runs ›
└─ Faults                   scan on open; module ›: faults, Clear faults, Identity ›, Coding ›, Adaptation ›
```

### Controls

| Where | Knob | Push | D-pad ← → | D-pad ↑ ↓ | Return |
|---|---|---|---|---|---|
| Panel, viewing | Next/prev panel | Panel menu | Next/prev panel | ↑ tab bar | Home |
| Tab bar | Move | Open | Switch tab | ↓ page | Home |
| Panel, editing | Next/prev widget | Widget rows | Move widget | Move widget | Done |
| Lists | Move | Activate | Change in place | Move | Back |

A viewing panel takes no focus.

### Panels

- **Look**: LIVI telemetry. Black, hairlines between widgets, no boxes. Tiny spaced uppercase label, big light numeral, small unit.
- **Full screen**: a panel being viewed has no header; its name and the log timer sit in the footer by the page dots. Up puts focus on the tabs and the header comes back.
- **Touch**: swipe left/right flips panels; tapping a page dot goes to that panel. Knob and D-pad do the same.
- **Grid**: per panel: 1, 2×1, 2×2, 3×2, 4×3. A widget covers 1–n cells.
- **Panel menu**: Edit panel, Log this panel, Reset peaks, New panel, Panels › (add, rename, reorder, remove).
- **Edit**: brightness-curve layout. Live panel left, focused widget's rows right. Empty cells dashed; push adds a value.
- **Scratch**: always last. Auto layout 1 → 2 → 4 → 6 → 9. Save as panel.
- **Widgets**: Style changes keep the settings that still apply; the rest go back to catalog defaults.

| Style | Settings |
|---|---|
| All | Value(s), Style, Size, Label, Decimals, Unit (where it converts) |
| Number | Min/max: Off / Since start / Last 30 s |
| Gauge | Range, Red from, Peak: Off / Hold / Hold 10 s, Major ticks |
| Bar | Range, Warn from, Limit from, Segments |
| Graph | Values 1–3, Window 10/30/60 s, Range Auto/Fixed, Fill, Marks |
| Pair | Actual, Specified, Difference: Absolute / % |
| Set | Wheels 2×2, Cylinders 1–4, Doors; per item Number / Bar |
| Forks | DSG selector forks 1-3, 2-4, 5-N, 6-R (02 grp 15); sync points from grp 17/18, read once; engaged gears big |

- **Boost** (Drew): `engine.boost.actual`, `.specified`, `.cluster_mbar` are absolute. Per-widget Pressure: Absolute / Relative. Relative subtracts atmospheric: ICAN `engine.altitude_factor` × 1013 mbar (0x555, faster), else `engine.atmospheric_mbar` (group 113). Below zero reads as vacuum.

- **Portal editor** (later): usable from a laptop, so full panel/profile editing there is worth doing. Head-unit editor stays complete on its own.
- **Picker**: adds **Module groups ›**: module ›, Group (D-pad steps 0–255), four fields live (unit from the measuring-block formula), push adds a field, Add all four. Key `diag.<addr>.<group>.<field>`, label "01 · 103 · 2" until named in the portal. Not offered for ABS (03).
- **Polling**: panel on screen and a running log poll separately. No unioning.

### Logs

- Rows: Start/Stop, Mark (focused while running), Profile (D-pad), Log this panel, Profiles ›, Runs ›.
- Running row: profile, values, rate, rows, timer. A timer shows top right on every Data screen.
- Mark: numbered, toast "Mark N", lines on graph widgets. No key binding yet.
- No pre-trigger.
- CSV: `Time (s)` from start, then `<catalog label> (<unit>)` per value, then `Mark` (mark number on its row). Last value held between samples. Wall clock, odometer, profile, versions in `info.txt`.

### Faults

- Scan when the tab opens: install list, then faults per module as they answer. Faults first. TP2 modules only.
- Each scan writes a text report into the day folder.
- Module: faults (code, text, status, count, freeze frame), Clear faults (Confirm clear), Identity › (part no., component, SW, workshop), Coding ›, Adaptation ›.
- Clear all: no gating.
- Coding and adaptation: read. Write path built, disabled.
- Status (2026-11-11):
  - Identity: `1A 9B` part no., SW, component (offsets as PQ35-Flasher; matches the Haldex). Coding from `22 06 00`; older KWP modules may refuse it. Workshop not decoded.
  - Adaptation read: not supported yet (service/layout unconfirmed). carpid answers "Adaptation not supported"; no row on the head unit.
  - Freeze frame: not read (no captured reply). Status byte shown raw.
  - Writes: `carpi_core::diagnostics::WRITES_ENABLED = false`, "Writes disabled".
  - Clear all: the head unit clears module by module, one job at a time.

## 6. Order

1. Debug logs rename (3).
2. Data page tabs (5c): Panels, Logs moved out of Car Integration, Faults.
3. carpid diag: scan, identity, coding, adaptation; Faults tab.
4. Panel widgets: grid, styles, settings, Scratch.
5. Module groups in the picker.
6. CSV format.

## 7. Open

- Starting panels (Drew: better ones, later).
