//! Virtual CAN bus on virtual time. Frames serialise one at a time at a bit rate
//! and are delivered to every other node when their transmission ends.

use carpi_can::Frame;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Anything that sits on the bus: simulators, or a test wrapper around a
/// sans-IO protocol machine.
pub trait Node {
    fn on_frame(&mut self, now: Instant, frame: &Frame);
    fn tick(&mut self, now: Instant);
    fn poll_transmit(&mut self, now: Instant) -> Option<Frame>;
    fn next_deadline(&self) -> Option<Instant>;
}

pub struct Bus {
    pub now: Instant,
    /// One bit, with stuffing folded in. Default: 100 kbit/s ICAN plus 10 %.
    pub bit_time: Duration,
    busy_until: Instant,
    flight: VecDeque<(Instant, usize, Frame)>,
    /// Every delivered frame: (end of transmission, sender index, frame).
    pub log: Vec<(Instant, usize, Frame)>,
}

impl Bus {
    pub fn new(start: Instant) -> Self {
        Bus {
            now: start,
            bit_time: Duration::from_nanos(11_000),
            busy_until: start,
            flight: VecDeque::new(),
            log: Vec::new(),
        }
    }

    /// Wire time of one standard data frame: 47 bits of overhead plus data.
    pub fn frame_time(&self, len: usize) -> Duration {
        self.bit_time * (47 + 8 * len as u32)
    }

    pub fn run_for(&mut self, nodes: &mut [&mut dyn Node], d: Duration) {
        let until = self.now + d;
        self.run_until(nodes, until);
    }

    pub fn run_until(&mut self, nodes: &mut [&mut dyn Node], until: Instant) {
        loop {
            for (i, n) in nodes.iter_mut().enumerate() {
                n.tick(self.now);
                while let Some(f) = n.poll_transmit(self.now) {
                    let start = self.busy_until.max(self.now);
                    let done = start + self.frame_time(f.len());
                    self.busy_until = done;
                    self.flight.push_back((done, i, f));
                }
            }
            let mut delivered = false;
            while self.flight.front().is_some_and(|(t, ..)| *t <= self.now) {
                let (t, src, f) = self.flight.pop_front().expect("checked");
                for (j, n) in nodes.iter_mut().enumerate() {
                    if j != src {
                        n.on_frame(self.now, &f);
                    }
                }
                self.log.push((t, src, f));
                delivered = true;
            }
            if delivered {
                continue;
            }
            let next = nodes
                .iter()
                .filter_map(|n| n.next_deadline())
                .chain(self.flight.front().map(|(t, ..)| *t))
                .min();
            match next {
                Some(t) if t <= until => {
                    // Guard against a node whose deadline doesn't move.
                    self.now = t.max(self.now + Duration::from_micros(1));
                }
                _ => {
                    self.now = until;
                    for n in nodes.iter_mut() {
                        n.tick(self.now);
                    }
                    return;
                }
            }
        }
    }

    /// Frames sent by node `src`, as (time, id, data).
    pub fn sent_by(&self, src: usize) -> impl Iterator<Item = (Instant, u32, &[u8])> {
        self.log
            .iter()
            .filter(move |(_, s, _)| *s == src)
            .map(|(t, _, f)| (*t, f.id, f.data()))
    }
}
