//! Small reusable pieces screens build on. Each one remembers what it last sent
//! and returns [`Group`]s for the packer; none of them own an order or a frame.
//!
//! Clearing tricks (Drew): pad text with blank glyphs so the footprint stays
//! constant, and clear only the tail the old text leaves behind
//! (`dc/text_render.py:40-72`). A clear and its redraw are one group, so they
//! share a buffer. Blanks never reach past the slot's own edges.

use crate::cmd::{Dir, Ink, Op, text_flags};
use crate::font::{BLANK_NARROW, BLANK_WIDE, Font, Metrics, TEXT_H, audscii};
use crate::icons::Art;
use crate::pack::{Group, PackError, Packer};
use crate::proto::{CENTRE_H, FULL, Profile, Rect};
use std::time::{Duration, Instant};

/// What helpers need from the session.
#[derive(Clone, Copy, Debug)]
pub struct Ctx {
    pub profile: Profile,
    pub metrics: Metrics,
}

impl Ctx {
    pub fn new(profile: Profile) -> Self {
        Ctx {
            profile,
            metrics: Metrics::for_variant(profile.variant),
        }
    }

    /// Glyphs one text group may carry: room for two tail clears, a clip and the
    /// `57` header inside one message.
    pub fn text_cap(&self) -> usize {
        self.profile
            .max_glyphs
            .min(self.profile.max_msg.saturating_sub(7 * 3 + 5))
    }
}

/// One opaque text run in a horizontal slot `[left, right)` at row `y`, full
/// centre clip. Remembers where the last run left glyph ink and clears only ink
/// the new run doesn't paint over. Blank glyphs paint background, so a pad left
/// behind by an earlier run never needs a clear.
#[derive(Clone, Debug)]
pub struct TextSlot {
    pub y: u8,
    pub left: u8,
    pub right: u8,
    pub font: Font,
    /// Last run sent.
    prev: Option<Sent>,
}

/// `(x, bytes, ink [x0, x1) px)`.
type Sent = (u8, Vec<u8>, Option<(u16, u16)>);

fn is_blank(b: u8) -> bool {
    b == BLANK_WIDE || b == BLANK_NARROW
}

impl TextSlot {
    pub fn new(y: u8, left: u8, right: u8, font: Font) -> Self {
        assert!(
            left < right && right <= 64 && y < CENTRE_H,
            "slot outside centre"
        );
        TextSlot {
            y,
            left,
            right,
            font,
            prev: None,
        }
    }

    pub fn width_px(&self) -> u16 {
        u16::from(self.right - self.left) * 2
    }

    /// Forget what's on screen (after a clear or a new session).
    pub fn reset(&mut self) {
        self.prev = None;
    }

    /// Draw `bytes` at logical `x`. `None` if identical to what's there.
    pub fn draw(&mut self, m: &Metrics, tag: &'static str, x: u8, bytes: Vec<u8>) -> Option<Group> {
        if self
            .prev
            .as_ref()
            .is_some_and(|(px, pb, _)| *px == x && *pb == bytes)
        {
            return None;
        }
        let edge = u16::from(self.right) * 2;
        let x0 = u16::from(x) * 2;
        let mut at = x0;
        let mut ink: Option<(u16, u16)> = None;
        for b in &bytes {
            let next = (at + m.advance(self.font, *b)).min(edge);
            if !is_blank(*b) && next > at {
                ink = Some(ink.map_or((at, next), |(s, _)| (s, next)));
            }
            at = next;
        }
        let (n0, n1) = (x0, at);
        let h = TEXT_H.min(CENTRE_H - self.y);
        let clear =
            |a: u16, b: u16| Op::Clear(Rect::new((a / 2) as u8, self.y, ((b - a) / 2) as u8, h));
        let mut ops = Vec::new();
        if let Some((_, _, Some((i0, i1)))) = self.prev {
            if i0 < n0.min(i1) {
                ops.push(clear(i0, n0.min(i1)));
            }
            if i1 > n1.max(i0) {
                ops.push(clear(n1.max(i0), i1));
            }
        }
        if !bytes.is_empty() {
            ops.push(Op::Text {
                clip: FULL,
                x,
                y: self.y,
                flags: text_flags(self.font, Ink::Opaque),
                bytes: bytes.clone(),
            });
        }
        self.prev = Some((x, bytes, ink));
        (!ops.is_empty()).then(|| Group::new(tag, ops))
    }
}

/// Right-aligned value in a fixed slot. Left-padded with blanks so the run
/// always covers the slot: a change re-sends its own run and never clears.
/// Too wide for the slot shows `--` (old rule, `dc/apps/readings.py:250-256`).
#[derive(Clone, Debug)]
pub struct NumberField {
    pub slot: TextSlot,
}

impl NumberField {
    pub fn new(y: u8, left: u8, right: u8, font: Font) -> Self {
        NumberField {
            slot: TextSlot::new(y, left, right, font),
        }
    }

    pub fn reset(&mut self) {
        self.slot.reset();
    }

    pub fn set(&mut self, cx: &Ctx, tag: &'static str, text: &str) -> Option<Group> {
        let m = &cx.metrics;
        let s = &self.slot;
        let mut bytes = audscii(text);
        if m.width(s.font, &bytes) > s.width_px() || bytes.len() > cx.text_cap() {
            bytes = audscii("--");
        }
        let d = m.width(s.font, &bytes);
        let right_px = u16::from(s.right) * 2;
        // Leftmost start whose gap blanks can fill exactly.
        let mut pick = None;
        for x in s.left..s.right {
            let Some(gap) = right_px.checked_sub(u16::from(x) * 2 + d) else {
                break;
            };
            if let Some(mut pad) = m.blanks(s.font, gap)
                && pad.len() + bytes.len() <= cx.text_cap()
            {
                pad.extend_from_slice(&bytes);
                pick = Some((x, pad));
                break;
            }
        }
        let (x, run) =
            pick.unwrap_or_else(|| ((right_px - d.min(right_px)).div_ceil(2) as u8, bytes));
        self.slot.draw(m, tag, x, run)
    }
}

/// Marquee over a long string. Continuous, one blank gap, one char per step.
/// Speed from Drew's config: 400 ms per step, no pauses
/// (`RNS-E-Hudiy/config.json:176-183`; stepping `dc/apps/base.py:197-247`).
#[derive(Clone, Debug)]
pub struct ScrollText {
    pub slot: TextSlot,
    pub step: Duration,
    /// Hold at offset 0 before scrolling (Drew: 0).
    pub pause: Duration,
    text: Vec<u8>,
    offset: usize,
    next_at: Option<Instant>,
}

impl ScrollText {
    pub fn new(y: u8, left: u8, right: u8, font: Font) -> Self {
        ScrollText {
            slot: TextSlot::new(y, left, right, font),
            step: Duration::from_millis(400),
            pause: Duration::ZERO,
            text: Vec::new(),
            offset: 0,
            next_at: None,
        }
    }

    pub fn reset(&mut self) {
        self.slot.reset();
    }

    /// New text restarts the marquee.
    pub fn set_text(&mut self, now: Instant, text: &str) {
        let b = audscii(text);
        if b != self.text {
            self.text = b;
            self.offset = 0;
            self.next_at = Some(now + self.pause + self.step);
        }
    }

    fn fits(&self, m: &Metrics) -> bool {
        m.width(self.slot.font, &self.text) <= self.slot.width_px()
    }

    pub fn next_deadline(&self, m: &Metrics) -> Option<Instant> {
        if self.fits(m) { None } else { self.next_at }
    }

    /// The run for now: a step if due, or whatever changed. `None` if nothing to send.
    pub fn poll(&mut self, now: Instant, cx: &Ctx, tag: &'static str) -> Option<Group> {
        let m = &cx.metrics;
        let font = self.slot.font;
        let field = self.slot.width_px();
        let window: Vec<u8> = if self.fits(m) {
            self.text.clone()
        } else {
            if self.next_at.is_some_and(|t| now >= t) {
                self.offset = (self.offset + 1) % (self.text.len() + 1);
                let mut next = now + self.step;
                if self.offset == 0 {
                    next += self.pause;
                }
                self.next_at = Some(next);
            }
            let mut ring = self.text.clone();
            ring.push(BLANK_WIDE);
            let tail: Vec<u8> = ring
                .iter()
                .cycle()
                .skip(self.offset)
                .take(ring.len())
                .copied()
                .collect();
            let n = m.fit(font, &tail, field).min(cx.text_cap());
            tail[..n].to_vec()
        };
        let mut run = window;
        let used = m.width(font, &run);
        let mut pad = m.fill_blanks(font, field.saturating_sub(used));
        pad.truncate(cx.text_cap().saturating_sub(run.len()));
        run.extend(pad);
        self.slot.draw(m, tag, self.slot.left, run)
    }
}

/// Graphics-font cells at fixed anchors (approach bar, lane arrows). Only
/// changed cells go out, 6 B each (`57 04 0A x y g`, stock bar, bench).
#[derive(Clone, Debug)]
pub struct GlyphCells {
    pub cells: Vec<(u8, u8)>,
    cur: Vec<Option<u8>>,
}

impl GlyphCells {
    pub fn new(cells: Vec<(u8, u8)>) -> Self {
        let n = cells.len();
        GlyphCells {
            cells,
            cur: vec![None; n],
        }
    }

    pub fn reset(&mut self) {
        self.cur.iter_mut().for_each(|c| *c = None);
    }

    pub fn set(&mut self, tag: &'static str, glyphs: &[u8]) -> Vec<Group> {
        let mut out = Vec::new();
        for (i, g) in glyphs.iter().enumerate().take(self.cells.len()) {
            if self.cur[i] == Some(*g) {
                continue;
            }
            self.cur[i] = Some(*g);
            let (x, y) = self.cells[i];
            out.push(Group::new(
                tag,
                vec![Op::Text {
                    clip: FULL,
                    x,
                    y,
                    flags: text_flags(Font::Graphics, Ink::Opaque),
                    bytes: vec![*g],
                }],
            ));
        }
        out
    }
}

/// Native refine pacing: at least 10 ms between stroke messages. Bench.
pub const STROKE_GAP: Duration = Duration::from_millis(10);

/// Icon with a fast logical base and an optional native refine job.
///
/// The base is opaque `55` rows covering the whole rect, so it needs no clear and
/// overwrites any half-done refine. The refine job is base + XOR V/H/dot planes
/// (`dc/native_bitmap.py`, bench), one selector+stroke per group, run in the
/// refine lane. A new icon bumps the version; the old job is dropped before the
/// next frame is built, so a cancel never splits a message.
#[derive(Clone, Debug)]
pub struct QuickThenRefineIcon<K> {
    pub rect: Rect,
    shown: Option<K>,
    version: u32,
    job: Option<RefineJob>,
}

#[derive(Clone, Debug)]
struct RefineJob {
    version: u32,
    strokes: Vec<Op>,
    next: usize,
}

impl<K: PartialEq + Clone> QuickThenRefineIcon<K> {
    pub fn new(rect: Rect) -> Self {
        QuickThenRefineIcon {
            rect,
            shown: None,
            version: 0,
            job: None,
        }
    }

    pub fn reset(&mut self) {
        self.shown = None;
        self.job = None;
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn refining(&self) -> bool {
        self.job.as_ref().is_some_and(|j| j.version == self.version)
    }

    /// Base groups for `key`, or nothing if it's already shown. `art` must be
    /// `rect.w*2` x `rect.h*2` px.
    pub fn set(&mut self, cx: &Ctx, key: K, art: &Art) -> Vec<Group> {
        if self.shown.as_ref() == Some(&key) {
            return Vec::new();
        }
        assert_eq!(
            (art.w, art.h),
            (usize::from(self.rect.w) * 2, usize::from(self.rect.h) * 2)
        );
        self.shown = Some(key);
        self.version = self.version.wrapping_add(1);
        let p = &cx.profile;
        let rows = art.base_rows();
        let stride = usize::from(self.rect.w).div_ceil(8);
        let per = if !self.rect.w.is_multiple_of(8) {
            1
        } else {
            p.max_bitmap_rows
                .min((0x7F - 3) / stride)
                .min((p.max_msg.min(p.cmd_buffer) - 5 - 7) / stride)
                .max(1)
        };
        let groups = rows
            .chunks(per)
            .enumerate()
            .map(|(i, chunk)| {
                Group::new(
                    "icon-base",
                    vec![Op::Bitmap {
                        clip: self.rect,
                        mode: crate::cmd::Rop::Draw,
                        y: (i * per) as u8,
                        raster: chunk.concat(),
                    }],
                )
            })
            .collect();
        self.job = p.hires.then(|| RefineJob {
            version: self.version,
            strokes: strokes(self.rect, art),
            next: 0,
        });
        groups
    }

    /// Push refine strokes until the packer would exceed `max_msgs` messages.
    /// Returns the number pushed.
    pub fn refine(&mut self, p: &mut Packer, max_msgs: usize) -> Result<usize, PackError> {
        let Some(job) = &mut self.job else {
            return Ok(0);
        };
        if job.version != self.version {
            self.job = None;
            return Ok(0);
        }
        let mut n = 0;
        while job.next < job.strokes.len() {
            let g = Group::new("refine", vec![job.strokes[job.next].clone()]).gap(STROKE_GAP);
            if p.count_after(&g)? > max_msgs {
                break;
            }
            p.push(&g)?;
            job.next += 1;
            n += 1;
        }
        if job.next == job.strokes.len() {
            self.job = None;
        }
        Ok(n)
    }
}

/// XOR planes over a bottom-right base: V where BL^BR, H where TR^BR, a dot
/// where TL^TR^BL^BR. Runs capped at 15. `dc/native_bitmap.py` model, bench.
fn strokes(rect: Rect, art: &Art) -> Vec<Op> {
    let (lw, lh) = (usize::from(rect.w), usize::from(rect.h));
    let cell = |lx: usize, ly: usize| {
        let g = |dx, dy| art.get(lx * 2 + dx, ly * 2 + dy);
        (g(0, 0), g(1, 0), g(0, 1), g(1, 1))
    };
    let mut v = vec![vec![false; lw]; lh];
    let mut h = vec![vec![false; lw]; lh];
    let mut d = Vec::new();
    for ly in 0..lh {
        for lx in 0..lw {
            let (tl, tr, bl, br) = cell(lx, ly);
            v[ly][lx] = bl ^ br;
            h[ly][lx] = tr ^ br;
            if tl ^ tr ^ bl ^ br {
                d.push((lx, ly));
            }
        }
    }
    let mut out = Vec::new();
    let mut runs = |n_outer: usize, n_inner: usize, at: &dyn Fn(usize, usize) -> bool, dir: Dir| {
        for o in 0..n_outer {
            let mut i = 0;
            while i < n_inner {
                if !at(o, i) {
                    i += 1;
                    continue;
                }
                let start = i;
                while i < n_inner && at(o, i) && i - start < 15 {
                    i += 1;
                }
                let (x, y) = if dir == Dir::V {
                    (o, start)
                } else {
                    (start, o)
                };
                out.push(Op::Stroke {
                    clip: rect,
                    x: x as u8,
                    y: y as u8,
                    dir,
                    len: (i - start) as u8,
                });
            }
        }
    };
    runs(lw, lh, &|x, y| v[y][x], Dir::V);
    runs(lh, lw, &|y, x| h[y][x], Dir::H);
    out.extend(d.into_iter().map(|(x, y)| Op::Stroke {
        clip: rect,
        x: x as u8,
        y: y as u8,
        dir: Dir::Dot,
        len: 1,
    }));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cx() -> Ctx {
        Ctx::new(Profile::WHITE)
    }

    #[test]
    fn number_field_keeps_footprint_and_never_clears() {
        let c = cx();
        let mut f = NumberField::new(10, 33, 56, Font::Prop);
        let a = f.set(&c, "dist", "350").unwrap();
        let b = f.set(&c, "dist", "50").unwrap();
        for g in [&a, &b] {
            assert_eq!(g.ops.len(), 1, "{g:?}");
            assert!(matches!(g.ops[0], Op::Text { .. }));
        }
        assert!(f.set(&c, "dist", "50").is_none());
    }

    #[test]
    fn text_slot_clears_only_the_tail() {
        let c = cx();
        let mut s = TextSlot::new(0, 0, 64, Font::Prop);
        s.draw(&c.metrics, "t", 0, audscii("Hello"));
        let g = s.draw(&c.metrics, "t", 0, audscii("Hi")).unwrap();
        assert_eq!(g.ops.len(), 2);
        assert!(matches!(g.ops[0], Op::Clear(r) if r.y == 0 && r.h == TEXT_H));
    }

    #[test]
    fn scroll_steps_on_time() {
        let c = cx();
        let t0 = Instant::now();
        let mut s = ScrollText::new(39, 0, 64, Font::Prop);
        s.set_text(t0, "Very long road name that will not fit");
        assert!(s.poll(t0, &c, "road").is_some());
        assert!(s.poll(t0, &c, "road").is_none());
        let t1 = s.next_deadline(&c.metrics).unwrap();
        assert_eq!(t1 - t0, Duration::from_millis(400));
        assert!(s.poll(t1, &c, "road").is_some());
    }
}
