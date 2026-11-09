# Remaining work

This is the short acceptance list after the 2026-11-09 car test. PLAN.md describes the
architecture and build order; older implementation-status notes in it and README.md are not
an up-to-date checklist. In particular, the brightness curves/learning, Data pages, logging,
web setup, signing, NVMe move and SDLink integration already have implementations.

## Next acceptance run: no manual repairs

- [ ] Setup appears first in main LIVI Settings, hides on completion, and can be restored
  from Car Integration > Advanced. Its checklist and Phone connection return navigation work
  with both touch and the knob.
- [ ] Ship and live-test the final update confirmation correction. Passive status checks do
  not invent a pending update; an installed successful release is confirmed without a false
  failure label. Real rollback/failure remains visible.
- [ ] Fresh desktop Trixie SD, configured by the web tool with Beta selected: first boot
  handles time synchronization, packages, release download, service ownership and reboot
  without terminal edits.
- [ ] The selected 480/SDLink output starts LIVI fullscreen at 800x480, and the wireless
  phone settings are discoverable through Setup.
- [ ] SDLink video/audio works in the car from that fresh install, with no environment-file
  edits. This has worked after the current install's one-time repair; the untouched path
  still needs its acceptance run.
- [ ] Move to NVMe through the UI shows progress, reboots onto NVMe, and leaves a working
  rescue SD. The copy/reboot has succeeded on hardware; repeat with the corrected UI.
- [ ] From that install, apply a newer signed beta through the Update button. Check the
  final version/result and SDLink/phone reconnection without an SSH workaround.

## Car and installation coverage

- [ ] Repeated cold starts, ignition off, bus sleep, safe shutdown and wake/power cycles.
  Confirm the Pi does not keep the car bus awake.
- [ ] Automatic phone reconnection and recovery after Wi-Fi update handoffs, including a
  missing saved network or failed download.
- [x] DIS and diagnostics worked in Drew's car (confirmed 2026-11-09). Longer concurrency/rate
  tests or a separate OBD adapter are additional coverage, not missing implementations.
- [ ] Complete a logging/capture session and retrieve it through the phone portal. These
  workflows are implemented; the end-to-end car acceptance matters more than new screens.
- [ ] Brightness/learning and input holds/knob behavior over a representative drive.
- [ ] HDMI 480i/480p and touch/no-touch installation coverage in addition to Drew's SDLink
  configuration. Retest an updated release as well as a fresh card.
- [ ] Choose a first stable release after the acceptance runs, rather than treating every
  development beta as hardware-validated.

## Remaining features

- Teensy firmware builds and a confirmed update workflow; Pi-driven partial RNS-E firmware
  updates require separate RNS-E/SDLink design work. Current bundle metadata is not flashing.
- UI polish from Drew's next review; keep a concrete list of observed issues rather than
  assuming the older page build-order notes are unfinished features.
- Haldex/exhaust extras and the planned hat MCU/backend.
- Lite-image support with an explicit dependency set; fully offline first boot.
- Optional desktop on leaving LIVI.

Boot-time tuning has met the original under-12-second target and reached approximately
10 seconds to paging on the tuned NVMe installation. Further boot micro-optimizations are
not a prerequisite for the acceptance run.
