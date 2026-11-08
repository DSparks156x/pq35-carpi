//! OSEK network management observer and bus-state machine.
//!
//! The Pi is not a ring member and never sends NM frames. This watches the
//! ring (gateway 0x42B and RNS-E 0x436 on the TT, each about every 93 ms) and
//! decides whether the bus is awake, going to sleep, or silent. The TX gate
//! (plan step 3) asks it whether anything may be sent.
//!
//! Sans-IO: feed it frames and ticks with their times; it returns transitions.

use carpi_signals::ids;
use std::collections::BTreeMap;
use std::fmt;
use std::time::{Duration, Instant};

/// One decoded NM ring message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NmMessage {
    /// Sender's node address (`id - 0x420`).
    pub node: u8,
    /// Logical successor in the ring.
    pub successor: u8,
    pub ring: bool,
    pub alive: bool,
    pub limp_home: bool,
    pub sleep_ind: bool,
    pub sleep_ack: bool,
}

impl NmMessage {
    pub fn decode(id: u32, data: &[u8]) -> Option<Self> {
        if !ids::is_nm(id) || data.len() < 2 {
            return None;
        }
        let f = data[1];
        Some(NmMessage {
            node: (id - ids::NM_BASE) as u8,
            successor: data[0],
            ring: f & 0x01 != 0,
            alive: f & 0x02 != 0,
            limp_home: f & 0x04 != 0,
            sleep_ind: f & 0x10 != 0,
            sleep_ack: f & 0x20 != 0,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BusState {
    /// No frames for `silence`. Nothing may be sent; one frame wakes every node.
    Silent,
    /// Frames are flowing but the ring hasn't shown a member yet.
    Waking,
    /// Ring members present and at least one not asking to sleep.
    Awake,
    /// Every present member shows SleepInd, or one sent SleepAck.
    SleepPending,
}

impl fmt::Display for BusState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            BusState::Silent => "silent",
            BusState::Waking => "waking",
            BusState::Awake => "awake",
            BusState::SleepPending => "sleep-pending",
        };
        f.write_str(s)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// A member is gone if not heard from for this long.
    pub member_timeout: Duration,
    /// The bus is silent after this long with no frames at all.
    pub silence: Duration,
}

impl Default for Config {
    fn default() -> Self {
        // Ring messages come every ~93 ms; 0.5 s is five missed in a row.
        // Silence of 2 s is far beyond any ICAN cycle time.
        Config {
            member_timeout: Duration::from_millis(500),
            silence: Duration::from_secs(2),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transition {
    pub from: BusState,
    pub to: BusState,
}

#[derive(Clone, Copy, Debug)]
pub struct Member {
    pub last: NmMessage,
    pub at: Instant,
}

pub struct BusMonitor {
    cfg: Config,
    state: BusState,
    last_frame: Option<Instant>,
    members: BTreeMap<u8, Member>,
}

impl BusMonitor {
    pub fn new(cfg: Config) -> Self {
        BusMonitor {
            cfg,
            state: BusState::Silent,
            last_frame: None,
            members: BTreeMap::new(),
        }
    }

    pub fn state(&self) -> BusState {
        self.state
    }

    /// Feed every frame received on the bus.
    pub fn on_frame(&mut self, now: Instant, id: u32, data: &[u8]) -> Option<Transition> {
        self.last_frame = Some(now);
        if let Some(msg) = NmMessage::decode(id, data) {
            self.members.insert(msg.node, Member { last: msg, at: now });
        }
        self.evaluate(now)
    }

    /// Call periodically (every 100 ms is plenty).
    pub fn tick(&mut self, now: Instant) -> Option<Transition> {
        self.evaluate(now)
    }

    /// Members heard from within the timeout.
    pub fn members(&self, now: Instant) -> impl Iterator<Item = &Member> {
        let timeout = self.cfg.member_timeout;
        self.members
            .values()
            .filter(move |m| now.saturating_duration_since(m.at) <= timeout)
    }

    pub fn member(&self, node: u8, now: Instant) -> Option<&Member> {
        self.members(now).find(|m| m.last.node == node)
    }

    /// Node is present and not asking to sleep. For the RNS-E this gates
    /// everything we send it.
    pub fn node_awake(&self, node: u8, now: Instant) -> bool {
        self.member(node, now)
            .is_some_and(|m| !m.last.sleep_ind && !m.last.sleep_ack)
    }

    fn evaluate(&mut self, now: Instant) -> Option<Transition> {
        let silent = match self.last_frame {
            None => true,
            Some(t) => now.saturating_duration_since(t) > self.cfg.silence,
        };
        let next = if silent {
            BusState::Silent
        } else {
            let mut any = false;
            let mut all_sleep = true;
            let mut ack = false;
            for m in self.members(now) {
                any = true;
                all_sleep &= m.last.sleep_ind;
                ack |= m.last.sleep_ack;
            }
            if !any && self.state == BusState::SleepPending {
                // The ring stops before the last stragglers do; stay until silence.
                BusState::SleepPending
            } else if !any {
                BusState::Waking
            } else if ack || all_sleep {
                BusState::SleepPending
            } else {
                BusState::Awake
            }
        };
        if silent {
            self.members.clear();
        }
        if next == self.state {
            return None;
        }
        let t = Transition {
            from: self.state,
            to: next,
        };
        self.state = next;
        Some(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GW: u32 = 0x42B;
    const RNSE: u32 = 0x436;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn decode_ring() {
        let m = NmMessage::decode(GW, &[0x16, 0x01, 0, 0, 0, 0]).unwrap();
        assert_eq!(m.node, 0x0B);
        assert_eq!(m.successor, 0x16);
        assert!(m.ring && !m.sleep_ind);
        let m = NmMessage::decode(RNSE, &[0x0B, 0x11]).unwrap();
        assert!(m.sleep_ind);
        assert!(NmMessage::decode(0x5C3, &[0, 0]).is_none());
    }

    #[test]
    fn wake_awake_sleep_silent() {
        let t0 = Instant::now();
        let mut b = BusMonitor::new(Config::default());
        assert_eq!(b.state(), BusState::Silent);

        let t = b.on_frame(t0, 0x359, &[0; 8]).unwrap();
        assert_eq!(t.to, BusState::Waking);

        let t = b.on_frame(t0 + ms(10), GW, &[0x16, 0x01]).unwrap();
        assert_eq!(t.to, BusState::Awake);
        b.on_frame(t0 + ms(20), RNSE, &[0x0B, 0x01]);
        assert!(b.node_awake(0x16, t0 + ms(20)));

        // Gateway asks to sleep, RNS-E doesn't yet: still awake.
        assert!(b.on_frame(t0 + ms(100), GW, &[0x16, 0x11]).is_none());
        assert_eq!(b.state(), BusState::Awake);
        // Both ask: sleep pending.
        let t = b.on_frame(t0 + ms(110), RNSE, &[0x0B, 0x11]).unwrap();
        assert_eq!(t.to, BusState::SleepPending);
        assert!(!b.node_awake(0x16, t0 + ms(110)));

        // Ring wakes back up.
        let t = b.on_frame(t0 + ms(200), RNSE, &[0x0B, 0x01]).unwrap();
        assert_eq!(t.to, BusState::Awake);

        // Sleep ack, then nothing.
        let t = b.on_frame(t0 + ms(300), GW, &[0x16, 0x31]).unwrap();
        assert_eq!(t.to, BusState::SleepPending);
        assert!(b.tick(t0 + ms(2000)).is_none());
        let t = b.tick(t0 + ms(2400)).unwrap();
        assert_eq!(t.to, BusState::Silent);
        assert_eq!(b.members(t0 + ms(2400)).count(), 0);
    }

    #[test]
    fn member_timeout_drops_to_waking() {
        let t0 = Instant::now();
        let mut b = BusMonitor::new(Config::default());
        b.on_frame(t0, GW, &[0x16, 0x01]);
        assert_eq!(b.state(), BusState::Awake);
        // Other traffic continues but the ring stops.
        let t = b.on_frame(t0 + ms(800), 0x359, &[0; 8]).unwrap();
        assert_eq!(t.to, BusState::Waking);
    }
}
