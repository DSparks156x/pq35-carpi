//! Cluster mockups: run the real screens on fixture inputs, apply every
//! message they'd send to the framebuffer model (real glyphs from the font
//! capture, native strokes, clips), and write what the centre would show.
//!
//! `cargo run -p carpi-dis --example mock -- <out dir> [nav-lanes]`
//!
//! Each sheet is a grid of labelled tiles, 4x, lit pixels white on black.
//! Optional "top lines" above a tile show the two 8-cell radio lines.

use carpi_dis::fb::{Fb, PX_H, PX_W};
use carpi_dis::font::{Font, audscii};
use carpi_dis::glyphs;
use carpi_dis::helpers::Ctx;
use carpi_dis::lanes::{Lane, LaneArrow};
use carpi_dis::pack::Registers;
use carpi_dis::proto::Profile;
use carpi_dis::screens::nav::{LaneLayout, NavConfig, NavInput, NavScreen};
use carpi_dis::screens::{Screen, build_frame};
use image::{Rgb, RgbImage};
use std::path::{Path, PathBuf};
use std::time::Instant;

const S: u32 = 4;
const LIT: Rgb<u8> = Rgb([240, 244, 255]);
const DARK: Rgb<u8> = Rgb([0, 0, 0]);
const BG: Rgb<u8> = Rgb([48, 48, 56]);
const LABEL: Rgb<u8> = Rgb([200, 200, 210]);
/// Height of the top-lines strip above a tile (two 14 px lines + gap), px.
const TOP_H: u32 = 32;

/// Run `s` from enter until it has nothing left to send.
fn render(s: &mut dyn Screen) -> Fb {
    let cx = Ctx::new(Profile::WHITE);
    let now = Instant::now();
    let mut fb = Fb::new(cx.metrics);
    let mut regs = Registers::default();
    let mut enter = true;
    for _ in 0..2000 {
        let f = build_frame(s, now, &cx, regs, enter).expect("pack");
        enter = false;
        let idle = f.messages.iter().all(|m| m.bytes == [0x39]) || f.messages.is_empty();
        f.messages.iter().for_each(|m| fb.apply(&m.bytes));
        regs = f.regs;
        if idle {
            break;
        }
    }
    assert!(fb.errors.is_empty(), "{:?}", fb.errors);
    fb
}

/// Text in the cluster's fixed font, straight from the glyph capture, 1x.
fn text(img: &mut RgbImage, x0: u32, y0: u32, s: &str, font: Font, scale: u32) {
    let m = carpi_dis::font::Metrics::for_variant(carpi_dis::proto::Variant::White);
    let mut x = x0;
    for b in audscii(s) {
        for gy in 0..glyphs::H {
            let row = glyphs::row(font, b, gy);
            for gx in 0..glyphs::W {
                if row & (0x8000 >> gx) != 0 {
                    for d in 0..scale * scale {
                        let (px, py) = (x + gx as u32 * scale + d % scale, y0 + gy as u32 * scale + d / scale);
                        if px < img.width() && py < img.height() {
                            img.put_pixel(px, py, LIT);
                        }
                    }
                }
            }
        }
        x += u32::from(m.advance(font, b)) * scale;
    }
}

struct Tile {
    label: String,
    fb: Fb,
    top: Option<(String, String)>,
}

fn sheet(tiles: &[Tile], cols: u32, out: &Path) {
    let (tw, th) = (PX_W as u32 * S, PX_H as u32 * S);
    let top = if tiles.iter().any(|t| t.top.is_some()) { TOP_H * 2 + 8 } else { 0 };
    let (cw, ch) = (tw + 24, th + top + 40);
    let rows = (tiles.len() as u32).div_ceil(cols);
    let mut img = RgbImage::from_pixel(cw * cols, ch * rows, BG);
    for (i, t) in tiles.iter().enumerate() {
        let (x0, y0) = ((i as u32 % cols) * cw + 12, (i as u32 / cols) * ch + 8);
        // Label: the cluster's proportional font at 2x, on the background.
        let _ = LABEL;
        text(&mut img, x0, y0, &t.label, Font::Prop, 2);
        let mut y = y0 + 30;
        if let Some((a, b)) = &t.top {
            for yy in y..y + TOP_H * 2 {
                for xx in x0..x0 + tw {
                    img.put_pixel(xx, yy, DARK);
                }
            }
            text(&mut img, x0 + 8, y + 2, a, Font::Fixed, 2);
            text(&mut img, x0 + 8, y + 2 + TOP_H, b, Font::Fixed, 2);
            y += TOP_H * 2 + 8;
        }
        let px = t.fb.pixels();
        for py in 0..PX_H as u32 {
            for pxx in 0..PX_W as u32 {
                let c = if px[(py as usize) * PX_W + pxx as usize] { LIT } else { DARK };
                for d in 0..S * S {
                    img.put_pixel(x0 + pxx * S + d % S, y + py * S + d / S, c);
                }
            }
        }
    }
    img.save(out).expect("png");
    println!("{}", out.display());
}

fn lane(arrows: &[(&str, bool)]) -> Lane {
    arrows
        .iter()
        .map(|(s, h)| LaneArrow {
            shape: (*s).into(),
            highlighted: *h,
        })
        .collect()
}

fn nav(layout: LaneLayout, input: NavInput) -> Fb {
    let mut s = NavScreen::new(NavConfig {
        lanes: layout,
        ..NavConfig::default()
    });
    s.set(Instant::now(), input);
    render(&mut s)
}

fn nav_lanes(out: &Path) {
    let right = NavInput {
        maneuver: Some("turn".into()),
        side: Some("right".into()),
        distance_m: Some(250.0),
        road: Some("Ring Road".into()),
        lanes: vec![
            lane(&[("normal_left", false)]),
            lane(&[("straight", false)]),
            lane(&[("straight", false), ("normal_right", true)]),
            lane(&[("normal_right", true)]),
        ],
        ..Default::default()
    };
    let left5 = NavInput {
        maneuver: Some("turn".into()),
        side: Some("left".into()),
        distance_m: Some(180.0),
        road: Some("A38 Bristol Road".into()),
        lanes: vec![
            lane(&[("uturn_left", false), ("normal_left", true)]),
            lane(&[("normal_left", true)]),
            lane(&[("straight", false)]),
            lane(&[("straight", false)]),
            lane(&[("normal_right", false)]),
        ],
        ..Default::default()
    };
    let top = Some(("ETA14:32".to_string(), "12km 15m".to_string()));
    let mut tiles = Vec::new();
    for (name, inp) in [("right, 4 lanes", &right), ("left, 5 lanes", &left5)] {
        tiles.push(Tile {
            label: format!("Now (no lanes) - {name}"),
            fb: nav(LaneLayout::Off, inp.clone()),
            top: top.clone(),
        });
        tiles.push(Tile {
            label: format!("Swap road - {name}"),
            fb: nav(LaneLayout::SwapRoad, inp.clone()),
            top: top.clone(),
        });
        tiles.push(Tile {
            label: format!("Pocket - {name}"),
            fb: nav(LaneLayout::Pocket, inp.clone()),
            top: top.clone(),
        });
        tiles.push(Tile {
            label: format!("Shrink icon - {name}"),
            fb: nav(LaneLayout::ShrinkIcon, inp.clone()),
            top: top.clone(),
        });
    }
    sheet(&tiles, 4, &out.join("nav_lanes.png"));
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = PathBuf::from(args.first().expect("usage: mock <out dir> [sheet]"));
    std::fs::create_dir_all(&out).expect("out dir");
    let which = args.get(1).map_or("nav-lanes", String::as_str);
    match which {
        "nav-lanes" => nav_lanes(&out),
        other => panic!("unknown sheet {other}"),
    }
}
