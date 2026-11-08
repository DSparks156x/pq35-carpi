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

/// Marquee timing, Drew's old config (`RNS-E-Hudiy/config.json`
/// `text_scrolling`): continuous, one blank cell between loops, 400 ms per
/// step, no pauses; line 2 starts 2 s after line 1.
pub const STEP: Duration = Duration::from_millis(400);
pub const LINE2_OFFSET: Duration = Duration::from_millis(2000);
/// Cells per line.
pub const CELLS: usize = 8;

/// One 8-cell line: the text as is if it fits, else a continuous marquee.
/// Spaces scroll as the wide blank (the old encoder's `0x1F`), so the
/// proportional line doesn't jitter; one wide blank separates the loops.
#[derive(Clone, Debug, Default)]
struct Marquee {
    text: String,
    stream: Vec<u8>,
    start: Option<Instant>,
}

impl Marquee {
    fn set(&mut self, now: Instant, text: &str, delay: Duration) {
        let text = text.trim();
        if text == self.text && self.start.is_some() {
            return;
        }
        self.text = text.into();
        if !self.scrolls() {
            self.stream = audscii(text);
            self.start = Some(now);
        } else {
            let mut s = audscii(&text.replace(' ', "\u{1F}"));
            s.push(audscii("\u{1F}")[0]);
            self.stream = s;
            self.start = Some(now + delay);
        }
    }

    fn scrolls(&self) -> bool {
        audscii(&self.text).len() > CELLS
    }

    /// The 8 cells due at `now`.
    fn cells(&self, now: Instant) -> [u8; CELLS] {
        let mut out = [0x20u8; CELLS];
        if !self.scrolls() {
            for (o, b) in out.iter_mut().zip(&self.stream) {
                *o = *b;
            }
            return out;
        }
        let n = self.stream.len();
        let steps = self.start.map_or(0, |s| {
            (now.saturating_duration_since(s).as_millis() / STEP.as_millis()) as usize
        });
        for (i, o) in out.iter_mut().enumerate() {
            *o = self.stream[(steps + i) % n];
        }
        out
    }

    /// When the window next moves.
    fn next_step(&self, now: Instant) -> Option<Instant> {
        if !self.scrolls() {
            return None;
        }
        let s = self.start?;
        if now < s {
            return Some(s);
        }
        let k = now.duration_since(s).as_millis() / STEP.as_millis() + 1;
        Some(s + STEP * k as u32)
    }
}

/// Change-driven sender with the 50 ms floor. Latest wins.
#[derive(Debug, Default)]
pub struct TopLines {
    sent: Option<[u8; 16]>,
    want: Option<[u8; 16]>,
    last_tx: Option<Instant>,
    out: VecDeque<Frame>,
    /// `set_lines` mode: marquees; line 2 `None` = leave 0x365 alone (the
    /// cluster shows its outside temperature there while the centre is ours).
    lines: Option<(Marquee, Option<Marquee>)>,
    sent1: Option<[u8; CELLS]>,
    sent2: Option<[u8; CELLS]>,
}

impl TopLines {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_text(&mut self, now: Instant, text: &str) {
        self.set_cells(now, encode(text));
    }

    /// Two 8-cell lines, each a marquee when longer than 8 (Drew's old top
    /// display). `line2: None` leaves 0x365 to the cluster.
    pub fn set_lines(&mut self, now: Instant, line1: &str, line2: Option<&str>) {
        let (m1, m2) = self.lines.get_or_insert_with(|| (Marquee::default(), None));
        m1.set(now, line1, Duration::ZERO);
        match line2 {
            Some(t) => m2.get_or_insert_with(Marquee::default).set(now, t, LINE2_OFFSET),
            None => *m2 = None,
        }
        self.want = None;
        self.tick(now);
    }

    /// Raw cells, e.g. the radio's own text forwarded byte-exact from 0x7B4/0x7B5.
    pub fn set_cells(&mut self, now: Instant, cells: [u8; 16]) {
        self.lines = None;
        self.want = Some(cells);
        self.tick(now);
    }

    /// Forget what was sent (after the caller was gated off), so the next
    /// `set` goes out even if unchanged.
    pub fn invalidate(&mut self) {
        self.sent = None;
        self.sent1 = None;
        self.sent2 = None;
    }

    pub fn tick(&mut self, now: Instant) {
        if let Some((m1, m2)) = &self.lines {
            if self.last_tx.is_some_and(|t| now < t + MIN_GAP) {
                return;
            }
            let c1 = m1.cells(now);
            let c2 = m2.as_ref().map(|m| m.cells(now));
            let mut sent = false;
            if self.sent1 != Some(c1) {
                self.out.push_back(Frame::new(TOP1_ID, &c1).expect("8 bytes"));
                self.sent1 = Some(c1);
                sent = true;
            }
            if let Some(c2) = c2
                && self.sent2 != Some(c2)
            {
                self.out.push_back(Frame::new(TOP2_ID, &c2).expect("8 bytes"));
                self.sent2 = Some(c2);
                sent = true;
            }
            if sent {
                self.last_tx = Some(now);
            }
            return;
        }
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
        if let Some((m1, m2)) = &self.lines {
            let now = self.last_tx?;
            return [m1.next_step(now), m2.as_ref().and_then(|m| m.next_step(now))]
                .into_iter()
                .flatten()
                .min()
                .map(|t| t.max(now + MIN_GAP));
        }
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

    #[test]
    fn marquee_steps_and_line2_waits() {
        let t0 = Instant::now();
        let mut t = TopLines::new();
        t.set_lines(t0, "One More Time", Some("Daft Punk Discovery"));
        let a = t.poll_transmit().unwrap();
        assert_eq!(a.id, TOP1_ID);
        assert_eq!(a.data()[..3], audscii("One")[..]);
        // Spaces scroll as the wide blank.
        assert_eq!(a.data()[3], audscii("\u{1F}")[0]);
        assert_eq!(t.poll_transmit().unwrap().id, TOP2_ID);
        // One step later line 1 moves; line 2 still holds (2 s offset).
        t.tick(t0 + STEP);
        let b = t.poll_transmit().unwrap();
        assert_eq!(b.id, TOP1_ID);
        assert_eq!(b.data()[..2], audscii("ne")[..]);
        assert!(t.poll_transmit().is_none());
        // No line 2: no 0x365 at all, and nothing to step.
        let mut t = TopLines::new();
        t.set_lines(t0, "Short", None);
        assert_eq!(t.poll_transmit().unwrap().id, TOP1_ID);
        assert!(t.poll_transmit().is_none());
        assert_eq!(t.next_deadline(), None);
    }
}
