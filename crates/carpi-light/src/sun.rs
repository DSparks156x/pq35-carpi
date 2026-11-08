//! Sunrise and sunset, NOAA's spreadsheet method (the "General Solar
//! Position Calculations" equations). Good to a minute or two away from
//! the poles, which is far finer than a rough location needs.

use std::f64::consts::PI;

/// Where the car roughly is. East and north are positive.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Location {
    pub latitude: f64,
    pub longitude: f64,
    /// What the car clock runs on. `None`: one hour per 15° of longitude.
    pub utc_offset_hours: Option<f64>,
}

impl Location {
    pub fn utc_offset(&self) -> f64 {
        self.utc_offset_hours
            .unwrap_or_else(|| (self.longitude / 15.0).round())
    }
}

/// One local day's sun.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SunDay {
    /// Minutes after local midnight. Either can fall outside 0-1440 for
    /// odd offsets; comparisons still work.
    Normal { rise: f64, set: f64 },
    /// The sun never rises.
    PolarNight,
    /// The sun never sets.
    MidnightSun,
}

impl SunDay {
    /// Dark at `minute` after local midnight.
    pub fn dark_at(&self, minute: f64) -> bool {
        match *self {
            SunDay::Normal { rise, set } => minute < rise || minute >= set,
            SunDay::PolarNight => true,
            SunDay::MidnightSun => false,
        }
    }
}

fn rad(d: f64) -> f64 {
    d * PI / 180.0
}

fn deg(r: f64) -> f64 {
    r * 180.0 / PI
}

/// Julian day at 0h UT of a Gregorian date.
fn julian_day(year: i32, month: u32, day: u32) -> f64 {
    let (y, m) = if month <= 2 {
        (year - 1, month + 12)
    } else {
        (year, month)
    };
    let a = (y as f64 / 100.0).floor();
    let b = 2.0 - a + (a / 4.0).floor();
    (365.25 * (y as f64 + 4716.0)).floor() + (30.6001 * (m as f64 + 1.0)).floor() + day as f64 + b
        - 1524.5
}

/// Sunrise and sunset (apparent, -0.833° for refraction and the disc) on
/// a local date.
pub fn sun_day(year: i32, month: u32, day: u32, loc: &Location) -> SunDay {
    let tz = loc.utc_offset();
    // Evaluate the sun at local noon.
    let jd = julian_day(year, month, day) + (12.0 - tz) / 24.0;
    let t = (jd - 2_451_545.0) / 36_525.0;

    let l0 = (280.46646 + t * (36_000.769_83 + t * 0.000_303_2)).rem_euclid(360.0);
    let m = 357.52911 + t * (35_999.050_29 - 0.000_153_7 * t);
    let e = 0.016_708_634 - t * (0.000_042_037 + 0.000_000_126_7 * t);
    let c = rad(m).sin() * (1.914602 - t * (0.004817 + 0.000014 * t))
        + rad(2.0 * m).sin() * (0.019993 - 0.000101 * t)
        + rad(3.0 * m).sin() * 0.000289;
    let omega = 125.04 - 1934.136 * t;
    let lambda = l0 + c - 0.00569 - 0.00478 * rad(omega).sin();
    let eps0 = 23.0 + (26.0 + (21.448 - t * (46.815 + t * (0.00059 - t * 0.001813))) / 60.0) / 60.0;
    let eps = eps0 + 0.00256 * rad(omega).cos();
    let decl = (rad(eps).sin() * rad(lambda).sin()).asin();

    let y = rad(eps / 2.0).tan().powi(2);
    let (l0r, mr) = (rad(l0), rad(m));
    let eot = 4.0
        * deg(y * (2.0 * l0r).sin() - 2.0 * e * mr.sin()
            + 4.0 * e * y * mr.sin() * (2.0 * l0r).cos()
            - 0.5 * y * y * (4.0 * l0r).sin()
            - 1.25 * e * e * (2.0 * mr).sin());

    let lat = rad(loc.latitude);
    let cos_ha = rad(90.833).cos() / (lat.cos() * decl.cos()) - lat.tan() * decl.tan();
    if cos_ha > 1.0 {
        return SunDay::PolarNight;
    }
    if cos_ha < -1.0 {
        return SunDay::MidnightSun;
    }
    let ha = deg(cos_ha.acos());
    let noon = 720.0 - 4.0 * loc.longitude - eot + tz * 60.0;
    SunDay::Normal {
        rise: noon - 4.0 * ha,
        set: noon + 4.0 * ha,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hm(min: f64) -> String {
        let m = min.round() as i64;
        format!("{:02}:{:02}", m / 60, m % 60)
    }

    fn times(d: SunDay) -> (f64, f64) {
        match d {
            SunDay::Normal { rise, set } => (rise, set),
            other => panic!("{other:?}"),
        }
    }

    fn near(got: f64, h: u32, m: u32) {
        let want = (h * 60 + m) as f64;
        assert!(
            (got - want).abs() <= 3.0,
            "got {}, want {h:02}:{m:02}",
            hm(got)
        );
    }

    #[test]
    fn london_in_november() {
        // Published tables: London 2026-11-07, sunrise 07:05, sunset 16:21 (GMT).
        let loc = Location {
            latitude: 51.5072,
            longitude: -0.1276,
            utc_offset_hours: Some(0.0),
        };
        let (rise, set) = times(sun_day(2026, 11, 7, &loc));
        near(rise, 7, 5);
        near(set, 16, 21);
    }

    #[test]
    fn phoenix_at_the_solstice() {
        // Arizona keeps MST all year. 2026-06-21: sunrise 05:19, sunset 19:42.
        let loc = Location {
            latitude: 33.4484,
            longitude: -112.0740,
            utc_offset_hours: Some(-7.0),
        };
        let (rise, set) = times(sun_day(2026, 6, 21, &loc));
        near(rise, 5, 19);
        near(set, 19, 42);
    }

    #[test]
    fn poles_and_default_offset() {
        let north = |lat| Location {
            latitude: lat,
            longitude: 15.0,
            utc_offset_hours: None,
        };
        assert_eq!(sun_day(2026, 12, 21, &north(78.0)), SunDay::PolarNight);
        assert_eq!(sun_day(2026, 6, 21, &north(78.0)), SunDay::MidnightSun);
        assert!(SunDay::PolarNight.dark_at(720.0));
        assert!(!SunDay::MidnightSun.dark_at(0.0));
        assert_eq!(north(50.0).utc_offset(), 1.0);
    }
}
