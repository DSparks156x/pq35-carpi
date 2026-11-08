//! Nav screen, in Drew's order: fast icon, text, bar, commit; then the refine
//! lane (native strokes, white only), cancelled when the maneuver changes.
//!
//! Layout (logical, centre-relative), the old driver's, which Drew saw work
//! (`dc/apps/nav.py`, `dc/nav_icons.py`):
//! - icon rect (0,1) 40x36 = 80x72 px, the 72x72 mask at px x=6; opaque `55` base;
//! - distance value y=8, unit y=17, each centred in x 40..56;
//! - approach bar: stock graphics cells at x=57, y=3/10/17/24/31
//!   (`dc/stock_mono_catalog.json` "bar", bench);
//! - road y=39 in x 1..63: centred if it fits, else marquee.

use super::Screen;
use crate::font::Font;
use crate::font_data::STOCK_BAR;
use crate::cmd::Op;
use crate::helpers::{CentredField, Ctx, GlyphCells, QuickThenRefineIcon, ScrollText};
use crate::lanes::{self, Lane};
use crate::icons::{self, Maneuver};
use crate::pack::{Group, PackError, Packer};
use crate::proto::Rect;
use std::time::Instant;

pub const ICON: Rect = Rect::new(0, 1, 40, 36);
/// Mask offset inside [`ICON`], physical px.
pub const ICON_MASK_X: usize = 6;
pub const BAR_X: u8 = 57;
pub const BAR_Y: [u8; 5] = [3, 10, 17, 24, 31];

/// Inputs as carpi-core publishes them (`nav.maneuver`, `nav.side`,
/// `nav.roundabout_angle`, `nav.distance_m`, `nav.road`, `nav.lanes`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NavInput {
    pub maneuver: Option<String>,
    pub side: Option<String>,
    pub roundabout_angle: Option<i32>,
    pub distance_m: Option<f64>,
    pub road: Option<String>,
    /// Left to right; empty when the phone sends none.
    pub lanes: Vec<Lane>,
}

/// Where lane guidance goes (on trial; see `examples/mock`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LaneLayout {
    #[default]
    Off,
    /// Near the turn the road line becomes a full-width lane strip.
    SwapRoad,
    /// A small strip in the right column under the unit (~3 lanes).
    Pocket,
    /// Near the turn the icon shrinks and lanes go under it.
    ShrinkIcon,
}

/// SwapRoad strip: the road row, full width.
pub const LANES_ROAD: Rect = Rect::new(0, 38, 64, 9);
/// Pocket: under the unit, left of the bar.
pub const LANES_POCKET: Rect = Rect::new(40, 25, 17, 13);

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
    /// Roundabouts and U-turns go clockwise.
    pub left_hand_traffic: bool,
    pub lanes: LaneLayout,
    /// Lanes show only closer than this to the turn.
    pub lane_range_m: f64,
}

impl Default for NavConfig {
    fn default() -> Self {
        NavConfig {
            bar_range_m: 500.0,
            lanes: LaneLayout::Off,
            lane_range_m: 400.0,
            left_hand_traffic: false,
        }
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

/// The icon as it fills [`ICON`].
pub fn icon_art(m: Maneuver, left_hand_traffic: bool) -> icons::Art {
    let (w, h) = (usize::from(ICON.w) * 2, usize::from(ICON.h) * 2);
    icons::draw(m, left_hand_traffic, w, h, ICON_MASK_X, 0)
}

pub struct NavScreen {
    pub cfg: NavConfig,
    input: NavInput,
    /// Keyed by maneuver plus the lanes drawn into it (ShrinkIcon only).
    icon: QuickThenRefineIcon<(Maneuver, Vec<Lane>)>,
    /// SwapRoad / Pocket strip.
    lane_strip: QuickThenRefineIcon<Vec<Lane>>,
    /// The strip (SwapRoad / Pocket) is on screen.
    strip_shown: bool,
    dist: CentredField,
    unit: CentredField,
    road: ScrollText,
    bar: GlyphCells,
}

impl NavScreen {
    pub fn new(cfg: NavConfig) -> Self {
        NavScreen {
            cfg,
            input: NavInput::default(),
            icon: QuickThenRefineIcon::new(ICON),
            lane_strip: QuickThenRefineIcon::new(match cfg.lanes {
                LaneLayout::Pocket => LANES_POCKET,
                _ => LANES_ROAD,
            }),
            strip_shown: false,
            dist: CentredField::new(8, 40, 56, Font::Prop),
            unit: CentredField::new(17, 40, 56, Font::Prop),
            road: {
                let mut r = ScrollText::new(39, 1, 63, Font::Prop);
                r.centred = true;
                r
            },
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
        self.icon.refining() || self.lane_strip.refining()
    }

    /// Lanes to show now: the layout wants them, the phone sent some, and
    /// the turn is close.
    fn lanes_now(&self) -> Option<&[Lane]> {
        let near = self
            .input
            .distance_m
            .is_some_and(|d| d < self.cfg.lane_range_m);
        (self.cfg.lanes != LaneLayout::Off && near && !self.input.lanes.is_empty())
            .then_some(&self.input.lanes[..])
    }

    fn icon_groups(&mut self, cx: &Ctx) -> Vec<Group> {
        let m = self.input.maneuver();
        let lht = self.cfg.left_hand_traffic;
        let (w, h) = (usize::from(ICON.w) * 2, usize::from(ICON.h) * 2);
        let shrink = (self.cfg.lanes == LaneLayout::ShrinkIcon)
            .then(|| self.lanes_now().map(<[Lane]>::to_vec))
            .flatten();
        let (key, art) = match shrink {
            Some(l) => {
                // 52 px icon top-centre, an 18 px lane strip under it.
                let mut art = icons::draw_scaled(m, lht, w, h, (w - 52) / 2, 0, 52);
                let strip = lanes::strip(&l, w, 18, 12);
                for y in 0..18 {
                    for x in 0..w {
                        if strip.get(x, y) {
                            art.px[(54 + y) * w + x] = true;
                        }
                    }
                }
                ((m, l), art)
            }
            None => ((m, Vec::new()), icon_art(m, lht)),
        };
        self.icon.set(cx, key, &art)
    }

    /// SwapRoad / Pocket: draw, change or take away the strip. Taking it
    /// away clears its rect in the same group as whatever comes back.
    fn strip_groups(&mut self, now: Instant, cx: &Ctx) -> Vec<Group> {
        if !matches!(self.cfg.lanes, LaneLayout::SwapRoad | LaneLayout::Pocket) {
            return Vec::new();
        }
        let rect = self.lane_strip.rect;
        match self.lanes_now().map(<[Lane]>::to_vec) {
            Some(l) => {
                self.strip_shown = true;
                let (w, h) = (usize::from(rect.w) * 2, usize::from(rect.h) * 2);
                let pitch = if self.cfg.lanes == LaneLayout::Pocket { 11 } else { 12 };
                let art = lanes::strip(&l, w, h, pitch);
                self.lane_strip.set(cx, l, &art)
            }
            None if self.strip_shown => {
                self.strip_shown = false;
                self.lane_strip.reset();
                let mut ops = vec![Op::Clear(rect)];
                if self.cfg.lanes == LaneLayout::SwapRoad {
                    self.road.reset();
                    if let Some(g) = self.road.poll(now, cx, "road") {
                        ops.extend(g.ops);
                    }
                }
                vec![Group::new("lanes-off", ops)]
            }
            None => Vec::new(),
        }
    }

    fn road_groups(&mut self, now: Instant, cx: &Ctx) -> Vec<Group> {
        // The strip owns the road row while it's up.
        if self.cfg.lanes == LaneLayout::SwapRoad && self.lanes_now().is_some() {
            return Vec::new();
        }
        self.road.poll(now, cx, "road").into_iter().collect()
    }

    fn distance_groups(&mut self, cx: &Ctx) -> Vec<Group> {
        let (v, u) = format_distance(self.input.distance_m);
        let mut out: Vec<Group> = self.dist.set(cx, "dist", &v).into_iter().collect();
        out.extend(self.unit.set(cx, "unit", u));
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
        self.lane_strip.reset();
        self.strip_shown = false;
        self.dist.reset();
        self.unit.reset();
        self.road.reset();
        self.bar.reset();
        let mut g = self.icon_groups(cx);
        g.extend(self.distance_groups(cx));
        g.extend(self.road_groups(now, cx));
        g.extend(self.bar_groups());
        g.extend(self.strip_groups(now, cx));
        g
    }

    fn update(&mut self, now: Instant, cx: &Ctx) -> Vec<Group> {
        let mut g = self.distance_groups(cx);
        g.extend(self.bar_groups());
        g.extend(self.strip_groups(now, cx));
        g.extend(self.road_groups(now, cx));
        g.extend(self.icon_groups(cx));
        g
    }

    fn refine(&mut self, _cx: &Ctx, p: &mut Packer, max_msgs: usize) -> Result<(), PackError> {
        self.icon.refine(p, max_msgs)?;
        if !self.icon.refining() {
            self.lane_strip.refine(p, max_msgs)?;
        }
        Ok(())
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
