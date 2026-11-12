//! Animated previews for the website: run the real screens through a short
//! scripted timeline, apply every message to the framebuffer model as the
//! cluster would (one message at a time, so paints land in their real order),
//! and write one GIF per scene.
//!
//! `cargo run -p carpi-dis --example gifs -- <out dir> [cover.png] [scene]`
//!
//! Scenes: nav, media, readings, accel, call. `cover.png` feeds the media
//! scene's art (any RGB/RGBA image).

#![allow(
    clippy::type_complexity,
    clippy::collapsible_if,
    clippy::needless_update
)]

use carpi_dis::fb::{Fb, PX_H, PX_W};
use carpi_dis::font::{Font, audscii};
use carpi_dis::glyphs;
use carpi_dis::helpers::Ctx;
use carpi_dis::lanes::{Lane, LaneArrow};
use carpi_dis::pack::Registers;
use carpi_dis::proto::Profile;
use carpi_dis::screens::{Screen, build_frame};
use image::codecs::gif::{GifEncoder, Repeat};
use image::{Delay, Frame, Rgba, RgbaImage};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Screen px per cluster px, solid (the real panel shows no pixel grid).
const S: u32 = 4;
const LIT: Rgba<u8> = Rgba([236, 242, 255, 255]);
const OFF: Rgba<u8> = Rgba([0, 0, 0, 255]);
const BG: Rgba<u8> = Rgba([8, 7, 12, 255]);
/// Whole-cluster layout, cluster px: two 8-cell top lines in the fixed font,
/// a 1 px rule, the centre, a 1 px rule, the gear row.
const ROW_H: u32 = 16;
const GAP: u32 = 2;
const CENTRE_Y: u32 = ROW_H * 2 + GAP * 2 + 1;
const GEAR_Y: u32 = CENTRE_Y + PX_H as u32 + GAP * 2 + 1;
const PANEL_H: u32 = GEAR_Y + ROW_H;
const PAD: u32 = 16;
/// Per-message frame time while a paint lands, ms (GIF floor is 20).
const MSG_MS: u32 = 30;
/// Simulation tick, ms.
const TICK: u64 = 100;

struct Sim {
    cx: Ctx,
    fb: Fb,
    regs: Registers,
    enter: bool,
    t0: Instant,
    /// Sim time, ms.
    t: u64,
    /// (picture, top lines, delay ms)
    frames: Vec<(Vec<bool>, Option<(String, String)>, u32)>,
    top: Option<(String, String)>,
    /// Line 1 source (marquees when longer than 8); line 2 is the cluster's
    /// outside temperature while the centre is ours.
    line1: String,
    line2: String,
    /// Selected of `PRNDS`.
    gear: char,
}

impl Sim {
    fn new() -> Self {
        let cx = Ctx::new(Profile::WHITE);
        Sim {
            fb: Fb::new(cx.metrics),
            cx,
            regs: Registers::default(),
            enter: true,
            t0: Instant::now(),
            t: 0,
            frames: Vec::new(),
            top: None,
            line1: "Grass, Mostly".into(),
            line2: "41\u{B0}F".into(),
            gear: 'D',
        }
    }

    fn now(&self) -> Instant {
        self.t0 + Duration::from_millis(self.t)
    }

    /// Re-enter on the next tick (a page change or a new screen).
    fn reenter(&mut self) {
        self.enter = true;
    }

    /// One tick: build frames until idle, one picture per message that
    /// changed something, the rest of the tick on the last picture.
    fn tick(&mut self, s: &mut dyn Screen) {
        let now = self.now();
        self.top = Some((marquee(&self.line1, self.t), self.line2.clone()));
        let mut spent = 0u32;
        for _ in 0..200 {
            let f = build_frame(s, now, &self.cx, self.regs, self.enter).expect("pack");
            self.enter = false;
            self.regs = f.regs;
            let idle = f.messages.is_empty() || f.messages.iter().all(|m| m.bytes == [0x39]);
            for m in &f.messages {
                let before = self.fb.pixels().to_vec();
                self.fb.apply(&m.bytes);
                if self.fb.pixels() != before.as_slice() {
                    self.push(MSG_MS);
                    spent += MSG_MS;
                }
            }
            if idle {
                break;
            }
        }
        assert!(self.fb.errors.is_empty(), "{:?}", self.fb.errors);
        let rest = (TICK as u32).saturating_sub(spent);
        if rest > 0 {
            self.push(rest);
        }
        self.t += TICK;
    }

    fn push(&mut self, ms: u32) {
        let px = self.fb.pixels().to_vec();
        if let Some(last) = self.frames.last_mut() {
            if last.0 == px && last.1 == self.top {
                last.2 += ms;
                return;
            }
        }
        self.frames.push((px, self.top.clone(), ms));
    }

    fn write(&self, out: &Path) {
        let (iw, ih) = (PX_W as u32 * S + PAD * 2, PANEL_H * S + PAD * 2);
        let file = std::fs::File::create(out).expect("create");
        let mut enc = GifEncoder::new_with_speed(file, 10);
        enc.set_repeat(Repeat::Infinite).expect("repeat");
        for (px, top, ms) in &self.frames {
            let mut img = RgbaImage::from_pixel(iw, ih, BG);
            fill(&mut img, 0, 0, PX_W as u32, PANEL_H, OFF);
            if let Some((a, b)) = top {
                for (k, line) in [a, b].iter().enumerate() {
                    fixed_line(&mut img, line, k as u32 * ROW_H + 1);
                }
            }
            fill(&mut img, 0, CENTRE_Y - GAP - 1, PX_W as u32, 1, LIT);
            fill(&mut img, 0, GEAR_Y - GAP - 1, PX_W as u32, 1, LIT);
            for py in 0..PX_H as u32 {
                for pxx in 0..PX_W as u32 {
                    if px[py as usize * PX_W + pxx as usize] {
                        fill(&mut img, pxx, CENTRE_Y + py, 1, 1, LIT);
                    }
                }
            }
            // The selected gear, one letter.
            fixed_line(&mut img, &self.gear.to_string(), GEAR_Y + 1);
            let frame = Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms((*ms).max(20), 1));
            enc.encode_frame(frame).expect("frame");
        }
        println!("{} ({} frames)", out.display(), self.frames.len());
    }
}

/// Up to 8 cells of fixed-font text at its own advance, centred.
fn fixed_line(img: &mut RgbaImage, s: &str, y: u32) {
    let m = carpi_dis::font::Metrics::for_variant(carpi_dis::proto::Variant::White);
    let b: Vec<u8> = audscii(s).into_iter().take(8).collect();
    let blank = |c: u8| (0..glyphs::H).all(|y| glyphs::row(Font::Fixed, c, y) == 0);
    let adv = |c: u8| {
        if blank(c) && c != 0x20 {
            12
        } else {
            u32::from(m.advance(Font::Fixed, c))
        }
    };
    let w: u32 = b.iter().map(|&c| adv(c)).sum();
    let mut x = (PX_W as u32).saturating_sub(w) / 2;
    for c in b {
        // The fixed-font capture has no `l`; borrow the prop one.
        let f = if blank(c) { Font::Prop } else { Font::Fixed };
        glyph(img, f, c, x, y, LIT);
        x += adv(c);
    }
}

/// A rect in cluster px, onto the panel.
fn fill(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, c: Rgba<u8>) {
    for yy in (y * S)..((y + h) * S) {
        for xx in (x * S)..((x + w) * S) {
            img.put_pixel(PAD + xx, PAD + yy, c);
        }
    }
}

/// One captured glyph at cluster px (x, y).
fn glyph(img: &mut RgbaImage, font: Font, b: u8, x: u32, y: u32, c: Rgba<u8>) {
    for gy in 0..glyphs::H {
        let row = glyphs::row(font, b, gy);
        for gx in 0..glyphs::W {
            if row & (0x8000 >> gx) != 0 {
                fill(img, x + gx as u32, y + gy as u32, 1, 1, c);
            }
        }
    }
}

/// An 8-cell marquee window over `s`, 400 ms a step (the top lines' timing).
fn marquee(s: &str, t: u64) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= 8 {
        return s.to_string();
    }
    let stream: Vec<char> = chars.iter().copied().chain(std::iter::once(' ')).collect();
    let k = (t / 400) as usize;
    (0..8).map(|i| stream[(k + i) % stream.len()]).collect()
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

/// Approach a right turn with lanes, the pocket cycling, then the next leg.
fn nav(out: &Path, stock: bool) {
    use carpi_dis::screens::nav::{LaneLayout, NavConfig, NavInput, NavScreen, NavStyle};
    let mut sim = Sim::new();
    let cfg = if stock {
        NavConfig {
            style: NavStyle::Stock,
            show_distance: false,
            pocket: false,
            lanes: LaneLayout::Off,
            ..NavConfig::default()
        }
    } else {
        NavConfig::default()
    };
    let mut s = NavScreen::new(cfg);
    // A short route: (maneuver, side, roundabout angle, road, lanes, metres).
    let route: Vec<(&str, Option<&str>, Option<i32>, &str, Vec<Lane>, f64)> = vec![
        (
            "turn",
            Some("right"),
            None,
            "County Road 12",
            vec![
                lane(&[("normal_left", false)]),
                lane(&[("straight", false)]),
                lane(&[("straight", false), ("normal_right", true)]),
                lane(&[("normal_right", true)]),
            ],
            320.0,
        ),
        (
            "roundabout_enter_and_exit",
            Some("right"),
            Some(270),
            "Harmony Rd",
            vec![],
            240.0,
        ),
        ("on_ramp", Some("right"), None, "I-25 N", vec![], 220.0),
        (
            "fork",
            Some("left"),
            None,
            "US-34 W",
            vec![
                lane(&[("slight_left", true)]),
                lane(&[("straight", false), ("slight_left", true)]),
                lane(&[("straight", false)]),
                lane(&[("straight", false)]),
                lane(&[("slight_right", false)]),
            ],
            300.0,
        ),
        ("destination", None, None, "Pasture", vec![], 200.0),
    ];
    let total: f64 = route.iter().map(|r| r.5).sum();
    let mut done = 0.0;
    for (man, side, angle, road, lanes, start) in route {
        let mut d = start;
        loop {
            let left = total - done - (start - d);
            let now = sim.now();
            s.set(
                now,
                NavInput {
                    maneuver: Some(man.into()),
                    side: side.map(Into::into),
                    roundabout_angle: angle,
                    distance_m: Some(d.max(0.0)),
                    road: Some(road.into()),
                    lanes: lanes.clone(),
                    eta: Some("2:41 PM".into()),
                    distance_unit: Some("mi".into()),
                    time_to_destination_s: Some(left / 15.0 + 20.0),
                    destination_distance_m: Some(left),
                    ..Default::default()
                },
            );
            sim.tick(&mut s);
            if d <= 0.0 {
                break;
            }
            d -= 36.0 * TICK as f64 / 1000.0;
        }
        done += start;
    }
    for _ in 0..15 {
        sim.tick(&mut s);
    }
    sim.write(&out.join(if stock { "nav-stock.gif" } else { "nav.gif" }));
}

/// Track playing with the art layout, then a track change and the cover flash.
/// `mode`: `art` (cover + text), `text`, or `flash` (the cover full screen on
/// a new track, then cover + text).
fn media(out: &Path, cover_png: Option<&Path>, mode: &str) {
    use carpi_dis::image::{CoverParams, cover};
    use carpi_dis::screens::cover::CoverScreen;
    use carpi_dis::screens::media::{ART, MediaInput, MediaLayout, MediaScreen};
    let p = CoverParams::default();
    let rgb = cover_png.map(|f| {
        // Transparent PNGs: flatten onto black, like a cover would be.
        let img = image::open(f).expect("cover").to_rgba8();
        let mut out = image::RgbImage::new(img.width(), img.height());
        for (x, y, px) in img.enumerate_pixels() {
            let a = u32::from(px[3]);
            out.put_pixel(
                x,
                y,
                image::Rgb([0, 1, 2].map(|c| (u32::from(px[c]) * a / 255) as u8)),
            );
        }
        out
    });
    let art = |id: u64, w: usize, h: usize| {
        rgb.as_ref().map(|img| {
            (
                id,
                cover(
                    img.as_raw(),
                    img.width() as usize,
                    img.height() as usize,
                    w,
                    h,
                    &p,
                ),
            )
        })
    };
    let input = |title: &str, artist: &str, album: &str, pos: u64, len: u64, id: u64| MediaInput {
        title: Some(title.into()),
        artist: Some(artist.into()),
        album: Some(album.into()),
        playing: true,
        position_ms: Some(pos),
        duration_ms: Some(len),
        art: art(id, usize::from(ART.w) * 2, usize::from(ART.h) * 2),
        cover: art(id, 128, 96),
    };
    let mut sim = Sim::new();
    sim.line1 = "Low Frequency Moo".into();
    let next = input("Low Frequency Moo", "The Herd", "Pasture", 0, 241_000, 2);
    if mode == "flash" {
        if let Some(c) = next.cover.clone() {
            let mut cs = CoverScreen::new();
            cs.set(Some(c));
            for _ in 0..30 {
                sim.tick(&mut cs);
            }
            sim.reenter();
        }
    }
    let layout = if mode == "text" {
        MediaLayout::Stack
    } else {
        MediaLayout::Art
    };
    let mut s = MediaScreen::new(layout);
    let n = if mode == "flash" { 50 } else { 90 };
    for k in 0..n as u64 {
        let now = sim.now();
        let mut i = next.clone();
        i.position_ms = Some(k * TICK / 1000 * 1000);
        s.set(now, i);
        sim.tick(&mut s);
    }
    sim.write(&out.join(format!("media-{mode}.gif")));
}

/// Two readings pages with live values; DIS focus on, the wheel steps pages.
fn readings(out: &Path) {
    use carpi_dis::screens::readings::{ReadingCell, ReadingsInput, ReadingsPage, ReadingsScreen};
    let cell = |k: &str, d: u8, u: &str| ReadingCell {
        key: k.into(),
        decimals: d,
        icon: None,
        unit: Some(u.into()),
    };
    let pages = vec![
        ReadingsPage {
            name: "Engine".into(),
            cells: vec![
                cell("vehicle.speed_mph", 0, "mph"),
                cell("engine.rpm", 0, "rpm"),
                cell("engine.coolant_f", 0, "\u{B0}F"),
                cell("engine.oil_temp_f", 0, "\u{B0}F"),
                cell("power.battery_v", 1, "V"),
                cell("engine.boost_psi", 1, "psi"),
                cell("engine.iat_f", 0, "\u{B0}F"),
                cell("engine.afr", 1, ""),
            ],
        },
        ReadingsPage {
            name: "Haldex".into(),
            cells: vec![
                cell("haldex.oil_temperature_f", 0, "\u{B0}F"),
                cell("haldex.plate_temperature_f", 0, "\u{B0}F"),
                cell("haldex.pressure_bar", 1, "bar"),
                cell("haldex.torque_nm", 0, "Nm"),
                cell("haldex.valve_pct", 0, "%"),
                cell("haldex.valve_ma", 0, "mA"),
                cell("transmission.oil_temperature_f", 0, "\u{B0}F"),
                cell("ambient.outside_temp_f", 0, "\u{B0}F"),
            ],
        },
    ];
    let mut s = ReadingsScreen::with_pages(pages);
    let mut sim = Sim::new();
    let w = |t: f64, a: f64, f: f64| (t * f).sin() * a;
    for k in 0..120u64 {
        let t = k as f64 * 0.1;
        // A gentle pull from ~2 s to ~6 s.
        let pull = ((t - 2.0) / 4.0).clamp(0.0, 1.0);
        let pull = if t > 6.0 {
            (1.0 - (t - 6.0) / 2.0).max(0.0)
        } else {
            pull
        };
        if k == 20 {
            s.set_focus(true);
        }
        if k == 80 {
            s.step(1);
            sim.reenter();
        }
        let values = if s.page_index() == 0 {
            [
                Some(42.0 + pull * 26.0 + w(t, 0.6, 1.3)),
                Some(2100.0 + pull * 3400.0 + w(t, 40.0, 2.1)),
                Some(194.0),
                Some(219.0 + pull * 3.0),
                Some(14.2 + w(t, 0.05, 0.7)),
                Some((-6.0 + pull * 22.0 + w(t, 0.3, 3.0)).max(-11.0)),
                Some(86.0 + pull * 4.0),
                Some(14.7 - pull * 3.1 + w(t, 0.1, 2.7)),
            ]
        } else {
            [
                Some(118.0 + w(t, 0.4, 0.5)),
                Some(131.0 + w(t, 1.0, 0.8)),
                Some(12.4 + w(t, 2.0, 1.9)),
                Some(240.0 + w(t, 60.0, 1.1)),
                Some(31.0 + w(t, 8.0, 1.6)),
                Some(410.0 + w(t, 40.0, 1.6)),
                Some(171.0),
                Some(41.0),
            ]
        };
        s.set(ReadingsInput {
            title: String::new(),
            values,
        });
        sim.tick(&mut s);
    }
    sim.write(&out.join("readings.gif"));
}

/// A full accel run: ready, timing, each leg landing, the lift.
fn accel(out: &Path, metric: bool) {
    use carpi_dis::screens::accel::{AccelConfig, AccelScreen, Leg};
    let cfg = if metric {
        AccelConfig {
            legs: vec![
                Leg::range(0.0, 100.0),
                Leg::range(80.0, 120.0),
                Leg::Distance {
                    m: 400.0,
                    label: "400 m".into(),
                },
                Leg::Speed,
            ],
            imperial: false,
            ..AccelConfig::default()
        }
    } else {
        AccelConfig {
            legs: vec![
                Leg::range(0.0, 60.0),
                Leg::range(30.0, 70.0),
                Leg::Distance {
                    m: 402.336,
                    label: "1/4 mi".into(),
                },
                Leg::Speed,
            ],
            imperial: true,
            ..AccelConfig::default()
        }
    };
    let k = if metric { 1.609_344 } else { 1.0 };
    let v = |s: f64| {
        k * {
            if s < 1.5 {
                0.0
            } else if s < 20.0 {
                120.0 * (1.0 - (-(s - 1.5) / 9.0).exp())
            } else {
                (120.0 * (1.0 - (-18.5f64 / 9.0).exp()) - (s - 20.0) * 15.0).max(0.0)
            }
        }
    };
    let mut s = AccelScreen::new(cfg);
    let mut sim = Sim::new();
    let end = 25_000;
    while sim.t < end {
        // 20 samples a second.
        for k in 0..2 {
            let tm = sim.t + k * 50;
            s.sample(sim.t0 + Duration::from_millis(tm), v(tm as f64 / 1000.0));
        }
        sim.tick(&mut s);
    }
    sim.write(&out.join(if metric {
        "accel-metric.gif"
    } else {
        "accel.gif"
    }));
}

/// Incoming, answered, a few seconds of call, hung up.
fn call(out: &Path) {
    use carpi_dis::screens::call::{CallInput, CallScreen};
    let mut s = CallScreen::new();
    s.set_focus(true);
    let mut sim = Sim::new();
    let base = CallInput {
        name: Some("Mom".into()),
        number: Some("+1 555 0100".into()),
        signal_pct: Some(75),
        battery_pct: Some(60),
        ..Default::default()
    };
    for k in 0..90u64 {
        let state = if k < 30 { "ringing" } else { "active" };
        let now = sim.now();
        s.set(
            now,
            CallInput {
                state: Some(state.into()),
                ..base.clone()
            },
        );
        sim.tick(&mut s);
    }
    sim.write(&out.join("call.gif"));
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = PathBuf::from(
        args.first()
            .expect("usage: gifs <out dir> [cover.png] [scene]"),
    );
    std::fs::create_dir_all(&out).expect("out dir");
    let cover = args
        .get(1)
        .filter(|a| a.ends_with(".png") || a.ends_with(".jpg"));
    let which = args
        .iter()
        .skip(1)
        .find(|a| Some(*a) != cover)
        .map(String::as_str);
    let cover = cover.map(PathBuf::from);
    let all = which.is_none();
    if all || which == Some("nav") {
        nav(&out, false);
        nav(&out, true);
    }
    if all || which == Some("media") {
        for m in ["art", "text", "flash"] {
            media(&out, cover.as_deref(), m);
        }
    }
    if all || which == Some("readings") {
        readings(&out);
    }
    if all || which == Some("accel") {
        accel(&out, false);
        accel(&out, true);
    }
    if all || which == Some("call") {
        call(&out);
    }
}
