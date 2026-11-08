//! The TX gate. Nothing is sent except through it.
//!
//! Every frame carries a [`TxClass`]. The gate checks the frame against a
//! [`BusView`] (bus state, RNS-E awake, Kl15) and the class policy, then a
//! per-class rate cap. Refused frames are dropped and counted, never queued.
//!
//! Policy (PLAN.md 2.1, 2.3):
//! - Nothing in `Silent`, `SleepPending` or `Waking`. One frame wakes every
//!   ICAN node, and the Pi never holds the bus awake.
//! - `Override`, `DisplayTop`: only while the RNS-E node is awake on the ring.
//! - `DisplayDdp`, `Diag`, `Extra`: only with Kl15 on. Unknown Kl15 counts as off.
//! - While a lease is held, only `Lease` passes. `Lease` needs a lease, Kl15
//!   and an awake bus.
//! - Fixed-ID classes are checked against their IDs, and NM IDs never pass.
//!
//! One gate per interface. A separate diag bus has no ring the Pi can see, so
//! its view is built from the ICAN monitor and store as well.
//!
//! Sans-IO: the caller passes `now` and the view; the gate never sends.

use carpi_can::Frame;
use carpi_nm::{BusMonitor, BusState};
use carpi_signals::ids;
use carpi_state::Store;
use std::fmt;
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// What a frame is for. Decides the policy and rate cap it gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TxClass {
    /// 0x7B0 RNS-E firmware commands.
    Override,
    /// 0x363/0x365 DIS top lines.
    DisplayTop,
    /// 0x6C0 DDP session to the cluster, 0x7B3 cluster role toward the RNS-E.
    DisplayDdp,
    /// TP2.0: 0x200 setup and channel IDs.
    Diag,
    /// Drew's add-ons (exhaust, Haldex mode, ...).
    Extra,
    /// Frames sent on behalf of whoever holds the bus lease.
    Lease,
}

impl TxClass {
    pub const ALL: [TxClass; 6] = [
        TxClass::Override,
        TxClass::DisplayTop,
        TxClass::DisplayDdp,
        TxClass::Diag,
        TxClass::Extra,
        TxClass::Lease,
    ];

    fn idx(self) -> usize {
        self as usize
    }

    pub fn name(self) -> &'static str {
        match self {
            TxClass::Override => "override",
            TxClass::DisplayTop => "display-top",
            TxClass::DisplayDdp => "display-ddp",
            TxClass::Diag => "diag",
            TxClass::Extra => "extra",
            TxClass::Lease => "lease",
        }
    }
}

impl fmt::Display for TxClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// 0x7B0 opcodes the gate lets through: `BA`-`BF`. `B0`-`B9` are boot,
/// flash-cave, RAM-write and Doom commands on the SD-link firmware (`B7 00`-`03`
/// overwrite flash descriptor slots); those go through a bus lease or not at all.
const OVERRIDE_OPCODES: std::ops::RangeInclusive<u8> = 0xBA..=0xBF;

/// The bus as the gate sees it at one instant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BusView {
    pub bus: BusState,
    /// RNS-E present on the ring and not asking to sleep.
    pub rnse_awake: bool,
    /// Terminal 15. `None` = unknown (stale), treated as off.
    pub kl15: Option<bool>,
}

impl BusView {
    /// Builds the view from the ICAN monitor and the store.
    pub fn new(monitor: &BusMonitor, store: &Store, now: Instant) -> Self {
        BusView {
            bus: monitor.state(),
            rnse_awake: monitor.node_awake(ids::NM_NODE_RNSE, now),
            kl15: kl15(store, now),
        }
    }

    fn kl15_on(&self) -> bool {
        self.kl15 == Some(true)
    }
}

/// Kl15 from the BCM (0x575) or the ignition switch (0x2C3), whichever is
/// fresh. On if either says on; off only if a fresh source says off.
pub fn kl15(store: &Store, now: Instant) -> Option<bool> {
    let a = store.get_bool("ignition.kl15", now);
    let b = store.get_bool("ignition.zas.kl15", now);
    match (a, b) {
        (Some(true), _) | (_, Some(true)) => Some(true),
        (Some(false), _) | (_, Some(false)) => Some(false),
        _ => None,
    }
}

/// Why a frame was dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Refused {
    BusSilent,
    SleepPending,
    /// Frames flowing but no ring member seen yet.
    BusWaking,
    RnseAsleep,
    Kl15Off,
    /// Someone else holds the bus lease.
    Leased,
    /// `Lease` class frame without a lease.
    NoLease,
    RateCap,
    /// ID or DLC not allowed for this class.
    BadFrame,
    /// 0x7B0 opcode outside `BA`-`BF`.
    ForbiddenOpcode,
    /// The Pi never sends NM frames.
    NmId,
}

impl Refused {
    const COUNT: usize = 11;

    pub const ALL: [Refused; Refused::COUNT] = [
        Refused::BusSilent,
        Refused::SleepPending,
        Refused::BusWaking,
        Refused::RnseAsleep,
        Refused::Kl15Off,
        Refused::Leased,
        Refused::NoLease,
        Refused::RateCap,
        Refused::BadFrame,
        Refused::ForbiddenOpcode,
        Refused::NmId,
    ];

    fn idx(self) -> usize {
        self as usize
    }
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Refused::BusSilent => "bus silent",
            Refused::SleepPending => "bus going to sleep",
            Refused::BusWaking => "bus waking",
            Refused::RnseAsleep => "RNS-E asleep",
            Refused::Kl15Off => "Kl15 off",
            Refused::Leased => "bus leased",
            Refused::NoLease => "no lease",
            Refused::RateCap => "rate cap",
            Refused::BadFrame => "ID/DLC not allowed for class",
            Refused::ForbiddenOpcode => "forbidden 0x7B0 opcode",
            Refused::NmId => "NM ID",
        };
        f.write_str(s)
    }
}

impl std::error::Error for Refused {}

/// Token bucket: `per_sec` sustained, `burst` back to back.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rate {
    pub per_sec: f64,
    pub burst: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub rates: [Rate; 6],
}

impl Config {
    pub fn rate(&self, class: TxClass) -> Rate {
        self.rates[class.idx()]
    }

    pub fn set_rate(&mut self, class: TxClass, rate: Rate) {
        self.rates[class.idx()] = rate;
    }
}

impl Default for Config {
    /// Caps catch runaway senders; they aren't a budget. 100 kbit/s ICAN
    /// carries ~800 frames/s at most.
    fn default() -> Self {
        let r = |per_sec, burst| Rate { per_sec, burst };
        Config {
            rates: [
                // One frame per user action; re-assert sends two.
                r(5.0, 8.0),
                // 0x363 + 0x365 at 10 Hz.
                r(25.0, 10.0),
                // A full-centre bitmap is ~95 frames, 4-5 a second on the bench.
                r(500.0, 200.0),
                // ~20 group reads/s per channel, a few channels.
                r(200.0, 100.0),
                r(50.0, 10.0),
                r(1000.0, 200.0),
            ],
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Bucket {
    tokens: f64,
    last: Option<Instant>,
    /// Currently refusing on the cap; logs once per episode.
    capped: bool,
}

impl Bucket {
    fn take(&mut self, rate: Rate, now: Instant) -> bool {
        let dt = match self.last {
            Some(t) => now.saturating_duration_since(t).as_secs_f64(),
            None => f64::INFINITY,
        };
        self.last = Some(now);
        self.tokens = (self.tokens + dt * rate.per_sec).min(rate.burst);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Per-class counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClassStats {
    pub passed: u64,
    refused: [u64; Refused::COUNT],
}

impl ClassStats {
    pub fn refused(&self, why: Refused) -> u64 {
        self.refused[why.idx()]
    }

    pub fn refused_total(&self) -> u64 {
        self.refused.iter().sum()
    }
}

pub struct Gate {
    name: String,
    cfg: Config,
    buckets: [Bucket; 6],
    stats: [ClassStats; 6],
    lease_until: Option<Instant>,
}

impl Gate {
    /// `name` is the interface, for logs.
    pub fn new(name: impl Into<String>, cfg: Config) -> Self {
        let bucket = Bucket {
            tokens: 0.0,
            last: None,
            capped: false,
        };
        Gate {
            name: name.into(),
            cfg,
            buckets: [bucket; 6],
            stats: [ClassStats::default(); 6],
            lease_until: None,
        }
    }

    /// May this frame be sent now? Counts the result either way. On `Ok` the
    /// caller sends the frame; on `Err` it drops it.
    pub fn check(
        &mut self,
        class: TxClass,
        frame: &Frame,
        now: Instant,
        view: &BusView,
    ) -> Result<(), Refused> {
        self.expire_lease(now);
        let r = self.decide(class, frame, now, view);
        let s = &mut self.stats[class.idx()];
        match r {
            Ok(()) => s.passed += 1,
            Err(why) => s.refused[why.idx()] += 1,
        }
        r
    }

    fn decide(
        &mut self,
        class: TxClass,
        frame: &Frame,
        now: Instant,
        view: &BusView,
    ) -> Result<(), Refused> {
        if !frame.extended && ids::is_nm(frame.id) {
            return Err(Refused::NmId);
        }
        check_frame(class, frame)?;
        match view.bus {
            BusState::Silent => return Err(Refused::BusSilent),
            BusState::SleepPending => return Err(Refused::SleepPending),
            BusState::Waking => return Err(Refused::BusWaking),
            BusState::Awake => {}
        }
        let leased = self.lease_until.is_some();
        match (class, leased) {
            (TxClass::Lease, false) => return Err(Refused::NoLease),
            (TxClass::Lease, true) => {}
            (_, true) => return Err(Refused::Leased),
            (_, false) => {}
        }
        match class {
            TxClass::Override | TxClass::DisplayTop => {
                if !view.rnse_awake {
                    return Err(Refused::RnseAsleep);
                }
            }
            TxClass::DisplayDdp | TxClass::Diag | TxClass::Extra | TxClass::Lease => {
                if !view.kl15_on() {
                    return Err(Refused::Kl15Off);
                }
            }
        }
        let b = &mut self.buckets[class.idx()];
        if b.take(self.cfg.rate(class), now) {
            if b.capped {
                b.capped = false;
                info!(iface = %self.name, %class, "rate cap released");
            }
            Ok(())
        } else {
            if !b.capped {
                b.capped = true;
                warn!(iface = %self.name, %class, "rate cap hit, dropping frames");
            }
            Err(Refused::RateCap)
        }
    }

    /// Takes the bus for an external tool for at most `max`. Needs Kl15 and
    /// an awake bus; a lease never holds the bus awake. Every other class is
    /// refused until it's released or expires.
    pub fn take_lease(
        &mut self,
        now: Instant,
        max: Duration,
        view: &BusView,
    ) -> Result<(), Refused> {
        self.expire_lease(now);
        if self.lease_until.is_some() {
            return Err(Refused::Leased);
        }
        match view.bus {
            BusState::Silent => return Err(Refused::BusSilent),
            BusState::SleepPending => return Err(Refused::SleepPending),
            BusState::Waking => return Err(Refused::BusWaking),
            BusState::Awake => {}
        }
        if !view.kl15_on() {
            return Err(Refused::Kl15Off);
        }
        self.lease_until = Some(now + max);
        info!(iface = %self.name, ?max, "bus leased");
        Ok(())
    }

    pub fn release_lease(&mut self) {
        if self.lease_until.take().is_some() {
            info!(iface = %self.name, "bus lease released");
        }
    }

    pub fn leased(&self, now: Instant) -> bool {
        self.lease_until.is_some_and(|t| now < t)
    }

    /// Call on every tick; the lease also expires lazily in `check`.
    pub fn tick(&mut self, now: Instant) {
        self.expire_lease(now);
    }

    /// When the lease runs out, if one is held.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.lease_until
    }

    fn expire_lease(&mut self, now: Instant) {
        if self.lease_until.is_some_and(|t| now >= t) {
            self.lease_until = None;
            warn!(iface = %self.name, "bus lease expired");
        }
    }

    pub fn stats(&self, class: TxClass) -> ClassStats {
        self.stats[class.idx()]
    }
}

/// Fixed-ID classes must use their IDs. `Diag`, `Extra` and `Lease` IDs vary.
fn check_frame(class: TxClass, f: &Frame) -> Result<(), Refused> {
    let std_id = |allowed: &[u32]| !f.extended && allowed.contains(&f.id);
    match class {
        TxClass::Override => {
            if !std_id(&[ids::RNSE_CMD]) || f.len() != 8 {
                return Err(Refused::BadFrame);
            }
            if !OVERRIDE_OPCODES.contains(&f.data()[0]) {
                return Err(Refused::ForbiddenOpcode);
            }
            Ok(())
        }
        TxClass::DisplayTop => std_id(&[ids::DIS_TOPLINE_1, ids::DIS_TOPLINE_2])
            .then_some(())
            .ok_or(Refused::BadFrame),
        TxClass::DisplayDdp => std_id(&[ids::DDP_TX, ids::RNSE_DDP_RX])
            .then_some(())
            .ok_or(Refused::BadFrame),
        TxClass::Diag | TxClass::Extra | TxClass::Lease => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn view(bus: BusState, rnse_awake: bool, kl15: Option<bool>) -> BusView {
        BusView {
            bus,
            rnse_awake,
            kl15,
        }
    }

    fn awake() -> BusView {
        view(BusState::Awake, true, Some(true))
    }

    /// A frame each class would legitimately send.
    fn legit(class: TxClass) -> Frame {
        match class {
            TxClass::Override => Frame::new(0x7B0, &[0xBB, 0x50, 0, 0, 0, 0, 0, 0]),
            TxClass::DisplayTop => Frame::new(0x363, b"HELLO   "),
            TxClass::DisplayDdp => Frame::new(0x6C0, &[0x10, 0x00]),
            TxClass::Diag => Frame::new(0x200, &[0x01, 0xC0, 0, 0x10, 0, 0x03, 0x01]),
            TxClass::Extra => Frame::new(0x67A, &[0x01]),
            TxClass::Lease => Frame::new(0x700, &[0x00]),
        }
        .unwrap()
    }

    /// xorshift64, so the property tests need no extra crates.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
            xs[(self.next() % xs.len() as u64) as usize]
        }
    }

    fn random_frame(rng: &mut Rng) -> Frame {
        let id = match rng.next() % 4 {
            0 => rng.pick(&[0x7B0, 0x363, 0x365, 0x6C0, 0x7B3, 0x200, 0x436, 0x42B]),
            _ => (rng.next() % 0x800) as u32,
        };
        let len = (rng.next() % 9) as usize;
        let mut data = [0u8; 8];
        for b in &mut data {
            *b = rng.next() as u8;
        }
        if rng.next().is_multiple_of(2) {
            data[0] = rng.pick(&[0xB7, 0xBA, 0xBB, 0xBF]);
        }
        Frame::new(id, &data[..len]).unwrap()
    }

    #[test]
    fn policy_table() {
        let t0 = Instant::now();
        let cases = [
            (
                TxClass::Override,
                view(BusState::Awake, true, Some(false)),
                Ok(()),
            ),
            (
                TxClass::Override,
                view(BusState::Awake, false, Some(true)),
                Err(Refused::RnseAsleep),
            ),
            (
                TxClass::DisplayTop,
                view(BusState::Awake, true, None),
                Ok(()),
            ),
            (
                TxClass::DisplayTop,
                view(BusState::Awake, false, Some(true)),
                Err(Refused::RnseAsleep),
            ),
            (
                TxClass::DisplayDdp,
                view(BusState::Awake, false, Some(true)),
                Ok(()),
            ),
            (
                TxClass::DisplayDdp,
                view(BusState::Awake, true, Some(false)),
                Err(Refused::Kl15Off),
            ),
            (
                TxClass::Diag,
                view(BusState::Awake, true, None),
                Err(Refused::Kl15Off),
            ),
            (
                TxClass::Diag,
                view(BusState::Awake, false, Some(true)),
                Ok(()),
            ),
            (
                TxClass::Extra,
                view(BusState::Awake, true, Some(false)),
                Err(Refused::Kl15Off),
            ),
            (TxClass::Lease, awake(), Err(Refused::NoLease)),
            (
                TxClass::Override,
                view(BusState::Waking, true, Some(true)),
                Err(Refused::BusWaking),
            ),
        ];
        for (class, v, want) in cases {
            let mut g = Gate::new("ican", Config::default());
            assert_eq!(g.check(class, &legit(class), t0, &v), want, "{class} {v:?}");
        }
    }

    #[test]
    fn frame_checks() {
        let t0 = Instant::now();
        let mut g = Gate::new("ican", Config::default());
        let v = awake();
        let f = |id, d: &[u8]| Frame::new(id, d).unwrap();
        // B7 and the rest of B0-B9 never pass, nor anything below BA.
        for op in 0x00..=0xB9u8 {
            let r = g.check(
                TxClass::Override,
                &f(0x7B0, &[op, 0, 0, 0, 0, 0, 0, 0]),
                t0,
                &v,
            );
            assert_eq!(r, Err(Refused::ForbiddenOpcode), "{op:02X}");
        }
        assert_eq!(
            g.check(TxClass::Override, &f(0x7B0, &[0xBA, 0]), t0, &v),
            Err(Refused::BadFrame)
        );
        assert_eq!(
            g.check(
                TxClass::Override,
                &f(0x363, &[0xBA, 0, 0, 0, 0, 0, 0, 0]),
                t0,
                &v
            ),
            Err(Refused::BadFrame)
        );
        assert_eq!(
            g.check(TxClass::DisplayTop, &f(0x6C0, &[0]), t0, &v),
            Err(Refused::BadFrame)
        );
        assert_eq!(
            g.check(TxClass::DisplayDdp, &f(0x7B3, &[0]), t0, &v),
            Ok(())
        );
        assert_eq!(
            g.check(TxClass::Diag, &f(0x436, &[0x0B, 0x01]), t0, &v),
            Err(Refused::NmId)
        );
        let s = g.stats(TxClass::Override);
        assert_eq!(s.refused(Refused::ForbiddenOpcode), 0xBA);
        assert_eq!(s.passed, 0);
    }

    #[test]
    fn rate_cap() {
        let t0 = Instant::now();
        let mut cfg = Config::default();
        cfg.set_rate(
            TxClass::Override,
            Rate {
                per_sec: 10.0,
                burst: 3.0,
            },
        );
        let mut g = Gate::new("ican", cfg);
        let v = awake();
        let f = legit(TxClass::Override);
        for _ in 0..3 {
            assert_eq!(g.check(TxClass::Override, &f, t0, &v), Ok(()));
        }
        assert_eq!(
            g.check(TxClass::Override, &f, t0, &v),
            Err(Refused::RateCap)
        );
        // One token back after 100 ms.
        assert_eq!(g.check(TxClass::Override, &f, t0 + ms(100), &v), Ok(()));
        assert_eq!(
            g.check(TxClass::Override, &f, t0 + ms(100), &v),
            Err(Refused::RateCap)
        );
        // Other classes have their own bucket.
        assert_eq!(
            g.check(TxClass::Diag, &legit(TxClass::Diag), t0, &v),
            Ok(())
        );
        let s = g.stats(TxClass::Override);
        assert_eq!((s.passed, s.refused(Refused::RateCap)), (4, 2));
    }

    #[test]
    fn lease_blocks_others_and_expires() {
        let t0 = Instant::now();
        let mut g = Gate::new("ican", Config::default());
        let v = awake();
        assert_eq!(
            g.take_lease(t0, ms(1000), &view(BusState::Awake, true, Some(false))),
            Err(Refused::Kl15Off)
        );
        assert_eq!(
            g.take_lease(
                t0,
                ms(1000),
                &view(BusState::SleepPending, true, Some(true))
            ),
            Err(Refused::SleepPending)
        );
        g.take_lease(t0, ms(1000), &v).unwrap();
        assert_eq!(g.take_lease(t0, ms(1000), &v), Err(Refused::Leased));
        assert_eq!(g.next_deadline(), Some(t0 + ms(1000)));
        for class in TxClass::ALL {
            let want = if class == TxClass::Lease {
                Ok(())
            } else {
                Err(Refused::Leased)
            };
            assert_eq!(g.check(class, &legit(class), t0, &v), want, "{class}");
        }
        // Expired: everything comes back, Lease frames stop.
        let t = t0 + ms(1000);
        assert!(!g.leased(t));
        assert_eq!(
            g.check(TxClass::Lease, &legit(TxClass::Lease), t, &v),
            Err(Refused::NoLease)
        );
        assert_eq!(
            g.check(TxClass::Override, &legit(TxClass::Override), t, &v),
            Ok(())
        );

        g.take_lease(t, ms(1000), &v).unwrap();
        g.release_lease();
        assert_eq!(g.check(TxClass::Diag, &legit(TxClass::Diag), t, &v), Ok(()));
    }

    #[test]
    fn kl15_from_store() {
        let t0 = Instant::now();
        let mut s = Store::new();
        assert_eq!(kl15(&s, t0), None);
        let to = Duration::from_secs(1);
        s.set("ignition.kl15", Some(false.into()), t0, to);
        assert_eq!(kl15(&s, t0), Some(false));
        s.set("ignition.zas.kl15", Some(true.into()), t0, to);
        assert_eq!(kl15(&s, t0), Some(true));
        // Stale = unknown.
        assert_eq!(kl15(&s, t0 + ms(1500)), None);
    }

    #[test]
    fn view_from_monitor() {
        let t0 = Instant::now();
        let mut m = BusMonitor::new(Default::default());
        let s = Store::new();
        assert_eq!(
            BusView::new(&m, &s, t0),
            view(BusState::Silent, false, None)
        );
        m.on_frame(t0, 0x42B, &[0x16, 0x01]);
        m.on_frame(t0, 0x436, &[0x0B, 0x01]);
        assert_eq!(BusView::new(&m, &s, t0), view(BusState::Awake, true, None));
        m.on_frame(t0 + ms(50), 0x436, &[0x0B, 0x11]);
        assert!(!BusView::new(&m, &s, t0 + ms(50)).rnse_awake);
    }

    /// Property: in SleepPending or Silent nothing passes, whatever the class,
    /// frame, flags, lease or timing.
    #[test]
    fn nothing_passes_asleep_exhaustive() {
        let t0 = Instant::now();
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for bus in [BusState::Silent, BusState::SleepPending] {
            for rnse in [false, true] {
                for kl15 in [None, Some(false), Some(true)] {
                    for lease in [false, true] {
                        let mut g = Gate::new("ican", Config::default());
                        if lease {
                            g.take_lease(t0, Duration::from_secs(3600), &awake())
                                .unwrap();
                        }
                        let v = view(bus, rnse, kl15);
                        for class in TxClass::ALL {
                            assert!(g.check(class, &legit(class), t0, &v).is_err());
                            for i in 0..200 {
                                let f = random_frame(&mut rng);
                                let t = t0 + ms(i * 37);
                                assert!(g.check(class, &f, t, &v).is_err(), "{class} {f:?} {v:?}");
                            }
                            assert_eq!(g.stats(class).passed, 0);
                        }
                    }
                }
            }
        }
    }

    /// Property: over random sequences of views, leases and frames, a frame
    /// passes only with the bus awake and its class condition met.
    #[test]
    fn random_sequences_obey_policy() {
        let t0 = Instant::now();
        let mut rng = Rng(0xDEAD_BEEF_CAFE_F00D);
        let states = [
            BusState::Silent,
            BusState::Waking,
            BusState::Awake,
            BusState::SleepPending,
        ];
        let mut g = Gate::new("ican", Config::default());
        let mut t = t0;
        let mut passed = 0;
        for _ in 0..50_000 {
            t += ms(rng.next() % 50);
            let v = view(
                rng.pick(&states),
                rng.next().is_multiple_of(2),
                rng.pick(&[None, Some(false), Some(true)]),
            );
            match rng.next() % 200 {
                0 => {
                    let _ = g.take_lease(t, ms(rng.next() % 5000), &v);
                }
                1 => g.release_lease(),
                _ => {}
            }
            let class = rng.pick(&TxClass::ALL);
            let f = if rng.next().is_multiple_of(2) {
                legit(class)
            } else {
                random_frame(&mut rng)
            };
            let leased = g.leased(t);
            if g.check(class, &f, t, &v).is_ok() {
                passed += 1;
                assert_eq!(v.bus, BusState::Awake);
                assert!(!ids::is_nm(f.id));
                assert_eq!(leased, class == TxClass::Lease);
                match class {
                    TxClass::Override => {
                        assert!(v.rnse_awake);
                        assert_eq!(f.id, 0x7B0);
                        assert!((0xBA..=0xBF).contains(&f.data()[0]));
                    }
                    TxClass::DisplayTop => assert!(v.rnse_awake),
                    _ => assert_eq!(v.kl15, Some(true)),
                }
            }
        }
        assert!(
            passed > 1000,
            "the sequence should exercise passing frames too"
        );
    }
}
