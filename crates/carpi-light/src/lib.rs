//! The light model (PLAN 2.6): one place that decides how bright the RNS-E
//! panel is and whether it's night. Sans-IO.
//!
//! Consumers: `carpi-rnse` (the `BB` level), the LIVI bridge (dark mode, the
//! AA/CarPlay night sensor) and the DIS.
//!
//! **Brightness**
//! 1. Ambient, log scale ([`ambient::log_scale`]), from the configured
//!    source: the cluster sensor (default), the climatronic sun sensor, or
//!    a blend where the sun sensor only extends the bright end once the
//!    cluster sensor saturates.
//! 2. Smoothed (1.5 s time constant, so about 3 s to settle) with a 0.05
//!    deadband: tree shadows don't flicker the screen, tunnels still react.
//! 3. The curve ([`curve`]): the manual points, or the default curve for
//!    the source with the learned nudges blended in.
//! 4. Scroll trim: the car's display dimming (58d, which the car computes
//!    from the scroll, its sensor and the lights) shifts the result by
//!    [`TRIM_GAIN`] % per % away from [`TRIM_REF`]. Gentle on purpose.
//! 5. Clamped to 6-100 (`BB` 1-5 act as 6), then [`Sender`]: a new level
//!    only on a change of 2 % or more, at most every 500 ms.
//!
//! **Night**: see [`dark`].

pub mod ambient;
pub mod curve;
pub mod dark;
pub mod sun;

pub use ambient::{AmbientSource, Estimator, Raw, Smoother};
pub use curve::{Curve, CurveMode, FLOOR_PCT, Learned, default_curve};
pub use dark::{ClockTime, DarkInputs, DarkMode, DarkSetup, DarkSource, NightSource};
pub use sun::{Location, SunDay, sun_day};

use carpi_state::{Store, Value};
use std::time::{Duration, Instant};

/// Scroll trim: panel % per % of display dimming away from [`TRIM_REF`].
/// Guess: ±7.5 % over the dimmer's whole range.
pub const TRIM_GAIN: f64 = 0.15;
/// Display dimming that leaves the curve as it is. Guess.
pub const TRIM_REF: f64 = 50.0;
/// Panel % a brightness key moves.
pub const KEY_STEP: i32 = 5;

/// The light settings (`[light]` in carpid.toml).
#[derive(Clone, Debug, PartialEq)]
pub struct Setup {
    /// Off: `BB 00` once, then the RNS-E is left alone (the caller does it).
    pub auto_brightness: bool,
    pub scroll_trim: bool,
    pub ambient_source: AmbientSource,
    pub curve_mode: CurveMode,
    /// (ambient, panel %), ascending ambient.
    pub manual_curve: Vec<(f64, u8)>,
    /// Learned points, keyed by the source they were learned on.
    pub learned: Learned,
    pub dark: DarkSetup,
}

impl Default for Setup {
    /// Drew's defaults (PLAN 2.6).
    fn default() -> Self {
        Setup {
            auto_brightness: true,
            scroll_trim: true,
            ambient_source: AmbientSource::Cluster,
            curve_mode: CurveMode::Learned,
            manual_curve: default_curve(AmbientSource::Cluster).to_vec(),
            learned: Vec::new(),
            dark: DarkSetup::default(),
        }
    }
}

/// Store values, already decoded by `carpi-signals`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Inputs {
    /// 0x621 `KO1_Lichtsensor`.
    pub cluster_sensor: Option<f64>,
    /// 0x635 `DI1_Sensor`, used when 0x621's is missing.
    pub dim_sensor: Option<f64>,
    pub sun_left_wm2: Option<f64>,
    pub sun_right_wm2: Option<f64>,
    /// 58d as the RNS-E sees it (0x635).
    pub dim_display_pct: Option<f64>,
    /// 58d from 0x470 and 0x621, fallbacks for trim.
    pub display_pct: Option<f64>,
    pub cluster_display_pct: Option<f64>,
    /// 58s (0x635): switch illumination, nonzero with the lights on.
    pub dim_switch_pct: Option<f64>,
    pub low_beam: Option<bool>,
    pub side: Option<bool>,
    pub kl15: Option<bool>,
    pub clock: Option<ClockTime>,
}

impl Inputs {
    pub fn from_store(s: &Store, now: Instant) -> Self {
        Inputs {
            cluster_sensor: s.get_num("light.cluster_sensor", now),
            dim_sensor: s.get_num("light.dim_sensor", now),
            sun_left_wm2: s.get_num("light.sun_left_wm2", now),
            sun_right_wm2: s.get_num("light.sun_right_wm2", now),
            dim_display_pct: s.get_num("light.dim_display_pct", now),
            display_pct: s.get_num("light.display_pct", now),
            cluster_display_pct: s.get_num("light.cluster_display_pct", now),
            dim_switch_pct: s.get_num("light.dim_switch_pct", now),
            low_beam: s.get_bool("lights.low_beam", now),
            side: s.get_bool("lights.side", now),
            // The same rule as the DIS and the bus gate: on if either the
            // BCM (0x575) or the ignition switch (0x2C3) says on.
            kl15: carpi_gate::kl15(s, now),
            clock: match s.get("clock.datetime", now) {
                Some(Value::Text(t)) => ClockTime::parse(t),
                _ => None,
            },
        }
    }

    /// Headlights: 58s > 0 (what the old code and the RNS-E's own day/night
    /// use) or the 0x470 side/low-beam bits. Any on → `Some(true)`; none on
    /// but one known → `Some(false)`.
    pub fn lights_on(&self) -> Option<bool> {
        let sources = [
            self.side,
            self.low_beam,
            self.dim_switch_pct.map(|p| p > 0.0),
        ];
        if sources.contains(&Some(true)) {
            Some(true)
        } else if sources.iter().any(Option::is_some) {
            Some(false)
        } else {
            None
        }
    }

    pub fn raw(&self) -> Raw {
        Raw {
            cluster: self.cluster_sensor.or(self.dim_sensor),
            sun: match (self.sun_left_wm2, self.sun_right_wm2) {
                (Some(a), Some(b)) => Some((a + b) / 2.0),
                (a, b) => a.or(b),
            },
        }
    }

    /// The car's display dimming, for scroll trim.
    pub fn dimmer_pct(&self) -> Option<f64> {
        self.dim_display_pct
            .or(self.display_pct)
            .or(self.cluster_display_pct)
    }
}

/// What goes out: a level only on a change of `min_step` or more (or onto
/// the floor or ceiling), at most every `min_interval`.
#[derive(Clone, Debug)]
pub struct Sender {
    pub min_step: u8,
    pub min_interval: Duration,
    level: Option<u8>,
    at: Option<Instant>,
}

impl Default for Sender {
    fn default() -> Self {
        Sender {
            min_step: 2,
            min_interval: Duration::from_millis(500),
            level: None,
            at: None,
        }
    }
}

impl Sender {
    /// The level to hold now. `None` until there's been a target.
    pub fn update(&mut self, now: Instant, target: Option<f64>) -> Option<u8> {
        let Some(t) = target else {
            return self.level;
        };
        let t = t.round().clamp(FLOOR_PCT as f64, 100.0) as u8;
        let due = self
            .at
            .is_none_or(|at| now.saturating_duration_since(at) >= self.min_interval);
        let worth = match self.level {
            None => true,
            Some(l) => l.abs_diff(t) >= self.min_step || (l != t && (t == FLOOR_PCT || t == 100)),
        };
        if due && worth {
            self.level = Some(t);
            self.at = Some(now);
        }
        self.level
    }

    pub fn level(&self) -> Option<u8> {
        self.level
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Light {
    pub night: bool,
    pub night_source: Option<NightSource>,
    /// Raw headlight reading; `None` if unknown.
    pub lights_on: Option<bool>,
    /// Smoothed log-scale ambient from the configured source.
    pub ambient: Option<f64>,
    /// Curve plus trim, before the send limits.
    pub target_pct: Option<f64>,
    /// The level for `BB`, rate limited. `None` until the first ambient.
    pub panel_pct: Option<u8>,
    pub dimmer_pct: Option<f64>,
    pub trim_pct: f64,
}

pub struct LightModel {
    setup: Setup,
    est: Estimator,
    smooth: Smoother,
    curve: Curve,
    sender: Sender,
    dark: DarkMode,
    ambient: Option<f64>,
    trim: f64,
    /// `light.curve`, rebuilt when the curve or trim moves.
    json: String,
    json_trim: f64,
}

impl LightModel {
    pub fn new(setup: Setup) -> Self {
        let mut m = LightModel {
            curve: Curve::default(),
            setup,
            est: Estimator::default(),
            smooth: Smoother::default(),
            sender: Sender::default(),
            dark: DarkMode::default(),
            ambient: None,
            trim: 0.0,
            json: String::new(),
            json_trim: 0.0,
        };
        m.rebuild();
        m
    }

    pub fn setup(&self) -> &Setup {
        &self.setup
    }

    pub fn set_setup(&mut self, s: Setup) {
        if s.ambient_source != self.setup.ambient_source {
            self.est.reset();
            self.smooth.reset();
            self.ambient = None;
        }
        self.setup = s;
        self.rebuild();
    }

    fn rebuild(&mut self) {
        let src = self.setup.ambient_source;
        let mine: Vec<(f64, u8)> = self
            .setup
            .learned
            .iter()
            .filter(|p| p.0 == src)
            .map(|p| (p.1, p.2))
            .collect();
        self.curve = Curve::new(self.setup.curve_mode, src, &self.setup.manual_curve, &mine);
        self.json = self.render_json();
        self.json_trim = self.trim;
    }

    /// Call every tick (~100 ms).
    pub fn update(&mut self, now: Instant, i: &Inputs) -> Light {
        let raw = self.est.estimate(self.setup.ambient_source, i.raw());
        self.ambient = self.smooth.update(now, raw);
        let dimmer = i.dimmer_pct();
        self.trim = match dimmer {
            Some(d) if self.setup.scroll_trim => TRIM_GAIN * (d - TRIM_REF),
            _ => 0.0,
        };
        if (self.trim - self.json_trim).abs() >= 0.5 {
            self.json = self.render_json();
            self.json_trim = self.trim;
        }
        let target = self
            .ambient
            .map(|a| (self.curve.eval(a) + self.trim).clamp(FLOOR_PCT as f64, 100.0));
        let panel = self.sender.update(now, target);

        let lights_on = i.lights_on();
        let night = self.dark.update(
            now,
            &self.setup.dark,
            &DarkInputs {
                lights_on,
                kl15: i.kl15,
                clock: i.clock,
                ambient: self.ambient,
            },
        );
        Light {
            night,
            night_source: self.dark.source(),
            lights_on,
            ambient: self.ambient,
            target_pct: target,
            panel_pct: panel,
            dimmer_pct: dimmer,
            trim_pct: self.trim,
        }
    }

    /// Brighter or darker by `delta` % at the current ambient, learned for
    /// the current source. Returns the level stored; the caller persists
    /// [`LightModel::learned`].
    pub fn nudge(&mut self, delta: i32) -> Result<u8, String> {
        if !self.setup.auto_brightness {
            return Err("auto brightness is off".into());
        }
        if self.setup.curve_mode != CurveMode::Learned {
            return Err("the curve is manual; edit its points instead".into());
        }
        let Some(a) = self.ambient else {
            return Err("no ambient reading yet".into());
        };
        let src = self.setup.ambient_source;
        let pct = curve::nudge(&mut self.setup.learned, &self.curve, src, a, delta);
        self.rebuild();
        Ok(pct)
    }

    /// Forgets the learned points of `src` (`None`: the current source).
    pub fn reset_learned(&mut self, src: Option<AmbientSource>) {
        let src = src.unwrap_or(self.setup.ambient_source);
        self.setup.learned.retain(|p| p.0 != src);
        self.rebuild();
    }

    pub fn learned(&self) -> &Learned {
        &self.setup.learned
    }

    /// `light.curve`: the curve in use, trim included, sampled for the
    /// page's plot, plus this source's learned points. Compact JSON:
    /// `{"source":"cluster","mode":"learned","trim":-2.0,
    /// "points":[[0.00,12],...],"learned":[[1.56,60],...]}`.
    pub fn curve_json(&self) -> &str {
        &self.json
    }

    fn render_json(&self) -> String {
        use std::fmt::Write;
        let src = self.setup.ambient_source;
        let top = ambient::scale_top(src);
        let mode = match self.setup.curve_mode {
            CurveMode::Manual => "manual",
            CurveMode::Learned => "learned",
        };
        let mut s = format!(
            "{{\"source\":\"{}\",\"mode\":\"{mode}\",\"trim\":{:.1},\"points\":[",
            src.as_str(),
            self.trim
        );
        const N: usize = 24;
        for k in 0..=N {
            let a = top * k as f64 / N as f64;
            let p = (self.curve.eval(a) + self.trim).clamp(FLOOR_PCT as f64, 100.0);
            let _ = write!(s, "{}[{a:.2},{p:.0}]", if k == 0 { "" } else { "," });
        }
        s.push_str("],\"learned\":[");
        for (k, &(a, p)) in self.curve.learned.iter().enumerate() {
            let _ = write!(s, "{}[{a:.2},{p:.0}]", if k == 0 { "" } else { "," });
        }
        s.push_str("]}");
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn ignition_is_on_if_either_source_says_so() {
        // The light model used to take the BCM's word first: BCM off and the
        // ignition switch on read as off, and night never followed the lights.
        let now = Instant::now();
        let mut s = carpi_state::Store::new();
        let t = Duration::from_secs(5);
        s.set(
            "ignition.kl15",
            Some(carpi_state::Value::Bool(false)),
            now,
            t,
        );
        s.set(
            "ignition.zas.kl15",
            Some(carpi_state::Value::Bool(true)),
            now,
            t,
        );
        assert_eq!(Inputs::from_store(&s, now).kl15, Some(true));
        s.set(
            "ignition.zas.kl15",
            Some(carpi_state::Value::Bool(false)),
            now,
            t,
        );
        assert_eq!(Inputs::from_store(&s, now).kl15, Some(false));
    }

    fn parked() -> Inputs {
        // The parked daytime baseline (PLAN 2.6).
        Inputs {
            cluster_sensor: Some(35.0),
            sun_left_wm2: Some(100.0),
            sun_right_wm2: Some(68.0),
            dim_display_pct: Some(37.0),
            dim_switch_pct: Some(0.0),
            kl15: Some(true),
            ..Inputs::default()
        }
    }

    #[test]
    fn lights_on_sources() {
        assert_eq!(Inputs::default().lights_on(), None);
        let l = |side, low| Inputs {
            side: Some(side),
            low_beam: Some(low),
            ..Inputs::default()
        };
        assert_eq!(l(false, false).lights_on(), Some(false));
        assert_eq!(l(true, false).lights_on(), Some(true));
        let sw = |p| Inputs {
            dim_switch_pct: Some(p),
            ..Inputs::default()
        };
        assert_eq!(sw(0.0).lights_on(), Some(false));
        assert_eq!(sw(30.0).lights_on(), Some(true));
    }

    #[test]
    fn sender_rate_limit() {
        let t0 = Instant::now();
        let mut s = Sender::default();
        assert_eq!(s.update(t0, None), None);
        assert_eq!(s.update(t0, Some(50.2)), Some(50));
        // Under 2 %: nothing new.
        assert_eq!(s.update(t0 + ms(600), Some(51.4)), Some(50));
        // 2 % but too soon after the last.
        assert_eq!(s.update(t0 + ms(100), Some(52.0)), Some(50));
        assert_eq!(s.update(t0 + ms(499), Some(60.0)), Some(50));
        assert_eq!(s.update(t0 + ms(500), Some(60.0)), Some(60));
        // A target sweeping every tick: at most 2 Hz.
        let mut changes = 0;
        let mut last = s.level();
        for k in 1..=50 {
            let l = s.update(t0 + ms(500 + 100 * k), Some(60.0 - k as f64));
            if l != last {
                changes += 1;
                last = l;
            }
        }
        assert!(changes <= 10, "{changes} changes in 5 s");
        // Onto the floor even by 1 %; nothing below it.
        let mut s = Sender::default();
        s.update(t0, Some(7.0));
        assert_eq!(s.update(t0 + ms(600), Some(2.0)), Some(FLOOR_PCT));
        // Stale target: hold.
        assert_eq!(s.update(t0 + ms(1200), None), Some(FLOOR_PCT));
    }

    #[test]
    fn parked_baseline_and_trim() {
        let t0 = Instant::now();
        let mut m = LightModel::new(Setup::default());
        let l = m.update(t0, &parked());
        let a = l.ambient.unwrap();
        assert!((a - 36f64.log10()).abs() < 1e-9);
        // 37 % display dimming: a small step down.
        assert!((l.trim_pct - TRIM_GAIN * (37.0 - TRIM_REF)).abs() < 1e-9);
        let p = l.panel_pct.unwrap();
        assert!((48..=62).contains(&p), "{p}");
        assert!(!l.night);
        assert_eq!(l.night_source, Some(NightSource::Headlights));
        // Trim off: the plain curve.
        let mut m2 = LightModel::new(Setup {
            scroll_trim: false,
            ..Setup::default()
        });
        let l2 = m2.update(t0, &parked());
        assert_eq!(l2.trim_pct, 0.0);
        assert!(l2.target_pct.unwrap() > l.target_pct.unwrap());
        // No ambient: nothing to send yet.
        let mut m3 = LightModel::new(Setup::default());
        assert_eq!(m3.update(t0, &Inputs::default()).panel_pct, None);
    }

    #[test]
    fn nudge_learns_and_reset_forgets() {
        let t0 = Instant::now();
        let mut m = LightModel::new(Setup {
            scroll_trim: false,
            ..Setup::default()
        });
        assert!(m.nudge(5).is_err(), "no ambient yet");
        let before = m.update(t0, &parked()).target_pct.unwrap();
        let stored = m.nudge(10).unwrap();
        assert_eq!(stored as f64, before.round() + 10.0);
        assert_eq!(m.learned().len(), 1);
        assert_eq!(m.learned()[0].0, AmbientSource::Cluster);
        let l = m.update(t0 + ms(600), &parked());
        assert_eq!(l.panel_pct, Some(stored));
        assert!(m.curve_json().contains("\"learned\":[[1.56,"));
        // Learned points survive a settings round trip (what carpid stores).
        let saved = m.setup().clone();
        let mut m2 = LightModel::new(saved);
        let l2 = m2.update(t0, &parked());
        assert_eq!(l2.panel_pct, Some(stored));
        // A new source learns afresh; the cluster points stay.
        m2.set_setup(Setup {
            ambient_source: AmbientSource::Climatronic,
            ..m2.setup().clone()
        });
        m2.update(t0, &parked());
        m2.nudge(-5).unwrap();
        assert_eq!(m2.learned().len(), 2);
        m2.reset_learned(None);
        assert_eq!(m2.learned(), &vec![m.learned()[0]]);
        // Manual mode and auto off refuse.
        m2.set_setup(Setup {
            curve_mode: CurveMode::Manual,
            ..m2.setup().clone()
        });
        assert!(m2.nudge(5).is_err());
        m2.set_setup(Setup {
            auto_brightness: false,
            curve_mode: CurveMode::Learned,
            ..m2.setup().clone()
        });
        assert!(m2.nudge(5).is_err());
    }

    #[test]
    fn curve_json_shape() {
        let m = LightModel::new(Setup::default());
        let j = m.curve_json();
        assert!(
            j.starts_with(
                "{\"source\":\"cluster\",\"mode\":\"learned\",\"trim\":0.0,\"points\":[[0.00,12],"
            ),
            "{j}"
        );
        assert!(j.ends_with("],\"learned\":[]}"), "{j}");
        assert_eq!(j.matches('[').count(), 2 + 25);
    }

    #[test]
    fn from_store_reads_names() {
        let t0 = Instant::now();
        let mut s = Store::new();
        let to = Duration::from_secs(1);
        s.set("light.dim_switch_pct", Some(12.0.into()), t0, to);
        s.set("lights.side", Some(false.into()), t0, to);
        s.set("light.dim_sensor", Some(35.0.into()), t0, to);
        s.set("ignition.zas.kl15", Some(true.into()), t0, to);
        s.set(
            "clock.datetime",
            Some(Value::Text("2026-11-07T19:05:30".into())),
            t0,
            to,
        );
        let i = Inputs::from_store(&s, t0);
        assert_eq!(i.dim_switch_pct, Some(12.0));
        assert_eq!(i.raw().cluster, Some(35.0));
        assert_eq!(i.kl15, Some(true));
        assert_eq!(i.clock.unwrap().second, 19 * 3600 + 5 * 60 + 30);
        assert_eq!(i.lights_on(), Some(true));
        assert_eq!(Inputs::from_store(&s, t0 + ms(1500)), Inputs::default());
    }
}
