//! Nav screen, in Drew's order: fast icon, text, bar, commit; then the refine
//! lane (native strokes, white only), cancelled when the maneuver changes.
//!
//! Layout (logical, centre-relative):
//! - icon (0,1) 32x38, opaque `55` base;
//! - distance value at y=10, x 33..56, right-aligned; unit at y=20;
//! - approach bar: stock graphics cells at x=57, y=3/10/17/24/31
//!   (`dc/stock_mono_catalog.json` "bar", bench);
//! - road at y=39, full width, marquee.

use super::Screen;
use crate::font::{Font, audscii};
use crate::font_data::STOCK_BAR;
use crate::helpers::{Ctx, GlyphCells, NumberField, QuickThenRefineIcon, ScrollText, TextSlot};
use crate::icons::{self, Maneuver};
use crate::pack::{Group, PackError, Packer};
use crate::proto::Rect;
use std::time::Instant;

pub const ICON: Rect = Rect::new(0, 1, 32, 38);
pub const BAR_X: u8 = 57;
pub const BAR_Y: [u8; 5] = [3, 10, 17, 24, 31];

/// Inputs as carpi-core publishes them (`nav.maneuver`, `nav.side`,
/// `nav.roundabout_angle`, `nav.distance_m`, `nav.road`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NavInput {
    pub maneuver: Option<String>,
    pub side: Option<String>,
    pub roundabout_angle: Option<i32>,
    pub distance_m: Option<f64>,
    pub road: Option<String>,
}

impl NavInput {
    pub fn maneuver(&self) -> Maneuver {
        match &self.maneuver {
            Some(m) => Maneuver::from_aa(m, self.side.as_deref(), self.roundabout_angle),
            None => Maneuver::Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct NavConfig {
    /// Bar spans this distance; full far out, empty at the turn. Guess: the stock
    /// bar's progress source isn't decoded (raw 0..255 → 21 levels,
    /// `dc/stock_mono_frame.py:78`).
    pub bar_range_m: f64,
}

impl Default for NavConfig {
    fn default() -> Self {
        NavConfig { bar_range_m: 500.0 }
    }
}

/// `("350", "m")`, `("1.2", "km")`, `("12", "km")`, `("--", "")`.
pub fn format_distance(d: Option<f64>) -> (String, &'static str) {
    match d.filter(|d| d.is_finite() && *d >= 0.0) {
        None => ("--".into(), ""),
        Some(d) if d < 995.0 => (format!("{}", ((d / 10.0).round() * 10.0) as u32), "m"),
        Some(d) if d < 9950.0 => (format!("{:.1}", d / 1000.0), "km"),
        Some(d) => (format!("{}", (d / 1000.0).round() as u32), "km"),
    }
}

/// Stock bar level 0..=20 (index into [`STOCK_BAR`]).
pub fn bar_level(d: Option<f64>, range: f64) -> usize {
    match d.filter(|d| d.is_finite() && *d >= 0.0) {
        Some(d) if d < range => (((1.0 - d / range) * 20.0).round() as usize).min(20),
        _ => 0,
    }
}

pub struct NavScreen {
    pub cfg: NavConfig,
    input: NavInput,
    icon: QuickThenRefineIcon<Maneuver>,
    dist: NumberField,
    unit: TextSlot,
    road: ScrollText,
    bar: GlyphCells,
}

impl NavScreen {
    pub fn new(cfg: NavConfig) -> Self {
        NavScreen {
            cfg,
            input: NavInput::default(),
            icon: QuickThenRefineIcon::new(ICON),
            dist: NumberField::new(10, 33, 56, Font::Prop),
            unit: TextSlot::new(20, 33, 56, Font::Prop),
            road: ScrollText::new(39, 0, 64, Font::Prop),
            bar: GlyphCells::new(BAR_Y.iter().map(|y| (BAR_X, *y)).collect()),
        }
    }

    pub fn input(&self) -> &NavInput {
        &self.input
    }

    pub fn set(&mut self, now: Instant, input: NavInput) {
        self.road
            .set_text(now, input.road.as_deref().unwrap_or_default());
        self.input = input;
    }

    /// Icon version; bumps when the maneuver changes (refine jobs carry it).
    pub fn icon_version(&self) -> u32 {
        self.icon.version()
    }

    pub fn refining(&self) -> bool {
        self.icon.refining()
    }

    fn icon_groups(&mut self, cx: &Ctx) -> Vec<Group> {
        let m = self.input.maneuver();
        let art = icons::draw(m, usize::from(ICON.w) * 2, usize::from(ICON.h) * 2);
        self.icon.set(cx, m, &art)
    }

    fn distance_groups(&mut self, cx: &Ctx) -> Vec<Group> {
        let (v, u) = format_distance(self.input.distance_m);
        let mut out: Vec<Group> = self.dist.set(cx, "dist", &v).into_iter().collect();
        let x = self.unit.left;
        out.extend(self.unit.draw(&cx.metrics, "unit", x, audscii(u)));
        out
    }

    fn bar_groups(&mut self) -> Vec<Group> {
        let lvl = bar_level(self.input.distance_m, self.cfg.bar_range_m);
        self.bar.set("bar", &STOCK_BAR[lvl])
    }
}

impl Screen for NavScreen {
    fn enter(&mut self, now: Instant, cx: &Ctx) -> Vec<Group> {
        self.icon.reset();
        self.dist.reset();
        self.unit.reset();
        self.road.reset();
        self.bar.reset();
        let mut g = self.icon_groups(cx);
        g.extend(self.distance_groups(cx));
        g.extend(self.road.poll(now, cx, "road"));
        g.extend(self.bar_groups());
        g
    }

    fn update(&mut self, now: Instant, cx: &Ctx) -> Vec<Group> {
        let mut g = self.distance_groups(cx);
        g.extend(self.bar_groups());
        g.extend(self.road.poll(now, cx, "road"));
        g.extend(self.icon_groups(cx));
        g
    }

    fn refine(&mut self, _cx: &Ctx, p: &mut Packer, max_msgs: usize) -> Result<(), PackError> {
        self.icon.refine(p, max_msgs).map(|_| ())
    }

    fn next_deadline(&self, cx: &Ctx) -> Option<Instant> {
        self.road.next_deadline(&cx.metrics)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_text() {
        assert_eq!(format_distance(Some(347.0)), ("350".into(), "m"));
        assert_eq!(format_distance(Some(1240.0)), ("1.2".into(), "km"));
        assert_eq!(format_distance(Some(12400.0)), ("12".into(), "km"));
        assert_eq!(format_distance(None), ("--".into(), ""));
        assert_eq!(bar_level(Some(0.0), 500.0), 20);
        assert_eq!(bar_level(Some(250.0), 500.0), 10);
        assert_eq!(bar_level(Some(900.0), 500.0), 0);
    }
}
