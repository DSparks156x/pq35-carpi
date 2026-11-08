//! Maneuver icons, drawn in code at native resolution (physical px).
//!
//! Our own art, not the old Google-derived set. The fast base is the bottom-right
//! pixel of each 2x2 cell, because the proven native refine (base + XOR V/H/dot
//! planes) can't change that pixel (`dc/native_bitmap.py:353-360`, bench).
//! Stock ROM-glyph arrows (`dc/stock_mono_catalog.json`) are a later option: the
//! catalog has no maneuver names yet.

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

    /// Logical base: bottom-right pixel of each cell, MSB-first rows.
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

/// What we can draw. Mapped from AA's maneuver vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Maneuver {
    Straight,
    Slight(Side),
    Turn(Side),
    Sharp(Side),
    UTurn(Side),
    /// Exit direction in degrees clockwise from straight ahead.
    Roundabout(i32),
    Destination,
    Unknown,
}

impl Maneuver {
    /// From `nav.maneuver` / `side` / `roundabout_angle` as carpi-core publishes
    /// them (`carpi-proto` `NavState`). Unknown words map to `Unknown`.
    pub fn from_aa(maneuver: &str, side: Option<&str>, angle: Option<i32>) -> Maneuver {
        let s = if side == Some("left") {
            Side::Left
        } else {
            Side::Right
        };
        let sign = if s == Side::Left { -1 } else { 1 };
        match maneuver {
            "depart" | "straight" | "continue" | "name_change" => Maneuver::Straight,
            "slight_turn" | "on_ramp" | "off_ramp" | "fork" | "merge" | "keep" => {
                Maneuver::Slight(s)
            }
            "turn" => Maneuver::Turn(s),
            "sharp_turn" => Maneuver::Sharp(s),
            "u_turn" => Maneuver::UTurn(s),
            "roundabout_enter_and_exit" | "roundabout_exit" | "roundabout_enter" => {
                Maneuver::Roundabout(angle.unwrap_or(90 * sign))
            }
            "destination" => Maneuver::Destination,
            _ => Maneuver::Unknown,
        }
    }
}

type P = (f32, f32);

fn seg_dist(p: P, a: P, b: P) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let l2 = dx * dx + dy * dy;
    let t = if l2 == 0.0 {
        0.0
    } else {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / l2).clamp(0.0, 1.0)
    };
    let (qx, qy) = (a.0 + t * dx - p.0, a.1 + t * dy - p.1);
    (qx * qx + qy * qy).sqrt()
}

fn in_tri(p: P, a: P, b: P, c: P) -> bool {
    let s = |p1: P, p2: P, p3: P| (p1.0 - p3.0) * (p2.1 - p3.1) - (p2.0 - p3.0) * (p1.1 - p3.1);
    let (d1, d2, d3) = (s(p, a, b), s(p, b, c), s(p, c, a));
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

struct Shape {
    strokes: Vec<(P, P)>,
    heads: Vec<(P, P)>, // (tip, unit direction)
    ring: Option<(P, f32)>,
    dot: Option<(P, f32)>,
}

fn dir(deg: f32) -> P {
    let r = deg.to_radians();
    (r.sin(), -r.cos())
}

/// Draw `m` into a `w` x `h` px box. Unit = box width.
pub fn draw(m: Maneuver, w: usize, h: usize) -> Art {
    let u = w as f32;
    let cx = u * 0.5;
    let bottom = h as f32 - 2.0;
    let joint = (cx, h as f32 * 0.55);
    let stem = (joint, (cx, bottom));
    let mut s = Shape {
        strokes: vec![],
        heads: vec![],
        ring: None,
        dot: None,
    };
    let turn = |s: &mut Shape, deg: f32, len: f32| {
        let d = dir(deg);
        let tip = (joint.0 + d.0 * len, joint.1 + d.1 * len);
        s.strokes.push(stem);
        s.strokes.push((joint, tip));
        s.heads.push((tip, d));
    };
    let side = |sd: Side| if sd == Side::Left { -1.0 } else { 1.0 };
    match m {
        Maneuver::Straight | Maneuver::Unknown => {
            let tip = (cx, h as f32 * 0.08);
            s.strokes.push(((cx, bottom), tip));
            s.heads.push((tip, (0.0, -1.0)));
            if m == Maneuver::Unknown {
                s.dot = Some(((cx, h as f32 * 0.55), u * 0.12));
            }
        }
        Maneuver::Slight(sd) => turn(&mut s, 35.0 * side(sd), u * 0.42),
        Maneuver::Turn(sd) => turn(&mut s, 90.0 * side(sd), u * 0.30),
        Maneuver::Sharp(sd) => turn(&mut s, 140.0 * side(sd), u * 0.34),
        Maneuver::UTurn(sd) => {
            let k = side(sd);
            let top = h as f32 * 0.22;
            let r = u * 0.18;
            let a = (cx - k * r, bottom);
            let b = (cx - k * r, top + r);
            s.strokes.push((a, b));
            let mut prev = b;
            for i in 1..=8 {
                let t = std::f32::consts::PI * i as f32 / 8.0;
                let p = (cx - k * r * t.cos(), top + r - r * t.sin());
                s.strokes.push((prev, p));
                prev = p;
            }
            let tip = (cx + k * r, h as f32 * 0.62);
            s.strokes.push((prev, tip));
            s.heads.push((tip, (0.0, 1.0)));
        }
        Maneuver::Roundabout(deg) => {
            let c = (cx, h as f32 * 0.48);
            let r = u * 0.2;
            s.ring = Some((c, r));
            s.strokes.push(((cx, c.1 + r), (cx, bottom)));
            let d = dir(deg as f32);
            let from = (c.0 + d.0 * r, c.1 + d.1 * r);
            let tip = (c.0 + d.0 * u * 0.44, c.1 + d.1 * u * 0.44);
            s.strokes.push((from, tip));
            s.heads.push((tip, d));
        }
        Maneuver::Destination => {
            let c = (cx, h as f32 * 0.35);
            s.ring = Some((c, u * 0.24));
            s.dot = Some((c, u * 0.09));
            s.strokes.push(((cx, c.1 + u * 0.24), (cx, bottom)));
        }
    }
    let half = u * 0.075;
    let head_len = u * 0.24;
    let head_half = u * 0.21;
    let mut px = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let p = (x as f32 + 0.5, y as f32 + 0.5);
            let mut on = s.strokes.iter().any(|(a, b)| seg_dist(p, *a, *b) <= half);
            for (tip, d) in &s.heads {
                let base = (tip.0 - d.0 * head_len, tip.1 - d.1 * head_len);
                let n = (-d.1, d.0);
                let l = (base.0 + n.0 * head_half, base.1 + n.1 * head_half);
                let r = (base.0 - n.0 * head_half, base.1 - n.1 * head_half);
                let tip2 = (tip.0 + d.0 * 2.0, tip.1 + d.1 * 2.0);
                on |= in_tri(p, tip2, l, r);
            }
            if let Some((c, r)) = s.ring {
                let dd = ((p.0 - c.0).powi(2) + (p.1 - c.1).powi(2)).sqrt();
                on |= (dd - r).abs() <= half;
            }
            if let Some((c, r)) = s.dot {
                on |= ((p.0 - c.0).powi(2) + (p.1 - c.1).powi(2)).sqrt() <= r;
            }
            px[y * w + x] = on;
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
            Maneuver::Roundabout(45)
        );
        assert_eq!(Maneuver::from_aa("ferry", None, None), Maneuver::Unknown);
    }

    #[test]
    fn arrows_differ_and_have_ink() {
        let a = draw(Maneuver::Turn(Side::Left), 64, 76);
        let b = draw(Maneuver::Turn(Side::Right), 64, 76);
        assert_ne!(a, b);
        assert!(a.px.iter().filter(|p| **p).count() > 300);
    }
}
