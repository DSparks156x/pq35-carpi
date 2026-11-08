//! Acceleration test: a header and up to four timed legs (speed ranges,
//! distances, or live speed), the old page's behaviour (`dc/apps/
//! acceleration_test.py`), rebuilt as a pure timing engine fed speed samples
//! plus a screen.
//!
//! Engine rules (old page): times interpolate the exact crossing between two
//! samples; a 0-X leg starts at the last sample that read 0; a leg aborts when
//! speed falls 5 below its peak; everything resets once stopped (< 1) for
//! 250 ms. The header shows `Ready` / `Timing` / `Done` / `Rolling` and the
//! timing uncertainty, ± half a sample interval at each end.
//!
//! Layout (logical): header centred y=1; a 1 px rule at y=10; legs at
//! y 13/22/31/40, label left from x=1, value right-aligned to x=63 and free
//! to start right after its own label. Live speed has no label (Drew: the
//! unit says it), centred, and drops its unit if it still doesn't fit. Running legs tick at 5/s. A leg
//! that just finished shows inverted for 1 s (the old page did; Drew wants
//! it, an explicit exception to "highlight means DIS focus").

use super::Screen;
use crate::cmd::{Dir, Ink, Op, Rop, text_flags};
use crate::font::{Font, TEXT_H, audscii};
use crate::proto::{FULL, Rect};
use crate::helpers::{Ctx, TextSlot};
use crate::pack::Group;
use std::time::{Duration, Instant};

pub const ROWS: [u8; 4] = [13, 22, 31, 40];
pub const HEADER_Y: u8 = 1;
/// Separator under the header at logical y=10, with clearance both sides
/// (bench: at y=9 between text at y=1 and y=12 the opaque text cells painted
/// over it). Drawn as `63` H lines, one physical px tall like the cluster's
/// own separators (a `52` fill is a whole 2 px logical row; Drew: too thick).
pub const RULE_Y: u8 = 10;
const TICK: Duration = Duration::from_millis(200);
/// A finished leg shows inverted this long.
const FLASH: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq)]
pub enum Leg {
    /// From one speed to another, in display units.
    Range { from: f64, to: f64 },
    /// From a standing start over this many metres.
    Distance { m: f64, label: String },
    /// Live speed, no timing.
    Speed,
}

impl Leg {
    pub fn label(&self) -> String {
        match self {
            Leg::Range { from, to } => format!("{from}-{to}"),
            Leg::Distance { label, .. } => label.clone(),
            Leg::Speed => String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AccelConfig {
    pub legs: Vec<Leg>,
    /// mph and miles instead of km/h and km.
    pub imperial: bool,
    pub reset_after: Duration,
}

impl Default for AccelConfig {
    fn default() -> Self {
        AccelConfig {
            legs: vec![
                Leg::Range { from: 0.0, to: 100.0 },
                Leg::Range { from: 80.0, to: 120.0 },
                Leg::Distance {
                    m: 402.336,
                    label: "1/4 mi".into(),
                },
                Leg::Speed,
            ],
            imperial: false,
            reset_after: Duration::from_millis(250),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LegState {
    Waiting,
    Running {
        start: Instant,
        start_dt: f64,
        start_dist: f64,
        peak: f64,
    },
    Done {
        secs: f64,
        tol: f64,
        /// When it finished (for the 1 s flash).
        at: Instant,
    },
    Aborted,
}

/// The timing engine. Feed it `(time, speed in display units)` samples.
#[derive(Clone, Debug)]
pub struct AccelTimer {
    pub cfg: AccelConfig,
    pub states: Vec<LegState>,
    last: Option<(Instant, f64)>,
    /// Metres since start.
    dist: f64,
    last_zero: Option<(Instant, f64)>,
    stopped_since: Option<Instant>,
    /// Last sample interval, s.
    dt: f64,
}

impl AccelTimer {
    pub fn new(cfg: AccelConfig) -> Self {
        let n = cfg.legs.len().min(4);
        AccelTimer {
            cfg,
            states: vec![LegState::Waiting; n],
            last: None,
            dist: 0.0,
            last_zero: None,
            stopped_since: None,
            dt: 0.05,
        }
    }

    pub fn speed(&self) -> f64 {
        self.last.map_or(0.0, |(_, v)| v)
    }

    fn reset(&mut self) {
        self.states.iter_mut().for_each(|s| *s = LegState::Waiting);
        self.dist = 0.0;
        self.last_zero = None;
    }

    pub fn sample(&mut self, t: Instant, v: f64) {
        let Some((t0, v0)) = self.last else {
            self.last = Some((t, v));
            if v == 0.0 {
                self.last_zero = Some((t, 0.0));
            }
            return;
        };
        let dt = (t - t0).as_secs_f64();
        if dt <= 0.0 {
            return;
        }
        self.dt = dt;
        let per_unit = if self.cfg.imperial { 1609.344 } else { 1000.0 };
        let mps = v * per_unit / 3600.0;
        self.dist += mps * dt;
        if v == 0.0 {
            self.last_zero = Some((t, self.dist));
        }
        // Time at which speed crossed `x` between the two samples.
        let cross = |x: f64| {
            let f = if v != v0 { ((x - v0) / (v - v0)).clamp(0.0, 1.0) } else { 0.0 };
            t0 + Duration::from_secs_f64(dt * f)
        };
        if v < 1.0 {
            let since = *self.stopped_since.get_or_insert(t);
            if t - since >= self.cfg.reset_after {
                self.reset();
            }
            self.last = Some((t, v));
            return;
        }
        self.stopped_since = None;
        let (dist, last_zero) = (self.dist, self.last_zero);
        for (leg, st) in self.cfg.legs.iter().zip(self.states.iter_mut()) {
            match (leg, *st) {
                (Leg::Speed, _) => {}
                (Leg::Range { from, .. }, LegState::Waiting) => {
                    let trigger = from.max(1.0);
                    if v0 < trigger && v >= trigger {
                        let start = match last_zero {
                            Some((z, _)) if *from == 0.0 => z,
                            _ => cross(*from),
                        };
                        *st = LegState::Running {
                            start,
                            start_dt: dt,
                            start_dist: dist,
                            peak: v,
                        };
                    }
                }
                (Leg::Distance { .. }, LegState::Waiting) => {
                    if v0 < 1.0 && v >= 1.0 {
                        let (start, start_dist) = last_zero.unwrap_or((cross(0.0), dist - mps * dt));
                        *st = LegState::Running {
                            start,
                            start_dt: dt,
                            start_dist,
                            peak: v,
                        };
                    }
                }
                (_, LegState::Running { start, start_dt, start_dist, peak }) => {
                    let peak = peak.max(v);
                    let end = match leg {
                        Leg::Range { to, .. } if v >= *to => Some(cross(*to)),
                        Leg::Distance { m, .. } if dist - start_dist >= *m => Some(t),
                        _ => None,
                    };
                    *st = if let Some(end) = end {
                        LegState::Done {
                            secs: (end - start).as_secs_f64(),
                            tol: (start_dt + dt) / 2.0,
                            at: t,
                        }
                    } else if v < peak - 5.0 {
                        LegState::Aborted
                    } else {
                        LegState::Running { start, start_dt, start_dist, peak }
                    };
                }
                (Leg::Range { from, .. }, LegState::Done { .. } | LegState::Aborted) => {
                    if v < *from {
                        *st = LegState::Waiting;
                    }
                }
                _ => {}
            }
        }
        self.last = Some((t, v));
    }

    pub fn header(&self) -> String {
        let running = self.states.iter().any(|s| matches!(s, LegState::Running { .. }));
        let done = self.states.iter().any(|s| matches!(s, LegState::Done { .. }));
        let word = if self.speed() < 1.0 {
            "Ready"
        } else if running {
            "Timing"
        } else if done {
            "Done"
        } else {
            "Rolling"
        };
        let tol = self
            .states
            .iter()
            .filter_map(|s| match s {
                LegState::Running { start_dt, .. } => Some(start_dt / 2.0),
                LegState::Done { tol, .. } => Some(*tol),
                _ => None,
            })
            .fold(0.0, f64::max);
        if tol > 0.0 {
            format!("{word} \u{B1}{tol:.2}s")
        } else {
            word.into()
        }
    }

    pub fn value(&self, i: usize, now: Instant) -> String {
        let unit = if self.cfg.imperial { "mph" } else { "km/h" };
        match (&self.cfg.legs[i], self.states[i]) {
            (Leg::Speed, _) => format!("{} {unit}", self.speed().round() as i64),
            (_, LegState::Running { start, .. }) => {
                format!("{:.1}s", now.saturating_duration_since(start).as_secs_f64())
            }
            (_, LegState::Done { secs, .. }) => format!("{secs:.3}s"),
            _ => "-.---".into(),
        }
    }

    pub fn running(&self) -> bool {
        self.states.iter().any(|s| matches!(s, LegState::Running { .. }))
    }
}

pub struct AccelScreen {
    pub timer: AccelTimer,
    header: TextSlot,
    labels: Vec<TextSlot>,
    values: Vec<TextSlot>,
    /// Rows drawn inverted right now.
    inverted: Vec<bool>,
    /// Last update of running values.
    ticked: Option<Instant>,
}

impl AccelScreen {
    pub fn new(cfg: AccelConfig) -> Self {
        AccelScreen {
            timer: AccelTimer::new(cfg),
            header: TextSlot::new(HEADER_Y, 0, 64, Font::Prop),
            labels: ROWS.iter().map(|y| TextSlot::new(*y, 1, 30, Font::Prop)).collect(),
            values: ROWS.iter().map(|y| TextSlot::new(*y, 30, 63, Font::Prop)).collect(),
            inverted: vec![false; 4],
            ticked: None,
        }
    }

    /// A speed sample in display units.
    pub fn sample(&mut self, t: Instant, v: f64) {
        self.timer.sample(t, v);
    }

    fn groups(&mut self, now: Instant, cx: &Ctx, labels: bool) -> Vec<Group> {
        let m = &cx.metrics;
        self.ticked = Some(now);
        let h = audscii(&self.timer.header());
        let x = self.header.centre_x(m, &h);
        let mut out: Vec<Group> = self.header.draw(m, "header", x, h).into_iter().collect();
        if labels {
            // XOR is the only proven "on" mode for `63`; the row is blank after
            // the enter clear, so XOR lights it. Two 32-long runs (60 is the
            // longest line confirmed on the bench).
            let line = |x| Op::Line {
                clip: FULL,
                x,
                y: RULE_Y,
                dir: Dir::H,
                len: 32,
                rop: Rop::Xor,
            };
            out.push(Group::new("rule", vec![line(0), line(32)]));
        }
        for i in 0..self.timer.states.len() {
            let l = audscii(&self.timer.cfg.legs[i].label());
            let n = m.fit(Font::Prop, &l, self.labels[i].width_px());
            let l = l[..n].to_vec();
            // The value column starts just after this row's label.
            let left = if l.is_empty() {
                1
            } else {
                (1 + m.width(Font::Prop, &l).div_ceil(2) as u8 + 2).min(62)
            };
            if self.values[i].left != left {
                self.values[i].left = left;
            }
            let room = u16::from(63 - left) * 2;
            let mut v = audscii(&self.timer.value(i, now));
            if m.width(Font::Prop, &v) > room && self.timer.cfg.legs[i] == Leg::Speed {
                v = audscii(&format!("{}", self.timer.speed().round() as i64));
            }
            let vx = if l.is_empty() {
                // Unlabelled (live speed): centred.
                (32u8.saturating_sub((m.width(Font::Prop, &v) / 4) as u8)).max(1)
            } else {
                (63 - (m.width(Font::Prop, &v).div_ceil(2) as u8).min(63)).max(left)
            };
            let flash = matches!(self.timer.states[i], LegState::Done { at, .. } if now < at + FLASH);
            if flash && !self.inverted[i] {
                // Whole row: fill, then label and value in black, one group.
                let row = Rect::new(0, ROWS[i] - 1, 64, TEXT_H);
                let black = text_flags(Font::Prop, Ink::Black);
                out.push(Group::new(
                    "leg-done",
                    vec![
                        Op::Fill(row),
                        Op::Text { clip: row, x: 1, y: 1, flags: black, bytes: l },
                        Op::Text { clip: row, x: vx, y: 1, flags: black, bytes: v },
                    ],
                ));
                self.inverted[i] = true;
                self.labels[i].reset();
                self.values[i].reset();
                continue;
            }
            if flash {
                continue;
            }
            let mut ops = Vec::new();
            if self.inverted[i] {
                self.inverted[i] = false;
                ops.push(Op::Clear(Rect::new(0, ROWS[i] - 1, 64, TEXT_H)));
                self.labels[i].reset();
                self.values[i].reset();
            }
            if labels || !ops.is_empty() {
                ops.extend(self.labels[i].draw(m, "label", 1, l).map_or_else(Vec::new, |g| g.ops));
            }
            ops.extend(self.values[i].draw(m, "value", vx, v).map_or_else(Vec::new, |g| g.ops));
            if !ops.is_empty() {
                out.push(Group::new("leg", ops));
            }
        }
        out
    }
}

impl Screen for AccelScreen {
    fn enter(&mut self, now: Instant, cx: &Ctx) -> Vec<Group> {
        self.header.reset();
        self.labels.iter_mut().for_each(TextSlot::reset);
        self.values.iter_mut().for_each(TextSlot::reset);
        self.inverted = vec![false; 4];
        self.groups(now, cx, true)
    }

    fn update(&mut self, now: Instant, cx: &Ctx) -> Vec<Group> {
        self.groups(now, cx, false)
    }

    fn next_deadline(&self, _cx: &Ctx) -> Option<Instant> {
        let tick = self.timer.running().then(|| self.ticked.map(|t| t + TICK)).flatten();
        let unflash = self
            .timer
            .states
            .iter()
            .zip(&self.inverted)
            .filter_map(|(s, inv)| match s {
                LegState::Done { at, .. } if *inv => Some(*at + FLASH),
                _ => None,
            })
            .min();
        [tick, unflash].into_iter().flatten().min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(timer: &mut AccelTimer, t0: Instant, profile: impl Fn(f64) -> f64, secs: f64) {
        let mut s = 0.0;
        while s <= secs {
            timer.sample(t0 + Duration::from_secs_f64(s), profile(s));
            s += 0.05;
        }
    }

    #[test]
    fn constant_acceleration_times_and_reset() {
        let t0 = Instant::now();
        let mut a = AccelTimer::new(AccelConfig::default());
        // 0 for 1 s, then 20 km/h per s.
        let v = |s: f64| if s < 1.0 { 0.0 } else { (s - 1.0) * 20.0 };
        run(&mut a, t0, v, 8.0);
        match a.states[0] {
            LegState::Done { secs, .. } => assert!((secs - 5.0).abs() < 0.06, "{secs}"),
            s => panic!("{s:?}"),
        }
        match a.states[1] {
            LegState::Done { secs, .. } => assert!((secs - 2.0).abs() < 0.01, "{secs}"),
            s => panic!("{s:?}"),
        }
        assert!(a.header().starts_with("Timing"), "quarter mile still running");
        // Lift: 5 below the peak aborts the quarter mile.
        a.sample(t0 + Duration::from_secs_f64(8.1), 130.0);
        assert_eq!(a.states[2], LegState::Aborted);
        // Stop for 300 ms: everything resets.
        a.sample(t0 + Duration::from_secs(9), 0.0);
        a.sample(t0 + Duration::from_secs_f64(9.3), 0.0);
        assert!(a.states.iter().all(|s| *s == LegState::Waiting));
        assert_eq!(a.header(), "Ready");
        assert_eq!(a.value(3, t0), "0 km/h");
    }
}
