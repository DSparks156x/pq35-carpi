//! Readings: a title row and a 2x4 grid of icon + value + unit, on subpages
//! the wheel steps through (Drew: up/down while DIS focus is on).
//!
//! Layout (logical), the old page's (`dc/apps/readings.py`,
//! `dc/readings_assets.py`), minus the centre line Drew didn't want:
//! - title y=0: page name, centred.
//!   While the DIS has focus (the wheel steps pages) the name is inverted:
//!   `52` fill of its footprint + black text clipped to it, the old trick
//!   (`dc/dis_service.py:515-527`);
//! - cell i: column `i / 4` at cx = 0 or 32, row `i % 4` at ry = 12/21/30/39
//!   (left column top to bottom, then right: carpi-core's order);
//! - icon rect (cx, ry, 5, 8), 10x14 mask at px y=1;
//! - value centred in x cx+6..cx+26 (40 px: four proportional digits);
//! - unit legend rect (cx+27, ry, 5, 9), vertically centred.
//!
//! Order on enter: title, values, then icon and unit bases; the native refine
//! follows in the refine lane. A value change re-sends one centred run, its
//! tail clear in the same group. Icons and units never change within a page.

use super::Screen;
use crate::font::{Font, audscii};
use crate::cmd::{Ink, Op, text_flags};
use crate::font::TEXT_H;
use crate::helpers::{CentredField, Ctx, QuickThenRefineIcon, TextSlot};
use crate::icons::Art;
use crate::pack::{Group, PackError, Packer};
use crate::proto::Rect;
use crate::reading_art::{ICON_TERMS, ICONS, Mask, UNITS};
use std::time::Instant;

pub const ROWS: [u8; 4] = [12, 21, 30, 39];
pub const CELLS: usize = 8;

/// One cell's config, as carpi-core has it today.
#[derive(Clone, Debug, PartialEq)]
pub struct ReadingSpec {
    /// Store name, e.g. `engine.coolant_c`. The caller reads it; the screen doesn't.
    pub key: String,
    pub label: String,
    pub decimals: u8,
}

/// One cell on a page. `icon`/`unit` `None` = picked from the key.
#[derive(Clone, Debug, PartialEq)]
pub struct ReadingCell {
    pub key: String,
    pub decimals: u8,
    pub icon: Option<String>,
    pub unit: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReadingsPage {
    pub name: String,
    pub cells: Vec<ReadingCell>,
}

impl ReadingsPage {
    pub fn from_specs(name: &str, specs: &[ReadingSpec]) -> Self {
        ReadingsPage {
            name: name.into(),
            cells: specs
                .iter()
                .map(|s| ReadingCell {
                    key: s.key.clone(),
                    decimals: s.decimals,
                    icon: None,
                    unit: None,
                })
                .collect(),
        }
    }
}

/// Inputs: the current page's values in cell order. `None` shows `--`.
/// `title` names the page when it has no name of its own.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReadingsInput {
    pub title: String,
    pub values: [Option<f64>; 8],
}

/// Icon for a store key: the old term table first, then our own key names.
pub fn icon_for(key: &str) -> &'static str {
    const OURS: &[(&str, &str)] = &[
        ("haldex", "awd"),
        ("oil_temp", "oil"),
        ("coolant", "cool"),
        ("iat", "intake"),
        ("battery", "voltage"),
        ("afr", "lambda"),
        ("outside_temp", "ambient"),
        ("odometer", "distance"),
        ("dim_", "lights"),
    ];
    let k = key.to_ascii_lowercase();
    ICON_TERMS
        .iter()
        .chain(OURS)
        .find(|(t, _)| k.contains(t))
        .map_or("engine", |(_, i)| i)
}

/// Unit legend from the key's suffix (`_c`, `_kmh`, ...). `""` = none.
pub fn unit_for(key: &str) -> &'static str {
    const SUFFIX: &[(&str, &str)] = &[
        ("_kmh", "km/h"),
        ("_mph", "mph"),
        ("rpm", "rpm"),
        ("_c", "°C"),
        ("_f", "°F"),
        ("_v", "V"),
        ("_a", "A"),
        ("_l", "L"),
        ("_km", "km"),
        ("_pct", "%"),
        ("_bar", "bar"),
        ("_mbar", "mbar"),
        ("_kpa", "kPa"),
        ("_psi", "psi"),
        ("_nm", "Nm"),
        ("_deg", "°"),
        ("_ms", "ms"),
        ("_gs", "g/s"),
        ("_s", "s"),
    ];
    let k = key.to_ascii_lowercase();
    SUFFIX
        .iter()
        .filter(|(s, _)| k.ends_with(s))
        .max_by_key(|(s, _)| s.len())
        .map_or("", |(_, u)| u)
}

fn find(table: &'static [(&str, Mask)], name: &str) -> Option<Mask> {
    table.iter().find(|(n, _)| *n == name).map(|(_, m)| *m)
}

/// `m` in a `rect`-sized art at px (`x0`, `y0`).
fn place(rect: Rect, m: Option<Mask>, x0: usize, y0: usize) -> Art {
    let (w, h) = (usize::from(rect.w) * 2, usize::from(rect.h) * 2);
    let mut px = vec![false; w * h];
    if let Some(m) = m {
        for (y, _) in m.rows.iter().enumerate() {
            for x in 0..usize::from(m.w) {
                if m.get(x, y) && x0 + x < w && y0 + y < h {
                    px[(y0 + y) * w + x0 + x] = true;
                }
            }
        }
    }
    Art { w, h, px }
}

fn cell_xy(i: usize) -> (u8, u8) {
    ((i / 4) as u8 * 32, ROWS[i % 4])
}

pub fn icon_rect(i: usize) -> Rect {
    let (cx, ry) = cell_xy(i);
    Rect::new(cx, ry, 5, 8)
}

pub fn unit_rect(i: usize) -> Rect {
    let (cx, ry) = cell_xy(i);
    Rect::new(cx + 27, ry, 5, 9)
}

/// Icon art for a cell, as it fills [`icon_rect`].
pub fn icon_art(i: usize, c: &ReadingCell) -> Art {
    let name = c.icon.as_deref().unwrap_or_else(|| icon_for(&c.key));
    let m = find(ICONS, name).or_else(|| find(ICONS, "engine"));
    place(icon_rect(i), m, 0, 1)
}

/// Unit art for a cell, as it fills [`unit_rect`], vertically centred.
pub fn unit_art(i: usize, c: &ReadingCell) -> Art {
    let name = c.unit.as_deref().unwrap_or_else(|| unit_for(&c.key));
    let m = find(UNITS, name);
    let h = m.map_or(0, |m| m.rows.len());
    place(unit_rect(i), m, 0, 1.max(18usize.saturating_sub(h) / 2))
}

pub fn format_value(v: Option<f64>, decimals: u8) -> String {
    match v.filter(|v| v.is_finite()) {
        Some(v) => format!("{:.*}", usize::from(decimals), v),
        None => "--".into(),
    }
}

pub struct ReadingsScreen {
    pages: Vec<ReadingsPage>,
    page: usize,
    /// Pages came from bare specs: the title comes from the input.
    unnamed: bool,
    input: ReadingsInput,
    focus: bool,
    /// Inverted title as last sent: (bytes, fill rect).
    inverted: Option<(Vec<u8>, Rect)>,
    title: TextSlot,
    values: Vec<CentredField>,
    icons: Vec<QuickThenRefineIcon<String>>,
    units: Vec<QuickThenRefineIcon<String>>,
}

impl ReadingsScreen {
    /// One page from bare specs (carpi-core today). Specs beyond 8 are ignored.
    pub fn new(specs: Vec<ReadingSpec>) -> Self {
        let mut s = Self::with_pages(vec![ReadingsPage::from_specs("", &specs)]);
        s.unnamed = true;
        s
    }

    /// Subpages; empty gets one empty page.
    pub fn with_pages(mut pages: Vec<ReadingsPage>) -> Self {
        if pages.is_empty() {
            pages.push(ReadingsPage {
                name: String::new(),
                cells: Vec::new(),
            });
        }
        let quick = |r| {
            let mut q = QuickThenRefineIcon::new(r);
            q.on_clear = true;
            q
        };
        ReadingsScreen {
            pages,
            page: 0,
            unnamed: false,
            input: ReadingsInput::default(),
            focus: false,
            inverted: None,
            title: TextSlot::new(0, 0, 64, Font::Prop),
            values: (0..CELLS)
                .map(|i| {
                    let (cx, ry) = cell_xy(i);
                    CentredField::new(ry, cx + 6, cx + 26, Font::Prop)
                })
                .collect(),
            icons: (0..CELLS).map(|i| quick(icon_rect(i))).collect(),
            units: (0..CELLS).map(|i| quick(unit_rect(i))).collect(),
        }
    }

    pub fn page_index(&self) -> usize {
        self.page
    }

    /// New pages in place (the settings changed), keeping the page shown
    /// where it still exists and the focus mark. The caller re-enters the
    /// screen so it redraws whole.
    pub fn replace_pages(&mut self, next: ReadingsScreen) {
        let page = self.page.min(next.pages.len().saturating_sub(1));
        let focus = self.focus;
        *self = next;
        self.page = page;
        self.focus = focus;
    }
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }
    pub fn page(&self) -> &ReadingsPage {
        &self.pages[self.page]
    }

    /// Step subpages, wrapping. The caller re-enters the screen (a blank on
    /// page switch is fine, Drew). Returns whether the page changed.
    pub fn step(&mut self, delta: i32) -> bool {
        let n = self.pages.len() as i32;
        let next = (self.page as i32 + delta).rem_euclid(n) as usize;
        let moved = next != self.page;
        self.page = next;
        moved
    }

    pub fn set(&mut self, input: ReadingsInput) {
        self.input = input;
    }

    /// DIS focus on: the wheel steps subpages, and the page name shows inverted.
    pub fn set_focus(&mut self, on: bool) {
        self.focus = on;
    }

    fn cells(&self) -> &[ReadingCell] {
        let c = &self.pages[self.page].cells;
        &c[..c.len().min(CELLS)]
    }

    fn value_groups(&mut self, cx: &Ctx) -> Vec<Group> {
        let specs: Vec<(Option<f64>, u8)> = self
            .cells()
            .iter()
            .enumerate()
            .map(|(i, c)| (self.input.values[i], c.decimals))
            .collect();
        // Too wide with its decimals (MAF 100.0): fewer decimals before `--`.
        let texts: Vec<String> = specs
            .iter()
            .zip(&self.values)
            .map(|(&(v, dp), f)| {
                (0..=dp)
                    .rev()
                    .map(|d| format_value(v, d))
                    .find(|t| {
                        let b = audscii(t);
                        cx.metrics.width(f.slot.font, &b) <= f.slot.width_px() && b.len() <= cx.text_cap()
                    })
                    .unwrap_or_else(|| format_value(v, dp))
            })
            .collect();
        let mut out = Vec::new();
        for (f, t) in self.values.iter_mut().zip(texts) {
            out.extend(f.set(cx, "value", &t));
        }
        out
    }

    fn title_groups(&mut self, cx: &Ctx) -> Vec<Group> {
        let m = &cx.metrics;
        let name = if self.unnamed {
            self.input.title.clone()
        } else {
            self.pages[self.page].name.clone()
        };
        let b = audscii(&name);
        let n = m
            .fit(Font::Prop, &b, self.title.width_px())
            .min(cx.text_cap());
        let b = b[..n].to_vec();
        let x = self.title.centre_x(m, &b);
        let area = Rect::new(0, 0, 64, TEXT_H);
        let mut out: Vec<Group> = Vec::new();
        if self.focus {
            let w = (m.width(Font::Prop, &b).div_ceil(2) as u8).clamp(1, 64 - x);
            let rect = Rect::new(x, 0, w, TEXT_H);
            if self.inverted.as_ref() != Some(&(b.clone(), rect)) {
                // Clear whatever was there, fill, black text: one group.
                self.inverted = Some((b.clone(), rect));
                out.push(Group::new(
                    "title-focus",
                    vec![
                        Op::Clear(area),
                        Op::Fill(rect),
                        Op::Text {
                            clip: rect,
                            x: 0,
                            y: 0,
                            flags: text_flags(Font::Prop, Ink::Black),
                            bytes: b,
                        },
                    ],
                ));
                self.title.reset();
            }
        } else if let Some((_, rect)) = self.inverted.take() {
            // Back to normal: the fill's clear rides with the plain run.
            let mut g = self.title.draw(m, "title", x, b).map_or_else(Vec::new, |g| g.ops);
            g.insert(0, Op::Clear(rect));
            out.push(Group::new("title", g));
        } else {
            out.extend(self.title.draw(m, "title", x, b));
        }
        out
    }
}

impl Screen for ReadingsScreen {
    fn enter(&mut self, _now: Instant, cx: &Ctx) -> Vec<Group> {
        self.title.reset();
        self.inverted = None;
        self.values.iter_mut().for_each(CentredField::reset);
        self.icons.iter_mut().for_each(QuickThenRefineIcon::reset);
        self.units.iter_mut().for_each(QuickThenRefineIcon::reset);
        let mut out = self.title_groups(cx);
        out.extend(self.value_groups(cx));
        let cells: Vec<ReadingCell> = self.cells().to_vec();
        for (i, c) in cells.iter().enumerate() {
            let key = c.icon.clone().unwrap_or_else(|| icon_for(&c.key).into());
            out.extend(self.icons[i].set(cx, key, &icon_art(i, c)));
        }
        for (i, c) in cells.iter().enumerate() {
            let key = c.unit.clone().unwrap_or_else(|| unit_for(&c.key).into());
            out.extend(self.units[i].set(cx, key, &unit_art(i, c)));
        }
        out
    }

    fn update(&mut self, _now: Instant, cx: &Ctx) -> Vec<Group> {
        let mut out = self.title_groups(cx);
        out.extend(self.value_groups(cx));
        out
    }

    /// Icons, then units, until the slice is full.
    fn refine(&mut self, _cx: &Ctx, p: &mut Packer, max_msgs: usize) -> Result<(), PackError> {
        for q in self.icons.iter_mut().chain(self.units.iter_mut()) {
            if !q.refining() {
                continue;
            }
            q.refine(p, max_msgs)?;
            if q.refining() {
                break;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_pick_icons_and_units() {
        assert_eq!(icon_for("engine.coolant_c"), "cool");
        assert_eq!(icon_for("power.battery_v"), "voltage");
        assert_eq!(icon_for("engine.rpm"), "rpm");
        assert_eq!(icon_for("vehicle.speed_kmh"), "speed");
        assert_eq!(icon_for("something.else"), "engine");
        assert_eq!(unit_for("engine.coolant_c"), "°C");
        assert_eq!(unit_for("vehicle.speed_kmh"), "km/h");
        assert_eq!(unit_for("engine.rpm"), "rpm");
        assert_eq!(unit_for("power.battery_v"), "V");
        assert_eq!(unit_for("light.dim_display_pct"), "%");
        assert_eq!(unit_for("engine.boost_mbar"), "mbar");
        for (_, u) in [("", "km/h"), ("", "°C"), ("", "rpm"), ("", "V"), ("", "%")] {
            assert!(find(UNITS, u).is_some(), "{u}");
        }
    }

    #[test]
    fn step_wraps() {
        let p = |n: &str| ReadingsPage {
            name: n.into(),
            cells: vec![],
        };
        let mut s = ReadingsScreen::with_pages(vec![p("A"), p("B"), p("C")]);
        assert!(s.step(-1));
        assert_eq!(s.page().name, "C");
        assert!(s.step(1));
        assert_eq!(s.page_index(), 0);
        let mut one = ReadingsScreen::with_pages(vec![p("A")]);
        assert!(!one.step(1));
    }
}
