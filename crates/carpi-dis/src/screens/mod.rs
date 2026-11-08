//! Hand-written screens. Each one decides its own order and what to re-send;
//! the shared layer only packs.
//!
//! Two lanes per frame: the update lane (cheap slot re-sends) always goes
//! first; the refine lane (native strokes) fills at most [`REFINE_MSGS`] more
//! messages and is cancelled when its slot changes (DIS.md §2.3).

pub mod nav;
pub mod readings;

use crate::cmd::Op;
use crate::helpers::Ctx;
use crate::pack::{Group, PackError, Packed, Packer, Registers};
use crate::proto::FULL;
use std::time::Instant;

/// Refine messages per frame (DIS.md §2.4: slices of <=3 messages).
pub const REFINE_MSGS: usize = 3;

pub trait Screen {
    /// Full paint onto a just-cleared centre, in the screen's order.
    fn enter(&mut self, now: Instant, cx: &Ctx) -> Vec<Group>;
    /// Update lane: only what changed since the last frame.
    fn update(&mut self, now: Instant, cx: &Ctx) -> Vec<Group>;
    /// Refine lane: push into `p` while it stays within `max_msgs` messages.
    fn refine(&mut self, _cx: &Ctx, _p: &mut Packer, _max_msgs: usize) -> Result<(), PackError> {
        Ok(())
    }
    /// Next time `update` has something to do without new input (scrolling).
    fn next_deadline(&self, _cx: &Ctx) -> Option<Instant> {
        None
    }
}

/// Build one frame. `enter` clears the centre first; then the screen's groups,
/// then (update frames only) a refine slice.
pub fn build_frame(
    s: &mut dyn Screen,
    now: Instant,
    cx: &Ctx,
    regs: Registers,
    enter: bool,
) -> Result<Packed, PackError> {
    let mut p = Packer::new(cx.profile, regs);
    if enter {
        p.push(&Group::new("clear", vec![Op::Clear(FULL)]))?;
        for g in s.enter(now, cx) {
            p.push(&g)?;
        }
    } else {
        for g in s.update(now, cx) {
            p.push(&g)?;
        }
        let max = p.message_count() + REFINE_MSGS;
        s.refine(cx, &mut p, max)?;
    }
    Ok(p.finish())
}
