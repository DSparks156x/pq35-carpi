//! Easter eggs: a short 1-bit animation across the centre, played over media
//! when the track matches (the old page, `dc/apps/easteregg.py`, `eggs.json`).
//!
//! Frames are 64x48 logical, made offline (`cargo run -p carpi-dis --example
//! eggs`) as an `.egg` file: `EGG1`, frame count (u16 LE), then per frame its
//! delay in ms (u16 LE) and 48 rows of 8 bytes, MSB-first. The caller matches
//! titles to eggs; this only plays one.
//!
//! Each frame sends only the rows that changed since the last one shown, as
//! opaque `55` records of up to 12 rows. The cluster applies each message as it
//! arrives, so a big change can tear across messages, like the old player.
//! When the link falls behind, frames are dropped to keep the clock (the frame
//! shown is the one due now), never queued. Delays under 66 ms are raised to
//! 66 (the old 15 fps cap).

use super::Screen;
use crate::cmd::{Op, Rop};
use crate::helpers::Ctx;
use crate::pack::Group;
use crate::proto::FULL;
use std::time::{Duration, Instant};

pub const W: usize = 64;
pub const H: usize = 48;
const ROW: usize = W / 8;
const MIN_DELAY: Duration = Duration::from_millis(66);

/// One frame: 48 rows of 8 bytes.
pub type Rows = Vec<[u8; ROW]>;

#[derive(Clone, Debug, PartialEq)]
pub struct Egg {
    pub frames: Vec<(Rows, Duration)>,
}

impl Egg {
    /// Parse an `.egg` file; `None` if it's malformed or empty.
    pub fn parse(b: &[u8]) -> Option<Egg> {
        if b.get(..4)? != b"EGG1" {
            return None;
        }
        let n = usize::from(u16::from_le_bytes([*b.get(4)?, *b.get(5)?]));
        let mut at = 6;
        let mut frames = Vec::with_capacity(n);
        for _ in 0..n {
            let d = u16::from_le_bytes([*b.get(at)?, *b.get(at + 1)?]);
            at += 2;
            let raw = b.get(at..at + ROW * H)?;
            at += ROW * H;
            let rows = raw
                .chunks(ROW)
                .map(|c| <[u8; ROW]>::try_from(c).expect("chunk"))
                .collect();
            frames.push((rows, Duration::from_millis(u64::from(d)).max(MIN_DELAY)));
        }
        (!frames.is_empty()).then_some(Egg { frames })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = b"EGG1".to_vec();
        out.extend((self.frames.len() as u16).to_le_bytes());
        for (rows, d) in &self.frames {
            out.extend((d.as_millis().min(u128::from(u16::MAX)) as u16).to_le_bytes());
            rows.iter().for_each(|r| out.extend(r));
        }
        out
    }

    fn length(&self) -> Duration {
        self.frames.iter().map(|(_, d)| *d).sum()
    }
}

pub struct EggScreen {
    egg: Egg,
    loops: u32,
    started: Option<Instant>,
    shown: Option<Rows>,
    /// When the next frame (or the end) is due.
    next_at: Option<Instant>,
}

impl EggScreen {
    /// Play `egg` `loops` times (at least once).
    pub fn new(egg: Egg, loops: u32) -> Self {
        EggScreen {
            egg,
            loops: loops.max(1),
            started: None,
            shown: None,
            next_at: None,
        }
    }

    /// The frame due at `now`, or `None` once all loops are over.
    fn due(&self, now: Instant) -> Option<(usize, Instant)> {
        let start = self.started?;
        let len = self.egg.length();
        let el = now.saturating_duration_since(start);
        if len.is_zero() || el >= len * self.loops {
            return None;
        }
        let into = Duration::from_nanos((el.as_nanos() % len.as_nanos()) as u64);
        let mut t = Duration::ZERO;
        for (i, (_, d)) in self.egg.frames.iter().enumerate() {
            if into < t + *d {
                return Some((i, now + (t + *d - into)));
            }
            t += *d;
        }
        None
    }

    /// Finished all loops (the caller hands the centre back).
    pub fn done(&self, now: Instant) -> bool {
        self.started.is_some() && self.due(now).is_none()
    }

    fn frame_groups(&mut self, i: usize) -> Vec<Group> {
        let rows = self.egg.frames[i].0.clone();
        // Rows to send: changed since the last frame, or (first frame on a
        // cleared centre) any with ink.
        let send: Vec<bool> = match &self.shown {
            Some(prev) => rows.iter().zip(prev).map(|(a, b)| a != b).collect(),
            None => rows.iter().map(|r| r.iter().any(|b| *b != 0)).collect(),
        };
        let mut out = Vec::new();
        let mut y = 0;
        while y < H {
            if !send[y] {
                y += 1;
                continue;
            }
            let start = y;
            while y < H && send[y] && y - start < 12 {
                y += 1;
            }
            out.push(Group::new(
                "egg",
                vec![Op::Bitmap {
                    clip: FULL,
                    mode: Rop::Draw,
                    y: start as u8,
                    raster: rows[start..y].concat(),
                }],
            ));
        }
        self.shown = Some(rows);
        out
    }
}

impl Screen for EggScreen {
    fn enter(&mut self, now: Instant, _cx: &Ctx) -> Vec<Group> {
        self.started.get_or_insert(now);
        self.shown = None;
        self.update(now, _cx)
    }

    fn update(&mut self, now: Instant, _cx: &Ctx) -> Vec<Group> {
        match self.due(now) {
            Some((i, next)) => {
                self.next_at = Some(next);
                self.frame_groups(i)
            }
            None => {
                self.next_at = None;
                Vec::new()
            }
        }
    }

    fn next_deadline(&self, _cx: &Ctx) -> Option<Instant> {
        self.next_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn egg(n: usize) -> Egg {
        Egg {
            frames: (0..n)
                .map(|i| {
                    let mut rows = vec![[0u8; ROW]; H];
                    rows[(i * 5) % H] = [0xFF; ROW];
                    (rows, Duration::from_millis(100))
                })
                .collect(),
        }
    }

    #[test]
    fn roundtrip_and_deltas() {
        let e = egg(3);
        assert_eq!(Egg::parse(&e.encode()), Some(e.clone()));
        let cx = Ctx::new(crate::proto::Profile::WHITE);
        let t0 = Instant::now();
        let mut s = EggScreen::new(e, 2);
        assert_eq!(s.enter(t0, &cx).len(), 1, "one inked row");
        // Next frame: old row cleared, new row drawn: two 1-row records.
        let g = s.update(t0 + Duration::from_millis(100), &cx);
        assert_eq!(g.len(), 2);
        assert!(!s.done(t0 + Duration::from_millis(599)));
        assert!(s.done(t0 + Duration::from_millis(600)));
    }
}
