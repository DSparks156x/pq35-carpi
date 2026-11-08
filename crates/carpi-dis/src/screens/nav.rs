//! Nav screen, in Drew's order: fast icon, text, bar, commit; then the refine
//! lane (native strokes, white only), cancelled when the maneuver changes.
//!
//! Layout (logical, centre-relative), the old driver's, which Drew saw work
//! (`dc/apps/nav.py`, `dc/nav_icons.py`):
//! - icon rect (0,1) 40x36 = 80x72 px, the 72x72 mask at px x=6; opaque `55` base;
//! - distance value y=8, unit y=18, each centred in x 40..56;
//! - approach bar: stock graphics cells at x=57, y=3/10/17/24/31
//!   (`dc/stock_mono_catalog.json` "bar", bench);
//! - pocket y=28: trip info (ETA, time left, distance left), cycling,
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
use crate::proto::{FULL, Rect};
use std::time::{Duration, Instant};

pub const ICON: Rect = Rect::new(0, 1, 40, 36);
/// Mask offset inside [`ICON`], physical px.
pub const ICON_MASK_X: usize = 6;
pub const BAR_X: u8 = 57;
pub const BAR_Y: [u8; 5] = [3, 10, 17, 24, 31];
/// Lane strip: the road row, full width.
pub const LANES: Rect = Rect::new(0, 38, 64, 9);
/// Lane cell pitch in the strip, px (18-px arrows + 3): 6 lanes across.
pub const LANE_PITCH: usize = 21;
/// Pocket text row, and its right edge (one unit clear of the bar at x=57).
pub const POCKET_Y: u8 = 28;
/// Distance value and unit rows. Rows 8 / 18 / 28 / road 39: even 10-row
/// steps (Drew: 8 / 17 / 27 / 39 read unevenly).
pub const DIST_Y: u8 = 8;
pub const UNIT_Y: u8 = 18;
pub const POCKET_RIGHT: u8 = 56;
/// Pocket page time.
const POCKET_PAGE: Duration = Duration::from_secs(3);
/// Everything the pocket text can cover: its row, left of the bar.
const POCKET_AREA: Rect = Rect::new(4, POCKET_Y, POCKET_RIGHT - 4, TEXT_H);

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
    /// Arrival time as local text, `14:32` or `2:32 PM` (AA sends it so; the
    /// caller formats CarPlay's Unix time). Re-formatted per [`Clock`].
    pub eta: Option<String>,
    /// The unit the phone shows distances in (`m`, `km`, `mi`, `ft`, `yd`):
    /// [`Units::Auto`] follows it.
    pub distance_unit: Option<String>,
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
    /// Show the trip-info pocket at all (its pages below).
    pub pocket: bool,
    /// Distance to the maneuver (value and unit column).
    pub show_distance: bool,
    /// Pocket pages.
    pub show_eta: bool,
    pub show_time_left: bool,
    pub show_dest_distance: bool,
    pub units: Units,
    pub clock: Clock,
    pub style: NavStyle,
}

/// `Fancy`: the Maps icons, distance, lanes and pocket. `Stock`: a lookalike
/// of the RNS-E's own nav screen: the ROM graphics-glyph arrow in the stock
/// 54x40 icon area, the stock bar, the road at the stock row (y=41); no
/// distance column (stock arrows reach across where it would be).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NavStyle {
    #[default]
    Fancy,
    Stock,
}

/// Distance units. `Auto`: whatever the phone shows (Drew's phone: imperial).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Units {
    #[default]
    Auto,
    Metric,
    Imperial,
}

/// ETA clock. `Auto`: 12 h if the phone's ETA has AM/PM.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Clock {
    #[default]
    Auto,
    H12,
    H24,
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
            show_distance: true,
            show_eta: true,
            show_time_left: true,
            show_dest_distance: true,
            units: Units::Auto,
            clock: Clock::Auto,
            style: NavStyle::Fancy,
        }
    }
}

/// Metric: `("350", "m")`, `("1.2", "km")`, `("12", "km")`, `("--", "")`.
pub fn format_distance(d: Option<f64>) -> (String, &'static str) {
    format_distance_in(d, false)
}

/// Metric, or imperial: feet below 0.1 mi in 50 ft steps (`"450", "ft"`),
/// then `"0.3", "mi"` and from 10 mi whole miles.
pub fn format_distance_in(d: Option<f64>, imperial: bool) -> (String, &'static str) {
    let Some(d) = d.filter(|d| d.is_finite() && *d >= 0.0) else {
        return ("--".into(), "");
    };
    if imperial {
        let mi = d / 1609.344;
        let ft = d * 3.280_84;
        return match mi {
            mi if mi < 0.1 => (format!("{}", ((ft / 50.0).round() * 50.0) as u32), "ft"),
            mi if mi < 9.95 => (format!("{mi:.1}"), "mi"),
            mi => (format!("{}", mi.round() as u32), "mi"),
        };
    }
    match d {
        d if d < 995.0 => (format!("{}", ((d / 10.0).round() * 10.0) as u32), "m"),
        d if d < 9950.0 => (format!("{:.1}", d / 1000.0), "km"),
        d => (format!("{}", (d / 1000.0).round() as u32), "km"),
    }
}

/// Imperial distances for this input and config.
pub fn imperial(i: &NavInput, cfg: &NavConfig) -> bool {
    match cfg.units {
        Units::Metric => false,
        Units::Imperial => true,
        Units::Auto => matches!(i.distance_unit.as_deref(), Some("mi" | "ft" | "yd")),
    }
}

/// ETA forms, longest first, from the phone's text (`14:32`, `2:32 PM`):
/// just the time, no label (Drew: too crowded). 12 h is always `2:32pm`,
/// lowercase, no space, else `2:32`; 24 h `14:32`. Unparseable text is shown
/// as it came.
pub fn eta_forms(text: &str, clock: Clock) -> Vec<String> {
    let t = text.trim();
    let upper = t.to_ascii_uppercase();
    let pm = upper.ends_with("PM");
    let ampm = pm || upper.ends_with("AM");
    let digits = upper.trim_end_matches("AM").trim_end_matches("PM").trim();
    let parsed = digits
        .split_once(':')
        .and_then(|(h, m)| Some((h.trim().parse::<u32>().ok()?, m.trim().parse::<u32>().ok()?)));
    let Some((h, m)) = parsed.filter(|(h, m)| *h < 24 && *m < 60) else {
        return vec![t.to_string()];
    };
    let h24 = match (ampm, pm, h) {
        (true, true, 12) | (false, _, _) => h,
        (true, true, h) => h + 12,
        (true, false, 12) => 0,
        (true, false, h) => h,
    };
    let twelve = match clock {
        Clock::H12 => true,
        Clock::H24 => false,
        Clock::Auto => ampm,
    };
    if twelve {
        let h12 = if h24 % 12 == 0 { 12 } else { h24 % 12 };
        let ap = if h24 < 12 { "am" } else { "pm" };
        vec![format!("{h12}:{m:02}{ap}"), format!("{h12}:{m:02}")]
    } else {
        vec![format!("{h24:02}:{m:02}")]
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
pub fn pocket_pages(i: &NavInput, cfg: &NavConfig) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    if !cfg.pocket {
        return out;
    }
    if let Some(e) = i.eta.as_ref().filter(|e| !e.is_empty() && cfg.show_eta) {
        out.push(eta_forms(e, cfg.clock));
    }
    if let Some(s) = i
        .time_to_destination_s
        .filter(|s| s.is_finite() && *s >= 0.0 && cfg.show_time_left)
    {
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
    if let Some(d) = i
        .destination_distance_m
        .filter(|d| d.is_finite() && *d >= 0.0 && cfg.show_dest_distance)
    {
        let (v, u) = format_distance_in(Some(d), imperial(i, cfg));
        // Never without its unit (Drew: a bare "9.9" reads as nothing).
        out.push(vec![format!("{v} {u}"), format!("{v}{u}")]);
    }
    out
}

/// Stock icon area, cleared before the glyphs (the RNS-E's `52 05 02 00 1B 36 28`).
pub const STOCK_ICON: Rect = Rect::new(0, 0, 54, 40);
/// Stock road row.
pub const STOCK_ROAD_Y: u8 = 41;
/// Stock with info: the arrow's area (the fancy icon area, rows 0..37).
pub const STOCK_INFO_ICON: Rect = Rect::new(0, 0, 40, 38);

/// Stock with no info at all: the RNS-E's own layout (arrow in the 54x40
/// area, road at y=41). Any info on puts the ROM arrow in the fancy layout.
pub fn pure_stock(cfg: &NavConfig) -> bool {
    cfg.style == NavStyle::Stock
            && !cfg.show_distance
            && !(cfg.pocket && (cfg.show_eta || cfg.show_time_left || cfg.show_dest_distance))
            && cfg.lanes == LaneLayout::Off
}

/// The stock ROM object for a maneuver: `(type, direction)`, see `stock_nav`.
pub fn stock_object(m: Maneuver, left_hand_traffic: bool) -> (u8, u8) {
    use crate::icons::Side::{Left, Right};
    // Direction byte from a heading in 45-degree steps, + = right.
    let dir = |deg: i32| ((0x100 - ((deg.rem_euclid(360) + 22) / 45 % 8) * 0x20) & 0xFF) as u8;
    match m {
        Maneuver::Slight(Right) => (0x0D, 0xE0),
        Maneuver::Slight(Left) => (0x0D, 0x20),
        Maneuver::Turn(Right) => (0x0D, 0xC0),
        Maneuver::Turn(Left) => (0x0D, 0x40),
        Maneuver::Sharp(Right) => (0x0D, 0xA0),
        Maneuver::Sharp(Left) => (0x0D, 0x60),
        Maneuver::UTurn if left_hand_traffic => (0x19, 0xC0),
        Maneuver::UTurn => (0x19, 0x40),
        Maneuver::Fork(Left) => (0x13, 0x40),
        Maneuver::Fork(Right) => (0x13, 0xC0),
        Maneuver::RampOn(Left) | Maneuver::RampOff(Left) => (0x14, 0x20),
        Maneuver::RampOn(Right) | Maneuver::RampOff(Right) => (0x14, 0xE0),
        Maneuver::Roundabout(Some(a)) => {
            // Exit heading relative to the entry: AA's angle is how far round.
            let rel = if left_hand_traffic { a - 180 } else { 180 - a };
            (if left_hand_traffic { 0x16 } else { 0x15 }, dir(rel))
        }
        Maneuver::RoundaboutEnter | Maneuver::RoundaboutExit | Maneuver::Roundabout(None) => {
            (if left_hand_traffic { 0x16 } else { 0x15 }, 0x00)
        }
        _ => (0x03, 0x00),
    }
}

/// The ROM glyph records for a stock object, as text ops.
fn stock_records((t, d): (u8, u8)) -> Vec<Op> {
    let recs = crate::stock_nav::OBJECTS
        .iter()
        .find(|(ot, od, _)| *ot == t && *od == d)
        .or_else(|| crate::stock_nav::OBJECTS.iter().find(|(ot, _, _)| *ot == 0x03))
        .map_or(&[][..], |(_, _, r)| *r);
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < recs.len() {
        let n = usize::from(recs[i + 1]) + 2;
        let r = &recs[i..i + n];
        out.push(Op::Text {
            clip: FULL,
            x: r[3],
            y: r[4],
            flags: r[2],
            bytes: r[5..].to_vec(),
        });
        i += n;
    }
    out
}

/// A stock arrow recentred in the fancy icon area (0..80 px across, bottom
/// clear of the road row), for Stock with info shown. Returns the shifted
/// records and their ink as art in [`ICON`] coordinates (for the pocket).
fn stock_placed(obj: (u8, u8)) -> (Vec<Op>, Art) {
    let recs = stock_records(obj);
    // Ink in centre px.
    let mut ink = Vec::new();
    for op in &recs {
        if let Op::Text { x, y, flags, bytes, .. } = op {
            let font = Font::from_flags(*flags);
            for (k, b) in bytes.iter().enumerate() {
                for gy in 0..crate::glyphs::H {
                    let row = crate::glyphs::row(font, *b, gy);
                    for gx in 0..crate::glyphs::W {
                        if row & (0x8000 >> gx) != 0 {
                            ink.push((
                                i32::from(*x) * 2 + (k * 12 + gx) as i32,
                                i32::from(*y) * 2 + gy as i32,
                            ));
                        }
                    }
                }
            }
        }
    }
    let (minx, maxx) = ink
        .iter()
        .fold((i32::MAX, i32::MIN), |(a, b), (x, _)| (a.min(*x), b.max(*x)));
    let maxy = ink.iter().map(|(_, y)| *y).max().unwrap_or(0);
    // Whole logical steps: centre on x=40 px, bottom at most y=75 px.
    let dx = ((80 - (minx + maxx + 1)) as f32 / 4.0).round() as i32;
    let dy = ((75 - maxy).min(0) as f32 / 2.0).floor() as i32;
    let ops = recs
        .into_iter()
        .map(|op| match op {
            Op::Text {
                clip,
                x,
                y,
                flags,
                bytes,
            } => Op::Text {
                clip,
                x: (i32::from(x) + dx).clamp(0, 63) as u8,
                y: (i32::from(y) + dy).clamp(0, 47) as u8,
                flags,
                bytes,
            },
            other => other,
        })
        .collect();
    let (w, h) = (usize::from(ICON.w) * 2, usize::from(ICON.h) * 2);
    let mut px = vec![false; w * h];
    for (x, y) in ink {
        let (x, y) = (x + dx * 2, y + dy * 2 - i32::from(ICON.y) * 2);
        if (0..w as i32).contains(&x) && (0..h as i32).contains(&y) {
            px[y as usize * w + x as usize] = true;
        }
    }
    (ops, Art { w, h, px })
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
    /// Maneuver whose icon is up (or going up this frame).
    icon_shown: Option<Maneuver>,
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
            dist: CentredField::new(DIST_Y, 40, 56, Font::Prop),
            unit: CentredField::new(UNIT_Y, 40, 56, Font::Prop),
            road: {
                // Stock: the RNS-E's road row (y=0x29).
                let y = if pure_stock(&cfg) { STOCK_ROAD_Y } else { 39 };
                let mut r = ScrollText::new(y, 1, 63, Font::Prop);
                r.centred = true;
                r
            },
            bar: GlyphCells::new(BAR_Y.iter().map(|y| (BAR_X, *y)).collect()),
            pocket: TextSlot::new(POCKET_Y, 4, POCKET_RIGHT, Font::Prop),
            pocket_since: None,
            wake: None,
            icon_shown: None,
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
        if self.cfg.style == NavStyle::Stock {
            if self.icon_shown == Some(m) {
                return Vec::new();
            }
            self.icon_shown = Some(m);
            // The RNS-E's own sequence: clear the icon area, then the ROM
            // glyph records. Up to 140 B, more than one message: the clear
            // and the first records go together, the rest follow at once
            // (the RNS-E splits them too). With info shown, the arrow sits
            // recentred in the fancy icon area instead.
            let obj = stock_object(m, self.cfg.left_hand_traffic);
            let (area, records) = if pure_stock(&self.cfg) {
                (STOCK_ICON, stock_records(obj))
            } else {
                (STOCK_INFO_ICON, stock_placed(obj).0)
            };
            let mut groups = Vec::new();
            let mut ops = vec![Op::Clear(area)];
            let mut bytes = 7;
            for op in records {
                let n = op.encode().len();
                if bytes + n > 90 {
                    groups.push(Group::new("stock-icon", std::mem::take(&mut ops)));
                    bytes = 0;
                }
                bytes += n;
                ops.push(op);
            }
            groups.push(Group::new("stock-icon", ops));
            return groups;
        }
        self.icon_shown = Some(m);
        let art = icon_art(m, self.cfg.left_hand_traffic);
        self.icon.set(cx, m, &art)
    }

    /// The icon's ink, whichever style draws it (the pocket fits beside it).
    fn current_art(&self) -> Art {
        let m = self.input.maneuver();
        if self.cfg.style == NavStyle::Stock {
            stock_placed(stock_object(m, self.cfg.left_hand_traffic)).1
        } else {
            icon_art(m, self.cfg.left_hand_traffic)
        }
    }

    fn distance_groups(&mut self, cx: &Ctx) -> Vec<Group> {
        if !self.cfg.show_distance {
            return Vec::new();
        }
        let (v, u) = format_distance_in(self.input.distance_m, imperial(&self.input, &self.cfg));
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
        let pages = if self.cfg.pocket { pocket_pages(&self.input, &self.cfg) } else { Vec::new() };
        let left = pocket_left(&self.current_art());
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
        self.icon_shown = None;
        if pure_stock(&self.cfg) {
            self.road.reset();
            self.bar.reset();
            self.wake = None;
            let mut g = self.icon_groups(cx);
            g.extend(self.bar_groups());
            g.extend(self.road.poll(now, cx, "road"));
            return g;
        }
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
        if pure_stock(&self.cfg) {
            let mut g = self.bar_groups();
            g.extend(self.road.poll(now, cx, "road"));
            g.extend(self.icon_groups(cx));
            return g;
        }
        let mut g = self.distance_groups(cx);
        g.extend(self.bar_groups());
        g.extend(self.bottom_groups(now, cx));
        // A new icon may reach where the pocket text was: clear the pocket
        // first, let the icon draw over that, then fit the pocket to the new
        // icon. (Bench: the pocket's tail clear after the icon cut the new
        // arrow's stem.)
        let m = self.input.maneuver();
        if self.icon_shown.is_some_and(|s| s != m) {
            g.push(Group::new("pocket-clear", vec![Op::Clear(POCKET_AREA)]));
            self.pocket.reset();
        }
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
    fn imperial_and_12h() {
        assert_eq!(format_distance_in(Some(100.0), true), ("350".into(), "ft"));
        assert_eq!(format_distance_in(Some(500.0), true), ("0.3".into(), "mi"));
        assert_eq!(format_distance_in(Some(20_000.0), true), ("12".into(), "mi"));
        let i = NavInput {
            distance_unit: Some("mi".into()),
            ..Default::default()
        };
        assert!(imperial(&i, &NavConfig::default()));
        assert_eq!(eta_forms("2:32 PM", Clock::Auto), vec!["2:32pm", "2:32"]);
        assert_eq!(eta_forms("14:32", Clock::H12), vec!["2:32pm", "2:32"]);
        assert_eq!(eta_forms("12:05 AM", Clock::H24), vec!["00:05"]);
        assert_eq!(eta_forms("soon", Clock::Auto), vec!["soon"]);
    }

    #[test]
    fn pocket_forms() {
        let i = NavInput {
            eta: Some("14:32".into()),
            time_to_destination_s: Some(3900.0),
            destination_distance_m: Some(12_400.0),
            ..Default::default()
        };
        let p = pocket_pages(&i, &NavConfig::default());
        assert_eq!(p[0], vec!["14:32"]);
        assert_eq!(p[1], vec!["1h 05", "1h05", "1h"]);
        assert_eq!(p[2], vec!["12 km", "12km"]);
    }
}
