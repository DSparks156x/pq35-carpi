//! Diagnostics on the `diag` bus role: one [`carpi_diag::Diag`] fed every
//! frame from that interface, its frames out through that interface's gate
//! as `TxClass::Diag`, its samples into the store.
//!
//! - API subscriptions map onto diag subscriptions: every catalog value
//!   under one of the subscription's prefixes, with a lease of 3x the
//!   period that the subscription task renews. An empty prefix maps to
//!   nothing, so "everything" never starts polling. Module-group names
//!   (`diag.01.103.2`, or `diag.01.103` for all four fields) subscribe to
//!   that measuring-block field; they never match as a plain prefix.
//! - Jobs: install list, DTC read, DTC clear, identity, one measuring
//!   block, and the scan (install list, then faults and identity of every
//!   coded module in turn). A clear takes the exclusive token, so only an
//!   explicit `confirm` starts one; the token goes back when the job ends.
//!   Fault and identity reads of ABS take a read-only token the same way:
//!   ABS is never polled, but one-off reads are fine.
//! - Coding and adaptation writes are refused while [`WRITES_ENABLED`] is
//!   off; adaptation reads aren't supported yet (the VW KWP2000 layout isn't
//!   established).
//! - `diag.mode`, `diag.vcds_auto_pause`, `diag.vcds_manual_pause`,
//!   `diag.channels_open` in the store, every tick.

use carpi_diag::modules::{self, GroupField};
use carpi_diag::{
    Catalog, ChannelState, Degraded, Diag, Event, Job, JobError, JobEvent, JobId, JobOutput,
    JobStage, Mode, Source, SubId, Subscription,
};
use carpi_gate::{BusView, Gate, TxClass};
use carpi_kwp::group::{Group, Value as GroupValue};
use carpi_kwp::services::Installed;
pub use carpi_kwp::services::{Dtc, Ident};
use carpi_kwp::{Class, ExclusiveToken};
use carpi_proto as proto;
use carpi_state::{Store, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, Instant};

use crate::control::Output;

/// Lease = this x the subscription period.
pub const LEASE_PERIODS: u32 = 3;
/// Priority of API subscriptions. All equal for now.
const PRIORITY: u8 = 1;
const DERIVED: Duration = Duration::from_secs(1);

/// Coding and adaptation writes. Off: carpid refuses them and no bytes go to
/// the car. Turning it on needs the write services built too.
pub const WRITES_ENABLED: bool = false;

/// One module of a finished scan, for the report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanModule {
    /// VCDS address.
    pub address: u8,
    pub tp2: u8,
    pub name: &'static str,
    /// Install-list status byte.
    pub status: u8,
    /// `None` when the fault read failed.
    pub dtcs: Option<Vec<Dtc>>,
    pub ident: Option<Ident>,
    /// Why the fault read failed, or the module was skipped.
    pub error: Option<String>,
}

/// A finished scan: every coded module of the install list, in its order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScanReport {
    pub modules: Vec<ScanModule>,
    /// Why it stopped early, if it did.
    pub error: Option<String>,
}

impl ScanReport {
    /// Modules that answered the fault read.
    pub fn answered(&self) -> usize {
        self.modules.iter().filter(|m| m.dtcs.is_some()).count()
    }
    /// Fault codes over every module.
    pub fn faults(&self) -> usize {
        self.modules
            .iter()
            .filter_map(|m| m.dtcs.as_ref())
            .map(Vec::len)
            .sum()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScanStep {
    Dtcs,
    Ident,
}

/// The scan going on: the install list's job ID names it.
struct Scan {
    id: JobId,
    queue: VecDeque<(usize, ScanStep)>,
    /// (index into `report.modules`, step, sub-job).
    current: Option<(usize, ScanStep, JobId)>,
    report: ScanReport,
}

pub(crate) struct Diagnostics {
    pub iface: usize,
    diag: Diag,
    /// Exclusive tokens held for DTC clears and ABS reads, by job.
    tokens: BTreeMap<JobId, ExclusiveToken>,
    /// Module of each running job.
    job_modules: BTreeMap<JobId, u8>,
    jobs: Vec<(JobId, Vec<proto::JobEvent>)>,
    scan: Option<Scan>,
    /// Finished scans, until carpid takes them.
    reports: BTreeMap<JobId, ScanReport>,
    haldex: proto::HaldexSettings,
    haldex_tx_ready: bool,
    haldex_job: Option<(JobId, proto::HaldexMode)>,
    haldex_next: Option<Instant>,
    haldex_status: String,
    haldex_applied: Option<proto::HaldexMode>,
    haldex_confirmed: Vec<(JobId, proto::HaldexMode)>,
}

/// The catalog for pickers (`Request::Catalog`). Static: it's the same
/// table whether or not there's a diag bus.
pub fn catalog_response(catalog: &Catalog) -> proto::Response {
    use carpi_diag::{Category, Provider};
    let values = catalog
        .iter()
        .map(|d| {
            let source = if d.ican_name().is_some() {
                proto::ValueSource::Ican
            } else {
                let mut modules: Vec<u8> = d
                    .providers
                    .iter()
                    .filter_map(|p| match *p {
                        Provider::Diag { module, .. } => Some(module),
                        Provider::Ican(_) => None,
                    })
                    .collect();
                modules.dedup();
                proto::ValueSource::Diag { modules }
            };
            proto::CatalogEntry {
                name: d.name.into(),
                label: d.label.into(),
                unit: d.unit_label().into(),
                category: d.category.label().into(),
                sub: d.sub.into(),
                source,
            }
        })
        .collect();
    proto::Response::Catalog {
        categories: Category::ALL.iter().map(|c| c.label().into()).collect(),
        values,
    }
}

pub fn mode_name(m: Mode) -> &'static str {
    match m {
        Mode::Normal => "normal",
        Mode::Exclusive { .. } => "exclusive",
        Mode::VcdsPause { .. } => "vcds_pause",
        Mode::BusLease => "bus_lease",
    }
}

/// Catalog names under any of `prefixes`. Empty prefixes match nothing, and
/// module-group names only match through [`subscription_names`].
pub fn matching(catalog: &Catalog, prefixes: &[String]) -> Vec<&'static str> {
    catalog
        .iter()
        .map(|d| d.name)
        .filter(|n| GroupField::parse(n).is_none())
        .filter(|n| prefixes.iter().any(|p| !p.is_empty() && n.starts_with(p)))
        .collect()
}

/// What a subscription to `prefixes` polls: catalog values under them, and
/// the module-group fields they name.
pub fn subscription_names(catalog: &Catalog, prefixes: &[String]) -> Vec<String> {
    let mut names: BTreeSet<String> = matching(catalog, prefixes)
        .into_iter()
        .map(String::from)
        .collect();
    for p in prefixes {
        if let Some(fields) = GroupField::for_prefix(p) {
            names.extend(fields.iter().map(GroupField::name));
        }
    }
    names.into_iter().collect()
}

/// A diag job error as a short message.
fn job_error(e: &JobError) -> String {
    match e {
        JobError::Paused(m) => format!("Diag paused ({})", mode_name(*m)),
        JobError::Denied => "Module denied".into(),
        JobError::Exclusive => "Diag busy".into(),
        JobError::Channel(_) => "No answer".into(),
        JobError::Kwp(carpi_kwp::KwpError::Negative { nrc }) => format!("Refused (NRC {nrc:02X})"),
        JobError::Kwp(carpi_kwp::KwpError::Timeout | carpi_kwp::KwpError::PendingTimeout) => {
            "Timeout".into()
        }
        JobError::Cancelled => "Cancelled".into(),
        JobError::HaldexUnsupported => "Requires RNSE-HUDIY Haldex firmware 7716+".into(),
        JobError::HaldexUnconfirmed => "Haldex did not confirm the requested mode".into(),
        e => format!("{e:?}"),
    }
}

impl Diagnostics {
    pub fn new(iface: usize) -> Self {
        Diagnostics {
            iface,
            diag: Diag::new(Default::default(), Catalog::seed()),
            tokens: BTreeMap::new(),
            job_modules: BTreeMap::new(),
            jobs: Vec::new(),
            scan: None,
            reports: BTreeMap::new(),
            haldex: Default::default(),
            haldex_tx_ready: false,
            haldex_job: None,
            haldex_next: None,
            haldex_status: "Off".into(),
            haldex_applied: None,
            haldex_confirmed: Vec::new(),
        }
    }

    pub fn diag(&self) -> &Diag {
        &self.diag
    }

    pub fn on_frame(&mut self, now: Instant, f: &carpi_can::Frame) {
        self.diag.on_frame(now, f);
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.diag
            .next_deadline()
            .into_iter()
            .chain(self.haldex_next)
            .min()
    }

    pub fn configure_haldex(&mut self, cfg: proto::HaldexSettings, tx_ready: bool, now: Instant) {
        if self.haldex == cfg && self.haldex_tx_ready == tx_ready {
            return;
        }
        self.haldex = cfg;
        self.haldex_tx_ready = tx_ready;
        if !cfg.enabled || !tx_ready {
            if let Some((id, _)) = self.haldex_job.take() {
                self.diag.cancel_job(now, id);
            }
            self.haldex_next = None;
            self.haldex_applied = None;
            self.haldex_status = if cfg.enabled {
                "CAN transmission inhibited"
            } else {
                "Off"
            }
            .into();
        } else {
            self.haldex_next = cfg.last_mode.map(|_| now);
            self.haldex_status = if cfg.last_mode.is_some() {
                "Waiting to restore"
            } else {
                "Choose a mode"
            }
            .into();
        }
    }

    pub fn haldex_start(&mut self, now: Instant, mode: proto::HaldexMode) -> Result<JobId, String> {
        if !self.haldex.enabled {
            return Err("Haldex mode switching is disabled in Car Integration".into());
        }
        if !self.haldex_tx_ready {
            return Err("CAN transmission is inhibited".into());
        }
        if self.haldex_job.is_some() {
            return Err("Haldex mode switch busy".into());
        }
        let id = self.start(now, Job::HaldexMode { mode: mode.code() }, true)?;
        self.haldex_job = Some((id, mode));
        self.haldex_next = None;
        self.haldex_status = format!("Switching to {}", mode.name());
        Ok(id)
    }

    fn restore_haldex(&mut self, now: Instant, ignition: bool) {
        if !self.haldex_next.is_some_and(|t| now >= t) {
            return;
        }
        self.haldex_next = None;
        if !self.haldex.enabled || !self.haldex_tx_ready {
            return;
        }
        // Wait for the car and let user jobs, VCDS and bus leases take precedence.
        if !ignition || !self.job_modules.is_empty() || self.diag.mode() != Mode::Normal {
            self.haldex_next = Some(now + Duration::from_secs(2));
            return;
        }
        if let Some(mode) = self.haldex.last_mode {
            if let Err(e) = self.haldex_start(now, mode) {
                self.haldex_status = e;
                self.haldex_next = Some(now + Duration::from_secs(10));
            }
        }
    }

    pub fn subscribe(&mut self, now: Instant, prefixes: &[String], period: Duration) -> Vec<SubId> {
        let names = subscription_names(self.diag.catalog(), prefixes);
        let lease = now + period * LEASE_PERIODS;
        names
            .into_iter()
            .filter_map(|value| {
                self.diag
                    .subscribe(
                        now,
                        Subscription {
                            value,
                            period,
                            priority: PRIORITY,
                            lease_expires: lease,
                        },
                    )
                    .ok()
            })
            .collect()
    }

    pub fn renew(&mut self, ids: &[SubId], lease_expires: Instant) {
        for id in ids {
            self.diag.renew(*id, lease_expires);
        }
    }

    pub fn unsubscribe(&mut self, now: Instant, ids: &[SubId]) {
        for id in ids {
            self.diag.unsubscribe(now, *id);
        }
    }

    pub fn start(&mut self, now: Instant, job: Job, confirm: bool) -> Result<JobId, String> {
        let id = self.start_inner(now, job, confirm)?;
        self.job_modules.insert(id, job.module());
        Ok(id)
    }

    fn start_inner(&mut self, now: Instant, job: Job, confirm: bool) -> Result<JobId, String> {
        let denied = self.diag.is_denied(job.module());
        let class = match job {
            Job::ClearDtcs { .. } if !confirm => {
                return Err("clearing fault codes needs confirm: true".into());
            }
            Job::ClearDtcs { .. } | Job::HaldexMode { .. } => Class::Persistent,
            Job::ReadGroup { .. } if denied => return Err("ABS not polled".into()),
            _ if denied => Class::ReadOnly,
            _ => {
                return self
                    .diag
                    .start_job(now, job, None)
                    .map_err(|e| job_error(&e));
            }
        };
        let token = self.diag.begin_exclusive(now, class).map_err(|e| match e {
            carpi_diag::ModeError::Paused(m) => job_error(&JobError::Paused(m)),
            _ => "Diag busy".into(),
        })?;
        match self.diag.start_job(now, job, Some(&token)) {
            Ok(id) => {
                self.tokens.insert(id, token);
                Ok(id)
            }
            Err(e) => {
                let _ = self.diag.end_exclusive(now, token);
                Err(job_error(&e))
            }
        }
    }

    /// Starts a scan; its ID is the install list's. One at a time.
    pub fn scan(&mut self, now: Instant) -> Result<JobId, String> {
        if self.scan.is_some() {
            return Err("Scan running".into());
        }
        let id = self.start(now, Job::InstallList, false)?;
        self.scan = Some(Scan {
            id,
            queue: VecDeque::new(),
            current: None,
            report: ScanReport::default(),
        });
        Ok(id)
    }

    /// A finished scan's report, once.
    pub fn take_report(&mut self, id: JobId) -> Option<ScanReport> {
        self.reports.remove(&id)
    }

    pub fn status(&self, now: Instant) -> proto::DiagStatus {
        let mode = self.diag.mode();
        proto::DiagStatus {
            mode: mode_name(mode).into(),
            vcds_auto_pause: matches!(mode, Mode::VcdsPause { manual: false }),
            channels: self
                .diag
                .channels()
                .into_iter()
                .map(|c| proto::DiagChannel {
                    module: c.module,
                    state: match c.state {
                        ChannelState::Closed => "closed",
                        ChannelState::Opening => "opening",
                        ChannelState::Open => "open",
                        ChannelState::Grace { .. } => "grace",
                        ChannelState::Failed { .. } => "failed",
                    }
                    .into(),
                    failures: c.failures,
                    last_error: c.last_error.map(|e| format!("{e:?}")),
                })
                .collect(),
            subscriptions: self
                .diag
                .statuses(now)
                .into_iter()
                .map(|(_, s)| proto::DiagSubscription {
                    value: s.value.into(),
                    source: match s.source {
                        Source::Ican(_) => "ican".into(),
                        Source::Diag { module, group, .. } => format!("diag:{module}:{group}"),
                        Source::Unavailable => "unavailable".into(),
                    },
                    requested_ms: s.requested.as_millis() as u64,
                    granted_ms: s.granted.map(|g| g.as_millis() as u64),
                    achieved_hz: s.achieved_hz,
                    degraded: s.degraded.map(degraded),
                })
                .collect(),
        }
    }

    /// Timers, then frames out through the gate, samples into the store.
    pub fn service(
        &mut self,
        now: Instant,
        store: &mut Store,
        view: Option<BusView>,
        gate: &mut Gate,
        out: &mut Vec<Output>,
    ) {
        self.restore_haldex(now, view.is_some_and(|v| v.kl15 == Some(true)));
        self.diag.tick(now);
        self.pump(now, store, view, gate, out);
    }

    /// Events, frames and samples, after any call into `Diag`. Events go
    /// first: a scan starts its next read from them.
    pub fn pump(
        &mut self,
        now: Instant,
        store: &mut Store,
        view: Option<BusView>,
        gate: &mut Gate,
        out: &mut Vec<Output>,
    ) {
        self.events(now);
        for (id, mode) in self.haldex_confirmed.drain(..) {
            out.push(Output::HaldexConfirmed { id, mode });
        }
        while let Some(f) = self.diag.poll_transmit() {
            // No ICAN view: no Kl15, nothing passes.
            if let Some(v) = &view
                && gate.check(TxClass::Diag, &f, now, v).is_ok()
            {
                out.push(Output::Send {
                    iface: self.iface,
                    frame: f,
                });
            }
        }
        self.diag.write_samples(store);
        let mode = self.diag.mode();
        let mut set = |n: &str, v: Value| store.set(n, Some(v), now, DERIVED);
        set("diag.mode", Value::Text(mode_name(mode).into()));
        set(
            "diag.vcds_auto_pause",
            Value::Bool(matches!(mode, Mode::VcdsPause { manual: false })),
        );
        set(
            "diag.vcds_manual_pause",
            Value::Bool(matches!(mode, Mode::VcdsPause { manual: true })),
        );
        let open = self
            .diag
            .channels()
            .iter()
            .filter(|c| matches!(c.state, ChannelState::Open | ChannelState::Grace { .. }))
            .count();
        set("diag.channels_open", Value::Num(open as f64));
        set(
            "haldex.control.status",
            Value::Text(self.haldex_status.clone()),
        );
        set(
            "haldex.control.applied",
            Value::Text(self.haldex_applied.map_or("Unknown", |m| m.name()).into()),
        );
    }

    /// Diag's events: tokens back, API job events, the scan's next step.
    fn events(&mut self, now: Instant) {
        while let Some(e) = self.diag.poll_event() {
            if let Event::Job(j) = e {
                if let JobEvent::Done { id, .. } = &j
                    && let Some(t) = self.tokens.remove(id)
                {
                    let _ = self.diag.end_exclusive(now, t);
                }
                let id = match &j {
                    JobEvent::Progress { id, .. } | JobEvent::Done { id, .. } => *id,
                };
                let module = match &j {
                    JobEvent::Done { .. } => self.job_modules.remove(&id),
                    JobEvent::Progress { .. } => self.job_modules.get(&id).copied(),
                };
                if let JobEvent::Done { result, .. } = &j
                    && let Some((_, mode)) = self.haldex_job.filter(|j| j.0 == id)
                {
                    self.haldex_job = None;
                    self.haldex_next = Some(now + Duration::from_secs(10));
                    match result {
                        Ok(JobOutput::HaldexMode(_)) => {
                            self.haldex.last_mode = Some(mode);
                            self.haldex_applied = Some(mode);
                            self.haldex_status = "Confirmed".into();
                            self.haldex_confirmed.push((id, mode));
                        }
                        Err(e) => {
                            self.haldex_applied = None;
                            self.haldex_status = job_error(e);
                            // An unsupported handler never receives repeated probes or writes.
                            if matches!(e, JobError::HaldexUnsupported) {
                                self.haldex_next = None;
                            }
                        }
                        _ => {}
                    }
                }
                if self.scan_owns(id) {
                    self.on_scan(now, &j);
                    continue;
                }
                self.jobs.push((id, job_events(&j, module.unwrap_or(0))));
            }
        }
    }

    fn scan_owns(&self, id: JobId) -> bool {
        self.scan
            .as_ref()
            .is_some_and(|s| s.id == id || s.current.is_some_and(|c| c.2 == id))
    }

    /// One event of the scan's install list or of its current read.
    fn on_scan(&mut self, now: Instant, ev: &JobEvent) {
        let JobEvent::Done { id, result } = ev else {
            return;
        };
        let Some(scan) = self.scan.as_mut() else {
            return;
        };
        let sid = scan.id;
        let mut events = Vec::new();
        if *id == sid {
            match result {
                Ok(JobOutput::InstallList(l)) => {
                    events.push(install_list(l));
                    for m in l.iter().filter(|m| m.coded()) {
                        let i = scan.report.modules.len();
                        scan.report.modules.push(ScanModule {
                            address: m.vcds,
                            tp2: m.tp2,
                            name: modules::name_of(m.vcds),
                            status: m.status,
                            dtcs: None,
                            ident: None,
                            error: None,
                        });
                        scan.queue.push_back((i, ScanStep::Dtcs));
                        scan.queue.push_back((i, ScanStep::Ident));
                    }
                }
                Ok(_) => scan.report.error = Some("Bad install list".into()),
                Err(e) => scan.report.error = Some(job_error(e)),
            }
        } else if let Some((i, step, _)) = scan.current.take() {
            let m = &mut scan.report.modules[i];
            match (step, result) {
                (ScanStep::Dtcs, Ok(JobOutput::Dtcs(d))) => {
                    events.push(dtcs(m.tp2, d));
                    m.dtcs = Some(d.clone());
                }
                (ScanStep::Ident, Ok(JobOutput::Identity(id))) => {
                    events.push(identity(m.tp2, id));
                    m.ident = Some(id.clone());
                }
                (ScanStep::Dtcs, Err(e)) => {
                    m.error = Some(job_error(e));
                    events.push(stage("failed", format!("{:02X} {}", m.address, m.name)));
                    // No answer: don't ask again for its identity.
                    if matches!(e, JobError::Channel(_)) {
                        scan.queue.retain(|q| q.0 != i);
                    }
                }
                _ => {}
            }
        }
        self.jobs.push((sid, events));
        if self.scan.as_ref().is_some_and(|s| s.report.error.is_none()) {
            self.scan_next(now);
        } else {
            self.scan_end();
        }
    }

    /// Starts the scan's next read, or ends it.
    fn scan_next(&mut self, now: Instant) {
        loop {
            let Some(scan) = self.scan.as_mut() else {
                return;
            };
            let Some((i, step)) = scan.queue.pop_front() else {
                return self.scan_end();
            };
            let (tp2, address, name) = {
                let m = &scan.report.modules[i];
                (m.tp2, m.address, m.name)
            };
            let job = match step {
                ScanStep::Dtcs => Job::ReadDtcs { module: tp2 },
                ScanStep::Ident => Job::Identify { module: tp2 },
            };
            match self.start(now, job, false) {
                Ok(jid) => {
                    let scan = self.scan.as_mut().expect("scan is on");
                    scan.current = Some((i, step, jid));
                    if step == ScanStep::Dtcs {
                        let sid = scan.id;
                        self.jobs
                            .push((sid, vec![stage("module", format!("{address:02X} {name}"))]));
                    }
                    return;
                }
                Err(e) => {
                    let scan = self.scan.as_mut().expect("scan is on");
                    if self.diag.mode() != Mode::Normal
                        && !matches!(self.diag.mode(), Mode::Exclusive { .. })
                    {
                        scan.report.error = Some(e);
                        return self.scan_end();
                    }
                    if step == ScanStep::Dtcs {
                        scan.report.modules[i].error = Some(e);
                    }
                }
            }
        }
    }

    fn scan_end(&mut self) {
        let Some(scan) = self.scan.take() else {
            return;
        };
        let r = &scan.report;
        let ev = match &r.error {
            None => finished(
                true,
                format!(
                    "{} of {} modules, {} faults",
                    r.answered(),
                    r.modules.len(),
                    r.faults()
                ),
            ),
            Some(e) => finished(false, e.clone()),
        };
        self.jobs.push((scan.id, vec![ev]));
        self.reports.insert(scan.id, scan.report);
    }

    /// Job events since the last call, as API events. A job that started
    /// and failed in the same call still gets its events here.
    pub fn drain_jobs(&mut self) -> Vec<(JobId, Vec<proto::JobEvent>)> {
        std::mem::take(&mut self.jobs)
    }
}

fn degraded(d: Degraded) -> String {
    match d {
        Degraded::Paused(m) => format!("paused ({})", mode_name(m)),
        Degraded::Denied { module } => format!("module {module} is denied"),
        Degraded::Rejected { module, group, nrc } => match nrc {
            Some(n) => format!("module {module} refused group {group} (NRC {n:02X})"),
            None => format!("module {module} refused group {group}"),
        },
        Degraded::Backoff { module, .. } => format!("module {module} failed, retrying"),
        Degraded::WaitingForChannel { module } => format!("waiting for a channel to {module}"),
        Degraded::Budget => "slower than asked".into(),
    }
}

/// One diag job event as API job events. A `Done` gives its result and then
/// `Finished`.
pub fn job_events(ev: &JobEvent, module: u8) -> Vec<proto::JobEvent> {
    match ev {
        JobEvent::Progress { stage: s, .. } => vec![match s {
            JobStage::Queued => stage("queued", "waiting for the channel".into()),
            JobStage::Request { n, bytes } => stage(
                "request",
                format!("request {}: {:02X} {:02X}", n + 1, bytes[0], bytes[1]),
            ),
        }],
        JobEvent::Done { result, .. } => match result {
            Ok(JobOutput::InstallList(l)) => vec![
                install_list(l),
                finished(true, format!("{} modules", l.len())),
            ],
            Ok(JobOutput::Dtcs(d)) => vec![
                dtcs(module, d),
                finished(true, format!("{} fault codes", d.len())),
            ],
            Ok(JobOutput::Cleared { remaining }) => vec![
                dtcs(module, remaining),
                finished(
                    true,
                    format!("cleared; {} fault codes remain", remaining.len()),
                ),
            ],
            Ok(JobOutput::Identity(i)) => vec![
                identity(module, i),
                finished(true, i.part_no.clone().unwrap_or_default()),
            ],
            Ok(JobOutput::Group(g)) => vec![
                group_event(module, g),
                finished(true, format!("group {}", g.group)),
            ],
            Ok(JobOutput::HaldexMode(s)) => vec![finished(
                true,
                format!(
                    "{} confirmed",
                    proto::HaldexMode::ALL[usize::from(s.applied)].name()
                ),
            )],
            Err(e) => vec![finished(false, job_error(e))],
        },
    }
}

fn install_list(l: &[Installed]) -> proto::JobEvent {
    proto::JobEvent::InstallList {
        modules: l
            .iter()
            .map(|m| proto::InstalledModule {
                vcds: m.vcds,
                tp2: m.tp2,
                coded: m.coded(),
                status: m.status,
            })
            .collect(),
    }
}

fn dtcs(module: u8, d: &[Dtc]) -> proto::JobEvent {
    proto::JobEvent::Dtcs {
        module,
        dtcs: d
            .iter()
            .map(|d| proto::Dtc {
                code: d.code,
                status: d.status,
            })
            .collect(),
    }
}

fn identity(module: u8, i: &Ident) -> proto::JobEvent {
    proto::JobEvent::Identity {
        module,
        part_no: i.part_no.clone(),
        component: i.component.clone(),
        sw_version: i.sw_version.clone(),
        coding: i.coding.clone(),
        workshop: None,
    }
}

/// A measuring block's fields as the UI shows them.
pub fn group_event(module: u8, g: &Group) -> proto::JobEvent {
    proto::JobEvent::Group {
        module,
        group: g.group,
        fields: g
            .records
            .iter()
            .map(|r| match r.decode() {
                Some(d) => match d.value {
                    GroupValue::Number(n) => proto::GroupField {
                        value: Some(n),
                        text: number(n),
                        unit: d.unit.into(),
                    },
                    GroupValue::Text(t) => proto::GroupField {
                        value: None,
                        text: t,
                        unit: d.unit.into(),
                    },
                },
                None => proto::GroupField {
                    value: None,
                    text: format!("{:02X} {:02X} {:02X}", r.formula, r.a, r.b),
                    unit: String::new(),
                },
            })
            .collect(),
    }
}

/// At most 3 decimals, no trailing zeros.
fn number(n: f64) -> String {
    let s = format!("{n:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

fn stage(stage: &str, message: String) -> proto::JobEvent {
    proto::JobEvent::Stage {
        stage: stage.into(),
        message,
    }
}

fn finished(ok: bool, message: String) -> proto::JobEvent {
    proto::JobEvent::Finished { ok, message }
}

#[cfg(test)]
mod tests {
    use super::*;
    use carpi_kwp::group::Record;

    #[test]
    fn haldex_opt_in_restore_and_disable() {
        use carpi_sim::tp2::{Gateway, ModuleSpec};
        use proto::{HaldexMode as M, HaldexSettings as S};
        let mut spec = ModuleSpec::haldex();
        spec.replies.insert(
            vec![0x3B, 0xF0],
            [
                vec![0x7B, 0xF0, 0, 0, 0xFF, 3],
                vec![0x7B, 0xF0, 1, 1, 0xFF, 3],
            ]
            .into(),
        );
        spec.replies.insert(
            vec![0x3B, 0xF0, 0xA5, 0x5A],
            [vec![0x7B, 0xF0, 0, 0, 1, 1]].into(),
        );
        let mut gw = Gateway::new([spec]);
        let mut bus = carpi_sim::bus::Bus::new(Instant::now());
        let mut n = Node {
            d: Diagnostics::new(0),
            events: Vec::new(),
        };
        let saved = S {
            enabled: false,
            last_mode: Some(M::Performance),
        };
        n.d.configure_haldex(saved, true, bus.now);
        bus.run_for(&mut [&mut n, &mut gw], Duration::from_secs(20));
        assert!(gw.module(0x0A).requests.is_empty());
        assert!(n.d.haldex_start(bus.now, M::Stock).is_err());
        n.d.configure_haldex(
            S {
                enabled: true,
                ..saved
            },
            true,
            bus.now,
        );
        bus.run_for(&mut [&mut n, &mut gw], Duration::from_secs(2));
        assert_eq!(n.d.haldex_applied, Some(M::Performance));
        assert_eq!(n.d.haldex_confirmed.len(), 1);
        n.d.configure_haldex(saved, true, bus.now);
        let count = gw.module(0x0A).requests.len();
        bus.run_for(&mut [&mut n, &mut gw], Duration::from_secs(20));
        assert_eq!(gw.module(0x0A).requests.len(), count);
        assert!(n.d.haldex_next.is_none());
    }

    #[test]
    fn haldex_disable_cancels_inflight_switch_without_remembering_it() {
        use carpi_sim::tp2::{Gateway, ModuleSpec};
        let mut spec = ModuleSpec::haldex();
        spec.replies.insert(
            vec![0x3B, 0xF0],
            [
                vec![0x7B, 0xF0, 0, 0, 0xFF, 3],
                vec![0x7B, 0xF0, 2, 2, 0xFF, 3],
            ]
            .into(),
        );
        spec.replies.insert(
            vec![0x3B, 0xF0, 0x3C, 0xC3],
            [vec![0x7B, 0xF0, 0, 0, 2, 1]].into(),
        );
        let mut gw = Gateway::new([spec]);
        let mut bus = carpi_sim::bus::Bus::new(Instant::now());
        let mut n = Node {
            d: Diagnostics::new(0),
            events: Vec::new(),
        };
        let cfg = proto::HaldexSettings {
            enabled: true,
            last_mode: None,
        };
        n.d.configure_haldex(cfg, true, bus.now);
        n.d.haldex_start(bus.now, proto::HaldexMode::Competition)
            .unwrap();
        for _ in 0..100 {
            bus.run_for(&mut [&mut n, &mut gw], Duration::from_millis(10));
            if gw
                .module(0x0A)
                .requests
                .iter()
                .any(|r| r.len() == 4 && r[..2] == [0x3B, 0xF0])
            {
                break;
            }
        }
        assert!(n.d.haldex_job.is_some());
        n.d.configure_haldex(
            proto::HaldexSettings {
                enabled: false,
                ..cfg
            },
            true,
            bus.now,
        );
        bus.run_for(&mut [&mut n, &mut gw], Duration::from_secs(20));
        assert!(n.d.haldex_confirmed.is_empty());
        assert!(n.d.haldex.last_mode.is_none());
        assert!(n.d.tokens.is_empty());
        assert_eq!(
            gw.module(0x0A)
                .requests
                .iter()
                .filter(|r| r[..2] == [0x3B, 0xF0])
                .count(),
            2
        );
    }

    #[test]
    fn haldex_fresh_enable_and_inhibited_tx_do_not_restore() {
        let now = Instant::now();
        let mut d = Diagnostics::new(0);
        d.configure_haldex(
            proto::HaldexSettings {
                enabled: true,
                last_mode: None,
            },
            true,
            now,
        );
        d.restore_haldex(now, true);
        assert!(d.haldex_job.is_none());
        d.configure_haldex(
            proto::HaldexSettings {
                enabled: true,
                last_mode: Some(proto::HaldexMode::Stock),
            },
            false,
            now,
        );
        assert!(d.haldex_start(now, proto::HaldexMode::Stock).is_err());
        assert!(d.haldex_next.is_none());
    }

    #[test]
    fn group_fields_for_the_ui() {
        let g = Group {
            group: 3,
            records: vec![
                Record {
                    formula: 0x01,
                    a: 0xC8,
                    b: 0x15,
                },
                Record {
                    formula: 0x19,
                    a: 0x20,
                    b: 0x03,
                },
                Record {
                    formula: 0x0A,
                    a: 0,
                    b: 1,
                },
                Record {
                    formula: 0x7D,
                    a: 1,
                    b: 2,
                },
            ],
        };
        let proto::JobEvent::Group { fields, .. } = group_event(1, &g) else {
            panic!()
        };
        assert_eq!(fields[0].text, "840");
        assert_eq!(fields[0].unit, "rpm");
        assert_eq!(fields[1].text, "4.439");
        assert_eq!(fields[2].value, None);
        assert_eq!(fields[2].text, "WARM");
        assert_eq!(fields[3].text, "7D 01 02");
        assert_eq!(fields[3].unit, "");
    }

    /// Diagnostics on the sim bus, frames straight out (no gate).
    struct Node {
        d: Diagnostics,
        events: Vec<(JobId, proto::JobEvent)>,
    }

    impl Node {
        fn settle(&mut self, now: Instant) {
            self.d.events(now);
            for (id, evs) in self.d.drain_jobs() {
                self.events.extend(evs.into_iter().map(|e| (id, e)));
            }
        }
    }

    impl carpi_sim::bus::Node for Node {
        fn on_frame(&mut self, now: Instant, f: &carpi_can::Frame) {
            self.d.on_frame(now, f);
            self.settle(now);
        }
        fn tick(&mut self, now: Instant) {
            self.d.restore_haldex(now, true);
            self.d.diag.tick(now);
            self.settle(now);
        }
        fn poll_transmit(&mut self, _now: Instant) -> Option<carpi_can::Frame> {
            self.d.diag.poll_transmit()
        }
        fn next_deadline(&self) -> Option<Instant> {
            self.d.next_deadline()
        }
    }

    #[test]
    fn scan_reads_every_coded_module_in_turn() {
        use carpi_sim::tp2::{Gateway, ModuleSpec};
        let abs = ModuleSpec {
            tp2: 0x03,
            rx_id: 0x743,
            params: [0x0F, 0x8A, 0xFF, 0x4A, 0xFF],
            ident: [&b"1K0614517DL 0110"[..], &[0; 10], b"ESP MK60EC1"].concat(),
            dtcs: vec![(1314, 0x23)],
            ..Default::default()
        };
        let mut gw = Gateway::new([
            ModuleSpec::gateway(),
            ModuleSpec::engine(),
            ModuleSpec::haldex(),
            abs,
        ]);
        let mut bus = carpi_sim::bus::Bus::new(Instant::now());
        let mut n = Node {
            d: Diagnostics::new(0),
            events: Vec::new(),
        };
        let id = n.d.scan(bus.now).unwrap();
        assert_eq!(n.d.scan(bus.now), Err("Scan running".into()));
        n.settle(bus.now);
        for _ in 0..600 {
            bus.run_for(&mut [&mut n, &mut gw], Duration::from_millis(100));
            if n.d.scan.is_none() {
                break;
            }
        }
        assert!(n.events.iter().all(|(j, _)| *j == id));
        let evs: Vec<&proto::JobEvent> = n.events.iter().map(|(_, e)| e).collect();
        // Install list first, Finished last, a fault read per answering module.
        assert!(matches!(evs[0], proto::JobEvent::InstallList { modules } if modules.len() == 6));
        let Some(proto::JobEvent::Finished { ok: true, message }) = evs.last() else {
            panic!("{evs:#?}")
        };
        let dtc_modules: Vec<u8> = evs
            .iter()
            .filter_map(|e| match e {
                proto::JobEvent::Dtcs { module, .. } => Some(*module),
                _ => None,
            })
            .collect();
        // Engine, ABS (one-off, under a read-only token), Haldex; the BCM,
        // the gateway's own entry and KSG don't answer here (no sim module
        // for 0x20 / 0x21; the gateway sim answers no 18).
        assert!(
            dtc_modules.contains(&0x01) && dtc_modules.contains(&0x03),
            "{dtc_modules:?}"
        );
        assert!(evs.iter().any(|e| matches!(e,
            proto::JobEvent::Identity { module: 3, part_no: Some(p), component: Some(c), .. }
                if p == "1K0614517DL" && c == "ESP MK60EC1")));
        assert!(
            evs.iter()
                .any(|e| matches!(e, proto::JobEvent::Stage { stage, .. } if stage == "failed"))
        );
        let r = n.d.take_report(id).unwrap();
        assert!(n.d.take_report(id).is_none());
        // Coded only: KSG 46 has status 09 (coded) and stays in.
        assert_eq!(r.modules.len(), 6);
        assert_eq!(r.modules[1].name, "ABS brakes");
        assert_eq!(r.faults(), 2);
        assert_eq!(message, &format!("{} of 6 modules, 2 faults", r.answered()));
        // The token went back, and ABS was never asked for a measuring block.
        assert_eq!(n.d.diag.mode(), Mode::Normal);
        assert!(
            bus.sent_by(0)
                .all(|(_, id, d)| !(id == 0x743 && d.len() > 3 && d[3] == 0x21))
        );
    }

    #[test]
    fn subscription_names_add_module_groups() {
        let c = Catalog::seed();
        let p = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(
            subscription_names(&c, &p(&["diag.01.115"])),
            [
                "diag.01.115.1",
                "diag.01.115.2",
                "diag.01.115.3",
                "diag.01.115.4"
            ]
        );
        let n = subscription_names(&c, &p(&["diag.22.1.2", "engine.boost.actual"]));
        assert_eq!(n, ["diag.22.1.2", "engine.boost.actual"]);
        assert!(subscription_names(&c, &p(&["diag.", ""])).is_empty());
    }
}
