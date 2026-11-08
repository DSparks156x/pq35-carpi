//! Ambient light: one log-scale estimate from the configured source,
//! smoothed and deadbanded.

use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AmbientSource {
    /// 0x621 `KO1_Lichtsensor` (else 0x635 `DI1_Sensor`). Drew's default.
    #[default]
    Cluster,
    /// 0x3E3 sun load, W/m2.
    Climatronic,
    /// Cluster, sun load filling in the bright end once the cluster
    /// sensor saturates.
    Blend,
}

impl AmbientSource {
    pub fn as_str(self) -> &'static str {
        match self {
            AmbientSource::Cluster => "cluster",
            AmbientSource::Climatronic => "climatronic",
            AmbientSource::Blend => "blend",
        }
    }
}

/// Cluster sensor raw value treated as saturated. Guess: 254/255 are the
/// invalid codes, so the top of the valid range. A capture in full sun
/// will say where it really stops.
pub const CLUSTER_SATURATION: f64 = 250.0;

/// `log10(1 + raw)`: 0 at a dark sensor, about 2.4 for the cluster's top,
/// about 3 for 1000 W/m2.
pub fn log_scale(raw: f64) -> f64 {
    (raw.max(0.0) + 1.0).log10()
}

/// Where the estimate's scale ends for the plot.
pub fn scale_top(src: AmbientSource) -> f64 {
    match src {
        AmbientSource::Cluster => log_scale(CLUSTER_SATURATION),
        AmbientSource::Climatronic => log_scale(1016.0),
        AmbientSource::Blend => log_scale(CLUSTER_SATURATION) + 1.0,
    }
}

/// Raw readings; `sun` is the mean of the left and right sensors.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Raw {
    pub cluster: Option<f64>,
    pub sun: Option<f64>,
}

/// The unsmoothed estimate. Keeps Blend's anchor between calls.
#[derive(Clone, Debug, Default)]
pub struct Estimator {
    /// Blend: the sun load (log) when the cluster sensor was last below
    /// saturation. Above saturation, the estimate climbs from the cluster's
    /// top by however much the sun load has risen since, so there's no jump.
    anchor: Option<f64>,
}

impl Estimator {
    pub fn estimate(&mut self, src: AmbientSource, raw: Raw) -> Option<f64> {
        match src {
            AmbientSource::Cluster => raw.cluster.map(log_scale),
            AmbientSource::Climatronic => raw.sun.map(log_scale),
            AmbientSource::Blend => {
                let c = raw.cluster?;
                let sun = raw.sun.map(log_scale);
                if c < CLUSTER_SATURATION {
                    self.anchor = sun;
                    return Some(log_scale(c));
                }
                let top = log_scale(CLUSTER_SATURATION);
                match (sun, self.anchor) {
                    (Some(s), Some(a)) => Some(top + (s - a).max(0.0)),
                    (Some(s), None) => {
                        self.anchor = Some(s);
                        Some(top)
                    }
                    (None, _) => Some(top),
                }
            }
        }
    }

    pub fn reset(&mut self) {
        self.anchor = None;
    }
}

/// Exponential smoothing in time, then a deadband on the output.
#[derive(Clone, Debug)]
pub struct Smoother {
    /// Time constant. 1.5 s: about 86 % of a step after 3 s.
    pub tau: Duration,
    /// The output moves only once the smoothed value is this far away (log).
    pub deadband: f64,
    /// A gap longer than this starts afresh from the next sample.
    pub gap: Duration,
    state: Option<(f64, Instant)>,
    out: Option<f64>,
}

impl Default for Smoother {
    fn default() -> Self {
        Smoother::new(Duration::from_millis(1500), 0.05)
    }
}

impl Smoother {
    pub fn new(tau: Duration, deadband: f64) -> Self {
        Smoother {
            tau,
            deadband,
            gap: Duration::from_secs(10),
            state: None,
            out: None,
        }
    }

    /// `None` while the input is stale.
    pub fn update(&mut self, now: Instant, x: Option<f64>) -> Option<f64> {
        let Some(x) = x else {
            if let Some((_, t)) = self.state
                && now.saturating_duration_since(t) > self.gap
            {
                self.reset();
            }
            return None;
        };
        let s = match self.state {
            Some((s, t)) if now.saturating_duration_since(t) <= self.gap => {
                let dt = now.saturating_duration_since(t).as_secs_f64();
                let alpha = 1.0 - (-dt / self.tau.as_secs_f64().max(1e-3)).exp();
                s + alpha * (x - s)
            }
            _ => {
                self.out = Some(x);
                x
            }
        };
        self.state = Some((s, now));
        if self.out.is_none_or(|o| (s - o).abs() >= self.deadband) {
            self.out = Some(s);
        }
        self.out
    }

    pub fn reset(&mut self) {
        self.state = None;
        self.out = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn log_scale_points() {
        assert_eq!(log_scale(0.0), 0.0);
        assert!((log_scale(9.0) - 1.0).abs() < 1e-12);
        assert!((log_scale(99.0) - 2.0).abs() < 1e-12);
        assert_eq!(log_scale(-5.0), 0.0);
        // The parked baseline: cluster 35, sun mean 84.
        let mut e = Estimator::default();
        let raw = Raw {
            cluster: Some(35.0),
            sun: Some(84.0),
        };
        let c = e.estimate(AmbientSource::Cluster, raw).unwrap();
        assert!((c - 36f64.log10()).abs() < 1e-12);
        let s = e.estimate(AmbientSource::Climatronic, raw).unwrap();
        assert!((s - 85f64.log10()).abs() < 1e-12);
        assert_eq!(e.estimate(AmbientSource::Climatronic, Raw::default()), None);
    }

    #[test]
    fn blend_fills_only_the_bright_end() {
        let mut e = Estimator::default();
        let b =
            |e: &mut Estimator, c, s| e.estimate(AmbientSource::Blend, Raw { cluster: c, sun: s });
        // Below saturation: the cluster alone, whatever the sun says.
        assert_eq!(b(&mut e, Some(35.0), Some(400.0)), Some(log_scale(35.0)));
        assert_eq!(b(&mut e, Some(200.0), Some(500.0)), Some(log_scale(200.0)));
        // Saturated: starts at the cluster's top, climbs with the sun.
        let top = log_scale(CLUSTER_SATURATION);
        assert_eq!(b(&mut e, Some(250.0), Some(500.0)), Some(top));
        let up = b(&mut e, Some(253.0), Some(1000.0)).unwrap();
        assert!((up - (top + log_scale(1000.0) - log_scale(500.0))).abs() < 1e-12);
        // Sun dropping below the anchor doesn't take it under the top.
        assert_eq!(b(&mut e, Some(253.0), Some(100.0)), Some(top));
        assert_eq!(b(&mut e, Some(253.0), None), Some(top));
        // No cluster: nothing (the sun alone is on another scale).
        assert_eq!(b(&mut e, None, Some(500.0)), None);
    }

    #[test]
    fn smoothing_and_deadband() {
        let t0 = Instant::now();
        let mut s = Smoother::default();
        assert_eq!(s.update(t0, Some(1.0)), Some(1.0));
        // Small wobble stays inside the deadband.
        for i in 1..=30 {
            let x = if i % 2 == 0 { 1.04 } else { 0.97 };
            assert_eq!(s.update(t0 + ms(100 * i), Some(x)), Some(1.0));
        }
        // A 0.3 s shadow, a whole decade down, barely moves it.
        let t1 = t0 + ms(3000);
        let mut low: f64 = 1.0;
        for i in 1..=3 {
            low = low.min(s.update(t1 + ms(100 * i), Some(0.0)).unwrap());
        }
        assert!(low > 0.75, "shadow pulled it to {low}");
        // A tunnel: mostly there after 3 s, all there after 6 s.
        let t2 = t1 + ms(3000);
        for i in 1..=60 {
            s.update(t2 + ms(100 * i), Some(0.0));
        }
        let at3 = {
            let mut s2 = Smoother::default();
            s2.update(t0, Some(1.0));
            (1..=30).fold(None, |_, i| s2.update(t0 + ms(100 * i), Some(0.0)))
        }
        .unwrap();
        assert!(at3 < 0.25, "after 3 s: {at3}");
        assert!(s.update(t2 + ms(6100), Some(0.0)).unwrap() < 0.1);
        // Stale: None, but a short gap carries on from where it was.
        let t3 = t2 + ms(7000);
        assert_eq!(s.update(t3, None), None);
        assert!(s.update(t3 + ms(500), Some(0.0)).unwrap() < 0.1);
        // A long gap starts afresh.
        assert_eq!(s.update(t3 + ms(20_000), None), None);
        assert_eq!(s.update(t3 + ms(20_100), Some(2.0)), Some(2.0));
    }
}
