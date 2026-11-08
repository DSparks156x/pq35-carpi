//! Desired-state client for the RNS-E firmware's 0x7B0 commands.
//!
//! Protocol: `RNSE hacking/CAN_PROTOCOL.md` and `R54B_R56B_FEATURES.md`
//! (the car runs the R56B lineage). Every 0x7B0 frame is DLC 8, byte 0 the
//! opcode. Nothing is saved on the RNS-E: settings are RAM only and reset on
//! a cold boot, and a frame sent before its CAN receiver is up is lost.
//!
//! So this keeps a desired state and re-sends it whenever the RNS-E node
//! comes back on the NM ring (after being gone, or out of sleep). It waits
//! `settle` first, then sends again at each of `resend` after that, since
//! when the receiver comes up relative to the NM frame isn't known.
//!
//! Only two commands are ever built:
//! - `BA MM`: projection label/icon.
//! - `BB SS`: LCD brightness override.
//!
//! **`B7` is never sent.** On this lineage the flash cave owns `B3`-`B7`, and
//! `B7 00`-`03` overwrite flash descriptor slots. The old dial is dead anyway.
//! `B8`/`B9` (Doom, RAM write on r56b) aren't built either.
//!
//! Not implemented, but available on the firmware:
//! - `BE 01 kk` panel key press/release, `BE 02`/`06` volume, `BE 03` mute,
//!   `BE 04 kk` select source (13 = TV), `BE 05` band. BE doesn't wake the
//!   RNS-E's ACON; commands queue until a key, a 0x5C3 frame or ~5 s after wake.
//! - `BF` status request: one `53 grp src availLo availHi vol mute flags`
//!   reply on 0x7B6 (src `07` = TV, flags bit 7 = last BE rejected). It
//!   doesn't report BA/BB, so it can't confirm our state; it could detect a
//!   reset only indirectly.
//!
//! The Pi doesn't send 0x602; the firmware feeds itself.
//!
//! Sans-IO: outputs are frames tagged [`TxClass::Override`]; the caller puts
//! them through the gate.

use carpi_can::Frame;
use carpi_gate::TxClass;
use carpi_signals::ids;
use std::time::{Duration, Instant};

const OP_LABEL: u8 = 0xBA;
const OP_BRIGHTNESS: u8 = 0xBB;

/// `BA` label/icon. Three slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceLabel {
    /// Slot 0: LIVI, the label when nothing is projecting, and the boot
    /// default. Current firmware still shows "Hudiy" here until Drew renames it.
    Livi = 0,
    CarPlay = 1,
    AndroidAuto = 2,
}

/// `BB` LCD brightness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Brightness {
    /// `BB 00`: cluster dimmer (0x635) plus the RNS-E's own dial.
    FollowCar,
    /// `BB 01`-`64`: absolute level 1-100. 1-5 act as the stock minimum 6.
    /// Out-of-range values are clamped.
    Level(u8),
}

/// One frame to send, already tagged for the gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Command {
    pub class: TxClass,
    pub frame: Frame,
}

fn command(op: u8, arg: u8) -> Command {
    debug_assert!(op == OP_LABEL || op == OP_BRIGHTNESS);
    Command {
        class: TxClass::Override,
        frame: Frame::new(ids::RNSE_CMD, &[op, arg, 0, 0, 0, 0, 0, 0]).expect("valid frame"),
    }
}

pub fn encode_label(l: SourceLabel) -> Command {
    command(OP_LABEL, l as u8)
}

pub fn encode_brightness(b: Brightness) -> Command {
    let ss = match b {
        Brightness::FollowCar => 0,
        Brightness::Level(n) => n.clamp(1, 100),
    };
    command(OP_BRIGHTNESS, ss)
}

#[derive(Clone, Debug)]
pub struct Config {
    /// Wait after the RNS-E appears before the first send.
    pub settle: Duration,
    /// Further full re-sends, measured from the first one.
    pub resend: Vec<Duration>,
    /// Re-send everything this often while awake. `None` = only on change
    /// and on reappearance (the firmware wants one frame per action).
    pub refresh: Option<Duration>,
    /// Retry delay after the gate refused a command.
    pub retry: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            settle: Duration::from_secs(1),
            resend: vec![Duration::from_secs(5)],
            refresh: None,
            retry: Duration::from_secs(1),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Slot<T> {
    want: Option<T>,
    /// Last value sent since the RNS-E appeared.
    sent: Option<T>,
}

impl<T: Copy + PartialEq> Slot<T> {
    const EMPTY: Self = Slot {
        want: None,
        sent: None,
    };

    fn pending(&self) -> Option<T> {
        self.want.filter(|w| self.sent != Some(*w))
    }
}

pub struct Client {
    cfg: Config,
    label: Slot<SourceLabel>,
    brightness: Slot<Brightness>,
    awake: bool,
    /// Nothing is sent before this (settle or retry).
    hold_until: Option<Instant>,
    /// Scheduled full re-sends.
    resend_at: Vec<Instant>,
    refresh_at: Option<Instant>,
}

impl Client {
    pub fn new(cfg: Config) -> Self {
        Client {
            cfg,
            label: Slot::EMPTY,
            brightness: Slot::EMPTY,
            awake: false,
            hold_until: None,
            resend_at: Vec::new(),
            refresh_at: None,
        }
    }

    /// `None` = leave the RNS-E's own setting alone.
    pub fn set_label(&mut self, l: Option<SourceLabel>) {
        self.label.want = l;
    }

    pub fn set_brightness(&mut self, b: Option<Brightness>) {
        self.brightness.want = b;
    }

    pub fn label(&self) -> Option<SourceLabel> {
        self.label.want
    }

    pub fn brightness(&self) -> Option<Brightness> {
        self.brightness.want
    }

    /// Call on every tick and after any change. `rnse_awake` is the RNS-E
    /// node present on the ring and not asking to sleep. Returns frames to
    /// put through the gate.
    pub fn update(&mut self, now: Instant, rnse_awake: bool) -> Vec<Command> {
        if !rnse_awake {
            // Gone or asleep: assume it forgets everything.
            self.awake = false;
            self.forget();
            self.hold_until = None;
            self.resend_at.clear();
            self.refresh_at = None;
            return Vec::new();
        }
        if !self.awake {
            self.awake = true;
            let first = now + self.cfg.settle;
            self.hold_until = Some(first);
            self.resend_at = self.cfg.resend.iter().map(|d| first + *d).collect();
            return Vec::new();
        }
        if self.hold_until.is_some_and(|t| now < t) {
            return Vec::new();
        }
        self.hold_until = None;

        let before = self.resend_at.len();
        self.resend_at.retain(|t| now < *t);
        let refresh_due = self.refresh_at.is_some_and(|t| now >= t);
        if self.resend_at.len() != before || refresh_due {
            self.forget();
        }

        let mut out = Vec::new();
        if let Some(l) = self.label.pending() {
            self.label.sent = Some(l);
            out.push(encode_label(l));
        }
        if let Some(b) = self.brightness.pending() {
            self.brightness.sent = Some(b);
            out.push(encode_brightness(b));
        }
        if refresh_due || (self.refresh_at.is_none() && !out.is_empty()) {
            self.refresh_at = self.cfg.refresh.map(|d| now + d);
        }
        out
    }

    /// The gate refused `cmd`: send it again after `retry`.
    pub fn refused(&mut self, cmd: &Command, now: Instant) {
        match cmd.frame.data()[0] {
            OP_LABEL => self.label.sent = None,
            OP_BRIGHTNESS => self.brightness.sent = None,
            _ => return,
        }
        if self.awake {
            self.hold_until = Some(now + self.cfg.retry);
        }
    }

    /// When `update` next has something to do, if anything.
    pub fn next_deadline(&self) -> Option<Instant> {
        if !self.awake {
            return None;
        }
        [
            self.hold_until,
            self.resend_at.iter().min().copied(),
            self.refresh_at,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    fn forget(&mut self) {
        self.label.sent = None;
        self.brightness.sent = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn bytes(cmds: &[Command]) -> Vec<Vec<u8>> {
        cmds.iter().map(|c| c.frame.data().to_vec()).collect()
    }

    #[test]
    fn encoding() {
        let c = encode_label(SourceLabel::AndroidAuto);
        assert_eq!(c.class, TxClass::Override);
        assert_eq!(c.frame.id, 0x7B0);
        assert_eq!(c.frame.data(), &[0xBA, 0x02, 0, 0, 0, 0, 0, 0]);
        let d = |b| encode_brightness(b).frame.data().to_vec();
        assert_eq!(d(Brightness::FollowCar), [0xBB, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(d(Brightness::Level(100))[1], 0x64);
        assert_eq!(d(Brightness::Level(250))[1], 0x64);
        assert_eq!(d(Brightness::Level(0))[1], 0x01);
    }

    #[test]
    fn sends_after_settle_then_resend_then_on_change() {
        let t0 = Instant::now();
        let mut c = Client::new(Config::default());
        c.set_label(Some(SourceLabel::AndroidAuto));
        c.set_brightness(Some(Brightness::Level(40)));

        assert!(c.update(t0, false).is_empty());
        assert_eq!(c.next_deadline(), None);
        // RNS-E appears: nothing until settle.
        assert!(c.update(t0, true).is_empty());
        assert_eq!(c.next_deadline(), Some(t0 + ms(1000)));
        assert!(c.update(t0 + ms(999), true).is_empty());
        let out = c.update(t0 + ms(1000), true);
        assert_eq!(
            bytes(&out),
            [[0xBA, 2, 0, 0, 0, 0, 0, 0], [0xBB, 40, 0, 0, 0, 0, 0, 0]]
        );
        // Deduplicated.
        assert!(c.update(t0 + ms(1100), true).is_empty());
        // Scheduled re-send.
        assert_eq!(c.next_deadline(), Some(t0 + ms(6000)));
        assert_eq!(c.update(t0 + ms(6000), true).len(), 2);
        assert_eq!(c.next_deadline(), None);
        // A change goes out at once, alone.
        c.set_brightness(Some(Brightness::FollowCar));
        assert_eq!(
            bytes(&c.update(t0 + ms(7000), true)),
            [[0xBB, 0, 0, 0, 0, 0, 0, 0]]
        );
        // Unset = leave alone.
        c.set_label(None);
        assert!(c.update(t0 + ms(7100), true).is_empty());
    }

    #[test]
    fn reasserts_after_gone_or_sleep() {
        let t0 = Instant::now();
        let mut c = Client::new(Config {
            resend: vec![],
            ..Config::default()
        });
        c.set_label(Some(SourceLabel::CarPlay));
        c.update(t0, true);
        assert_eq!(c.update(t0 + ms(1000), true).len(), 1);
        assert!(c.update(t0 + ms(2000), true).is_empty());
        // Gone (or SleepInd), then back: re-sent after settle.
        assert!(c.update(t0 + ms(3000), false).is_empty());
        assert!(c.update(t0 + ms(4000), true).is_empty());
        assert!(c.update(t0 + ms(4500), true).is_empty());
        assert_eq!(
            bytes(&c.update(t0 + ms(5000), true)),
            [[0xBA, 1, 0, 0, 0, 0, 0, 0]]
        );
    }

    #[test]
    fn refused_retries() {
        let t0 = Instant::now();
        let mut c = Client::new(Config {
            resend: vec![],
            ..Config::default()
        });
        c.set_brightness(Some(Brightness::Level(10)));
        c.update(t0, true);
        let out = c.update(t0 + ms(1000), true);
        c.refused(&out[0], t0 + ms(1000));
        assert!(c.update(t0 + ms(1500), true).is_empty());
        assert_eq!(c.next_deadline(), Some(t0 + ms(2000)));
        assert_eq!(c.update(t0 + ms(2000), true), out);
    }

    #[test]
    fn refresh() {
        let t0 = Instant::now();
        let mut c = Client::new(Config {
            resend: vec![],
            refresh: Some(Duration::from_secs(60)),
            ..Config::default()
        });
        c.set_label(Some(SourceLabel::Livi));
        c.update(t0, true);
        let t1 = t0 + ms(1000);
        assert_eq!(c.update(t1, true).len(), 1);
        assert_eq!(c.next_deadline(), Some(t1 + ms(60_000)));
        assert!(c.update(t1 + ms(59_999), true).is_empty());
        assert_eq!(c.update(t1 + ms(60_000), true).len(), 1);
    }

    #[test]
    fn gate_accepts_output() {
        use carpi_gate::{BusView, Config as GateConfig, Gate};
        let t0 = Instant::now();
        let mut g = Gate::new("ican", GateConfig::default());
        let v = BusView {
            bus: carpi_nm::BusState::Awake,
            rnse_awake: true,
            kl15: None,
        };
        for cmd in [
            encode_label(SourceLabel::AndroidAuto),
            encode_brightness(Brightness::FollowCar),
            encode_brightness(Brightness::Level(100)),
        ] {
            assert_eq!(g.check(cmd.class, &cmd.frame, t0, &v), Ok(()));
        }
    }

    /// Whatever is asked for, in any order, only BA/BB with valid arguments
    /// come out: never B7 or anything else.
    #[test]
    fn never_emits_b7() {
        let t0 = Instant::now();
        let mut c = Client::new(Config {
            refresh: Some(ms(300)),
            resend: vec![ms(100), ms(700)],
            ..Config::default()
        });
        let mut seed = 0x1234_5678_9ABC_DEF1u64;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let labels = [
            None,
            Some(SourceLabel::Livi),
            Some(SourceLabel::CarPlay),
            Some(SourceLabel::AndroidAuto),
        ];
        let mut t = t0;
        let mut seen = 0;
        for _ in 0..20_000 {
            t += ms(rnd() % 400);
            match rnd() % 4 {
                0 => c.set_label(labels[(rnd() % 4) as usize]),
                1 => {
                    let b = match rnd() % 3 {
                        0 => None,
                        1 => Some(Brightness::FollowCar),
                        _ => Some(Brightness::Level(rnd() as u8)),
                    };
                    c.set_brightness(b)
                }
                _ => {}
            }
            let out = c.update(t, !rnd().is_multiple_of(8));
            for cmd in &out {
                seen += 1;
                let d = cmd.frame.data();
                assert_eq!(cmd.class, TxClass::Override);
                assert_eq!(cmd.frame.id, 0x7B0);
                assert_eq!(d.len(), 8);
                assert_ne!(d[0], 0xB7);
                match d[0] {
                    0xBA => assert!(d[1] <= 2),
                    0xBB => assert!(d[1] <= 100),
                    op => panic!("unexpected opcode {op:02X}"),
                }
                assert_eq!(&d[2..], &[0; 6]);
                if rnd().is_multiple_of(10) {
                    c.refused(cmd, t);
                }
            }
        }
        assert!(seen > 1000);
    }
}
