//! Rolling raw-frame recorder. Keeps the last N seconds of every bus in RAM
//! and dumps them as a candump log on request.

use crate::{RxFrame, candump};
use std::collections::VecDeque;
use std::io::{self, Write};
use std::time::Duration;

struct Entry {
    bus: u8,
    rx: RxFrame,
}

pub struct Recorder {
    keep: Duration,
    max_frames: usize,
    buses: Vec<String>,
    frames: VecDeque<Entry>,
}

impl Recorder {
    /// `keep` is how far back to hold; `max_frames` caps memory regardless.
    /// ICAN runs around 400 frames/s, so 10 minutes is ~240k frames, ~10 MB.
    pub fn new(keep: Duration, max_frames: usize) -> Self {
        Recorder {
            keep,
            max_frames,
            buses: Vec::new(),
            frames: VecDeque::new(),
        }
    }

    /// Registers a bus name (as written in the dump) and returns its handle.
    pub fn bus(&mut self, name: &str) -> u8 {
        if let Some(i) = self.buses.iter().position(|b| b == name) {
            return i as u8;
        }
        self.buses.push(name.to_string());
        (self.buses.len() - 1) as u8
    }

    pub fn push(&mut self, bus: u8, rx: RxFrame) {
        self.frames.push_back(Entry { bus, rx });
        while self.frames.len() > self.max_frames {
            self.frames.pop_front();
        }
        while let Some(first) = self.frames.front() {
            if rx.at.saturating_duration_since(first.rx.at) > self.keep {
                self.frames.pop_front();
            } else {
                break;
            }
        }
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Writes everything held, oldest first, in candump log format.
    pub fn dump(&self, w: &mut impl Write) -> io::Result<usize> {
        for e in &self.frames {
            candump::write_line(w, e.rx.wall, &self.buses[e.bus as usize], &e.rx.frame)?;
        }
        Ok(self.frames.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Frame;
    use std::time::{Instant, SystemTime};

    fn rx(at: Instant, id: u32) -> RxFrame {
        RxFrame {
            frame: Frame::new(id, &[1]).unwrap(),
            at,
            wall: SystemTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn trims_by_age_and_count() {
        let t0 = Instant::now();
        let mut r = Recorder::new(Duration::from_secs(10), 1000);
        let b = r.bus("ican");
        r.push(b, rx(t0, 1));
        r.push(b, rx(t0 + Duration::from_secs(5), 2));
        r.push(b, rx(t0 + Duration::from_secs(11), 3));
        assert_eq!(r.len(), 2);

        let mut r = Recorder::new(Duration::from_secs(10), 2);
        let b = r.bus("ican");
        for i in 0..5 {
            r.push(b, rx(t0, i));
        }
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn dump_writes_bus_names() {
        let t0 = Instant::now();
        let mut r = Recorder::new(Duration::from_secs(10), 10);
        let a = r.bus("ican");
        let b = r.bus("obd");
        assert_eq!(r.bus("ican"), a);
        r.push(a, rx(t0, 0x123));
        r.push(b, rx(t0, 0x456));
        let mut out = Vec::new();
        assert_eq!(r.dump(&mut out).unwrap(), 2);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains(" ican 123#01"));
        assert!(text.contains(" obd 456#01"));
    }
}
