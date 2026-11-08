//! Screen goldens (bytes and what we believe is on screen) and the nav refine lane.

mod common;

use carpi_dis::cmd::{Op, records};
use carpi_dis::fb::Fb;
use carpi_dis::helpers::Ctx;
use carpi_dis::icons::{Maneuver, Side};
use carpi_dis::pack::{Packed, Packer, Registers};
use carpi_dis::proto::Profile;
use carpi_dis::screens::nav::{ICON, NavConfig, NavInput, NavScreen, icon_art};
use carpi_dis::screens::readings::{ReadingSpec, ReadingsInput, ReadingsScreen};
use carpi_dis::screens::{REFINE_MSGS, Screen, build_frame};
use common::{apply, dump, golden};
use std::time::{Duration, Instant};

fn nav_input(maneuver: &str, side: &str, d: f64, road: &str) -> NavInput {
    NavInput {
        maneuver: Some(maneuver.into()),
        side: Some(side.into()),
        roundabout_angle: None,
        distance_m: Some(d),
        road: Some(road.into()),
    }
}

/// Pack only the update lane (no refine slice).
fn update_only(s: &mut dyn Screen, now: Instant, cx: &Ctx, regs: Registers) -> Packed {
    let mut p = Packer::new(cx.profile, regs);
    for g in s.update(now, cx) {
        p.push(&g).unwrap();
    }
    p.finish()
}

fn readings() -> ReadingsScreen {
    let spec = |k: &str, l: &str, d| ReadingSpec {
        key: k.into(),
        label: l.into(),
        decimals: d,
    };
    ReadingsScreen::new(vec![
        spec("engine.oil_temp_c", "Oil", 0),
        spec("engine.coolant_c", "H2O", 0),
        spec("engine.boost_bar", "Bst", 2),
        spec("engine.iat_c", "IAT", 0),
        spec("battery.v", "Bat", 1),
        spec("engine.rpm", "RPM", 0),
        spec("haldex.lock_pct", "Hdx", 0),
        spec("engine.afr", "AFR", 1),
    ])
}

#[test]
fn nav_enter_and_distance_tick() {
    let cx = Ctx::new(Profile::WHITE);
    let t0 = Instant::now();
    let mut s = NavScreen::new(NavConfig::default());
    s.set(t0, nav_input("turn", "left", 347.0, "Hauptstrasse"));
    let enter = build_frame(&mut s, t0, &cx, Registers::default(), true).unwrap();
    golden("nav_enter_bytes", &dump(&enter));
    let mut fb = Fb::new(cx.metrics);
    apply(&mut fb, &enter);
    golden("nav_enter_screen", &fb.ascii());
    // Icon first after the clear, then text, then bar (Drew's order).
    let tags: Vec<&str> = enter.spans.iter().map(|g| g.tag).collect();
    let pos = |t| tags.iter().position(|x| *x == t).unwrap();
    assert!(
        pos("icon-base") < pos("dist") && pos("dist") < pos("road") && pos("road") < pos("bar")
    );
    assert!(
        enter.messages.iter().all(|m| m.gap.is_zero()),
        "no strokes in enter"
    );

    // Distance tick far out: one padded run, two CAN frames at most.
    let mut s2 = NavScreen::new(NavConfig::default());
    s2.set(t0, nav_input("turn", "left", 820.0, "Hauptstrasse"));
    let e2 = build_frame(&mut s2, t0, &cx, Registers::default(), true).unwrap();
    s2.set(t0, nav_input("turn", "left", 790.0, "Hauptstrasse"));
    let tick = update_only(&mut s2, t0, &cx, e2.regs);
    golden("nav_distance_tick_bytes", &dump(&tick));
    assert_eq!(tick.spans.len(), 1);
    assert!(tick.bytes() <= 14, "{} B", tick.bytes());
}

#[test]
fn nav_refine_completes_to_native_art() {
    let cx = Ctx::new(Profile::WHITE);
    let t0 = Instant::now();
    let mut s = NavScreen::new(NavConfig::default());
    s.set(
        t0,
        nav_input("roundabout_enter_and_exit", "right", 600.0, "A9"),
    );
    let f = build_frame(&mut s, t0, &cx, Registers::default(), true).unwrap();
    let mut fb = Fb::new(cx.metrics);
    apply(&mut fb, &f);
    let mut regs = f.regs;
    let mut frames = 0;
    while s.refining() {
        let f = build_frame(&mut s, t0, &cx, regs, false).unwrap();
        assert!(f.messages.len() <= REFINE_MSGS + 1, "slice too big");
        for m in &f.messages {
            if records(&m.bytes).unwrap().iter().any(|r| r[0] == 0x63) {
                assert_eq!(m.gap, Duration::from_millis(10));
            }
        }
        apply(&mut fb, &f);
        regs = f.regs;
        frames += 1;
        assert!(frames < 500);
    }
    let art = icon_art(Maneuver::Roundabout(None), false);
    assert_eq!(fb.region(ICON), art.px, "refined icon != native art");
    golden("nav_refined_screen", &fb.ascii());
}

#[test]
fn nav_refine_cancels_on_maneuver_change() {
    let cx = Ctx::new(Profile::WHITE);
    let t0 = Instant::now();
    let mut s = NavScreen::new(NavConfig::default());
    s.set(
        t0,
        nav_input("roundabout_enter_and_exit", "right", 600.0, "A9"),
    );
    let f = build_frame(&mut s, t0, &cx, Registers::default(), true).unwrap();
    let mut fb = Fb::new(cx.metrics);
    apply(&mut fb, &f);
    let mut regs = f.regs;
    for _ in 0..2 {
        let f = build_frame(&mut s, t0, &cx, regs, false).unwrap();
        apply(&mut fb, &f);
        regs = f.regs;
    }
    assert!(s.refining(), "job A should still be mid-way");
    let v = s.icon_version();

    s.set(t0, nav_input("turn", "right", 600.0, "A9"));
    let f = build_frame(&mut s, t0, &cx, regs, false).unwrap();
    assert_eq!(s.icon_version(), v + 1);
    // The new base comes before any stroke, and covers the whole icon.
    let recs: Vec<Vec<u8>> = f
        .messages
        .iter()
        .flat_map(|m| records(&m.bytes).unwrap().into_iter().map(<[u8]>::to_vec))
        .collect();
    let first_base = recs.iter().position(|r| r[0] == 0x55 && r[2] == 2).unwrap();
    let first_stroke = recs.iter().position(|r| r[0] == 0x63);
    assert!(first_stroke.is_none_or(|i| i > first_base));
    apply(&mut fb, &f);
    regs = f.regs;
    while s.refining() {
        let f = build_frame(&mut s, t0, &cx, regs, false).unwrap();
        apply(&mut fb, &f);
        regs = f.regs;
    }
    let art = icon_art(Maneuver::Turn(Side::Right), false);
    assert_eq!(fb.region(ICON), art.px, "leftovers from job A");
}

#[test]
fn red_cluster_never_refines() {
    let cx = Ctx::new(Profile::RED);
    let t0 = Instant::now();
    let mut s = NavScreen::new(NavConfig::default());
    s.set(t0, nav_input("turn", "left", 300.0, "A9"));
    let f = build_frame(&mut s, t0, &cx, Registers::default(), true).unwrap();
    assert!(f.messages.iter().all(|m| m.bytes.len() <= 42));
    assert!(!s.refining());
}

#[test]
fn readings_enter_and_value_change() {
    let cx = Ctx::new(Profile::WHITE);
    let t0 = Instant::now();
    let mut s = readings();
    let mut input = ReadingsInput {
        title: "Engine".into(),
        values: [
            Some(92.0),
            Some(88.0),
            Some(1.234),
            Some(31.0),
            Some(14.1),
            Some(2450.0),
            None,
            Some(14.7),
        ],
    };
    s.set(input.clone());
    let enter = build_frame(&mut s, t0, &cx, Registers::default(), true).unwrap();
    golden("readings_enter_bytes", &dump(&enter));
    let mut fb = Fb::new(cx.metrics);
    apply(&mut fb, &enter);
    golden("readings_enter_screen", &fb.ascii());

    input.values[0] = Some(103.0);
    s.set(input.clone());
    let upd = update_only(&mut s, t0, &cx, enter.regs);
    golden("readings_value_bytes", &dump(&upd));
    // One centred run; the tail it uncovers is cleared in the same message.
    assert_eq!(upd.messages.len(), 1);
    assert!(s_value_ops(&upd) <= 3, "run + at most two tail clears");
    apply(&mut fb, &upd);
    golden("readings_value_screen", &fb.ascii());

    // Same width: just the run.
    input.values[0] = Some(104.0);
    s.set(input);
    let upd = update_only(&mut s, t0, &cx, upd.regs);
    assert_eq!(s_value_ops(&upd), 1);
}

#[test]
fn readings_icons_refine_to_art() {
    use carpi_dis::screens::readings::{icon_art, icon_rect, unit_art, unit_rect};
    let cx = Ctx::new(Profile::WHITE);
    let t0 = Instant::now();
    let mut s = readings();
    s.set(ReadingsInput::default());
    let f = build_frame(&mut s, t0, &cx, Registers::default(), true).unwrap();
    let mut fb = Fb::new(cx.metrics);
    apply(&mut fb, &f);
    let mut regs = f.regs;
    for _ in 0..500 {
        let f = build_frame(&mut s, t0, &cx, regs, false).unwrap();
        if f.messages.iter().all(|m| m.bytes == [0x39]) || f.messages.is_empty() {
            break;
        }
        apply(&mut fb, &f);
        regs = f.regs;
    }
    let page = s.page().clone();
    for (i, c) in page.cells.iter().enumerate() {
        assert_eq!(fb.region(icon_rect(i)), icon_art(i, c).px, "icon {i}");
        assert_eq!(fb.region(unit_rect(i)), unit_art(i, c).px, "unit {i}");
    }
    golden("readings_refined_screen", &fb.ascii());
}

#[test]
fn readings_focus_inverts_title_in_one_group() {
    let cx = Ctx::new(Profile::WHITE);
    let t0 = Instant::now();
    let mut s = readings();
    s.set(ReadingsInput {
        title: "Engine".into(),
        ..Default::default()
    });
    let f = build_frame(&mut s, t0, &cx, Registers::default(), true).unwrap();
    s.set_focus(true);
    let on = update_only(&mut s, t0, &cx, f.regs);
    assert_eq!(on.messages.len(), 1);
    let recs: Vec<u8> = records(&on.messages[0].bytes).unwrap().iter().map(|r| r[0]).collect();
    // clear, fill, black text (and the clip restore / commit around them).
    assert!(recs.windows(3).any(|w| w == [0x52, 0x52, 0x57]), "{recs:02X?}");
    s.set_focus(false);
    let off = update_only(&mut s, t0, &cx, on.regs);
    assert_eq!(off.messages.len(), 1);
    assert!(update_only(&mut s, t0, &cx, off.regs).messages.iter().all(|m| m.bytes == [0x39]));
}

/// Records in the frame other than the trailing commit.
fn s_value_ops(p: &Packed) -> usize {
    p.messages
        .iter()
        .flat_map(|m| records(&m.bytes).unwrap())
        .filter(|r| r[0] != 0x39)
        .count()
}

#[test]
fn enter_starts_with_full_clear() {
    let cx = Ctx::new(Profile::WHITE);
    let mut s = readings();
    let f = build_frame(&mut s, Instant::now(), &cx, Registers::default(), true).unwrap();
    assert_eq!(
        &f.messages[0].bytes[..7],
        &Op::Clear(carpi_dis::proto::FULL).encode()[..]
    );
}

fn media_input(pos_s: u64, playing: bool) -> carpi_dis::screens::media::MediaInput {
    use carpi_dis::screens::media::{ART, MediaInput};
    let n = 48usize;
    let g: Vec<u8> = (0..n * n).map(|i| ((i % n) * 255 / n) as u8).collect();
    let art = carpi_dis::image::dither(&g, n, n, usize::from(ART.w) * 2, usize::from(ART.h) * 2);
    MediaInput {
        title: Some("Title".into()),
        artist: Some("Artist".into()),
        album: Some("Album".into()),
        playing,
        position_ms: Some(pos_s * 1000),
        duration_ms: Some(200_000),
        art: Some((1, art)),
    }
}

#[test]
fn media_layouts_enter_tick_and_seek() {
    use carpi_dis::screens::media::{MediaLayout, MediaScreen};
    for (layout, name) in [(MediaLayout::Stack, "media_stack"), (MediaLayout::Art, "media_art")] {
        let cx = Ctx::new(Profile::WHITE);
        let t0 = Instant::now();
        let mut s = MediaScreen::new(layout);
        s.set(t0, media_input(100, true));
        let f = build_frame(&mut s, t0, &cx, Registers::default(), true).unwrap();
        let mut fb = Fb::new(cx.metrics);
        apply(&mut fb, &f);
        let mut regs = f.regs;
        while {
            let f = build_frame(&mut s, t0, &cx, regs, false).unwrap();
            let busy = f.messages.iter().any(|m| m.bytes != [0x39]);
            apply(&mut fb, &f);
            regs = f.regs;
            busy
        } {}
        golden(&format!("{name}_screen"), &fb.ascii());
        // One second on: a time run and maybe a bar grow, nothing else.
        s.set(t0, media_input(101, true));
        let u = update_only(&mut s, t0, &cx, regs);
        assert_eq!(u.messages.len(), 1, "{name}");
        // Seek back: a clear + the track row, one group, one message.
        s.set(t0, media_input(20, false));
        let u = update_only(&mut s, t0, &cx, u.regs);
        assert_eq!(u.messages.len(), 1, "{name}");
    }
}
