//! Diagnostics on the `diag` bus role: one [`carpi_diag::Diag`] fed every
//! frame from that interface, its frames out through that interface's gate
//! as `TxClass::Diag`, its samples into the store.
//!
//! - API subscriptions map onto diag subscriptions: every catalog value
//!   under one of the subscription's prefixes, with a lease of 3x the
//!   period that the subscription task renews. An empty prefix maps to
//!   nothing, so "everything" never starts polling.
//! - Jobs: install list, DTC read, DTC clear. A clear takes the exclusive
//!   token, so only an explicit `confirm` starts one; the token goes back
//!   when the job ends.
//! - `diag.mode`, `diag.vcds_auto_pause`, `diag.vcds_manual_pause`,
//!   `diag.channels_open` in the store, every tick.

use carpi_diag::{
    Catalog, ChannelState, Degraded, Diag, Event, Job, JobEvent, JobId, JobOutput, JobStage, Mode,
    Source, SubId, Subscription,
};
use carpi_gate::{BusView, Gate, TxClass};
use carpi_kwp::{Class, ExclusiveToken};
use carpi_proto as proto;
use carpi_state::{Store, Value};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::control::Output;

/// Lease = this x the subscription period.
pub const LEASE_PERIODS: u32 = 3;
/// Priority of API subscriptions. All equal for now.
const PRIORITY: u8 = 1;
const DERIVED: Duration = Duration::from_secs(1);

pub(crate) struct Diagnostics {
    pub iface: usize,
    diag: Diag,
    /// Exclusive tokens held for DTC clears, by job.
    tokens: BTreeMap<JobId, ExclusiveToken>,
    /// Module of each running job.
    job_modules: BTreeMap<JobId, u8>,
    jobs: Vec<(JobId, Vec<proto::JobEvent>)>,
}

pub fn mode_name(m: Mode) -> &'static str {
    match m {
        Mode::Normal => "normal",
        Mode::Exclusive { .. } => "exclusive",
        Mode::VcdsPause { .. } => "vcds_pause",
        Mode::BusLease => "bus_lease",
    }
}

/// Catalog names under any of `prefixes`. Empty prefixes match nothing.
pub fn matching(catalog: &Catalog, prefixes: &[String]) -> Vec<&'static str> {
    catalog
        .iter()
        .map(|d| d.name)
        .filter(|n| prefixes.iter().any(|p| !p.is_empty() && n.starts_with(p)))
        .collect()
}

impl Diagnostics {
    pub fn new(iface: usize) -> Self {
        Diagnostics {
            iface,
            diag: Diag::new(Default::default(), Catalog::seed()),
            tokens: BTreeMap::new(),
            job_modules: BTreeMap::new(),
            jobs: Vec::new(),
        }
    }

    pub fn diag(&self) -> &Diag {
        &self.diag
    }

    pub fn on_frame(&mut self, now: Instant, f: &carpi_can::Frame) {
        self.diag.on_frame(now, f);
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.diag.next_deadline()
    }

    pub fn subscribe(&mut self, now: Instant, prefixes: &[String], period: Duration) -> Vec<SubId> {
        let names = matching(self.diag.catalog(), prefixes);
        let lease = now + period * LEASE_PERIODS;
        names
            .into_iter()
            .filter_map(|value| {
                self.diag
                    .subscribe(
                        now,
                        Subscription {
                            value: value.into(),
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
        if !matches!(job, Job::ClearDtcs { .. }) {
            return self
                .diag
                .start_job(now, job, None)
                .map_err(|e| format!("{e:?}"));
        }
        if !confirm {
            return Err("clearing fault codes needs confirm: true".into());
        }
        let token = self
            .diag
            .begin_exclusive(now, Class::Persistent)
            .map_err(|e| format!("can't take the diag token: {e:?}"))?;
        match self.diag.start_job(now, job, Some(&token)) {
            Ok(id) => {
                self.tokens.insert(id, token);
                Ok(id)
            }
            Err(e) => {
                let _ = self.diag.end_exclusive(now, token);
                Err(format!("{e:?}"))
            }
        }
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
        self.diag.tick(now);
        self.pump(now, store, view, gate, out);
    }

    /// Frames, samples and events, after any call into `Diag`.
    pub fn pump(
        &mut self,
        now: Instant,
        store: &mut Store,
        view: Option<BusView>,
        gate: &mut Gate,
        out: &mut Vec<Output>,
    ) {
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
                self.jobs.push((id, job_events(&j, module.unwrap_or(0))));
            }
        }
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
        JobEvent::Progress { stage, .. } => vec![match stage {
            JobStage::Queued => proto::JobEvent::Stage {
                stage: "queued".into(),
                message: "waiting for the channel".into(),
            },
            JobStage::Request { n, bytes } => proto::JobEvent::Stage {
                stage: "request".into(),
                message: format!("request {}: {:02X} {:02X}", n + 1, bytes[0], bytes[1]),
            },
        }],
        JobEvent::Done { result, .. } => match result {
            Ok(JobOutput::InstallList(l)) => vec![
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
                },
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
            Err(e) => vec![finished(false, format!("{e:?}"))],
        },
    }
}

fn dtcs(module: u8, d: &[carpi_kwp::services::Dtc]) -> proto::JobEvent {
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

fn finished(ok: bool, message: String) -> proto::JobEvent {
    proto::JobEvent::Finished { ok, message }
}
