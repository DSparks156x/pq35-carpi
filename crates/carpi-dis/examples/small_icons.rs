//! Small 1-bit icons from the Maps vectors, grid-fitted (offline).
//!
//! Downscaling the 72 px masks butchers small icons. This renders each SVG
//! (`dc/nav_icons_sources/*.svg`, 24-unit Material grid, 2.5-unit strokes)
//! straight at the target size with analytic anti-aliasing, and searches
//! scale and sub-pixel offset for the crispest result: strokes landing on
//! whole pixels, so the 1-bit threshold keeps clean, even stems instead of
//! smearing. Then thresholds.
//!
//! `cargo run -p carpi-dis --example small_icons -- <svg dir> <out dir> [sizes...]`
//!
//! Writes `small_icons.png` (per shape: the box-filtered downscale vs the
//! grid fit, at each size, 1x and 4x), `<shape>_<size>.txt` masks (`#`/`.`),
//! and with `--rust <size> <file>` the masks at that size as the
//! `lane_icons.rs` data module carpi-dis ships.

use carpi_dis::icons::{self, Maneuver, Side};
use image::{Rgb, RgbImage};
use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg;
use std::path::{Path, PathBuf};

/// Lane shapes we need, the SVG for each, and the 72 px icon it matches.
const SHAPES: [(&str, &str, Maneuver); 9] = [
    ("straight", "maneuver_straight.svg", Maneuver::Straight),
    (
        "slight_right",
        "maneuver_turn_slight_right.svg",
        Maneuver::Slight(Side::Right),
    ),
    (
        "slight_left",
        "maneuver_turn_slight_left.svg",
        Maneuver::Slight(Side::Left),
    ),
    (
        "normal_right",
        "maneuver_turn_normal_right.svg",
        Maneuver::Turn(Side::Right),
    ),
    (
        "normal_left",
        "maneuver_turn_normal_left.svg",
        Maneuver::Turn(Side::Left),
    ),
    (
        "sharp_right",
        "maneuver_turn_sharp_right.svg",
        Maneuver::Sharp(Side::Right),
    ),
    (
        "sharp_left",
        "maneuver_turn_sharp_left.svg",
        Maneuver::Sharp(Side::Left),
    ),
    ("uturn_left", "maneuver_u_turn_left.svg", Maneuver::UTurn),
    ("uturn_right", "maneuver_u_turn_right.svg", Maneuver::UTurn),
];

struct Fit {
    px: Vec<bool>,
    scale: f32,
    score: f32,
}

/// Coverage 0..1 of `tree` at `scale` and offset into a `size` square.
fn render(tree: &usvg::Tree, size: u32, scale: f32, dx: f32, dy: f32) -> Vec<f32> {
    let mut pm = Pixmap::new(size, size).expect("pixmap");
    resvg::render(
        tree,
        Transform::from_row(scale, 0.0, 0.0, scale, dx, dy),
        &mut pm.as_mut(),
    );
    pm.pixels()
        .iter()
        .map(|p| f32::from(p.alpha()) / 255.0)
        .collect()
}

/// Search scale and phase for the crispest render that fits the box.
fn fit(tree: &usvg::Tree, size: u32) -> Fit {
    let bb = tree.root().abs_bounding_box();
    let (bw, bh) = (bb.width(), bb.height());
    let mut best: Option<Fit> = None;
    // Largest scale that fits, down to 70 % of it.
    let max = (size as f32 - 0.5) / bw.max(bh);
    for si in 0..=30 {
        let scale = max * (1.0 - 0.01 * si as f32);
        for py in 0..8 {
            for px in 0..8 {
                // Centre, then nudge by 1/8 px phases.
                let dx =
                    (size as f32 - bw * scale) / 2.0 - bb.left() * scale + (px as f32 - 3.5) / 8.0;
                let dy =
                    (size as f32 - bh * scale) / 2.0 - bb.top() * scale + (py as f32 - 3.5) / 8.0;
                let cov = render(tree, size, scale, dx, dy);
                // Crispness: how far each pixel sits from fully on or off.
                let fuzz: f32 = cov.iter().map(|c| c.min(1.0 - c)).sum::<f32>() / cov.len() as f32;
                // Prefer bigger (more detail) when nearly as crisp.
                let score = fuzz - 0.02 * (scale / max);
                if best.as_ref().is_none_or(|b| score < b.score) {
                    best = Some(Fit {
                        px: cov.iter().map(|c| *c >= 0.5).collect(),
                        scale,
                        score,
                    });
                }
            }
        }
    }
    best.expect("a fit")
}

fn put(img: &mut RgbImage, px: &[bool], size: u32, x0: u32, y0: u32, s: u32) {
    for y in 0..size {
        for x in 0..size {
            let c = if px[(y * size + x) as usize] {
                Rgb([240, 244, 255])
            } else {
                Rgb([0, 0, 0])
            };
            for d in 0..s * s {
                img.put_pixel(x0 + x * s + d % s, y0 + y * s + d / s, c);
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    assert!(
        args.len() >= 2,
        "usage: small_icons <svg dir> <out dir> [sizes...]"
    );
    let (dir, out) = (PathBuf::from(&args[0]), PathBuf::from(&args[1]));
    let mut rust: Option<(u32, PathBuf)> = None;
    let mut rest = Vec::new();
    let mut it = args[2..].iter();
    while let Some(a) = it.next() {
        if a == "--rust" {
            let z = it.next().expect("size").parse().expect("size");
            rust = Some((z, PathBuf::from(it.next().expect("file"))));
        } else {
            rest.push(a.parse().expect("size"));
        }
    }
    let sizes: Vec<u32> = if rest.is_empty() {
        vec![14, 16, 18, 20]
    } else {
        rest
    };
    let mut data_rows = String::new();
    std::fs::create_dir_all(&out).expect("out dir");
    let s = 4;
    let cell = |z: u32| z * s + 12;
    let row_w: u32 = sizes.iter().map(|z| cell(*z) * 2).sum::<u32>() + 12;
    let row_h = sizes.iter().max().copied().unwrap_or(20) * s + 16;
    let mut img = RgbImage::from_pixel(row_w, row_h * SHAPES.len() as u32, Rgb([48, 48, 56]));
    for (r, (name, file, m)) in SHAPES.iter().enumerate() {
        let data = std::fs::read(dir.join(file)).expect("svg");
        let tree = usvg::Tree::from_data(&data, &usvg::Options::default()).expect("parse");
        let mut x = 8;
        let y = r as u32 * row_h + 8;
        for &z in &sizes {
            // Box-filtered downscale of the 72 px mask (what nav did).
            let lht = *name == "uturn_right";
            let down = icons::draw_scaled(*m, lht, z as usize, z as usize, 0, 0, z as usize);
            put(&mut img, &down.px, z, x, y, s);
            x += cell(z);
            let f = fit(&tree, z);
            put(&mut img, &f.px, z, x, y, s);
            x += cell(z);
            let txt: String =
                f.px.chunks(z as usize)
                    .map(|r| {
                        r.iter()
                            .map(|p| if *p { '#' } else { '.' })
                            .collect::<String>()
                            + "\n"
                    })
                    .collect();
            std::fs::write(out.join(format!("{name}_{z}.txt")), txt).expect("mask");
            if rust.as_ref().is_some_and(|(rz, _)| *rz == z) {
                let rows: Vec<String> =
                    f.px.chunks(z as usize)
                        .map(|r| {
                            let v = r
                                .iter()
                                .enumerate()
                                .fold(0u32, |v, (x, p)| v | (u32::from(*p) << (31 - x)));
                            format!("0x{v:08X}")
                        })
                        .collect();
                data_rows += &format!(
                    "    (\"{name}\", [{}]),
",
                    rows.join(", ")
                );
            }
            eprintln!(
                "{name} {z}px: scale {:.3} (stroke {:.2} px)",
                f.scale,
                2.5 * f.scale
            );
        }
    }
    if let Some((z, file)) = rust {
        let src = [
            format!("//! Lane arrows: Drew's Maps SVGs rendered at {z} px and grid-fitted by"),
            format!("//! `cargo run -p carpi-dis --example small_icons -- <svg dir> <out> --rust {z} <this file>`."),
            "//! Generated; regenerate rather than edit. Row bit `0x8000_0000 >> x`.".to_string(),
            String::new(),
            format!("pub const SIZE: usize = {z};"),
            String::new(),
            "#[rustfmt::skip]".to_string(),
            format!("pub const ICONS: &[(&str, [u32; {z}])] = &["),
            format!("{data_rows}];"),
            String::new(),
        ]
        .join("
");
        std::fs::write(&file, src).expect("rust");
        eprintln!("wrote {}", file.display());
    }
    let p: &Path = &out.join("small_icons.png");
    img.save(p).expect("png");
    println!("{}", p.display());
}
