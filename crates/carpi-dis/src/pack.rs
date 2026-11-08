//! Packer: groups of ops into application messages, in order, with the clip and
//! ROP registers tracked and the cluster's command buffer respected.
//!
//! **The buffer rule.** The cluster buffers commands and applies them on `39`,
//! and on its own when the buffer fills (Drew). A clear only looks clean if its
//! redraw lands in the same buffer. Nobody knows the buffer size.
//!
//! How the old driver avoided obvious auto-commits: every clear starts a fresh
//! application message (`dc/dis_service.py:1654-1666`), and its paired draw is
//! forced into that same message (`must_colocate`, `:1771-1788`); a text
//! replacement (tail clear + text) is one indivisible payload that must fit the
//! message budget or is rejected (`:649-668`, `:1628-1635`). It never let a clear
//! and its redraw span two messages, and never relied on more than one message.
//!
//! Encoded here as:
//! 1. A [`Group`] is indivisible: it always sits inside one message, so it must
//!    fit `max_msg` (with the clip/latch records the packer inserts).
//! 2. Bytes since the last `39` are counted across messages. If a group would
//!    push that count past `cmd_buffer`, the packer commits first (`39` ends the
//!    current message) and starts the group in a fresh buffer and message.
//!    `cmd_buffer` defaults to `max_msg` (guess), which makes rule 1 the old rule.
//! 3. A frame ends with `39`. Inline `39` at a message end works (bench,
//!    `clusterRE/dis_findings.txt:334-335`); we never put records after a `39` in
//!    the same message.
//!
//! The packer never reorders and never splits a record or a selector/stroke pair.

use crate::cmd::{COMMIT, EncodeError, Op, Rop, region, selector};
use crate::proto::{Profile, Rect};
use std::time::Duration;

/// What the cluster's registers hold, as far as we know. `None` = unknown.
/// Reset on every session generation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Registers {
    pub clip: Option<Rect>,
    pub rop: Option<Rop>,
}

/// Ops that must land in one buffer: a clear and its redraw, one scroll step,
/// one bitmap record, one stroke.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub ops: Vec<Op>,
    /// Quiet time on both sides of the message that carries this group. Native
    /// strokes: 10 ms (5 ms corrupted tiles; bench,
    /// `clusterRE/nav_sweep_checkpoint.txt:79-81`). The session enforces it.
    pub gap: Duration,
    /// For tests and logs.
    pub tag: &'static str,
}

impl Group {
    pub fn new(tag: &'static str, ops: Vec<Op>) -> Self {
        Group {
            ops,
            gap: Duration::ZERO,
            tag,
        }
    }
    pub fn gap(mut self, d: Duration) -> Self {
        self.gap = d;
        self
    }
}

/// One application message for the link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub bytes: Vec<u8>,
    /// Largest group gap inside: wait this long after the previous message,
    /// and make the next one wait as long.
    pub gap: Duration,
}

/// Where a group ended up: message index and byte range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub tag: &'static str,
    pub msg: usize,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PackError {
    Encode(&'static str, EncodeError),
    /// Group bigger than one message or the buffer: a helper bug.
    GroupTooBig(&'static str, usize),
}

/// A packed frame.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Packed {
    pub messages: Vec<Message>,
    /// Registers once every message is applied.
    pub regs: Registers,
    pub spans: Vec<Span>,
}

impl Packed {
    pub fn bytes(&self) -> usize {
        self.messages.iter().map(|m| m.bytes.len()).sum()
    }
}

#[derive(Clone, Debug)]
pub struct Packer {
    p: Profile,
    regs: Registers,
    msgs: Vec<Message>,
    cur: Vec<u8>,
    cur_gap: Duration,
    /// Bytes since the last `39`, across messages.
    acc: usize,
    spans: Vec<Span>,
}

impl Packer {
    pub fn new(p: Profile, regs: Registers) -> Self {
        Packer {
            p,
            regs,
            msgs: Vec::new(),
            cur: Vec::new(),
            cur_gap: Duration::ZERO,
            acc: 0,
            spans: Vec::new(),
        }
    }

    /// Bytes for `g` given the current registers, and the registers after it.
    pub fn resolve(&self, g: &Group) -> Result<(Vec<u8>, Registers), PackError> {
        let mut regs = self.regs;
        let mut out = Vec::new();
        for op in &g.ops {
            op.check(&self.p).map_err(|e| PackError::Encode(g.tag, e))?;
            if let Some(c) = op.clip()
                && regs.clip != Some(c)
            {
                out.extend_from_slice(&region(0x00, &c));
                regs.clip = Some(c);
            }
            match op {
                Op::Clip(r) | Op::Clear(r) | Op::Fill(r) => regs.clip = Some(*r),
                Op::Line { rop, .. } => {
                    if regs.rop != Some(*rop) {
                        out.extend_from_slice(&selector(*rop));
                        regs.rop = Some(*rop);
                    }
                }
                Op::Bitmap { mode, .. } => regs.rop = Some(*mode),
                Op::Stroke { .. } => regs.rop = Some(Rop::Xor),
                Op::Text { .. } => {}
            }
            out.extend_from_slice(&op.encode());
        }
        let cap = self.p.max_msg.min(self.p.cmd_buffer);
        if out.len() > cap {
            return Err(PackError::GroupTooBig(g.tag, out.len()));
        }
        Ok((out, regs))
    }

    /// Append a group. Empty groups are ignored.
    pub fn push(&mut self, g: &Group) -> Result<(), PackError> {
        if g.ops.is_empty() {
            return Ok(());
        }
        let (bytes, regs) = self.resolve(g)?;
        if self.acc + bytes.len() > self.p.cmd_buffer {
            self.commit();
        }
        if !self.cur.is_empty() && self.cur.len() + bytes.len() > self.p.max_msg {
            self.close();
        }
        let start = self.cur.len();
        self.cur.extend_from_slice(&bytes);
        self.cur_gap = self.cur_gap.max(g.gap);
        self.acc += bytes.len();
        self.regs = regs;
        self.spans.push(Span {
            tag: g.tag,
            msg: self.msgs.len(),
            start,
            end: self.cur.len(),
        });
        Ok(())
    }

    /// Messages so far, counting a partly filled one.
    pub fn message_count(&self) -> usize {
        self.msgs.len() + usize::from(!self.cur.is_empty())
    }

    /// Message count if `g` were pushed now.
    pub fn count_after(&self, g: &Group) -> Result<usize, PackError> {
        let mut c = self.clone();
        c.push(g)?;
        Ok(c.message_count())
    }

    pub fn is_empty(&self) -> bool {
        self.acc == 0 && self.msgs.is_empty()
    }

    /// `39` ends the current message (or gets one of its own if it's full).
    fn commit(&mut self) {
        if self.cur.len() + 1 > self.p.max_msg {
            self.close();
        }
        self.cur.push(COMMIT);
        self.close();
        self.acc = 0;
    }

    fn close(&mut self) {
        if self.cur.is_empty() {
            return;
        }
        self.msgs.push(Message {
            bytes: std::mem::take(&mut self.cur),
            gap: self.cur_gap,
        });
        self.cur_gap = Duration::ZERO;
    }

    /// Final `39` and the packed frame. Empty if nothing was pushed.
    pub fn finish(mut self) -> Packed {
        if self.acc > 0 {
            self.commit();
        }
        self.close();
        Packed {
            messages: self.msgs,
            regs: self.regs,
            spans: self.spans,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::Ink;
    use crate::font::Font;
    use crate::proto::FULL;

    fn text(n: usize) -> Op {
        Op::Text {
            clip: FULL,
            x: 0,
            y: 0,
            flags: crate::cmd::text_flags(Font::Prop, Ink::Opaque),
            bytes: vec![0x30; n],
        }
    }

    #[test]
    fn inserts_clip_once_and_commits() {
        let mut p = Packer::new(Profile::WHITE, Registers::default());
        p.push(&Group::new("a", vec![text(2)])).unwrap();
        p.push(&Group::new("b", vec![text(2)])).unwrap();
        let f = p.finish();
        assert_eq!(f.messages.len(), 1);
        let m = &f.messages[0].bytes;
        assert_eq!(m.len(), 7 + 7 + 7 + 1);
        assert_eq!(&m[..7], &[0x52, 0x05, 0x00, 0x00, 0x1B, 0x40, 0x30]);
        assert_eq!(*m.last().unwrap(), 0x39);
        assert_eq!(f.regs.clip, Some(FULL));
    }

    #[test]
    fn commits_before_a_group_that_would_overflow_the_buffer() {
        let regs = Registers {
            clip: Some(FULL),
            rop: None,
        };
        let mut p = Packer::new(Profile::WHITE, regs);
        p.push(&Group::new("a", vec![text(40), text(15)])).unwrap(); // 65 B
        p.push(&Group::new("b", vec![Op::Clear(FULL), text(40)]))
            .unwrap(); // 7+7+45
        let f = p.finish();
        assert_eq!(f.messages.len(), 2);
        assert_eq!(f.messages[0].bytes.len(), 66);
        assert_eq!(*f.messages[0].bytes.last().unwrap(), 0x39);
        assert_eq!(f.spans[1].msg, 1);
    }

    #[test]
    fn rejects_oversized_group() {
        let mut p = Packer::new(Profile::WHITE, Registers::default());
        let g = Group::new("big", vec![text(44), text(44), text(5)]);
        assert!(matches!(p.push(&g), Err(PackError::GroupTooBig("big", _))));
    }
}
