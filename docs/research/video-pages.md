# Video (avsd/) and Pages/UI — research notes for the LIVI + Rust daemon rewrite

Repo: `C:\Users\raccoon\Documents\carstuff\RNS-E-Hudiy`, branch `testing` (read-only). All paths below are repo-relative unless absolute.
Memory notes read: `avsd-app.md`, `key-nav-bench-test.md`.

---

## A. Video: the A/V-over-SD chain

### A.1 The chain at a glance

```
Hudiy (Qt/Chromium, draws on labwc output NOOP-1, 800x480@60)
  -> wlcap      (C, own process; Wayland wire protocol by hand; zwlr_export_dmabuf by default)
  -> pipe: raw 800x480 bgr0 frames, no header
  -> rnv3enc    (C11 + NEON, 2 threads; RNV2/RNV3 codec for the RNS-E's SH-4A decoder)
  -> pipe: 4-byte BE length (top bit = key) + frame
  -> avlink     (C; packs 60 superframes/s with audio, USB CDC to Teensy, credits)
       ^ pcm.fifo <- pw-record (48 kHz s16le stereo, tap on echo_cancel_sink or hudiy_equalizer_sink)
       ^ key.fifo  (avlink -> encoder: "key", "cap <B/s>")
  -> Teensy 4.1 "SDemu_Touch" (USB CDC, emulates the SD card, 4-bit bus)
  -> RNS-E SD slot, sdstream firmware (decodes video, plays audio through the DSP)
```
README summary: `avsd/README.md:5-7`. Launch line (whole pipeline in one shell line): `avsd/chain/hud2.sh:49`.

### A.2 Stages: language, process, rate, resolution

| Stage | Source | Lang / process | Notes |
|---|---|---|---|
| Compositor | (system) labwc, headless output `NOOP-1` | wlroots | Exists only because no HDMI is attached. `hud2.sh:16` forces `wlr-randr --output NOOP-1 --custom-mode 800x480@60Hz`. `avsd_run.sh:25,32` waits for NOOP-1. HDMI present = warning, capture takes the first wl_output (`avsd_run.sh:56-57`). Installer sets `XCURSOR_THEME=invisible` and disables wayvnc (`avsd/install.sh:70-80`). |
| Capture | `avsd/chain/wlcap/wlcap.c` | C, single binary, 2 threads | Speaks Wayland wire protocol itself (no libwayland). Default `--dmabuf` = `zwlr_export_dmabuf_manager_v1`: takes labwc's finished linear XRGB8888 buffer, copies it after `DMA_BUF_IOCTL_SYNC` (`wlcap.c:6-11`). Alt `CAPMODE=shm` = `zwlr_screencopy_manager_v1` copy_with_damage, rgb0. Drops frames identical to the last one; newest-wins buffer; writer thread emits at most `--fps` (60, ×0.99 gap) and repeats a still screen 10×/s so the encoder keeps running (`wlcap.c:12-17`, writer `wlcap.c:250-311`). Flips y-inverted buffers. Stats line every 5 s to `cap.log`. |
| Encoder | `avsd/chain/enc3/src/{main.c,enc.c,kern_neon.c,...}`, built as `rnv3enc` | C11, `-O3 -mcpu=cortex-a76` (`install.sh:45-46`), `--threads 2` | Fixed 800x480 (`enc3/include/rnv2enc.h:12-13`). Reads exactly `stride*480` bytes per frame from stdin (`main.c:350-382`). Rate control uses the wall time between frame arrivals (`--refill wall`, `main.c:383`). Polls `key.fifo` each frame for `key` / `cap <B/s>` lines (`main.c:204-226, 388`). Writes 4-byte BE length (bit 31 = key) + frame (`main.c:421`). Hard caps: `CYC` decode cycles per frame for the SH-4A (default 4.05e6), `LINKCAP` 4.5e6 B/s, `MAXVID` 161 sectors/superframe. Extra flags `--tonepal 3 --nurelax 0.5 --nurelaxthr 0.97` (`chain/avsd.env:16`). |
| Link | `avsd/chain/avlink/*.c` | C, `make` | One superframe per tick at 60 Hz, 800 audio frames each; REPEAT when no new frame; frames > maxvid go out as TLV 0x11 fragments (`avlink/README.txt:22-25`). 528-byte wire sectors with per-line CRC16, TLNK framing over USB CDC with STATUS credits (`avlink.c:1-12`). Control FIFO `/tmp/avlink_sdctl`: `IMG1` (bootstrap image), `STOP` (sdstream exits), `WSF1/WSF0` (`avlink/sdctl.h:1-6`). STAT line every 5 s to `live.log`. |
| Teensy | `avsd/teensy/firmware.hex`, flashed by `teensy/hkflash_linux.py` (HalfKay over hidraw, sudo) | Teensy 4.1 firmware (source not in this repo) | USB id `usb-Teensyduino_SDemu_Touch_*-if00` (`avsd_run.sh:27`). Firmware "sdemu 0.3". |
| RNS-E | sdstream firmware sectors (decoder in sector 96), flashed from the Pi over SD + CAN | SH-4A | Decoder before CRC `0696DDE4` rejects R3 stream (`avsd/README.md:52`). |

**Frame rate:** up to 60 fps captured/encoded; 60 superframes/s on the link. Still screen: 10 fps refresh into encoder.
**Resolution:** 800x480 everywhere in code (wlcap/output mode, encoder constants). The memory note says "The RNS-E screen is 800x400" — conflict, see questions.

### A.3 Latency (what the code bounds; no measured glass-to-glass number found)

From `avlink.c:30-62` and `avlink/README.txt:9-16`:
- Encoder pipe 16 KB, at most 2 frames in avlink's buffer, then the encoder blocks and wlcap's newest-wins drops (so no queue builds before encode).
- Teensy window: at most 8 superframes (133 ms) ahead of the reader (`RD_WINDOW`).
- Backlog > 7 frames for 300 ms: discarded, then a key is requested. Reader silent 400 ms: frames discarded on arrival, audio-only superframe every 200 ms.
- AV sync: `video_pts = audio_pts + 2400` (50 ms at 48 kHz) (`avlink.c:31, 286`).
- Encode time: mean under 13 ms/frame on the Pi with nurelax (commit eea4a38).
- Rough worst case by construction: ~17 ms capture + ~13 ms encode + 1-2 frames pipe + up to 133 ms on the Teensy + decode. Typical probably ~50-100 ms. **Not measured** — ask Drew.

Reader overload: RNS-E reader tops out near 4.3 MB/s under heavy decode; avlink backs off the encoder cap live (×0.85 per 100 ms, floor 0.55 of cap) and sends early "catch-up" ticks for audio (`avlink.c:47-62`, commit 098be3a).

### A.4 CPU cost

- Encoder: 2 threads, mean encode < 13 ms/frame (≈ 0.8 core at 60 fps if every frame is new; still screens cost little because wlcap drops identical frames). `ENCTHREADS=3` "gains little and takes CPU from Hudiy" (`avsd/app/settings_store.py` FIELDS).
- `avsd-governor.service` pins all cores to `performance` (`chain/avsd-governor.service`).
- RT priorities (`hud2.sh:19-39`, `RTPRIO=5` default): wlcap FIFO 4, encoder 5, avlink 6, pw-record 7, pipewire-pulse 7, PipeWire `data-loop.0` 20; Hudiy reniced -10. Needs passwordless `sudo -n chrt`.
- No total CPU % figure in the repo. Pi 5 2 GB, so RAM headroom matters for a Chromium-based page renderer too.

### A.5 Audio path — yes, the chain carries audio

- Capture: `pw-record --target <sink> -P '{stream.capture.sink=true}' --rate 48000 --channels 2 --format s16 --latency 20ms --raw - > pcm.fifo` (`hud2.sh:43-48`). Sink is `echo_cancel_sink` if present (follows Hudiy volume + EQ), else `hudiy_equalizer_sink`. `AUDIOCAP=parec` reverts to parec (lost buffers).
- avlink: s16 LE -> s16 BE, 800 frames per tick, gain `--gain-db` (GAIN default -3, cap -3) (`avlink.c:660`, `avlink/audio.c`). Up to 250 ms source backlog kept (commit a789766).
- Clock: `avsd_run.sh:36-50` creates `avsd_clock`, a null sink driven by the system clock with `priority.driver=50000`, and moves `echo-cancel-playback` onto it, so a USB mic can't become the graph clock (commit d5383be: a dome mic ran 0.72% slow -> padded silence/crackles).
- So analog audio out of the Pi is no longer needed in this mode; the RNS-E DSP plays the SD stream audio. Phone-call mic path is separate (USB mic + echo canceller, Hudiy's).
- Hudiy-specific names baked in: `hudiy_equalizer_sink`, `echo_cancel_sink`, `pgrep -x hudiy`. A LIVI rewrite must provide an equivalent named sink to tap.

### A.6 Start/stop with power, supervision and failure handling

- Units (systemd --user, linger enabled): `avsd-chain.service` (Restart=always, RestartSec=2, StartLimitIntervalSec=0, `ConditionPathExists=!~/avsd/car/DISABLE`), `avsd-app.service` (Flask, port 5005), plus system `avsd-governor.service` (`avsd/chain/*.service`, `install.sh:83-92`).
- Power: chain starts at boot with the user session and dies with the Pi; no ignition logic of its own. Pi power-down is handled elsewhere (`rns-e_can/can_base_function.py` auto-shutdown / GPIO).
- Supervisor `avsd_run.sh`: waits forever (one log line per 30 s) for wayland socket, pulse, NOOP-1, `hudiy` process (or the updating marker), the Hudiy sink, `avsd_clock`, Teensy on USB, port free (`avsd_run.sh:30-52`). Then runs `hud2.sh` and checks every 2 s: hud2 alive, wlcap/encoder/avlink pids alive, pw-record alive, hudiy alive, `live.log` updated within 20 s. Any failure -> exit 1 -> systemd kills the cgroup and restarts (`avsd_run.sh:74-91`).
- Logs: live in `$XDG_RUNTIME_DIR/avsd` (tmpfs), copied by `tail -F` into `~/avsd/logs/<stamp>-{live,enc,cap}.log`, newest 40 runs kept (`avsd_run.sh:62-67`).
- Status logic for the app: `chain.py:66-82` `stream_health` compares the last two STAT lines: `frames` rising = Pi side OK; `served` rising and not `rd_stalled` = streaming; else `not_on_tv`.
- Stop with persistence: app's Stop touches `~/avsd/car/DISABLE` (`chain.py:96-100`).
- Update safety: after an update, `Releases.watch_step` rolls back to `~/avsd/previous` if the new chain doesn't reach streaming/not_on_tv within 180 s or restarts 3+ times (`avsd/app/operations.py:222-249`). Releases are versioned dirs with `current`/`previous` symlinks (`install.sh:57-69`).
- Update flow: `update_avsd.sh` touches `$XDG_RUNTIME_DIR/avsd-updating` (so missing Hudiy isn't fatal), kills Hudiy to drop the hotspot, waits for GitHub, sparse-clones, `install.sh --update`, reboots. Its terminal is `foot --fullscreen` on wayland-0 (`operations.py:251-257`) — shown on the RNS-E *through the chain*. So the chain must keep running without Hudiy, and some compositor must still be on NOOP-1.

### A.7 The boot checksum guard (RNS-E flashing)

- `Flasher.check_boot_checksums` (`avsd/app/rnse_flash.py:218-249`): the boot ROM sums each module listed in the descriptor at phys 0x400000 and compares with the module header's stored sum, which may sit in another sector (OSM spans sectors 32-53, sum only in 32). Before staging, for every module the target sectors touch it computes `stored + sum(new words) - sum(old words)` and refuses if it ≠ the new stored value. Refuses descriptor changes and module size changes outright.
- Old sector bytes: a file in `~/rnsefw` or `~/avsd/sector_cache` with the same CRC as the live sector, else read over CAN (~6 min/sector) (`_old_sector`, `rnse_flash.py:191`).
- Called by both `rehearse` and `flash` (`rnse_flash.py:455-480`). Proven on r61 candidate sectors (96 alone, 44+53 pass; 44 or 53 alone refused); **not yet run against a live unit** (commit 6bf226b, memory note).
- Flash flow (`operations.py:69-115`): CAN not listen-only (ignition on) -> firmware identity bytes -> auto-start off -> `STOP` to sdstream via avlink FIFO -> stop chain -> wait dormant -> checksum guard -> stage over SD -> verify staged CRC -> commit -> wait reset (restart count must be 0) -> verify live CRC -> restart chain. Uses `can0` IDs 0x7B0/0x7B6 (`can_link.py:15-16`).
- Ops refuse while a job runs; one job at a time (`avsd/app/jobs.py`).

### A.8 avsd app (port 5005) API — this is also a "page"

`avsd/app/app.py`. GET open to the network; any non-GET needs loopback + header `X-AVSD: 1` (`app.py:64-71`).

| Method/path | Purpose |
|---|---|
| GET `/api/status` | chain (active, disabled, supervisor state, verdict, last STAT, encoder_over), can state, teensy (repo/flashed hex sha, present), releases, decoder CRC, portal URLs, `can_change` |
| GET `/api/job` | current/last long job with log lines (UI polls 1 s) |
| GET/POST `/api/settings`, POST `/api/settings/save`, `/api/settings/defaults` | Apply (tmpfs trial env + restart), Save (`~/avsd/avsd.env`), Restore defaults. 11 fields (`settings_store.py` FIELDS) |
| POST `/api/chain` `{action: start|stop|restart}` | |
| GET `/api/logs?source=live|enc|cap|supervisor&lines=N` | UI polls 3 s |
| GET `/api/firmware` | files in `~/rnsefw` |
| POST `/api/rnse` `{op: check|rehearse|flash|decoder, files}` | job |
| POST `/api/unit/counters` | sdstream counters over CAN |
| POST `/api/teensy/flash`, `/api/update`, `/api/rollback` | |

UI source `hudiy_dataview/src/avsd/AvsdApp.tsx` (tabs Status/Settings/Logs/Update, polls 3 s status / 1 s job), built to `avsd/app/static/avsd.js` by CI. Hudiy menu entry `avsd_settings` -> `http://localhost:5005` (`install.sh:94-139`).

### A.9 What a LIVI fork could use to hand frames straight to the encoder

**Encoder input contract (CLI, today):**
- stdin, back-to-back raw frames, no header, exactly `stride * 480` bytes each; `--fmt bgr0` (B,G,R,X = DRM XRGB8888 little-endian) or `rgb0`/`rgb24`/`bgr24`; `--stride N` optional (default width*bpp = 3200 for 4-byte formats) (`enc3/src/main.c:1-14, 350-382`, `rnv2enc.h:15-20`).
- Arrival timing matters: `--refill wall` uses the time between reads as the link-bucket drain. Feed at most 60/s, and re-send the last frame ~10/s when still (wlcap behaviour) or the encoder stalls its refinement.
- Optional damage hint: `--damage FILE` one line per frame `x,y,w,h x,y,w,h ...` (empty = nothing changed, `*` = unknown) (`main.c:13`).
- Control in: `--linkctl FIFO` lines `key` / `cap <B/s>` (avlink writes these).
- Output: `[u32 BE len | 0x80000000 if key][frame]` to avlink.

**Library contract** (`enc3/include/rnv2enc.h:95-114`): `rnv2enc_create(params)`, `rnv2enc_encode(e, pix, stride, fmt, dmg_rects, ndmg, &out, &outlen, &info)`, `rnv2enc_set_elapsed(seconds)`, `rnv2enc_set_linkcap(B/s)` (thread-safe), `rnv2enc_force_key()`. Pure C, no deps beyond pthread/libm; could be linked into a Rust process via FFI.

**Options for LIVI:**
1. Zero change: run LIVI nested in labwc on NOOP-1 (or any wlroots compositor) and keep wlcap. Works with any client (pages, foot update terminal, test pattern).
2. LIVI's own smithay compositor renders into an 800x480 offscreen buffer (GBM/dmabuf or shm) and writes XRGB8888 frames (plus damage rects, which smithay knows exactly) into a pipe/memfd for `rnv3enc` — replaces wlcap and labwc. Must pace ≤60/s and repeat on still.
3. Link `rnv2enc` in-process (FFI) — fewest copies, but couples LIVI to the encoder's RT priority and crash domain; today the chain is isolated processes restarted by a supervisor.
4. Implement `zwlr_export_dmabuf_manager_v1` (or `ext-image-copy-capture`) in LIVI's smithay compositor so wlcap works unchanged.

**What else must share the output** (everything that is "on the RNS-E screen" goes through this one 800x480 surface):
- DataView pages (5003), RNS-E Manager (5004), A/V over SD app (5005) — today Chromium web views inside Hudiy.
- `foot --fullscreen` update terminals (`update_avsd.sh`, `update_rnse.sh`) while Hudiy is quit.
- ADC test pattern window (Hudiy `web_viewer --width 800 --height 480`, `hudiy_manager/test_pattern.py:42-63`) — only relevant to the analog path.
- Pi colour (gamma) control `hudiy_manager/pi_wayland_color.py` uses `wlr-gamma-control-v1` on the output; unknown whether it affects what wlcap exports on a headless output.
- Reverse camera? Not in this repo's chain; the analog ADC is "shared with the reverse camera" (`hudiy_manager/ADC_TUNING.md`).

### A.10 Untested / open (from code + memory)
- Flashing the RNS-E from the Pi: only against a simulated unit (`avsd/README.md:74-77`, `app.py` commit b3d800d).
- Boot checksum guard on a live unit (6bf226b).
- Update terminal on the RNS-E screen with Hudiy quit; post-update rollback (memory note).
- `avlink/README.txt:5` still says "host-tested only" — stale; memory says the chain runs on the car Pi since 2026-11-06.
- Repo encoder matches the deployed binary, not the newer encoder in `RNSE hacking` (memory note).
- Uplink from Teensy (touch? `SDemu_Touch`; `TL_UPLINK` ignored, `avlink/link.c:152`) — nothing consumes RNS-E -> Pi data over USB yet. Rate control from RNS-E STATUS not done (`avlink/README.txt:47-49`).
- Teensy firmware source not in this repo.

---

## B. Pages / UI

### B.1 How pages are shown today

- Hudiy "applications" are URLs it opens in its embedded Chromium web view, full app area: `config/hudiy/applications.json` -> `hudiy_manager` = `http://localhost:5004`, `hudiy_diagnostics` = `http://localhost:5003`; avsd adds `avsd_settings` = `http://localhost:5005` (+ `?update=1` standalone). Menu category "RNS-E" in `applications_menu.json`. Not separate windows; Hudiy's bar stays around them. Surface is the 800x480 output (minus Hudiy's chrome).
- Hudiy injects `window.hudiy`: `colorScheme` (Material palette, `darkThemeEnabled`), `onColorSchemeChanged`, `onAttached`, and input callbacks (`references/hudiy references/hudiy-main/README.md:520-690`). Pages theme themselves from it (`src/hooks/useHudiyTheme.ts`); DataView forwards the palette to the server (`log_theme`, `app.py:583`) so the phone portal can use the same colours (`file_portal.py:306 cache_hudiy_theme`, GET `/api/files/theme`).
- Touch: the head unit's touch arrives as mouse pointers; `managerPointerScroll.ts` turns mouse drags into scrolling. (Via SD there's no touch today.)
- Dashboard widgets: `config/hudiy/dashboards.json` uses only native widgets. `car_widget/car_widget.html` (car model/colour picker, localStorage, `file:///home/pi/.hudiy/...` fonts) and `boost_widget.html/js` (socket.io to `http://localhost:5003`, raw `toggle_group` subscriptions on module 0 groups 0-2 + 1:113 + configurable channels, `boost_widget.js:~720-1080`) are hand-loaded Hudiy web widgets; nothing installs them.

### B.2 Knob / button navigation (key-nav work)

- Physical buttons: RNS-E faceplate/knob CAN frames -> `rns-e_can/can_keyboard_control.py` -> uinput keyboard (and Hudiy API actions over `hudiy_data_api`). TV-panel shortcuts (r21 firmware, ID 0x461): INFO -> DataView, CAR -> Manager, NAME -> app menu, etc. (`references/TV_BUTTONS_HUDIY_MAPPING.md`).
- Hudiy translates keys into web-view callbacks; `hudiy_dataview/src/hudiyKeyNav.ts` implements the highlight: knob = `onMoveToNext/PreviousControl` (move highlight; scrolls `data-key-scroll` panes first), press = `onTriggered` (click; ranges/selects enter an edit mode where the knob changes the value, press confirms, RETURN restores), RETURN = `onGoBack` (`data-key-back` control or close dialog), long-press track buttons = `onGoLeft/Right` -> switch tab (`hudiyKeyNav.ts:1-24, 307-312`). Highlight is an attribute (`data-key-focus`), not DOM focus, so text fields don't pop the keyboard.
- Up/Down: Hudiy keeps them for its own areas and gives web views nothing; Down leaves the app. Workaround: `can_keyboard_control` remaps short Up/Down to the knob keys while `HUDIY_UI` (published every 2 s by `hudiy_client/hudiy_data.py`) says `hudiy_manager`/`hudiy_diagnostics` is on screen (TV_BUTTONS_HUDIY_MAPPING.md "Up and Down"). Note avsd's `avsd_settings` is not in that list.
- Used by all three apps (`App.tsx:58-63`, `ManagementApp.tsx`, `AvsdApp.tsx:33-38`).
- Temporary probe still in: `probe()` `hudiyKeyNav.ts:116-119, 321-330` + POST `/api/manage/key-nav-probe` (`hudiy_manager/routes.py:36-43`, registered on DataView's 0.0.0.0:5003 too). To remove after the next bench test (memory note). Bench tests still pending: Up/Down, knob past page end, flasher dialog, holding prev/next.
- Drew's UI rule (memory): everything must work by knob and buttons; rows show friendly name + variable, e.g. "Decode limit (CYC)".

### B.3 Processes and ports

| App | Process | Bind | Auth for writes |
|---|---|---|---|
| DataView + file portal | `hudiy_dataview/app.py`, Flask-SocketIO threading, `hudiy_dataview.service` | 0.0.0.0:5003 (`app.py:1465`) | socket.io events: none. Portal upload: optional `file_portal.upload_pin` (`X-Hudiy-Pin`, `file_portal.py:578-590`). Config routes: `X-Hudiy-Management: 1` + same-origin + optional PIN (`hudiy_manager/routes.py:12-28`) |
| RNS-E Manager | `hudiy_manager/app.py`, Flask, `hudiy_manager.service` | 127.0.0.1:5004 (`app.py:93`) | same `require_mutation_access` |
| A/V over SD | `avsd/app/app.py`, `avsd-app.service` | 0.0.0.0:5005 | loopback + `X-AVSD: 1` |
| Vehicle values broker | `vehicle_data/service.py` (via `hudiy_dataview/can_service.py`, `hudiy_status_service.service`) | ZMQ ipc | — |

Backend fan-in for DataView (ZMQ ipc, `app.py:37-70`): `tp2_stream` (PUB, topic HUDIY_DIAG: raw groups, dtc_report, install_list), `tp2_command` (REQ: SYNC/READ_DTC/CLEAR_DTC/READ_INSTALL_LIST), `status_stream`, `haldex_cmd`/`haldex_status` (Haldex manager, also exhaust valve), `vehicle_data_stream`/`vehicle_data_command` (named values).

### B.4 Every page / tab and what it needs

**DataView (5003, action `hudiy_diagnostics`, INFO button)** — tabs in `src/App.tsx:16-22`, all kept mounted, swipe + long-press to switch; global "~" smoothing toggle (server-wide flag, `app.py:541-554`).

1. **Engine** (`src/tabs/EngineTab.tsx`): live named values `ENGINE_VALUES` (15 ids: rpm, maf, ignition timing, boost actual/spec, rail actual/spec, pump duty, injection time, oil/ambient/intake/coolant temp, timing retard cyl 1-4) (`hooks/useSocket.ts:34-41`). Gauges, knock bars.
2. **Transmission** (`TransmissionTab.tsx`): `TRANSMISSION_VALUES` (clutch 1/2 shaft speed, spec torque, valve current, pressure; selector travel ×4; fluid/module/clutch oil temps; idle status) (`useSocket.ts:43-49`).
3. **AWD** (`AWDTab.tsx`): `AWD_VALUES` (13 ids incl. estimated torque) + **Haldex mode** select (Stock/Perf/Comp/Snow/Full, `AWDTab.tsx:19-23`): socket `get_haldex_status` (UI polls 1 s), `set_haldex_mode {mode}`, `cycle_haldex_mode`; pushed `haldex_update` from `haldex_status` SUB (`app.py:1411-1442`), status shape `types.ts:46-67`. **Drive Logger** (legacy CSV logger, `data_logger.py`): `get_logger_status`, `start_logger {profile, output}`, `stop_logger`, `add_logger_marker {note}`, events `logger_update`, `logger_error`, `logger_marker_added`; logger gets its own TP2 group subscriptions (`app.py:420-437`).
4. **Data & Logs** (`DataLogsTab.tsx`, `hooks/useDataLogs.ts`, server `data_logs.py`): sections Record / Review / DIS pages.
   - REST `/api/data-logs` (state: workspace config + full value catalog + sessions + recording), `GET|PUT /api/data-logs/config` (profiles and **DIS page layouts** for the cluster), `GET /status`, `POST /start {profile_id}`, `/stop`, `/marker {note}`, `GET /sessions`, `GET|DELETE /sessions/<id>` (paged samples: limit/offset/window_sec/since/until), `GET /sessions/<id>/export` (CSV stream) (`data_logs.py:523-566`). Push events `data_logs_state`, `data_logs_status`, `data_logs_config`.
   - Live values for the chosen profile via `sync_values` (ValueRequest may carry `rate_hz`/`period_ms`/`source`, `components/dataLogs/model.ts:3`).
   - Recording subscribes through the broker as `dataview_recording`, stores NDJSON samples with acquisition metadata (no resampling).
5. **Diagnostics** (`DiagnosticsTab.tsx`, opens its own socket via `useSocket('diagnostics')` at line 65):
   - Module list from gateway install list (`request_install_list {refresh}` -> `install_list`), default list fallback (`DiagnosticsTab.tsx:30-60`).
   - DTCs: `request_dtcs {module}`, `clear_dtcs {module}` -> `dtc_report` (codes, status bits decoded, freeze frame) (`app.py:556-581`).
   - Raw measuring groups: three per module, keypad entry, `toggle_group {module, group, action add|remove, priority}` -> `diagnostic_batch`/`diagnostic_update` `{module, group, data:[{value, unit}]}` (`app.py:502-539`).
   - **Flash Controller** modal (`components/HaldexFlashModal.tsx`): Haldex gen4 / PQ EPS. `get_tunes_list {module}` -> `tunes_list`; `get_ecu_flash_info` -> `ecu_flash_info`; `start_haldex_flash {module, artifact_id, dry_run, start_addr, end_addr}`; `start_haldex_readout {module, readout_kind eeprom|firmware, ...}`; `cancel_haldex_flash`; progress/started/complete/error/aborted events; recovery marker `~/.hudiy/haldex_recovery_required.json`; GET `/haldex/readouts/<id>/image|report` downloads (`app.py:938-1409`). Pauses other CAN traffic (`flasher.traffic.flashing_operation`).
   - **Exhaust valve controller** panel (`components/ExhaustValvePanel.tsx`, `exhaust_valve.py`, `EXHAUST_VALVE.md`): `get_exhaust_valve` -> `exhaust_valve_status` + `exhaust_valve_artifacts`; `exhaust_valve_command {target 0-100 | toggle}` (via Haldex manager REQ, CAN 0x67A, unconfirmed — no reply ID); `start_exhaust_valve_update {artifact_id, dry_run}`, `cancel_exhaust_valve_update`; progress events (`app.py:649-806`).

**File portal (5003 `/files`, phone-facing)** — `src/FilePortal.tsx`, `tabs/FilesTab.tsx`, `file_portal.py`.
- Collections (`build_collections`, `file_portal.py:352-479`): firmware targets haldex (`~/haldexfw`), pq-eps, exhaust-valve (`~/exhaustfw`, ZIP bundle -> versioned dir), rnse (`~/rnsefw`, only if `~/avsd/current` exists, 128 KiB sector validator); logs: avsd_logs, hudiy_api captures, drive_logs (`~/logs` CSV), service_logs, runtime_logs (`/var/log/rnse_control`), flash_logs, Haldex/EPS readouts.
- API: GET `/api/files` (catalog), GET `/api/files/download/<collection>/<path>`, POST `/api/files/archive/<collection>[/<path>]` -> job, GET `/api/files/archive-jobs/<id>` (+`/download`), POST `/api/files/upload/<collection>` (multipart `file`, XHR progress, 16 MB default per target, 409 on existing name; no delete route), GET `/api/files/theme`.
- **Configuration** category: upload a whole JSON to replace `config.json` or Hudiy configs via `/api/manage/configs/<id>/import` (`src/ConfigUploadPanel.tsx`, `routes.py:65-80`), with backup + revision check.
- How the phone gets there: Pi runs NetworkManager hotspot "Hudiy AccessPoint" (10.42.0.1); phone joins it (or both on the same Wi-Fi) and opens `http://<Pi address>:5003/files`. Manager's top bar shows the address (`hudiy_manager/network_info.py:24-44`, hotspot first, refreshed every 10 s; `ManagementApp.tsx` `manager-files-url`). avsd app also lists portal URLs (`avsd/app/app.py:33-43`). Updates drop the hotspot (`nmcli connection down "Hudiy AccessPoint"`, `update_avsd.sh:26`) — the hotspot belongs to Hudiy today, so LIVI/daemon must own it.

**RNS-E Manager (5004 loopback, action `hudiy_manager`, CAR button)** — `src/ManagementApp.tsx`, tabs `settings | video | services | logs` (line 18).
- **Settings**: config targets `rnse` (`config.json`, affects 12 services) and five Hudiy JSONs (`hudiy_manager/config_store.py:189-202`). GET `/api/manage/configs`, GET/PUT `/api/manage/configs/<id>` (`{document, revision}`, stale revision rejected, backup to `~/confbackup/DATE/N/`), metadata GET `/api/manage/metadata` (`static/configMetadata.json`, generated by `tools/export_config_metadata.cjs`), search, on-screen keyboard (`components/touchKeyboard`).
- **RNS-E live controls** (in Settings -> RNS-E): GET `/api/manage/rnse-control`, POST `/manual` (dial 0-10, LCD 0-100, source mode), POST `/reload`; via ZMQ REQ to `rnse_control_command` (ipc only) in `can_base_function` (`rnse_bridge.py`, `routes.py:160-190`). CAN 0x7B0 `B7/BB/BA` (manager README "RNS-E bridge controls").
- **Video -> RNS-E ADC** (`RnseAdcPanel.tsx`, `rnse_adc_protocol.py`, `ADC_TUNING.md`): GET `/api/manage/rnse-adc`, POST `/write {values}` (register->byte, server-enforced encodings), `/dump`, `/revert`, `/resend-clock`, GET/POST `/presets`; test pattern window (`/api/.../test-pattern` open/close/status, `test_pattern.py:82-110`). Analog-path only; obsolete if video goes over SD.
- **Video -> Pi colors** (`VideoPanel.tsx`, `video_control.py`, `pi_wayland_color.py`): GET `/api/manage/video`, POST `/apply {settings, output}`, `/profile` (save to `~/.hudiy/share/video-color.json`), `/reset`. Gamma/contrast/black point/RGB gains via `wlr-gamma-control-v1`.
- **Services**: GET `/api/manage/services`, POST `/api/manage/services/<id>/<start|stop|restart>` (sudo allowlist of 12 units, `service_control.py:40-54`), refused during active controller flashes.
- **Logs**: GET `/api/manage/services/<id>/logs?lines=50..500` (journal tail, 3 s follow).
- GET `/api/manage/network`, GET `/api/manage/theme`.

**A/V over SD (5005)** — see A.8.

**car_widget** — see B.1; uses legacy raw groups (module 0 = `status_stream` compatibility groups).

### B.5 Live value delivery today (rates)

- Named values (current path): browser `sync_values {values:[id | {id, rate_hz|period_ms, source auto|diag|ican|<provider>, allow_estimated, allow_unverified}]}` replaces the client's whole set; renewed every 5 s by browser and by `value_bridge` heartbeat; lease 15 s (`value_bridge.py:41-113`, `vehicle_data/broker.py:47-98`, `vehicle_data/README.md`). Server immediately emits a `SNAPSHOT` as `values_batch`, then pushes `values_batch` per publication (`value_bridge.py:115-133`). Defaults: diag 2 Hz per value, ICAN capped 10 Hz. Max 512 values per client.
- Value object (`src/types.ts:14-27`): `{version, id, value, unit, type, status ok|stale|invalid|unavailable|paused, timestamp, age_ms, max_age_ms, sample_sequence, source{id,kind}, quality{valid,fresh,verified,estimated,reason}}`. Client expires values locally by `age_ms + elapsed > max_age_ms` (`store/DataStore.ts:12-50`).
- Raw TP2 groups (Diagnostics, widgets, legacy logger): `toggle_group` add/remove with priority; server re-SYNCs every 5 s per client id `dataview:<sid>`; server-side interpolation + EMA re-emits `diagnostic_batch` at 4 Hz when smoothing on (`app.py:133-262`), raw `diagnostic_update` when off. Source rate ~0.5-2 Hz.
- Status-style things are polled: Haldex 1 s, logger 1 s while recording, avsd status 3 s / job 1 s, manager logs 3 s, network 10 s.

### B.6 What a local page API must provide (shape proposal derived from the above)

Transport: one WebSocket per page (JSON messages) + plain REST for documents and files. Socket.IO isn't required; current events map 1:1.

1. **Values subscription** (WS): `subscribe {values:[{id, rate_hz?, source?, allow_estimated?, allow_unverified?}]}` replaces the set; server replies with snapshot then pushes `values [{...VehicleValue}]` batched per tick (≤10 Hz per value, coalesced per frame ~20-30 Hz max). Lease/renew or tie to socket lifetime. `GET /api/values/catalog`, `/status`.
2. **Raw measuring groups** (WS): `group.add/remove {module, group, priority}` -> `group {module, group, data:[{value, unit}], t}`; optional server smoothing per client (not global).
3. **Diagnostics actions** (WS or REST + events): install list (cached per ignition, refresh), read DTCs, clear DTCs -> report with freeze frames; long ops as jobs.
4. **Jobs** (generic, one-at-a-time per resource): start -> `{job_id}`; events `progress {stage, percent, detail, speed, eta}`, `done|error|aborted`; cancel. Needed for Haldex/EPS flash + readout, exhaust valve OTA, RNS-E sector check/rehearse/flash/decoder, Teensy flash, archive builds. Persist last job (avsd writes `last_job.json`) because the screen may be down during it.
5. **Haldex / exhaust**: get status, set mode / cycle, push status changes; valve target / toggle (unconfirmed by design).
6. **Recording**: profiles CRUD (workspace doc incl. DIS pages), start/stop/marker, sessions list/read (paged, time window)/delete/export CSV; push recording status ~1 Hz.
7. **Config**: list targets, read `{document, revision}`, write with revision (409 on stale), metadata/schema, import file; backups.
8. **Services**: list with state, start/stop/restart (allowlist), journal tail.
9. **RNS-E head unit**: brightness/dial/source live + reload; (ADC only if analog stays).
10. **A/V chain**: status (supervisor state, verdict, STAT), settings apply(temp)/save/defaults with field schema, logs by source, start/stop/restart.
11. **Files** (network-facing): collections catalog, download, archive job, upload with validator per target, optional PIN.
12. **Theme**: current palette + dark flag pushed on change (today from Hudiy; with LIVI the daemon/LIVI must own the palette and day/night).
13. **Network**: addresses (hotspot first) for the portal URL.
14. **Input**: an equivalent of `window.hudiy` key callbacks (next/prev/trigger/back/left/right, activated/inputFocus) delivered to the page, plus Up/Down that the app actually receives.

Security split that exists today and should be kept: phone (non-loopback) can read status, download, upload firmware/config (PIN optional); only the car (loopback) can flash, change settings, control services.

### B.7 Pages-related loose ends
- Smoothing toggle is global across all DataView clients (`app.py:541`).
- DataView config routes (incl. key-nav probe) are on the network-facing port 5003 with header check only (+ optional PIN).
- `can_service.py` is just a shim to `vehicle_data.service` (`hudiy_dataview/can_service.py`).
- Local `node_modules` drifted from the lockfile (memory key-nav note); CI builds bundles (`static/js/*.js`, `avsd/app/static/avsd.js`).
