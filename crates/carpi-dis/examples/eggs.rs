//! GIF to `.egg` (see [`carpi_dis::screens::egg`]): every frame through the
//! cover pipeline at 64x48, with the old egg defaults (`dc/apps/easteregg.py`:
//! Atkinson 0.85, contrast 1.4, sharpen 1.5) unless flags say otherwise.
//!
//! `cargo run -p carpi-dis --example eggs -- <in.gif> <out.egg> [--contrast 1.5]
//!  [--gamma 1.2] [--sharpen 1.5] [--floor 45] [--invert] [--bg white]
//!  [--dither fs|atkinson:0.85|threshold]`
//!
//! Drew's eggs (`dc/eggs.json`): polish_cow `--bg white --contrast 1.5 --invert`,
//! tron `--gamma 1.2`, rickroll defaults.

use carpi_dis::image::{CoverParams, Dither, cover};
use carpi_dis::screens::egg::{Egg, H, W};
use image::AnimationDecoder;
use image::codecs::gif::GifDecoder;
use std::io::BufReader;
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    assert!(args.len() >= 2, "usage: eggs <in.gif> <out.egg> [flags]");
    let mut p = CoverParams {
        dither: Dither::Atkinson { diffusion: 0.85 },
        ..CoverParams::default()
    };
    let mut it = args[2..].iter();
    while let Some(k) = it.next() {
        let mut val = || it.next().expect("flag value").clone();
        match k.as_str() {
            "--invert" => p.invert = true,
            "--contrast" => p.contrast = val().parse().expect("number"),
            "--gamma" => p.gamma = val().parse().expect("number"),
            "--sharpen" => p.sharpen = val().parse().expect("number"),
            "--floor" => p.black_floor = val().parse().expect("0..255"),
            "--bg" => p.background = if val() == "white" { 255 } else { 0 },
            "--dither" => {
                let v = val();
                p.dither = match v.split_once(':') {
                    Some(("atkinson", d)) => Dither::Atkinson {
                        diffusion: d.parse().expect("diffusion"),
                    },
                    None if v == "fs" => Dither::FloydSteinberg,
                    None if v == "threshold" => Dither::Threshold,
                    _ => Dither::Atkinson { diffusion: 0.85 },
                }
            }
            _ => panic!("unknown flag {k}"),
        }
    }
    let f = std::fs::File::open(&args[0]).expect("gif");
    let frames = GifDecoder::new(BufReader::new(f))
        .expect("decode")
        .into_frames()
        .collect_frames()
        .expect("frames");
    let mut egg = Egg { frames: Vec::new() };
    for fr in &frames {
        let (num, den) = fr.delay().numer_denom_ms();
        let delay = Duration::from_millis(u64::from(num / den.max(1)));
        let rgba = fr.buffer();
        let (w, h) = (rgba.width() as usize, rgba.height() as usize);
        let rgb: Vec<u8> = rgba.pixels().flat_map(|p| [p[0], p[1], p[2]]).collect();
        let a = cover(&rgb, w, h, W, H, &p);
        let rows = (0..H)
            .map(|y| {
                let mut r = [0u8; W / 8];
                for x in 0..W {
                    if a.get(x, y) {
                        r[x / 8] |= 0x80 >> (x % 8);
                    }
                }
                r
            })
            .collect();
        egg.frames.push((rows, delay));
    }
    std::fs::write(&args[1], egg.encode()).expect("write");
    println!("{} frames -> {}", egg.frames.len(), args[1]);
}
