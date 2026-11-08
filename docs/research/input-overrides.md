# Input and radio overrides: research notes for the Rust rewrite

Sources: repo `RNS-E-Hudiy` on `testing` (read-only), plus Drew's firmware notes outside the repo in
`C:\Users\raccoon\Documents\carstuff\RNSE hacking\` (`CAN_PROTOCOL.md`, `R54B_R56B_FEATURES.md`,
`analysis\TV_BUTTONS_0x461_R21.md`, `analysis\FIRMWARE_R21/R38/R44_CHANGE_RECORD.txt`), and LIVI at commit
`bfbea31` cloned to the scratchpad `livi-input`. Repo paths are relative to the repo root.

"r21" is the public name for Drew's firmware. The protocol has kept changing: r21, r24, r31, r35, r38, r44 and
r52+ all changed it. The repo code targets **r38** for brightness (`rns-e_can/rnse_control.py:3`) and **r44+** for the
`0x7B6` reply ID (`hudiy_manager/ADC_TUNING.md:8`). The car unit now has flash-over-CAN and Doom (`avsd/app/can_link.py:3-6`,
`avsd/app/rnse_flash.py:94`), which puts it in the r52+ lineage. That matters for B7 (see section 3).

---

## 1. Input sources

### 1a. Steering wheel: `0x5C3` mMFL_Cmd_ (gateway → infotainment)

- DBC: `references/PQ35_46_ICAN.dbc:1247` gives DLC 2. Byte 0 is `ML1_Ziel_` (target) and byte 1 is `ML1_Befehl_`
  (command). `GenMsgCycleTime` is 500 ms and `GenMsgDelayTime` 50 ms (`dbc:8075-8076`). The rundown is at
  `references/ICAN_rundown.md:803`.
- The idle frame is `39 00`, repeated every 500 ms (`references/canlog.txt`, 21x `39 00`).
- Byte 0 varies. The captured MODE double-click is `3A 1C, 3A 00, 39 01, 39 00`
  (`tests/test_dis_readings_controls.py:412`). The old code ignores byte 0 and reads only byte 1
  (`rns-e_can/can_keyboard_control.py:460`, `data_hex[2:4]`).
- Command codes come from config (`config.json` `input_mappings.mfsw.commands`):

| Byte 1 | Button | Old handling |
|---|---|---|
| `0B` | thumbwheel up | `scroll_up` |
| `0C` | thumbwheel down | `scroll_down` |
| `08` | thumbwheel click | `scroll_click` |
| `1C` | MODE | `mode` |
| `01` | MODE second press of a double-click (gateway-generated) | `mode_double` (`can_keyboard_control.py:463-464`) |
| `2A` | PTT | `ptt` |
| `06` / `07` | volume roller up/down | mapped to `null`: the RNS-E handles volume itself |
| `A7` | volume roller click | mapped to `null` |
| `00` | release / idle | release (config lists `"0x00"` twice) |

- PTT on r21+: the firmware makes the RNS-E **ignore PTT**, so the Pi or phone can own it
  (`RNSE hacking\R41_FEATURES.md:59`, `R54B_R56B_FEATURES.md:154`).
- The RNS-E also consumes 0x5C3 natively: volume, and on radio sources the up/down buttons change station or track.
  The Pi only sniffs. The old code does **not** gate wheel keys on the active RNS-E source (`source_controls: false`),
  so on FM a wheel up/down skips the station on the RNS-E *and* sends `KEY_V`/`KEY_N` to Hudiy.
  **Guess or bug:** I can't tell whether Drew intends this.
- Stock RNS-E in TV mode echoed wheel up/down onto 0x461 for the TV tuner. r21 removes that echo
  (`TV_BUTTONS_0x461_R21.md`, last line).

**Wheel gesture semantics** (`rns-e_can/wheel_controls.py`, `can_keyboard_control.py:456-518`):
- Holds repeat the press frame. The old code *assumes* every 500 ms (`can_keyboard_control.py:189`, comment only).
  Long press = 3 press frames (`long_press_message_count`, `wheel_controls.py:20`, about 1 s). This is
  **unverified**: it's based on the DBC cycle time, not a capture.
- Thumbwheel up/down: one action per press. `scroll_lock` swallows repeats until the release `00`
  (`wheel_controls.py:168-175`).
- Click (`08`): short fires on release unless the long press fired (`wheel_controls.py:187-197`). Long fires at
  frame 3 (`:215-221`). In DIS-owner mode a click starts a 250 ms double-click window: single = `select`,
  double = `back` (`:198-206`, `:145-156`).
- MODE (`1C`): single fires only after the 250 ms double-click window expires (`:149-154`, `double_click_ms`,
  clamped to 150–800 ms at `:19`). Double = gateway code `01` right after a MODE click, or two MODE clicks
  (`:176-186`, `:198-202`). It toggles the "wheel owner" between Hudiy keys and DIS apps. Long = frame 3.
  A stray `01` without a preceding MODE does nothing.
- PTT (`2A`): counted per frame. Long at frame 3, short on release (`can_keyboard_control.py:486-491`, `:508-509`).
- Volume up/down: one per press with a lock (`:492-499`). Volume click: short/long like PTT (`:500-505`, `:510-511`).

### 1b. RNS-E faceplate: `0x461` (RNS-E → Pi), **only while TV is the active source**

- DBC name `mRadio_MUX_1` (`dbc:1608`), DLC 6: `RA6_Ziel`, `RA6_Command`, `Operand_0..3`. This is the stock
  RNS-E → TV-tuner remote channel. Target `37` and command `30` are the TV tuner.
- Frame: `37 30 SS K1 K2 00`. `SS` `01` = pressed, about every 110 ms (3 frames for a tap, repeating while held).
  `04` = released, 5 frames about 110 ms apart (`TV_BUTTONS_0x461_R21.md`, `CAN_PROTOCOL.md` "0x461").
- Validation in the old code (`rns-e_can/tv_panel_controls.py:10-16`): ID 0x461, DLC 6, `37 30` prefix, byte 5 = 0,
  SS ∈ {1,4}, standard frame only. It is applied **only to the six r21 masks**. The older masks (knob, arrows etc.)
  are accepted with DLC ≥ 5 and no prefix check (`can_keyboard_control.py:399-410`).
- Key masks (K1,K2), with the meaning from Hudiy's key table (`references/hudiy references/hudiy-main/README.md:101-123`)
  and `config.json`:

| K1,K2 | Physical key (inferred) | Short | Long (5 frames ≈ 0.55 s) | Extended (30 frames ≈ 3.3 s) |
|---|---|---|---|---|
| `01,00` | prev / left arrow (track) | `KEY_V` previous media | `KEY_LEFT` | shell `save_logs.py` |
| `02,00` | next / right arrow | `KEY_N` next media | `KEY_RIGHT` | `sudo shutdown -h now` |
| `40,00` | Up | `KEY_UP` (→ knob CCW key inside our web apps) | — | — |
| `80,00` | Down | `KEY_DOWN` (→ knob CW key inside web apps) | `KEY_M` voice | — |
| `00,10` | knob push / ENTER | `KEY_ENTER` | — | `sudo reboot` |
| `00,40` | knob rotate (Hudiy "scroll left" `1`) | `KEY_1`, fires on **every** `01` frame | — | — |
| `00,20` | knob rotate (Hudiy "scroll right" `2`) | `KEY_2`, every `01` frame | — | — |
| `00,02` | RETURN | `KEY_ESC` | `KEY_0` | — |
| `00,01` | SETUP | `KEY_H` home | — | — |
| `08,00` | NAV (r21) | `KEY_F` nav | — | — |
| `00,08` | TEL (r21) | `KEY_G` phone | — | — |
| `00,04` | MEDIA (r21; was RADIO in r20d) | `KEY_J` media | — | — |
| `04,00` | NAME (r21) | Hudiy action `applications_menu` | — | — |
| `00,0C` | INFO (r21) | Hudiy action `hudiy_diagnostics` | — | — |
| `0C,00` | CAR (r21) | Hudiy action `hudiy_manager` | — | — |
| — | RADIO | not sent: it leaves TV (stock) | | |

- Knob rotation: the two directions are separate masks (`00,20` / `00,40`), and the old code dispatches on every
  press frame and resets the counter (`can_keyboard_control.py:422-425`, `scroll_commands` in config).
  **Unknown:** how many `01` frames one detent produces, and whether fast spins coalesce. No knob capture exists in
  the repo or tests. Which mask is CW is also only implied by the KEY_1/KEY_2 mapping.
- Press semantics (`can_keyboard_control.py:399-454`):
  - Cooldown: the first press frame of a new key within 0.2 s of the last fired action is dropped (`:414-417`,
    `cooldown_period` 0.2).
  - Long fires at frame 5, extended at frame 30, each once. Extended suppresses long (`:427-441`).
  - Short fires on the first `04` release if neither long nor extended fired (`:443-452`). Later release repeats are
    ignored because the counter is reset (`:454`).
  - A null long/extended mapping leaves short-on-release available even after a long hold
    (`TV_BUTTONS_HUDIY_MAPPING.md:32`).
  - **Workaround:** `(1,0)`/`(2,0)` releases within 0.25 s of a wheel scroll are dropped as "MFSW dominant"
    (`can_keyboard_control.py:446-447`). This was the stock wheel→0x461 echo, which r21 removed, so it is probably
    dead code now.
- Not visible to the Pi outside TV mode: **every** faceplate key and the knob. The RNS-E forwards them only while TV is
  the source. In radio, CD or other modes the RNS-E keeps them, and only the wheel (0x5C3) reaches the Pi.
- r52+/r55 note: outside Doom every key and knob push goes to stock firmware; only RETURN+SETUP is eaten as the Doom
  launch gesture (`R54B_R56B_FEATURES.md:184`).

### 1c. "Lower TV panel buttons"

These are the NAV/TEL/MEDIA/NAME/INFO/CAR/SETUP masks above, the same 0x461 envelope. There is no separate CAN ID.
RADIO always exits TV.

### 1d. Pi → RNS-E key injection (exists in firmware, unused by the repo)

`BE 01 kk` on 0x7B0 presses a panel key: 1 RADIO, 2 MEDIA, 3 TEL, 4 NAME, 5 NAV, 6 INFO, 7 CAR, 8 SETUP. `BE 01 00`
releases it and should follow 50–150 ms later (`CAN_PROTOCOL.md`, "BE 01 key codes"; r31+, present in r54b/r56b).
**Caveat:** BE does not wake the RNS-E's ACON. Commands queue until a key, a 0x5C3 frame, or about 5 s after wake
(`R54B_R56B_FEATURES.md:143`).

---

## 2. How inputs become actions today

Pipeline: `can_handler.py` reads SocketCAN and publishes ZMQ topics `CAN_XXX` on `ipc:///run/rnse_control/can_stream.ipc`.
`can_keyboard_control.py` subscribes to 0x461 / 0x5C3 / 0x661 (`:260-271`, gated by `features.mmi_controls`,
`mfsw_controls`, `source_controls`). From there the input goes one of four ways:

1. **uinput virtual keyboard** `can-virtual-keyboard` via python-uinput, `emit_click` = down+up with no hold
   (`can_keyboard_control.py:290-333`). It chmods `/dev/uinput` 666 with sudo (`:301-306`). Hudiy picks up keys as a
   normal keyboard.
2. **Hudiy actions**: a binding `{"action": name}` is published as `HUDIY_ACTION` on `input_control_stream.ipc`
   (`:349-370`). `hudiy_data.py:1402-1425` → `InputActionDispatcher` (`hudiy_client/input_actions.py`) →
   Hudiy API `DispatchAction`. The action is dropped if older than 1 s or from before the current API session
   (`input_actions.py:356-377`).
3. **Shell commands** (extended press only, needs `features.system_actions`): `subprocess.run(shell=True)`
   (`:389-396`).
4. **DIS semantic events**: in DIS-owner mode the wheel emits `DIS_INPUT` (`select`/`back`/`next`/`previous`) and
   `WHEEL_CONTROL` (`mode` heartbeat each 1 s) to dis_client (`:336-347`, `:569-571`; consumers
   `dis_client/dis_display.py:465`, `dis_top_display_service.py:896`).

Wheel map (`config.json` `mfsw`):

| Input | Normal owner | DIS owner |
|---|---|---|
| thumbwheel up/down | `KEY_V` / `KEY_N` (prev/next media) | `previous` / `next` |
| click short / long | `KEY_B` / `KEY_B` (play/pause) | single `select`, double `back` |
| MODE short / long / double | `KEY_ENTER` / `KEY_ESC` / toggle owner | toggle owner |
| PTT short / long | `KEY_M` (voice) / `KEY_H` (home) | same (not routed) |
| volume | nothing: the RNS-E does volume | same |

Source auto play/pause (disabled): on a 0x661 byte 3 change to/from `tv_mode_identifier` 0x37 it presses
`KEY_X`/`KEY_C` (`:520-533`).

---

## 3. Radio overrides (Pi → RNS-E)

All Pi TX goes `send_can_message` → ZMQ PUSH `can_send.ipc` → `can_handler` send worker, as standard 11-bit frames
(`can_base_function.py:315-321`, `can_handler.py:223-257`). Every override is gated: nothing is sent while
listen-only is on, pending or switching, or while Flashing Mode is on (`can_base_function.py:646-651`, `:659`).
Brightness, label and ADC also require `is_radio_active()`: NM 0x436 seen within 5 s with SleepInd = 0 (`:139-148`).

| What | ID / payload | When sent | If it stops |
|---|---|---|---|
| TV tuner emulation | `0x602`, DLC 8. Config `01 12 30 20 20 20 20 20`; code default `09 12 30 20 20 20 20 20` (`can_base_function.py:66`); firmware doc `01 12 30 20 00 00 00 00`. Byte 0 `01` = 480p60, `09` = 480i60. | Every 0.5 s when `features.tv_simulation.enabled`, not gated on radio-active (`:748-756`) | r30+: after about 2.2 s the firmware injects `[last b0 or 01] 12 30 20` internally, and TV stays listed (`CAN_PROTOCOL.md` "0x602"). Changing byte 0 re-initializes TV mode and **overwrites ADC regs 01–03** (`ADC_TUNING.md:17-19`). |
| Source label/icon | `0x7B0 BA MM 00 00 00 00 00 00`, MM 0 Hudiy, 1 CarPlay, 2 Android Auto (`rnse_control.py:51-53`) | Once per change, deduplicated (`:356-360`). Re-armed on radio wake (`force_reapply`, `:324-328`; NM handler `can_base_function.py:509-514`). Off by default (`rnse.source_label.enabled: false`). | Latched in RAM until an RNS-E cold boot, which resets to Hudiy. It repaints only at the next page lookup, with no forced repaint (`CAN_PROTOCOL.md` "BA"). |
| LCD brightness override | `0x7B0 BB SS 00…`, SS 0 = native (0x635 cluster dimmer + dial), 1–100 absolute; 1–5 act as 6 (r38 layout, `rnse_control.py:46-49`) | Same dedupe and wake re-arm as above. Day/night policy from 0x635 byte 1 > 0 (`can_base_function.py:430-447`). | Latched until cold boot. |
| Local dial | `0x7B0 B7 LL 00…`, LL 0–10, an offset of 2·LL−10 on the dimmer in native mode only (`rnse_control.py:42-44`) | Same as BB | **On the r52+ lineage, B7 is captured by the flash cave: `B7 00`–`03` overwrite B6 flash-descriptor slots and ≥ 04 is dropped** (`R54B_R56B_FEATURES.md`, B7 warning; read from the source, not bench-tested). The repo still sends B7 (manual 10 → `B7 0A`, harmless). Auto night = 1 or a slider at 0–3 would write a descriptor slot. |
| ADC tuning (AK5406) | `0x7B0 BC rr vv 00…` write, `BD 00…` dump. Replies on `0x7B6`: `57 rr bb ss` (W ack: 00 ok, EE rejected, FC readback failed) and an 8-frame `41 44 43 …` dump (`hudiy_manager/rnse_adc_protocol.py`, `ADC_TUNING.md`) | Only on an explicit user action, one in flight, 2 s reply timeout, 3 s dump timeout (`rnse_adc_protocol.py:219-243`) | Volatile: lost on reboot. 01–03 are also lost on a TV re-init. Nothing is reapplied automatically (`ADC_TUNING.md:15-22`). |
| Not used by the repo but in firmware | `B0` video-ready (r39 only, **absent** in r54b/r56b); `BE 02/06` volume abs/rel; `BE 03` mute; `BE 04 kk` select source (13 = TV); `BE 05` band; `BF` status → `53 grp src availLo availHi vol mute flags` on 0x7B6 (src `07` = TV) | — | — |

The old code has no heartbeat for BA/BB/B7, by design (`hudiy_manager/README.md:82-92`).
**Gap:** an RNS-E reboot without an NM sleep (for example a mid-drive crash or flash reset) loses the overrides
until the next value change. `BF` status polling or the 0x436 NM restart could detect it.

Brightness inputs: `rnse_control.py` treats 0x635 byte 1 > 0 as night with no validity check, while
`dark_mode_api.py:245-254` treats bit 7 or `7F` as invalid and needs 4 s of "off" before switching to day
(`DAY_CONFIRM`, `:242`). The two consumers of the same signal disagree.

Other Pi → RNS-E traffic outside this area: the DIS transport remap (0x7B3) and the B1/B8/B9/B3–B7 flash and debug
commands in `avsd/app`.

---

## 4. Context the old code uses to route keys

- **On TV or not:** never computed for routing. Faceplate keys arrive only in TV, so TV is implicit. Wheel keys are
  always sent. The only explicit TV check is the disabled 0x661 byte-3 == 0x37 play/pause. That is a guess: per the
  DBC, byte 3 of 0x661 is `RA4_Radio_Para2`, and the firmware doc names TV modes `0x38/0x47` on another frame
  (`TV_BUTTONS_HUDIY_MAPPING.md:44`). Firmware `BF` status (`src 07`) would give a real answer.
- **Hudiy screen** (`HUDIY_UI`, every 2 s, 5 s expiry): `menu_action` + `projection_active` (`hudiy_data.py:202-218`).
  It is used only to swap Up/Down to knob keys inside our web apps, because Hudiy eats Up/Down
  (`can_keyboard_control.py:373-386`, `WEB_APP_ARROWS :37`).
- **Wheel owner**: `normal` (keys to Hudiy) or `dis` (semantic events to DIS apps), from `DIS_DISPLAY_STATUS`
  (app, READY, `wheel_supported`, `control_epoch`), `DIS_TOP_STATUS`, and `HUDIY_PHONE` call state. It falls back to
  normal if context is stale for more than 3 s (`wheel_controls.py:132-144`). Optional auto-takeover during calls
  (`display.phone.scroll_wheel_phone_menu`, `:54-64`), and an app or stalk change during a call hands the wheel back
  (`:94-98`).
- **Projection provider** for the BA label: inferred from Hudiy media/nav source IDs and remapped 1→2, 2→1
  (`hudiy_client/projection_source.py:29-30`). `connection_known` is always False. The lease expires after 6 s without
  a snapshot (`rnse_bridge_runtime.py:285-322`).
- **Radio awake** (NM 0x436) gates every override TX.

---

## 5. Guesswork, workarounds, dead ends

1. Wheel hold repeat = 500 ms and long = 3 frames: an assumption from the DBC, not a capture (`can_keyboard_control.py:189`).
2. The 0x5C3 byte 0 (`39`/`3A`) meaning is unknown and ignored.
3. The `0x01` "MODE double-click" is a gateway quirk captured once (`tests/test_dis_readings_controls.py:412`).
4. Knob detent → frame count is unknown. Each `01` frame = one scroll.
5. MFSW-dominant dedupe for `(1,0)`/`(2,0)` (`can_keyboard_control.py:446`) is for the stock wheel echo that r21
   removed. Probably dead.
6. Faceplate validation is strict only for r21 masks. Old masks accept any DLC ≥ 5 frame on 0x461.
7. The 0x661 TV detection byte and value are a guess (disabled).
8. The source label uses a provider *hint* (last media/nav source), not connection state. The ID swap is hand-coded.
9. **B7 dial is dead and hazardous on r52+ firmware** (flash descriptor slots).
10. 0x602 payload disagrees three ways: config `…20 20 20 20`, code default `09…` (480i), doc `…00 00 00 00`.
11. The Up/Down → knob-key swap in web apps works around Hudiy eating Up/Down. Not needed with LIVI.
12. Config `mfsw.phone_alt` is parsed nowhere. `release` lists `0x00` twice.
13. uinput permissions are fixed with `sudo chmod 666` at runtime (`can_keyboard_control.py:301-306`).
14. Wheel keys fire while on FM/CD too (no source gating). Possible double action.
15. No reassertion of BA/BB after an RNS-E reboot that isn't preceded by NM sleep.

---

## 6. LIVI input today (commit bfbea31, scratchpad `livi-input/native/crates`)

- **Keyboard path:** in DRM mode the compositor reads evdev through libinput (`livi-compositor/src/drm/libinput.rs:131-220`),
  so a uinput virtual keyboard would be picked up. In host mode (inside labwc, Drew's planned setup) it reads the
  host Wayland seat's keyboard (`livi-compositor/src/host/input.rs:48-51`), so uinput → labwc → focused LIVI window.
  Key repeat is set to 600 ms delay and 25/s.
- **Keymap:** `config.json` `bindings`, one DOM-style code per action (`"KeyH"`, `"ArrowUp"`, `""` = unbound). Defaults are
  at `livi-core-proto/src/config.rs:582-623`: up/down/left/right arrows, selectDown Enter, back Backspace, knob*
  unbound, home KeyH, playPause KeyP, next KeyN, prev KeyB, acceptPhone KeyA, rejectPhone KeyR, voiceAssistant KeyV,
  cycleSession KeyS, and so on. Bindings are editable in Settings → Key Bindings (`livi-ui-model/src/settings/schemas/general.rs:475-510`).
- **UI decision:** `livi-ui-model/src/keys.rs:91+` (`decide`). Navigation bindings go to the core only while projection
  is streaming and focused. Otherwise they drive LIVI's own menus. Non-navigation bindings (home, media, phone, voice)
  always go to the core.
- **Core → AA:** `livi-core/src/projection.rs:252` (`bound`) and `projection/input.rs:86-110,185-215` (`aa_key`).
  Only key-down is acted on, as a click (down+up). The only exception is voice assistant, which has a release.
  Left/Right and KnobLeft/KnobRight become **AA rotary** `RelativeEvent{keycode: ROTARY_CONTROLLER, delta ±1}`
  (`livi-aa-stack/src/channels/input.rs:58-70`, `commands.rs:124-125`). Up/Down are DPAD up/down.
  SelectDown/KnobDown are DPAD_CENTER. Media, call and voice map to their AA keycodes. `long_press` is always false,
  so AA never sees a long press. With the projection not in front, only home, media, call and voice keys pass
  (`works_behind`, `projection.rs:225-238`). With a radio source active, media keys go to LIVI's FM/DAB instead.
- **Open app commands** exist: `OpenNavigation`/`OpenMedia`/`OpenPhone`/`OpenRadio` (`aa_open`, `projection.rs:339-347`).
  They are reached via `open_app(target)`, not via key bindings, so NAV/TEL/MEDIA shortcuts would need a core action
  or a fork.
- **External injection without uinput:**
  - Core socket `<runtime>/core.sock` (`livi-core/src/paths.rs:42`, `lib.rs:170-174`). It takes u32-LE-length-prefixed
    JSON (`livi-core-proto/src/frame.rs`) and only peers with the **same uid** (`server.rs:364-368`). The first message
    must be `{"type":"hello","protocol":N,"client":"…"}`. After that, `{"type":"input","input":{"kind":"key","code":"KeyN","down":true}}`
    goes through the same binding table, and actions like `{"type":"action","id":1,"action":{"kind":"media","control":"next"}}`
    also work (`message.rs:14-22,55-170`). Limitation: keys sent here skip the UI's `decide`, so they reach
    AA/CarPlay but **cannot navigate LIVI's own menus**. The protocol is internal and versioned (Hello is refused on a
    mismatch).
  - "LIVI Bridge" USB CDC-ACM serial (`livi-core/src/car_bridge.rs`). It auto-finds a ttyACM with that product
    string. Lines in: `button next|previous|scan` and `car <key> <value>`. Lines out: media and nav text. Only
    3 buttons.
- There is no long-press, hold or double-click logic in LIVI, so the car daemon must do gesture recognition and send
  plain clicks or distinct keys.

---

## Questions only Drew can answer

See the summary returned to the parent agent. The same list:

1. Which firmware is actually installed now (r52+ lineage with the flash cave)? Is B7 really dead there, and should
   the rewrite drop the dial entirely in favour of BB?
2. Should wheel up/down, click and PTT go to AA only while TV is the RNS-E source, or always (today: always)?
3. Do you want the Pi to drive the RNS-E (BE 04 13 to switch to TV on AA connect, BE volume, BF status polling)?
4. Knob: how many 0x461 frames per detent, and which mask is CW? Is a candump of a slow and a fast spin available?
5. Wheel hold timing: is 500 ms repeat / 3 frames for long press right, or a guess? Is a candump of a held MODE/PTT
   available?
6. 0x602: 480p (`01`) or 480i (`09`), and what tail bytes? Is the Pi's 0x602 even needed with r30+ self-injection?
7. Should BA/BB be reasserted periodically or after BF shows a reset, rather than only on NM wake?
8. LIVI input route: uinput keyboard (works for LIVI menus) vs core.sock (projection only) vs forking LIVI to add a
   proper car-input socket with long press and open-app?
9. Faceplate long and extended actions (shutdown, reboot, save logs): keep any of them?
10. Is DIS-owner wheel mode (MODE double-click to drive DIS apps) carried into the rewrite?
