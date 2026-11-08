//! KWP2000 request/response client over one TP2 channel. Sans-IO.
//!
//! One request outstanding at a time. Handles `7F xx 78` (response pending),
//! optional busy retry on `7F xx 21`, echo checks, and `9n` deadline extension.
//! Values from RNS-E-Hudiy `flasher/vag_protocols/kwp.py` (code that ran on the
//! car for the Haldex) unless marked ours.

use crate::request::{ExclusiveToken, Request};
use carpi_can::Frame;
use carpi_tp2::{Channel, CloseReason, Event as LinkEvent, LinkError};
use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

/// Per-module behaviour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Profile {
    /// `false`: refuse to open or request without an exclusive token.
    pub allow_polling: bool,
    /// First response wait. Old worker: 2 s. ECU first frame is 23-40 ms after the
    /// request on the engine (proven).
    pub response_timeout: Duration,
    /// Each `7F xx 78` moves the deadline to now + this. Ours.
    pub pending_step: Duration,
    /// Hard cap from the request, however many 78s. Old: 30 s.
    pub pending_cap: Duration,
    /// Max 78s per request. Old: 30.
    pub pending_limit: u16,
    /// Each `9n` adds this to the deadline (capped by `pending_cap`). Old: 1 s.
    pub not_ready_extend: Duration,
    /// Resend on NRC 0x21 this many times. Old default: 0.
    pub busy_retries: u8,
    pub busy_delay: Duration,
}

impl Default for Profile {
    fn default() -> Self {
        Profile {
            allow_polling: true,
            response_timeout: Duration::from_secs(2),
            pending_step: Duration::from_secs(5),
            pending_cap: Duration::from_secs(30),
            pending_limit: 30,
            not_ready_extend: Duration::from_secs(1),
            busy_retries: 0,
            busy_delay: Duration::from_millis(200),
        }
    }
}

/// TP2 ID of ABS. Polling it causes an ESP fault and upsets the Haldex on Drew's
/// car (`references/HALDEX_LOGGING_SPEC.md` in RNS-E-Hudiy; PLAN 2.8). The TP2 ID
/// equals the VCDS address here by assumption; check the install list.
pub const ABS: u8 = 0x03;

/// Profiles keyed by TP2 module ID.
#[derive(Clone, Debug)]
pub struct Profiles {
    pub default: Profile,
    pub modules: BTreeMap<u8, Profile>,
}

impl Default for Profiles {
    /// ABS denied.
    fn default() -> Self {
        let mut p = Profiles {
            default: Profile::default(),
            modules: BTreeMap::new(),
        };
        p.deny(ABS);
        p
    }
}

impl Profiles {
    pub fn get(&self, module: u8) -> Profile {
        self.modules.get(&module).copied().unwrap_or(self.default)
    }
    pub fn deny(&mut self, module: u8) {
        let mut p = self.get(module);
        p.allow_polling = false;
        self.modules.insert(module, p);
    }
    pub fn is_denied(&self, module: u8) -> bool {
        !self.get(module).allow_polling
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ReqId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KwpError {
    /// Module denied by profile and no token.
    Denied,
    NotOpen,
    /// A request is already outstanding.
    Busy,
    /// `7F sid nrc`.
    Negative {
        nrc: u8,
    },
    Timeout,
    PendingTimeout,
    /// Reply SID or echo byte doesn't match the request.
    Mismatch(Vec<u8>),
    LinkClosed(CloseReason),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KwpEvent {
    Opened {
        generation: u32,
    },
    Closed {
        generation: u32,
        reason: CloseReason,
    },
    /// `7F xx 78` seen; still waiting.
    Pending(ReqId),
    Done {
        id: ReqId,
        result: Result<Vec<u8>, KwpError>,
    },
    /// A message with no request outstanding.
    Unsolicited(Vec<u8>),
    Link(LinkError),
}

struct Outstanding {
    id: ReqId,
    req: Request,
    started: Instant,
    deadline: Instant,
    pending: u16,
    busy: u8,
    retry_at: Option<Instant>,
}

pub struct KwpClient {
    link: Channel,
    profile: Profile,
    cur: Option<Outstanding>,
    next_id: u64,
    events: VecDeque<KwpEvent>,
}

/// SIDs whose positive reply echoes byte 1 of the request: the old `KWPProfile`
/// echo set plus 21 (`61 gg`, proven).
const ECHO: &[u8] = &[0x10, 0x11, 0x1A, 0x21, 0x27, 0x31, 0x33];

impl KwpClient {
    /// Profile chosen by the channel's module (fixed channels get the default).
    pub fn new(link: Channel, profiles: &Profiles) -> Self {
        let profile = link.module().map_or(profiles.default, |m| profiles.get(m));
        KwpClient {
            link,
            profile,
            cur: None,
            next_id: 0,
            events: VecDeque::new(),
        }
    }

    pub fn link(&self) -> &Channel {
        &self.link
    }
    pub fn link_mut(&mut self) -> &mut Channel {
        &mut self.link
    }
    pub fn profile(&self) -> &Profile {
        &self.profile
    }
    pub fn is_open(&self) -> bool {
        self.link.is_open()
    }
    pub fn busy(&self) -> bool {
        self.cur.is_some()
    }

    /// Open the channel. Refused for denied modules.
    pub fn open(&mut self, now: Instant) -> Result<(), KwpError> {
        if !self.profile.allow_polling {
            return Err(KwpError::Denied);
        }
        self.link.open(now);
        Ok(())
    }

    /// Open a denied module under an exclusive token (a deliberate one-off job).
    pub fn open_exclusive(&mut self, now: Instant, _token: &ExclusiveToken) {
        self.link.open(now);
    }

    pub fn close(&mut self, now: Instant) {
        self.link.close(now);
        self.drain_link(now);
    }

    pub fn request(&mut self, now: Instant, req: Request) -> Result<ReqId, KwpError> {
        if !self.profile.allow_polling && req.lease().is_none() {
            return Err(KwpError::Denied);
        }
        if self.cur.is_some() {
            return Err(KwpError::Busy);
        }
        self.link
            .send(now, req.bytes().to_vec())
            .map_err(|_| KwpError::NotOpen)?;
        let id = ReqId(self.next_id);
        self.next_id += 1;
        self.cur = Some(Outstanding {
            id,
            req,
            started: now,
            deadline: now + self.profile.response_timeout,
            pending: 0,
            busy: 0,
            retry_at: None,
        });
        self.drain_link(now);
        Ok(id)
    }

    pub fn on_frame(&mut self, now: Instant, frame: &Frame) {
        self.link.on_frame(now, frame);
        self.drain_link(now);
    }

    pub fn tick(&mut self, now: Instant) {
        self.link.tick(now);
        self.drain_link(now);
        let Some(c) = &mut self.cur else { return };
        if let Some(t) = c.retry_at
            && now >= t
        {
            c.retry_at = None;
            c.deadline = now + self.profile.response_timeout;
            let bytes = c.req.bytes().to_vec();
            if self.link.send(now, bytes).is_err() {
                self.finish(Err(KwpError::NotOpen));
            }
            self.drain_link(now);
            return;
        }
        if c.retry_at.is_none() && now >= c.deadline {
            let e = if c.pending > 0 {
                KwpError::PendingTimeout
            } else {
                KwpError::Timeout
            };
            self.finish(Err(e));
        }
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        let mine = self.cur.as_ref().map(|c| c.retry_at.unwrap_or(c.deadline));
        match (self.link.next_deadline(), mine) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    pub fn poll_transmit(&mut self) -> Option<Frame> {
        self.link.poll_transmit()
    }

    pub fn poll_event(&mut self) -> Option<KwpEvent> {
        self.events.pop_front()
    }

    fn finish(&mut self, result: Result<Vec<u8>, KwpError>) {
        if let Some(c) = self.cur.take() {
            self.events.push_back(KwpEvent::Done { id: c.id, result });
        }
    }

    fn drain_link(&mut self, now: Instant) {
        while let Some(ev) = self.link.poll_event() {
            match ev {
                LinkEvent::Opened { generation } => {
                    self.events.push_back(KwpEvent::Opened { generation })
                }
                LinkEvent::Closed { generation, reason } => {
                    self.finish(Err(KwpError::LinkClosed(reason.clone())));
                    self.events
                        .push_back(KwpEvent::Closed { generation, reason });
                }
                LinkEvent::Message(m) => self.on_message(now, m),
                LinkEvent::NotReady => {
                    if let Some(c) = &mut self.cur {
                        let cap = c.started + self.profile.pending_cap;
                        c.deadline = (c.deadline + self.profile.not_ready_extend).min(cap);
                    }
                }
                LinkEvent::Sent(_) => {}
                LinkEvent::Error(e) => self.events.push_back(KwpEvent::Link(e)),
            }
        }
    }

    fn on_message(&mut self, now: Instant, m: Vec<u8>) {
        let p = self.profile;
        let Some(c) = &mut self.cur else {
            self.events.push_back(KwpEvent::Unsolicited(m));
            return;
        };
        let req = c.req.bytes();
        let sid = req[0];
        if m.first() == Some(&0x7F) {
            if m.len() != 3 || m[1] != sid {
                self.finish(Err(KwpError::Mismatch(m)));
                return;
            }
            match m[2] {
                0x78 => {
                    c.pending += 1;
                    let cap = c.started + p.pending_cap;
                    if c.pending > p.pending_limit || now >= cap {
                        self.finish(Err(KwpError::PendingTimeout));
                        return;
                    }
                    c.deadline = (now + p.pending_step).min(cap);
                    let id = c.id;
                    self.events.push_back(KwpEvent::Pending(id));
                }
                0x21 if c.busy < p.busy_retries => {
                    c.busy += 1;
                    c.retry_at = Some(now + p.busy_delay);
                }
                nrc => self.finish(Err(KwpError::Negative { nrc })),
            }
            return;
        }
        let sid_ok = m.first() == Some(&(sid.wrapping_add(0x40)));
        let echo_ok = !ECHO.contains(&sid) || req.len() < 2 || m.get(1) == req.get(1);
        if sid_ok && echo_ok {
            self.finish(Ok(m));
        } else {
            self.finish(Err(KwpError::Mismatch(m)));
        }
    }
}
