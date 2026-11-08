//! Cluster mockups: run the real screens on fixture inputs, apply every
//! message they'd send to the framebuffer model (real glyphs from the font
//! capture, native strokes, clips), and write what the centre would show.
//!
//! `cargo run -p carpi-dis --example mock -- <out dir> [nav-lanes|nav-stock]`
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
    // Line 1 is ours: 8 cells, the title marquees through it (here its first
    // window). Line 2 is the cluster's outside temperature while the centre is
    // in use (Drew), so nothing of ours goes there.
    let top = Some(("One More".to_string(), "4.5\u{B0}C".to_string()));
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

/// Stock style next to fancy, same maneuvers.
fn nav_stock(out: &Path) {
    use carpi_dis::screens::nav::NavStyle;
    let m = |man: &str, side: Option<&str>, angle: Option<i32>, d: f64, road: &str| NavInput {
        maneuver: Some(man.into()),
        side: side.map(Into::into),
        roundabout_angle: angle,
        distance_m: Some(d),
        road: Some(road.into()),
        eta: Some("2:32 PM".into()),
        time_to_destination_s: Some(900.0),
        destination_distance_m: Some(9_000.0),
        distance_unit: Some("mi".into()),
        ..Default::default()
    };
    let cases = [
        ("turn right", m("turn", Some("right"), None, 250.0, "Ring Road")),
        ("slight left", m("slight_turn", Some("left"), None, 600.0, "A38 Bristol Road")),
        ("roundabout, 3rd exit", m("roundabout_enter_and_exit", Some("right"), Some(270), 400.0, "Ring Road")),
        ("u-turn", m("u_turn", Some("left"), None, 150.0, "Back Lane")),
        ("fork right", m("fork", Some("right"), None, 900.0, "M5")),
        ("straight", m("straight", None, None, 3000.0, "M5")),
    ];
    let top = Some(("One More".to_string(), "4.5\u{B0}C".to_string()));
    let mut tiles = Vec::new();
    for (name, inp) in cases {
        for (style, sname) in [(NavStyle::Stock, "stock"), (NavStyle::Fancy, "fancy")] {
            let mut s = NavScreen::new(NavConfig {
                style,
                ..NavConfig::default()
            });
            let t0 = Instant::now();
            s.set(t0, inp.clone());
            tiles.push(Tile {
                label: format!("{sname}: {name}"),
                fb: render_at(&mut s, t0, &[]),
                top: top.clone(),
            });
        }
    }
    sheet(&tiles, 4, &out.join("nav_stock.png"));
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = PathBuf::from(args.first().expect("usage: mock <out dir> [sheet]"));
    std::fs::create_dir_all(&out).expect("out dir");
    let which = args.get(1).map_or("nav-lanes", String::as_str);
    match which {
        "nav-lanes" => nav_lanes(&out),
        "nav-stock" => nav_stock(&out),
        other => panic!("unknown sheet {other}"),
    }
}
