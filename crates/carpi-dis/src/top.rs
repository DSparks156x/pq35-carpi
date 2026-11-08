//! The two top text lines: 0x363 holds cells 1-8, 0x365 cells 9-16, one AUDSCII
//! byte per cell, plain CAN (`dbc:1564-1582`, `ICAN_rundown.md:192-226`).
//!
//! DBC send rules (`dbc:4993-5018`): send on change, no faster than every
//! 50 ms; stop when the display is busy or the phone bit is set. On TV the
//! firmware redirects the RNS-E's own text to 0x7B4/0x7B5, so these IDs are
//! ours (PLAN §6). This type only builds frames on change; the caller gates them
//! (`TxClass::DisplayTop`) and decides when to be silent. No heartbeat: the old
//! 100 ms resend only fought OEM writes the redirect removed.
//!
//! Charset: AUDSCII as in the centre (`dc/dis_top_display_service.py:151-166`
//! uses the same encoder). Unused cells are `0x20`.

use crate::font::audscii;
use carpi_can::Frame;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub const TOP1_ID: u32 = 0x363;
pub const TOP2_ID: u32 = 0x365;
/// DBC minimum spacing between changes.
pub const MIN_GAP: Duration = Duration::from_millis(50);

/// 16 cells from text: AUDSCII, truncated, padded with spaces.
pub fn encode(text: &str) -> [u8; 16] {
    let mut out = [0x20u8; 16];
    for (o, b) in out.iter_mut().zip(audscii(text)) {
        *o = b;
    }
    out
}

/// The two frames for 16 cells.
pub fn frames(cells: &[u8; 16]) -> [Frame; 2] {
    [
        Frame::new(TOP1_ID, &cells[..8]).expect("8 bytes"),
        Frame::new(TOP2_ID, &cells[8..]).expect("8 bytes"),
    ]
}

/// Change-driven sender with the 50 ms floor. Latest wins.
#[derive(Debug, Default)]
pub struct TopLines {
    sent: Option<[u8; 16]>,
    want: Option<[u8; 16]>,
    last_tx: Option<Instant>,
    out: VecDeque<Frame>,
}

impl TopLines {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_text(&mut self, now: Instant, text: &str) {
        self.set_cells(now, encode(text));
    }

    /// Raw cells, e.g. the radio's own text forwarded byte-exact from 0x7B4/0x7B5.
    pub fn set_cells(&mut self, now: Instant, cells: [u8; 16]) {
        self.want = Some(cells);
        self.tick(now);
    }

    /// Forget what was sent (after the caller was gated off), so the next
    /// `set` goes out even if unchanged.
    pub fn invalidate(&mut self) {
        self.sent = None;
    }

    pub fn tick(&mut self, now: Instant) {
        let Some(w) = self.want else { return };
        if self.sent == Some(w) {
            return;
        }
        if self.last_tx.is_some_and(|t| now < t + MIN_GAP) {
            return;
        }
        self.out.extend(frames(&w));
        self.sent = Some(w);
        self.last_tx = Some(now);
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        match (self.want, self.last_tx) {
            (Some(w), Some(t)) if self.sent != Some(w) => Some(t + MIN_GAP),
            _ => None,
        }
    }

    pub fn poll_transmit(&mut self) -> Option<Frame> {
        self.out.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_and_rate_limits() {
        let t0 = Instant::now();
        let mut t = TopLines::new();
        t.set_text(t0, "Radio 1 abc");
        let a = t.poll_transmit().unwrap();
        let b = t.poll_transmit().unwrap();
        assert_eq!(a.id, TOP1_ID);
        assert_eq!(a.data(), &[0x52, 0x01, 0x04, 0x09, 0x0F, 0x20, 0x31, 0x20]);
        assert_eq!(b.data(), &[0x01, 0x02, 0x03, 0x20, 0x20, 0x20, 0x20, 0x20]);
        t.set_text(t0 + Duration::from_millis(10), "Radio 2");
        assert!(t.poll_transmit().is_none());
        assert_eq!(t.next_deadline(), Some(t0 + MIN_GAP));
        t.tick(t0 + MIN_GAP);
        assert_eq!(t.poll_transmit().unwrap().data()[6], 0x32);
        assert_eq!(t.poll_transmit().unwrap().id, TOP2_ID);
        t.set_text(t0 + Duration::from_secs(1), "Radio 2");
        assert!(t.poll_transmit().is_none());
    }
}
