//! Maneuver icons: the old driver's 72x72 Google Maps masks (`nav_icon_data`),
//! the set Drew approved on the cluster. Placed at physical (6,2) in a
//! 128x96 canvas, like the old code (`dc/nav_icons.py`), which leaves distance
//! text from x84, the road from y78 and the bar from x114 clear.
//!
//! Maneuver → icon follows the old mapping (`dc/apps/nav.py` `_get_icon_name`),
//! including the roundabout angle buckets. Unknown → STRAIGHT.

use crate::nav_icon_data::{BLOB, BYTES, NAMES, SIZE};

/// A 1-bit image, physical px, row-major.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Art {
    pub w: usize,
    pub h: usize,
    pub px: Vec<bool>,
}

impl Art {
    pub fn get(&self, x: usize, y: usize) -> bool {
        self.px[y * self.w + x]
    }

    /// Logical base: bottom-right pixel of each cell, MSB-first rows. The native
    /// refine (base + XOR V/H/dot planes) can't change that pixel
    /// (`dc/native_bitmap.py:353-360`, bench).
    pub fn base_rows(&self) -> Vec<Vec<u8>> {
        let (lw, lh) = (self.w / 2, self.h / 2);
        (0..lh)
            .map(|ly| {
                let mut row = vec![0u8; lw.div_ceil(8)];
                for lx in 0..lw {
                    if self.get(lx * 2 + 1, ly * 2 + 1) {
                        row[lx / 8] |= 0x80 >> (lx % 8);
                    }
                }
                row
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Left,
    Right,
}

/// What we can show. Mapped from AA's maneuver vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Maneuver {
    Depart,
    Straight,
    Slight(Side),
    Turn(Side),
    Sharp(Side),
    UTurn,
    RampOn(Side),
    RampOff(Side),
    Fork(Side),
    Merge(Option<Side>),
    /// Entering; no exit known.
    RoundaboutEnter,
    RoundaboutExit,
    /// AA's exit angle in degrees (180 = straight on), if it sent one.
    Roundabout(Option<i32>),
    FerryBoat,
    FerryTrain,
    Destination(Option<Side>),
    Unknown,
}

impl Maneuver {
    /// From `nav.maneuver` / `side` / `roundabout_angle` as carpi-core publishes
    /// them (`carpi-proto` `NavState`).
    pub fn from_aa(maneuver: &str, side: Option<&str>, angle: Option<i32>) -> Maneuver {
        let opt = match side {
            Some("left") => Some(Side::Left),
            Some("right") => Some(Side::Right),
            _ => None,
        };
        let s = opt.unwrap_or(Side::Right);
        match maneuver {
            "depart" => Maneuver::Depart,
            "straight" | "continue" | "name_change" => Maneuver::Straight,
            "slight_turn" | "keep" => Maneuver::Slight(s),
            "turn" => Maneuver::Turn(s),
            "sharp_turn" => Maneuver::Sharp(s),
            "u_turn" => Maneuver::UTurn,
            "on_ramp" => Maneuver::RampOn(s),
            "off_ramp" => Maneuver::RampOff(s),
            "fork" => Maneuver::Fork(s),
            "merge" => Maneuver::Merge(opt),
            "roundabout_enter" => Maneuver::RoundaboutEnter,
            "roundabout_exit" => Maneuver::RoundaboutExit,
            "roundabout_enter_and_exit" => Maneuver::Roundabout(angle),
            "ferry_boat" | "ferry" => Maneuver::FerryBoat,
            "ferry_train" => Maneuver::FerryTrain,
            "destination" => Maneuver::Destination(opt),
            _ => Maneuver::Unknown,
        }
    }

    /// Icon name in the old set. `left_hand_traffic` makes roundabouts and
    /// U-turns clockwise.
    pub fn icon_name(self, left_hand_traffic: bool) -> String {
        let lr = |s: Side| if s == Side::Left { "LEFT" } else { "RIGHT" };
        let rot = if left_hand_traffic {
            "CLOCKWISE"
        } else {
            "COUNTERCLOCKWISE"
        };
        match self {
            Maneuver::Depart => "DEPART".into(),
            Maneuver::Straight | Maneuver::Unknown => "STRAIGHT".into(),
            Maneuver::Slight(s) => format!("TURN_SLIGHT_{}", lr(s)),
            Maneuver::Turn(s) => format!("TURN_{}", lr(s)),
            Maneuver::Sharp(s) => format!("TURN_SHARP_{}", lr(s)),
            Maneuver::UTurn => format!("TURN_U_TURN_{rot}"),
            Maneuver::RampOn(s) => format!("RAMP_ON_{}", lr(s)),
            Maneuver::RampOff(s) => format!("RAMP_OFF_{}", lr(s)),
            Maneuver::Fork(s) => format!("FORK_{}", lr(s)),
            Maneuver::Merge(None) => "MERGE".into(),
            Maneuver::Merge(Some(s)) => format!("MERGE_{}", lr(s)),
            Maneuver::RoundaboutEnter | Maneuver::Roundabout(None) => format!("ROUNDABOUT_{rot}"),
            Maneuver::RoundaboutExit => format!("ROUNDABOUT_EXIT_{rot}"),
            Maneuver::Roundabout(Some(a)) => {
                let a = a.rem_euclid(360);
                // Old rule: the side comes from the angle, not AA's side field.
                let side = if (a < 180) != left_hand_traffic {
                    "RIGHT"
                } else {
                    "LEFT"
                };
                let shape = match a {
                    23..=67 | 293..=337 => "SHARP",
                    68..=112 | 248..=292 => "NORMAL",
                    113..=157 | 203..=247 => "SLIGHT",
                    158..=202 => "STRAIGHT",
                    _ => "U_TURN",
                };
                match shape {
                    "STRAIGHT" | "U_TURN" => format!("ROUNDABOUT_{shape}_{rot}"),
                    "NORMAL" => format!("ROUNDABOUT_{side}_{rot}"),
                    _ => format!("ROUNDABOUT_{shape}_{side}_{rot}"),
                }
            }
            Maneuver::FerryBoat => "FERRY_BOAT".into(),
            Maneuver::FerryTrain => "FERRY_TRAIN".into(),
            Maneuver::Destination(None) => "DESTINATION".into(),
            Maneuver::Destination(Some(s)) => format!("DESTINATION_{}", lr(s)),
        }
    }
}

/// The 72x72 mask for `name`, or STRAIGHT if there's no such icon.
pub fn mask(name: &str) -> &'static [u8] {
    let i = NAMES
        .iter()
        .position(|n| *n == name)
        .or_else(|| NAMES.iter().position(|n| *n == "STRAIGHT"))
        .expect("STRAIGHT is in the set");
    &BLOB[i * BYTES..(i + 1) * BYTES]
}

/// Icon `m` in a `w` x `h` px box with the 72x72 mask at (`x0`, `y0`).
pub fn draw(m: Maneuver, left_hand_traffic: bool, w: usize, h: usize, x0: usize, y0: usize) -> Art {
    assert!(x0 + SIZE <= w && y0 + SIZE <= h, "icon box too small");
    let bits = mask(&m.icon_name(left_hand_traffic));
    let mut px = vec![false; w * h];
    for y in 0..SIZE {
        for x in 0..SIZE {
            if bits[y * SIZE / 8 + x / 8] & (0x80 >> (x % 8)) != 0 {
                px[(y0 + y) * w + x0 + x] = true;
            }
        }
    }
    Art { w, h, px }
}

/// Icon `m` scaled to `size` x `size` px (box-filtered, 50 % coverage) at
/// (`x0`, `y0`) in a `w` x `h` box. For layouts that need a smaller icon.
pub fn draw_scaled(
    m: Maneuver,
    left_hand_traffic: bool,
    w: usize,
    h: usize,
    x0: usize,
    y0: usize,
    size: usize,
) -> Art {
    assert!(x0 + size <= w && y0 + size <= h, "icon box too small");
    let bits = mask(&m.icon_name(left_hand_traffic));
    let at = |x: usize, y: usize| bits[y * SIZE / 8 + x / 8] & (0x80 >> (x % 8)) != 0;
    let mut px = vec![false; w * h];
    for oy in 0..size {
        for ox in 0..size {
            let (sx0, sx1) = (ox * SIZE / size, ((ox + 1) * SIZE).div_ceil(size));
            let (sy0, sy1) = (oy * SIZE / size, ((oy + 1) * SIZE).div_ceil(size));
            let mut on = 0;
            let mut n = 0;
            for y in sy0..sy1.min(SIZE) {
                for x in sx0..sx1.min(SIZE) {
                    on += usize::from(at(x, y));
                    n += 1;
                }
            }
            px[(y0 + oy) * w + x0 + ox] = on * 2 >= n.max(1);
        }
    }
    Art { w, h, px }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aa_words() {
        assert_eq!(
            Maneuver::from_aa("turn", Some("left"), None),
            Maneuver::Turn(Side::Left)
        );
        assert_eq!(
            Maneuver::from_aa("roundabout_enter_and_exit", Some("right"), Some(45)),
            Maneuver::Roundabout(Some(45))
        );
        assert_eq!(Maneuver::from_aa("warp", None, None), Maneuver::Unknown);
    }

    #[test]
    fn every_name_is_in_the_set() {
        let all = [
            Maneuver::Depart,
            Maneuver::Slight(Side::Left),
            Maneuver::Turn(Side::Right),
            Maneuver::Sharp(Side::Left),
            Maneuver::UTurn,
            Maneuver::RampOn(Side::Left),
            Maneuver::RampOff(Side::Right),
            Maneuver::Fork(Side::Left),
            Maneuver::Merge(None),
            Maneuver::Merge(Some(Side::Right)),
            Maneuver::RoundaboutEnter,
            Maneuver::RoundaboutExit,
            Maneuver::FerryBoat,
            Maneuver::FerryTrain,
            Maneuver::Destination(None),
            Maneuver::Destination(Some(Side::Left)),
        ];
        for lht in [false, true] {
            for m in all {
                let n = m.icon_name(lht);
                assert!(NAMES.contains(&n.as_str()), "{n}");
            }
            for a in (0..360).step_by(5) {
                let n = Maneuver::Roundabout(Some(a)).icon_name(lht);
                assert!(NAMES.contains(&n.as_str()), "{a}: {n}");
            }
        }
        assert_eq!(
            Maneuver::Roundabout(Some(90)).icon_name(false),
            "ROUNDABOUT_RIGHT_COUNTERCLOCKWISE"
        );
    }

    #[test]
    fn arrows_differ_and_have_ink() {
        let a = draw(Maneuver::Turn(Side::Left), false, 80, 72, 6, 0);
        let b = draw(Maneuver::Turn(Side::Right), false, 80, 72, 6, 0);
        assert_ne!(a, b);
        assert!(a.px.iter().filter(|p| **p).count() > 300);
    }
}
