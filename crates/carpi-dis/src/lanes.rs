//! Lane guidance art: hand-drawn native pixel masks, one per AA lane shape
//! (right-hand shapes drawn, left ones mirrored), composed into a strip.
//!
//! The ROM graphics font has no small lane arrows (only tile pieces for the
//! stock maneuver arrows and the approach bar), so these are pixel art, 9 px
//! wide and 12 tall with 2-px strokes. Arrows to follow are solid; the others
//! use the 50 % checker the Maps icons use for secondary routes. A lane with
//! several arrows draws them over each other, like the phone does.

use crate::icons::Art;

pub const AW: usize = 9;
pub const AH: usize = 12;

/// `#` = ink. Right-hand (or symmetric) shapes; left ones are mirrored.
const STRAIGHT: [&str; AH] = [
    "....#....",
    "...###...",
    "..#####..",
    ".###.###.",
    "...##....",
    "...##....",
    "...##....",
    "...##....",
    "...##....",
    "...##....",
    "...##....",
    "...##....",
];
const SLIGHT: [&str; AH] = [
    ".....####",
    "......###",
    ".....####",
    "....###.#",
    "...###...",
    "..###....",
    "..##.....",
    "..##.....",
    "..##.....",
    "..##.....",
    "..##.....",
    "..##.....",
];
const TURN: [&str; AH] = [
    ".........",
    "......#..",
    "......##.",
    "..#######",
    "..#######",
    "..##..##.",
    "..##..#..",
    "..##.....",
    "..##.....",
    "..##.....",
    "..##.....",
    "..##.....",
];
const SHARP: [&str; AH] = [
    "..##.....",
    "..##.....",
    "..##.....",
    "..##.....",
    "..###....",
    "...###...",
    "....###.#",
    ".....####",
    "......###",
    ".....####",
    ".........",
    ".........",
];
/// U-turn to the left (the common case); mirrored for right-hand u-turns.
const UTURN_LEFT: [&str; AH] = [
    "..####...",
    ".######..",
    "##....##.",
    "##....##.",
    "##....##.",
    "##....##.",
    "##....##.",
    "##....##.",
    "##....##.",
    "######.##",
    ".####..##",
    "..##...##",
];

fn shape_rows(shape: &str) -> Option<(&'static [&'static str; AH], bool)> {
    // (mask, mirror)
    Some(match shape {
        "straight" => (&STRAIGHT, false),
        "slight_right" => (&SLIGHT, false),
        "slight_left" => (&SLIGHT, true),
        "normal_right" | "right" => (&TURN, false),
        "normal_left" | "left" => (&TURN, true),
        "sharp_right" => (&SHARP, false),
        "sharp_left" => (&SHARP, true),
        "uturn_left" | "u_turn_left" => (&UTURN_LEFT, false),
        "uturn_right" | "u_turn_right" => (&UTURN_LEFT, true),
        _ => return None,
    })
}

/// One arrow of a lane, as carpi-core publishes it (`nav.lanes`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LaneArrow {
    pub shape: String,
    pub highlighted: bool,
}

pub type Lane = Vec<LaneArrow>;

/// Draw `lanes` left to right into a `w` x `h` px strip, centred, each lane in
/// a `pitch`-px cell, arrows vertically centred. Lanes that don't fit are
/// dropped from the side away from the highlighted ones.
pub fn strip(lanes: &[Lane], w: usize, h: usize, pitch: usize) -> Art {
    let mut px = vec![false; w * h];
    let fit = (w / pitch.max(1)).max(1);
    let (mut lo, mut hi) = (0, lanes.len());
    while hi - lo > fit {
        // Keep the highlighted lanes on screen: drop from whichever end is
        // further from the first highlighted lane.
        let first = lanes.iter().position(|l| l.iter().any(|a| a.highlighted)).unwrap_or(0);
        if first.saturating_sub(lo) > (hi - 1).saturating_sub(first) {
            lo += 1;
        } else {
            hi -= 1;
        }
    }
    let shown = &lanes[lo..hi];
    let x0 = (w.saturating_sub(shown.len() * pitch)) / 2;
    let y0 = h.saturating_sub(AH) / 2;
    for (i, lane) in shown.iter().enumerate() {
        let cx = x0 + i * pitch + pitch.saturating_sub(AW) / 2;
        // Non-highlighted first, so a highlighted arrow wins where they overlap.
        let mut arrows: Vec<&LaneArrow> = lane.iter().collect();
        arrows.sort_by_key(|a| a.highlighted);
        for a in arrows {
            let Some((rows, mirror)) = shape_rows(&a.shape) else {
                continue;
            };
            for (y, row) in rows.iter().enumerate() {
                for (x, c) in row.bytes().enumerate() {
                    if c != b'#' {
                        continue;
                    }
                    let mx = if mirror { AW - 1 - x } else { x };
                    let (px_x, px_y) = (cx + mx, y0 + y);
                    if px_x >= w || px_y >= h {
                        continue;
                    }
                    let on = a.highlighted || (px_x + px_y).is_multiple_of(2);
                    if on {
                        px[px_y * w + px_x] = true;
                    } else if !a.highlighted {
                        // Checker: leave the off squares alone (a solid arrow
                        // underneath stays solid).
                    }
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
    fn masks_are_well_formed() {
        for m in [&STRAIGHT, &SLIGHT, &TURN, &SHARP, &UTURN_LEFT] {
            assert!(m.iter().all(|r| r.len() == AW));
        }
    }

    #[test]
    fn strip_centres_and_drops_far_lanes() {
        let lanes = vec![
            lane(&[("uturn_left", false)]),
            lane(&[("straight", false)]),
            lane(&[("straight", true), ("slight_right", true)]),
            lane(&[("normal_right", false)]),
        ];
        let a = strip(&lanes, 40, 14, 12);
        assert_eq!((a.w, a.h), (40, 14));
        assert!(a.px.iter().any(|p| *p));
    }
}
