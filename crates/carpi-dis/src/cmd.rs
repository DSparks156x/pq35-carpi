//! Typed cluster commands and their byte encoding, with the hard limits checked
//! at build time.
//!
//! Coordinates are centre-relative logical units. `52` is absolute on the wire
//! (we add [`CENTRE_Y`]); `57`, `55` and `63` are relative to the current clip
//! (`dc/dis_service.py:572-577`, bench). Relative ops carry the clip they were
//! written against; the packer emits that clip when the register differs.

use crate::proto::{
    CENTRE_Y, FULL, OP_BITMAP, OP_COMMIT, OP_LINE, OP_REGION, OP_TEXT, Profile, Rect,
};

/// `57` ink, flag bits 0x03. Bench (`[dis]` §2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ink {
    /// 00: glyph pixels black, background untouched.
    Black,
    /// 01.
    Xor,
    /// 10: paints the whole advance. Narrower text leaves the old tail
    /// (`dc/README.md:42-45`).
    Opaque,
    /// 11: glyph pixels only.
    Transparent,
}

impl Ink {
    pub fn bits(self) -> u8 {
        match self {
            Ink::Black => 0,
            Ink::Xor => 1,
            Ink::Opaque => 2,
            Ink::Transparent => 3,
        }
    }
    pub fn from_flags(f: u8) -> Ink {
        match f & 3 {
            0 => Ink::Black,
            1 => Ink::Xor,
            2 => Ink::Opaque,
            _ => Ink::Transparent,
        }
    }
}

/// `57` flags for a font and ink. 0x06 = proportional opaque (old `FLAG_ITEM`);
/// 0x0A = graphics opaque (stock bar cells). Bench.
pub fn text_flags(font: crate::font::Font, ink: Ink) -> u8 {
    font.bits() | ink.bits()
}

/// `55` mode, which `63` lines then inherit (the ROP latch). Bench
/// (`clusterRE/dis_findings.txt:588-596`). Which mode gives "set" lines is not
/// pinned down; only XOR priming is proven.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Rop {
    Erase = 0,
    Xor = 1,
    /// Opaque raster (1 = on, 0 = off). What every bitmap uses.
    Draw = 2,
    Set = 3,
}

impl Rop {
    pub fn from_mode(m: u8) -> Rop {
        match m & 3 {
            0 => Rop::Erase,
            1 => Rop::Xor,
            2 => Rop::Draw,
            _ => Rop::Set,
        }
    }
}

/// `63` orientation byte. Physically V1 = 1x2 px (left column of the cell),
/// H1 = 2x1 px (top row), dot = the top-left pixel. Bench
/// (`dc/native_bitmap.py` planes model, `[dis]` §2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Dir {
    V,
    H,
    Dot,
}

impl Dir {
    pub fn byte(self) -> u8 {
        match self {
            Dir::V => 0x10,
            Dir::H => 0x20,
            Dir::Dot => 0x00,
        }
    }
    pub fn from_byte(b: u8) -> Option<Dir> {
        match b {
            0x10 => Some(Dir::V),
            0x20 => Some(Dir::H),
            0x00 => Some(Dir::Dot),
            _ => None,
        }
    }
}

/// One drawing command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    /// `52 05 00`: set the clip.
    Clip(Rect),
    /// `52 05 02`: clear the rect. Also leaves the clip there.
    Clear(Rect),
    /// `52 05 03`: fill the rect (inverse background). Also leaves the clip there.
    Fill(Rect),
    /// `57 LEN FL X Y bytes`.
    Text {
        clip: Rect,
        x: u8,
        y: u8,
        flags: u8,
        bytes: Vec<u8>,
    },
    /// `63 04 OR X Y LEN` in the given ROP. The packer primes the latch.
    Line {
        clip: Rect,
        x: u8,
        y: u8,
        dir: Dir,
        len: u8,
        rop: Rop,
    },
    /// `55 LEN MODE 00 Y raster`. MSB first, rows of `ceil(clip.w/8)` bytes, X = 0.
    Bitmap {
        clip: Rect,
        mode: Rop,
        y: u8,
        raster: Vec<u8>,
    },
    /// Native sub-pixel XOR stroke: zero-byte `55 mode1` selector + `63`, always
    /// together. A selector before every stroke is required; priming once per
    /// message failed (`clusterRE/native_performance_checkpoint.txt:18-21`). Bench.
    Stroke {
        clip: Rect,
        x: u8,
        y: u8,
        dir: Dir,
        len: u8,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EncodeError {
    /// Rect empty or outside the centre.
    Rect(Rect),
    /// Anchor or extent outside the op's clip.
    OutsideClip,
    /// `57` with no glyphs or more than the profile allows (44).
    Glyphs(usize),
    /// `55` LEN >= 0x80 froze the white cluster (`clusterRE/dis_findings.txt:517-524`). Bench.
    BitmapLen(usize),
    /// Raster not whole rows, too many rows, or multi-row on a non-byte width.
    BitmapRows,
    /// `63` length 0 (a no-op) or too long.
    LineLen(u8),
}

fn check_rect(r: &Rect) -> Result<(), EncodeError> {
    if r.w == 0 || r.h == 0 || !FULL.contains(r) {
        return Err(EncodeError::Rect(*r));
    }
    Ok(())
}

/// Encode a `52` record.
pub fn region(flags: u8, r: &Rect) -> [u8; 7] {
    [OP_REGION, 0x05, flags, r.x, r.y + CENTRE_Y, r.w, r.h]
}

/// Zero-data `55` that only sets the ROP latch. XOR (mode 1) is proven as a
/// selector (`dc/native_bitmap.py:213, 284`, bench); other modes are a guess.
pub fn selector(rop: Rop) -> [u8; 6] {
    [OP_BITMAP, 0x04, rop as u8, 0x00, 0x00, 0x00]
}

pub const COMMIT: u8 = OP_COMMIT;

impl Op {
    /// Clip a relative op needs, or `None` for `52` ops.
    pub fn clip(&self) -> Option<Rect> {
        match self {
            Op::Clip(_) | Op::Clear(_) | Op::Fill(_) => None,
            Op::Text { clip, .. }
            | Op::Line { clip, .. }
            | Op::Bitmap { clip, .. }
            | Op::Stroke { clip, .. } => Some(*clip),
        }
    }

    /// Check the hard limits for this profile.
    pub fn check(&self, p: &Profile) -> Result<(), EncodeError> {
        if let Some(c) = self.clip() {
            check_rect(&c)?;
        }
        match self {
            Op::Clip(r) | Op::Clear(r) | Op::Fill(r) => check_rect(r),
            Op::Text {
                clip, x, y, bytes, ..
            } => {
                if bytes.is_empty() || bytes.len() > p.max_glyphs {
                    return Err(EncodeError::Glyphs(bytes.len()));
                }
                if *x >= clip.w || *y >= clip.h {
                    return Err(EncodeError::OutsideClip);
                }
                Ok(())
            }
            Op::Line {
                clip,
                x,
                y,
                dir,
                len,
                ..
            }
            | Op::Stroke {
                clip,
                x,
                y,
                dir,
                len,
            } => {
                // Logical runs up to 60 confirmed (`clusterRE/dis_findings.txt:104-106`);
                // native runs are capped at 15 by the old compiler.
                let max = if matches!(self, Op::Stroke { .. }) {
                    15
                } else {
                    60
                };
                if *len == 0 || *len > max || (*dir == Dir::Dot && *len != 1) {
                    return Err(EncodeError::LineLen(*len));
                }
                let (end_x, end_y) = match dir {
                    Dir::V => (u16::from(*x) + 1, u16::from(*y) + u16::from(*len)),
                    Dir::H => (u16::from(*x) + u16::from(*len), u16::from(*y) + 1),
                    Dir::Dot => (u16::from(*x) + 1, u16::from(*y) + 1),
                };
                if end_x > u16::from(clip.w) || end_y > u16::from(clip.h) {
                    return Err(EncodeError::OutsideClip);
                }
                Ok(())
            }
            Op::Bitmap {
                clip, y, raster, ..
            } => {
                let stride = usize::from(clip.w).div_ceil(8);
                if raster.is_empty() || raster.len() % stride != 0 {
                    return Err(EncodeError::BitmapRows);
                }
                let rows = raster.len() / stride;
                if rows > p.max_bitmap_rows || (rows > 1 && clip.w % 8 != 0) {
                    return Err(EncodeError::BitmapRows);
                }
                if raster.len() + 3 >= 0x80 {
                    return Err(EncodeError::BitmapLen(raster.len() + 3));
                }
                if usize::from(*y) + rows > usize::from(clip.h) {
                    return Err(EncodeError::OutsideClip);
                }
                Ok(())
            }
        }
    }

    /// Bytes for this op alone (no clip or latch priming; the packer adds those).
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Op::Clip(r) => region(0x00, r).to_vec(),
            Op::Clear(r) => region(0x02, r).to_vec(),
            Op::Fill(r) => region(0x03, r).to_vec(),
            Op::Text {
                x, y, flags, bytes, ..
            } => {
                let mut v = vec![OP_TEXT, bytes.len() as u8 + 3, *flags, *x, *y];
                v.extend_from_slice(bytes);
                v
            }
            Op::Line { x, y, dir, len, .. } => vec![OP_LINE, 0x04, dir.byte(), *x, *y, *len],
            Op::Bitmap {
                mode, y, raster, ..
            } => {
                let mut v = vec![OP_BITMAP, raster.len() as u8 + 3, *mode as u8, 0x00, *y];
                v.extend_from_slice(raster);
                v
            }
            Op::Stroke { x, y, dir, len, .. } => {
                let mut v = selector(Rop::Xor).to_vec();
                v.extend_from_slice(&[OP_LINE, 0x04, dir.byte(), *x, *y, *len]);
                v
            }
        }
    }
}

/// Split a message into records. `52`/`55`/`57`/`63` carry LEN at byte 1;
/// everything else we send or expect inside a drawing message is one byte.
/// Returns `None` on a truncated record.
pub fn records(msg: &[u8]) -> Option<Vec<&[u8]>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < msg.len() {
        let n = match msg[at] {
            OP_REGION | OP_BITMAP | OP_TEXT | OP_LINE => usize::from(*msg.get(at + 1)?) + 2,
            _ => 1,
        };
        if at + n > msg.len() {
            return None;
        }
        out.push(&msg[at..at + n]);
        at += n;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::Font;

    #[test]
    fn encodings() {
        assert_eq!(
            Op::Clear(FULL).encode(),
            vec![0x52, 0x05, 0x02, 0x00, 0x1B, 0x40, 0x30]
        );
        let t = Op::Text {
            clip: FULL,
            x: 57,
            y: 3,
            flags: text_flags(Font::Graphics, Ink::Opaque),
            bytes: vec![0x6F],
        };
        // Stock bar cell (`dc/stock_mono_catalog.json` "bar").
        assert_eq!(t.encode(), vec![0x57, 0x04, 0x0A, 0x39, 0x03, 0x6F]);
        assert_eq!(text_flags(Font::Prop, Ink::Opaque), 0x06);
        let s = Op::Stroke {
            clip: FULL,
            x: 1,
            y: 2,
            dir: Dir::V,
            len: 3,
        };
        assert_eq!(s.encode().len(), 12);
    }

    #[test]
    fn limits() {
        let p = Profile::WHITE;
        let t = |n| Op::Text {
            clip: FULL,
            x: 0,
            y: 0,
            flags: 6,
            bytes: vec![0x30; n],
        };
        assert!(t(44).check(&p).is_ok());
        assert_eq!(t(45).check(&p), Err(EncodeError::Glyphs(45)));
        let wide = Rect::new(0, 0, 64, 48);
        let b = |rows: usize| Op::Bitmap {
            clip: wide,
            mode: Rop::Draw,
            y: 0,
            raster: vec![0; rows * 8],
        };
        assert!(b(12).check(&p).is_ok());
        assert_eq!(b(13).check(&p), Err(EncodeError::BitmapRows));
        let tall = Rect::new(0, 0, 64, 48);
        let p20 = Profile {
            max_bitmap_rows: 20,
            ..p
        };
        let big = Op::Bitmap {
            clip: tall,
            mode: Rop::Draw,
            y: 0,
            raster: vec![0; 16 * 8],
        };
        assert_eq!(big.check(&p20), Err(EncodeError::BitmapLen(131)));
    }
}
