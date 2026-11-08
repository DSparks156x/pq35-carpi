//! Ambient (log scale) -> RNS-E panel %.
//!
//! Learned mode works like phone auto-brightness: each nudge stores a point
//! (ambient, pct) and the curve bends towards it near that ambient, falling
//! back to the default curve away from it.

use crate::ambient::AmbientSource;

/// The firmware treats `BB` 1-5 as 6.
pub const FLOOR_PCT: u8 = 6;

/// How far a learned point reaches (log units; 0.25 is a factor of 1.8).
pub const LEARN_REACH: f64 = 0.25;
/// A nudge this close to an old point replaces it.
pub const LEARN_MERGE: f64 = 0.1;
/// Learned points kept per source; the oldest go first.
pub const LEARN_MAX: usize = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CurveMode {
    Manual,
    #[default]
    Learned,
}

/// The default curve per source. Guesses until a daylight-to-dark capture
/// seeds them (PLAN 2.6 open item). The night end sits at 12 %; the parked
/// daytime baseline (cluster 35, sun 84 W/m2) lands near 55-58 %.
pub fn default_curve(src: AmbientSource) -> &'static [(f64, u8)] {
    match src {
        AmbientSource::Cluster | AmbientSource::Blend => &[
            (0.0, 12),
            (0.6, 18),
            (1.2, 40),
            (1.7, 65),
            (2.1, 85),
            (2.4, 100),
        ],
        AmbientSource::Climatronic => &[
            (0.0, 12),
            (1.0, 22),
            (1.6, 40),
            (2.2, 65),
            (2.7, 85),
            (3.0, 100),
        ],
    }
}

/// Piecewise linear, flat beyond the ends. Empty: `None`.
pub fn interp(points: &[(f64, f64)], a: f64) -> Option<f64> {
    let first = points.first()?;
    if a <= first.0 {
        return Some(first.1);
    }
    for w in points.windows(2) {
        let ((a0, p0), (a1, p1)) = (w[0], w[1]);
        if a <= a1 {
            let f = if a1 > a0 { (a - a0) / (a1 - a0) } else { 1.0 };
            return Some(p0 + f * (p1 - p0));
        }
    }
    points.last().map(|p| p.1)
}

/// The curve in use: a base and, in learned mode, the learned points.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Curve {
    pub base: Vec<(f64, f64)>,
    pub learned: Vec<(f64, f64)>,
}

impl Curve {
    pub fn new(
        mode: CurveMode,
        src: AmbientSource,
        manual: &[(f64, u8)],
        learned: &[(f64, u8)],
    ) -> Self {
        let pts = |p: &[(f64, u8)]| p.iter().map(|&(a, p)| (a, p as f64)).collect();
        match mode {
            CurveMode::Manual if !manual.is_empty() => Curve {
                base: pts(manual),
                learned: Vec::new(),
            },
            CurveMode::Manual => Curve {
                base: pts(default_curve(src)),
                learned: Vec::new(),
            },
            CurveMode::Learned => Curve {
                base: pts(default_curve(src)),
                learned: pts(learned),
            },
        }
    }

    /// Panel % at `a`, before scroll trim, within [`FLOOR_PCT`]-100.
    pub fn eval(&self, a: f64) -> f64 {
        let base = |x| interp(&self.base, x).unwrap_or(50.0);
        let mut wsum = 0.0;
        let mut acc = 0.0;
        for &(la, lp) in &self.learned {
            let w = (-((a - la) / LEARN_REACH).powi(2)).exp();
            wsum += w;
            acc += w * (lp - base(la));
        }
        // Normalised once several points overlap, so two nudges at one
        // ambient average rather than add.
        let v = base(a) + acc / f64::max(wsum, 1.0);
        v.clamp(FLOOR_PCT as f64, 100.0)
    }
}

/// Learned points of every source; a new source learns afresh and the old
/// points stay.
pub type Learned = Vec<(AmbientSource, f64, u8)>;

/// Stores a nudge of `delta` at ambient `a` for `src`; returns the level
/// stored. `curve` is the curve before the nudge.
pub fn nudge(learned: &mut Learned, curve: &Curve, src: AmbientSource, a: f64, delta: i32) -> u8 {
    let now = curve.eval(a).round() as i32;
    let pct = (now + delta).clamp(FLOOR_PCT as i32, 100) as u8;
    learned.retain(|&(s, la, _)| !(s == src && (la - a).abs() < LEARN_MERGE));
    learned.push((src, a, pct));
    let mine = learned.iter().filter(|p| p.0 == src).count();
    if mine > LEARN_MAX
        && let Some(i) = learned.iter().position(|p| p.0 == src)
    {
        learned.remove(i);
    }
    pct
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: AmbientSource = AmbientSource::Cluster;

    fn learned_curve(l: &Learned) -> Curve {
        let mine: Vec<(f64, u8)> = l.iter().filter(|p| p.0 == C).map(|p| (p.1, p.2)).collect();
        Curve::new(CurveMode::Learned, C, &[], &mine)
    }

    #[test]
    fn defaults() {
        for src in [C, AmbientSource::Climatronic] {
            let d = default_curve(src);
            assert!((10..=15).contains(&d[0].1), "night end");
            assert!(d.iter().all(|p| p.1 >= FLOOR_PCT));
            assert!(d.windows(2).all(|w| w[0].0 < w[1].0 && w[0].1 <= w[1].1));
        }
        let c = Curve::new(CurveMode::Learned, C, &[], &[]);
        let day = c.eval(crate::ambient::log_scale(35.0));
        assert!((50.0..65.0).contains(&day), "baseline {day}");
        assert_eq!(c.eval(-1.0), 12.0);
        assert_eq!(c.eval(9.0), 100.0);
    }

    #[test]
    fn interpolates_and_manual() {
        let p = [(0.0, 10.0), (1.0, 30.0), (2.0, 70.0)];
        assert_eq!(interp(&p, 0.5), Some(20.0));
        assert_eq!(interp(&p, 1.5), Some(50.0));
        assert_eq!(interp(&p, 3.0), Some(70.0));
        assert_eq!(interp(&[], 1.0), None);
        let m = Curve::new(CurveMode::Manual, C, &[(0.0, 2), (2.0, 80)], &[(1.0, 99)]);
        assert_eq!(m.eval(1.0), 41.0, "learned points don't apply in manual");
        assert_eq!(m.eval(0.0), FLOOR_PCT as f64, "floor");
    }

    #[test]
    fn nudges_move_the_curve_locally() {
        let mut l = Learned::new();
        let before = learned_curve(&l);
        let a = 1.5;
        let stored = nudge(&mut l, &before, C, a, 10);
        assert_eq!(stored as f64, before.eval(a).round() + 10.0);
        let after = learned_curve(&l);
        assert!((after.eval(a) - stored as f64).abs() < 0.5);
        // Near: most of it. Far: nothing.
        assert!(after.eval(a + 0.1) - before.eval(a + 0.1) > 7.0);
        assert!((after.eval(0.0) - before.eval(0.0)).abs() < 0.5);
        assert!((after.eval(2.4) - before.eval(2.4)).abs() < 0.5);
        // A second nudge at the same ambient replaces the point.
        nudge(&mut l, &after, C, a + 0.05, 5);
        assert_eq!(l.len(), 1);
        // Learning can go down to the floor, not below.
        let mut l = Learned::new();
        assert_eq!(nudge(&mut l, &before, C, 0.0, -50), FLOOR_PCT);
        // Another source's points are kept and capped separately.
        let mut l = vec![(AmbientSource::Climatronic, 1.0, 50)];
        for i in 0..40 {
            let c = learned_curve(&l);
            nudge(&mut l, &c, C, i as f64 * 0.11, 1);
        }
        assert_eq!(l.iter().filter(|p| p.0 == C).count(), LEARN_MAX);
        assert_eq!(l[0].0, AmbientSource::Climatronic);
    }
}
