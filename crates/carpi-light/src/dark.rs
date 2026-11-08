//! Night / dark mode (PLAN 2.6).
//!
//! **Headlights** (default): the car's lights decide while Kl15 is on, with
//! the old repo's conditioning against brief flips
//! (`RNS-E-Hudiy/hudiy_client/dark_mode_api.py`, `LightFilter`, lines
//! 242-312):
//! - night is followed at once (:301);
//! - day only once the lights have read off for [`DAY_CONFIRM`] (4 s, :242)
//!   with the ignition on, counted from ignition-on at the earliest
//!   (:289-292, :305). KL58 drops while the ignition is off and comes back
//!   a moment after it's on; without this every ignition cycle would flash
//!   the light theme;
//! - the off-timer starts when the reading turns off, not on every frame
//!   (:276-277), and ignition-on only counts from a known off (:283-284);
//! - invalid readings are ignored and the last valid one kept (:274-275);
//! - the first valid reading is trusted as is (:298-300).
//!
//! The old code held its state while Kl15 was off (:303-304), because the
//! lights always read off then. Here the car's clock takes over instead:
//! dark between sunset and sunrise at the configured location, or between
//! fixed evening and morning hours without one. Once Kl15 is on and the
//! headlight signal is valid, headlights decide again straight away; the
//! state carries over, so clock-night stays night through the KL58 gap.
//!
//! **Ambient**: the brightness model's smoothed ambient against a threshold
//! with hysteresis and a minimum dwell. While the sensor is stale, the
//! clock decides.
//!
//! Nothing known at all: the last state holds.

use crate::sun::{Location, sun_day};
use std::time::{Duration, Instant};

/// `DAY_CONFIRM` in `dark_mode_api.py:242`.
pub const DAY_CONFIRM: Duration = Duration::from_secs(4);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DarkSource {
    #[default]
    Headlights,
    Ambient,
}

/// What decided the current state; `light.night_source`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NightSource {
    Headlights,
    /// Sun times at the configured location.
    Clock,
    /// Fixed hours (no location set).
    Fallback,
    Ambient,
}

impl NightSource {
    pub fn as_str(self) -> &'static str {
        match self {
            NightSource::Headlights => "headlights",
            NightSource::Clock => "clock",
            NightSource::Fallback => "fallback",
            NightSource::Ambient => "ambient",
        }
    }
}

/// The car clock (0x65D), local time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    /// Seconds after local midnight.
    pub second: u32,
}

impl ClockTime {
    /// `clock.datetime`: `YYYY-MM-DDTHH:MM:SS`.
    pub fn parse(s: &str) -> Option<Self> {
        let (date, time) = s.split_once('T')?;
        let mut d = date.splitn(3, '-');
        let year = d.next()?.parse().ok()?;
        let month = d.next()?.parse().ok()?;
        let day = d.next()?.parse().ok()?;
        let mut t = time.splitn(3, ':');
        let h: u32 = t.next()?.parse().ok()?;
        let m: u32 = t.next()?.parse().ok()?;
        let sec: u32 = t.next().unwrap_or("0").parse().ok()?;
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) || h > 23 || m > 59 || sec > 59 {
            return None;
        }
        Some(ClockTime {
            year,
            month,
            day,
            second: h * 3600 + m * 60 + sec,
        })
    }

    pub fn minute(&self) -> f64 {
        self.second as f64 / 60.0
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DarkSetup {
    pub source: DarkSource,
    /// Ambient mode: night below this (log ambient)...
    pub threshold: f64,
    /// ...day again above threshold + hysteresis...
    pub hysteresis: f64,
    /// ...each only after the ambient has asked for it this long.
    pub dwell: Duration,
    pub location: Option<Location>,
    /// Without a location: night from this minute of the day...
    pub night_from: u16,
    /// ...until this one.
    pub day_from: u16,
}

impl Default for DarkSetup {
    fn default() -> Self {
        DarkSetup {
            source: DarkSource::Headlights,
            // Guesses (cluster scale: raw 9 / 14).
            threshold: 1.0,
            hysteresis: 0.2,
            dwell: Duration::from_secs(10),
            location: None,
            night_from: 19 * 60,
            day_from: 7 * 60,
        }
    }
}

impl DarkSetup {
    /// Dark by the clock, and which rule said so.
    pub fn clock_dark(&self, c: &ClockTime) -> (bool, NightSource) {
        match &self.location {
            Some(loc) => (
                sun_day(c.year, c.month, c.day, loc).dark_at(c.minute()),
                NightSource::Clock,
            ),
            None => {
                let m = c.minute();
                let (nf, df) = (self.night_from as f64, self.day_from as f64);
                let dark = if nf > df {
                    m >= nf || m < df
                } else {
                    m >= nf && m < df
                };
                (dark, NightSource::Fallback)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DarkInputs {
    /// Any of the car's lights on; `None` if invalid or stale.
    pub lights_on: Option<bool>,
    pub kl15: Option<bool>,
    pub clock: Option<ClockTime>,
    /// The smoothed ambient (log).
    pub ambient: Option<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct DarkMode {
    night: bool,
    source: Option<NightSource>,
    decided: bool,
    // Headlight filter (old `LightFilter`).
    reading: Option<bool>,
    day_since: Option<Instant>,
    kl15: Option<bool>,
    ignition_on_at: Option<Instant>,
    // Ambient dwell: the other state, and since when it's been asked for.
    pending: Option<(bool, Instant)>,
}

impl DarkMode {
    pub fn night(&self) -> bool {
        self.night
    }

    pub fn source(&self) -> Option<NightSource> {
        self.source
    }

    pub fn update(&mut self, now: Instant, s: &DarkSetup, i: &DarkInputs) -> bool {
        // The filter follows its inputs whatever decides (old on_ignition /
        // on_reading).
        if let Some(k) = i.kl15
            && Some(k) != self.kl15
        {
            if k && self.kl15 == Some(false) {
                self.ignition_on_at = Some(now);
            }
            self.kl15 = Some(k);
        }
        if let Some(r) = i.lights_on {
            if !r && self.reading != Some(false) {
                self.day_since = Some(now);
            }
            self.reading = Some(r);
        }
        if s.source != DarkSource::Ambient {
            self.pending = None;
        }

        match s.source {
            // Only while the ignition is known on. With it off or unknown (the
            // bus naps and wakes with the car off) the clock decides, so the
            // two rules don't take turns.
            DarkSource::Headlights if self.kl15 == Some(true) && i.lights_on.is_some() => {
                self.headlights(now)
            }
            DarkSource::Ambient if i.ambient.is_some() => {
                self.ambient(now, s, i.ambient.unwrap_or_default())
            }
            _ => {
                self.pending = None;
                if let Some(c) = &i.clock {
                    let (dark, src) = s.clock_dark(c);
                    self.set(dark, src);
                }
            }
        }
        self.night
    }

    fn set(&mut self, night: bool, src: NightSource) {
        self.night = night;
        self.source = Some(src);
        self.decided = true;
    }

    fn headlights(&mut self, now: Instant) {
        let reading = self.reading.unwrap_or(false);
        self.source = Some(NightSource::Headlights);
        if !self.decided || reading || !self.night {
            self.set(reading, NightSource::Headlights);
            return;
        }
        let since = self.day_since.unwrap_or(now);
        let start = self.ignition_on_at.map_or(since, |t| t.max(since));
        if now.saturating_duration_since(start) >= DAY_CONFIRM {
            self.set(false, NightSource::Headlights);
        }
    }

    fn ambient(&mut self, now: Instant, s: &DarkSetup, a: f64) {
        self.source = Some(NightSource::Ambient);
        if !self.decided {
            self.pending = None;
            self.set(a < s.threshold, NightSource::Ambient);
            return;
        }
        let want = if self.night {
            a <= s.threshold + s.hysteresis
        } else {
            a < s.threshold
        };
        if want == self.night {
            self.pending = None;
            return;
        }
        let since = match self.pending {
            Some((w, t)) if w == want => t,
            _ => now,
        };
        self.pending = Some((want, since));
        if now.saturating_duration_since(since) >= s.dwell {
            self.pending = None;
            self.set(want, NightSource::Ambient);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn hl(kl15: bool, on: Option<bool>) -> DarkInputs {
        DarkInputs {
            lights_on: on,
            kl15: Some(kl15),
            ..Default::default()
        }
    }

    fn clock(s: &str) -> Option<ClockTime> {
        ClockTime::parse(s)
    }

    #[test]
    fn parses_the_car_clock() {
        let c = clock("2026-11-07T19:05:30").unwrap();
        assert_eq!((c.year, c.month, c.day, c.second), (2026, 11, 7, 68730));
        assert_eq!(clock("2026-13-07T19:05:30"), None);
        assert_eq!(clock("garbage"), None);
    }

    #[test]
    fn car_off_bus_napping_stays_on_the_clock() {
        // Car off at night: the bus wakes and naps, so Kl15 comes and goes
        // while the light frames still say "off". The clock must decide.
        let t0 = Instant::now();
        let s = DarkSetup::default();
        let mut d = DarkMode::default();
        let night = clock("2026-11-07T22:00:00");
        for n in 0..10u64 {
            let i = DarkInputs {
                kl15: if n % 2 == 0 { Some(false) } else { None },
                lights_on: Some(false),
                clock: night,
                ..Default::default()
            };
            assert!(d.update(t0 + ms(n * 500), &s, &i), "step {n}");
            assert_eq!(d.source(), Some(NightSource::Fallback));
        }
    }

    #[test]
    fn headlights_with_anti_flip() {
        let t0 = Instant::now();
        let s = DarkSetup::default();
        let mut d = DarkMode::default();
        // First valid reading is trusted.
        assert!(!d.update(t0, &s, &hl(true, Some(false))));
        assert_eq!(d.source(), Some(NightSource::Headlights));
        // Night at once.
        assert!(d.update(t0 + ms(100), &s, &hl(true, Some(true))));
        // Flash-to-pass off, tunnel exit: day needs 4 s of off.
        assert!(d.update(t0 + ms(200), &s, &hl(true, Some(false))));
        assert!(d.update(t0 + ms(3000), &s, &hl(true, Some(false))));
        // Invalid frames are ignored, the timer keeps running.
        assert!(d.update(t0 + ms(3500), &s, &hl(true, None)));
        assert!(d.update(t0 + ms(4100), &s, &hl(true, Some(false))));
        assert!(!d.update(t0 + ms(4200), &s, &hl(true, Some(false))));
        // Back on, briefly off again: the timer restarts.
        assert!(d.update(t0 + ms(5000), &s, &hl(true, Some(true))));
        assert!(d.update(t0 + ms(5100), &s, &hl(true, Some(false))));
        assert!(d.update(t0 + ms(6000), &s, &hl(true, Some(true))));
        assert!(d.update(t0 + ms(9500), &s, &hl(true, Some(false))));
        assert!(d.update(t0 + ms(13_400), &s, &hl(true, Some(false))));
        assert!(!d.update(t0 + ms(13_500), &s, &hl(true, Some(false))));
    }

    #[test]
    fn clock_with_kl15_off() {
        // London, 2026-11-07: sunset 16:21 GMT.
        let s = DarkSetup {
            location: Some(Location {
                latitude: 51.5072,
                longitude: -0.1276,
                utc_offset_hours: Some(0.0),
            }),
            ..Default::default()
        };
        let t0 = Instant::now();
        let mut d = DarkMode::default();
        let off = |c| DarkInputs {
            lights_on: Some(false),
            kl15: Some(false),
            clock: clock(c),
            ambient: Some(2.0),
        };
        assert!(!d.update(t0, &s, &off("2026-11-07T16:15:00")));
        assert_eq!(d.source(), Some(NightSource::Clock));
        assert!(d.update(t0, &s, &off("2026-11-07T16:25:00")));
        assert!(d.update(t0, &s, &off("2026-11-08T06:55:00")));
        assert!(!d.update(t0, &s, &off("2026-11-08T07:15:00")));
        // No clock either: hold.
        let none = DarkInputs {
            kl15: Some(false),
            ..Default::default()
        };
        assert!(!d.update(t0, &s, &none));
        // No location: fixed hours.
        let f = DarkSetup::default();
        assert!(!d.update(t0, &f, &off("2026-11-07T18:59:00")));
        assert_eq!(d.source(), Some(NightSource::Fallback));
        assert!(d.update(t0, &f, &off("2026-11-07T19:00:00")));
        assert!(d.update(t0, &f, &off("2026-11-08T06:59:59")));
        assert!(!d.update(t0, &f, &off("2026-11-08T07:00:00")));
        // A daytime-only window works too.
        let w = DarkSetup {
            night_from: 60,
            day_from: 120,
            ..Default::default()
        };
        assert!(d.update(t0, &w, &off("2026-11-08T01:30:00")));
        assert!(!d.update(t0, &w, &off("2026-11-08T02:30:00")));
    }

    #[test]
    fn kl15_on_hands_over_to_headlights() {
        let s = DarkSetup::default();
        let t0 = Instant::now();
        let mut d = DarkMode::default();
        let at = |kl15, on, c| DarkInputs {
            lights_on: on,
            kl15: Some(kl15),
            clock: clock(c),
            ambient: None,
        };
        // Parked at night: clock says night.
        assert!(d.update(t0, &s, &at(false, Some(false), "2026-11-07T21:00:00")));
        assert_eq!(d.source(), Some(NightSource::Fallback));
        // Kl15 on, KL58 still off for a moment: headlights decide at once,
        // but day needs 4 s off from ignition-on, so night holds...
        let c = "2026-11-07T21:00:01";
        assert!(d.update(t0 + ms(100), &s, &at(true, Some(false), c)));
        assert_eq!(d.source(), Some(NightSource::Headlights));
        assert!(d.update(t0 + ms(1500), &s, &at(true, Some(false), c)));
        // ...and the lights come back.
        assert!(d.update(t0 + ms(1600), &s, &at(true, Some(true), c)));
        // Kl15 off again, then on with the lights left off: day after 4 s.
        assert!(d.update(t0 + ms(10_000), &s, &at(false, Some(false), c)));
        assert!(d.update(t0 + ms(20_000), &s, &at(true, Some(false), c)));
        assert!(d.update(t0 + ms(23_900), &s, &at(true, Some(false), c)));
        assert!(!d.update(t0 + ms(24_000), &s, &at(true, Some(false), c)));
        // Clock day, Kl15 on, lights on: night at once.
        let mut d = DarkMode::default();
        assert!(!d.update(t0, &s, &at(false, Some(false), "2026-11-07T12:00:00")));
        assert!(d.update(
            t0 + ms(100),
            &s,
            &at(true, Some(true), "2026-11-07T12:00:00")
        ));
        assert_eq!(d.source(), Some(NightSource::Headlights));
        // Kl15 on but the headlight signal invalid: the clock still decides.
        let mut d = DarkMode::default();
        assert!(d.update(t0, &s, &at(true, None, "2026-11-07T22:00:00")));
        assert_eq!(d.source(), Some(NightSource::Fallback));
    }

    #[test]
    fn ambient_threshold_hysteresis_dwell() {
        let s = DarkSetup {
            source: DarkSource::Ambient,
            threshold: 1.0,
            hysteresis: 0.2,
            dwell: Duration::from_secs(5),
            ..Default::default()
        };
        let t0 = Instant::now();
        let mut d = DarkMode::default();
        let a = |x| DarkInputs {
            lights_on: Some(true),
            kl15: Some(true),
            clock: clock("2026-11-07T12:00:00"),
            ambient: x,
        };
        // Headlights on, but ambient decides in this mode.
        assert!(!d.update(t0, &s, &a(Some(1.5))));
        assert_eq!(d.source(), Some(NightSource::Ambient));
        // Below the threshold, not for the dwell yet.
        assert!(!d.update(t0 + ms(1000), &s, &a(Some(0.8))));
        assert!(!d.update(t0 + ms(5900), &s, &a(Some(0.8))));
        // A blip above resets the dwell.
        assert!(!d.update(t0 + ms(5950), &s, &a(Some(1.1))));
        assert!(!d.update(t0 + ms(6000), &s, &a(Some(0.8))));
        assert!(d.update(t0 + ms(11_000), &s, &a(Some(0.8))));
        // Inside the hysteresis band: stays night.
        assert!(d.update(t0 + ms(30_000), &s, &a(Some(1.15))));
        assert!(d.update(t0 + ms(40_000), &s, &a(Some(1.2))));
        // Above it for the dwell: day.
        assert!(d.update(t0 + ms(41_000), &s, &a(Some(1.3))));
        assert!(!d.update(t0 + ms(46_000), &s, &a(Some(1.3))));
        // Sensor stale: the clock (fixed hours, noon) decides.
        assert!(!d.update(t0 + ms(47_000), &s, &a(None)));
        assert_eq!(d.source(), Some(NightSource::Fallback));
        let night = DarkInputs {
            clock: clock("2026-11-07T23:00:00"),
            ..a(None)
        };
        assert!(d.update(t0 + ms(48_000), &s, &night));
    }
}
