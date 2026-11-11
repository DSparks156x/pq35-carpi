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

## 3. Captures

One capture is one press of the capture trigger. It saves the **last 5 minutes** of everything.

- **Triggers**:
  - Prev extended (exists).
  - A Capture button on the Logs page.
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

- Profiles: a channel set (slot editor), a rate, and manual start/stop. Start and stop from a Logs page row or a bound key.
- Each run is a # in the day folder: `data.csv` (wide: time plus one column per value, rows at the profile rate, blank when stale), `profile.json` and `info.txt`.
- Diag-sourced values go through the catalog scheduler (PLAN 2.8). The editor shows what the set will cost before you start.

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
  - Names say what happens.
  - Live state sits next to the setting it explains.

### Tree

```
Car Integration
├─ Flashing mode                tx.flashing_mode (persistent CAN send inhibit)
├─ Status                       live: bus, ignition, RNS-E source, phone, cluster, carpid version
│  ├─ All values ▸              every store value with its age
│  └─ Buses ▸                   ring members, frames, gate refusals
├─ Cluster display
│  ├─ Show on the cluster       on/off                        dis.enabled
│  ├─ Our screen shows          Always / From the cluster menu  dis.claim
│  ├─ Start page                Readings / Accel / Media      dis.default_page
│  ├─ Readings pages ▸          named pages: add, rename (portal for text), reorder, remove
│  │    └─ <page> ▸             Name, Cells ▸ (slot editor + picker), per cell: decimals, unit, icon
│  ├─ Navigation ▸
│  │    Take over when a route starts   dis.claim_nav
│  │    Give back when it ends          dis.release_nav
│  │    Look                            Ours / Stock            dis.nav_style
│  │    Distance to the turn            dis.nav_distance
│  │    Trip info                       dis.nav_pocket
│  │      ETA / Time left / Distance left  dis.nav_eta, nav_time_left, nav_dest_distance
│  │    Lanes                           Off / Instead of the road / Alternate with the road  dis.nav_lanes
│  │    Units                           As the phone / Metric / Imperial  dis.units
│  │    ETA clock                       As the phone / 12 h / 24 h        dis.clock
│  ├─ Calls ▸                   Take over on a call / Give back after it
│  └─ Media ▸                   Take over on a new track / Give back when it stops /
│                               Layout (Text / Cover + text) / Cover flash (Off, 2–15 s)
├─ Screen & lighting
│  ├─ Auto brightness, Brightness now, Light sensor, Curve, Learned points, Scroll trim
│  ├─ Brightness curve ▸        the plot and point editor
│  └─ Night mode ▸              Decided by, Night threshold, Night from / Day from, Location
├─ Controls
│  ├─ Steering wheel ▸          per key: Short / Long → action
│  ├─ Faceplate ▸               per key: Short / Long / Held 10 s → action
│  ├─ Long press                ms                            input.long_ms (panel)
│  ├─ Hold for system actions   s                             input.extended_s
│  └─ Reset to defaults
├─ Logging & captures
│  ├─ Logging ▸                 the logging page (profiles, start/stop)
│  ├─ Capture now
│  ├─ Capture length            1–30 min                      logs.capture_minutes
│  ├─ Space for logs            5–75 % of the disk            logs.max_pct
│  └─ On your phone             http://10.10.0.1:5003 (info)
├─ Power & clock
│  ├─ Shut down when the car is off      power.shutdown
│  ├─ Wait after the car goes quiet      power.silent_s (15–600 s)
│  ├─ Stay on while the radio plays      power.radio_hold_min (0–120, 0 = off)
│  ├─ Set the clock from the car         clock.set_from_car
│  └─ Time zone                          clock.timezone (picker)
├─ A/V over SD
│  ├─ Status                    Teensy, link, stream (info)
│  ├─ Firmware ▸                RNS-E / Teensy installed and bundled versions, negotiated SDLink version
│  ├─ Audio gain                dB                            avsd GAIN
│  ├─ Picture ▸                 LINKCAP, MAXVID, KEYINT, DITHER, palette snap ... with ranges
│  └─ Restart the stream
├─ Phone portal
│  ├─ Address                   info
│  ├─ PIN                       set / clear
│  └─ Downloads need the PIN    portal.protect_downloads
└─ Advanced
   ├─ Send to the car           tx.enabled (off = log only)
   └─ Keys to LIVI              input.uinput
```

The bundled updater replaces LIVI's **Settings > System > Software Update** screen.
It shows the installed version, last update result (this session's attempt, else the recorded apply), Stable/Beta channel, Check and install,
and Wi-Fi networks. Installing asks for confirmation before dropping the hotspot.

The updater and **A/V over SD** also open the same **SDLink firmware** screen. It
keeps the RNS-E module identity separate from the negotiated link protocol and
shows installed and bundled firmware for both the RNS-E and Teensy. A newer bundle
is an optional update offer; an older RNS-E may keep streaming through the
encoder's negotiated compatibility mode. carpid supplies the comparison (build
history, stream formats, firmware table); the screen never orders version strings.
**Check versions** probes both (read only); **Install** needs a second press and
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

## 6. Order

1. Captures (3) and the portal's day/zip view (5). This is what Drew asked for first.
2. UI rules (1) applied to the existing pages: Data, Light, Car, Diagnostics.
3. The slot editor (2) for the Data page, then DIS readings.
4. Data logging (4) and its portal retrieval.

## 7. Open

- Data page: keep the fixed groups (Engine/Drivetrain/Body/Electrical) as the starting pages, or start empty?
