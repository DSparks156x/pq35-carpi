//! Cover art across the whole centre: on its own, or flashed for a few
//! seconds when the track's cover changes (the old page's option).
//!
//! The cover comes in either as 64x48 logical art (the old tester's target)
//! or 128x96 native. Logical art is doubled to 128x96, so its `55` base is
//! already exact and the refine lane has nothing to do: ~630 B, about seven
//! messages. Native art gets the same base plus native refine strokes, which
//! for a dithered photo is many seconds of traffic. The flash uses native:
//! Drew's old page let the high-res draw finish and counted the hold from
//! there (`Dis` does the same).

use super::Screen;
use crate::helpers::{Ctx, QuickThenRefineIcon};
use crate::icons::Art;
use crate::pack::{Group, PackError, Packer};
use crate::proto::FULL;
use std::time::Instant;

/// Logical size of a cover made for this screen.
pub const LOGICAL: (usize, usize) = (64, 48);

/// 64x48 logical art doubled to 128x96 px.
pub fn double(a: &Art) -> Art {
    let (w, h) = (a.w * 2, a.h * 2);
    let px = (0..w * h).map(|i| a.get((i % w) / 2, (i / w) / 2)).collect();
    Art { w, h, px }
}

pub struct CoverScreen {
    cover: Option<(u64, Art)>,
    image: QuickThenRefineIcon<u64>,
}

impl Default for CoverScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl CoverScreen {
    pub fn new() -> Self {
        let mut image = QuickThenRefineIcon::new(FULL);
        image.on_clear = true;
        CoverScreen { cover: None, image }
    }

    /// `art` is 64x48 (logical) or 128x96 (native); anything else is ignored.
    pub fn set(&mut self, cover: Option<(u64, Art)>) {
        self.cover = cover.and_then(|(id, a)| match (a.w, a.h) {
            (64, 48) => Some((id, double(&a))),
            (128, 96) => Some((id, a)),
            _ => None,
        });
    }

    /// Native refine still has strokes to send.
    pub fn refining(&self) -> bool {
        self.image.refining()
    }

    pub fn id(&self) -> Option<u64> {
        self.cover.as_ref().map(|(id, _)| *id)
    }

    fn groups(&mut self, cx: &Ctx) -> Vec<Group> {
        match self.cover.clone() {
            Some((id, art)) => self.image.set(cx, id, &art),
            None => Vec::new(),
        }
    }
}

impl Screen for CoverScreen {
    fn enter(&mut self, _now: Instant, cx: &Ctx) -> Vec<Group> {
        self.image.reset();
        self.image.on_clear = true;
        let g = self.groups(cx);
        self.image.on_clear = false;
        g
    }

    fn update(&mut self, _now: Instant, cx: &Ctx) -> Vec<Group> {
        self.groups(cx)
    }

    fn refine(&mut self, _cx: &Ctx, p: &mut Packer, max_msgs: usize) -> Result<(), PackError> {
        self.image.refine(p, max_msgs).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubling_is_exact_so_nothing_refines() {
        let a = Art {
            w: 64,
            h: 48,
            px: (0..64 * 48).map(|i| (i * 7) % 3 == 0).collect(),
        };
        let d = double(&a);
        assert_eq!((d.w, d.h), (128, 96));
        assert!(d.get(3, 5) == a.get(1, 2) && d.get(127, 95) == a.get(63, 47));
        let mut s = CoverScreen::new();
        s.set(Some((1, a)));
        let cx = Ctx::new(crate::proto::Profile::WHITE);
        let g = s.enter(Instant::now(), &cx);
        assert!(!g.is_empty());
        assert!(!s.image.refining(), "doubled art needs no refine");
    }
}
