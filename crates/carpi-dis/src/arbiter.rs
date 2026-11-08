//! Who owns each display surface. A fixed priority table, plus the nav policy:
//! auto-switch to nav near a maneuver, return after a peek.
//!
//! Nav policy numbers are the old driver's (`dc/navigation_policy.py:6-58`):
//! approach within 500 m, re-arm beyond 1000 m, 5 s peek on a maneuver change,
//! manual selection suppresses until the maneuver changes or the distance
//! retreats. Route gaps under 3 s are held (`dc/dis_display.py:584-605`).
//! The cluster itself outranks all of this (`53` busy); that lives in the session.

use std::time::{Duration, Instant};

/// Priority order: earlier wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
    Alert,
    NavManeuver,
    Call,
    UserPage,
    Media,
    StockRadio,
    Idle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Surface {
    /// The claimed 64x48 window.
    Centre,
    /// 0x363/0x365, only while the RNS-E is on TV.
    Top,
}

pub struct Rule {
    pub class: Class,
    pub surfaces: &'static [Surface],
}

use Surface::{Centre, Top};

/// The rule table, in priority order. A user page (readings, eggs) is centre
/// only; the top lines carry text sources.
pub const RULES: [Rule; 7] = [
    Rule {
        class: Class::Alert,
        surfaces: &[Centre],
    },
    Rule {
        class: Class::NavManeuver,
        surfaces: &[Centre, Top],
    },
    Rule {
        class: Class::Call,
        surfaces: &[Centre, Top],
    },
    Rule {
        class: Class::UserPage,
        surfaces: &[Centre],
    },
    Rule {
        class: Class::Media,
        surfaces: &[Centre, Top],
    },
    Rule {
        class: Class::StockRadio,
        surfaces: &[Centre, Top],
    },
    Rule {
        class: Class::Idle,
        surfaces: &[Centre, Top],
    },
];

#[derive(Clone, Copy, Debug)]
pub struct ArbiterConfig {
    pub approach_m: f64,
    pub return_m: f64,
    /// Show nav this long after the maneuver changes ("return after T").
    pub peek: Duration,
    pub route_gap: Duration,
    /// A lower class can't take over sooner than this after a switch. Ours.
    pub min_dwell: Duration,
}

impl Default for ArbiterConfig {
    fn default() -> Self {
        ArbiterConfig {
            approach_m: 500.0,
            return_m: 1000.0,
            peek: Duration::from_secs(5),
            route_gap: Duration::from_secs(3),
            min_dwell: Duration::from_secs(2),
        }
    }
}

#[derive(Clone, Debug, Default)]
struct NavPolicy {
    available: bool,
    maneuver: Option<String>,
    peek_until: Option<Instant>,
    approaching: bool,
    armed: bool,
    gap_until: Option<Instant>,
}

pub struct Arbiter {
    pub cfg: ArbiterConfig,
    active: [bool; 7],
    nav: NavPolicy,
    current: [Option<(Class, Instant)>; 2],
}

fn idx(s: Surface) -> usize {
    match s {
        Centre => 0,
        Top => 1,
    }
}

impl Arbiter {
    pub fn new(cfg: ArbiterConfig) -> Self {
        let mut active = [false; 7];
        active[Class::Idle as usize] = true;
        Arbiter {
            cfg,
            active,
            nav: NavPolicy::default(),
            current: [None; 2],
        }
    }

    /// Turn a class on or off. `NavManeuver` comes from [`Arbiter::nav`] instead.
    pub fn set(&mut self, class: Class, on: bool) {
        if class != Class::NavManeuver && class != Class::Idle {
            self.active[class as usize] = on;
        }
    }

    /// Feed nav state as published (`nav.maneuver`, `nav.distance_m`).
    pub fn nav(&mut self, now: Instant, maneuver: Option<&str>, distance_m: Option<f64>) {
        let n = &mut self.nav;
        let Some(m) = maneuver else {
            if n.available && n.gap_until.is_none() {
                n.gap_until = Some(now + self.cfg.route_gap);
            }
            return;
        };
        n.gap_until = None;
        let changed = !n.available || n.maneuver.as_deref() != Some(m);
        n.available = true;
        if changed {
            n.maneuver = Some(m.to_string());
            n.peek_until = Some(now + self.cfg.peek);
            n.armed = true;
        }
        if let Some(d) = distance_m.filter(|d| d.is_finite() && *d >= 0.0) {
            if d > self.cfg.return_m {
                n.approaching = false;
                n.armed = true;
            } else if d <= self.cfg.approach_m && n.armed {
                n.approaching = true;
                n.armed = false;
            }
        }
    }

    /// Driver picked a page by hand: suppress the nav interruption until the
    /// maneuver changes or the distance retreats past `return_m`.
    pub fn manual_select(&mut self) {
        self.nav.peek_until = None;
        self.nav.approaching = false;
        self.nav.armed = false;
    }

    fn nav_active(&mut self, now: Instant) -> bool {
        let n = &mut self.nav;
        if n.gap_until.is_some_and(|t| now >= t) {
            *n = NavPolicy::default();
        }
        n.available && (n.approaching || n.peek_until.is_some_and(|t| now < t))
    }

    fn is_active(&mut self, now: Instant, c: Class) -> bool {
        match c {
            Class::NavManeuver => self.nav_active(now),
            _ => self.active[c as usize],
        }
    }

    /// Winning class for a surface.
    pub fn winner(&mut self, now: Instant, s: Surface) -> Class {
        let mut best = Class::Idle;
        for r in &RULES {
            if r.surfaces.contains(&s) && self.is_active(now, r.class) {
                best = r.class;
                break;
            }
        }
        let i = idx(s);
        match self.current[i] {
            Some((cur, since)) if cur == best => {
                let _ = since;
            }
            Some((cur, since)) if best > cur && now < since + self.cfg.min_dwell => {
                return cur;
            }
            _ => self.current[i] = Some((best, now)),
        }
        best
    }

    /// Next time a winner can change without new input.
    pub fn next_deadline(&self, now: Instant) -> Option<Instant> {
        let dwell = self
            .current
            .iter()
            .flatten()
            .map(|(_, since)| *since + self.cfg.min_dwell);
        [self.nav.peek_until, self.nav.gap_until]
            .into_iter()
            .flatten()
            .chain(dwell)
            .filter(|t| *t > now)
            .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn nav_peeks_then_returns_to_user_page() {
        let t0 = Instant::now();
        let mut a = Arbiter::new(ArbiterConfig::default());
        a.set(Class::UserPage, true);
        assert_eq!(a.winner(t0, Centre), Class::UserPage);
        a.nav(t0, Some("turn"), Some(3000.0));
        assert_eq!(a.winner(t0, Centre), Class::NavManeuver);
        assert_eq!(a.winner(t0 + s(4), Centre), Class::NavManeuver);
        assert_eq!(a.winner(t0 + s(6), Centre), Class::UserPage);
        // Approach.
        a.nav(t0 + s(30), Some("turn"), Some(450.0));
        assert_eq!(a.winner(t0 + s(30), Centre), Class::NavManeuver);
        // Turn done, next maneuver far away: peek, then back.
        a.nav(t0 + s(40), Some("straight"), Some(5000.0));
        assert_eq!(a.winner(t0 + s(44), Centre), Class::NavManeuver);
        assert_eq!(a.winner(t0 + s(46), Centre), Class::UserPage);
    }

    #[test]
    fn route_gap_held_and_alert_preempts() {
        let t0 = Instant::now();
        let mut a = Arbiter::new(ArbiterConfig::default());
        a.nav(t0, Some("turn"), Some(100.0));
        a.nav(t0 + s(1), None, None);
        assert_eq!(a.winner(t0 + s(3), Centre), Class::NavManeuver);
        a.set(Class::Alert, true);
        assert_eq!(a.winner(t0 + s(3), Centre), Class::Alert);
        assert_eq!(a.winner(t0 + s(3), Top), Class::NavManeuver);
        a.set(Class::Alert, false);
        // Gap over 3 s: nav gone; Alert held for the dwell first.
        assert_eq!(a.winner(t0 + s(4), Centre), Class::Alert);
        assert_eq!(a.winner(t0 + s(6), Centre), Class::Idle);
    }

    #[test]
    fn manual_select_suppresses_approach() {
        let t0 = Instant::now();
        let mut a = Arbiter::new(ArbiterConfig::default());
        a.set(Class::UserPage, true);
        a.nav(t0, Some("turn"), Some(300.0));
        a.manual_select();
        assert_eq!(a.winner(t0 + s(10), Centre), Class::UserPage);
        a.nav(t0 + s(11), Some("turn"), Some(250.0));
        assert_eq!(a.winner(t0 + s(11), Centre), Class::UserPage);
    }
}
