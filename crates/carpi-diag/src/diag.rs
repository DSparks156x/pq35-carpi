//! The diag engine: channels, scheduler, modes, jobs. See the crate docs.
//!
//! Refs: `[research]` rnse-carpi `docs/research/diagnostics.md`; `[trace]` as in
//! [`crate::catalog`].

use crate::catalog::{Basis, Catalog, Category, Provider, Unit, ValueDef};
use crate::job::{Job, JobError, JobEvent, JobId, JobOutput, JobRun, JobStage, Step};
use crate::modules::{self, GroupField};
use carpi_can::Frame;
use carpi_kwp::group::{self, parse_group};
use carpi_kwp::{
    Class, ExclusiveToken, KwpClient, KwpError, KwpEvent, Profiles, Request, services,
};
use carpi_state::{Store, Value};
use carpi_tp2::{Channel, CloseReason, TesterIds, wire};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, Instant};

/// VCDS's tester RX ID: modules answer VCDS here. Proven `[trace]` (the old
/// worker borrowed it). Any traffic on it means another tester is active.
pub const VCDS_TESTER_RX: u32 = 0x300;

#[derive(Clone, Debug)]
pub struct Config {
    /// Concurrent channels. For safety, not throughput: the gateway handles many
    /// (Drew, PLAN 6). Ours: 4.
    pub max_channels: usize,
    /// Tester RX IDs. 0x300 is always dropped by `TesterIds`.
    pub tester_ids: Vec<u16>,
    /// Keep a channel open this long after its last demand goes. Ours: 5 s
    /// (old code: 2 s after a job, none for slow polls).
    pub grace: Duration,
    /// Failed opens back off from `backoff_min`, doubling to `backoff_max`.
    /// Ours: 1 s..30 s (old worker: flat 5 s).
    pub backoff_min: Duration,
    pub backoff_max: Duration,
    /// Auto VCDS pause ends after this long with no foreign tester traffic. Ours.
    pub vcds_quiet: Duration,
    /// Fastest period a subscription gets. Ours: about one group read on the
    /// engine (~48 ms `[trace]`).
    pub min_period: Duration,
    /// Slowest granted period before we call a group starved.
    pub max_period: Duration,
    /// Store timeout = `timeout_factor` x granted period, at least `timeout_floor`.
    pub timeout_factor: u32,
    pub timeout_floor: Duration,
    /// Cost of one group read before we've measured one. `[trace]`: ~48 ms on the
    /// wire; old Python mean 83-91 ms. Ours: 60 ms.
    pub default_cost: Duration,
    /// Share of a channel's time the scheduler plans to fill. Leaves room for
    /// A3 and jobs. Ours.
    pub utilisation: f64,
    /// Consecutive KWP timeouts that drop a channel. Ours.
    pub max_timeouts: u32,
    /// Per-module KWP behaviour; ABS 0x03 denied by default.
    pub profiles: Profiles,
    pub link: carpi_tp2::Config,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            max_channels: 4,
            tester_ids: (0x308..=0x30F).collect(),
            grace: Duration::from_secs(5),
            backoff_min: Duration::from_secs(1),
            backoff_max: Duration::from_secs(30),
            vcds_quiet: Duration::from_secs(10),
            min_period: Duration::from_millis(50),
            max_period: Duration::from_secs(10),
            timeout_factor: 3,
            timeout_floor: Duration::from_secs(1),
            default_cost: Duration::from_millis(60),
            utilisation: 0.8,
            max_timeouts: 3,
            profiles: Profiles::default(),
            link: carpi_tp2::Config::diag(),
        }
    }
}

/// What diag is doing overall. Strongest first: a bus lease beats a VCDS pause
/// beats an exclusive job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    /// Polling stopped; jobs only under the token for `lease`.
    Exclusive {
        lease: u64,
    },
    /// No diag TX at all. `manual` if set by `set_vcds_pause`.
    VcdsPause {
        manual: bool,
    },
    /// The daemon leased the bus out. No diag TX at all.
    BusLease,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChannelError {
    Link(CloseReason),
    Kwp(KwpError),
    /// `max_timeouts` KWP timeouts in a row.
    Timeouts,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelState {
    Closed,
    /// TP2 setup, A0/A1 and `10 89`.
    Opening,
    Open,
    /// Open, no demand; closes at `until`.
    Grace {
        until: Instant,
    },
    /// Retry no sooner than `until`.
    Failed {
        until: Instant,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelStatus {
    pub module: u8,
    pub state: ChannelState,
    pub tester_rx: Option<u16>,
    /// Consecutive failures (sets the backoff).
    pub failures: u32,
    pub last_error: Option<ChannelError>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subscription {
    /// Catalog name.
    pub value: String,
    pub period: Duration,
    /// Higher wins.
    pub priority: u8,
    pub lease_expires: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SubId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Already in the store under this name; diag does nothing.
    Ican(&'static str),
    Diag {
        module: u8,
        group: u8,
        field: u8,
    },
    Unavailable,
}

/// Why a subscription isn't getting what it asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Degraded {
    Paused(Mode),
    /// Only provider is on a denied module (ABS).
    Denied {
        module: u8,
    },
    /// The module refused the group. `nrc` 11/12/31 drop the group from the
    /// plan (treated as permanent, as the old code did); others back off.
    Rejected {
        module: u8,
        group: u8,
        nrc: Option<u8>,
    },
    /// Channel failed; retrying at `until`.
    Backoff {
        module: u8,
        until: Instant,
    },
    /// Channel cap or tester IDs exhausted.
    WaitingForChannel {
        module: u8,
    },
    /// Granted period is longer than requested.
    Budget,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SubStatus {
    pub value: &'static str,
    pub source: Source,
    pub requested: Duration,
    /// What the scheduler plans to deliver. ICAN: the request, as given.
    pub granted: Option<Duration>,
    /// Successful reads per second over the last 5 s. `None` for ICAN.
    pub achieved_hz: Option<f64>,
    pub degraded: Option<Degraded>,
    pub lease_expires: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubError {
    UnknownValue,
    LeaseExpired,
    /// A module-group name for a module that's never polled (ABS).
    Denied,
    /// Too many distinct module-group names since carpid started.
    Full,
}

/// Module-group names (`diag.01.103.2`) carpid keeps at most. Each one stays
/// in the catalog for good.
pub const MAX_GROUP_FIELDS: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModeError {
    /// Someone already holds the exclusive token.
    Held,
    Paused(Mode),
    WrongToken,
}

/// One decoded value for the store. `value: None` = unknown formula or unit
/// mismatch.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub name: &'static str,
    pub value: Option<Value>,
    pub at: Instant,
    pub timeout: Duration,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Mode(Mode),
    Job(JobEvent),
    SubExpired(SubId),
}

enum Inflight {
    Session,
    Group { group: u8, started: Instant },
    Job(JobId),
}

struct ModuleCh {
    state: ChannelState,
    kwp: Option<KwpClient>,
    tester_rx: Option<u16>,
    inflight: Option<Inflight>,
    failures: u32,
    timeouts: u32,
    last_error: Option<ChannelError>,
}

impl ModuleCh {
    fn new() -> Self {
        ModuleCh {
            state: ChannelState::Closed,
            kwp: None,
            tester_rx: None,
            inflight: None,
            failures: 0,
            timeouts: 0,
            last_error: None,
        }
    }
    fn holds_slot(&self) -> bool {
        matches!(
            self.state,
            ChannelState::Opening | ChannelState::Open | ChannelState::Grace { .. }
        )
    }
    fn idle_open(&self) -> bool {
        self.state == ChannelState::Open
            && self.inflight.is_none()
            && self.kwp.as_ref().is_some_and(|k| k.is_open())
    }
}

struct Sub {
    spec: Subscription,
    def: usize,
    source: Source,
    /// Why there's no source.
    blocked: Option<Degraded>,
}

#[derive(Clone, Copy)]
struct GroupBackoff {
    until: Instant,
    step: Duration,
    nrc: Option<u8>,
}

/// One planned `21 gg` read.
struct Task {
    /// (catalog index, one-based field).
    members: Vec<(usize, u8)>,
    req_period: Duration,
    priority: u8,
    granted: Duration,
    next_due: Instant,
    created: Instant,
    done: VecDeque<Instant>,
    /// EWMA of request duration.
    cost: Duration,
    backoff: Option<GroupBackoff>,
}

/// A candidate read for one value: (module, group, one-based field, provider rank).
type Cand = (u8, u8, u8, usize);

const RATE_WINDOW: Duration = Duration::from_secs(5);
const DONE_KEEP: usize = 256;

impl Task {
    fn achieved_hz(&self, now: Instant) -> Option<f64> {
        let span = now.saturating_duration_since(self.created).min(RATE_WINDOW);
        if span < Duration::from_millis(500) {
            return None;
        }
        let n = self
            .done
            .iter()
            .filter(|&&t| now.saturating_duration_since(t) <= span)
            .count();
        Some(n as f64 / span.as_secs_f64())
    }
    fn ready_at(&self) -> Instant {
        self.backoff
            .map_or(self.next_due, |b| b.until.max(self.next_due))
    }
}

pub struct Diag {
    cfg: Config,
    catalog: Catalog,
    ids: TesterIds,
    modules: BTreeMap<u8, ModuleCh>,
    subs: BTreeMap<SubId, Sub>,
    tasks: BTreeMap<(u8, u8), Task>,
    /// Groups a module refused with NRC 11/12/31.
    rejected: BTreeMap<(u8, u8), u8>,
    waiting: BTreeSet<u8>,
    jobs: Vec<JobRun>,
    bus_lease: bool,
    vcds_manual: bool,
    vcds_until: Option<Instant>,
    exclusive: Option<u64>,
    /// TP2 ID by VCDS address: the table, then the last install list.
    addresses: BTreeMap<u8, u8>,
    /// Module-group values added to the catalog.
    group_fields: usize,
    next_id: u64,
    out: VecDeque<Frame>,
    samples: VecDeque<Sample>,
    events: VecDeque<Event>,
    last_mode: Mode,
}

/// Close a channel. `polite` sends the A8 the link emits; otherwise nothing.
fn shut(
    ch: &mut ModuleCh,
    now: Instant,
    polite: bool,
    ids: &mut TesterIds,
    out: &mut VecDeque<Frame>,
) {
    if let Some(mut k) = ch.kwp.take() {
        k.close(now);
        while let Some(f) = k.poll_transmit() {
            if polite {
                out.push_back(f);
            }
        }
    }
    if let Some(id) = ch.tester_rx.take() {
        ids.release(id);
    }
    ch.inflight = None;
    ch.state = ChannelState::Closed;
}

impl Diag {
    pub fn new(cfg: Config, catalog: Catalog) -> Self {
        let ids = TesterIds::new(cfg.tester_ids.iter().copied());
        Diag {
            cfg,
            catalog,
            ids,
            modules: BTreeMap::new(),
            subs: BTreeMap::new(),
            tasks: BTreeMap::new(),
            rejected: BTreeMap::new(),
            waiting: BTreeSet::new(),
            jobs: Vec::new(),
            bus_lease: false,
            vcds_manual: false,
            vcds_until: None,
            exclusive: None,
            addresses: modules::MODULES.iter().map(|m| (m.0, m.1)).collect(),
            group_fields: 0,
            next_id: 1,
            out: VecDeque::new(),
            samples: VecDeque::new(),
            events: VecDeque::new(),
            last_mode: Mode::Normal,
        }
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// TP2 ID of the module at VCDS `address`: from the last install list,
    /// else the table.
    pub fn tp2_of(&self, address: u8) -> Option<u8> {
        self.addresses.get(&address).copied()
    }

    /// VCDS address of TP2 module `tp2`.
    pub fn address_of(&self, tp2: u8) -> Option<u8> {
        self.addresses
            .iter()
            .find(|(_, t)| **t == tp2)
            .map(|(a, _)| *a)
    }

    /// Whether `tp2` is never polled (ABS).
    pub fn is_denied(&self, tp2: u8) -> bool {
        self.cfg.profiles.is_denied(tp2)
    }

    /// The catalog index of a module-group name, adding it the first time.
    fn group_field(&mut self, f: GroupField) -> Result<usize, SubError> {
        let name = f.name();
        if let Some(i) = self.catalog.index_of(&name) {
            return Ok(i);
        }
        let tp2 = self.tp2_of(f.address).ok_or(SubError::UnknownValue)?;
        if f.address == modules::ABS_ADDRESS || self.cfg.profiles.is_denied(tp2) {
            return Err(SubError::Denied);
        }
        if self.group_fields >= MAX_GROUP_FIELDS {
            return Err(SubError::Full);
        }
        self.group_fields += 1;
        // Bounded by MAX_GROUP_FIELDS, so leaking the names is fine.
        let def = ValueDef::new(Box::leak(name.into_boxed_str()), Unit::Reported)
            .label(Box::leak(f.label().into_boxed_str()))
            .place(Category::Other, "Module groups")
            .diag(tp2, f.group, f.field, Basis::Custom);
        Ok(self.catalog.push(def))
    }

    pub fn mode(&self) -> Mode {
        if self.bus_lease {
            Mode::BusLease
        } else if self.vcds_manual || self.vcds_until.is_some() {
            Mode::VcdsPause {
                manual: self.vcds_manual,
            }
        } else if let Some(lease) = self.exclusive {
            Mode::Exclusive { lease }
        } else {
            Mode::Normal
        }
    }

    fn silent(&self) -> bool {
        self.bus_lease || self.vcds_manual || self.vcds_until.is_some()
    }

    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    // ----- IO ---------------------------------------------------------------

    pub fn on_frame(&mut self, now: Instant, f: &Frame) {
        if f.extended {
            return;
        }
        if f.id == wire::SETUP_ID
            && let Some((module, rx)) = wire::parse_setup_request(f.data())
            && !self.cfg.tester_ids.contains(&rx)
        {
            self.foreign(now, Some(module));
        } else if f.id == VCDS_TESTER_RX {
            self.foreign(now, None);
        }
        if !self.silent() {
            for ch in self.modules.values_mut() {
                if let Some(k) = &mut ch.kwp {
                    k.on_frame(now, f);
                }
            }
        }
        self.pump(now);
    }

    pub fn tick(&mut self, now: Instant) {
        let expired: Vec<SubId> = self
            .subs
            .iter()
            .filter(|(_, s)| now >= s.spec.lease_expires)
            .map(|(id, _)| *id)
            .collect();
        if !expired.is_empty() {
            for id in expired {
                self.subs.remove(&id);
                self.events.push_back(Event::SubExpired(id));
            }
            self.replan(now);
        }
        if self.vcds_until.is_some_and(|t| now >= t) {
            self.vcds_until = None;
        }
        for ch in self.modules.values_mut() {
            if let Some(k) = &mut ch.kwp {
                k.tick(now);
            }
        }
        self.pump(now);
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        let mut d: Option<Instant> = None;
        let mut take = |t: Option<Instant>| {
            if let Some(t) = t {
                d = Some(d.map_or(t, |x| x.min(t)));
            }
        };
        for (m, ch) in &self.modules {
            take(ch.kwp.as_ref().and_then(|k| k.next_deadline()));
            match ch.state {
                ChannelState::Grace { until } | ChannelState::Failed { until } => take(Some(until)),
                _ => {}
            }
            if ch.idle_open() && !self.silent() && self.exclusive.is_none() {
                for ((tm, _), t) in &self.tasks {
                    if tm == m {
                        take(Some(t.ready_at()));
                    }
                }
            }
        }
        for s in self.subs.values() {
            take(Some(s.spec.lease_expires));
        }
        take(self.vcds_until);
        d
    }

    pub fn poll_transmit(&mut self) -> Option<Frame> {
        self.out.pop_front()
    }

    pub fn poll_sample(&mut self) -> Option<Sample> {
        self.samples.pop_front()
    }

    /// Move every pending sample into the store.
    pub fn write_samples(&mut self, store: &mut Store) {
        while let Some(s) = self.samples.pop_front() {
            store.set(s.name, s.value, s.at, s.timeout);
        }
    }

    pub fn poll_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    // ----- subscriptions ----------------------------------------------------

    /// Subscribe to a catalog value, or to any measuring-block field by its
    /// module-group name (`diag.01.103.2`, see [`GroupField`]).
    pub fn subscribe(&mut self, now: Instant, sub: Subscription) -> Result<SubId, SubError> {
        if now >= sub.lease_expires {
            return Err(SubError::LeaseExpired);
        }
        let def = match self.catalog.index_of(&sub.value) {
            Some(i) => i,
            None => {
                let f = GroupField::parse(&sub.value).ok_or(SubError::UnknownValue)?;
                self.group_field(f)?
            }
        };
        let id = SubId(self.id());
        self.subs.insert(
            id,
            Sub {
                spec: sub,
                def,
                source: Source::Unavailable,
                blocked: None,
            },
        );
        self.replan(now);
        self.pump(now);
        Ok(id)
    }

    /// Extend a lease. `false` if it already expired.
    pub fn renew(&mut self, id: SubId, lease_expires: Instant) -> bool {
        match self.subs.get_mut(&id) {
            Some(s) => {
                s.spec.lease_expires = lease_expires;
                true
            }
            None => false,
        }
    }

    pub fn unsubscribe(&mut self, now: Instant, id: SubId) -> bool {
        let had = self.subs.remove(&id).is_some();
        if had {
            self.replan(now);
            self.pump(now);
        }
        had
    }

    pub fn status(&self, now: Instant, id: SubId) -> Option<SubStatus> {
        let s = self.subs.get(&id)?;
        let name = self.catalog.at(s.def).name;
        let (granted, achieved_hz, degraded) = match s.source {
            Source::Ican(_) => (Some(s.spec.period), None, None),
            Source::Unavailable => (None, None, s.blocked),
            Source::Diag { module, group, .. } => {
                let t = &self.tasks[&(module, group)];
                let granted = s.spec.period.max(t.granted);
                let ch = self.modules.get(&module);
                let deg = if self.mode() != Mode::Normal {
                    Some(Degraded::Paused(self.mode()))
                } else if let Some(ChannelState::Failed { until }) = ch.map(|c| c.state) {
                    Some(Degraded::Backoff { module, until })
                } else if self.waiting.contains(&module) {
                    Some(Degraded::WaitingForChannel { module })
                } else if let Some(b) = t.backoff {
                    Some(Degraded::Rejected {
                        module,
                        group,
                        nrc: b.nrc,
                    })
                } else if granted > s.spec.period.max(self.cfg.min_period) {
                    Some(Degraded::Budget)
                } else {
                    None
                };
                (Some(granted), t.achieved_hz(now), deg)
            }
        };
        Some(SubStatus {
            value: name,
            source: s.source,
            requested: s.spec.period,
            granted,
            achieved_hz,
            degraded,
            lease_expires: s.spec.lease_expires,
        })
    }

    pub fn statuses(&self, now: Instant) -> Vec<(SubId, SubStatus)> {
        self.subs
            .keys()
            .filter_map(|id| self.status(now, *id).map(|s| (*id, s)))
            .collect()
    }

    pub fn channels(&self) -> Vec<ChannelStatus> {
        self.modules
            .iter()
            .map(|(m, c)| ChannelStatus {
                module: *m,
                state: c.state,
                tester_rx: c.tester_rx,
                failures: c.failures,
                last_error: c.last_error.clone(),
            })
            .collect()
    }

    /// Let groups refused with NRC 11/12/31 be planned again (e.g. after the
    /// ECU changed).
    pub fn forget_rejections(&mut self, now: Instant) {
        self.rejected.clear();
        self.replan(now);
        self.pump(now);
    }

    // ----- modes ------------------------------------------------------------

    /// Manual VCDS pause. Turning it off also ends an automatic pause (user
    /// override).
    pub fn set_vcds_pause(&mut self, now: Instant, on: bool) {
        if on {
            let was = self.silent();
            self.vcds_manual = true;
            if !was {
                self.go_quiet(now, true, None);
            }
        } else {
            self.vcds_manual = false;
            self.vcds_until = None;
        }
        self.pump(now);
    }

    /// The daemon's bus lease (PLAN 2.10). On: close every channel with A8, fail
    /// jobs, then send nothing. Diag is drained once `poll_transmit` is empty.
    pub fn set_bus_lease(&mut self, now: Instant, on: bool) {
        let was = self.silent();
        self.bus_lease = on;
        if on && !was {
            self.go_quiet(now, true, None);
        }
        self.pump(now);
    }

    /// Take exclusive control: polling stops, and only jobs under this token
    /// run. `max` caps what the token allows.
    pub fn begin_exclusive(
        &mut self,
        now: Instant,
        max: Class,
    ) -> Result<ExclusiveToken, ModeError> {
        if self.silent() {
            return Err(ModeError::Paused(self.mode()));
        }
        if self.exclusive.is_some() {
            return Err(ModeError::Held);
        }
        let lease = self.id();
        self.exclusive = Some(lease);
        self.pump(now);
        Ok(ExclusiveToken::mint(lease, max))
    }

    /// Give the token back. Jobs still queued under it are cancelled.
    pub fn end_exclusive(&mut self, now: Instant, token: ExclusiveToken) -> Result<(), ModeError> {
        if self.exclusive != Some(token.lease()) {
            return Err(ModeError::WrongToken);
        }
        self.exclusive = None;
        let lease = token.lease();
        self.fail_jobs(|j| j.lease == Some(lease), JobError::Cancelled);
        self.pump(now);
        Ok(())
    }

    /// Foreign tester seen. `module`: the one it's setting up, if known.
    fn foreign(&mut self, now: Instant, module: Option<u8>) {
        let was = self.silent();
        self.vcds_until = Some(now + self.cfg.vcds_quiet);
        if !was {
            // Setup seen: A8 everything except the module the other tester wants
            // (A8 goes to the module's own RX ID, which would hit its channel too).
            // Only 0x300 traffic: we don't know which module, so say nothing.
            // Ours, unproven.
            self.go_quiet(now, module.is_some(), module);
        }
    }

    fn go_quiet(&mut self, now: Instant, polite: bool, spare: Option<u8>) {
        self.out.clear();
        for (m, ch) in &mut self.modules {
            let p = polite && Some(*m) != spare;
            shut(ch, now, p, &mut self.ids, &mut self.out);
        }
        self.waiting.clear();
        let mode = self.mode();
        self.fail_jobs(|_| true, JobError::Paused(mode));
    }

    // ----- jobs -------------------------------------------------------------

    /// Queue a job. Persistent jobs (DTC clear) and jobs on denied modules need
    /// the current exclusive token; while someone holds it, nothing else runs.
    pub fn start_job(
        &mut self,
        now: Instant,
        job: Job,
        token: Option<&ExclusiveToken>,
    ) -> Result<JobId, JobError> {
        if self.silent() {
            return Err(JobError::Paused(self.mode()));
        }
        if matches!(job, Job::ClearDtcs { .. }) && token.is_none() {
            return Err(JobError::NeedsToken);
        }
        match (token, self.exclusive) {
            (Some(t), Some(l)) if t.lease() == l => {}
            (Some(_), _) => return Err(JobError::StaleToken),
            (None, Some(_)) => return Err(JobError::Exclusive),
            (None, None) => {}
        }
        if self.cfg.profiles.is_denied(job.module())
            && (token.is_none() || matches!(job, Job::ReadGroup { .. }))
        {
            // Measuring blocks never go to a denied module, token or not.
            return Err(JobError::Denied);
        }

        let clear = match job {
            Job::ClearDtcs { .. } => Some(
                services::clear_dtcs(token.expect("checked")).map_err(|_| JobError::StaleToken)?,
            ),
            _ => None,
        };
        let id = JobId(self.id());
        self.jobs
            .push(JobRun::new(id, job, token.map(|t| t.lease()), clear));

        self.events.push_back(Event::Job(JobEvent::Progress {
            id,
            stage: JobStage::Queued,
        }));
        self.pump(now);
        Ok(id)
    }

    /// Drop a job. A request already on the wire finishes; its reply is ignored.
    pub fn cancel_job(&mut self, now: Instant, id: JobId) -> bool {
        let had = self.jobs.iter().any(|j| j.id == id);
        self.fail_jobs(|j| j.id == id, JobError::Cancelled);
        self.pump(now);
        had
    }

    fn fail_jobs(&mut self, pred: impl Fn(&JobRun) -> bool, err: JobError) {
        let mut i = 0;
        while i < self.jobs.len() {
            if pred(&self.jobs[i]) {
                let j = self.jobs.remove(i);
                self.events.push_back(Event::Job(JobEvent::Done {
                    id: j.id,
                    result: Err(err.clone()),
                }));
            } else {
                i += 1;
            }
        }
    }

    // ----- planning ---------------------------------------------------------

    /// Resolve every subscription and pack diag values into group reads.
    fn replan(&mut self, now: Instant) {
        let mut old = std::mem::take(&mut self.tasks);
        // (sub, candidate reads)
        let mut need: Vec<(SubId, Vec<Cand>)> = Vec::new();
        for (id, s) in &mut self.subs {
            let def = self.catalog.at(s.def);
            s.blocked = None;
            if let Some(n) = def.ican_name() {
                s.source = Source::Ican(n);
                continue;
            }
            let mut cands = Vec::new();
            let mut blocked = None;
            for (rank, p) in def.providers.iter().enumerate() {
                let Provider::Diag {
                    module,
                    group,
                    field,
                    ..
                } = *p
                else {
                    continue;
                };
                if self.cfg.profiles.is_denied(module) {
                    blocked.get_or_insert(Degraded::Denied { module });
                } else if let Some(&nrc) = self.rejected.get(&(module, group)) {
                    blocked.get_or_insert(Degraded::Rejected {
                        module,
                        group,
                        nrc: Some(nrc),
                    });
                } else {
                    cands.push((module, group, field, rank));
                }
            }
            if cands.is_empty() {
                s.source = Source::Unavailable;
                s.blocked = blocked;
            } else {
                need.push((*id, cands));
            }
        }

        // Greedy set cover: the read that serves most values; ties to a read
        // already planned, then the catalog's preferred provider, then lowest ID.
        let mut assign: BTreeMap<(u8, u8), Vec<(SubId, u8)>> = BTreeMap::new();
        while !need.is_empty() {
            let mut score: BTreeMap<(u8, u8), (usize, usize)> = BTreeMap::new();
            for (_, c) in &need {
                for &(m, g, _, r) in c {
                    let e = score.entry((m, g)).or_insert((0, usize::MAX));
                    e.0 += 1;
                    e.1 = e.1.min(r);
                }
            }
            let best = *score
                .iter()
                .max_by(|a, b| {
                    a.1.0
                        .cmp(&b.1.0)
                        .then(old.contains_key(a.0).cmp(&old.contains_key(b.0)))
                        .then(b.1.1.cmp(&a.1.1))
                        .then(b.0.cmp(a.0))
                })
                .expect("need is non-empty")
                .0;
            need.retain(|(id, c)| match c.iter().find(|x| (x.0, x.1) == best) {
                Some(&(_, _, f, _)) => {
                    assign.entry(best).or_default().push((*id, f));
                    false
                }
                None => true,
            });
        }

        for (key, members) in assign {
            let mut t = old.remove(&key).unwrap_or_else(|| Task {
                members: Vec::new(),
                req_period: Duration::MAX,
                priority: 0,
                granted: Duration::MAX,
                next_due: now,
                created: now,
                done: VecDeque::new(),
                cost: self.cfg.default_cost,
                backoff: None,
            });
            t.members.clear();
            t.req_period = Duration::MAX;
            t.priority = 0;
            for (id, field) in members {
                let s = self.subs.get_mut(&id).expect("planned sub exists");
                s.source = Source::Diag {
                    module: key.0,
                    group: key.1,
                    field,
                };
                t.req_period = t.req_period.min(s.spec.period.max(self.cfg.min_period));
                t.priority = t.priority.max(s.spec.priority);
                if !t.members.contains(&(s.def, field)) {
                    t.members.push((s.def, field));
                }
            }
            self.tasks.insert(key, t);
        }
        self.grant();
        for t in self.tasks.values_mut() {
            t.next_due = t.next_due.min(now + t.granted);
        }
    }

    /// Granted periods per channel: highest priority class first gets what it
    /// asks for; the next class shares what's left; the rest get `max_period`.
    fn grant(&mut self) {
        let mut by_mod: BTreeMap<u8, Vec<(u8, u8)>> = BTreeMap::new();
        for k in self.tasks.keys() {
            by_mod.entry(k.0).or_default().push(*k);
        }
        for keys in by_mod.into_values() {
            let mut prios: Vec<u8> = keys.iter().map(|k| self.tasks[k].priority).collect();
            prios.sort_unstable_by(|a, b| b.cmp(a));
            prios.dedup();
            let mut left = self.cfg.utilisation;
            for p in prios {
                let class: Vec<(u8, u8)> = keys
                    .iter()
                    .copied()
                    .filter(|k| self.tasks[k].priority == p)
                    .collect();
                let demand: f64 = class
                    .iter()
                    .map(|k| {
                        let t = &self.tasks[k];
                        t.cost.as_secs_f64() / t.req_period.as_secs_f64()
                    })
                    .sum();
                let factor = if demand <= left {
                    left -= demand;
                    Some(1.0)
                } else if left > 0.01 {
                    let f = demand / left;
                    left = 0.0;
                    Some(f)
                } else {
                    None
                };
                for k in class {
                    let t = self.tasks.get_mut(&k).expect("key from map");
                    t.granted = match factor {
                        Some(f) => t.req_period.mul_f64(f).min(self.cfg.max_period),
                        None => self.cfg.max_period,
                    };
                }
            }
        }
    }

    // ----- the loop ---------------------------------------------------------

    fn pump(&mut self, now: Instant) {
        for _ in 0..16 {
            let a = self.drain(now);
            self.drive(now);
            let b = self.drain(now);
            if a + b == 0 {
                break;
            }
        }
        let m = self.mode();
        if m != self.last_mode {
            self.last_mode = m;
            self.events.push_back(Event::Mode(m));
        }
    }

    /// Move frames out of every client and handle their events.
    fn drain(&mut self, now: Instant) -> usize {
        let mut n = 0;
        let mods: Vec<u8> = self.modules.keys().copied().collect();
        for m in mods {
            let evs: Vec<KwpEvent> = {
                let Diag { modules, out, .. } = self;
                let Some(k) = modules.get_mut(&m).and_then(|c| c.kwp.as_mut()) else {
                    continue;
                };
                while let Some(f) = k.poll_transmit() {
                    out.push_back(f);
                }
                std::iter::from_fn(|| k.poll_event()).collect()
            };
            for ev in evs {
                if self.modules[&m].kwp.is_none() {
                    break;
                }
                n += 1;
                self.on_kwp(now, m, ev);
            }
            // Requests issued while handling events.
            let Diag { modules, out, .. } = self;
            if let Some(k) = modules.get_mut(&m).and_then(|c| c.kwp.as_mut()) {
                while let Some(f) = k.poll_transmit() {
                    out.push_back(f);
                }
            }
        }
        n
    }

    fn on_kwp(&mut self, now: Instant, m: u8, ev: KwpEvent) {
        match ev {
            KwpEvent::Opened { .. } => {
                let req = self.wrap(m, services::start_session());
                let ch = self.modules.get_mut(&m).expect("exists");
                ch.inflight = Some(Inflight::Session);
                let r = ch.kwp.as_mut().expect("open").request(now, req);
                if let Err(e) = r {
                    self.fail_channel(now, m, ChannelError::Kwp(e));
                }
            }
            KwpEvent::Closed { reason, .. } => {
                self.fail_channel(now, m, ChannelError::Link(reason))
            }
            KwpEvent::Done { result, .. } => {
                let ch = self.modules.get_mut(&m).expect("exists");
                let inf = ch.inflight.take();
                if let Err(KwpError::LinkClosed(_)) = result {
                    // `Closed` follows and fails the channel and its jobs.
                    return;
                }
                match inf {
                    Some(Inflight::Session) => match result {
                        Ok(_) => {
                            ch.state = ChannelState::Open;
                            ch.failures = 0;
                            ch.timeouts = 0;
                            ch.last_error = None;
                        }
                        Err(e) => self.fail_channel(now, m, ChannelError::Kwp(e)),
                    },
                    Some(Inflight::Group { group, started }) => {
                        self.on_group(now, m, group, started, result)
                    }
                    Some(Inflight::Job(id)) => self.on_job(now, m, id, result),
                    None => {}
                }
            }
            KwpEvent::Pending(_) | KwpEvent::Unsolicited(_) | KwpEvent::Link(_) => {}
        }
    }

    /// Requests to a denied module go under the current exclusive lease.
    fn wrap(&self, m: u8, req: Request) -> Request {
        match self.exclusive {
            Some(l) if req.lease().is_none() && self.cfg.profiles.is_denied(m) => {
                Request::exclusive(
                    req.bytes().to_vec(),
                    &ExclusiveToken::mint(l, Class::ReadOnly),
                )
                .unwrap_or(req)
            }
            _ => req,
        }
    }

    fn fail_channel(&mut self, now: Instant, m: u8, err: ChannelError) {
        let (min, max) = (self.cfg.backoff_min, self.cfg.backoff_max);
        let Diag {
            modules, ids, out, ..
        } = self;
        let ch = modules.get_mut(&m).expect("exists");
        shut(ch, now, true, ids, out);
        ch.failures += 1;
        let back = min
            .saturating_mul(1u32 << (ch.failures - 1).min(16))
            .min(max);
        ch.state = ChannelState::Failed { until: now + back };
        ch.last_error = Some(err.clone());
        self.fail_jobs(|j| j.job.module() == m, JobError::Channel(err));
    }

    fn on_group(
        &mut self,
        now: Instant,
        m: u8,
        group: u8,
        started: Instant,
        result: Result<Vec<u8>, KwpError>,
    ) {
        let parsed = match result {
            Ok(resp) => match parse_group(&resp, group) {
                Ok(g) => Some(g),
                Err(_) => {
                    self.group_error(now, m, group, None);
                    None
                }
            },
            // Not supported / out of range: drop the group, plan around it.
            // Old code treated these as non-transient too `[research]`.
            Err(KwpError::Negative {
                nrc: nrc @ (0x11 | 0x12 | 0x31),
            }) => {
                self.rejected.insert((m, group), nrc);
                self.replan(now);
                None
            }
            Err(KwpError::Negative { nrc }) => {
                self.group_error(now, m, group, Some(nrc));
                None
            }
            Err(KwpError::Timeout | KwpError::PendingTimeout) => {
                let ch = self.modules.get_mut(&m).expect("exists");
                ch.timeouts += 1;
                if ch.timeouts >= self.cfg.max_timeouts {
                    self.fail_channel(now, m, ChannelError::Timeouts);
                }
                None
            }
            Err(_) => {
                self.group_error(now, m, group, None);
                None
            }
        };
        let Some(g) = parsed else { return };
        if let Some(ch) = self.modules.get_mut(&m) {
            ch.timeouts = 0;
        }
        let Some(t) = self.tasks.get_mut(&(m, group)) else {
            return;
        };
        let took = now.saturating_duration_since(started);
        t.cost = t.cost.mul_f64(0.8) + took.mul_f64(0.2);
        t.done.push_back(now);
        if t.done.len() > DONE_KEEP {
            t.done.pop_front();
        }
        t.backoff = None;
        let timeout = t
            .granted
            .saturating_mul(self.cfg.timeout_factor)
            .max(self.cfg.timeout_floor);
        for &(def, field) in &t.members {
            let d = self.catalog.at(def);
            let value = g
                .records
                .get(usize::from(field) - 1)
                .and_then(|r: &group::Record| d.decode(r));
            self.samples.push_back(Sample {
                name: d.name,
                value,
                at: now,
                timeout,
            });
        }
        self.grant();
    }

    fn group_error(&mut self, now: Instant, m: u8, group: u8, nrc: Option<u8>) {
        if let Some(t) = self.tasks.get_mut(&(m, group)) {
            let step = t.backoff.map_or(Duration::from_secs(2), |b| {
                (b.step * 2).min(Duration::from_secs(60))
            });
            t.backoff = Some(GroupBackoff {
                until: now + step,
                step,
                nrc,
            });
        }
    }

    fn on_job(&mut self, _now: Instant, _m: u8, id: JobId, result: Result<Vec<u8>, KwpError>) {
        let Some(i) = self.jobs.iter().position(|j| j.id == id) else {
            return; // cancelled
        };
        match self.jobs[i].on_result(result) {
            Step::Next => self.jobs[i].active = false,
            Step::Done(r) => {
                if let Ok(JobOutput::InstallList(l)) = &r {
                    for m in l {
                        self.addresses.insert(m.vcds, m.tp2);
                    }
                }
                self.jobs.remove(i);
                self.events
                    .push_back(Event::Job(JobEvent::Done { id, result: r }));
            }
        }
    }

    /// Channel transitions, opens, and new requests.
    fn drive(&mut self, now: Instant) {
        for ch in self.modules.values_mut() {
            if let ChannelState::Failed { until } = ch.state
                && now >= until
            {
                ch.state = ChannelState::Closed;
            }
        }
        if self.silent() {
            self.waiting.clear();
            return;
        }
        let polling = self.exclusive.is_none();

        // Demand per module: highest subscription priority; jobs beat polling.
        let mut demand: BTreeMap<u8, u16> = BTreeMap::new();
        if polling {
            for ((m, _), t) in &self.tasks {
                let e = demand.entry(*m).or_insert(0);
                *e = (*e).max(u16::from(t.priority));
            }
        }
        for j in &self.jobs {
            demand.insert(j.job.module(), 256);
        }
        for m in demand.keys() {
            self.modules.entry(*m).or_insert_with(ModuleCh::new);
        }

        let Diag {
            modules,
            ids,
            out,
            cfg,
            ..
        } = self;
        for (m, ch) in modules.iter_mut() {
            let d = demand.contains_key(m);
            match ch.state {
                ChannelState::Open if !d && ch.inflight.is_none() => {
                    ch.state = ChannelState::Grace {
                        until: now + cfg.grace,
                    }
                }
                ChannelState::Grace { .. } if d => ch.state = ChannelState::Open,
                ChannelState::Grace { until } if now >= until => shut(ch, now, true, ids, out),
                _ => {}
            }
        }

        // Open what's needed, highest demand first, within the cap.
        let mut active = modules.values().filter(|c| c.holds_slot()).count();
        let mut needy: Vec<(u16, u8)> = demand
            .iter()
            .filter(|(m, _)| modules[m].state == ChannelState::Closed)
            .map(|(m, p)| (*p, *m))
            .collect();
        needy.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        self.waiting.clear();
        for (_, m) in needy {
            if active >= self.cfg.max_channels {
                let victim = self
                    .modules
                    .iter()
                    .filter_map(|(k, c)| match c.state {
                        ChannelState::Grace { until } => Some((until, *k)),
                        _ => None,
                    })
                    .min();
                match victim {
                    Some((_, v)) => {
                        let ch = self.modules.get_mut(&v).expect("exists");
                        shut(ch, now, true, &mut self.ids, &mut self.out);
                        active -= 1;
                    }
                    None => {
                        self.waiting.insert(m);
                        continue;
                    }
                }
            }
            let Some(rx) = self.ids.alloc() else {
                self.waiting.insert(m);
                continue;
            };
            let mut k = KwpClient::new(Channel::dynamic(m, rx, self.cfg.link), &self.cfg.profiles);
            let opened = if self.cfg.profiles.is_denied(m) {
                // Only jobs under the exclusive token create demand here.
                match self.exclusive {
                    Some(l) => {
                        k.open_exclusive(now, &ExclusiveToken::mint(l, Class::ReadOnly));
                        true
                    }
                    None => false,
                }
            } else {
                k.open(now).is_ok()
            };
            if !opened {
                self.ids.release(rx);
                continue;
            }
            let ch = self.modules.get_mut(&m).expect("exists");
            ch.kwp = Some(k);
            ch.tester_rx = Some(rx);
            ch.state = ChannelState::Opening;
            active += 1;
        }

        // Requests on idle open channels: jobs first, then the next group.
        let idle: Vec<u8> = self
            .modules
            .iter()
            .filter(|(_, c)| c.idle_open())
            .map(|(m, _)| *m)
            .collect();
        for m in idle {
            if let Some(i) = self.jobs.iter().position(|j| j.job.module() == m) {
                let req = self.wrap(m, self.jobs[i].request());
                let mut bytes = [0u8; 2];
                for (b, r) in bytes.iter_mut().zip(req.bytes()) {
                    *b = *r;
                }
                let ch = self.modules.get_mut(&m).expect("exists");
                match ch.kwp.as_mut().expect("open").request(now, req) {
                    Ok(_) => {
                        let j = &mut self.jobs[i];
                        j.active = true;
                        ch.inflight = Some(Inflight::Job(j.id));
                        self.events.push_back(Event::Job(JobEvent::Progress {
                            id: j.id,
                            stage: JobStage::Request { n: j.step, bytes },
                        }));
                    }
                    Err(e) => {
                        let id = self.jobs[i].id;
                        self.fail_jobs(|j| j.id == id, JobError::Kwp(e));
                    }
                }
                continue;
            }
            if !polling {
                continue;
            }
            // Highest priority among due groups, then earliest due (EDF).
            let pick = self
                .tasks
                .iter()
                .filter(|((tm, _), t)| *tm == m && t.ready_at() <= now)
                .max_by(|a, b| {
                    a.1.priority
                        .cmp(&b.1.priority)
                        .then(b.1.next_due.cmp(&a.1.next_due))
                })
                .map(|(k, _)| *k);
            let Some(key) = pick else { continue };
            let ch = self.modules.get_mut(&m).expect("exists");
            match ch
                .kwp
                .as_mut()
                .expect("open")
                .request(now, group::read_group(key.1))
            {
                Ok(_) => {
                    ch.inflight = Some(Inflight::Group {
                        group: key.1,
                        started: now,
                    });
                    let t = self.tasks.get_mut(&key).expect("picked");
                    // No catch-up: the next read is a period after this one starts.
                    t.next_due = now + t.granted;
                }
                Err(_) => self.group_error(now, m, key.1, None),
            }
        }
    }
}
