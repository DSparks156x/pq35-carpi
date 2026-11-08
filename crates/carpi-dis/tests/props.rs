//! Property tests: whatever the groups, no message exceeds the profile limit, no
//! group straddles a commit (so the cluster buffer never fills), records stay
//! within their limits, and relative ops never run with an unknown clip or latch.
//! Hand-rolled xorshift; no extra dependency.

use carpi_dis::cmd::{Dir, Ink, Op, Rop, records, text_flags};
use carpi_dis::font::Font;
use carpi_dis::helpers::Ctx;
use carpi_dis::pack::{Group, PackError, Packed, Packer, Registers};
use carpi_dis::proto::{FULL, Profile, Rect};
use carpi_dis::screens::nav::{NavConfig, NavInput, NavScreen};
use carpi_dis::screens::readings::{ReadingSpec, ReadingsInput, ReadingsScreen};
use carpi_dis::screens::{Screen, build_frame};
use std::time::{Duration, Instant};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// Uniform in `lo..=hi`.
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo + 1)
    }
    fn u8(&mut self, lo: u8, hi: u8) -> u8 {
        self.range(u64::from(lo), u64::from(hi)) as u8
    }
    fn coin(&mut self, p: u64) -> bool {
        self.next() % 100 < p
    }
}

fn rect(r: &mut Rng) -> Rect {
    if r.coin(40) {
        return FULL;
    }
    let x = r.u8(0, 63);
    let y = r.u8(0, 47);
    Rect::new(x, y, r.u8(1, 64 - x), r.u8(1, 48 - y))
}

fn op(r: &mut Rng, p: &Profile) -> Op {
    let clip = rect(r);
    match r.range(0, 6) {
        0 => Op::Clear(clip),
        1 => Op::Fill(clip),
        2 | 3 => {
            let font = [Font::Fixed, Font::Prop, Font::Graphics][r.range(0, 2) as usize];
            let ink = [Ink::Opaque, Ink::Transparent, Ink::Black][r.range(0, 2) as usize];
            let n = r.range(1, p.max_glyphs as u64) as usize;
            Op::Text {
                clip,
                x: r.u8(0, clip.w - 1),
                y: r.u8(0, clip.h - 1),
                flags: text_flags(font, ink),
                bytes: (0..n).map(|_| r.u8(0x20, 0x7A)).collect(),
            }
        }
        4 => {
            let stride = usize::from(clip.w).div_ceil(8);
            let y = r.u8(0, clip.h - 1);
            let mut rows = if clip.w.is_multiple_of(8) {
                r.range(1, p.max_bitmap_rows as u64) as usize
            } else {
                1
            };
            rows = rows.min(usize::from(clip.h - y)).min(124 / stride).max(1);
            Op::Bitmap {
                clip,
                mode: [Rop::Draw, Rop::Xor, Rop::Set][r.range(0, 2) as usize],
                y,
                raster: (0..rows * stride).map(|_| r.next() as u8).collect(),
            }
        }
        k => {
            let dir = [Dir::V, Dir::H, Dir::Dot][r.range(0, 2) as usize];
            let x = r.u8(0, clip.w - 1);
            let y = r.u8(0, clip.h - 1);
            let room = match dir {
                Dir::V => clip.h - y,
                Dir::H => clip.w - x,
                Dir::Dot => 1,
            };
            let len = r.u8(1, room.min(15));
            if k == 5 {
                Op::Stroke {
                    clip,
                    x,
                    y,
                    dir,
                    len,
                }
            } else {
                Op::Line {
                    clip,
                    x,
                    y,
                    dir,
                    len,
                    rop: [Rop::Set, Rop::Xor][r.range(0, 1) as usize],
                }
            }
        }
    }
}

/// Every invariant the cluster needs from one packed frame.
fn check(p: &Packed, prof: &Profile, regs: Registers) {
    if p.messages.is_empty() {
        return;
    }
    let mut acc = 0usize;
    let mut clip_known = regs.clip.is_some();
    let mut latch_known = regs.rop.is_some();
    let n = p.messages.len();
    for (i, m) in p.messages.iter().enumerate() {
        assert!(
            !m.bytes.is_empty() && m.bytes.len() <= prof.max_msg,
            "msg {} B",
            m.bytes.len()
        );
        let recs = records(&m.bytes).expect("whole records");
        for (j, r) in recs.iter().enumerate() {
            match r[0] {
                0x39 => {
                    assert_eq!(j + 1, recs.len(), "records after a 39 in one message");
                    acc = 0;
                    continue;
                }
                0x52 => clip_known = true,
                0x55 => {
                    assert!(r[1] < 0x80, "55 LEN {:02X}", r[1]);
                    assert!(clip_known);
                    latch_known = true;
                }
                0x57 => {
                    assert!(r.len() - 5 <= prof.max_glyphs);
                    assert!(clip_known);
                }
                0x63 => assert!(clip_known && latch_known, "63 with unknown clip/latch"),
                op => panic!("unexpected record {op:02X}"),
            }
            acc += r.len();
            assert!(acc <= prof.cmd_buffer, "buffer would auto-commit: {acc} B");
        }
        if i + 1 == n {
            assert_eq!(*m.bytes.last().unwrap(), 0x39, "frame ends with 39");
        }
    }
    let mut last = (0, 0);
    for s in &p.spans {
        assert!((s.msg, s.start) >= last, "groups out of order");
        assert!(s.start < s.end && s.end <= p.messages[s.msg].bytes.len());
        let inner =
            records(&p.messages[s.msg].bytes[s.start..s.end]).expect("group = whole records");
        assert!(inner.iter().all(|r| r[0] != 0x39), "commit inside a group");
        last = (s.msg, s.end);
    }
}

#[test]
fn random_groups_never_break_limits() {
    let profiles = [
        Profile::WHITE,
        Profile::RED,
        Profile {
            cmd_buffer: 60,
            ..Profile::WHITE
        },
        Profile {
            cmd_buffer: 300,
            ..Profile::WHITE
        },
    ];
    let mut r = Rng(0x2545_F491_4F6C_DD1D);
    let mut packed_groups = 0;
    for prof in profiles {
        for _ in 0..1500 {
            let regs = Registers {
                clip: r.coin(30).then_some(FULL),
                rop: r.coin(30).then_some(Rop::Xor),
            };
            let mut p = Packer::new(prof, regs);
            for _ in 0..r.range(1, 12) {
                let ops = (0..r.range(1, 3)).map(|_| op(&mut r, &prof)).collect();
                let g = Group::new("g", ops);
                match p.push(&g) {
                    Ok(()) => packed_groups += 1,
                    Err(PackError::GroupTooBig(_, n)) => {
                        assert!(n > prof.max_msg.min(prof.cmd_buffer));
                    }
                    Err(e) => panic!("{e:?}"),
                }
            }
            check(&p.finish(), &prof, regs);
        }
    }
    assert!(packed_groups > 10_000);
}

#[test]
fn random_screen_inputs_never_break_limits() {
    let mut r = Rng(0x9E37_79B9_7F4A_7C15);
    let roads = [
        "",
        "A9",
        "Hauptstrasse",
        "A very long road name that has to scroll",
    ];
    let man = [
        "turn",
        "slight_turn",
        "u_turn",
        "roundabout_enter_and_exit",
        "destination",
        "x",
    ];
    for prof in [Profile::WHITE, Profile::RED] {
        let cx = Ctx::new(prof);
        let t0 = Instant::now();
        let mut nav = NavScreen::new(NavConfig::default());
        let specs = (0..8)
            .map(|i| ReadingSpec {
                key: format!("k{i}"),
                label: format!("L{i}"),
                decimals: (i % 3) as u8,
            })
            .collect();
        let mut rd = ReadingsScreen::new(specs);
        let mut regs = Registers::default();
        for step in 0..400u64 {
            let now = t0 + Duration::from_millis(step * 137);
            nav.set(
                now,
                NavInput {
                    maneuver: Some(man[r.range(0, 5) as usize].into()),
                    side: Some(if r.coin(50) { "left" } else { "right" }.into()),
                    roundabout_angle: Some(r.range(0, 359) as i32 - 180),
                    distance_m: r.coin(90).then(|| r.range(0, 20_000) as f64),
                    road: Some(roads[r.range(0, 3) as usize].into()),
                    lanes: Vec::new(),
                    ..Default::default()
                },
            );
            let mut values = [None; 8];
            for v in values.iter_mut() {
                *v = r.coin(85).then(|| r.range(0, 30_000) as f64 / 7.0 - 100.0);
            }
            rd.set(ReadingsInput {
                title: "Readings".into(),
                values,
            });
            let screen: &mut dyn Screen = if step % 50 < 25 { &mut nav } else { &mut rd };
            let enter = step % 25 == 0;
            let f = build_frame(screen, now, &cx, regs, enter).unwrap();
            check(&f, &prof, regs);
            if !f.messages.is_empty() {
                regs = f.regs;
            }
        }
    }
}
