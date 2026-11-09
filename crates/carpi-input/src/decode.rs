//! Raw frames to key edges.
//!
//! Wheel (0x5C3): byte 1 is the command, byte 0 is ignored. A held key
//! repeats its press frame (every ~500 ms per the DBC cycle time; no capture),
//! `00` releases. The TT gateway sends a second MODE press as `01`.
//!
//! Faceplate (0x461): `37 30 SS K1 K2 00`. `SS` 01 = pressed, repeated every
//! ~110 ms; 04 = released, sent five times. Knob rotation is two masks, one per
//! direction, and every press frame counts.
//!
//! Stalk rocker (0x2C1): byte 2 bit 0x20 up, 0x10 down, held while set; a
//! frame without either releases.
//!
//! Releases that never arrive are synthesised after a timeout.

use crate::{Edge, Key, KeyEvent};
use carpi_signals::ids;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug)]
pub struct DecoderConfig {
    /// Wheel key with no frame for this long is released. Must exceed the
    /// hold repeat interval.
    pub wheel_release_timeout: Duration,
    /// Same for faceplate keys (repeat ~110 ms).
    pub panel_release_timeout: Duration,
    /// A wheel `01` is a MODE press only this soon after a MODE release (or
    /// while MODE is down). Stray `01`s are ignored.
    pub mode_again_window: Duration,
    /// Knob press frames per detent. 1 until a capture says otherwise.
    pub knob_frames_per_detent: u8,
}

impl Default for DecoderConfig {
    fn default() -> Self {
        DecoderConfig {
            // Three missed 500 ms cycles.
            wheel_release_timeout: Duration::from_millis(1500),
            // Three missed 110 ms repeats, plus slack.
            panel_release_timeout: Duration::from_millis(400),
            mode_again_window: Duration::from_millis(1000),
            knob_frames_per_detent: 1,
        }
    }
}

enum WheelCmd {
    Press(Key),
    ModeAgain,
    Release,
    Unknown,
}

fn wheel_cmd(b: u8) -> WheelCmd {
    match b {
        0x0B => WheelCmd::Press(Key::WheelUp),
        0x0C => WheelCmd::Press(Key::WheelDown),
        0x08 => WheelCmd::Press(Key::WheelClick),
        0x1C => WheelCmd::Press(Key::Mode),
        0x2A => WheelCmd::Press(Key::Ptt),
        0x06 => WheelCmd::Press(Key::VolUp),
        0x07 => WheelCmd::Press(Key::VolDown),
        0xA7 => WheelCmd::Press(Key::VolClick),
        0x01 => WheelCmd::ModeAgain,
        0x00 => WheelCmd::Release,
        _ => WheelCmd::Unknown,
    }
}

enum PanelCmd {
    Button(Key),
    /// +1 clockwise, -1 counter-clockwise.
    Knob(i8),
}

/// Whole (K1, K2) pairs, not bits: INFO `00 0C` is its own key.
fn panel_cmd(k1: u8, k2: u8) -> Option<PanelCmd> {
    use PanelCmd::*;
    Some(match (k1, k2) {
        (0x01, 0x00) => Button(Key::Prev),
        (0x02, 0x00) => Button(Key::Next),
        (0x40, 0x00) => Button(Key::Up),
        (0x80, 0x00) => Button(Key::Down),
        (0x00, 0x10) => Button(Key::Enter),
        (0x00, 0x02) => Button(Key::Return),
        (0x00, 0x01) => Button(Key::Setup),
        (0x08, 0x00) => Button(Key::Nav),
        (0x00, 0x08) => Button(Key::Tel),
        (0x00, 0x04) => Button(Key::Media),
        (0x04, 0x00) => Button(Key::Name),
        (0x00, 0x0C) => Button(Key::Info),
        (0x0C, 0x00) => Button(Key::Car),
        // Old code: 00 20 = "scroll right", 00 40 = "scroll left". Direction
        // is inferred, not captured.
        (0x00, 0x20) => Knob(1),
        (0x00, 0x40) => Knob(-1),
        _ => return None,
    })
}

/// Whether a key frame is one the decoder knows: `false` for a 0x461 press
/// or 0x5C3 command it would drop. For logging codes the car sends that
/// aren't mapped yet.
pub fn known_key_frame(id: u32, data: &[u8]) -> bool {
    match id {
        ids::MFL_CMD => data.len() < 2 || !matches!(wheel_cmd(data[1]), WheelCmd::Unknown),
        // Byte 2 with anything but the rocker bits: logged, to learn the rest.
        ids::STALK_KEYS => data.get(2).is_none_or(|b| b & !0xB0 == 0),
        ids::RNSE_KEYS => {
            data.len() < 5
                || data[0] != 0x37
                || data[1] != 0x30
                || data[2] != 0x01
                || panel_cmd(data[3], data[4]).is_some()
        }
        _ => true,
    }
}

/// A key that is down and when its last frame arrived.
#[derive(Clone, Copy)]
struct Held {
    key: Key,
    last: Instant,
}

pub struct Decoder {
    cfg: DecoderConfig,
    wheel: Option<Held>,
    panel: Option<Held>,
    stalk: Option<Held>,
    last_mode_up: Option<Instant>,
    knob_acc: i16,
}

impl Decoder {
    pub fn new(cfg: DecoderConfig) -> Self {
        Decoder {
            cfg,
            wheel: None,
            panel: None,
            stalk: None,
            last_mode_up: None,
            knob_acc: 0,
        }
    }

    /// Feed every frame; ones that aren't key frames are ignored.
    pub fn on_frame(&mut self, at: Instant, id: u32, data: &[u8]) -> Vec<KeyEvent> {
        let mut out = self.tick(at);
        match id {
            ids::MFL_CMD => self.wheel_frame(at, data, &mut out),
            ids::RNSE_KEYS => self.panel_frame(at, data, &mut out),
            ids::STALK_KEYS => self.stalk_frame(at, data, &mut out),
            _ => {}
        }
        out
    }

    /// Releases keys whose frames stopped.
    pub fn tick(&mut self, now: Instant) -> Vec<KeyEvent> {
        let mut out = Vec::new();
        if let Some(h) = self.wheel
            && now.saturating_duration_since(h.last) >= self.cfg.wheel_release_timeout
        {
            self.release_wheel(now, &mut out);
        }
        if let Some(h) = self.panel
            && now.saturating_duration_since(h.last) >= self.cfg.panel_release_timeout
        {
            self.panel = None;
            out.push(ev(h.key, Edge::Up, now));
        }
        if let Some(h) = self.stalk
            && now.saturating_duration_since(h.last) >= self.cfg.wheel_release_timeout
        {
            self.stalk = None;
            out.push(ev(h.key, Edge::Up, now));
        }
        out
    }

    /// When `tick` next has something to do.
    pub fn next_deadline(&self) -> Option<Instant> {
        let w = self.wheel.map(|h| h.last + self.cfg.wheel_release_timeout);
        let p = self.panel.map(|h| h.last + self.cfg.panel_release_timeout);
        let s = self.stalk.map(|h| h.last + self.cfg.wheel_release_timeout);
        [w, p, s].into_iter().flatten().min()
    }

    fn wheel_frame(&mut self, at: Instant, data: &[u8], out: &mut Vec<KeyEvent>) {
        if data.len() < 2 {
            return;
        }
        match wheel_cmd(data[1]) {
            WheelCmd::Press(k) => self.wheel_press(at, k, out),
            WheelCmd::ModeAgain => {
                let held_mode = self.wheel.is_some_and(|h| h.key == Key::Mode);
                let recent = self
                    .last_mode_up
                    .is_some_and(|t| at.saturating_duration_since(t) <= self.cfg.mode_again_window);
                if held_mode || (self.wheel.is_none() && recent) {
                    self.wheel_press(at, Key::Mode, out);
                }
            }
            // Unknown command: whatever was held isn't any more.
            WheelCmd::Release | WheelCmd::Unknown => self.release_wheel(at, out),
        }
    }

    fn wheel_press(&mut self, at: Instant, key: Key, out: &mut Vec<KeyEvent>) {
        match self.wheel {
            Some(h) if h.key == key => out.push(ev(key, Edge::Repeat, at)),
            Some(_) => {
                self.release_wheel(at, out);
                out.push(ev(key, Edge::Down, at));
            }
            None => out.push(ev(key, Edge::Down, at)),
        }
        self.wheel = Some(Held { key, last: at });
    }

    fn release_wheel(&mut self, at: Instant, out: &mut Vec<KeyEvent>) {
        if let Some(h) = self.wheel.take() {
            if h.key == Key::Mode {
                self.last_mode_up = Some(at);
            }
            out.push(ev(h.key, Edge::Up, at));
        }
    }

    fn stalk_frame(&mut self, at: Instant, data: &[u8], out: &mut Vec<KeyEvent>) {
        let Some(&b) = data.get(2) else {
            return;
        };
        let key = if b & 0x20 != 0 {
            Some(Key::StalkUp)
        } else if b & 0x10 != 0 {
            Some(Key::StalkDown)
        } else {
            None
        };
        match (self.stalk, key) {
            (Some(h), Some(k)) if h.key == k => out.push(ev(k, Edge::Repeat, at)),
            (held, Some(k)) => {
                if let Some(h) = held {
                    out.push(ev(h.key, Edge::Up, at));
                }
                out.push(ev(k, Edge::Down, at));
            }
            (Some(h), None) => out.push(ev(h.key, Edge::Up, at)),
            (None, None) => {}
        }
        self.stalk = key.map(|key| Held { key, last: at });
    }

    fn panel_frame(&mut self, at: Instant, data: &[u8], out: &mut Vec<KeyEvent>) {
        if data.len() < 5 || data[0] != 0x37 || data[1] != 0x30 {
            return;
        }
        let (ss, k1, k2) = (data[2], data[3], data[4]);
        let Some(cmd) = panel_cmd(k1, k2) else {
            return;
        };
        match (cmd, ss) {
            (PanelCmd::Knob(dir), 0x01) => {
                if self.knob_acc.signum() != i16::from(dir) {
                    self.knob_acc = 0;
                }
                self.knob_acc += i16::from(dir);
                let per = i16::from(self.cfg.knob_frames_per_detent.max(1));
                if self.knob_acc.abs() >= per {
                    self.knob_acc = 0;
                    out.push(ev(Key::Knob(dir), Edge::Down, at));
                }
            }
            (PanelCmd::Knob(_), 0x04) => self.knob_acc = 0,
            (PanelCmd::Button(k), 0x01) => {
                match self.panel {
                    Some(h) if h.key == k => out.push(ev(k, Edge::Repeat, at)),
                    Some(h) => {
                        out.push(ev(h.key, Edge::Up, at));
                        out.push(ev(k, Edge::Down, at));
                    }
                    None => out.push(ev(k, Edge::Down, at)),
                }
                self.panel = Some(Held { key: k, last: at });
            }
            // Only the first of the five releases counts; the rest find
            // nothing held.
            (PanelCmd::Button(k), 0x04) if self.panel.is_some_and(|h| h.key == k) => {
                self.panel = None;
                out.push(ev(k, Edge::Up, at));
            }
            _ => {}
        }
    }
}

fn ev(key: Key, edge: Edge, at: Instant) -> KeyEvent {
    KeyEvent { key, edge, at }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u32 = ids::MFL_CMD;
    const P: u32 = ids::RNSE_KEYS;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn edges(v: &[KeyEvent]) -> Vec<(Key, Edge)> {
        v.iter().map(|e| (e.key, e.edge)).collect()
    }

    fn run(d: &mut Decoder, t0: Instant, frames: &[(u64, u32, &[u8])]) -> Vec<(Key, Edge)> {
        let mut all = Vec::new();
        for &(t, id, data) in frames {
            all.extend(edges(&d.on_frame(t0 + ms(t), id, data)));
        }
        all
    }

    #[test]
    fn stalk_rocker_from_2c1() {
        let t0 = Instant::now();
        let mut d = Decoder::new(DecoderConfig::default());
        const S: u32 = ids::STALK_KEYS;
        let got = run(
            &mut d,
            t0,
            &[
                (0, S, &[0x00, 0x00, 0x00]),
                (100, S, &[0x00, 0x00, 0x20]),
                (200, S, &[0x00, 0x00, 0x20]),
                (300, S, &[0x00, 0x00, 0x00]),
                (400, S, &[0x00, 0x00, 0x10]),
                (500, S, &[0x00, 0x00, 0x00]),
            ],
        );
        use Edge::*;
        assert_eq!(
            got,
            [
                (Key::StalkUp, Down),
                (Key::StalkUp, Repeat),
                (Key::StalkUp, Up),
                (Key::StalkDown, Down),
                (Key::StalkDown, Up),
            ]
        );
        assert!(known_key_frame(S, &[0, 0, 0x20]));
        assert!(!known_key_frame(S, &[0, 0, 0x08]));
    }

    #[test]
    fn wheel_hold_repeats_and_release() {
        let t0 = Instant::now();
        let mut d = Decoder::new(DecoderConfig::default());
        let got = run(
            &mut d,
            t0,
            &[
                (0, W, &[0x39, 0x00]),
                (100, W, &[0x3A, 0x2A]),
                (600, W, &[0x3A, 0x2A]),
                (1100, W, &[0x3A, 0x2A]),
                (1200, W, &[0x39, 0x00]),
                (1700, W, &[0x39, 0x00]),
            ],
        );
        use Edge::*;
        assert_eq!(
            got,
            vec![
                (Key::Ptt, Down),
                (Key::Ptt, Repeat),
                (Key::Ptt, Repeat),
                (Key::Ptt, Up)
            ]
        );
    }

    #[test]
    fn wheel_captured_mode_double() {
        // TT capture: 3A 1C, 3A 00, 39 01, 39 00.
        let t0 = Instant::now();
        let mut d = Decoder::new(DecoderConfig::default());
        let got = run(
            &mut d,
            t0,
            &[
                (0, W, &[0x3A, 0x1C]),
                (160, W, &[0x3A, 0x00]),
                (240, W, &[0x39, 0x01]),
                (400, W, &[0x39, 0x00]),
            ],
        );
        use Edge::*;
        assert_eq!(
            got,
            vec![
                (Key::Mode, Down),
                (Key::Mode, Up),
                (Key::Mode, Down),
                (Key::Mode, Up)
            ]
        );
    }

    #[test]
    fn stray_01_ignored() {
        let t0 = Instant::now();
        let mut d = Decoder::new(DecoderConfig::default());
        assert!(
            run(
                &mut d,
                t0,
                &[(0, W, &[0x39, 0x01]), (100, W, &[0x39, 0x00])]
            )
            .is_empty()
        );
        // MODE long ago: still stray.
        let got = run(
            &mut d,
            t0,
            &[
                (200, W, &[0x39, 0x1C]),
                (300, W, &[0x39, 0x00]),
                (2000, W, &[0x39, 0x01]),
            ],
        );
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn wheel_key_change_without_release() {
        let t0 = Instant::now();
        let mut d = Decoder::new(DecoderConfig::default());
        let got = run(&mut d, t0, &[(0, W, &[0x39, 0x0B]), (50, W, &[0x39, 0x0C])]);
        use Edge::*;
        assert_eq!(
            got,
            vec![
                (Key::WheelUp, Down),
                (Key::WheelUp, Up),
                (Key::WheelDown, Down)
            ]
        );
    }

    #[test]
    fn wheel_missing_release_times_out() {
        let t0 = Instant::now();
        let mut d = Decoder::new(DecoderConfig::default());
        d.on_frame(t0, W, &[0x39, 0x08]);
        assert_eq!(d.next_deadline(), Some(t0 + ms(1500)));
        assert!(d.tick(t0 + ms(1499)).is_empty());
        let up = d.tick(t0 + ms(1500));
        assert_eq!(edges(&up), vec![(Key::WheelClick, Edge::Up)]);
        assert_eq!(d.next_deadline(), None);
        // The late release frame does nothing.
        assert!(d.on_frame(t0 + ms(1600), W, &[0x39, 0x00]).is_empty());
    }

    #[test]
    fn wheel_short_frame_and_volume() {
        let t0 = Instant::now();
        let mut d = Decoder::new(DecoderConfig::default());
        assert!(d.on_frame(t0, W, &[0x39]).is_empty());
        let got = run(&mut d, t0, &[(0, W, &[0x39, 0x06]), (10, W, &[0x39, 0x07])]);
        assert_eq!(got[0], (Key::VolUp, Edge::Down));
        assert_eq!(got[2], (Key::VolDown, Edge::Down));
    }

    #[test]
    fn panel_tap_with_five_releases() {
        let t0 = Instant::now();
        let mut d = Decoder::new(DecoderConfig::default());
        let p = |ss: u8| [0x37, 0x30, ss, 0x00, 0x04, 0x00];
        let (on, off) = (p(1), p(4));
        let mut frames: Vec<(u64, u32, &[u8])> = vec![(0, P, &on), (110, P, &on), (220, P, &on)];
        for i in 0..5 {
            frames.push((330 + 110 * i, P, &off));
        }
        let got = run(&mut d, t0, &frames);
        use Edge::*;
        assert_eq!(
            got,
            vec![
                (Key::Media, Down),
                (Key::Media, Repeat),
                (Key::Media, Repeat),
                (Key::Media, Up)
            ]
        );
    }

    #[test]
    fn panel_masks_are_pairs_not_bits() {
        let t0 = Instant::now();
        let mut d = Decoder::new(DecoderConfig::default());
        let got = run(&mut d, t0, &[(0, P, &[0x37, 0x30, 1, 0x00, 0x0C, 0])]);
        assert_eq!(got, vec![(Key::Info, Edge::Down)]);
        let got = run(&mut d, t0, &[(10, P, &[0x37, 0x30, 4, 0x0C, 0x00, 0])]);
        assert!(got.is_empty(), "CAR release while INFO held");
        // Unknown mask, bad prefix, bad status: ignored.
        assert!(run(&mut d, t0, &[(20, P, &[0x37, 0x30, 1, 0x03, 0x00, 0])]).is_empty());
        assert!(run(&mut d, t0, &[(30, P, &[0x38, 0x30, 1, 0x00, 0x0C, 0])]).is_empty());
        assert!(run(&mut d, t0, &[(40, P, &[0x37, 0x30, 2, 0x00, 0x0C, 0])]).is_empty());
        assert!(run(&mut d, t0, &[(50, P, &[0x37, 0x30, 1, 0x00])]).is_empty());
    }

    #[test]
    fn panel_missing_release_times_out() {
        let t0 = Instant::now();
        let mut d = Decoder::new(DecoderConfig::default());
        d.on_frame(t0, P, &[0x37, 0x30, 1, 0x02, 0x00, 0]);
        d.on_frame(t0 + ms(110), P, &[0x37, 0x30, 1, 0x02, 0x00, 0]);
        assert_eq!(d.next_deadline(), Some(t0 + ms(510)));
        // A wheel frame after the deadline flushes the release first.
        let got = d.on_frame(t0 + ms(600), W, &[0x39, 0x00]);
        assert_eq!(edges(&got), vec![(Key::Next, Edge::Up)]);
        // Then the real release burst finds nothing.
        assert!(
            d.on_frame(t0 + ms(650), P, &[0x37, 0x30, 4, 0x02, 0x00, 0])
                .is_empty()
        );
    }

    #[test]
    fn knob_every_frame_and_direction() {
        let t0 = Instant::now();
        let mut d = Decoder::new(DecoderConfig::default());
        let cw = [0x37, 0x30, 1, 0x00, 0x20, 0];
        let ccw = [0x37, 0x30, 1, 0x00, 0x40, 0];
        let rel = [0x37, 0x30, 4, 0x00, 0x20, 0];
        let got = run(
            &mut d,
            t0,
            &[
                (0, P, &cw),
                (110, P, &cw),
                (220, P, &rel),
                (330, P, &rel),
                (400, P, &ccw),
            ],
        );
        use Edge::*;
        assert_eq!(
            got,
            vec![
                (Key::Knob(1), Down),
                (Key::Knob(1), Down),
                (Key::Knob(-1), Down)
            ]
        );
        // The knob never holds anything, so no timeout.
        assert_eq!(d.next_deadline(), None);
    }

    #[test]
    fn knob_frames_per_detent() {
        let t0 = Instant::now();
        let mut d = Decoder::new(DecoderConfig {
            knob_frames_per_detent: 3,
            ..Default::default()
        });
        let cw = [0x37, 0x30, 1, 0x00, 0x20, 0];
        let ccw = [0x37, 0x30, 1, 0x00, 0x40, 0];
        let got = run(
            &mut d,
            t0,
            &[
                (0, P, &cw),
                (1, P, &cw),
                (2, P, &ccw), // direction change resets
                (3, P, &cw),
                (4, P, &cw),
                (5, P, &cw),
            ],
        );
        assert_eq!(got, vec![(Key::Knob(1), Edge::Down)]);
    }

    #[test]
    fn wheel_and_panel_are_independent() {
        let t0 = Instant::now();
        let mut d = Decoder::new(DecoderConfig::default());
        let got = run(
            &mut d,
            t0,
            &[
                (0, W, &[0x39, 0x1C]),
                (10, P, &[0x37, 0x30, 1, 0x00, 0x10, 0]),
                (20, W, &[0x39, 0x00]),
                (30, P, &[0x37, 0x30, 4, 0x00, 0x10, 0]),
            ],
        );
        use Edge::*;
        assert_eq!(
            got,
            vec![
                (Key::Mode, Down),
                (Key::Enter, Down),
                (Key::Mode, Up),
                (Key::Enter, Up)
            ]
        );
    }
}
