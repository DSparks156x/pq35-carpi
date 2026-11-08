//! Native white-cluster glyph bitmaps, 12x14 px per code, from Drew's font
//! capture charts (`clusterRE/font_capture/native_reference_charts/`, the
//! data the old DIS emulator drew with). Converted once; `glyphs.bin` is the
//! fixed, proportional and graphics fonts in that order, 256 codes each, 14
//! rows each as u16 LE with pixel x at bit `0x8000 >> x`.
//!
//! - `NativeWhiteFixedFont.png` sha256 154666c3cf3818207966d142d588ff87020fb80f763116c35d9930ac1b3636f5
//! - `NativeWhiteProportionalFont.png` sha256 17a4b1a1d76a39a409ec99bd9f5792a489e18c45038078b316ab57fec5c9e3a2
//! - `NativeWhiteGraphicsFont.png` sha256 3830e1a0f516f463f4069891f59dab5169dcf8f211586ac2224f1a13c7fd5114
//! - `glyphs.bin` sha256 3dead0598a3b42264331eae9905f23451da616a8758593a03a67b22ae2425e7d

use crate::font::Font;

static BIN: &[u8; 3 * 256 * 14 * 2] = include_bytes!("glyphs.bin");

pub const W: usize = 12;
pub const H: usize = 14;

/// Row `y` (0..14) of `code` in `font`: bit `0x8000 >> x` is pixel x.
pub fn row(font: Font, code: u8, y: usize) -> u16 {
    let f = match font {
        Font::Fixed => 0,
        Font::Prop => 1,
        Font::Graphics => 2,
    };
    let i = ((f * 256 + usize::from(code)) * H + y) * 2;
    u16::from_le_bytes([BIN[i], BIN[i + 1]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capital_a_has_a_crossbar() {
        // Proportional 'A': rows 7 and 8 are solid across the glyph.
        assert_eq!(row(Font::Prop, b'A', 7) & 0xFF00, 0xFF00);
        assert_eq!(row(Font::Prop, b'A', 3), 0xC300);
    }
}
