//! The light model: one place that decides day or night.
//!
//! Three consumers read its output: `carpi-rnse` (BB panel level), the LIVI
//! bridge (dark mode, AA night sensor) and the DIS.
//!
//! Choice of inputs:
//! - **Night = the car's lights are on**: `lights.side`, `lights.low_beam`
//!   (0x470) or `light.dim_switch_pct` > 0 (0x635 58s, switch illumination).
//!   58s is what the old code and the RNS-E's own auto day/night use; the
//!   0x470 lamp bits say the same thing from another message, so either
//!   being on counts. The driver or the auto-light sensor has already made
//!   the call, and it's the one signal all three consumers agree with.
//! - **Not the light sensors** (`light.dim_sensor`, `light.cluster_sensor`):
//!   their scale and polarity aren't known yet, so they're passed through as
//!   `ambient` (raw) and decide nothing. Once a capture calibrates them they
//!   can join the night decision here and nowhere else.
//! - **Not 58d/58t** (`light.dim_display_pct`, `light.kl58t_pct`): those
//!   are the dimmer wheel, i.e. how bright, not whether it's dark. The RNS-E
//!   already follows 58d in `FollowCar` mode. Passed through as `dimmer_pct`.
//!
//! Hysteresis is in time: lights on for `night_after` → night, off for
//! `day_after` → day. That rides through flash-to-pass and short tunnels.
//! Unknown inputs (stale, ignition off) hold the last state.

use carpi_state::Store;
use std::time::{Duration, Instant};

/// Store values, already decoded by `carpi-signals`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Inputs {
    pub dim_display_pct: Option<f64>,
    pub dim_switch_pct: Option<f64>,
    pub dim_sensor: Option<f64>,
    pub cluster_sensor: Option<f64>,
    pub kl58t_pct: Option<f64>,
    pub low_beam: Option<bool>,
    pub side: Option<bool>,
}

impl Inputs {
    pub fn from_store(s: &Store, now: Instant) -> Self {
        Inputs {
            dim_display_pct: s.get_num("light.dim_display_pct", now),
            dim_switch_pct: s.get_num("light.dim_switch_pct", now),
            dim_sensor: s.get_num("light.dim_sensor", now),
            cluster_sensor: s.get_num("light.cluster_sensor", now),
            kl58t_pct: s.get_num("light.kl58t_pct", now),
            low_beam: s.get_bool("lights.low_beam", now),
            side: s.get_bool("lights.side", now),
        }
    }

    /// Any light on → `Some(true)`; none on but one known → `Some(false)`.
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
}

/// Panel brightness, mapped by the caller onto RNS-E `BB`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelLevel {
    FollowCar,
    /// 1-100.
    Level(u8),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Light {
    pub night: bool,
    /// Raw lights-on input before hysteresis; `None` if unknown.
    pub lights_on: Option<bool>,
    pub panel_level: PanelLevel,
    /// Raw light sensor (0x635, else 0x621). Scale and polarity unknown.
    pub ambient: Option<f64>,
    /// Dimmer wheel, 58d (0x635), else 58t (0x470).
    pub dimmer_pct: Option<f64>,
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub night_after: Duration,
    pub day_after: Duration,
    pub day_panel: PanelLevel,
    pub night_panel: PanelLevel,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            night_after: Duration::from_secs(2),
            day_after: Duration::from_secs(10),
            day_panel: PanelLevel::FollowCar,
            night_panel: PanelLevel::FollowCar,
        }
    }
}

pub struct LightModel {
    cfg: Config,
    night: bool,
    /// The other state, and since when the input has asked for it.
    pending: Option<(bool, Instant)>,
}

impl LightModel {
    pub fn new(cfg: Config) -> Self {
        LightModel {
            cfg,
            night: false,
            pending: None,
        }
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    pub fn set_config(&mut self, cfg: Config) {
        self.cfg = cfg;
    }

    /// Call when any input changes and at `next_deadline`.
    pub fn update(&mut self, now: Instant, i: &Inputs) -> Light {
        let lights_on = i.lights_on();
        match lights_on {
            None => self.pending = None,
            Some(want) if want == self.night => self.pending = None,
            Some(want) => {
                let since = match self.pending {
                    Some((w, t)) if w == want => t,
                    _ => now,
                };
                self.pending = Some((want, since));
                if now.saturating_duration_since(since) >= self.delay(want) {
                    self.night = want;
                    self.pending = None;
                }
            }
        }
        Light {
            night: self.night,
            lights_on,
            panel_level: if self.night {
                self.cfg.night_panel
            } else {
                self.cfg.day_panel
            },
            ambient: i.dim_sensor.or(i.cluster_sensor),
            dimmer_pct: i.dim_display_pct.or(i.kl58t_pct),
        }
    }

    /// When a pending day/night switch will land if inputs hold.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.pending.map(|(want, t)| t + self.delay(want))
    }

    fn delay(&self, night: bool) -> Duration {
        if night {
            self.cfg.night_after
        } else {
            self.cfg.day_after
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn lights(side: bool, low: bool) -> Inputs {
        Inputs {
            side: Some(side),
            low_beam: Some(low),
            ..Inputs::default()
        }
    }

    #[test]
    fn lights_on_sources() {
        assert_eq!(Inputs::default().lights_on(), None);
        assert_eq!(lights(false, false).lights_on(), Some(false));
        assert_eq!(lights(true, false).lights_on(), Some(true));
        let sw = |p| Inputs {
            dim_switch_pct: Some(p),
            ..Inputs::default()
        };
        assert_eq!(sw(0.0).lights_on(), Some(false));
        assert_eq!(sw(30.0).lights_on(), Some(true));
    }

    #[test]
    fn hysteresis() {
        let t0 = Instant::now();
        let mut m = LightModel::new(Config::default());
        assert!(!m.update(t0, &lights(false, false)).night);
        // On, but not long enough.
        assert!(!m.update(t0 + ms(100), &lights(false, true)).night);
        assert_eq!(m.next_deadline(), Some(t0 + ms(2100)));
        assert!(!m.update(t0 + ms(2000), &lights(false, true)).night);
        assert!(m.update(t0 + ms(2100), &lights(false, true)).night);
        assert_eq!(m.next_deadline(), None);
        // Brief off (flash, tunnel exit) doesn't flip back.
        assert!(m.update(t0 + ms(3000), &lights(false, false)).night);
        assert!(m.update(t0 + ms(5000), &lights(true, false)).night);
        assert!(m.update(t0 + ms(12_000), &lights(false, false)).night);
        assert!(m.update(t0 + ms(21_999), &lights(false, false)).night);
        assert!(!m.update(t0 + ms(22_000), &lights(false, false)).night);
    }

    #[test]
    fn unknown_holds() {
        let t0 = Instant::now();
        let mut m = LightModel::new(Config::default());
        m.update(t0, &lights(true, true));
        assert!(m.update(t0 + ms(2000), &lights(true, true)).night);
        // Ignition off: everything stale.
        let l = m.update(t0 + ms(60_000), &Inputs::default());
        assert!(l.night);
        assert_eq!(l.lights_on, None);
        assert_eq!(m.next_deadline(), None);
    }

    #[test]
    fn outputs() {
        let t0 = Instant::now();
        let mut m = LightModel::new(Config {
            day_panel: PanelLevel::Level(100),
            night_panel: PanelLevel::Level(20),
            night_after: Duration::ZERO,
            ..Config::default()
        });
        let mut i = lights(false, false);
        i.cluster_sensor = Some(35.0);
        i.kl58t_pct = Some(58.0);
        let l = m.update(t0, &i);
        assert_eq!(l.panel_level, PanelLevel::Level(100));
        assert_eq!(l.ambient, Some(35.0));
        assert_eq!(l.dimmer_pct, Some(58.0));
        i.dim_sensor = Some(40.0);
        i.dim_display_pct = Some(37.0);
        i.side = Some(true);
        let l = m.update(t0, &i);
        assert!(l.night);
        assert_eq!(l.panel_level, PanelLevel::Level(20));
        assert_eq!((l.ambient, l.dimmer_pct), (Some(40.0), Some(37.0)));
    }

    #[test]
    fn from_store_reads_names() {
        let t0 = Instant::now();
        let mut s = Store::new();
        let to = Duration::from_secs(1);
        s.set("light.dim_switch_pct", Some(12.0.into()), t0, to);
        s.set("lights.side", Some(false.into()), t0, to);
        s.set("light.dim_sensor", Some(35.0.into()), t0, to);
        let i = Inputs::from_store(&s, t0);
        assert_eq!(i.dim_switch_pct, Some(12.0));
        assert_eq!(i.side, Some(false));
        assert_eq!(i.dim_sensor, Some(35.0));
        assert_eq!(i.lights_on(), Some(true));
        assert_eq!(Inputs::from_store(&s, t0 + ms(1500)), Inputs::default());
    }
}
