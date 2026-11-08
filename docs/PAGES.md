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
  - Text entry for custom page names and DIS labels.
  - Later: flashing jobs, which log into a # like everything else.
- Bug report stays: today's tar.gz, rebuilt as "zip of the latest capture plus status".

## 6. Order

1. Captures (3) and the portal's day/zip view (5). This is what Drew asked for first.
2. UI rules (1) applied to the existing pages: Data, Light, Car, Diagnostics.
3. The slot editor (2) for the Data page, then DIS readings.
4. Data logging (4) and its portal retrieval.

## 7. Open

- Data page: keep the fixed groups (Engine/Drivetrain/Body/Electrical) as the starting pages, or start empty?
