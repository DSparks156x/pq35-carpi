//! Colour images to 1-bit art for the cluster.
//!
//! [`cover`] runs the stages of the old pipeline Drew tuned in
//! `tools/dis_image_tester` (`dc/dis_image.py` `process_image`, its "legacy"
//! defaults): fit and letterbox on black, saturation-aware grey, 1 % autocontrast,
//! gamma, brightness, contrast about the mean, unsharp mask, black floor, then
//! Floyd-Steinberg or Atkinson-pattern dither. Parameters are [`CoverParams`];
//! iterate on them with `cargo run -p carpi-dis --example covers`.
//!
//! Bench 2026-11-08: Drew compared the old preset with a lower-gamma variant
//! plus auto-invert and an edge boost on 41 covers, at 48 px and 128x96. The
//! old preset looked better on nearly all of them; the extras were dropped.
//!
//! On the cluster a set pixel is lit, so light source pixels come out set.

use crate::icons::Art;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Dither {
    FloydSteinberg,
    /// Atkinson's 6-neighbour pattern; `diffusion` is the share of the error
    /// passed on (0.75 classic, 0.85 the old default, 1.0 FS-like).
    Atkinson { diffusion: f32 },
    Threshold,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CoverParams {
    pub contrast: f32,
    pub sharpen: f32,
    pub gamma: f32,
    pub brightness: f32,
    /// Grey below this goes black before dithering.
    pub black_floor: u8,
    pub dither: Dither,
    /// Share of the max-channel excess added to luma (0 = plain Rec.709).
    pub saturation_boost: f32,
}

impl Default for CoverParams {
    /// The old "legacy" preset (`dc/dis_image.py` `NATIVE_IMAGE_PRESETS`).
    fn default() -> Self {
        CoverParams {
            contrast: 1.4,
            sharpen: 1.5,
            gamma: 2.2,
            brightness: 1.0,
            black_floor: 45,
            dither: Dither::FloydSteinberg,
            saturation_boost: 0.25,
        }
    }
}

/// A grey image, 0..=255 as f32 while it's being worked on.
#[derive(Clone, Debug)]
pub struct Grey {
    pub w: usize,
    pub h: usize,
    pub v: Vec<f32>,
}

/// Area-average `rgb` (`w` x `h`, 3 B/px) into `ow` x `oh`, keeping aspect and
/// letterboxing on black. Returns RGB as f32.
fn fit_rgb(rgb: &[u8], w: usize, h: usize, ow: usize, oh: usize) -> Vec<[f32; 3]> {
    assert_eq!(rgb.len(), w * h * 3, "image size");
    let scale = (ow as f64 / w as f64).min(oh as f64 / h as f64);
    let (fw, fh) = (
        ((w as f64 * scale).round() as usize).clamp(1, ow),
        ((h as f64 * scale).round() as usize).clamp(1, oh),
    );
    let (ox, oy) = ((ow - fw) / 2, (oh - fh) / 2);
    let mut out = vec![[0f32; 3]; ow * oh];
    for ty in 0..fh {
        let (y0, y1) = (ty * h / fh, ((ty + 1) * h / fh).max(ty * h / fh + 1).min(h));
        for tx in 0..fw {
            let (x0, x1) = (tx * w / fw, ((tx + 1) * w / fw).max(tx * w / fw + 1).min(w));
            let mut s = [0f64; 3];
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = (y * w + x) * 3;
                    for c in 0..3 {
                        s[c] += f64::from(rgb[i + c]);
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as f64;
            out[(oy + ty) * ow + ox + tx] = s.map(|c| (c / n) as f32);
        }
    }
    out
}

/// Rec.709 luma plus a share of how far the brightest channel sits above it,
/// so saturated colours (red text on black) don't vanish.
fn smart_grey(px: &[[f32; 3]], boost: f32) -> Vec<f32> {
    px.iter()
        .map(|[r, g, b]| {
            let l = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            let m = r.max(*g).max(*b);
            (l + (m - l) * boost).min(255.0)
        })
        .collect()
}

/// Stretch so 1 % of pixels clip at each end.
fn autocontrast(v: &mut [f32]) {
    let mut s: Vec<f32> = v.to_vec();
    s.sort_by(f32::total_cmp);
    let cut = s.len() / 100;
    let (lo, hi) = (s[cut], s[s.len() - 1 - cut]);
    if hi - lo < 1.0 {
        return;
    }
    for p in v.iter_mut() {
        *p = ((*p - lo) * 255.0 / (hi - lo)).clamp(0.0, 255.0);
    }
}

/// Unsharp mask, Gaussian radius 1 (3x3 binomial), PIL-style threshold 3.
fn unsharp(g: &mut Grey, amount: f32) {
    let (w, h) = (g.w, g.h);
    let k = [1.0, 2.0, 1.0];
    let at = |v: &[f32], x: isize, y: isize| {
        let x = x.clamp(0, w as isize - 1) as usize;
        let y = y.clamp(0, h as isize - 1) as usize;
        v[y * w + x]
    };
    let mut blur = vec![0f32; w * h];
    for y in 0..h as isize {
        for x in 0..w as isize {
            let mut s = 0.0;
            for (j, kj) in k.iter().enumerate() {
                for (i, ki) in k.iter().enumerate() {
                    s += kj * ki * at(&g.v, x + i as isize - 1, y + j as isize - 1);
                }
            }
            blur[y as usize * w + x as usize] = s / 16.0;
        }
    }
    for (p, b) in g.v.iter_mut().zip(blur) {
        let d = *p - b;
        if d.abs() >= 3.0 {
            *p = (*p + d * amount).clamp(0.0, 255.0);
        }
    }
}

/// The tone stages, before the dither: what the old tester previewed as grey.
pub fn cover_grey(rgb: &[u8], w: usize, h: usize, ow: usize, oh: usize, p: &CoverParams) -> Grey {
    let px = fit_rgb(rgb, w, h, ow, oh);
    let mut v = smart_grey(&px, p.saturation_boost);
    autocontrast(&mut v);
    for x in v.iter_mut() {
        *x = (*x / 255.0).powf(p.gamma) * 255.0 * p.brightness;
    }
    let mean = v.iter().sum::<f32>() / v.len() as f32;
    for x in v.iter_mut() {
        *x = (mean + (*x - mean) * p.contrast).clamp(0.0, 255.0);
    }
    let mut g = Grey { w: ow, h: oh, v };
    if p.sharpen > 0.0 {
        unsharp(&mut g, p.sharpen);
    }
    for x in g.v.iter_mut() {
        if *x < f32::from(p.black_floor) {
            *x = 0.0;
        }
    }
    g
}

/// 1-bit, one pixel per output pixel.
pub fn dither_grey(g: &Grey, d: Dither) -> Art {
    let (w, h) = (g.w, g.h);
    let mut v = g.v.clone();
    let mut px = vec![false; w * h];
    let add = |v: &mut [f32], x: usize, y: usize, dx: isize, dy: usize, e: f32| {
        let nx = x as isize + dx;
        if nx >= 0 && (nx as usize) < w && y + dy < h {
            v[(y + dy) * w + nx as usize] += e;
        }
    };
    for y in 0..h {
        for x in 0..w {
            let old = v[y * w + x];
            let on = old > 127.0;
            px[y * w + x] = on;
            let err = old - if on { 255.0 } else { 0.0 };
            match d {
                Dither::Threshold => {}
                Dither::FloydSteinberg => {
                    add(&mut v, x, y, 1, 0, err * 7.0 / 16.0);
                    add(&mut v, x, y, -1, 1, err * 3.0 / 16.0);
                    add(&mut v, x, y, 0, 1, err * 5.0 / 16.0);
                    add(&mut v, x, y, 1, 1, err / 16.0);
                }
                Dither::Atkinson { diffusion } => {
                    let e = err * diffusion / 6.0;
                    for (dx, dy) in [(1, 0), (2, 0), (-1, 1), (0, 1), (1, 1), (0, 2)] {
                        add(&mut v, x, y, dx, dy, e);
                    }
                }
            }
        }
    }
    Art { w, h, px }
}

/// The whole pipeline: `rgb` (`w` x `h`, 3 B/px) to `ow` x `oh` 1-bit art.
pub fn cover(rgb: &[u8], w: usize, h: usize, ow: usize, oh: usize, p: &CoverParams) -> Art {
    dither_grey(&cover_grey(rgb, w, h, ow, oh, p), p.dither)
}

/// Plain grey to 1-bit: box downscale and Atkinson, no tone work. For
/// synthetic test patterns.
pub fn dither(gray: &[u8], w: usize, h: usize, ow: usize, oh: usize) -> Art {
    let rgb: Vec<u8> = gray.iter().flat_map(|g| [*g; 3]).collect();
    let px = fit_rgb(&rgb, w, h, ow, oh);
    let g = Grey {
        w: ow,
        h: oh,
        v: px.iter().map(|p| p[0]).collect(),
    };
    dither_grey(&g, Dither::Atkinson { diffusion: 0.75 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_and_mid_grey() {
        assert!(dither(&[255; 16], 4, 4, 4, 4).px.iter().all(|p| *p));
        assert!(dither(&[0; 16], 4, 4, 4, 4).px.iter().all(|p| !*p));
        let a = dither(&[128; 64 * 64], 64, 64, 32, 32);
        let lit = a.px.iter().filter(|p| **p).count();
        assert!((300..=700).contains(&lit), "{lit}");
    }

    #[test]
    fn cover_letterboxes_and_keeps_bright_shapes() {
        // 40x20 image: white square on black, left half.
        let (w, h) = (40, 20);
        let mut rgb = vec![0u8; w * h * 3];
        for y in 5..15 {
            for x in 5..15 {
                rgb[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&[255, 255, 255]);
            }
        }
        let a = cover(&rgb, w, h, 48, 48, &CoverParams::default());
        // Letterbox rows (top 12) stay dark; the square is lit.
        assert!(a.px[..48 * 10].iter().all(|p| !*p));
        assert!(a.get(10 * 48 / 40 + 2, 24));
    }
}
