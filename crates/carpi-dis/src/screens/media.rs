//! Media: title, artist, album, a progress bar and times. Two layouts, both on
//! trial (Drew: "try both"):
//!
//! - `Stack`: three centred text lines at y 1/11/21 across x 1..63, marquee
//!   when wider (the old page's lines, `dc/apps/media.py`).
//! - `Art`: a 48x48 px cover thumbnail at (0,3) logical, the three lines
//!   left-aligned in x 26..63 beside it. The thumbnail goes out as a logical
//!   base first, then refines natively in the refine lane.
//!
//! Both: progress bar x 2..62, y 33..36 (a 1-row track, a 3-row fill); times
//! y=39, position left, duration right, `II` between them while paused.
//!
//! Updates: a time tick re-sends one run (tail clear in the same group). The
//! bar only ever sends the part that changed: a grow is one fill; a shrink
//! (seek back, new track) is a clear plus the track row, one group.

use super::Screen;
use crate::cmd::Op;
use crate::font::{Font, TEXT_H, audscii};
use crate::helpers::{Ctx, QuickThenRefineIcon, ScrollText, TextSlot};
use crate::icons::Art;
use crate::pack::{Group, PackError, Packer};
use crate::proto::Rect;
use std::time::Instant;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MediaLayout {
    #[default]
    Stack,
    Art,
}

/// Inputs as carpi-core has them (`media.*`), plus the cover already dithered
/// to [`ART`] size (see [`crate::image::dither`]) with an id that changes with it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MediaInput {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub playing: bool,
    pub position_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    pub art: Option<(u64, Art)>,
    /// The same cover for the full-centre screen and the flash: 64x48 logical
    /// (`image::cover(.., 64, 48, ..)`) or 128x96 native. Same id as `art`.
    pub cover: Option<(u64, Art)>,
}

/// Centred on the three text rows beside it (y 2..29; Drew: it sat too high).
pub const ART: Rect = Rect::new(0, 3, 24, 24);
pub const BAR_X: u8 = 2;
pub const BAR_W: u8 = 60;
pub const BAR_Y: u8 = 33;
pub const TIME_Y: u8 = 39;

/// `m:ss`, or `h:mm:ss` from an hour.
pub fn clock(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Filled bar width, logical, 0..=BAR_W.
pub fn bar_fill(pos: Option<u64>, dur: Option<u64>) -> Option<u8> {
    let (p, d) = (pos?, dur.filter(|d| *d > 0)?);
    Some(((p.min(d) as f64 / d as f64) * f64::from(BAR_W)).round() as u8)
}

pub struct MediaScreen {
    pub layout: MediaLayout,
    input: MediaInput,
    lines: [ScrollText; 3],
    pos: TextSlot,
    dur: TextSlot,
    paused: TextSlot,
    /// Bar as drawn: `None` = not drawn, else the filled width.
    bar: Option<u8>,
    art: QuickThenRefineIcon<u64>,
}

impl MediaScreen {
    pub fn new(layout: MediaLayout) -> Self {
        let line = |y| {
            let mut s = match layout {
                MediaLayout::Stack => ScrollText::new(y, 1, 63, Font::Prop),
                MediaLayout::Art => ScrollText::new(y + 1, 26, 63, Font::Prop),
            };
            s.centred = layout == MediaLayout::Stack;
            s
        };
        MediaScreen {
            layout,
            input: MediaInput::default(),
            lines: [line(1), line(11), line(21)],
            pos: TextSlot::new(TIME_Y, 1, 26, Font::Prop),
            dur: TextSlot::new(TIME_Y, 38, 63, Font::Prop),
            paused: TextSlot::new(TIME_Y, 27, 37, Font::Prop),
            bar: None,
            art: {
                let mut q = QuickThenRefineIcon::new(ART);
                q.on_clear = true;
                q
            },
        }
    }

    pub fn set(&mut self, now: Instant, input: MediaInput) {
        let texts = [&input.title, &input.artist, &input.album];
        for (l, t) in self.lines.iter_mut().zip(texts) {
            l.set_text(now, t.as_deref().unwrap_or_default());
        }
        self.input = input;
    }

    fn time_groups(&mut self, cx: &Ctx) -> Vec<Group> {
        let m = &cx.metrics;
        let has_time = self.input.duration_ms.is_some_and(|d| d > 0);
        let p = if has_time {
            audscii(&clock(self.input.position_ms.unwrap_or(0)))
        } else {
            Vec::new()
        };
        let d = if has_time {
            audscii(&clock(self.input.duration_ms.unwrap_or(0)))
        } else {
            Vec::new()
        };
        let dx = 63 - (m.width(Font::Prop, &d).div_ceil(2) as u8).min(25);
        let pause = if has_time && !self.input.playing {
            audscii("II")
        } else {
            Vec::new()
        };
        let mut out: Vec<Group> = self.pos.draw(m, "pos", 1, p).into_iter().collect();
        out.extend(self.dur.draw(m, "dur", dx, d));
        out.extend(self.paused.draw_centred(m, "paused", pause));
        out
    }

    fn bar_groups(&mut self) -> Vec<Group> {
        let want = bar_fill(self.input.position_ms, self.input.duration_ms);
        if want == self.bar {
            return Vec::new();
        }
        let r = |x: u8, w: u8, y: u8, h: u8| Rect::new(BAR_X + x, y, w, h);
        let ops = match (self.bar, want) {
            (_, None) => vec![Op::Clear(r(0, BAR_W, BAR_Y, 3))],
            (None, Some(w)) => {
                let mut o = vec![Op::Fill(r(0, BAR_W, BAR_Y + 1, 1))];
                if w > 0 {
                    o.push(Op::Fill(r(0, w, BAR_Y, 3)));
                }
                o
            }
            (Some(old), Some(w)) if w > old => vec![Op::Fill(r(old, w - old, BAR_Y, 3))],
            (Some(old), Some(w)) => vec![
                Op::Clear(r(w, old - w, BAR_Y, 3)),
                Op::Fill(r(w, old - w, BAR_Y + 1, 1)),
            ],
        };
        self.bar = want;
        vec![Group::new("bar", ops)]
    }

    fn art_groups(&mut self, cx: &Ctx) -> Vec<Group> {
        if self.layout != MediaLayout::Art {
            return Vec::new();
        }
        match &self.input.art {
            Some((id, art)) if art.w == usize::from(ART.w) * 2 && art.h == usize::from(ART.h) * 2 => {
                // A new cover redraws the whole rect (Draw-mode base), so
                // nothing of the old one survives; skip blank rows only on enter.
                let (id, art) = (*id, art.clone());
                self.art.set(cx, id, &art)
            }
            _ => Vec::new(),
        }
    }
}

impl Screen for MediaScreen {
    fn enter(&mut self, now: Instant, cx: &Ctx) -> Vec<Group> {
        self.lines.iter_mut().for_each(ScrollText::reset);
        self.pos.reset();
        self.dur.reset();
        self.paused.reset();
        self.bar = None;
        self.art.reset();
        self.art.on_clear = true;
        let mut out = Vec::new();
        for (l, tag) in self.lines.iter_mut().zip(["title", "artist", "album"]) {
            out.extend(l.poll(now, cx, tag));
        }
        out.extend(self.bar_groups());
        out.extend(self.time_groups(cx));
        out.extend(self.art_groups(cx));
        self.art.on_clear = false;
        out
    }

    fn update(&mut self, now: Instant, cx: &Ctx) -> Vec<Group> {
        let mut out = self.time_groups(cx);
        out.extend(self.bar_groups());
        for (l, tag) in self.lines.iter_mut().zip(["title", "artist", "album"]) {
            out.extend(l.poll(now, cx, tag));
        }
        out.extend(self.art_groups(cx));
        out
    }

    fn refine(&mut self, _cx: &Ctx, p: &mut Packer, max_msgs: usize) -> Result<(), PackError> {
        self.art.refine(p, max_msgs).map(|_| ())
    }

    fn next_deadline(&self, cx: &Ctx) -> Option<Instant> {
        self.lines
            .iter()
            .filter_map(|l| l.next_deadline(&cx.metrics))
            .min()
    }
}

/// Text rows a layout uses, for tests and overlays.
pub fn line_rows(layout: MediaLayout) -> [u8; 3] {
    match layout {
        MediaLayout::Stack => [1, 11, 21],
        MediaLayout::Art => [2, 12, 22],
    }
}

const _: () = assert!(TIME_Y + TEXT_H <= 48);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_and_bar() {
        assert_eq!(clock(83_000), "1:23");
        assert_eq!(clock(3_725_000), "1:02:05");
        assert_eq!(bar_fill(Some(30_000), Some(60_000)), Some(30));
        assert_eq!(bar_fill(Some(90_000), Some(60_000)), Some(60));
        assert_eq!(bar_fill(Some(1), None), None);
    }
}
