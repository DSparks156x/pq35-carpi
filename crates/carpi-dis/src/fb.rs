//! What the driver believes is on the centre: a 1-bit 128x96 model plus a text
//! layer, fed the same messages the cluster gets. For goldens and tests only;
//! nothing in the driver reads pixels back.
//!
//! Model choices (all from `[dis]` §2 unless noted):
//! - `52` sets the clip; flag 0x02 clears, 0x03 fills. Bench.
//! - `57` glyphs advance by the measured widths; opaque ink paints the advance
//!   over a 14 px cell (native 12x14 cell). Glyph pixels aren't modelled: the
//!   text layer records the character at its left cell instead.
//! - `55` rows are 2x2 blocks per logical bit; mode sets the ROP latch. Bench.
//! - `63` V = left px column, H = top px row, dot = top-left px; XOR toggles,
//!   other modes set (set/draw guess). Bench planes model (`dc/native_bitmap.py`).

use crate::cmd::{Dir, Ink, Rop, records};
use crate::font::{BLANK_NARROW, BLANK_WIDE, Font, Metrics};
use crate::proto::{CENTRE_Y, FULL, Rect};

pub const PX_W: usize = 128;
pub const PX_H: usize = 96;
const CELL_PX_H: usize = 14;

#[derive(Clone)]
pub struct Fb {
    px: Vec<bool>,
    /// Logical-cell text layer: `Some(' ')` = text background, letters at glyph starts.
    text: Vec<Option<char>>,
    clip: Rect,
    rop: Option<Rop>,
    metrics: Metrics,
    /// Commits seen.
    pub commits: usize,
    /// Records the model didn't understand.
    pub errors: Vec<String>,
}

impl Fb {
    pub fn new(metrics: Metrics) -> Self {
        Fb {
            px: vec![false; PX_W * PX_H],
            text: vec![None; 64 * 48],
            clip: FULL,
            rop: None,
            metrics,
            commits: 0,
            errors: Vec::new(),
        }
    }

    pub fn pixel(&self, x: usize, y: usize) -> bool {
        self.px[y * PX_W + x]
    }

    pub fn apply(&mut self, msg: &[u8]) {
        let Some(recs) = records(msg) else {
            self.errors.push(format!("truncated {msg:02X?}"));
            return;
        };
        for r in recs {
            self.record(r);
        }
    }

    fn record(&mut self, r: &[u8]) {
        match r[0] {
            0x52 if r.len() == 7 => {
                let Some(y) = r[4].checked_sub(CENTRE_Y) else {
                    self.errors.push(format!("52 above centre {r:02X?}"));
                    return;
                };
                let rect = Rect::new(r[3], y, r[5], r[6]);
                self.clip = rect;
                match r[2] & 0x03 {
                    0x02 => self.paint(rect, false),
                    0x03 => self.paint(rect, true),
                    _ => {}
                }
            }
            0x57 if r.len() >= 5 => self.text_run(r[2], r[3], r[4], &r[5..]),
            0x55 if r.len() >= 5 => self.bitmap(Rop::from_mode(r[2]), r[4], &r[5..]),
            0x63 if r.len() == 6 => match Dir::from_byte(r[2]) {
                Some(d) => self.line(d, r[3], r[4], r[5]),
                None => self.errors.push(format!("63 orientation {r:02X?}")),
            },
            0x39 => self.commits += 1,
            _ => self.errors.push(format!("unknown {r:02X?}")),
        }
    }

    fn in_clip_px(&self, x: usize, y: usize) -> bool {
        let c = self.clip;
        x >= usize::from(c.x) * 2
            && x < c.right() as usize * 2
            && y >= usize::from(c.y) * 2
            && y < c.bottom() as usize * 2
            && x < PX_W
            && y < PX_H
    }

    fn paint(&mut self, r: Rect, on: bool) {
        for ly in r.y..(r.bottom().min(48) as u8) {
            for lx in r.x..(r.right().min(64) as u8) {
                self.text[usize::from(ly) * 64 + usize::from(lx)] = None;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (x, y) = (usize::from(lx) * 2 + dx, usize::from(ly) * 2 + dy);
                    self.px[y * PX_W + x] = on;
                }
            }
        }
    }

    fn text_run(&mut self, flags: u8, x: u8, y: u8, bytes: &[u8]) {
        let font = Font::from_flags(flags);
        let ink = Ink::from_flags(flags);
        let mut cx = (usize::from(self.clip.x) + usize::from(x)) * 2;
        let ly = usize::from(self.clip.y) + usize::from(y);
        for &b in bytes {
            let adv = usize::from(self.metrics.advance(font, b));
            if ink == Ink::Opaque {
                for py in ly * 2..ly * 2 + CELL_PX_H {
                    for px in cx..cx + adv {
                        if self.in_clip_px(px, py) {
                            self.px[py * PX_W + px] = false;
                            let i = (py / 2) * 64 + px / 2;
                            self.text[i] = Some(' ');
                        }
                    }
                }
            }
            let c = display_char(font, b);
            if (ink == Ink::Opaque || c != ' ') && self.in_clip_px(cx, ly * 2) {
                self.text[ly * 64 + cx / 2] = Some(c);
            }
            cx += adv;
        }
    }

    fn bitmap(&mut self, mode: Rop, y: u8, raster: &[u8]) {
        self.rop = Some(mode);
        let c = self.clip;
        let stride = usize::from(c.w).div_ceil(8);
        for (i, byte) in raster.iter().enumerate() {
            let row = i / stride;
            for bit in 0..8 {
                let col = (i % stride) * 8 + bit;
                if col >= usize::from(c.w) {
                    break;
                }
                let on = byte & (0x80 >> bit) != 0;
                let lx = usize::from(c.x) + col;
                let ly = usize::from(c.y) + usize::from(y) + row;
                if lx >= 64 || ly >= usize::from(c.bottom()) || ly >= 48 {
                    continue;
                }
                if mode == Rop::Draw || on {
                    self.text[ly * 64 + lx] = None;
                }
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let p = &mut self.px[(ly * 2 + dy) * PX_W + lx * 2 + dx];
                    *p = match mode {
                        Rop::Draw => on,
                        Rop::Xor => *p ^ on,
                        Rop::Set => *p || on,
                        Rop::Erase => *p && !on,
                    };
                }
            }
        }
    }

    fn line(&mut self, d: Dir, x: u8, y: u8, len: u8) {
        let bx = (usize::from(self.clip.x) + usize::from(x)) * 2;
        let by = (usize::from(self.clip.y) + usize::from(y)) * 2;
        let n = usize::from(len) * 2;
        let pts: Vec<(usize, usize)> = match d {
            Dir::V => (by..by + n).map(|py| (bx, py)).collect(),
            Dir::H => (bx..bx + n).map(|px| (px, by)).collect(),
            Dir::Dot => vec![(bx, by)],
        };
        let rop = self.rop.unwrap_or(Rop::Set);
        for (px, py) in pts {
            if !self.in_clip_px(px, py) {
                continue;
            }
            self.text[(py / 2) * 64 + px / 2] = None;
            let p = &mut self.px[py * PX_W + px];
            *p = match rop {
                Rop::Xor => !*p,
                Rop::Erase => false,
                Rop::Set | Rop::Draw => true,
            };
        }
    }

    /// Physical pixels of a logical rect, row-major.
    pub fn region(&self, r: Rect) -> Vec<bool> {
        let mut out = Vec::new();
        for py in usize::from(r.y) * 2..r.bottom() as usize * 2 {
            for px in usize::from(r.x) * 2..r.right() as usize * 2 {
                out.push(self.px[py * PX_W + px]);
            }
        }
        out
    }

    /// One char per logical cell: text layer first, else `#` all four pixels on,
    /// `.` all off, `+` mixed (sub-pixel detail or a 1 px line).
    pub fn ascii(&self) -> String {
        let mut s = String::with_capacity(65 * 48);
        for ly in 0..48 {
            for lx in 0..64 {
                if let Some(c) = self.text[ly * 64 + lx] {
                    s.push(c);
                    continue;
                }
                let n = [(0, 0), (1, 0), (0, 1), (1, 1)]
                    .iter()
                    .filter(|(dx, dy)| self.pixel(lx * 2 + dx, ly * 2 + dy))
                    .count();
                s.push(match n {
                    0 => '.',
                    4 => '#',
                    _ => '+',
                });
            }
            s.push('\n');
        }
        s
    }
}

/// Printable stand-in for an AUDSCII byte. Graphics-font glyphs show their low
/// hex digit in upper case, so bar levels stay visible.
pub fn display_char(font: Font, b: u8) -> char {
    if font == Font::Graphics {
        return char::from_digit(u32::from(b & 0x0F), 16)
            .unwrap_or('?')
            .to_ascii_uppercase();
    }
    match b {
        BLANK_NARROW | BLANK_WIDE => ' ',
        0x01..=0x10 => char::from(b'a' + b - 1),
        0x71..=0x7A => char::from(b'q' + b - 0x71),
        0x21..=0x5E => char::from(b),
        _ => '?',
    }
}
