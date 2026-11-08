//! DDP session over the TP2 link: handshake, centre claim, busy/free/reinit,
//! and one frame in flight. Sans-IO; drive it like a [`carpi_tp2::Channel`].
//!
//! Handshake (car + bench, `dc/NATIVE_HANDSHAKE_CONTROLLER.txt:61-71`):
//! `15 01 01 02 00 00` → `00 01` → `01 01 00`, `08` → `09 ..` → `20 3B A0 00` →
//! `21 ..` → `33`. Then claim `52 05 82 00 1B 40 30`; only `53 85` is ownership.
//! Startup on the bench: `53 84`, later `53 05`, `2E`; we send `2F` and claim
//! again (`BENCH_RESULTS.txt:181-185`). Keepalive (A3 every 2 s) is the link's.
//!
//! One generation counter: it bumps whenever ownership is gained or lost. The
//! register file resets with it, and screens repaint from scratch.

use crate::pack::{Message, Packed, Registers};
use crate::proto::{
    self, ANNOUNCE, CAPS_QUERY, CLAIM, Caps, OP_ERROR, OP_STATUS, OUR_CAPS, Profile, REINIT_ACK,
    REINIT_REQ, RELEASE, SETUP, Variant, WindowStatus,
};
use carpi_can::Frame;
use carpi_tp2::{Channel, CloseReason, Config, Event, MsgId};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug)]
pub struct SessionConfig {
    /// Variant for a mono `09` record we don't recognise (see [`Caps::profile`]).
    pub unknown_mono: Variant,
    /// Reopen the link this long after it closes. Ours.
    pub reopen_after: Duration,
    /// Give up on a handshake step after this. Ours.
    pub step_timeout: Duration,
    /// Wait for `53` after a claim. Old code: 1 s (`dc/dis_service.py:311`).
    pub claim_timeout: Duration,
    /// Retry after a claim timeout or `53 C0`. Old code: 5 s.
    pub claim_retry: Duration,
    /// After `53 05`, wait this long for `2E` before claiming anyway. Ours.
    pub free_wait: Duration,
    /// `0B 02 xx` recoveries per link session. Old code: 3 (`dc/ddp_protocol.py:1932-1971`).
    pub recovery_budget: u8,
    /// Extra message gap after a `9n`, for the next `backoff_msgs` messages. Ours.
    pub backoff_gap: Duration,
    pub backoff_msgs: u8,
}

impl Default for SessionConfig {
    fn default() -> Self {
        SessionConfig {
            unknown_mono: Variant::White,
            reopen_after: Duration::from_secs(2),
            step_timeout: Duration::from_secs(3),
            claim_timeout: Duration::from_secs(1),
            claim_retry: Duration::from_secs(5),
            free_wait: Duration::from_secs(1),
            recovery_budget: 3,
            backoff_gap: Duration::from_millis(20),
            backoff_msgs: 8,
        }
    }
}

/// Handshake progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    /// Link closed (or not started).
    Down,
    /// Link opening (A8 flush, A0/A1).
    Linking,
    AwaitMode,
    AwaitCaps,
    AwaitSetup,
    Ready,
}

/// Centre ownership, once `Ready`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Window {
    Idle,
    Claiming {
        until: Instant,
    },
    Owned,
    /// Cluster is using the centre (warning, menu, startup).
    Busy,
    /// `53 05` seen; claim at `until` unless a `2E` comes first.
    Free {
        until: Instant,
    },
    /// Claim timed out or faulted; try again at `at`.
    Retry {
        at: Instant,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionEvent {
    Ready(Profile),
    /// The `09` record had a format we don't render. The link is closed.
    Unsupported(Vec<u8>),
    Owned {
        generation: u32,
    },
    Lost {
        generation: u32,
    },
    Busy,
    Free,
    /// `53 C0`.
    Fault,
    /// `2E`/`2F` while we didn't own the centre: the driver picked our page in
    /// the cluster menu (`dc/dis_service.py:175-215`).
    ClusterSelected,
    /// `0B rr cc ..`. Late and not tied to a message (`[dis]` §1).
    AppError(Vec<u8>),
    FrameDone,
    FrameDropped,
    LinkClosed(CloseReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InFlight {
    Ctrl { close_after: bool },
    Draw { last: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubmitError {
    NotOwned,
    Busy,
}

pub struct Session {
    link: Channel,
    cfg: SessionConfig,
    enabled: bool,
    phase: Phase,
    window: Window,
    step_deadline: Option<Instant>,
    reopen_at: Option<Instant>,
    generation: u32,
    regs: Registers,
    caps: Option<Caps>,
    profile: Option<Profile>,
    want: bool,
    recoveries: u8,
    ctrl: VecDeque<(Vec<u8>, bool)>,
    frame: VecDeque<Message>,
    frame_active: bool,
    in_flight: Option<(MsgId, InFlight)>,
    backoff_left: u8,
    /// When the last message finished, and the gap the last drawing message asked for.
    last_done: Option<Instant>,
    prev_gap: Duration,
    hold_until: Option<Instant>,
    events: VecDeque<SessionEvent>,
}

impl Session {
    pub fn new(cfg: SessionConfig) -> Self {
        Session {
            link: Channel::fixed(proto::TX_ID, proto::RX_ID, Config::ddp()),
            cfg,
            enabled: false,
            phase: Phase::Down,
            window: Window::Idle,
            step_deadline: None,
            reopen_at: None,
            generation: 0,
            regs: Registers::default(),
            caps: None,
            profile: None,
            want: false,
            recoveries: 0,
            ctrl: VecDeque::new(),
            frame: VecDeque::new(),
            frame_active: false,
            in_flight: None,
            backoff_left: 0,
            last_done: None,
            prev_gap: Duration::ZERO,
            hold_until: None,
            events: VecDeque::new(),
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn window(&self) -> Window {
        self.window
    }
    pub fn owned(&self) -> bool {
        self.window == Window::Owned
    }
    pub fn generation(&self) -> u32 {
        self.generation
    }
    pub fn profile(&self) -> Option<Profile> {
        self.profile
    }
    pub fn caps(&self) -> Option<&Caps> {
        self.caps.as_ref()
    }
    pub fn registers(&self) -> Registers {
        self.regs
    }
    /// No frame in flight.
    pub fn frame_idle(&self) -> bool {
        !self.frame_active
    }
    pub fn link(&self) -> &Channel {
        &self.link
    }

    /// Open the link and keep it open.
    pub fn start(&mut self, now: Instant) {
        self.enabled = true;
        if self.phase == Phase::Down {
            self.phase = Phase::Linking;
            self.link.open(now);
        }
    }

    /// Close the link (A8) and stay down.
    pub fn stop(&mut self, now: Instant) {
        self.enabled = false;
        self.link.close(now);
        self.drain(now);
    }

    /// Whether we want the centre. Claims when possible; releases (`33`) when not.
    pub fn set_want(&mut self, now: Instant, want: bool) {
        if want == self.want {
            return;
        }
        self.want = want;
        if want {
            self.maybe_claim(now);
        } else if self.phase == Phase::Ready {
            if self.window == Window::Owned {
                self.lose();
                self.queue_ctrl(&RELEASE);
            }
            self.window = Window::Idle;
        }
        self.pump(now);
    }

    /// Hand over a packed frame. One at a time; the registers advance as if
    /// it lands (a loss resets them anyway).
    pub fn submit(&mut self, now: Instant, f: Packed) -> Result<(), SubmitError> {
        if !self.owned() {
            return Err(SubmitError::NotOwned);
        }
        if self.frame_active {
            return Err(SubmitError::Busy);
        }
        if f.messages.is_empty() {
            return Ok(());
        }
        self.frame = f.messages.into();
        self.frame_active = true;
        self.regs = f.regs;
        self.pump(now);
        Ok(())
    }

    pub fn on_frame(&mut self, now: Instant, f: &Frame) {
        self.link.on_frame(now, f);
        self.drain(now);
    }

    pub fn tick(&mut self, now: Instant) {
        if self.reopen_at.is_some_and(|t| now >= t) {
            self.reopen_at = None;
            if self.enabled && self.phase == Phase::Down {
                self.phase = Phase::Linking;
                self.link.open(now);
            }
        }
        self.link.tick(now);
        self.drain(now);
        if self.step_deadline.is_some_and(|t| now >= t) {
            self.step_deadline = None;
            self.link.close(now);
            self.drain(now);
        }
        match self.window {
            Window::Claiming { until } if now >= until => {
                self.window = Window::Retry {
                    at: now + self.cfg.claim_retry,
                };
            }
            Window::Free { until } | Window::Retry { at: until } if now >= until => {
                self.window = Window::Idle;
                self.maybe_claim(now);
            }
            _ => {}
        }
        self.pump(now);
    }

    pub fn poll_transmit(&mut self) -> Option<Frame> {
        self.link.poll_transmit()
    }

    pub fn poll_event(&mut self) -> Option<SessionEvent> {
        self.events.pop_front()
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        let w = match self.window {
            Window::Claiming { until } | Window::Free { until } => Some(until),
            Window::Retry { at } => Some(at),
            _ => None,
        };
        [
            self.link.next_deadline(),
            self.step_deadline,
            self.reopen_at,
            self.hold_until,
            w,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    // ----- internals --------------------------------------------------------

    fn drain(&mut self, now: Instant) {
        while let Some(e) = self.link.poll_event() {
            match e {
                Event::Opened { .. } => {
                    self.phase = Phase::AwaitMode;
                    self.window = Window::Idle;
                    self.recoveries = 0;
                    self.caps = None;
                    self.ctrl.clear();
                    self.in_flight = None;
                    self.queue_ctrl(&ANNOUNCE);
                    self.step_deadline = Some(now + self.cfg.step_timeout);
                }
                Event::Message(m) if !m.is_empty() => self.on_app(now, &m),
                Event::Message(_) => {}
                Event::Sent(id) => self.on_sent(now, id),
                Event::NotReady => self.backoff_left = self.cfg.backoff_msgs,
                Event::Closed { reason, .. } => {
                    self.lose();
                    self.phase = Phase::Down;
                    self.window = Window::Idle;
                    self.step_deadline = None;
                    self.ctrl.clear();
                    self.in_flight = None;
                    self.events.push_back(SessionEvent::LinkClosed(reason));
                    if self.enabled {
                        self.reopen_at = Some(now + self.cfg.reopen_after);
                    }
                }
                Event::Error(_) => {}
            }
        }
        self.pump(now);
    }

    fn on_sent(&mut self, now: Instant, id: MsgId) {
        let Some((fid, kind)) = self.in_flight else {
            return;
        };
        if fid != id {
            return;
        }
        self.in_flight = None;
        self.last_done = Some(now);
        match kind {
            InFlight::Ctrl { close_after: true } => {
                self.link.close(now);
            }
            InFlight::Ctrl { .. } => {}
            InFlight::Draw { last: true } => {
                if self.frame_active {
                    self.frame_active = false;
                    self.events.push_back(SessionEvent::FrameDone);
                }
            }
            InFlight::Draw { .. } => {}
        }
    }

    fn on_app(&mut self, now: Instant, m: &[u8]) {
        match m[0] {
            0x00 => self.on_mode(now, m),
            0x08 => {
                if self.phase >= Phase::AwaitCaps {
                    self.queue_ctrl(&OUR_CAPS);
                } else {
                    self.queue_ctrl(&[OP_ERROR, 0x02, 0x08]);
                }
            }
            0x09 if self.phase == Phase::AwaitCaps => {
                let caps = Caps::parse(m);
                match caps.as_ref().and_then(|c| c.profile(self.cfg.unknown_mono)) {
                    Some(p) => {
                        self.profile = Some(p);
                        self.caps = caps;
                        self.phase = Phase::AwaitSetup;
                        self.queue_ctrl(&SETUP);
                        self.step_deadline = Some(now + self.cfg.step_timeout);
                    }
                    None => {
                        self.events.push_back(SessionEvent::Unsupported(m.to_vec()));
                        self.enabled = false;
                        self.link.close(now);
                    }
                }
            }
            0x21 if self.phase == Phase::AwaitSetup => {
                self.phase = Phase::Ready;
                self.step_deadline = None;
                self.window = Window::Idle;
                self.queue_ctrl(&RELEASE);
                if let Some(p) = self.profile {
                    self.events.push_back(SessionEvent::Ready(p));
                }
                self.maybe_claim(now);
            }
            0x14 => {
                // Cluster restarted its application: announce again, redo setup
                // (`dc/ddp_protocol.py:560-566`).
                self.queue_ctrl(&ANNOUNCE);
                self.lose();
                self.window = Window::Idle;
                self.phase = Phase::AwaitMode;
                self.step_deadline = Some(now + self.cfg.step_timeout);
            }
            // 5B/7B carry the same mask (`dc/ddp_application.py:71-87`).
            OP_STATUS | 0x5B | 0x7B if m.len() >= 2 => self.on_status(now, m[1]),
            REINIT_REQ | 0x2F => {
                if m[0] == REINIT_REQ {
                    self.queue_ctrl(&REINIT_ACK);
                }
                match self.window {
                    Window::Owned => self.lose(),
                    Window::Idle | Window::Retry { .. } => {
                        self.events.push_back(SessionEvent::ClusterSelected)
                    }
                    // The expected reinit after a free or a busy spell.
                    _ => {}
                }
                self.window = Window::Idle;
                self.maybe_claim(now);
            }
            OP_ERROR => {
                self.events.push_back(SessionEvent::AppError(m.to_vec()));
                if m.get(1) == Some(&0x02)
                    && self.phase >= Phase::AwaitSetup
                    && self.recoveries < self.cfg.recovery_budget
                {
                    // Incompatible context: mode reply, caps, setup again.
                    self.recoveries += 1;
                    self.lose();
                    self.window = Window::Idle;
                    self.phase = Phase::AwaitCaps;
                    self.queue_ctrl(&[0x01, 0x01, 0x00]);
                    self.queue_ctrl(&CAPS_QUERY);
                    self.step_deadline = Some(now + self.cfg.step_timeout);
                }
            }
            op if op & 1 == 0 => {
                // Unknown even request: "unsupported" (`dc/ddp_protocol.py:567-568`).
                self.queue_ctrl(&[OP_ERROR, 0x01, op]);
            }
            _ => {}
        }
    }

    fn on_mode(&mut self, now: Instant, m: &[u8]) {
        match m.get(1) {
            Some(1) => {
                self.queue_ctrl(&[0x01, 0x01, 0x00]);
                match self.phase {
                    Phase::AwaitMode => {
                        self.phase = Phase::AwaitCaps;
                        self.queue_ctrl(&CAPS_QUERY);
                        self.step_deadline = Some(now + self.cfg.step_timeout);
                    }
                    // A repeat during discovery only gets its reply
                    // (`dc/NATIVE_HANDSHAKE_CONTROLLER.txt:114-130`).
                    Phase::AwaitCaps => {}
                    Phase::AwaitSetup | Phase::Ready => {
                        self.lose();
                        self.window = Window::Idle;
                        self.phase = Phase::AwaitCaps;
                        self.queue_ctrl(&CAPS_QUERY);
                        self.step_deadline = Some(now + self.cfg.step_timeout);
                    }
                    Phase::Down | Phase::Linking => {}
                }
            }
            // Mode 2 is unsupported: reply, then close.
            Some(2) => self.ctrl.push_back((vec![0x01, 0x02, 0x00], true)),
            _ => self.queue_ctrl(&[OP_ERROR, 0x03, 0x00]),
        }
    }

    fn on_status(&mut self, now: Instant, v: u8) {
        match WindowStatus::parse(v) {
            WindowStatus::Granted => {
                if matches!(self.window, Window::Claiming { .. }) {
                    self.window = Window::Owned;
                    self.generation = self.generation.wrapping_add(1);
                    self.regs = Registers::default();
                    self.events.push_back(SessionEvent::Owned {
                        generation: self.generation,
                    });
                }
            }
            WindowStatus::Busy => {
                self.lose();
                self.window = Window::Busy;
                self.events.push_back(SessionEvent::Busy);
            }
            WindowStatus::Free => {
                self.lose();
                self.window = Window::Free {
                    until: now + self.cfg.free_wait,
                };
                self.events.push_back(SessionEvent::Free);
            }
            WindowStatus::Fault => {
                self.lose();
                self.window = Window::Retry {
                    at: now + self.cfg.claim_retry,
                };
                self.events.push_back(SessionEvent::Fault);
            }
        }
    }

    fn maybe_claim(&mut self, now: Instant) {
        if self.phase == Phase::Ready && self.want && self.window == Window::Idle {
            self.queue_ctrl(&CLAIM);
            self.window = Window::Claiming {
                until: now + self.cfg.claim_timeout,
            };
        }
    }

    /// Drop ownership (if held): new generation, registers unknown, frame gone.
    fn lose(&mut self) {
        if self.window == Window::Owned {
            self.generation = self.generation.wrapping_add(1);
            self.regs = Registers::default();
            self.events.push_back(SessionEvent::Lost {
                generation: self.generation,
            });
        }
        if self.frame_active {
            self.frame_active = false;
            self.frame.clear();
            self.events.push_back(SessionEvent::FrameDropped);
        }
    }

    fn queue_ctrl(&mut self, m: &[u8]) {
        self.ctrl.push_back((m.to_vec(), false));
    }

    /// Feed the link one message at a time: control first, then the frame.
    fn pump(&mut self, now: Instant) {
        self.hold_until = None;
        if self.in_flight.is_some() || !self.link.is_open() {
            return;
        }
        let extra = if self.backoff_left > 0 {
            self.cfg.backoff_gap
        } else {
            Duration::ZERO
        };
        if let Some((m, close_after)) = self.ctrl.pop_front() {
            self.link.set_msg_gap(extra);
            if let Ok(id) = self.link.send(now, m) {
                self.in_flight = Some((id, InFlight::Ctrl { close_after }));
            }
            return;
        }
        if !self.frame_active || self.window != Window::Owned {
            return;
        }
        let Some(gap) = self.frame.front().map(|m| m.gap) else {
            return;
        };
        // Stroke spacing on both sides of a stroke message, across frames too.
        let need = gap.max(self.prev_gap);
        if let Some(t) = self.last_done
            && now < t + need
        {
            self.hold_until = Some(t + need);
            return;
        }
        let m = self.frame.pop_front().expect("checked");
        self.link.set_msg_gap(extra);
        self.backoff_left = self.backoff_left.saturating_sub(1);
        self.prev_gap = m.gap;
        let last = self.frame.is_empty();
        if let Ok(id) = self.link.send(now, m.bytes) {
            self.in_flight = Some((id, InFlight::Draw { last }));
        }
    }
}
