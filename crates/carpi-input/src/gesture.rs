//! Key edges to gestures.
//!
//! Per key, from its `Timing`:
//! - No long, extended or double: `Short` on the down edge (no waiting).
//! - Long only: `Long` at the threshold while held, else `Short` on release.
//! - Extended: `Long` still fires at its threshold while held (D-pad on
//!   Prev/Next has to feel like Hudiy's, not wait for release), then
//!   `Extended` at its own threshold if the key stays down.
//! - Double: a short press waits `double` after release. A second press inside
//!   the window is `Double` (fired on its down edge); otherwise `Short` when
//!   the window closes.
//!
//! Knob detents pass straight through as one `Detent` per step.

use crate::{Edge, Key, KeyEvent};
use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GestureKind {
    Short,
    Long,
    Extended,
    Double,
    /// One knob step; the key is `Knob(1)` or `Knob(-1)`.
    Detent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gesture {
    pub key: Key,
    pub kind: GestureKind,
    pub at: Instant,
}

/// Thresholds from the down edge; `double` from the release.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Timing {
    pub long: Option<Duration>,
    pub extended: Option<Duration>,
    pub double: Option<Duration>,
}

impl Timing {
    fn immediate(&self) -> bool {
        self.long.is_none() && self.extended.is_none() && self.double.is_none()
    }
}

/// Keys not in the map are immediate.
#[derive(Clone, Debug)]
pub struct GestureConfig {
    pub timings: HashMap<Key, Timing>,
}

impl GestureConfig {
    pub fn timing(&self, key: Key) -> Timing {
        self.timings.get(&key).copied().unwrap_or_default()
    }
}

impl Default for GestureConfig {
    /// Wheel: long = 3 press frames at the assumed 500 ms repeat (1 s). No
    /// key uses a double, so none waits for one. Faceplate: long = frame 5 at
    /// 110 ms (440 ms); extended 5.5 s (Drew: "quite long, 5 s+").
    fn default() -> Self {
        let ms = Duration::from_millis;
        let wheel_long = Timing {
            long: Some(ms(1000)),
            ..Default::default()
        };
        let panel = Timing {
            long: Some(ms(440)),
            extended: Some(ms(5500)),
            double: None,
        };
        let mut timings = HashMap::new();
        timings.insert(Key::WheelClick, wheel_long);
        timings.insert(Key::Mode, wheel_long);
        timings.insert(Key::Ptt, wheel_long);
        timings.insert(Key::VolClick, wheel_long);
        for k in [
            Key::Prev,
            Key::Next,
            Key::Up,
            Key::Down,
            Key::Enter,
            Key::Return,
            Key::Setup,
            Key::Nav,
            Key::Tel,
            Key::Media,
            Key::Name,
            Key::Info,
            Key::Car,
        ] {
            timings.insert(k, panel);
        }
        // WheelUp/WheelDown/VolUp/VolDown: immediate.
        GestureConfig { timings }
    }
}

#[derive(Clone, Copy, Debug)]
enum State {
    Held {
        down_at: Instant,
        long_fired: bool,
        ext_fired: bool,
        /// Second press of a double: already reported.
        swallow: bool,
    },
    WaitDouble {
        until: Instant,
    },
}

pub struct Recogniser {
    cfg: GestureConfig,
    keys: BTreeMap<Key, State>,
}

impl Recogniser {
    pub fn new(cfg: GestureConfig) -> Self {
        Recogniser {
            cfg,
            keys: BTreeMap::new(),
        }
    }

    pub fn on_event(&mut self, ev: KeyEvent) -> Vec<Gesture> {
        let mut out = self.tick(ev.at);
        let g = |key, kind, at| Gesture { key, kind, at };

        if let Key::Knob(n) = ev.key {
            if ev.edge == Edge::Down {
                let step = Key::Knob(n.signum());
                for _ in 0..n.unsigned_abs() {
                    out.push(g(step, GestureKind::Detent, ev.at));
                }
            }
            return out;
        }

        let t = self.cfg.timing(ev.key);
        match ev.edge {
            Edge::Down => match self.keys.remove(&ev.key) {
                Some(State::WaitDouble { .. }) => {
                    out.push(g(ev.key, GestureKind::Double, ev.at));
                    self.keys.insert(ev.key, held(ev.at, true));
                }
                // Already down: the decoder lost an up. Keep the first press.
                Some(s @ State::Held { .. }) => {
                    self.keys.insert(ev.key, s);
                }
                None if t.immediate() => out.push(g(ev.key, GestureKind::Short, ev.at)),
                None => {
                    self.keys.insert(ev.key, held(ev.at, false));
                }
            },
            // Thresholds are time-based; tick above did the work.
            Edge::Repeat => {}
            Edge::Up => match self.keys.remove(&ev.key) {
                Some(State::Held {
                    down_at,
                    long_fired,
                    ext_fired,
                    swallow,
                }) => {
                    let dur = ev.at.saturating_duration_since(down_at);
                    let past = |d: Option<Duration>| d.is_some_and(|d| dur >= d);
                    if swallow || ext_fired || long_fired {
                    } else if past(t.extended) {
                        out.push(g(ev.key, GestureKind::Extended, ev.at));
                    } else if past(t.long) {
                        out.push(g(ev.key, GestureKind::Long, ev.at));
                    } else if let Some(w) = t.double {
                        self.keys
                            .insert(ev.key, State::WaitDouble { until: ev.at + w });
                    } else {
                        out.push(g(ev.key, GestureKind::Short, ev.at));
                    }
                }
                Some(s @ State::WaitDouble { .. }) => {
                    self.keys.insert(ev.key, s);
                }
                None => {}
            },
        }
        out
    }

    /// Fires hold thresholds and closes double windows due by `now`.
    pub fn tick(&mut self, now: Instant) -> Vec<Gesture> {
        let mut out = Vec::new();
        let cfg = &self.cfg;
        self.keys.retain(|&key, st| match st {
            State::Held {
                down_at,
                long_fired,
                ext_fired,
                swallow: false,
            } => {
                let t = cfg.timing(key);
                if let Some(l) = t.long
                    && !*long_fired
                    && now >= *down_at + l
                {
                    *long_fired = true;
                    out.push(Gesture {
                        key,
                        kind: GestureKind::Long,
                        at: *down_at + l,
                    });
                }
                if let Some(e) = t.extended
                    && !*ext_fired
                    && now >= *down_at + e
                {
                    *ext_fired = true;
                    out.push(Gesture {
                        key,
                        kind: GestureKind::Extended,
                        at: *down_at + e,
                    });
                }
                true
            }
            State::Held { .. } => true,
            State::WaitDouble { until } => {
                if now >= *until {
                    out.push(Gesture {
                        key,
                        kind: GestureKind::Short,
                        at: *until,
                    });
                    false
                } else {
                    true
                }
            }
        });
        out
    }

    /// When `tick` next has something to do.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.keys
            .iter()
            .filter_map(|(&key, st)| match *st {
                State::Held {
                    down_at,
                    long_fired,
                    ext_fired,
                    swallow: false,
                } => {
                    let t = self.cfg.timing(key);
                    let long = t.long.filter(|_| !long_fired).map(|l| down_at + l);
                    let ext = t.extended.filter(|_| !ext_fired).map(|e| down_at + e);
                    match (long, ext) {
                        (Some(a), Some(b)) => Some(a.min(b)),
                        (a, b) => a.or(b),
                    }
                }
                State::Held { .. } => None,
                State::WaitDouble { until } => Some(until),
            })
            .min()
    }
}

fn held(at: Instant, swallow: bool) -> State {
    State::Held {
        down_at: at,
        long_fired: false,
        ext_fired: false,
        swallow,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use GestureKind::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Fixed timings for testing the mechanism (doubles, extended), so these
    /// tests don't move when the shipped defaults change.
    fn mechanism_config() -> GestureConfig {
        let mut c = GestureConfig::default();
        let wheel = Timing {
            long: Some(ms(1000)),
            extended: None,
            double: Some(ms(250)),
        };
        c.timings.insert(Key::WheelClick, wheel);
        c.timings.insert(Key::Mode, wheel);
        for t in c.timings.values_mut() {
            if t.extended.is_some() {
                t.extended = Some(ms(3190));
            }
        }
        c
    }

    #[test]
    fn shipped_defaults() {
        let c = GestureConfig::default();
        assert_eq!(c.timing(Key::Next).extended, Some(ms(5500)));
        assert_eq!(c.timing(Key::WheelClick).double, None);
        assert_eq!(c.timing(Key::Mode).double, None);
    }

    struct T {
        r: Recogniser,
        t0: Instant,
        got: Vec<(Key, GestureKind, u64)>,
    }

    impl T {
        fn new() -> Self {
            T {
                r: Recogniser::new(mechanism_config()),
                t0: Instant::now(),
                got: Vec::new(),
            }
        }
        fn push(&mut self, g: Vec<Gesture>) {
            for g in g {
                let t = g.at.duration_since(self.t0).as_millis() as u64;
                self.got.push((g.key, g.kind, t));
            }
        }
        fn ev(&mut self, t: u64, key: Key, edge: Edge) -> &mut Self {
            let at = self.t0 + ms(t);
            let g = self.r.on_event(KeyEvent { key, edge, at });
            self.push(g);
            self
        }
        fn tick(&mut self, t: u64) -> &mut Self {
            let g = self.r.tick(self.t0 + ms(t));
            self.push(g);
            self
        }
        fn take(&mut self) -> Vec<(Key, GestureKind, u64)> {
            std::mem::take(&mut self.got)
        }
    }

    #[test]
    fn immediate_keys_fire_on_down_only() {
        let mut t = T::new();
        t.ev(0, Key::WheelUp, Edge::Down)
            .ev(500, Key::WheelUp, Edge::Repeat)
            .ev(600, Key::WheelUp, Edge::Up);
        assert_eq!(t.take(), vec![(Key::WheelUp, Short, 0)]);
        assert_eq!(t.r.next_deadline(), None);
    }

    #[test]
    fn ptt_short_and_long() {
        let mut t = T::new();
        t.ev(0, Key::Ptt, Edge::Down).ev(300, Key::Ptt, Edge::Up);
        assert_eq!(t.take(), vec![(Key::Ptt, Short, 300)]);

        t.ev(1000, Key::Ptt, Edge::Down);
        assert_eq!(t.r.next_deadline(), Some(t.t0 + ms(2000)));
        t.ev(1500, Key::Ptt, Edge::Repeat).tick(1999);
        assert!(t.take().is_empty());
        t.tick(2000)
            .ev(2500, Key::Ptt, Edge::Repeat)
            .ev(2600, Key::Ptt, Edge::Up);
        assert_eq!(t.take(), vec![(Key::Ptt, Long, 2000)]);
    }

    #[test]
    fn long_fires_on_up_if_tick_was_missed() {
        let mut t = T::new();
        t.ev(0, Key::Ptt, Edge::Down);
        // No tick; the up edge arrives late. tick() inside on_event catches it
        // at the threshold time.
        t.ev(1200, Key::Ptt, Edge::Up);
        assert_eq!(t.take(), vec![(Key::Ptt, Long, 1000)]);
    }

    #[test]
    fn mode_single_waits_for_window() {
        let mut t = T::new();
        t.ev(0, Key::Mode, Edge::Down).ev(160, Key::Mode, Edge::Up);
        assert!(t.take().is_empty());
        assert_eq!(t.r.next_deadline(), Some(t.t0 + ms(410)));
        t.tick(409);
        assert!(t.take().is_empty());
        t.tick(410);
        assert_eq!(t.take(), vec![(Key::Mode, Short, 410)]);
    }

    #[test]
    fn mode_double_from_capture() {
        // 3A 1C, 3A 00, 39 01, 39 00 decoded as two MODE presses.
        let mut t = T::new();
        t.ev(0, Key::Mode, Edge::Down)
            .ev(160, Key::Mode, Edge::Up)
            .ev(240, Key::Mode, Edge::Down)
            .ev(400, Key::Mode, Edge::Up)
            .tick(2000);
        assert_eq!(t.take(), vec![(Key::Mode, Double, 240)]);
    }

    #[test]
    fn second_press_at_window_edge_is_new_press() {
        let mut t = T::new();
        t.ev(0, Key::WheelClick, Edge::Down)
            .ev(100, Key::WheelClick, Edge::Up)
            .ev(350, Key::WheelClick, Edge::Down)
            .ev(400, Key::WheelClick, Edge::Up)
            .tick(1000);
        assert_eq!(
            t.take(),
            vec![(Key::WheelClick, Short, 350), (Key::WheelClick, Short, 650)]
        );
    }

    #[test]
    fn double_second_press_held_does_not_go_long() {
        let mut t = T::new();
        t.ev(0, Key::Mode, Edge::Down)
            .ev(100, Key::Mode, Edge::Up)
            .ev(200, Key::Mode, Edge::Down)
            .tick(5000)
            .ev(5000, Key::Mode, Edge::Up)
            .tick(9000);
        assert_eq!(t.take(), vec![(Key::Mode, Double, 200)]);
    }

    #[test]
    fn mode_long_skips_double() {
        let mut t = T::new();
        t.ev(0, Key::Mode, Edge::Down)
            .tick(1000)
            .ev(1200, Key::Mode, Edge::Up)
            .tick(3000);
        assert_eq!(t.take(), vec![(Key::Mode, Long, 1000)]);
    }

    #[test]
    fn panel_short_long_extended() {
        let mut t = T::new();
        // Tap: 3 frames.
        t.ev(0, Key::Next, Edge::Down).ev(330, Key::Next, Edge::Up);
        assert_eq!(t.take(), vec![(Key::Next, Short, 330)]);
        // Held past long: Long at the threshold, nothing on release.
        t.ev(1000, Key::Next, Edge::Down)
            .tick(1440)
            .tick(2000)
            .ev(2100, Key::Next, Edge::Up);
        assert_eq!(t.take(), vec![(Key::Next, Long, 1440)]);
        // Exactly at long on release counts as long.
        t.ev(3000, Key::Next, Edge::Down)
            .ev(3440, Key::Next, Edge::Up);
        assert_eq!(t.take(), vec![(Key::Next, Long, 3440)]);
        // Held to extended: Long on the way, Extended at its threshold, each
        // once, nothing on release.
        t.ev(5000, Key::Next, Edge::Down);
        assert_eq!(t.r.next_deadline(), Some(t.t0 + ms(5440)));
        t.tick(5440);
        assert_eq!(t.take(), vec![(Key::Next, Long, 5440)]);
        assert_eq!(t.r.next_deadline(), Some(t.t0 + ms(8190)));
        t.tick(8189);
        assert!(t.take().is_empty());
        t.tick(8190).tick(9000).ev(9500, Key::Next, Edge::Up);
        assert_eq!(t.take(), vec![(Key::Next, Extended, 8190)]);
        assert_eq!(t.r.next_deadline(), None);
    }

    #[test]
    fn knob_passes_through() {
        let mut t = T::new();
        t.ev(0, Key::Knob(1), Edge::Down)
            .ev(10, Key::Knob(-3), Edge::Down)
            .ev(20, Key::Knob(1), Edge::Up);
        assert_eq!(
            t.take(),
            vec![
                (Key::Knob(1), Detent, 0),
                (Key::Knob(-1), Detent, 10),
                (Key::Knob(-1), Detent, 10),
                (Key::Knob(-1), Detent, 10)
            ]
        );
    }

    #[test]
    fn keys_are_independent() {
        let mut t = T::new();
        t.ev(0, Key::Mode, Edge::Down)
            .ev(50, Key::Enter, Edge::Down)
            .ev(100, Key::Mode, Edge::Up)
            .ev(150, Key::Enter, Edge::Up)
            .tick(1000);
        assert_eq!(
            t.take(),
            vec![(Key::Enter, Short, 150), (Key::Mode, Short, 350)]
        );
    }

    #[test]
    fn stray_edges_are_harmless() {
        let mut t = T::new();
        t.ev(0, Key::Ptt, Edge::Up).ev(10, Key::Ptt, Edge::Repeat);
        t.ev(20, Key::Ptt, Edge::Down).ev(30, Key::Ptt, Edge::Down);
        t.ev(1500, Key::Ptt, Edge::Up);
        assert_eq!(t.take(), vec![(Key::Ptt, Long, 1020)]);
    }
}
