//! The cluster's side of DDP on 0x6C0/0x6C1: TP2 acceptor, the application
//! handshake, centre ownership, and checks on what gets drawn. Written
//! independently of carpi-tp2 and carpi-dis so they don't share bugs.
//!
//! Behaviour is the A3 white bench cluster's (`docs/research/dis.md` §1):
//! - answers `15` with `00 01`, `08` with its `09` record, `20` with `21`;
//! - grants only the central claim (`53 85`), `53 C0` for anything else;
//! - optional startup busy: first claim gets `53 84`, then `53 05` and `2E`
//!   after a delay (`BENCH_RESULTS.txt:181-185`);
//! - ACKs on a 10 ms cadence (bench `[M500]`, ~9 ms round trip).
//!
//! Drawing checks, kept as notes rather than panics so tests can assert on them:
//! messages over 105 B (policy) or 195 B (freeze), `55` LEN >= 0x80 (freeze),
//! `57` with more than 44 glyphs (`0B 03 57`), native strokes less than 10 ms
//! after the previous message, drawing without ownership.

use crate::bus::Node;
use carpi_can::Frame;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub const HU_ID: u32 = 0x6C0;
pub const CLUSTER_ID: u32 = 0x6C1;
/// The A3 bench cluster's capability record.
pub const WHITE_CAPS: [u8; 12] = [
    0x09, 0x20, 0x0B, 0x50, 0x08, 0x0B, 0x50, 0x30, 0x39, 0x00, 0x32, 0x00,
];
const CLAIM_CENTRE: [u8; 7] = [0x52, 0x05, 0x82, 0x00, 0x1B, 0x40, 0x30];

#[derive(Clone, Debug)]
pub struct ClusterSpec {
    pub caps: Vec<u8>,
    /// A1 body.
    pub params: [u8; 5],
    /// Our ACK after a data frame that asks for one.
    pub ack_delay: Duration,
    /// Application replies after the request completes.
    pub reply_delay: Duration,
    pub frame_gap: Duration,
    /// First claim is answered busy, then free + `2E` after this long.
    pub startup_busy: Option<Duration>,
}

impl Default for ClusterSpec {
    fn default() -> Self {
        ClusterSpec {
            caps: WHITE_CAPS.to_vec(),
            params: [0x0F, 0x8A, 0xFF, 0x4A, 0xFF],
            ack_delay: Duration::from_millis(10),
            reply_delay: Duration::from_millis(10),
            frame_gap: Duration::from_millis(1),
            startup_busy: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ClusterFaults {
    /// Off the bus: hears nothing, sends nothing.
    pub silent: bool,
    /// Every claim gets `53 84` and no free follows, for the rest of the
    /// current (or first) link; a later handshake (`15`) clears it. Bench:
    /// left behind by a killed session.
    pub stuck_busy: bool,
    /// Answer the next N ACK-requesting frames with `9n` and drop them.
    pub not_ready: u32,
}

pub struct SimCluster {
    pub spec: ClusterSpec,
    pub faults: ClusterFaults,
    open: bool,
    rx_seq: u8,
    rx_buf: Vec<u8>,
    rx_first: Option<Instant>,
    last_acked: Option<Vec<u8>>,
    tx_seq: u8,
    block: u8,
    outbox: VecDeque<(Instant, Vec<u8>)>,
    frames: VecDeque<Vec<u8>>,
    wait: Option<(Instant, Vec<u8>, u8)>,
    next_frame_at: Instant,
    out: Vec<(Instant, Frame)>,
    menu_busy: bool,
    /// `stuck_busy` was set when this link's handshake ran.
    stuck_link: bool,
    startup_pending: Option<Duration>,
    last_msg_end: Option<Instant>,
    /// We hold the centre for the head unit.
    pub owner: bool,
    /// Every application message received: (time complete, bytes).
    pub received: Vec<(Instant, Vec<u8>)>,
    /// Drawing messages received while the head unit owned the centre.
    pub drawn: Vec<Vec<u8>>,
    /// Drawing messages that arrived while it didn't (late in-flight ones count).
    pub drawn_unowned: usize,
    pub violations: Vec<String>,
    /// A message the real cluster freezes on arrived.
    pub frozen: bool,
    /// `9n` sent.
    pub not_ready_sent: u32,
}

impl SimCluster {
    pub fn new(spec: ClusterSpec) -> Self {
        let now = Instant::now();
        SimCluster {
            startup_pending: spec.startup_busy,
            spec,
            faults: ClusterFaults::default(),
            open: false,
            rx_seq: 0,
            rx_buf: Vec::new(),
            rx_first: None,
            last_acked: None,
            tx_seq: 0,
            block: 15,
            outbox: VecDeque::new(),
            frames: VecDeque::new(),
            wait: None,
            next_frame_at: now,
            out: Vec::new(),
            menu_busy: false,
            stuck_link: false,
            last_msg_end: None,
            owner: false,
            received: Vec::new(),
            drawn: Vec::new(),
            drawn_unowned: 0,
            violations: Vec::new(),
            frozen: false,
            not_ready_sent: 0,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// A warning or menu takes the centre: `53 84`.
    pub fn busy(&mut self, now: Instant) {
        self.menu_busy = true;
        self.owner = false;
        self.say(now, vec![0x53, 0x84]);
    }

    /// Give the centre back: `53 05`, then `2E`.
    pub fn free(&mut self, now: Instant) {
        self.menu_busy = false;
        self.say(now, vec![0x53, 0x05]);
        self.say(now + Duration::from_millis(20), vec![0x2E]);
    }

    /// Ask for a reinit while the head unit owns the centre.
    pub fn reinit(&mut self, now: Instant) {
        self.owner = false;
        self.say(now, vec![0x2E]);
    }

    /// Repeat the mode request (`00 01`).
    pub fn mode_request(&mut self, now: Instant) {
        self.say(now, vec![0x00, 0x01]);
    }

    fn say(&mut self, at: Instant, msg: Vec<u8>) {
        if self.open {
            self.outbox.push_back((at, msg));
        }
    }

    fn send_at(&mut self, at: Instant, data: &[u8]) {
        self.out
            .push((at, Frame::new(CLUSTER_ID, data).expect("valid")));
    }

    fn reset_link(&mut self, now: Instant) {
        self.rx_seq = 0;
        self.tx_seq = 0;
        self.rx_buf.clear();
        self.rx_first = None;
        self.last_acked = None;
        self.outbox.clear();
        self.frames.clear();
        self.wait = None;
        self.next_frame_at = now;
        self.owner = false;
    }

    fn rx(&mut self, now: Instant, data: &[u8]) {
        if self.faults.silent || data.is_empty() {
            return;
        }
        let b0 = data[0];
        let mut a1 = vec![0xA1];
        a1.extend_from_slice(&self.spec.params);
        match b0 {
            0xA0 => {
                self.reset_link(now);
                self.open = true;
                self.block = data.get(1).copied().filter(|b| *b != 0).unwrap_or(15);
                self.send_at(now + self.spec.reply_delay, &a1);
            }
            0xA3 if self.open => self.send_at(now + self.spec.ack_delay, &a1),
            0xA8 => {
                self.reset_link(now);
                self.open = false;
            }
            _ if !self.open => {}
            _ if b0 >> 4 == 0xB || b0 >> 4 == 0x9 => {
                if let Some((_, f, _)) = &self.wait
                    && (f[0] + 1) & 0x0F == b0 & 0x0F
                    && b0 >> 4 == 0xB
                {
                    self.wait = None;
                    self.next_frame_at = now + self.spec.frame_gap;
                }
            }
            _ if b0 >> 4 <= 3 => self.data(now, data),
            _ => {}
        }
    }

    fn data(&mut self, now: Instant, data: &[u8]) {
        let op = data[0] >> 4;
        let seq = data[0] & 0x0F;
        let wants_ack = op & 2 == 0;
        let last = op & 1 != 0;
        let ack_at = now + self.spec.ack_delay;
        if seq != self.rx_seq {
            if wants_ack && self.last_acked.as_deref() == Some(data) {
                let a = 0xB0 | self.rx_seq;
                self.send_at(ack_at, &[a]);
            }
            return;
        }
        if wants_ack && self.faults.not_ready > 0 {
            self.faults.not_ready -= 1;
            self.not_ready_sent += 1;
            self.send_at(ack_at, &[0x90 | seq]);
            return;
        }
        self.rx_first.get_or_insert(now);
        self.rx_seq = (seq + 1) & 0x0F;
        self.rx_buf.extend_from_slice(&data[1..]);
        if wants_ack {
            self.last_acked = Some(data.to_vec());
            let a = 0xB0 | self.rx_seq;
            self.send_at(ack_at, &[a]);
        }
        if last {
            let msg = std::mem::take(&mut self.rx_buf);
            let first = self.rx_first.take().unwrap_or(now);
            self.app(now, first, ack_at, msg);
        }
    }

    /// `acked`: when our final ACK for this message goes out; the stroke gap
    /// is measured from there to the next message's first frame.
    fn app(&mut self, now: Instant, first: Instant, acked: Instant, m: Vec<u8>) {
        self.received.push((now, m.clone()));
        let at = now + self.spec.reply_delay;
        match m[0] {
            0x15 => {
                if self.stuck_link {
                    self.faults.stuck_busy = false;
                }
                self.stuck_link = self.faults.stuck_busy;
                self.say(at, vec![0x00, 0x01]);
            }
            0x08 => {
                let caps = self.spec.caps.clone();
                self.say(at, caps);
            }
            0x20 => {
                let mut r = m.clone();
                r[0] = 0x21;
                self.say(at, r);
            }
            0x33 => self.owner = false,
            0x52 if m.len() >= 3 && m[2] & 0x80 != 0 => {
                if m[..] != CLAIM_CENTRE {
                    self.say(at, vec![0x53, 0xC0]);
                } else if let Some(d) = self.startup_pending.take() {
                    self.say(at, vec![0x53, 0x84]);
                    self.say(at + d, vec![0x53, 0x05]);
                    self.say(at + d + Duration::from_millis(20), vec![0x2E]);
                } else if self.menu_busy || self.faults.stuck_busy {
                    self.say(at, vec![0x53, 0x84]);
                } else {
                    self.owner = true;
                    self.say(at, vec![0x53, 0x85]);
                }
            }
            0x52 | 0x55 | 0x57 | 0x63 | 0x39 => self.draw(now, first, acked, m),
            _ => {}
        }
    }

    fn draw(&mut self, now: Instant, first: Instant, acked: Instant, m: Vec<u8>) {
        let prev_end = self.last_msg_end.replace(acked);
        if !self.owner {
            self.drawn_unowned += 1;
            return;
        }
        if m.len() > 105 {
            self.violations.push(format!("message {} B > 105", m.len()));
        }
        if m.len() > 195 {
            self.frozen = true;
        }
        let mut at = 0;
        let mut strokes = false;
        while at < m.len() {
            let op = m[at];
            let n = match op {
                0x52 | 0x55 | 0x57 | 0x63 => match m.get(at + 1) {
                    Some(l) => usize::from(*l) + 2,
                    None => {
                        self.violations.push("truncated record".into());
                        break;
                    }
                },
                0x39 => 1,
                _ => {
                    self.violations.push(format!("unknown opcode {op:02X}"));
                    break;
                }
            };
            if at + n > m.len() {
                self.violations.push("truncated record".into());
                break;
            }
            let len = if n >= 2 { m[at + 1] } else { 0 };
            match op {
                0x55 if len >= 0x80 => {
                    self.violations.push(format!("55 LEN {len:02X}"));
                    self.frozen = true;
                }
                0x57 if n - 5 > 44 => {
                    self.violations.push(format!("57 with {} glyphs", n - 5));
                    self.say(now + self.spec.reply_delay, vec![0x0B, 0x03, 0x57]);
                }
                0x63 => strokes = true,
                _ => {}
            }
            at += n;
        }
        if strokes
            && let Some(t) = prev_end
            && first.saturating_duration_since(t) < Duration::from_millis(10)
        {
            self.violations
                .push("stroke message < 10 ms after the last".into());
        }
        self.drawn.push(m);
    }

    fn tick_link(&mut self, now: Instant) {
        if self.faults.silent || !self.open {
            return;
        }
        if let Some((dl, f, n)) = &mut self.wait
            && now >= *dl
        {
            if *n >= 2 {
                self.open = false;
                return;
            }
            *n += 1;
            *dl = now + Duration::from_secs(1);
            let f = f.clone();
            self.send_at(now, &f);
        }
        if self.wait.is_none()
            && self.frames.is_empty()
            && let Some((at, _)) = self.outbox.front()
            && now >= *at
        {
            let (_, msg) = self.outbox.pop_front().expect("checked");
            let chunks: Vec<&[u8]> = msg.chunks(7).collect();
            let n = chunks.len();
            for (i, c) in chunks.into_iter().enumerate() {
                let last = i + 1 == n;
                let boundary = (i + 1) % usize::from(self.block) == 0;
                let op: u8 = match (last, boundary) {
                    (true, _) => 1,
                    (false, true) => 0,
                    _ => 2,
                };
                let mut f = vec![(op << 4) | self.tx_seq];
                self.tx_seq = (self.tx_seq + 1) & 0x0F;
                f.extend_from_slice(c);
                self.frames.push_back(f);
            }
            self.next_frame_at = self.next_frame_at.max(now);
        }
        while self.wait.is_none() && now >= self.next_frame_at {
            let Some(f) = self.frames.pop_front() else {
                break;
            };
            if f[0] >> 4 <= 1 {
                self.wait = Some((now + Duration::from_secs(1), f.clone(), 0));
            }
            self.send_at(now, &f);
            self.next_frame_at = now + self.spec.frame_gap;
        }
    }
}

impl Node for SimCluster {
    fn on_frame(&mut self, now: Instant, frame: &Frame) {
        if !frame.extended && frame.id == HU_ID {
            self.rx(now, frame.data());
        }
    }

    fn tick(&mut self, now: Instant) {
        self.tick_link(now);
    }

    fn poll_transmit(&mut self, now: Instant) -> Option<Frame> {
        if self.faults.silent {
            self.out.clear();
            return None;
        }
        let i = self
            .out
            .iter()
            .enumerate()
            .filter(|(_, (t, _))| *t <= now)
            .min_by_key(|(i, (t, _))| (*t, *i))
            .map(|(i, _)| i)?;
        Some(self.out.remove(i).1)
    }

    fn next_deadline(&self) -> Option<Instant> {
        if self.faults.silent {
            return None;
        }
        let mut d = self.out.iter().map(|(t, _)| *t).min();
        let mut take = |t: Instant| d = Some(d.map_or(t, |x: Instant| x.min(t)));
        if self.open {
            if let Some((dl, ..)) = &self.wait {
                take(*dl);
            } else if !self.frames.is_empty() {
                take(self.next_frame_at);
            } else if let Some((at, _)) = self.outbox.front() {
                take(*at);
            }
        }
        d
    }
}
