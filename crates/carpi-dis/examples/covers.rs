//! Cover-art previews for iterating on [`carpi_dis::image::CoverParams`].
//!
//! `cargo run -p carpi-dis --example covers -- <covers dir> <out dir> [--contrast 1.4]
//!  [--sharpen 1.5] [--gamma 2.2] [--brightness 1] [--floor 45] [--boost 0.25]
//!  [--dither fs|atkinson:0.85|threshold]`
//!
//! Writes `sheet.png` (per cover: source, the grey stage, the 48x48 thumbnail
//! and the 96x96 full cover, drawn like the white cluster: lit px on black,
//! 4x) and `<name>.thumb.bin` / `<name>.full.bin` (1-bit,
//! MSB-first rows) for `carpi dis-bench` (`CARPI_BENCH_COVERS=<out dir>`), plus
//! `<name>.thumb.png` / `<name>.centre.png` tiles (48x48 and the full 128x96
//! centre) for comparisons.

use carpi_dis::icons::Art;
use carpi_dis::image::{CoverParams, Dither, cover, cover_grey, dither_grey};
use image::{Rgb, RgbImage, imageops::FilterType};
use std::path::{Path, PathBuf};

const S: u32 = 4;

fn params(args: &[String]) -> CoverParams {
    let mut p = CoverParams::default();
    let mut it = args.iter();
    while let Some(k) = it.next() {
        let v = it.next().expect("flag value");
        let f = || v.parse::<f32>().expect("number");
        match k.as_str() {
            "--contrast" => p.contrast = f(),
            "--sharpen" => p.sharpen = f(),
            "--gamma" => p.gamma = f(),
            "--brightness" => p.brightness = f(),
            "--floor" => p.black_floor = v.parse().expect("0..255"),
            "--boost" => p.saturation_boost = f(),
            "--dither" => {
                p.dither = match v.split_once(':') {
                    Some(("atkinson", d)) => Dither::Atkinson { diffusion: d.parse().expect("diffusion") },
                    None if v == "atkinson" => Dither::Atkinson { diffusion: 0.85 },
                    None if v == "threshold" => Dither::Threshold,
                    _ => Dither::FloydSteinberg,
                }
            }
            _ => panic!("unknown flag {k}"),
        }
    }
    p
}

fn bits(a: &Art) -> Vec<u8> {
    let mut out = Vec::new();
    for y in 0..a.h {
        let mut row = vec![0u8; a.w.div_ceil(8)];
        for x in 0..a.w {
            if a.get(x, y) {
                row[x / 8] |= 0x80 >> (x % 8);
            }
        }
        out.extend(row);
    }
    out
}

/// Draw `a` like the cluster at (x0, y0): each px an SxS lit or dark square, no gaps.
fn put_art(sheet: &mut RgbImage, a: &Art, x0: u32, y0: u32) {
    for y in 0..a.h as u32 {
        for x in 0..a.w as u32 {
            let c = if a.get(x as usize, y as usize) { Rgb([235, 240, 255]) } else { Rgb([0, 0, 0]) };
            for dy in 0..S {
                for dx in 0..S {
                    sheet.put_pixel(x0 + x * S + dx, y0 + y * S + dy, c);
                }
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    assert!(args.len() >= 2, "usage: covers <dir> <out> [flags]");
    let (dir, out) = (PathBuf::from(&args[0]), PathBuf::from(&args[1]));
    let p = params(&args[2..]);
    std::fs::create_dir_all(&out).expect("out dir");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("covers dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|f| {
            matches!(
                f.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(),
                Some("png" | "jpg" | "jpeg")
            )
        })
        .collect();
    files.sort();
    let cell = 96 * S + 8;
    let mut sheet = RgbImage::from_pixel(cell * 4, cell * files.len() as u32, Rgb([40, 40, 48]));
    for (i, f) in files.iter().enumerate() {
        let img = match image::open(f) {
            Ok(i) => i.to_rgb8(),
            Err(e) => {
                eprintln!("skip {}: {e}", f.display());
                continue;
            }
        };
        let (w, h) = (img.width() as usize, img.height() as usize);
        let raw = img.as_raw();
        let y0 = i as u32 * cell + 4;
        let src = image::imageops::resize(&img, 96 * S, 96 * S, FilterType::Triangle);
        image::imageops::overlay(&mut sheet, &src, 4, i64::from(y0));
        let g = cover_grey(raw, w, h, 96, 96, &p);
        for y in 0..96u32 {
            for x in 0..96u32 {
                let v = g.v[(y * 96 + x) as usize] as u8;
                for d in 0..S * S {
                    sheet.put_pixel(cell + 4 + x * S + d % S, y0 + y * S + d / S, Rgb([v, v, v]));
                }
            }
        }
        let thumb = cover(raw, w, h, 48, 48, &p);
        put_art(&mut sheet, &thumb, cell * 2 + 4, y0);
        let full = dither_grey(&g, p.dither);
        put_art(&mut sheet, &full, cell * 3 + 4, y0);
        let stem = f.file_stem().and_then(|s| s.to_str()).unwrap_or("cover");
        // Tiles for side-by-side comparisons: thumbnail and the full 128x96 centre.
        let centre = cover(raw, w, h, 128, 96, &p);
        for (kind, a) in [("thumb", &thumb), ("centre", &centre)] {
            let mut t = RgbImage::from_pixel(a.w as u32 * S, a.h as u32 * S, Rgb([0, 0, 0]));
            put_art(&mut t, a, 0, 0);
            t.save(out.join(format!("{stem}.{kind}.png"))).expect("tile");
        }
        let write = |kind: &str, a: &Art| std::fs::write(out.join(format!("{stem}.{kind}.bin")), bits(a)).expect("write");
        write("thumb", &thumb);
        write("full", &full);
    }
    let path: &Path = &out.join("sheet.png");
    sheet.save(path).expect("sheet");
    println!("{} covers, params {p:?}\n{}", files.len(), path.display());
}
