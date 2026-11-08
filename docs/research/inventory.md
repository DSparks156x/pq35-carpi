# RNS-E-Hudiy feature inventory (branch `testing`, 2026-11-07)

Reference for the LIVI + Rust daemon rewrite. Checked against code, not READMEs.
Tags: **[H]** Hudiy-specific · **[C]** car-specific (TT 8J / PQ35 / RNS-E 193) · **[G]** generic.
Repo root: `C:\Users\raccoon\Documents\carstuff\RNS-E-Hudiy`. Runtime config: `/home/pi/config.json`.

---

## 1. Services and IPC

### systemd units (written by `install.sh` ~L634–983)
| Unit | Entry | Starts | Deps |
|---|---|---|---|
| can_handler | `rns-e_can/can_handler.py` | multi-user | BindsTo can0 device |
| can_base_function | `rns-e_can/can_base_function.py` | multi-user | Requires/BindsTo can_handler |
| can_keyboard_control | `rns-e_can/can_keyboard_control.py` | multi-user | Wants can_handler |
| dark_mode_api | `hudiy_client/dark_mode_api.py` | multi-user | Requires can_handler |
| hudiy_data_api | `hudiy_client/hudiy_data.py` | multi-user | Requires can_handler |
| dis_service | `dis_client/dis_service.py` | multi-user | BindsTo can_handler |
| dis_display | `dis_client/dis_display.py` | multi-user | BindsTo dis_service |
| dis_top_display | `dis_client/dis_top_display_service.py` | multi-user | BindsTo can_handler |
| tp2_worker | `tp2/tp2_worker.py` | rnse-late.target | |
| hudiy_status_service | `hudiy_dataview/can_service.py` → `vehicle_data.service` (name is a leftover) | rnse-late | |
| hudiy_dataview | `hudiy_dataview/app.py` (Flask-SocketIO :5003, 0.0.0.0) | rnse-late | |
| hudiy_manager | `hudiy_manager/app.py` (Flask :5004, 127.0.0.1) | rnse-late | |
| haldex_manager | `rns-e_can/haldex_manager.py` | rnse-late | Requires can_handler |
| rnse-late.target/.path/.timer | path waits for `/run/rnse_control/hudiy-started` (touched by hudiy_data on API connect and by `hudiy_startup_fast.sh`); timer fallback 25 s | | |
| avsd-chain (user), avsd-app (user, :5005), avsd-governor (system oneshot) | `avsd/` — opt-in, separate installer | | |

Other install.sh system changes: apt deps; uinput module + udev rule; downloads + compiles Hudiy API protobuf; adds mcp2515 overlay to config.txt (asks osc/int pin); systemd-networkd can0 config; tmpfs dirs `/var/log/rnse_control`, `/run/rnse_control`; firmware dirs `~/haldexfw/readouts ~/epsfw/readouts ~/exhaustfw`; sudoers `/etc/sudoers.d/rnse-manager` from `hudiy_manager/service_permissions.render_sudoers`; `~/.hudiy_version.json`; deep-merges repo `config.json` and `config/hudiy/*.json` into user copies with backups (special-cases shortcut/menu insertion); persistent sparse git cache `~/.cache/rns-e-hudiy-installer`; flasher submodule; re-runs `avsd/install.sh --update` if `~/avsd/current` exists. Commented-out composite-video step (RNS-E 192 480x234 / 193 800x480).

### ZMQ map (all `ipc:///run/rnse_control/*.ipc`)
| Endpoint (config key) | Bind | Pattern / topics | Consumers |
|---|---|---|---|
| can_stream (`can_raw_stream`) | can_handler | PUB `CAN_%03X` JSON frame | base_function, keyboard, dark_mode, haldex, dis_display, vehicle_data, data_logger |
| can_send (`send_address`) | can_handler | PULL `[dec id, hex]`; `BARRIER token endpoint` → PUSH ack to `haldex_barrier_<token>.ipc` | base_function, top_display, haldex |
| base_events (`system_events`) | can_base_function | PUB `POWER_STATUS` {kl15, kls, bus_active, radio_active, door_open, door_event, doors, wake_signal} 1 Hz + on change | connection_manager, dark_mode, dis_service, top_display, tp2_worker |
| rnse_control (`rnse_control_command`, not in config) | can_base_function | REP: status, manual, reload, adc_status/write/dump/revert/resend_clock | hudiy_manager |
| hudiy_stream (`metric_stream`) | hudiy_data | PUB HUDIY_MEDIA/NAV/NAV_STATUS/PHONE/COVERART/PROJECTION/UI | dis_display, top_display, keyboard, base_function (PROJECTION) |
| input_control_stream | can_keyboard_control | PUB WHEEL_CONTROL, DIS_INPUT, HUDIY_ACTION | dis_display, top_display, hudiy_data |
| dis_draw | dis_service | PULL JSON draw cmds | dis_display |
| dis_status | dis_service | PUB DIS_STATE / DIS_REQUESTED / DRAW_ACK / DRAW_NACK | dis_display |
| dis_display_status | dis_display | PUB DIS_DISPLAY_STATUS | top_display, keyboard |
| dis_top_status | top_display | PUB DIS_TOP_STATUS | keyboard |
| tp2_stream | tp2_worker | PUB HUDIY_TP2_STATUS, HUDIY_DIAG (groups, dtc_report, install_list), HUDIY_DIAG_OBSERVATION, HUDIY_OPENPILOT | vehicle_data, dataview, hudiy_data, dis_display |
| tp2_cmd | tp2_worker | REP QUIESCE/RELEASE/STATUS/SYNC/KWP_REQUEST/KWP_RESULT/READ_DTC/CLEAR_DTC/READ_INSTALL_LIST/CLEAR/TOGGLE/SET_ENABLED | haldex, dataview, vehicle_data, hudiy_data |
| haldex_cmd / haldex_status | haldex_manager | REP STATUS/SET_MODE/CYCLE/TOGGLE_VALVE/SET_VALVE/GET_VALVE_STATUS/INHIBIT/RESUME/QUIESCE/RELEASE; PUB HALDEX_STATUS 2 Hz | dataview, hudiy_data |
| vehicle_data_stream / vehicle_data_command / status_stream (code defaults, not in config) | vehicle_data | PUB HUDIY_VALUES; REP CATALOG/SYNC_VALUES/UNSUBSCRIBE/SNAPSHOT/PLAN/STATUS; PUB legacy HUDIY_DIAG (fake module 0) | dataview, dis_display, data_logs |
| data_logs_cmd | dataview (data_logs.py) | REP START/STOP/MARK/STATUS | dis_display readings app |

Other IPC: Hudiy API TCP 127.0.0.1:44405 (protobuf, `<III` header) — hudiy_data (DATA + TP2_BRIDGE clients), dark_mode_api (own socket), save_logs. Files: `/tmp/now_playing.json`, `/tmp/current_nav.json`, `/tmp/current_call.json` (hudiy_data → top_display/media seed); `~/.hudiy/flashing_mode.json`, `flashing_operation.lock`, `haldex_recovery_required.json` (flasher.traffic cross-process gate); `~/.tp2_settings.json`; `/home/pi/dis_settings.json`; `$XDG_RUNTIME_DIR/avsd-updating`.

### HTTP ports
- 5003 DataView + file portal + `/api/manage/configs` mirror (0.0.0.0, phone-reachable)
- 5004 RNS-E Manager (loopback only)
- 5005 avsd app (GET open on LAN, POST loopback + `X-AVSD: 1`)
- 5000 DIS emulator (dev), 8766 bench cluster console (dev), 5173/5174/5190 vite dev servers

---

## 2. CAN interface

Bus: single `can0` (MCP2515, 100 kbit ICAN) carries infotainment + diagnostics (`interfaces.can.infotainment/diagnostic`). can_handler is the sole owner for app traffic; **dis_service, tp2_worker, avsd can_link, exhaust updater and flasher open their own socketcan** sockets.

| ID | Dir | Use | Owner |
|---|---|---|---|
| 0x602 | TX 2 Hz | TV tuner presence (TV simulation) — keeps RNS-E TV source available; only TX allowed in Flashing Mode | can_base_function [C] |
| 0x623 | RX | time/date → `sudo date` | can_base_function [C] |
| 0x2C3 | RX | KL15 / KLS (key) | can_base_function [C] |
| 0x470 | RX | doors/hood/trunk | can_base_function [C] |
| 0x420–0x43F | RX | OSEK NM ring; 0x42B gateway, 0x436 RNS-E → radio_active, sleep detection | can_base_function [C] |
| 0x635 | RX | KL58 dimming → day/night, brightness | dark_mode_api, base_function [C] |
| 0x7B0 / 0x7B6 | TX/RX | **patched RNS-E firmware** command channel: B7 dial brightness, BB LCD override, BA source label, BC/BD ADC write/dump; avsd: B1 mem read, B9 RAM write, B8 task, B3/B6/B7 flash | base_function, manager, avsd [C] |
| 0x461 | RX | MMI buttons + r21 TV-panel buttons | can_keyboard_control [C] |
| 0x5C3 | RX | MFSW steering wheel buttons | can_keyboard_control [C] |
| 0x661 | RX | RNS-E active source (0x37 = TV) | keyboard (source_controls), top_display [C] |
| 0x363 / 0x365 | TX | DIS top 2-line radio text (AUDSCII), overwrites OEM | dis_top_display [C] |
| 0x6C0 / 0x6C1 | TX/RX | DDP to cluster (impersonates RNS-E nav channel) | dis_service [C] |
| 0x2C1 | RX | wiper-stalk up/down → DIS paging | dis_display [C] |
| 0x351 | RX | vehicle speed → accel test | dis_display [C] |
| 0x60E | RX | cluster unit settings | dis_display [C] |
| 0x6DA / 0x679 | RX | custom Haldex firmware telemetry | haldex_manager, vehicle_data, data_logger [C, custom FW] |
| 0x67A | TX | exhaust valve (RP2040 SB2209, `C3 3C` namespace) commands + OTA update; shared with Haldex FW | haldex_manager, exhaust_valve.py [C, custom HW] |
| 0x200+ / 0x300–0x307 | TX/RX | TP2.0 channel setup + tester IDs (0x307 reserved openpilot) | tp2_worker [C] |
| ICAN signals (0x359, 0x35B, 0x527, 0x351, 0x3C3, 0x2A1, 0x555, …) | RX | passive vehicle values (67 signals) | vehicle_data, data_logger [C] |
| 0x655 | RX | gateway install bitmap | tp2/module_survey (tool) |

TP2 logical module IDs in use: engine 0x01, DSG 0x02, EPS 0x09, Haldex 0x0A (VCDS 22), Comma 0x0C, gateway 0x1F, BCM 0x20.

---

## 3. Features by area

### 3.1 CAN core / car integration (`rns-e_can/`)
- **CAN broker** — can_handler fans out RX, serialises TX, enforces Flashing Mode TX block, BARRIER drain for exclusive diag ownership. [G]
- **TV simulation** — periodic 0x602 (`features.tv_simulation.payload`); required for the RNS-E to show the Pi on its TV input. [C]
- **Time sync** — 0x623 BCD/hex, `car_time_zone`, threshold. [C]
- **Power state publisher** — POWER_STATUS from 0x2C3/0x470/NM/GPIO. [C]
- **auto_shutdown** — on KL15 or key-pulled + delay; disabled in config. [C]
- **listen_only_mode** — puts can0 into listen-only when bus is going to sleep (NM SleepAck/SleepInd or 900 s fallback) so Pi TX doesn't keep the car awake; reverts on wake; inhibits all app TX. [C]
- **gpio_shutdown** — GPIO 12 active-low wake line (CarPiHat) → shutdown after 60 s; feeds wake_signal. [C/G]
- **RNS-E brightness/LCD/source label** — `rnse.*`: auto day/night dial brightness (B7), LCD override (BB), source label (BA: Hudiy/CarPlay/AA from projection source). Needs patched RNS-E FW. Live manual override from Manager. [C]
- **RNS-E video ADC tuning** — AK5406 register writes/dump/revert/resend-clock, presets in `~/.hudiy/rnse-adc-presets.json`, volatile. Needs patched FW (BC widening patch for extra regs). [C]
- **MMI key mapping** — 0x461 → uinput keys; short (on release) / long (5 frames) / extended (30 frames, may be shell command gated by `system_actions`); knob scroll; actions `{"action": name}` → Hudiy DispatchAction; cooldown; MFSW-duplicate suppression; Up/Down borrow knob keys while a Hudiy web app is foreground (uses HUDIY_UI). [C map, H actions]
- **r21 TV-panel buttons** — NAV/TEL/MEDIA/NAME/INFO/CAR, strict frame filter (`tv_panel_controls.py`). [C]
- **MFSW mapping** — 0x5C3 scroll/mode/click/PTT/volume; MODE double-click toggles wheel ownership normal↔DIS (`wheel_controls.py` state machine); auto phone takeover; heartbeat. [C]
- **Source controls** — 0x661 TV-source enter/exit → play/pause keys; disabled in config. [C]
- **Haldex mode control** — 5 modes (Stock/Perf/Comp/Snow/Full) via KWP `3B F0` to 0x0A through tp2_worker; confirm + 10 s reconcile (ECU latch is RAM-only); persisted; telemetry decode; exclusive QUIESCE for flashing; exhaust valve open/close + intent file. [C, custom Haldex FW]
- **Log saver** — journalctl of `features.log_saver.services` + Hudiy API event logs → `~/logs/YYYY-MM-DD/n/`, Hudiy toast. [G+H]

### 3.2 Hudiy integration (`hudiy_client/`, `config/hudiy/`) — all [H]
- **Hudiy data bridge** (hudiy_data) — subscribes to Hudiy PROJECTION/MEDIA/NAVIGATION/PHONE/CURRENT_MENU_ACTION and republishes to ZMQ; cover art → DIS bitmap; projection provider inference (`projection_source.py`). LIVI equivalent needed: media metadata, nav maneuvers/distance, phone call state, cover art, projection state.
- **Custom Hudiy menu actions** (TP2_BRIDGE) — toggle_diagnostics, toggle_flashing_mode, update_rnse, update_hudiy, restore_configs, reboot, shutdown, check_version, save_logs, toggle_haldex_mode, toggle_exhaust_valve; status icons (Diagnostics Active, Flashing Mode); toast + notification channels.
- **Input action dispatch** — HUDIY_ACTION → Hudiy DispatchAction, bounded/stale-dropping queue.
- **Connection manager** — on ignition/radio active or door/wake window: bluetooth power on/unblock paired devices, Hudiy `connect_android_auto_wifi` retries; on idle: quit AA, block BT (or power off) after 60 s. [H+C]
- **Dark mode** — 0x635 → Hudiy SetDarkMode (+ optional AA day/night), night immediate, day after 4 s with ignition. [H+C]
- **API event capture** — JSON-lines log of every Hudiy callback, rotating 2 MB. Debug.
- **Update/restore scripts** — `update_rnse.sh` (branch/tag from config, runs installer, reboot), `update_hudiy.sh`, `restore_configs.sh` (sparse clone repo configs → `~/config.json` + Hudiy configs, backup). Run in fullscreen `foot`.
- **Hudiy config shipped** — `applications.json` (web apps: Manager :5004, DataView :5003), `applications_menu.json` (RNS-E category + items), `shortcuts.json` (top bar: DataView, Haldex mode, exhaust valve, resume AA), `dashboards.json`, `HudiySymbols.ttf` icon font.
- **Phone accept/reject** from DIS sends uinput KEY_P/KEY_O (Hudiy shortcuts).
- **Hudiy web-app theming/knob nav** (frontend, see 3.6).

### 3.3 Cluster DIS (`dis_client/`) — center display
- **DDP driver** (dis_service) — TP2-style session on 0x6C0/0x6C1, red/white cluster handshakes, zombie-session A8 close, init sequence, capability detect (0x52 mono / 0x7A hicolor), screen **claim** (regions central/full/centre_lower/top_centre), auto-reclaim after cluster warnings, claim_on_nav / claim_on_phone / start_inactive policies, draw command cache, framed atomic draws with ACK/NACK, ignition/bus gating, Flashing Mode gate. [C]
- **Page engine** (dis_display) — app rotation from `center_display.applist`, wiper-stalk paging (tap/hold, rate limit), wheel input when DIS owns wheel, priority resolver (phone > nav auto-switch > brief cover art > easter egg > user page), remembers last/startup app.
- **Rendering modes** — `high_resolution` true: native 128x96 bitmaps, white-cluster fonts with pixel metrics, delta tiles/planes/bands; false: legacy 64x48 / red cluster. Stock ROM glyph catalogs (`native_nav_catalog.json`, `stock_mono_catalog.json`, sha-guarded).
- **Apps**:
  - nav — maneuver icon (bitmap Maps-style 72x72 or stock ROM glyphs), distance + unit, street (scrolling), approach bar, road_side roundabouts, auto-switch (peek 5 s, 500 m interrupt, 1000 m rearm), "No Route". [H feed]
  - media — title/artist/album scrolling, position/duration, centering. [H feed]
  - phone — overlay during calls, accept/reject/end menu. [H feed]
  - coverart — brief 5 s takeover on track change; dithering pipeline (presets legacy/balanced/photo/text, `coverart.args`). [G processing]
  - car_info (legacy 5-line) / readings (hi-res, 8 values, pages from Data & Logs workspace `dis_pages`, units, staleness, icons, start/stop/mark data log). [C]
  - acceleration_test — 0x351 speed, configurable ranges/distances/speed lines, tolerance, auto reset, car units from 0x60E. [C]
  - openpilot — lane/path/lead render from HUDIY_OPENPILOT; off, receiver hard-disabled. [experimental]
  - **easter eggs** — `eggs.json` + GIFs (exists; not detailed).
- **Top display** (dis_top_display) — writes 0x363/0x365 radio text while RNS-E on TV source; priority phone > nav > media; per-line field modes; scrolling settings; fights OEM overwrites; phone menu via wheel; skips what the centre display shows. [C, H content]

### 3.4 Diagnostics / vehicle data (`tp2/`, `vehicle_data/`, `flasher/`)
- **TP2 worker** — up to 7 concurrent KWP sessions, scheduled measuring-group reads, error backoff/reconnect, DTC read (multiple service fallbacks) + freeze frames + clear, gateway install list (`1A 9F`, cached), one-off KWP jobs for other services, ignition-gated, enable toggle persisted, exclusive QUIESCE ownership for flashing. [C]
- **Value broker** (vehicle_data) — catalog of 466 values / 629 providers (engine 0x01 groups, DSG 0x02, Haldex 0x0A, 67 passive ICAN signals); clients subscribe by value ID with rate + lease (`diagnostics.values.lease_seconds`); prefers passive ICAN, else weighted set-cover of diag groups using measured costs; quality/stale status; legacy HUDIY_DIAG adapter. [G design, C catalog]
- **Three lease mechanisms** — TP2 SYNC 15 s hardcoded; broker value lease 15 s config; diag ownership token (no timeout, fails closed).
- **Flashing / readout** (via flasher submodule PQ35-Flasher, used only by dataview app.py):
  - Haldex Gen4 0x0A flash (with anti-brick patcher) + firmware/EEPROM readout. [C]
  - PQ EPS 0x09 full image or 4 KiB steering dataset flash + readout + EEPROM. [C]
  - pq-bcm 0x20 identify/readout; flash scaffold never run. [C, unfinished]
  - Flashing Mode + recovery marker; flash logs `~/.hudiy/flash_logs` (keep 2). [C]
- **Exhaust valve updater** — A/B slot OTA over 0x67A from uploaded ZIP bundle, unconfirmed (one-way). [C, custom HW]
- **Drive loggers** (two, overlapping):
  - `data_logger.py` legacy CSV (haldex/raw_can profiles, `~/logs/*.csv`), driven from AWD tab.
  - `data_logs.py` new broker-based sessions (`~/logs/data-logs/sessions/<id>/`), profiles, markers, review windows, CSV export, DIS control via ZMQ.
- **Openpilot TP2 receiver** — hard-disabled (IDs conflict with Haldex). [dead-ish]

### 3.5 Web backends
- **DataView** (`hudiy_dataview/app.py` :5003) — socket.io value streaming (`sync_values`/`values_batch`), legacy `toggle_group`/`diagnostic_*`, smoothing, DTC/install list, Haldex mode + status, legacy logger, exhaust valve, flash/readout jobs with progress, readout downloads, client log + theme capture. Plus file portal + data-logs routes + management config routes.
- **File portal** (`file_portal.py`) — collections: drive recordings, debug logs (journals, Hudiy API, avsd logs, runtime logs, flash logs), controller files (firmware + readouts), config upload; per-file download, async ZIP archive jobs (`archive_limit_mb`), uploads validated per firmware target (haldex, pq-eps, exhaust-valve, rnse-sector), upload PIN on uploads only, atomic no-overwrite. Theme cached for phone. [G]
- **Manager** (`hudiy_manager/` :5004) — config file editor (repo config + 5 Hudiy configs, revision check, flock, backups, atomic), service start/stop/restart/logs (allowlist, sudo -n, flasher lock for disruptive units), RNS-E live controls + ADC, Pi video colour (Wayland wlr-gamma: gamma/contrast/black point/RGB gain, profile auto-applied), native test pattern window (Hudiy `web_viewer`), network info (file-portal URLs incl. hotspot), theme proxy, mutation guard (header + same-origin + PIN). [G+H+C]
- **avsd app** (:5005) — see 3.8.

### 3.6 UI pages
- **DataView** (Hudiy web app, 800x480, swipeable tabs): Engine (MAF/boost/RPM/timing/knock x4/fuel rail/injection/temps), Transmission (DSG clutch pressures, shafts, selector travel, temps), AWD (Haldex gauges, mode selector, legacy drive logger with markers), Data & Logs (Record / Review graphs + CSV / DIS pages editor with value picker), Diagnostics (module list from install list, DTC read/clear + freeze frames, 3 measuring-group slots via keypad, Flash Controller modal for 0x0A/0x09, exhaust valve panel). FilesTab only in portal.
- **File portal** (`/files` on :5003, phone browser).
- **RNS-E Manager** (Hudiy web app): Settings (metadata-driven config editor, sections rnse/core/display/io/features/diagnostics/haldex/eps/exhaust/recordings/logger/files/net/can/transport/openpilot; RNS-E live controls + automatic brightness), Video (RNS-E ADC pages Clock/Gain/Black/Clamp/Sync/Output/Registers + presets + test image; Pi colours), Services, Logs (follow).
- **avsd app** (Hudiy web app): status, chain control, counters, settings apply/save/defaults, logs, RNS-E firmware check/rehearse/flash/decoder, Teensy flash, update/rollback.
- **Test pattern** native window (`rnse_test_pattern.html`).
- **car_widget** — Hudiy dashboard widgets loaded from file://: car picture (model/colour picker; broken for 8J .jpg) and boost gauge (legacy toggle_group on :5003). Not installed by install.sh. [H]
- **Shared frontend infra**: `useHudiyTheme` (window.hudiy.colorScheme Material You → CSS vars; cached server-side for phone views), `hudiyKeyNav` (Hudiy knob/triggered/goBack callbacks → focus model with data-key-* markup), drag-scroll for head-unit touch, touch keyboard incl. hex layout. Bundles: main/portal/management/avsd built by Vite, **committed by CI**.
- **help-site** (Netlify + manual GH Pages): user docs for all features, setup (hardware, CAN, video/audio, RNS-E FW, Hudiy install), how-tos; interactive faceplate/wheel explainers; DIS previews rendered from production app code; DataView/portal screenshots captured with fake socket; snapshot of tools (config editor, bitmap tool, audscii helper, theme viewer).

### 3.7 Pi platform (`pi-setup/`, manual, not applied by installer)
- Boot tuning 19.1 s → 11.3 s: raw kernel + postinst hook, readahead service + list tools, zram-only swap static units, /boot automount, tmpfs for rnse_control dirs, stop timeouts, getty delay, NM keyfile replacing netplan + netplan no-op divert, user-sessions without network.target. [G Pi 5]
- Hudiy session: labwc on tty7 from systemd (replaces lightdm), session guard (revert to lightdm if hudiy not up at 45 s; not boot-tested), lean session wrapper, minimal labwc autostart, usb-automount, `hudiy_startup_fast.sh` (EQ sink, echo cancel, NoiseTorch, waits for NM, touches hudiy-started). [H]
- config.txt: mcp2515 8 MHz int 23, GPIO 25 power latch + gpio-poweroff, custom 800x480 EDID (HDMI→VGA), PCIe gen3. [C hardware]
- revert.sh, measure.sh, ioproc/randread.

### 3.8 A/V over SD (`avsd/`) [C + H capture]
- Digital video+audio into RNS-E via Teensy 4.1 emulating an SD card; RNS-E runs custom "sdstream" firmware.
- Chain: `wlcap` (wlr dmabuf/screencopy of Hudiy's NOOP-1 800x480) → `rnv3enc` (custom RNV3 codec, NEON, link-rate control, keyframe FIFO) → `avlink` (60 Hz superframes video + 48 kHz audio, wire images, TLNK credits, sdctl FIFO) → USB CDC → Teensy. Audio from PipeWire `echo_cancel_sink`/`hudiy_equalizer_sink`, own `avsd_clock` null sink.
- Supervisor `avsd_run.sh` (waits for Wayland/pulse/NOOP-1/hudiy/sinks/Teensy; liveness watchdog; logs), `hud2.sh` (SCHED_FIFO priorities, renice hudiy), env tuning `avsd.env` + trial env.
- App: settings whitelist, chain control, RNS-E sector flasher over CAN 0x7B0/0x7B6 + SD stub (boot-ROM checksum fixups, sector cache, blocklists), RNS-E counters, Teensy HalfKay flash, releases with rollback + post-update watchdog, self-update.
- Installer adds Hudiy menu entries, disables wayvnc, invisible cursor, linger. Teensy firmware source not in repo.

---

## 4. Config option groups (`config.json`)
- top-level: `branch`, `repo`, `config_restore_repo`, `config_restore_branch` (updater)
- `rnse`: auto_brightness{enabled,day,night}, manual_brightness, auto_lcd_brightness{…}, manual_lcd_brightness, source_label.enabled
- `interfaces.can`: infotainment, diagnostic; `interfaces.zmq`: 14 endpoints (several more are code defaults only)
- `diagnostics`: enabled, values{diagnostic_hz, ican_hz, lease_seconds}
- `haldex`: enabled, default_mode, persistence_file, firmware_dir; `eps.firmware_dir`
- `exhaust_valve`: firmware_dir, installed_generation_floor, generation_file, intent_file
- `data_logger` (legacy): default_profile, log_directory, queue_size, profiles; `data_logs`: directory, queue_size (+workspace_path)
- `file_portal`: upload_pin, archive_limit_mb, runtime_log_directory, firmware_targets[]
- `openpilot.enabled`
- `display.center_display`: applist, enabled, mode, high_resolution, start_inactive, navigation{icon_style, auto_switch, thresholds, approach_bar_max_distance, claim_on_nav, native_render_order, native_message_delay_ms}, coverart{brief, args{…12}, native_preset, native_args, native_render_order, native_delta}, acceleration_test{auto_reset…, line_1..4}
- `display`: road_side, units{speed, ambient_temp, boost, boost_mode, cartemp}, text_scrolling{…6}, text_centering, top_display{applist, enabled, 7 line modes}, phone{scroll_wheel_phone_menu, claim_on_phone}
- `features`: day_night_mode, sync_android_auto, time_sync{enabled,data_format,threshold_minutes}, power_management{auto_shutdown, listen_only_mode, gpio_shutdown, connection_management{…11}}, tv_simulation{enabled,payload}, mmi_controls, mfsw_controls, source_controls, system_actions, car_time_zone, debug_mode, log_saver{services,minutes,log_directory}
- `can_ids`: 15 IDs (media_status/nav_status unused)
- `input_mappings`: mmi{cooldown, press counts, scroll_commands, short/long/extended tables}, mfsw{double_click_ms, commands, short/long, phone_alt(unused)}, source{tv_mode_identifier, play_key, pause_key}
- Undocumented code-only keys: `ddp_*` tuning, `can_ids.steering_module`, `features.initial_mode`, `mfsw.long_press_message_count`, vehicle_data endpoints, `media_lineN_scroll_speed_cps`, `transport.ddp_bitmap_message_bytes` (in metadata).
- Other persisted state: `~/.hudiy/haldex_mode.json`, `exhaust_valve_intent.json`, `exhaust_deployment.json`, `rnse-adc-presets.json`, `share/video-color.json`, `tp2_install_list.json`, `~/.tp2_settings.json`, `~/dis_settings.json`, `~/logs/data-logs/workspace.json`, `~/avsd/avsd.env`.

---

## 5. Dev / test tooling worth an equivalent
- **DIS emulator** (`dis_emulator/emulator_service.py` :5000 + `dev_runner.py` hot reload) — browser-rendered DIS + top display, mock CAN/Hudiy publishers (media playlist, cover art, nav, phone, openpilot demo). Gap: no `draw_native_font`.
- **Bench cluster** (`dis_emulator/bench_cluster/`) — real white cluster via J2534 on Windows, web console :8766, wake traffic, wire logs. Depends on out-of-repo `HaldexRE` shim.
- **Flasher runner** (`flasher/runner.py`) — desktop CLI for j2534/panda/socketcan; BSLScripts UART recovery for Haldex.
- **Tools**: config_editor.html (+ metadata export), bitmap_tool, audscii_helper, theme_viewer, can_decoder_pro (correlate CAN bytes vs groups), process_dumps, inspect_measuring_groups, export_value_catalog, nav icon generator, font importer, dis_image_tester (FastAPI+React), dhu_driver (AA DHU GPS driving, Windows), tp2 module_survey/module_probe.
- **Tests** (`tests/`, 120 files, pytest + node --test in CI; no conftest; many tests `ast.parse`+exec production code — fragile). Fakes worth porting: `_native_opening_peer.py` (simulated cluster DDP peer + virtual clock), `test_tp2_transport.FakeBus` (timed fake CAN + ECU responder), `test_avsd_app.FakeUnit` (simulated RNS-E memory map), FakeEcu (Haldex), SimulatedFlashTransport (EPS), FakeKwp, fake_zmq, FakeHudiy.
- **help-site render scripts** — render DIS previews from production app code (good regression/visual test idea).
- **CI**: tests.yml (pytest 3.11, dataview typecheck + node tests, help-site check), build-dataview.yml (builds + commits bundles), help-site-pages.yml.
- **References** (high value for rewrite): `PQ35_46_ICAN.dbc`, `ACAN.dbc`, `ICAN_rundown.md`, `dis display references/pcbbc.txt` (DDP), `tp2diag references/` (VW TP2.0 spec, tp20logger), `TV_BUTTONS_HUDIY_MAPPING.md`, `FLASHING_MODE.md`, `HALDEX_*`.

---

## 6. Dead / abandoned / suspect
- Root: `test_comparison.py`, `scratch/` (tracked pngs + test_draw.py), `example_cmdline.txt` + `example_config.txt` (Pi 4 composite, 12 MHz HAT), empty `package-lock.json`, `oldREADME.md`, `.DS_Store` files.
- Composite video / RNS-E 192 support: only in commented install.sh + oldREADME.
- `tp2/tp2_debug.py`, `wakeup_test.py`, `status_tp2_logger.py`, `dtc_lookup.py`; unused tcp 5557/5558 constants in tp2_worker.
- `flasher/kwp2000.py` (no importers); pq-bcm flash scaffold.
- openpilot app + receiver (hard-disabled); `dis_emulator/static/mock_model_data.json` 19 MB.
- `apps/nav.py::_get_stock_view` unused; stock_nav_glyph/composition/object, native_symbols, stock_mono_frame have no production producer; dis_display `t_mfsw`/`t_car` subscriptions unused; 5A capability parsed not rendered; `ENABLE_INACTIVITY_RELEASE`/`UNSAFE_BATCHING_BYPASS` off.
- `dis_client/dis_tests/` ad-hoc hardware scripts (DDP MITM bridge, telephone channel probes) + ~7 MB GIFs; profile .txt files are notes.
- `can_base_function`: `handle_gateway_nm_message`/`handle_nav_nm_message` unused; `rnse_control.pending_command/target/has_control` unused.
- Config keys unused: `can_ids.media_status`, `nav_status`, `input_mappings.mfsw.phone_alt`; `openpilot` "reserved".
- Legacy duplicates: `data_logger.py` vs `data_logs.py`; `toggle_group`/`diagnostic_*` path still used by mock mode, Diagnostics slots and boost_widget; `hudiy_status_service` unit name.
- Frontend: `useSmoothedData.ts`, `cycleMode`, `tsc_output.txt`, `build-validation/`, `management-preview` (dev fixture), DiagnosticsTab opens a 2nd socket.
- TEMPORARY key-nav probe route + client `probe()` (memory: remove after next bench test).
- car_widget not installed; 8J colours broken.
- Known bugs: brightness night detection ignores 0x635 invalid bit; `ZMQWorker.run` undefined `_recv_count`; restore_configs silently falls back to main; Manager config save doesn't notify services (only rnse section reloads); `.claude/launch.json` port 5173 clash; tools/README stale.
- Untested: avsd RNS-E flash from Pi + update terminal on RNS-E; hudiy-session-guard; pi_wayland_color on real Pi.
- FixList.md open items: GPIO sleep vs listen-only delays, nav auto-switch white flash, arrival time, Load/IAT/boost/coolant formulas and several group units wrong, TP2 group reads drop, faster car-info refresh.
