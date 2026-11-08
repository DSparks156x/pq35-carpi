//! Lane guidance art: the lane shapes from Drew's Maps SVGs, rendered at
//! 18 px and grid-fitted offline (`lane_icons`, made by
//! `examples/small_icons.rs`; downscaling the 72 px masks butchered them).
//!
//! Each lane shows one arrow: the one the phone highlights, else its first.
//! Lanes to take are solid; the others use the 50 % checker the Maps icons
//! use for secondary routes. Lanes that don't fit are dropped from the side
//! away from the highlighted ones.

use crate::icons::Art;
use crate::lane_icons::{ICONS, SIZE};
use crate::lane_icons_stock::ICONS as STOCK_ICONS;

/// One arrow of a lane, as carpi-core publishes it (`nav.lanes`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LaneArrow {
    pub shape: String,
    pub highlighted: bool,
}

pub type Lane = Vec<LaneArrow>;

/// Arrow size, px.
pub const AW: usize = SIZE;
pub const AH: usize = SIZE;

/// The mask for an AA lane shape (a few spellings), if we have one; `stock`
/// picks the stock-style set (square stems, broad heads, like the ROM arrows).
fn mask_in(shape: &str, stock: bool) -> Option<&'static [u32; SIZE]> {
    let name = match shape {
        "right" => "normal_right",
        "left" => "normal_left",
        "u_turn_left" => "uturn_left",
        "u_turn_right" => "uturn_right",
        s => s,
    };
    let set = if stock { STOCK_ICONS } else { ICONS };
    set.iter().find(|(n, _)| *n == name).map(|(_, m)| m)
}

#[cfg(test)]
fn mask(shape: &str) -> Option<&'static [u32; SIZE]> {
    mask_in(shape, false)
}

/// The arrow a lane shows: the highlighted one, else the first.
fn shown(lane: &Lane) -> Option<&LaneArrow> {
    lane.iter().find(|a| a.highlighted).or_else(|| lane.first())
}

/// Draw `lanes` left to right into a `w` x `h` px strip, centred, each lane in
/// a `pitch`-px cell, arrows vertically centred.
pub fn strip(lanes: &[Lane], w: usize, h: usize, pitch: usize) -> Art {
    strip_in(lanes, w, h, pitch, false)
}

/// [`strip`] with the stock-style arrows when `stock`.
pub fn strip_in(lanes: &[Lane], w: usize, h: usize, pitch: usize, stock: bool) -> Art {
    let mut px = vec![false; w * h];
    let fit = (w / pitch.max(1)).max(1);
    let (mut lo, mut hi) = (0, lanes.len());
    let first = lanes
        .iter()
        .position(|l| l.iter().any(|a| a.highlighted))
        .unwrap_or(0);
    while hi - lo > fit {
        if first.saturating_sub(lo) > (hi - 1).saturating_sub(first) {
            lo += 1;
        } else {
            hi -= 1;
        }
    }
    let lanes = &lanes[lo..hi];
    let x0 = w.saturating_sub(lanes.len() * pitch) / 2;
    let y0 = h.saturating_sub(AH) / 2;
    for (i, lane) in lanes.iter().enumerate() {
        let Some(a) = shown(lane) else { continue };
        let Some(m) = mask_in(&a.shape, stock) else { continue };
        let cx = x0 + i * pitch + pitch.saturating_sub(AW) / 2;
        for (y, row) in m.iter().enumerate() {
            for x in 0..AW {
                let (px_x, px_y) = (cx + x, y0 + y);
                if row & (0x8000_0000 >> x) == 0 || px_x >= w || px_y >= h {
                    continue;
                }
                if a.highlighted || (px_x + px_y).is_multiple_of(2) {
                    px[px_y * w + px_x] = true;
                }
            }
        }
    }
    Art { w, h, px }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lane(shapes: &[(&str, bool)]) -> Lane {
        shapes
            .iter()
            .map(|(s, h)| LaneArrow {
                shape: (*s).into(),
                highlighted: *h,
            })
            .collect()
    }

    #[test]
    fn every_shape_has_ink() {
        for s in [
            "straight", "slight_left", "slight_right", "normal_left", "normal_right",
            "sharp_left", "sharp_right", "uturn_left", "uturn_right",
        ] {
            assert!(mask(s).is_some_and(|m| m.iter().any(|r| *r != 0)), "{s}");
        }
    }

    #[test]
    fn strip_keeps_the_highlighted_lanes() {
        let lanes: Vec<Lane> = (0..9).map(|i| lane(&[("straight", i == 8)])).collect();
        // 3 cells fit: the highlighted last lane must be among them.
        let a = strip(&lanes, 63, 18, 21);
        let lit = (42..63)
            .flat_map(|x| (0..18).map(move |y| (x, y)))
            .filter(|(x, y)| a.get(*x, *y))
            .count();
        assert!(lit > 20, "{lit}");
    }
}
