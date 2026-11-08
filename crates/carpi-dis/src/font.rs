//! Text metrics and AUDSCII.
//!
//! Advances are measured per glyph in physical px (white: 128x96 grid) and come
//! from the old driver's capture (`dc/font_metrics_data.json`, method in
//! `clusterRE/nav_sweep_checkpoint.txt:86-91`). Bench. They are a Rust table
//! (`font_data.rs`), not loaded at build time: no serde/build script, no path into
//! the sibling repo, and the data is frozen with its hash in the file header.
//!
//! All advances are even, so every footprint is a whole number of logical units.

use crate::font_data as d;
use crate::proto::Variant;

/// Cluster font, as the `57` flag bits 0x0C select. Bench (`[dis]` §2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Font {
    Fixed,
    Prop,
    Graphics,
}

impl Font {
    pub fn bits(self) -> u8 {
        match self {
            Font::Fixed => 0x00,
            Font::Prop => 0x04,
            Font::Graphics => 0x08,
        }
    }
    pub fn from_flags(flags: u8) -> Font {
        match flags & 0x0C {
            0x04 => Font::Prop,
            0x08 | 0x0C => Font::Graphics,
            _ => Font::Fixed,
        }
    }
}

/// Glyph cell height in logical rows used for footprints and clears. The native
/// cell is 12x14 px (`NativeWhiteFontCharts.json`); the old code cleared 9 rows
/// per text line (`dc/text_render.py:24-33`). We use 9: the row pitch, guess.
pub const TEXT_H: u8 = 9;

/// 4 px proportional space. Bench.
pub const BLANK_NARROW: u8 = 0x20;
/// 12 px blank the stock RNS-E pads with (`ref/pcbbc.txt:123-126`), 12 px in every
/// font. Also what `0x1F` (marquee gap) encodes to. Bench.
pub const BLANK_WIDE: u8 = 0x65;

#[derive(Clone, Copy, Debug)]
pub struct Metrics {
    fixed: &'static [u8; 256],
    prop: &'static [u8; 256],
    gfx: &'static [u8; 256],
}

impl Metrics {
    /// White: native capture. Red: the legacy reference table, already scaled to
    /// the same physical units by the old tool. Red widths are unproven.
    pub fn for_variant(v: Variant) -> Metrics {
        match v {
            Variant::White => Metrics {
                fixed: &d::NATIVE_FIXED_ADV,
                prop: &d::NATIVE_PROP_ADV,
                gfx: &d::NATIVE_GFX_ADV,
            },
            Variant::Red => Metrics {
                fixed: &d::LEGACY_FIXED_ADV,
                prop: &d::LEGACY_PROP_ADV,
                gfx: &d::LEGACY_GFX_ADV,
            },
        }
    }

    /// Advance of one AUDSCII byte, physical px.
    pub fn advance(&self, font: Font, b: u8) -> u16 {
        let t = match font {
            Font::Fixed => self.fixed,
            Font::Prop => self.prop,
            Font::Graphics => self.gfx,
        };
        u16::from(t[usize::from(b)])
    }

    /// Width of a run, physical px.
    pub fn width(&self, font: Font, bytes: &[u8]) -> u16 {
        bytes.iter().map(|b| self.advance(font, *b)).sum()
    }

    /// Longest prefix that fits in `max_px`.
    pub fn fit(&self, font: Font, bytes: &[u8], max_px: u16) -> usize {
        let mut w = 0;
        for (i, b) in bytes.iter().enumerate() {
            w += self.advance(font, *b);
            if w > max_px {
                return i;
            }
        }
        bytes.len()
    }

    /// Blanks that fill exactly `px` in `font`, widest first; `None` if no mix of
    /// the known blanks hits it. Proportional blanks are 4 and 12 px, so `px` must
    /// be a multiple of 4; fixed and graphics have only 12.
    pub fn blanks(&self, font: Font, px: u16) -> Option<Vec<u8>> {
        let wide = self.advance(font, BLANK_WIDE);
        let narrow = match font {
            Font::Prop => self.advance(font, BLANK_NARROW),
            _ => wide,
        };
        if narrow == 0 || wide == 0 {
            return None;
        }
        let mut out = vec![BLANK_WIDE; usize::from(px / wide)];
        let rest = px % wide;
        if !rest.is_multiple_of(narrow) {
            return None;
        }
        out.extend(std::iter::repeat_n(
            BLANK_NARROW,
            usize::from(rest / narrow),
        ));
        Some(out)
    }

    /// Blanks covering as much of `px` as possible without going over.
    pub fn fill_blanks(&self, font: Font, px: u16) -> Vec<u8> {
        (0..=px)
            .rev()
            .find_map(|p| self.blanks(font, p))
            .unwrap_or_default()
    }
}

/// Encode text to AUDSCII, one byte per char. Unknown chars become a space.
/// Table from `dc/icons.py:11-69`: a-p map to 0x01-0x10, q-z to 0x71-0x7A, plus
/// captured Unicode aliases. Bench.
pub fn audscii(text: &str) -> Vec<u8> {
    text.chars().map(audscii_char).collect()
}

pub fn audscii_char(c: char) -> u8 {
    if let Some((_, b)) = d::AUDSCII_UNICODE.iter().find(|(u, _)| *u == c) {
        return *b;
    }
    let cp = u32::from(c);
    if cp < 256 {
        d::AUDSCII_LATIN1[cp as usize]
    } else {
        d::AUDSCII_LATIN1[0x20]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_widths() {
        let m = Metrics::for_variant(Variant::White);
        for b in b'0'..=b'9' {
            assert_eq!(m.advance(Font::Prop, b), 10);
            assert_eq!(m.advance(Font::Fixed, b), 12);
        }
        assert_eq!(m.advance(Font::Prop, BLANK_NARROW), 4);
        assert_eq!(m.advance(Font::Prop, BLANK_WIDE), 12);
        assert_eq!(audscii("az"), vec![0x01, 0x7A]);
        assert_eq!(audscii("\u{1F}"), vec![BLANK_WIDE]);
    }

    #[test]
    fn blanks_fill_exactly() {
        let m = Metrics::for_variant(Variant::White);
        assert_eq!(m.blanks(Font::Prop, 20), Some(vec![0x65, 0x20, 0x20]));
        assert_eq!(m.blanks(Font::Prop, 10), None);
        assert_eq!(m.blanks(Font::Fixed, 24), Some(vec![0x65, 0x65]));
        assert_eq!(m.blanks(Font::Prop, 0), Some(vec![]));
    }
}
