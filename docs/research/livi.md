# LIVI research notes (for the Hudiy -> LIVI foundation plan)

Clone: `C:\Users\raccoon\AppData\Local\Temp\claude\0d19c1a42222ca8b\66e23859-655e-46b7-b48c-43762d830db5\scratchpad\livi` (read only, nothing built or run).
HEAD: `bfbea31ee624ddce84bfa96ce49d4663e6cdeca7`, 2026-11-07 14:35 +0100, "chore(docs): updated ui". `main` == `origin/dev`. Workspace version 10.5.0 (`native/Cargo.toml`). Latest release v10.4.0 (2026-11-04); nightly #2662 built from this commit.
HEAD is the same commit as the prior findings, so nothing changed since; all prior findings re-checked below. Paths are relative to `native/crates/` unless they start with `native/`, `scripts/` or `assets/`.

## Prior findings: verified

| Claim | Verdict |
|---|---|
| core separate from UI, UI is a client over a Unix socket with JSON patches | True. Details in section 1. Correction: the core sends **leaf-level** diffs, not whole sections (`livi-core/src/hub.rs:95-106`, `livi-core-proto/src/patch.rs:36-66`). |
| telemetry: free-form JSON map, POST :4000/telemetry or Socket.IO `telemetry:push`; full snapshot broadcast per push; no rate limit, per-field timestamps or auth; bound to 0.0.0.0 | True. `livi-core/src/telemetry.rs:23-26, 175-179, 305-350`. It binds `[::]:4000` first, then `0.0.0.0:4000` (`:342-350`), with `CorsLayer::permissive()` (`:337`). Each push stamps one global `ts` (`:71-75`). Socket.IO clients get the full snapshot (`:177`). Core-socket clients get leaf diffs of `state.telemetry`. |
| AA sensors: 13 sent; doors/HVAC/accel/gyro/compass/GPS sats advertised but never sent; no dead reckoning | True. 21 advertised (`livi-aa-stack/src/discovery.rs:127-148`). The `Sensor` enum has 13 variants (`livi-aa-stack/src/sensors.rs:26-62`). They are filled in `livi-core/src/aa_telemetry.rs:102-290` and sent on change only (`:96-99`). |
| AA nav: turn, distance and maneuver parsed; 256x256 image; lanes and later steps not parsed | True. `decode_state` reads only the first step's maneuver, road and cue, plus the first destination (`livi-aa-stack/src/channels/navigation.rs:296-331`). Lanes (proto field 3, `livi-aa-proto/proto/navigation.proto:238-243`) are ignored. Image options: `discovery.rs:402-413`. The State already has `maneuvers`, `laneGuidance` and `currentManeuvers` (`livi-core-proto/src/state.rs:333-375`), but only CarPlay fills them (`livi-runtime/src/events.rs:14-64`). The AA mapping leaves them at their defaults (`livi-core/src/projection/aa.rs:317-347`). |
| Calls parsed but never consumed | True. Parsed at `livi-aa-stack/src/protocol.rs:1055-1071`, reported at `bridge.rs:108,247` and `manager.rs:56,121`, dropped by `_ => {}` at `livi-core/src/projection/aa.rs:108`. **There is no call state anywhere in `State`**, for CarPlay either (`state.rs:15-32`). |
| Media metadata, art, playback; phone battery and signal | True. `state.rs:378-387` (`nowPlaying`, artwork as base64). Battery and signal live in `devices[]` (`state.rs:223-243`). |
| Input: rotary + keycodes | True. 50 keycodes (`discovery.rs:74`); rotary at `livi-aa-stack/src/channels/input.rs:60`. |
| Cluster video channel exists | True (`discovery.rs:295-310`). |
| car control, car actions, radio, local media, vendor extension and car intent protos unimplemented | True. Protos exist (`livi-aa-proto/proto/*.proto`). The advertised services are video, cluster video and cluster input, audio, mic, sensor, input, bluetooth, navigation, media info, phone status and wifi (`discovery.rs:255-432`). |
| UI tabs are a fixed enum; no plugin slot; compositor lays out only LIVI's windows | True. `livi-ui-model/src/shell.rs:8-13` (Projection, Telemetry, Media, Camera, Settings). Foreign windows are classified as "Dialog" and centred (`livi-compositor/src/shell.rs:159-171`). **New:** issue #408 (2026-11-06, no reply yet) proposes an external-program tab. See 2e. |

## 1. Processes and how they talk

Linux kiosk launch is core-first (`livi-core/src/main.rs`, `resources.rs:54-140`):

- **livi-core** (user, tokio). Owns the state hub, the AA protocol stack (`livi-aa-stack` is linked in), the CarPlay stack (`livi-cp`), telemetry (:4000), radio (DAB/FM), the update check and config. Serves `core.sock`.
  - spawns **livi-helperd** through `sudo` (`livi-core/src/helper.rs:19,218-224`; sudoers template `assets/linux/99-LIVI-helper.sudoers.template:6` is NOPASSWD). It handles the privileged transport: the Wi-Fi AP, Bluetooth (its own host stack, `livi-bt-host`/`livi-hfp`), AA TCP/TLS/USB (`livi-aa`), and iAP2. AA bytes reach core over `aa.sock` (`livi-core/src/android_auto.rs:837`).
  - spawns **livi-gst-host**: the GStreamer process for video decode and audio.
  - spawns **livi-compositor** (smithay; `livi-media/src/compositor.rs:164-218`), configured through env (`compositor.rs:23-66`). Core then drives it over a line-based control socket (`LIVI_COMPOSITOR_CTRL`, `livi-compositor/src/ctrl.rs`).
  - spawns **livi-ui** (Slint) as a Wayland client of that compositor (`livi-core/src/ui.rs:54-82`). It is restarted up to 5 times (`ui.rs:16`).
- Desktop launch is the reverse: the `LIVI` UI binary starts `livi-core` with `LIVI_NO_UI=1` (`livi-ui/src/core_process.rs`).
- The processes are tied together by a lifeline: children exit when core exits (`LIVI_LIFELINE`).

### Core socket protocol
- Path: `$LIVI_CORE_SOCKET`, else `$XDG_RUNTIME_DIR/livi/core.sock`, else `/tmp/livi-<uid>/core.sock` (`livi-core-proto/src/runtime.rs:8-22`).
- Framing: u32 little-endian length, then a JSON body; max 16 MiB (`livi-core-proto/src/frame.rs:10-40`).
- Handshake: the first message must be `{"type":"hello","protocol":11,"client":"<name>"}`. **The protocol must match exactly**, or the core answers `refused` (`livi-core/src/server.rs:384-391`; `PROTOCOL = 11` at `livi-core-proto/src/lib.rs:22`). **The peer uid must equal the core's uid**; otherwise the connection is dropped silently (`server.rs:365-369`).
- From core: `welcome{protocol,version,rev,state}`, then `patch{rev,ops:[{op:"set"|"remove",path:[..],value}]}`, `reply{id,error?}`, `spectrum`, `openPage{path}`, `refused` (`livi-core-proto/src/message.rs:24-51`). A lagging client gets a fresh welcome (`server.rs:432-434`).
- To core: `resync`, `input{input}`, `action{id,action}`, `path`, `spectrum`, `linkSpeed` (`message.rs:12-22`).
  - Actions (`message.rs:53-168`): `setConfig{patch}` (persisted to config.json), `show{screen,front}`, `media`/`phoneMedia{control}`, `seek`, device select/forget/connect/next, cluster layout actions, DAB/FM controls, update check/download/install, `quit`, `restart`.
  - Input (`livi-core-proto/src/input.rs:9-31`): `{kind:"key",code:"KeyH",down}` (browser-style codes) or `pointer{screen,points}`.
- The State sections a car daemon wants (`state.rs:15-32`): `front`, `sessions`, `nowPlaying`, `navigation` (the active session's), `devices` (battery, signal, carrier), `telemetry` (echo of what is pushed), `config` (appearanceMode, darkMode, everything else), `cluster`, `update`.
- Multiple clients at once: yes. `serve` spawns a task per connection (`server.rs:327-340`), and the tests cover config changes reaching every client (`server.rs:814`).

**Could the car daemon be a client like livi-ui?** Yes, with conditions:
- It must run as the same uid as livi-core, or a same-uid shim must relay.
- It must speak exactly protocol 11.
- It should not send `path` (that is the UI's own page report; `server.rs:425-427`).
- It must not link the GPL crates `livi-core-client`/`livi-core-proto`, or it inherits the GPL if distributed. The framing is about 30 lines to reimplement.

What it can do over the socket:
- **Read:** nav (turn, distances, ETA, road, maneuver type, turn image as base64 PNG), media (title, artist, album, art, playing, position), devices (battery, signal), settings.
- **Push telemetry:** not over this socket. Use HTTP or Socket.IO on :4000. The `livi-core telemetry set` CLI does the same.
- **Inject input:** yes, but `Input::Key` only reaches the core's projection key path (`livi-core/src/projection/input.rs:86-112`). It acts only while projection is in front, except Home, media, phone and voice keys (`projection.rs:224-236`). It **does not drive LIVI's own menus**: livi-ui does focus navigation itself from Wayland key events and forwards keys to core only when appropriate (`livi-ui/src/keys_ui.rs:359,458`). For full knob control use uinput.
- **Set night mode:** yes, either `setConfig{appearanceMode:"night"|"day"|"auto"}` or telemetry `nightMode`/`darkMode`. See 2g.
- **Calls:** not available. No state exists.

**Stability:** not stable. `PROTOCOL` changed 8 times between 2026-10-04 and 2026-10-21 (`git log -G'PROTOCOL: u32'`). `state.rs` and `message.rs` were touched 11 times in the last 3 months. The native rewrite (v10.0.0) is only 2 weeks old. There are no compatibility promises or docs; the wire format is defined only by the Rust types and tests. The HTTP telemetry JSON is documented and looser (unknown keys are kept), so it is the more stable surface.

## 2. Where a fork would touch, size, upstream odds

Upstream rules apply to every PR: issue first; one maintainer; **AI may not be commit author or `Co-authored-by`** (use an `Assisted-by:` trailer; enforced by a bot, see PR #403); the CLA must be signed. Of the last 100 PRs, 6 external PRs were merged and about 13 closed unmerged. The maintainer often reimplements fixes himself (#367, #388).

| | Touch points | Size | Upstream |
|---|---|---|---|
| a. Calls into state | Add `calls: Vec<Call>` to `State` (`livi-core-proto/src/state.rs:15-32` + `refresh` at `:45`), keep it per session in the projection entry, handle `AaEvent::Calls` at `livi-core/src/projection/aa.rs:95-108`, bump `PROTOCOL`. Optional: the CarPlay side (`livi-cp`) and a UI phone overlay. | 150-300 LOC with tests (AA only) | Medium-high if it covers CarPlay too and has a use case (cluster display). |
| b. AA lanes and next steps | `decode_state` `livi-aa-stack/src/channels/navigation.rs:296-331` (loop over all steps, decode `lanes`/`lane_directions`, roundabout fields); the `Navigation` struct and merge at `livi-aa-stack/src/bridge.rs:37-70` and `:399-480`; map into the existing `maneuvers`/`laneGuidance`/`currentManeuvers` at `projection/aa.rs:317-347`. No protocol bump needed (the fields exist). | 200-350 LOC | High. It fills fields CarPlay already uses, and the AA cluster UI would benefit. |
| c. More AA sensors | `Sensor` enum and `batch()` (`livi-aa-stack/src/sensors.rs:26-200`); mapping from telemetry (`livi-core/src/aa_telemetry.rs:102-290`); the field table in `livi-core-proto/src/telemetry.rs:20-66`. Dead reckoning also needs `LOCATION_CHARACTERIZATION` (`discovery.rs:72`) set honestly, plus accel and gyro at a steady rate. Rate is the snag: `follow` sends on change only. | Doors, HVAC and compass about 40 LOC each; accel+gyro+DR 200-400 | Doors and compass: medium-high. Accel/gyro/DR: low-medium unless shown working with Maps on real phones. |
| d. Car actions / vendor extension | New channel ids (`livi-aa-proto/src/consts.rs:114+`), a `Service` in `discovery.rs`, a handler in `livi-aa-stack/src/channels/`, a `Report`, core consumption, and an external API to expose it. The protos are thin (`vendor_extension.proto` is 12 lines; `car_control.proto` is 97). A vendor extension needs a phone app that speaks it. | 500-1500 LOC + reverse engineering | Low-medium. |
| e. Pages | (i) External window slot: issue #408 proposes `app <pgid> [x y w h]` on the compositor ctrl line, with the UI reporting its content area and a settings command. About 480 LOC exists out of tree. Touches `livi-compositor/src/shell.rs:153-176` (classification), `layout.rs`, `ctrl.rs`; `livi-ui-model/src/shell.rs:8`; settings. (ii) Slint pages: a `Tab` variant plus a `.slint` page plus the model in `livi-ui`. | (i) about 500 LOC; (ii) 300-800 per page | (i) Medium-high if #408 lands (better to back it than duplicate it). (ii) Low for car-specific pages. |
| f. Frames to an external encoder | Today with no fork: nested mode (2 composites + screencopy). Fork: a third backend next to `drm.rs`/`host.rs`, selected at `livi-compositor/src/main.rs:56-60`, rendering through the generic `render::draw<T>` (`render.rs:302`) into an offscreen GLES/GBM buffer and exporting a dmabuf fd or shm to the encoder. Hack with no fork: `LIVI_GST_SINK` replaces `waylandsink` in the projection pipeline (`livi-video-player/src/lib.rs:330`, `livi-video/src/codec.rs:114-131`). That gives raw AA video only, without LIVI's UI. Untested. | 500-900 LOC | Low-medium. A generic "headless output + screencopy" might pass; a custom encoder hook will not. |
| g. Day/night from outside | Nothing to change. Set `appearanceMode:"auto"` once (`livi-core-proto/src/config.rs:214-228`). Then push telemetry `nightMode` (goes to AA, `aa_telemetry.rs:142`) and `darkMode` (LIVI's UI, `telemetry.rs:239-252`). Fixed Day/Night config overrides telemetry (`telemetry.rs:228-237`). | 0 | n/a |
| h. External input | Nothing to change. Use uinput: in nested mode labwc gives the keys to livi-compositor's host backend, then to livi-ui. In DRM mode the compositor uses libinput directly (`livi-compositor/src/drm/libinput.rs`). Key codes are mapped by LIVI's Key Bindings settings (`livi-core/src/projection.rs:196-222`). The socket `Input::Key` works for projection only. | 0 | n/a |

## 3. Display and compositor

- **Backend choice** (`livi-compositor/src/drm.rs:128-134`): `LIVI_COMPOSITOR_BACKEND=drm|wayland`. By default it uses DRM when `WAYLAND_DISPLAY` is unset; otherwise it nests as an xdg toplevel in the host compositor (`host.rs`, smithay-client-toolkit, EGL on a render node).
- **So:** it runs nested in labwc with no DRM/KMS ownership. It still needs a GPU render node (EGL/GLES; `host.rs:7-20`). There is no headless backend of its own.
- **Resolution:**
  - Nested: `req_width/height` from config, else `LIVI_OUTPUT_SIZE` (= `mainScreenWidth x mainScreenHeight`, plus the title bar unless kiosk), else 1280x720 (`host.rs:323-338`, `livi-media/src/compositor.rs:23-36`). Kiosk makes the window fullscreen (`state.rs:200-221`), so set kiosk and 800x480.
  - DRM: `LIVI_DISPLAY_MODE` / connector env per screen (`drm.rs:386-393`, `drm/modes.rs`).
- **AA video size:** fitted to tiers (800x480, 1280x720, 1920x1080; `livi-aa-stack/src/config.rs:128-130,187-200`).
- **Video path:** gst-host builds `appsrc ! parse ! decoder ! waylandsink` (`livi-video/src/codec.rs:159-177`) as a separate `livi-video` toplevel that the compositor stacks under the UI (`shell.rs:123-151`).
- **Decoders on Pi 5:** there is no H.264 hardware, so H.264 decodes in software (`avdec_h264`). HEVC uses `v4l2slh265dec` (hardware) (`codec.rs:48-85`). LIVI offers HEVC to the phone when H.265 hardware is present (`livi-core/src/carplay.rs:545-548`, used for AA at `android_auto.rs:29`). So on a Pi 5, AA should negotiate H.265 if the phone supports it.
- **Cost hints** (unmeasured): there are 5 processes plus GStreamer.
  - Nested chain per frame: HW decode, dmabuf, livi-compositor GLES composite, labwc composite of NOOP-1, screencopy, encoder.
  - At 800x480 the GPU load is small; the CPU risk is software H.264 if HEVC is not negotiated.
  - Memory on 2 GB is unknown. Measure RSS of livi-core, livi-ui (Slint/femtovg), gst-host, compositor and helperd on the bench.
  - The display calibration pass costs nothing when it is at defaults (README).

## 4. Config, update, build, packaging

- **Config:** `~/.config/LIVI/config.json` with a backup (`livi-core/src/paths.rs:20-36,52-58`), schema-typed (`config_file.rs:220`). State files are under `state/` (for example `climate.json`). Most knobs are env vars: `LIVI_KIOSK`, `LIVI_NO_UI`, `LIVI_UI_CMD`, `LIVI_COMPOSITOR_BACKEND`, `LIVI_GST_SINK`, `LIVI_GST_SWDEC`, `LIVI_CORE_SOCKET`, `UPDATE_REPO`/`UPDATE_FEED`.
- **Update:** checks GitHub releases (`/releases/latest` or the `nightly` tag; `update.rs:44-60`), downloads the arm64 AppImage and swaps it in place, as admin if needed (`update.rs:142-200`). It is user-triggered through actions. For a fork, set `UPDATE_REPO` to your own repo or never press update.
- **Packaging:**
  - Linux: an AppImage (x64, arm64), built natively on `ubuntu-24.04-arm` in CI (`.github/workflows/build.yml:72-73`).
  - Local cross-build: `scripts/linux-arm64/build.sh` runs a podman **Debian Trixie** container with `gcc-aarch64-linux-gnu` and `:arm64` dev libs, then `cargo xtask package-linux --arch arm64` (`scripts/linux-arm64/Containerfile`). This matches Pi OS Trixie well.
  - Rust toolchain pinned in `rust-toolchain.toml`.
  - Installers in `scripts/install/`: desktop, headless and pi. Headless means tty1 autologin, a `livi-kiosk` service, seatd and PipeWire, and sudoers drop-ins.
- **Power:** with `LIVI_KIOSK=1` LIVI powers the host off and reboots it itself (`livi-core/src/power.rs:26-28`). That conflicts with a daemon that owns power. Do not set `LIVI_KIOSK`; use the config `kiosk.main` for fullscreen instead (`compositor.rs:33`). Verify that this split works.
- **Networking:** LIVI runs its own Wi-Fi AP and Bluetooth stack ("Dedicated Interface" takes the adapter away from NetworkManager). Check against the current NetworkManager-first boot order.

## 5. Project health and license

- **Activity:** 535 commits in the last 3 months (Aug 85, Sep 143, Oct 206, Nov 1-7: 101). 478 by Lasse Heitgres, 55 by bots, 2 by others. The project has a bus factor of one.
- **Repo:** 1084 stars, 138 forks, discussions on, Discord.
- **Releases:** v9.4 (10-11) to v10.0 (10-22, native rewrite) to v10.4 (11-04): several a week, plus a rolling nightly.
- **Issues and PRs:** one open issue (#408, the custom tab, directly relevant). Closed PRs that matter: #332 "LIVI as telemetry client", rejected ("LIVI is designed as a server", wants an issue and a use case first); #352 AA sensor compatibility hack, rejected; #403 blocked by the AI-author rule.
- **Contributing** (`CONTRIBUTING.md`): issue before PR; `type(scope): summary` commits; fmt, clippy and tests clean; **no AI as author or co-author**, use `Assisted-by:` instead; CLA required.
- **CLA** (`CLA.md`): an **exclusive** licence to Lasse for each contribution, with a non-exclusive licence back to you for your contribution only. He can relicense contributions, including closed-source.
- **Licence:** GPL-3.0-or-later plus a section 7(b) attribution term (`NOTICE`): any work containing LIVI must show "Based on LIVI by Lasse Heitgres (https://github.com/f-io/LIVI)" in its About screen and README.
  - **Private fork** that is only installed in your own car: no GPL obligations (nothing is conveyed).
  - **Distributed fork** (public repo, images for others): GPL source plus the attribution term.
  - **Separate daemon** talking over socket/HTTP: an independent program, any licence. If it links `livi-core-proto`/`livi-core-client` it is a derivative work and must be GPL when distributed.

## 6. Recommendation

**Run stock LIVI (pinned release) with the external daemon over its APIs now. Keep a thin fork branch only for what cannot be reached from outside, and offer those patches upstream.** Do not deep-fork: the churn (about 6 commits a day, protocol bumps, a rewrite weeks old) would turn a deep fork into a full-time rebase.

Thin-fork candidates, in order:
1. b (AA lanes and steps): high upstream odds, and the DIS needs it.
2. a (call state): the only way to get calls while LIVI owns Bluetooth.
3. Doors and compass sensors.
4. Back #408 for an external page slot instead of writing Slint pages.

Skip f (the encoder backend) unless the nested screencopy path proves too slow. Skip d.

### LIVI bridge in the daemon
- **Optional module.** The daemon runs fully without LIVI (CAN, power, DIS, input, diagnostics). The bridge only adds data.
- **State in (core socket):**
  - Own implementation of the framing (u32 LE + JSON) and a `hello` with protocol 11.
  - Keep a JSON mirror and apply `set`/`remove` ops. Decode only the sections used: `navigation`, `nowPlaying`, `devices`, `sessions`, `front`, `config.appearanceMode/darkMode`, and `calls` once patched in.
  - Unknown fields are ignored.
  - On `refused` or a decode error: log, show "LIVI version not supported" in diagnostics, and fall back to HTTP-only.
  - Reconnect with backoff; send `resync` when `rev` jumps.
  - Run as the LIVI user (the uid check).
- **Telemetry out:**
  - HTTP `POST http://127.0.0.1:4000/telemetry` with a curated, rate-limited JSON: speed/rpm at about 5 Hz, everything else on change, coalesced.
  - Units must match the field names (`speedKph`, `coolantC`, ...).
  - Firewall :4000 to localhost (it is open on all interfaces, including LIVI's AP).
- **Night:** set `appearanceMode:"auto"` once through `setConfig`, then push `nightMode` (AA) and `darkMode` (LIVI UI) from the dash-light/lux logic.
- **Input:** a uinput keyboard device emitting the codes bound in LIVI's Key Bindings. Do not use socket input.
- **Actions out:** `media{control}` and `show{screen:"main",front:"projection"|"livi"}` for steering-wheel and MFSW shortcuts. Use telemetry `path` (`telemetry.rs:181-194`) to open LIVI pages.
- **Contract test:** keep captured `welcome`/`patch` frames from the pinned LIVI version as fixtures in the daemon's tests. Re-capture and diff on every LIVI upgrade.
