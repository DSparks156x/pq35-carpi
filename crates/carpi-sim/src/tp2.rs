//! Gateway plus TP2/KWP modules, wire level. Written independently of carpi-tp2
//! so the two don't share bugs.
//!
//! Latencies are from the engine trace (RNS-E-Hudiy
//! `references/logs/2026-06-17/tp2_worker_2.log`): D0 and A1 ~12 ms, our ACK
//! ~8 ms, first response frame 23-40 ms after the request, ~6 ms between frames.
//! Other modules' timing is unmeasured; they get the same.

use crate::bus::Node;
use carpi_can::Frame;
use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug)]
pub struct Latency {
    pub setup: Duration,
    pub params: Duration,
    pub ack: Duration,
    /// First response frame after the request completes: uniform in this range.
    pub first_frame: (Duration, Duration),
    pub frame_gap: Duration,
    /// Between `7F xx 78` replies, and from the last one to the answer.
    pub pending_gap: Duration,
    /// How long the module waits for our ACK before resending.
    pub ack_wait: Duration,
}

impl Default for Latency {
    fn default() -> Self {
        Latency {
            setup: Duration::from_millis(12),
            params: Duration::from_millis(12),
            ack: Duration::from_millis(8),
            first_frame: (Duration::from_millis(23), Duration::from_millis(40)),
            frame_gap: Duration::from_millis(6),
            pending_gap: Duration::from_millis(500),
            ack_wait: Duration::from_millis(500),
        }
    }
}

/// Scripted faults. Counters count down as they fire.
#[derive(Clone, Copy, Debug, Default)]
pub struct Faults {
    /// Ignore everything, including setup (module dropped off the bus).
    pub silent: bool,
    /// Answer the next N ACK-requesting frames with `9n` and drop them.
    pub not_ready: u32,
    /// Take the next N ACK-requesting frames but lose our ACK.
    pub lose_acks: u32,
    /// Send `7F sid 78` this many times before the next answer.
    pub pending: u32,
    /// Answer setup with this opcode (D6..D8) instead of D0.
    pub refuse_setup: Option<u8>,
}

/// What a module is and what it answers.
#[derive(Clone, Debug, Default)]
pub struct ModuleSpec {
    /// TP2 logical ID (setup byte 0).
    pub tp2: u8,
    /// ID the module listens on, given in D0. Engine 0x740 (proven); Haldex
    /// application 0x764 (proven); others are made up.
    pub rx_id: u16,
    /// A1 body. Engine: `0F 8A FF 4A FF` (proven).
    pub params: [u8; 5],
    /// `21 gg` -> records.
    pub groups: BTreeMap<u8, Vec<[u8; 3]>>,
    /// `1A 9B` reply body.
    pub ident: Vec<u8>,
    /// `18 02 FF 00` -> (code, status).
    pub dtcs: Vec<(u16, u8)>,
    /// `1A 9F` records (gateway only).
    pub install_list: Vec<[u8; 4]>,
    /// Exact scripted KWP answers, consumed in order (then normal behavior).
    pub replies: BTreeMap<Vec<u8>, VecDeque<Vec<u8>>>,
}

impl ModuleSpec {
    /// Engine 0x01 with groups 3 and 113 exactly as in the trace.
    pub fn engine() -> Self {
        let mut groups = BTreeMap::new();
        groups.insert(
            3,
            vec![
                [0x01, 0xC8, 0x15],
                [0x19, 0x20, 0x03],
                [0x21, 0xFF, 0x0D],
                [0x1B, 0x4B, 0x79],
                [0x05, 0x0A, 0xA0],
                [0x05, 0x0A, 0x7D],
                [0x25, 0x00, 0x00],
                [0x25, 0x00, 0x00],
            ],
        );
        groups.insert(
            113,
            vec![
                [0x01, 0xC8, 0x14],
                [0x21, 0x85, 0x22],
                [0x21, 0xFF, 0x0D],
                [0x12, 0x7D, 0x9F],
                [0x25, 0x00, 0x00],
                [0x25, 0x00, 0x00],
                [0x25, 0x00, 0x00],
                [0x25, 0x00, 0x00],
            ],
        );
        ModuleSpec {
            tp2: 0x01,
            rx_id: 0x740,
            params: [0x0F, 0x8A, 0xFF, 0x4A, 0xFF],
            groups,
            ident: b"8J0907115   MED17.5.1  0040".to_vec(),
            dtcs: vec![(17965, 0x23)],
            install_list: vec![],
            replies: BTreeMap::new(),
        }
    }

    /// Haldex (TP2 0x0A in the old code) in its application (0x764).
    pub fn haldex() -> Self {
        let mut groups = BTreeMap::new();
        groups.insert(1, vec![[0x21, 0xFF, 0x40], [0x05, 0x0A, 0x82]]);
        ModuleSpec {
            tp2: 0x0A,
            rx_id: 0x764,
            params: [0x0F, 0x8A, 0xFF, 0x4A, 0xFF],
            groups,
            ident: b"0AY907554  HALDEX".to_vec(),
            ..Default::default()
        }
    }

    /// Gateway 0x1F with an installation list shaped like Drew's (engine, Haldex
    /// as VCDS 22, BCM 09 -> 0x20, KSG 46 -> 0x21 coded but absent).
    pub fn gateway() -> Self {
        ModuleSpec {
            tp2: 0x1F,
            rx_id: 0x75F,
            params: [0x0F, 0x8A, 0xFF, 0x4A, 0xFF],
            ident: b"1K0907530   GATEWAY".to_vec(),
            install_list: vec![
                [0x01, 0x01, 0x01, 0x01],
                [0x03, 0x03, 0x01, 0x01],
                [0x09, 0x20, 0x02, 0x01],
                [0x19, 0x1F, 0x02, 0x01],
                [0x22, 0x0A, 0x01, 0x01],
                [0x46, 0x21, 0x02, 0x09],
            ],
            ..Default::default()
        }
    }
}

struct Session {
    tester_rx: u16,
    rx_seq: u8,
    rx_buf: Vec<u8>,
    last_acked: Option<Vec<u8>>,
    tx_seq: u8,
    block: u8,
    /// Messages waiting to go, with the earliest time they may start.
    outbox: VecDeque<(Instant, Vec<u8>)>,
    /// Frames of the message being sent, not yet scheduled.
    frames: VecDeque<Vec<u8>>,
    /// ACK-requesting frame we sent: (deadline, frame, retries).
    wait: Option<(Instant, Vec<u8>, u8)>,
    next_frame_at: Instant,
}

/// One TP2 module peer.
pub struct SimModule {
    pub spec: ModuleSpec,
    pub faults: Faults,
    pub lat: Latency,
    sess: Option<Session>,
    out: Vec<(Instant, Frame)>,
    rng: u64,
    /// Every complete KWP request received, for assertions.
    pub requests: Vec<Vec<u8>>,
}

impl SimModule {
    pub fn new(spec: ModuleSpec) -> Self {
        SimModule {
            rng: 0x9E37_79B9_7F4A_7C15 ^ u64::from(spec.tp2),
            spec,
            faults: Faults::default(),
            lat: Latency::default(),
            sess: None,
            out: Vec::new(),
            requests: Vec::new(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.sess.is_some()
    }

    fn rand_between(&mut self, (lo, hi): (Duration, Duration)) -> Duration {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        let span = (hi - lo).as_micros() as u64;
        lo + Duration::from_micros(if span == 0 { 0 } else { self.rng % (span + 1) })
    }

    fn send_at(&mut self, at: Instant, data: &[u8]) {
        let id = self.sess.as_ref().map_or(0, |s| s.tester_rx);
        self.out
            .push((at, Frame::new(u32::from(id), data).expect("valid")));
    }

    fn start(&mut self, now: Instant, tester_rx: u16) {
        self.sess = Some(Session {
            tester_rx,
            rx_seq: 0,
            rx_buf: Vec::new(),
            last_acked: None,
            tx_seq: 0,
            block: 15,
            outbox: VecDeque::new(),
            frames: VecDeque::new(),
            wait: None,
            next_frame_at: now,
        });
    }

    fn on_frame(&mut self, now: Instant, data: &[u8]) {
        if self.faults.silent || self.sess.is_none() || data.is_empty() {
            return;
        }
        let b0 = data[0];
        let mut a1 = vec![0xA1];
        a1.extend_from_slice(&self.spec.params);
        match b0 {
            0xA0 => {
                if let Some(s) = &mut self.sess {
                    s.block = data.get(1).copied().filter(|b| *b != 0).unwrap_or(15);
                }
                self.send_at(now + self.lat.params, &a1);
            }
            0xA3 => self.send_at(now + self.lat.ack, &a1),
            0xA8 => {
                self.send_at(now, &[0xA8]);
                self.sess = None;
            }
            _ if b0 >> 4 == 0xB || b0 >> 4 == 0x9 => self.on_ack(now, b0 & 0x0F),
            _ if b0 >> 4 <= 3 => self.on_data(now, data),
            _ => {}
        }
    }

    fn on_ack(&mut self, now: Instant, next: u8) {
        let Some(s) = &mut self.sess else { return };
        if let Some((_, f, _)) = &s.wait
            && (f[0] + 1) & 0x0F == next
        {
            s.wait = None;
            s.next_frame_at = now + self.lat.frame_gap;
        }
    }

    fn on_data(&mut self, now: Instant, data: &[u8]) {
        let ack_at = now + self.lat.ack;
        let op = data[0] >> 4;
        let seq = data[0] & 0x0F;
        let wants_ack = op & 2 == 0;
        let last = op & 1 != 0;
        let s = self.sess.as_mut().expect("checked");
        if seq != s.rx_seq {
            if wants_ack && s.last_acked.as_deref() == Some(data) {
                let a = 0xB0 | s.rx_seq;
                self.send_at(ack_at, &[a]);
            }
            return;
        }
        if wants_ack && self.faults.not_ready > 0 {
            self.faults.not_ready -= 1;
            let a = 0x90 | seq;
            self.send_at(ack_at, &[a]);
            return;
        }
        s.rx_seq = (seq + 1) & 0x0F;
        s.rx_buf.extend_from_slice(&data[1..]);
        if wants_ack {
            s.last_acked = Some(data.to_vec());
            let a = 0xB0 | s.rx_seq;
            if self.faults.lose_acks > 0 {
                self.faults.lose_acks -= 1;
            } else {
                self.send_at(ack_at, &[a]);
            }
        }
        if !last {
            return;
        }
        let s = self.sess.as_mut().expect("checked");
        let buf = std::mem::take(&mut s.rx_buf);
        if buf.len() < 2 {
            return;
        }
        let len = usize::from(u16::from_be_bytes([buf[0], buf[1]]) & 0x7FFF);
        if buf.len() - 2 != len {
            return;
        }
        let req = buf[2..].to_vec();
        self.requests.push(req.clone());
        let mut at = now + self.rand_between(self.lat.first_frame);
        while self.faults.pending > 0 {
            self.faults.pending -= 1;
            self.queue(at, vec![0x7F, req[0], 0x78]);
            at += self.lat.pending_gap;
        }
        let resp = self.answer(&req);
        self.queue(at, resp);
    }

    fn queue(&mut self, at: Instant, msg: Vec<u8>) {
        if let Some(s) = &mut self.sess {
            s.outbox.push_back((at, msg));
        }
    }

    fn answer(&mut self, req: &[u8]) -> Vec<u8> {
        if let Some(reply) = self.spec.replies.get_mut(req).and_then(VecDeque::pop_front) {
            return reply;
        }
        let sid = req[0];
        let sub = req.get(1).copied();
        let neg = |nrc| vec![0x7F, sid, nrc];
        match (sid, sub) {
            (0x10, Some(s)) => vec![0x50, s],
            (0x3E, _) => vec![0x7E],
            (0x21, Some(g)) => match self.spec.groups.get(&g) {
                Some(recs) => {
                    let mut r = vec![0x61, g];
                    recs.iter().for_each(|x| r.extend_from_slice(x));
                    r
                }
                None => neg(0x31),
            },
            (0x1A, Some(0x9B)) => {
                let mut r = vec![0x5A, 0x9B];
                r.extend_from_slice(&self.spec.ident);
                r
            }
            (0x1A, Some(0x9F)) if !self.spec.install_list.is_empty() => {
                let mut r = vec![0x5A, 0x9F, 0x00];
                self.spec
                    .install_list
                    .iter()
                    .for_each(|x| r.extend_from_slice(x));
                r.push(0xFF);
                r
            }
            (0x18, _) => {
                let mut r = vec![0x58, self.spec.dtcs.len() as u8];
                for (c, st) in &self.spec.dtcs {
                    r.extend_from_slice(&c.to_be_bytes());
                    r.push(*st);
                }
                r
            }
            (0x14, _) => {
                self.spec.dtcs.clear();
                vec![0x54, 0xFF, 0x00]
            }
            (0x1A, _) => neg(0x12),
            _ => neg(0x11),
        }
    }

    fn tick(&mut self, now: Instant) {
        if self.faults.silent {
            return;
        }
        let gap = self.lat.frame_gap;
        let ack_wait = self.lat.ack_wait;
        let Some(s) = &mut self.sess else { return };
        let mut emit = Vec::new();
        if let Some((dl, f, n)) = &mut s.wait
            && now >= *dl
        {
            if *n >= 2 {
                // Give up like a real module would: drop the channel.
                self.sess = None;
                return;
            }
            *n += 1;
            *dl = now + ack_wait;
            emit.push(f.clone());
        }
        if s.wait.is_none()
            && s.frames.is_empty()
            && let Some((at, _)) = s.outbox.front()
            && now >= *at
        {
            let (_, msg) = s.outbox.pop_front().expect("checked");
            // Pending replies flag bit 15 of the length `[jazdw]`.
            let flag = if msg.len() == 3 && msg[0] == 0x7F && msg[2] == 0x78 {
                0x8000
            } else {
                0
            };
            let mut w = ((msg.len() as u16) | flag).to_be_bytes().to_vec();
            w.extend_from_slice(&msg);
            let chunks: Vec<&[u8]> = w.chunks(7).collect();
            let n = chunks.len();
            for (i, c) in chunks.into_iter().enumerate() {
                let last = i + 1 == n;
                let boundary = (i + 1) % usize::from(s.block) == 0;
                let op: u8 = match (last, boundary) {
                    (true, _) => 1,
                    (false, true) => 0,
                    _ => 2,
                };
                let mut f = vec![(op << 4) | s.tx_seq];
                s.tx_seq = (s.tx_seq + 1) & 0x0F;
                f.extend_from_slice(c);
                s.frames.push_back(f);
            }
            s.next_frame_at = s.next_frame_at.max(now);
        }
        while s.wait.is_none() && now >= s.next_frame_at {
            let Some(f) = s.frames.pop_front() else { break };
            if f[0] >> 4 <= 1 {
                s.wait = Some((now + ack_wait, f.clone(), 0));
            }
            emit.push(f);
            s.next_frame_at = now + gap;
        }
        for f in emit {
            self.send_at(now, &f);
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        let mut d = self.out.iter().map(|(t, _)| *t).min();
        let mut take = |t: Instant| d = Some(d.map_or(t, |x: Instant| x.min(t)));
        if self.faults.silent {
            return d;
        }
        if let Some(s) = &self.sess {
            if let Some((dl, ..)) = &s.wait {
                take(*dl);
            } else if !s.frames.is_empty() {
                take(s.next_frame_at);
            } else if let Some((at, _)) = s.outbox.front() {
                take(*at);
            }
        }
        d
    }
}

/// The gateway: answers setups on 0x200 for its modules and routes their
/// channels. Its own module (0x1F) serves `1A 9F`.
pub struct Gateway {
    pub modules: BTreeMap<u8, SimModule>,
    /// Every setup seen: (module, tester RX ID).
    pub setups: Vec<(u8, u16)>,
    out: Vec<(Instant, Frame)>,
    pub setup_latency: Duration,
}

impl Gateway {
    pub fn new(specs: impl IntoIterator<Item = ModuleSpec>) -> Self {
        Gateway {
            modules: specs
                .into_iter()
                .map(|s| (s.tp2, SimModule::new(s)))
                .collect(),
            setups: Vec::new(),
            out: Vec::new(),
            setup_latency: Duration::from_millis(12),
        }
    }

    /// Gateway, engine and Haldex.
    pub fn tt() -> Self {
        Gateway::new([
            ModuleSpec::gateway(),
            ModuleSpec::engine(),
            ModuleSpec::haldex(),
        ])
    }

    pub fn module(&mut self, tp2: u8) -> &mut SimModule {
        self.modules.get_mut(&tp2).expect("no such sim module")
    }

    fn on_setup(&mut self, now: Instant, d: &[u8]) {
        if d.len() != 7 || d[1] != 0xC0 {
            return;
        }
        let tester = u16::from(d[4]) | (u16::from(d[5] & 0x07) << 8);
        self.setups.push((d[0], tester));
        let at = now + self.setup_latency;
        let Some(m) = self.modules.get_mut(&d[0]) else {
            return;
        };
        if m.faults.silent {
            return;
        }
        let id = 0x200 + u32::from(d[0]);
        let reply = match m.faults.refuse_setup {
            Some(op) => [0x00, op, 0, 0, 0, 0, 0],
            None => {
                m.start(at, tester);
                let rx = m.spec.rx_id;
                [
                    0x00,
                    0xD0,
                    (tester & 0xFF) as u8,
                    (tester >> 8) as u8,
                    (rx & 0xFF) as u8,
                    (rx >> 8) as u8,
                    0x01,
                ]
            }
        };
        self.out.push((at, Frame::new(id, &reply).expect("valid")));
    }
}

impl Node for Gateway {
    fn on_frame(&mut self, now: Instant, frame: &Frame) {
        if frame.extended {
            return;
        }
        if frame.id == 0x200 {
            self.on_setup(now, frame.data());
            return;
        }
        for m in self.modules.values_mut() {
            if u32::from(m.spec.rx_id) == frame.id {
                m.on_frame(now, frame.data());
            }
        }
    }

    fn tick(&mut self, now: Instant) {
        for m in self.modules.values_mut() {
            m.tick(now);
        }
    }

    fn poll_transmit(&mut self, now: Instant) -> Option<Frame> {
        if let Some(i) = due(&self.out, now) {
            return Some(self.out.remove(i).1);
        }
        for m in self.modules.values_mut() {
            if let Some(i) = due(&m.out, now) {
                return Some(m.out.remove(i).1);
            }
        }
        None
    }

    fn next_deadline(&self) -> Option<Instant> {
        self.out
            .iter()
            .map(|(t, _)| *t)
            .chain(self.modules.values().filter_map(|m| m.next_deadline()))
            .min()
    }
}

/// Index of the earliest frame due at `now` (stable for equal times).
fn due(out: &[(Instant, Frame)], now: Instant) -> Option<usize> {
    out.iter()
        .enumerate()
        .filter(|(_, (t, _))| *t <= now)
        .min_by_key(|(i, (t, _))| (*t, *i))
        .map(|(i, _)| i)
}
