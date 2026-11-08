//! Nav screen, in Drew's order: fast icon, text, bar, commit; then the refine
//! lane (native strokes, white only), cancelled when the maneuver changes.
//!
//! Layout (logical, centre-relative), the old driver's, which Drew saw work
//! (`dc/apps/nav.py`, `dc/nav_icons.py`):
//! - icon rect (0,1) 40x36 = 80x72 px, the 72x72 mask at px x=6; opaque `55` base;
//! - distance value y=8, unit y=17, each centred in x 40..56;
//! - approach bar: stock graphics cells at x=57, y=3/10/17/24/31
//!   (`dc/stock_mono_catalog.json` "bar", bench);
//! - pocket y=27: trip info (ETA, time left, distance left), cycling,
//!   right-aligned against the bar and as wide as the icon's empty corner
//!   allows;
//! - bottom row y=39: the road, centred or marquee; near a turn with lanes,
//!   the lane strip instead (`Swap`) or taking turns with the road (`Flip`).

use super::Screen;
use crate::cmd::Op;
use crate::font::{Font, TEXT_H, audscii};
use crate::font_data::STOCK_BAR;
use crate::helpers::{CentredField, Ctx, GlyphCells, QuickThenRefineIcon, ScrollText, TextSlot};
use crate::icons::{self, Art, Maneuver};
use crate::lanes::{self, Lane};
use crate::pack::{Group, PackError, Packer};
use crate::proto::Rect;
use std::time::{Duration, Instant};

pub const ICON: Rect = Rect::new(0, 1, 40, 36);
/// Mask offset inside [`ICON`], physical px.
pub const ICON_MASK_X: usize = 6;
pub const BAR_X: u8 = 57;
pub const BAR_Y: [u8; 5] = [3, 10, 17, 24, 31];
/// Lane strip: the road row, full width.
pub const LANES: Rect = Rect::new(0, 38, 64, 9);
/// Lane cell pitch in the strip, px (12-px arrows + 2).
pub const LANE_PITCH: usize = 14;
/// Pocket text row, and its right edge (one unit clear of the bar at x=57).
pub const POCKET_Y: u8 = 27;
pub const POCKET_RIGHT: u8 = 56;
/// Pocket page time.
const POCKET_PAGE: Duration = Duration::from_secs(3);

/// Inputs as carpi-core publishes them (`nav.*`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NavInput {
    pub maneuver: Option<String>,
    pub side: Option<String>,
    pub roundabout_angle: Option<i32>,
    pub distance_m: Option<f64>,
    pub road: Option<String>,
    /// Left to right; empty when the phone sends none.
    pub lanes: Vec<Lane>,
    /// Arrival time, already formatted local `HH:MM` (AA sends it so; the
    /// caller formats CarPlay's Unix time).
    pub eta: Option<String>,
    pub time_to_destination_s: Option<f64>,
    pub destination_distance_m: Option<f64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LaneLayout {
    Off,
    /// Near the turn the road row shows the lanes instead.
    Swap,
    /// Near the turn the road row alternates: lanes, then the road.
    #[default]
    Flip,
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
    /// Roundabouts and U-turns go clockwise.
    pub left_hand_traffic: bool,
    pub lanes: LaneLayout,
    /// Lanes show only closer than this to the turn.
    pub lane_range_m: f64,
    /// `Flip`: lanes this long, then the road this long.
    pub flip_lanes: Duration,
    pub flip_road: Duration,
    /// Show the trip-info pocket.
    pub pocket: bool,
}

impl Default for NavConfig {
    fn default() -> Self {
        NavConfig {
            bar_range_m: 500.0,
            left_hand_traffic: false,
            lanes: LaneLayout::default(),
            lane_range_m: 400.0,
            flip_lanes: Duration::from_secs(4),
            flip_road: Duration::from_secs(2),
            pocket: true,
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

/// Pocket pages, each as its forms from longest to shortest; the widest one
/// that fits is shown, a page with none that fits is skipped.
pub fn pocket_pages(i: &NavInput) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    if let Some(e) = i.eta.as_ref().filter(|e| !e.is_empty()) {
        out.push(vec![format!("ETA {e}"), e.clone()]);
    }
    if let Some(s) = i.time_to_destination_s.filter(|s| s.is_finite() && *s >= 0.0) {
        let min = (s / 60.0).round() as u64;
        out.push(if min < 60 {
            vec![format!("{min} min"), format!("{min}min"), format!("{min}m")]
        } else {
            vec![
                format!("{}h {:02}", min / 60, min % 60),
                format!("{}h{:02}", min / 60, min % 60),
                format!("{}h", min / 60),
            ]
        });
    }
    if let Some(d) = i.destination_distance_m.filter(|d| d.is_finite() && *d >= 0.0) {
        let (v, u) = format_distance(Some(d));
        out.push(vec![format!("{v} {u}"), format!("{v}{u}"), v]);
    }
    out
}

/// The icon as it fills [`ICON`].
pub fn icon_art(m: Maneuver, left_hand_traffic: bool) -> Art {
    let (w, h) = (usize::from(ICON.w) * 2, usize::from(ICON.h) * 2);
    icons::draw(m, left_hand_traffic, w, h, ICON_MASK_X, 0)
}

/// Leftmost logical x the pocket text may start at: one unit right of the
/// icon's ink in the pocket's rows (plus a unit above and below), else the
/// distance column's left edge.
fn pocket_left(art: &Art) -> u8 {
    let (y0, y1) = (
        usize::from(POCKET_Y - ICON.y - 1) * 2,
        (usize::from(POCKET_Y - ICON.y + TEXT_H) * 2).min(art.h),
    );
    let ink = (y0..y1)
        .flat_map(|y| (0..art.w).filter(move |x| art.get(*x, y)))
        .max();
    match ink {
        Some(x) => ((x / 2) as u8 + 2).max(4),
        None => 4,
    }
}

pub struct NavScreen {
    pub cfg: NavConfig,
    input: NavInput,
    icon: QuickThenRefineIcon<Maneuver>,
    lane_strip: QuickThenRefineIcon<Vec<Lane>>,
    /// The strip is on screen (the road isn't).
    strip_shown: bool,
    /// When lanes became available near this turn (the flip clock).
    lanes_since: Option<Instant>,
    dist: CentredField,
    unit: CentredField,
    road: ScrollText,
    bar: GlyphCells,
    pocket: TextSlot,
    /// Pocket page clock.
    pocket_since: Option<Instant>,
    /// Earliest time something here wants an update.
    wake: Option<Instant>,
}

impl NavScreen {
    pub fn new(cfg: NavConfig) -> Self {
        NavScreen {
            cfg,
            input: NavInput::default(),
            icon: QuickThenRefineIcon::new(ICON),
            lane_strip: QuickThenRefineIcon::new(LANES),
            strip_shown: false,
            lanes_since: None,
            dist: CentredField::new(8, 40, 56, Font::Prop),
            unit: CentredField::new(17, 40, 56, Font::Prop),
            road: {
                let mut r = ScrollText::new(39, 1, 63, Font::Prop);
                r.centred = true;
                r
            },
            bar: GlyphCells::new(BAR_Y.iter().map(|y| (BAR_X, *y)).collect()),
            pocket: TextSlot::new(POCKET_Y, 4, POCKET_RIGHT, Font::Prop),
            pocket_since: None,
            wake: None,
        }
    }

    pub fn input(&self) -> &NavInput {
        &self.input
    }

    pub fn set(&mut self, now: Instant, input: NavInput) {
        self.road
            .set_text(now, input.road.as_deref().unwrap_or_default());
        if input.maneuver != self.input.maneuver {
            self.lanes_since = None;
        }
        self.input = input;
    }

    /// Icon version; bumps when the maneuver changes (refine jobs carry it).
    pub fn icon_version(&self) -> u32 {
        self.icon.version()
    }

    pub fn refining(&self) -> bool {
        self.icon.refining() || self.lane_strip.refining()
    }

    fn want_wake(&mut self, t: Instant) {
        self.wake = Some(self.wake.map_or(t, |w| w.min(t)));
    }

    /// Lanes exist and the turn is close.
    fn lanes_near(&self) -> bool {
        self.cfg.lanes != LaneLayout::Off
            && !self.input.lanes.is_empty()
            && self
                .input
                .distance_m
                .is_some_and(|d| d < self.cfg.lane_range_m)
    }

    /// Whether the bottom row shows lanes now; schedules the next flip.
    fn strip_now(&mut self, now: Instant) -> bool {
        if !self.lanes_near() {
            self.lanes_since = None;
            return false;
        }
        let since = *self.lanes_since.get_or_insert(now);
        match self.cfg.lanes {
            LaneLayout::Flip => {
                let (a, b) = (self.cfg.flip_lanes, self.cfg.flip_road);
                let cycle = (a + b).as_nanos().max(1);
                let into = (now - since).as_nanos() % cycle;
                let lanes = into < a.as_nanos();
                let left = if lanes { a.as_nanos() - into } else { cycle - into };
                self.want_wake(now + Duration::from_nanos(left as u64));
                lanes
            }
            _ => true,
        }
    }

    fn icon_groups(&mut self, cx: &Ctx) -> Vec<Group> {
        let m = self.input.maneuver();
        let art = icon_art(m, self.cfg.left_hand_traffic);
        self.icon.set(cx, m, &art)
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

    /// Bottom row: the lane strip or the road. Each switch is one group, so
    /// the row never shows blank between them.
    fn bottom_groups(&mut self, now: Instant, cx: &Ctx) -> Vec<Group> {
        if self.strip_now(now) {
            self.strip_shown = true;
            let l = self.input.lanes.clone();
            let (w, h) = (usize::from(LANES.w) * 2, usize::from(LANES.h) * 2);
            let art = lanes::strip(&l, w, h, LANE_PITCH);
            // The strip's opaque base covers the whole road row.
            return self.lane_strip.set(cx, l, &art);
        }
        if self.strip_shown {
            self.strip_shown = false;
            self.lane_strip.reset();
            self.road.reset();
            let mut ops = vec![Op::Clear(LANES)];
            if let Some(g) = self.road.poll(now, cx, "road") {
                ops.extend(g.ops);
            }
            return vec![Group::new("road-back", ops)];
        }
        self.road.poll(now, cx, "road").into_iter().collect()
    }

    /// Pocket: the current page's widest form that fits beside the icon.
    fn pocket_groups(&mut self, now: Instant, cx: &Ctx) -> Vec<Group> {
        let m = &cx.metrics;
        let pages = if self.cfg.pocket { pocket_pages(&self.input) } else { Vec::new() };
        let left = pocket_left(&icon_art(self.input.maneuver(), self.cfg.left_hand_traffic));
        let room = u16::from(POCKET_RIGHT - left) * 2;
        let fitting: Vec<Vec<u8>> = pages
            .iter()
            .filter_map(|forms| {
                forms
                    .iter()
                    .map(|f| audscii(f))
                    .find(|b| m.width(Font::Prop, b) <= room)
            })
            .collect();
        let text = if fitting.is_empty() {
            self.pocket_since = None;
            Vec::new()
        } else {
            let since = *self.pocket_since.get_or_insert(now);
            let k = ((now - since).as_nanos() / POCKET_PAGE.as_nanos()) as usize;
            let next = since + POCKET_PAGE * (k as u32 + 1);
            if fitting.len() > 1 {
                self.want_wake(next);
            }
            fitting[k % fitting.len()].clone()
        };
        let x = POCKET_RIGHT - (m.width(Font::Prop, &text).div_ceil(2) as u8).min(POCKET_RIGHT);
        self.pocket.draw(m, "pocket", x, text).into_iter().collect()
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
        self.pocket.reset();
        self.wake = None;
        let mut g = self.icon_groups(cx);
        g.extend(self.distance_groups(cx));
        g.extend(self.bottom_groups(now, cx));
        g.extend(self.bar_groups());
        g.extend(self.pocket_groups(now, cx));
        g
    }

    fn update(&mut self, now: Instant, cx: &Ctx) -> Vec<Group> {
        self.wake = None;
        let mut g = self.distance_groups(cx);
        g.extend(self.bar_groups());
        g.extend(self.bottom_groups(now, cx));
        g.extend(self.icon_groups(cx));
        g.extend(self.pocket_groups(now, cx));
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
        let road = if self.strip_shown {
            None
        } else {
            self.road.next_deadline(&cx.metrics)
        };
        [road, self.wake].into_iter().flatten().min()
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

    #[test]
    fn pocket_forms() {
        let i = NavInput {
            eta: Some("14:32".into()),
            time_to_destination_s: Some(3900.0),
            destination_distance_m: Some(12_400.0),
            ..Default::default()
        };
        let p = pocket_pages(&i);
        assert_eq!(p[0], vec!["ETA 14:32", "14:32"]);
        assert_eq!(p[1], vec!["1h 05", "1h05", "1h"]);
        assert_eq!(p[2], vec!["12 km", "12km", "12"]);
    }
}
