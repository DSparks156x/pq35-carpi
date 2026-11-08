//! Cluster mockups: run the real screens on fixture inputs, apply every
//! message they'd send to the framebuffer model (real glyphs from the font
//! capture, native strokes, clips), and write what the centre would show.
//!
//! `cargo run -p carpi-dis --example mock -- <out dir> [nav-lanes|lane-art]`
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
use std::time::{Duration, Instant};

const S: u32 = 4;
const LIT: Rgb<u8> = Rgb([240, 244, 255]);
const DARK: Rgb<u8> = Rgb([0, 0, 0]);
const BG: Rgb<u8> = Rgb([48, 48, 56]);
const LABEL: Rgb<u8> = Rgb([200, 200, 210]);
/// Height of one top line above a tile: a 14 px glyph at the tile scale, px.
const TOP_H: u32 = 60;

/// Enter `s` at `t0`, run it until idle, then step its clock to each of
/// `at` (offsets from `t0`) and run it until idle again. The screen as it
/// stands after the last one.
fn render_at(s: &mut dyn Screen, t0: Instant, at: &[Duration]) -> Fb {
    let cx = Ctx::new(Profile::WHITE);
    let mut fb = Fb::new(cx.metrics);
    let mut regs = Registers::default();
    let mut enter = true;
    let times: Vec<Instant> = std::iter::once(t0).chain(at.iter().map(|d| t0 + *d)).collect();
    for now in times {
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
            // The cluster centres the 8-cell lines in its proportional font.
            for (k, line) in [a, b].iter().enumerate() {
                let w = m_width(line) * 2;
                let x = x0 + tw.saturating_sub(w * 2) / 2;
                text(&mut img, x, y + 2 + TOP_H * k as u32, line, Font::Prop, 4);
            }
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

fn m_width(s: &str) -> u32 {
    let m = carpi_dis::font::Metrics::for_variant(carpi_dis::proto::Variant::White);
    u32::from(m.width(Font::Prop, &audscii(s)))
}

/// Nav at `t` seconds after it was entered.
fn nav(layout: LaneLayout, input: NavInput, t: f64) -> Fb {
    let mut s = NavScreen::new(NavConfig {
        lanes: layout,
        ..NavConfig::default()
    });
    let t0 = Instant::now();
    s.set(t0, input);
    let steps: Vec<Duration> = (1..=(t * 2.0) as u32).map(|k| Duration::from_millis(500 * u64::from(k))).collect();
    render_at(&mut s, t0, &steps)
}

fn nav_lanes(out: &Path) {
    let trip = |mut i: NavInput| {
        i.eta = Some("14:32".into());
        i.time_to_destination_s = Some(15.0 * 60.0);
        i.destination_distance_m = Some(12_400.0);
        i
    };
    let right = trip(NavInput {
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
    });
    let far = NavInput {
        distance_m: Some(800.0),
        ..right.clone()
    };
    let left5 = trip(NavInput {
        maneuver: Some("turn".into()),
        side: Some("left".into()),
        distance_m: Some(180.0),
        road: Some("A38 Bristol Road".into()),
        lanes: vec![
            lane(&[("uturn_left", false), ("normal_left", true)]),
            lane(&[("normal_left", true)]),
            lane(&[("straight", false)]),
            lane(&[("straight", false)]),
            lane(&[("normal_right", false), ("slight_right", false)]),
        ],
        ..Default::default()
    });
    let straight = trip(NavInput {
        maneuver: Some("straight".into()),
        distance_m: Some(2400.0),
        road: Some("M5".into()),
        ..Default::default()
    });
    let roundabout = trip(NavInput {
        maneuver: Some("roundabout_enter_and_exit".into()),
        side: Some("right".into()),
        roundabout_angle: Some(270),
        distance_m: Some(600.0),
        road: Some("Ring Road".into()),
        ..Default::default()
    });
    let top = Some(("One More".to_string(), "Time".to_string()));
    let f = LaneLayout::Flip;
    let tiles = vec![
        Tile { label: "800 m, pocket: ETA".into(), fb: nav(f, far.clone(), 0.0), top: top.clone() },
        Tile { label: "pocket after 3 s: time".into(), fb: nav(f, far.clone(), 3.0), top: top.clone() },
        Tile { label: "pocket after 6 s: distance".into(), fb: nav(f, far, 6.0), top: top.clone() },
        Tile { label: "straight: narrow pocket".into(), fb: nav(f, straight, 0.0), top: top.clone() },
        Tile { label: "250 m, flip: lanes 4 s".into(), fb: nav(f, right.clone(), 0.0), top: top.clone() },
        Tile { label: "flip: road 2 s".into(), fb: nav(f, right, 4.5), top: top.clone() },
        Tile { label: "5 lanes".into(), fb: nav(f, left5, 0.0), top: top.clone() },
        Tile { label: "roundabout: pocket".into(), fb: nav(f, roundabout, 0.0), top },
    ];
    sheet(&tiles, 4, &out.join("nav_lanes.png"));
}

/// Lane arrow styles side by side: each row a style, each column a lane set,
/// the 128x18 px road-row strip at the tile scale.
fn lane_art(out: &Path) {
    use carpi_dis::icons::Art;
    use carpi_dis::lanes::{strip, strip_maps};
    let sets: Vec<(&str, Vec<Lane>)> = vec![
        (
            "right, 4 lanes",
            vec![
                lane(&[("normal_left", false)]),
                lane(&[("straight", false)]),
                lane(&[("straight", false), ("normal_right", true)]),
                lane(&[("normal_right", true)]),
            ],
        ),
        (
            "left, 5 lanes",
            vec![
                lane(&[("uturn_left", false), ("normal_left", true)]),
                lane(&[("normal_left", true)]),
                lane(&[("straight", false)]),
                lane(&[("straight", false)]),
                lane(&[("normal_right", false), ("slight_right", false)]),
            ],
        ),
        (
            "keep left, 3 lanes",
            vec![
                lane(&[("slight_left", true)]),
                lane(&[("straight", false), ("slight_left", true)]),
                lane(&[("straight", false)]),
            ],
        ),
    ];
    let (w, h) = (128usize, 18usize);
    let styles: Vec<(&str, Box<dyn Fn(&[Lane]) -> Art>)> = vec![
        ("pixel art 12x16 (now)", Box::new(move |l: &[Lane]| strip(l, w, h, 14))),
        ("Maps icons 18 px, the arrow to take", Box::new(move |l: &[Lane]| strip_maps(l, w, h, 21, 18, false))),
        ("Maps icons 18 px, all arrows overlaid", Box::new(move |l: &[Lane]| strip_maps(l, w, h, 21, 18, true))),
        ("Maps icons 16 px, the arrow to take", Box::new(move |l: &[Lane]| strip_maps(l, w, h, 19, 16, false))),
    ];
    let (cw, ch) = (w as u32 * S + 24, h as u32 * S + 40);
    let label_w = 560;
    let mut img = RgbImage::from_pixel(label_w + cw * sets.len() as u32, 40 + ch * styles.len() as u32, BG);
    for (c, (name, _)) in sets.iter().enumerate() {
        text(&mut img, label_w + c as u32 * cw + 12, 8, name, Font::Prop, 2);
    }
    for (r, (sname, f)) in styles.iter().enumerate() {
        let y0 = 40 + r as u32 * ch;
        text(&mut img, 12, y0 + 24, sname, Font::Prop, 2);
        for (c, (_, lanes)) in sets.iter().enumerate() {
            let a = f(lanes);
            let x0 = label_w + c as u32 * cw + 12;
            for y in 0..h as u32 {
                for x in 0..w as u32 {
                    let col = if a.get(x as usize, y as usize) { LIT } else { DARK };
                    for d in 0..S * S {
                        img.put_pixel(x0 + x * S + d % S, y0 + 12 + y * S + d / S, col);
                    }
                }
            }
        }
    }
    let p = out.join("lane_art.png");
    img.save(&p).expect("png");
    println!("{}", p.display());
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = PathBuf::from(args.first().expect("usage: mock <out dir> [sheet]"));
    std::fs::create_dir_all(&out).expect("out dir");
    let which = args.get(1).map_or("nav-lanes", String::as_str);
    match which {
        "nav-lanes" => nav_lanes(&out),
        "lane-art" => lane_art(&out),
        other => panic!("unknown sheet {other}"),
    }
}
