//! One TP2.0 channel: open, data transfer, keepalive, close. Sans-IO.
//!
//! Refs as in [`crate::wire`]. Defaults that are our own choice are marked "ours".

use crate::wire::{self, Params, Pdu, SetupReply};
use carpi_can::Frame;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How messages are delimited inside the data frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Framing {
    /// First two bytes of a message are its length, big-endian. Diagnostics (KWP).
    /// Proven `[trace]`. Bit 15 is a flag some ECUs set (engine pending replies,
    /// airbag, steering wheel) `[oldtp2]`; `[jazdw]` says the top nibble is flags.
    /// We accept either 15- or 12-bit lengths on receive and send plain lengths.
    LengthPrefixed,
    /// No length; the message ends at the `1n`/`3n` frame. DDP. Bench `[dis]`.
    Unprefixed,
}

/// Frame pacing between our own data frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gap {
    /// Use the T3 the peer gave in its A0/A1. What the old diag code did `[oldtp2]`;
    /// the engine asks for 10 ms.
    PeerT3,
    /// Fixed gap. The DDP bench ran with 0 at 500 kbit/s `[dis]`.
    Fixed(Duration),
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub framing: Framing,
    /// Our parameters: sent in A0 when we open, and in every A1 we send.
    pub params: Params,
    /// Our cap on frames per ACK block; the peer's BS caps it too.
    pub max_block: u8,
    pub gap: Gap,
    /// Extra wait between the end of one message and the start of the next.
    pub msg_gap: Duration,
    /// Wait for the setup reply (D0) and the A1 after our A0. Ours. D0 and A1 each
    /// come back in ~12 ms on the car `[trace]`.
    pub open_timeout: Duration,
    /// Wait for an ACK. Ours: the peer's T1 (100 ms) is recorded but not used,
    /// because ACKs cross the gateway. Old code waited 2 s (diag) / 1 s (DDP).
    pub ack_timeout: Duration,
    /// Resend the last ACK-requesting frame this many times before giving up.
    /// `[dis]`: 2.
    pub ack_retries: u8,
    /// After a `9n`, wait this long, then resend from `n`. `[dis]`: 100 ms.
    pub not_ready_wait: Duration,
    /// More `9n` than this in one message closes the channel. `[dis]`: 5.
    pub not_ready_limit: u8,
    /// Send A3 this often while open. Diag: 1 s `[oldtp2]`. DDP: 2 s `[dis]`.
    /// `None` never sends A3 (passive peer roles).
    pub keepalive: Option<Duration>,
    /// Longest message we reassemble before giving up.
    pub max_rx_len: usize,
    /// Fixed channels only: answer a peer's A0 while idle and count the channel
    /// open. The white cluster sometimes opens first `[dis]`.
    pub accept_peer_open: bool,
    /// Fixed channels only: on `open`, send A8 and wait this long before A0, to
    /// clear a zombie session. Old DDP did 1 s `[dis]`.
    pub flush_before_open: Option<Duration>,
    /// Fixed channels only: answer A3 or data while idle with A8 (stale session
    /// from before a restart) `[dis]`.
    pub disconnect_stale: bool,
}

impl Config {
    /// Dynamic diagnostic channel through the gateway.
    pub fn diag() -> Self {
        Config {
            framing: Framing::LengthPrefixed,
            params: Params::DIAG,
            max_block: wire::MAX_BLOCK,
            gap: Gap::PeerT3,
            msg_gap: Duration::ZERO,
            open_timeout: Duration::from_millis(500),
            ack_timeout: Duration::from_millis(500),
            ack_retries: 2,
            not_ready_wait: Duration::from_millis(100),
            not_ready_limit: 5,
            keepalive: Some(Duration::from_secs(1)),
            max_rx_len: 4096,
            accept_peer_open: false,
            flush_before_open: None,
            disconnect_stale: false,
        }
    }

    /// DDP on 0x6C0/0x6C1, us as the head unit. Values from the old driver `[dis]`.
    pub fn ddp() -> Self {
        Config {
            framing: Framing::Unprefixed,
            params: Params::DDP,
            max_block: wire::MAX_BLOCK,
            gap: Gap::Fixed(Duration::ZERO),
            msg_gap: Duration::ZERO,
            open_timeout: Duration::from_secs(1),
            ack_timeout: Duration::from_secs(1),
            ack_retries: 2,
            not_ready_wait: Duration::from_millis(100),
            not_ready_limit: 5,
            keepalive: Some(Duration::from_secs(2)),
            max_rx_len: 4096,
            accept_peer_open: true,
            flush_before_open: Some(Duration::from_secs(1)),
            disconnect_stale: true,
        }
    }
}

/// Handle for a queued message; reported back in [`Event::Sent`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MsgId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloseReason {
    /// We called `close`.
    Local,
    /// Peer sent A8.
    PeerDisconnect,
    /// Peer sent A0 on an open channel: it restarted.
    PeerRestart,
    SetupTimeout,
    /// D6..D8.
    SetupRefused(u8),
    ParamsTimeout,
    AckTimeout,
    KeepaliveTimeout,
    NotReadyLimit,
    Sequence {
        expected: u8,
        got: u8,
    },
    Length {
        declared: usize,
        got: usize,
    },
    TooLong,
}

/// Oddities that don't end the channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkError {
    /// An ACK that matches nothing in flight.
    StrayAck(u8),
    UnknownOpcode(u8),
    /// Data before the channel was open.
    DataWhileClosed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// Channel usable. `generation` is new for every open.
    Opened {
        generation: u32,
    },
    /// One complete received message (length prefix stripped).
    Message(Vec<u8>),
    /// All frames of a message were ACKed by the peer.
    Sent(MsgId),
    /// Peer sent `9n`. A KWP client extends its response deadline on this.
    NotReady,
    /// Channel is gone; every queued message was dropped. `generation` is the one
    /// that ended (unchanged if the open failed).
    Closed {
        generation: u32,
        reason: CloseReason,
    },
    Error(LinkError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendError {
    NotOpen,
    Empty,
    TooLong,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Dynamic { module: u8, tester_rx: u16 },
    Fixed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Idle,
    /// Sent A8 to flush a stale session; A0 follows at the deadline.
    Flushing,
    AwaitSetup,
    AwaitParams,
    Open,
}

struct TxMsg {
    id: MsgId,
    frames: Vec<Vec<u8>>,
    /// Frames confirmed by an ACK.
    acked: usize,
    /// Next frame to (re)send.
    next: usize,
    waiting: Option<AckWait>,
    not_ready: u8,
}

#[derive(Clone, Copy)]
struct AckWait {
    /// Index of the ACK-requesting frame.
    frame: usize,
    deadline: Instant,
    retries: u8,
}

/// One TP2.0 channel.
pub struct Channel {
    cfg: Config,
    mode: Mode,
    state: State,
    tx_id: u32,
    rx_id: u32,
    deadline: Option<Instant>,
    peer: Params,
    generation: u32,
    next_msg: u64,

    queue: VecDeque<(MsgId, Vec<u8>)>,
    cur: Option<TxMsg>,
    tx_seq: u8,
    next_frame_at: Option<Instant>,
    hold_until: Option<Instant>,

    rx_seq: u8,
    rx_buf: Vec<u8>,
    /// `(buffer length, rx_seq)` at the last ACK we sent, for A4.
    rx_mark: (usize, u8),
    last_acked_rx: Option<Vec<u8>>,

    ka_due: Option<Instant>,
    ka_wait: Option<Instant>,

    out: VecDeque<Frame>,
    events: VecDeque<Event>,
}

impl Channel {
    /// Dynamic diagnostic channel to TP2 `module` via setup on 0x200. We ask the
    /// module to transmit on `tester_rx`. Panics if `tester_rx` is 0 or not 11-bit.
    pub fn dynamic(module: u8, tester_rx: u16, cfg: Config) -> Self {
        assert!(tester_rx != 0 && tester_rx <= 0x7FF, "bad tester RX ID");
        Self::new(
            cfg,
            Mode::Dynamic { module, tester_rx },
            0,
            u32::from(tester_rx),
        )
    }

    /// Fixed, pre-agreed ID pair (DDP: `tx 0x6C0`, `rx 0x6C1`). No setup frame;
    /// `open` sends A0, or the peer's A0 opens it if `accept_peer_open`.
    pub fn fixed(tx_id: u16, rx_id: u16, cfg: Config) -> Self {
        assert!(
            tx_id <= 0x7FF && rx_id <= 0x7FF && tx_id != rx_id,
            "bad IDs"
        );
        Self::new(cfg, Mode::Fixed, u32::from(tx_id), u32::from(rx_id))
    }

    fn new(cfg: Config, mode: Mode, tx_id: u32, rx_id: u32) -> Self {
        Channel {
            cfg,
            mode,
            state: State::Idle,
            tx_id,
            rx_id,
            deadline: None,
            peer: cfg.params,
            generation: 0,
            next_msg: 0,
            queue: VecDeque::new(),
            cur: None,
            tx_seq: 0,
            next_frame_at: None,
            hold_until: None,
            rx_seq: 0,
            rx_buf: Vec::new(),
            rx_mark: (0, 0),
            last_acked_rx: None,
            ka_due: None,
            ka_wait: None,
            out: VecDeque::new(),
            events: VecDeque::new(),
        }
    }

    pub fn state(&self) -> State {
        self.state
    }
    pub fn is_open(&self) -> bool {
        self.state == State::Open
    }
    /// Bumps on every successful open; starts at 0 (never opened).
    pub fn generation(&self) -> u32 {
        self.generation
    }
    /// The peer's parameters from its A0/A1 (ours until it sends some).
    pub fn peer_params(&self) -> Params {
        self.peer
    }
    /// ID we transmit on (0 for a dynamic channel that isn't set up yet).
    pub fn tx_id(&self) -> u32 {
        self.tx_id
    }
    pub fn rx_id(&self) -> u32 {
        self.rx_id
    }
    /// TP2 module for dynamic channels.
    pub fn module(&self) -> Option<u8> {
        match self.mode {
            Mode::Dynamic { module, .. } => Some(module),
            Mode::Fixed => None,
        }
    }
    /// Messages queued or in flight.
    pub fn pending(&self) -> usize {
        self.queue.len() + usize::from(self.cur.is_some())
    }
    pub fn set_msg_gap(&mut self, gap: Duration) {
        self.cfg.msg_gap = gap;
    }
    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// Start opening. No-op unless idle.
    pub fn open(&mut self, now: Instant) {
        if self.state != State::Idle {
            return;
        }
        match self.mode {
            Mode::Dynamic { module, tester_rx } => {
                self.emit(wire::SETUP_ID, &wire::setup_request(module, tester_rx));
                self.state = State::AwaitSetup;
                self.deadline = Some(now + self.cfg.open_timeout);
            }
            Mode::Fixed => match self.cfg.flush_before_open {
                Some(wait) => {
                    self.emit(self.tx_id, &[wire::OP_DISCONNECT]);
                    self.state = State::Flushing;
                    self.deadline = Some(now + wait);
                }
                None => self.send_params_request(now),
            },
        }
    }

    /// Send A8 and drop everything. Emits `Closed(Local)` if not already idle.
    pub fn close(&mut self, _now: Instant) {
        if self.state == State::Idle {
            return;
        }
        if self.tx_id != 0 && self.state != State::AwaitSetup {
            self.emit(self.tx_id, &[wire::OP_DISCONNECT]);
        }
        self.reset(CloseReason::Local);
    }

    /// Queue one message. Segmented when it reaches the front of the queue.
    pub fn send(&mut self, now: Instant, msg: Vec<u8>) -> Result<MsgId, SendError> {
        if self.state != State::Open {
            return Err(SendError::NotOpen);
        }
        if msg.is_empty() {
            return Err(SendError::Empty);
        }
        if self.cfg.framing == Framing::LengthPrefixed && msg.len() > 0x7FFF {
            return Err(SendError::TooLong);
        }
        let id = MsgId(self.next_msg);
        self.next_msg += 1;
        self.queue.push_back((id, msg));
        self.pump(now);
        Ok(id)
    }

    pub fn poll_transmit(&mut self) -> Option<Frame> {
        self.out.pop_front()
    }

    pub fn poll_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    /// Earliest time `tick` has work to do.
    pub fn next_deadline(&self) -> Option<Instant> {
        let mut d = self.deadline;
        let mut take = |t: Option<Instant>| {
            if let Some(t) = t {
                d = Some(d.map_or(t, |x| x.min(t)));
            }
        };
        if self.state == State::Open {
            take(self.ka_wait.or(self.ka_due));
            take(self.hold_until);
            if let Some(m) = &self.cur {
                match m.waiting {
                    Some(w) => take(Some(w.deadline)),
                    None if self.hold_until.is_none() => take(self.next_frame_at),
                    None => {}
                }
            }
        }
        d
    }

    pub fn tick(&mut self, now: Instant) {
        match self.state {
            State::Idle => {}
            State::Flushing => {
                if self.deadline.is_some_and(|d| now >= d) {
                    self.send_params_request(now);
                }
            }
            State::AwaitSetup => {
                if self.deadline.is_some_and(|d| now >= d) {
                    self.reset(CloseReason::SetupTimeout);
                }
            }
            State::AwaitParams => {
                if self.deadline.is_some_and(|d| now >= d) {
                    self.emit(self.tx_id, &[wire::OP_DISCONNECT]);
                    self.reset(CloseReason::ParamsTimeout);
                }
            }
            State::Open => {
                if self.ka_wait.is_some_and(|d| now >= d) {
                    self.fail(CloseReason::KeepaliveTimeout);
                    return;
                }
                if self.hold_until.is_some_and(|t| now >= t) {
                    self.hold_until = None;
                }
                if let Some(m) = &mut self.cur
                    && let Some(w) = m.waiting
                    && now >= w.deadline
                {
                    if w.retries >= self.cfg.ack_retries {
                        self.fail(CloseReason::AckTimeout);
                        return;
                    }
                    // Resend only the ACK-requesting frame `[dis]`.
                    m.waiting = Some(AckWait {
                        frame: w.frame,
                        deadline: now + self.cfg.ack_timeout,
                        retries: w.retries + 1,
                    });
                    let f = m.frames[w.frame].clone();
                    self.emit(self.tx_id, &f);
                }
                self.pump(now);
                self.maybe_keepalive(now);
            }
        }
    }

    pub fn on_frame(&mut self, now: Instant, frame: &Frame) {
        if frame.extended {
            return;
        }
        if let Mode::Dynamic { module, tester_rx } = self.mode
            && frame.id == wire::SETUP_ID + u32::from(module)
        {
            if self.state == State::AwaitSetup {
                match wire::parse_setup_reply(frame.data()) {
                    Some(SetupReply::Accepted {
                        tester_rx: t,
                        module_rx,
                    }) if t == tester_rx => {
                        self.tx_id = u32::from(module_rx);
                        self.send_params_request(now);
                    }
                    Some(SetupReply::Refused(op)) => self.reset(CloseReason::SetupRefused(op)),
                    // Someone else's channel (VCDS on 0x300) or junk.
                    _ => {}
                }
            }
            return;
        }
        if frame.id != self.rx_id {
            return;
        }
        let Some(pdu) = wire::parse(frame.data()) else {
            return;
        };
        match self.state {
            State::Idle | State::Flushing => self.on_idle_pdu(now, pdu),
            State::AwaitSetup => {}
            State::AwaitParams => match pdu {
                Pdu::ParamsResponse(p) => self.opened(now, p),
                // Peer opened at the same time; take its parameters.
                Pdu::ParamsRequest(p) if self.mode == Mode::Fixed => {
                    self.emit_params_response();
                    self.opened(now, p);
                }
                Pdu::Disconnect => self.reset(CloseReason::PeerDisconnect),
                _ => {}
            },
            State::Open => self.on_open_pdu(now, pdu, frame.data()),
        }
    }

    // ----- open/close -------------------------------------------------------

    fn on_idle_pdu(&mut self, now: Instant, pdu: Pdu<'_>) {
        if self.mode != Mode::Fixed {
            return;
        }
        match pdu {
            Pdu::ParamsRequest(p) if self.cfg.accept_peer_open => {
                self.emit_params_response();
                self.deadline = None;
                self.opened(now, p);
            }
            Pdu::Test | Pdu::Data { .. } if self.cfg.disconnect_stale => {
                self.emit(self.tx_id, &[wire::OP_DISCONNECT]);
            }
            _ => {}
        }
    }

    fn send_params_request(&mut self, now: Instant) {
        self.emit(self.tx_id, &self.cfg.params.encode(wire::OP_PARAMS_REQ));
        self.state = State::AwaitParams;
        self.deadline = Some(now + self.cfg.open_timeout);
    }

    fn emit_params_response(&mut self) {
        self.emit(self.tx_id, &self.cfg.params.encode(wire::OP_PARAMS_RESP));
    }

    fn opened(&mut self, now: Instant, peer: Params) {
        self.state = State::Open;
        self.deadline = None;
        self.peer = peer;
        self.generation = self.generation.wrapping_add(1);
        self.tx_seq = 0;
        self.rx_seq = 0;
        self.rx_buf.clear();
        self.rx_mark = (0, 0);
        self.last_acked_rx = None;
        self.next_frame_at = Some(now);
        self.hold_until = None;
        self.ka_due = self.cfg.keepalive.map(|k| now + k);
        self.ka_wait = None;
        self.events.push_back(Event::Opened {
            generation: self.generation,
        });
    }

    /// Fatal error while open: A8, then reset.
    fn fail(&mut self, reason: CloseReason) {
        self.emit(self.tx_id, &[wire::OP_DISCONNECT]);
        self.reset(reason);
    }

    fn reset(&mut self, reason: CloseReason) {
        self.state = State::Idle;
        self.deadline = None;
        self.queue.clear();
        self.cur = None;
        self.rx_buf.clear();
        self.hold_until = None;
        self.ka_due = None;
        self.ka_wait = None;
        if let Mode::Dynamic { .. } = self.mode {
            self.tx_id = 0;
        }
        self.events.push_back(Event::Closed {
            generation: self.generation,
            reason,
        });
    }

    // ----- open channel -----------------------------------------------------

    fn on_open_pdu(&mut self, now: Instant, pdu: Pdu<'_>, raw: &[u8]) {
        match pdu {
            Pdu::Data {
                seq,
                last,
                wants_ack,
                payload,
            } => self.on_data(seq, last, wants_ack, payload, raw),
            Pdu::Ack { next, ready } => self.on_ack(now, next, ready),
            Pdu::Test => self.emit_params_response(),
            Pdu::ParamsResponse(p) => {
                // Keepalive answer.
                self.peer = p;
                self.ka_wait = None;
                self.ka_due = self.cfg.keepalive.map(|k| now + k);
            }
            Pdu::ParamsRequest(p) => {
                // Peer restarted (or a duplicate A0 right after open). Answer it;
                // if anything was exchanged, treat it as a new session.
                self.emit_params_response();
                let fresh = self.tx_seq == 0 && self.rx_seq == 0 && self.cur.is_none();
                if !fresh && self.mode == Mode::Fixed {
                    self.reset(CloseReason::PeerRestart);
                    self.opened(now, p);
                }
            }
            Pdu::Break => {
                self.rx_seq = self.rx_mark.1;
                self.rx_buf.truncate(self.rx_mark.0);
            }
            Pdu::Disconnect => {
                // `[jazdw]`: answer A8 with A8.
                self.emit(self.tx_id, &[wire::OP_DISCONNECT]);
                self.reset(CloseReason::PeerDisconnect);
            }
            Pdu::Unknown(op) => self
                .events
                .push_back(Event::Error(LinkError::UnknownOpcode(op))),
        }
    }

    fn on_data(&mut self, seq: u8, last: bool, wants_ack: bool, payload: &[u8], raw: &[u8]) {
        if seq != self.rx_seq {
            // The peer resent its last ACK-requesting frame because our ACK was
            // lost: ACK it again `[oldtp2]`.
            let prev = self.rx_seq.wrapping_sub(1) & 0x0F;
            if wants_ack && seq == prev && self.last_acked_rx.as_deref() == Some(raw) {
                self.emit(self.tx_id, &[wire::ack(self.rx_seq)]);
                return;
            }
            self.fail(CloseReason::Sequence {
                expected: self.rx_seq,
                got: seq,
            });
            return;
        }
        self.rx_seq = (seq + 1) & 0x0F;
        self.rx_buf.extend_from_slice(payload);
        if self.rx_buf.len() > self.cfg.max_rx_len + 2 {
            self.fail(CloseReason::TooLong);
            return;
        }
        if wants_ack {
            self.emit(self.tx_id, &[wire::ack(self.rx_seq)]);
            self.last_acked_rx = Some(raw.to_vec());
            self.rx_mark = (self.rx_buf.len(), self.rx_seq);
        }
        if !last {
            return;
        }
        let buf = std::mem::take(&mut self.rx_buf);
        self.rx_mark = (0, self.rx_seq);
        let msg = match self.cfg.framing {
            Framing::Unprefixed => buf,
            Framing::LengthPrefixed => {
                if buf.len() < 2 {
                    self.fail(CloseReason::Length {
                        declared: 0,
                        got: buf.len(),
                    });
                    return;
                }
                let word = usize::from(u16::from_be_bytes([buf[0], buf[1]]));
                let got = buf.len() - 2;
                // 15-bit per `[oldtp2]`, 12-bit per `[jazdw]`; both agree on 0x80.
                if word & 0x7FFF != got && word & 0x0FFF != got {
                    self.fail(CloseReason::Length {
                        declared: word & 0x7FFF,
                        got,
                    });
                    return;
                }
                buf[2..].to_vec()
            }
        };
        self.events.push_back(Event::Message(msg));
    }

    fn on_ack(&mut self, now: Instant, next: u8, ready: bool) {
        if !ready {
            self.events.push_back(Event::NotReady);
            self.hold_until = Some(now + self.cfg.not_ready_wait);
        }
        let Some(m) = &mut self.cur else {
            if ready {
                // Late or duplicate ACK; harmless.
                self.events
                    .push_back(Event::Error(LinkError::StrayAck(next)));
            }
            return;
        };
        // Which sent-but-unconfirmed frame does `next` follow?
        let mut confirmed = None;
        for i in m.acked..m.next.max(m.acked) {
            if (frame_seq(&m.frames[i]) + 1) & 0x0F == next {
                confirmed = Some(i + 1);
            }
        }
        let progress = match confirmed {
            Some(n) => {
                m.acked = n;
                true
            }
            None => m.acked < m.frames.len() && frame_seq(&m.frames[m.acked]) == next,
        };
        if !progress {
            if ready {
                self.events
                    .push_back(Event::Error(LinkError::StrayAck(next)));
            }
            return;
        }
        if !ready {
            m.not_ready += 1;
            if m.not_ready > self.cfg.not_ready_limit {
                self.fail(CloseReason::NotReadyLimit);
                return;
            }
        }
        // Resume from the first unconfirmed frame (after the hold, for 9n).
        m.next = m.acked;
        m.waiting = None;
        if m.acked == m.frames.len() {
            let id = m.id;
            self.cur = None;
            self.events.push_back(Event::Sent(id));
            self.next_frame_at = Some(now + self.cfg.msg_gap);
        }
        self.pump(now);
    }

    /// Send as many data frames as pacing allows.
    fn pump(&mut self, now: Instant) {
        if self.state != State::Open || self.hold_until.is_some() {
            return;
        }
        loop {
            if self.cur.is_none() {
                let Some((id, msg)) = self.queue.pop_front() else {
                    return;
                };
                let frames = self.segment(&msg);
                self.tx_seq = (self.tx_seq + frames.len() as u8) & 0x0F;
                self.cur = Some(TxMsg {
                    id,
                    frames,
                    acked: 0,
                    next: 0,
                    waiting: None,
                    not_ready: 0,
                });
            }
            if self.next_frame_at.is_some_and(|t| now < t) {
                return;
            }
            let gap = match self.cfg.gap {
                Gap::PeerT3 => self.peer.t3,
                Gap::Fixed(d) => d,
            };
            let ack_timeout = self.cfg.ack_timeout;
            let m = self.cur.as_mut().expect("set above");
            if m.waiting.is_some() || m.next >= m.frames.len() {
                return;
            }
            let i = m.next;
            let f = m.frames[i].clone();
            m.next += 1;
            if wire::parse(&f).is_some_and(|p| {
                matches!(
                    p,
                    Pdu::Data {
                        wants_ack: true,
                        ..
                    }
                )
            }) {
                m.waiting = Some(AckWait {
                    frame: i,
                    deadline: now + ack_timeout,
                    retries: 0,
                });
            }
            self.emit(self.tx_id, &f);
            self.next_frame_at = Some(now + gap);
        }
    }

    fn segment(&self, msg: &[u8]) -> Vec<Vec<u8>> {
        let mut wire_bytes = Vec::with_capacity(msg.len() + 2);
        if self.cfg.framing == Framing::LengthPrefixed {
            wire_bytes.extend_from_slice(&(msg.len() as u16).to_be_bytes());
        }
        wire_bytes.extend_from_slice(msg);
        let peer_bs = match self.peer.block_size {
            0 => wire::MAX_BLOCK,
            n => n,
        };
        let block = usize::from(self.cfg.max_block.clamp(1, wire::MAX_BLOCK).min(peer_bs));
        let chunks: Vec<&[u8]> = wire_bytes.chunks(7).collect();
        let n = chunks.len();
        chunks
            .into_iter()
            .enumerate()
            .map(|(i, c)| {
                let last = i + 1 == n;
                let boundary = (i + 1) % block == 0;
                let seq = (self.tx_seq.wrapping_add(i as u8)) & 0x0F;
                let mut f = Vec::with_capacity(8);
                f.push(wire::data_header(seq, last, last || boundary));
                f.extend_from_slice(c);
                f
            })
            .collect()
    }

    fn maybe_keepalive(&mut self, now: Instant) {
        if self.ka_wait.is_some() || self.ka_due.is_none_or(|d| now < d) {
            return;
        }
        // Not in the middle of an ACK block.
        if self.cur.as_ref().is_some_and(|m| m.waiting.is_some()) {
            return;
        }
        self.emit(self.tx_id, &[wire::OP_TEST]);
        self.ka_wait = Some(now + self.cfg.ack_timeout);
    }

    fn emit(&mut self, id: u32, data: &[u8]) {
        self.out
            .push_back(Frame::new(id, data).expect("11-bit ID, <= 8 bytes"));
    }
}

fn frame_seq(f: &[u8]) -> u8 {
    f[0] & 0x0F
}

/// Tester RX IDs for dynamic channels. One per open channel.
///
/// Default 0x308..=0x30F. 0x300 is VCDS's and is never handed out (PLAN 6).
/// `[jazdw]` suggests 0x300..0x310. The old worker used 0x300-0x306 but the only
/// trace shows 0x300; whether modules take other IDs is unproven on the car.
#[derive(Clone, Debug)]
pub struct TesterIds {
    free: Vec<u16>,
}

impl TesterIds {
    /// IDs outside 11 bits and 0x300 are dropped.
    pub fn new(ids: impl IntoIterator<Item = u16>) -> Self {
        let mut free: Vec<u16> = ids
            .into_iter()
            .filter(|&i| i != 0 && i <= 0x7FF && i != 0x300)
            .collect();
        free.sort_unstable();
        free.dedup();
        free.reverse();
        TesterIds { free }
    }

    pub fn alloc(&mut self) -> Option<u16> {
        self.free.pop()
    }

    pub fn release(&mut self, id: u16) {
        if id != 0x300 && !self.free.contains(&id) {
            self.free.push(id);
            self.free.sort_unstable_by(|a, b| b.cmp(a));
        }
    }

    pub fn available(&self) -> usize {
        self.free.len()
    }
}

impl Default for TesterIds {
    fn default() -> Self {
        TesterIds::new(0x308..=0x30F)
    }
}
