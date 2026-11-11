//! The car model: everything the daemon knows, fed one frame at a time.
//!
//! Sans-IO apart from writing capture files. The daemon owns the sockets and
//! the clock; tests and `carpi replay` drive this directly.

pub mod bridge;
mod control;
pub mod diagnostics;
pub mod display;
pub mod rnse_service;

pub use carpi_diag::{JobId as DiagJobId, SubId as DiagSubId};
pub use carpi_dis::ScreenId as DisScreen;
pub use carpi_dis::screens::accel::parse_leg as parse_accel_line;
pub use carpi_dis::screens::media::MediaLayout as DisMediaLayout;
pub use carpi_dis::screens::nav::{
    Clock as DisClock, LaneLayout as DisLanes, NavStyle as DisNavStyle, Units as DisUnits,
};
pub use control::{AmbientSource, CurveMode, DarkSetup, DarkSource, LightSetup, Location, Output};
pub use display::{DisCell, DisPage, DisSetup, Reading, default_accel};

use carpi_can::RxFrame;
use carpi_can::recorder::Recorder;
use carpi_gate::{BusView, Gate};
use carpi_input::RouteConfig;
use carpi_nm::{BusMonitor, Transition};
use carpi_proto as proto;
use carpi_state::{Store, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Bus roles. A role names what a bus is for; config maps roles to
/// interfaces, and one interface can carry several roles.
pub const ROLE_ICAN: &str = "ican";
pub const ROLE_DIAG: &str = "diag";

#[derive(Clone, Debug)]
pub struct IfaceSpec {
    /// Interface name as the OS knows it (`ican`, `can0`, `obd`, ...).
    pub name: String,
    /// Roles this interface carries.
    pub roles: Vec<String>,
}

impl IfaceSpec {
    pub fn has_role(&self, role: &str) -> bool {
        self.roles.iter().any(|r| r == role)
    }
}

struct Iface {
    spec: IfaceSpec,
    rec: u8,
    /// Only on the ICAN interface: that's where the ring is.
    monitor: Option<BusMonitor>,
    frames: u64,
}

/// What the daemon config sets in the car model. All of it can change live
/// through [`Car::configure`].
#[derive(Clone, Debug, Default)]
pub struct CoreConfig {
    pub dis: DisSetup,
    pub light: LightSetup,
    pub routes: RouteConfig,
}

/// A bus-state change, tagged with the interface it happened on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BusEvent {
    pub iface: String,
    pub transition: Transition,
    /// Ring members and their flags, frames since the last change.
    pub detail: String,
}

pub struct Car {
    ifaces: Vec<Iface>,
    store: Store,
    recorder: Recorder,
    capture_dir: PathBuf,
    /// API connection that last pushed bridge state (LIVI).
    bridge_conn: Option<u64>,
    controls: control::Controls,
    outputs: Vec<Output>,
    display: display::Display,
    /// On the interface with the `diag` role, if any.
    diag: Option<diagnostics::Diagnostics>,
    /// Diag subscriptions for the DIS readings page on screen: its keys, the
    /// ids, and when the lease was last renewed.
    dis_subs: (Vec<String>, Vec<DiagSubId>, Option<Instant>),
}

impl Car {
    pub fn new(ifaces: Vec<IfaceSpec>, mut recorder: Recorder, capture_dir: PathBuf) -> Self {
        let names: Vec<String> = ifaces.iter().map(|i| i.name.clone()).collect();
        let ifaces = ifaces
            .into_iter()
            .map(|spec| Iface {
                rec: recorder.bus(&spec.name),
                monitor: spec
                    .has_role(ROLE_ICAN)
                    .then(|| BusMonitor::new(Default::default())),
                frames: 0,
                spec,
            })
            .collect::<Vec<Iface>>();
        let diag = ifaces
            .iter()
            .position(|i| i.spec.has_role(ROLE_DIAG))
            .map(diagnostics::Diagnostics::new);
        Car {
            ifaces,
            store: Store::new(),
            recorder,
            capture_dir,
            bridge_conn: None,
            controls: control::Controls::new(&names),
            outputs: Vec::new(),
            display: display::Display::new(DisSetup::default()),
            diag,
            dis_subs: (Vec::new(), Vec::new(), None),
        }
    }

    /// Apply new settings. Nothing needs a restart.
    pub fn configure(&mut self, cfg: CoreConfig, now: Instant) {
        self.display.configure(cfg.dis, now);
        self.controls.set_light(cfg.light);
        self.controls.routes = cfg.routes;
    }

    /// Shutdown timings (`[power]`), live.
    pub fn set_power(&mut self, cfg: carpi_power::Config) {
        self.controls.set_power(cfg);
    }

    /// Key timings (`[input]`), live.
    pub fn set_gestures(&mut self, cfg: carpi_input::GestureConfig) {
        self.controls.set_gestures(cfg);
    }

    /// The gate of interface `idx`, for its counters.
    pub fn gate(&self, idx: usize) -> &Gate {
        &self.controls.gates[idx]
    }

    /// One read-only SDLink decoder request: an aligned 64-byte B1 read of
    /// sector 107, through [`Car::rnse_service_request`].
    pub fn decoder_read_request(
        &mut self,
        address: u32,
        now: Instant,
    ) -> Result<(usize, carpi_can::Frame), String> {
        let sector = rnse_service::SECTOR_XIP..rnse_service::SECTOR_XIP + rnse_service::SECTOR_SIZE;
        if !sector.contains(&address) || !address.is_multiple_of(64) {
            return Err("decoder read is outside sector 107".into());
        }
        self.rnse_service_request(&rnse_service::read(address, 64), now)
    }

    /// One RNS-E service frame on 0x7B0 ([`rnse_service`]): reads through the
    /// ICAN `Diag` class (`Lease` while leased), writes only under the lease.
    pub fn rnse_service_request(
        &mut self,
        data: &[u8; 8],
        now: Instant,
    ) -> Result<(usize, carpi_can::Frame), String> {
        let need = rnse_service::classify(data)?;
        let iface = self.ican_idx().ok_or("no ICAN interface configured")?;
        let view = self.view(now).ok_or("ICAN state unavailable")?;
        let gate = &mut self.controls.gates[iface];
        let class = match (need, gate.leased(now)) {
            (_, true) => carpi_gate::TxClass::Lease,
            (rnse_service::Need::Read, false) => carpi_gate::TxClass::Diag,
            (rnse_service::Need::Lease, false) => {
                return Err("RNS-E writes need the bus lease".into());
            }
        };
        let frame = rnse_service::frame(data);
        gate.check(class, &frame, now, &view)
            .map_err(|e| format!("RNS-E request refused: {e}"))?;
        Ok((iface, frame))
    }

    /// Takes the ICAN bus for an RNS-E update for at most `max`: every other
    /// sender is refused until [`Car::rnse_release`] or expiry.
    pub fn rnse_lease(&mut self, max: Duration, now: Instant) -> Result<(), String> {
        let iface = self.ican_idx().ok_or("no ICAN interface configured")?;
        let view = self.view(now).ok_or("ICAN state unavailable")?;
        self.controls.gates[iface]
            .take_lease(now, max, &view)
            .map_err(|e| format!("bus lease refused: {e}"))
    }

    pub fn rnse_release(&mut self) {
        if let Some(iface) = self.ican_idx() {
            self.controls.gates[iface].release_lease();
        }
    }

    /// Kl15 as the gates see it now.
    pub fn kl15(&self, now: Instant) -> Option<bool> {
        carpi_gate::kl15(&self.store, now)
    }

    /// The DDP session, for state.
    pub fn dis_session(&self) -> &carpi_dis::Session {
        self.display.session()
    }

    fn ican_idx(&self) -> Option<usize> {
        self.ifaces.iter().position(|i| i.monitor.is_some())
    }

    /// What the gates see now. A separate diag bus has no ring the Pi can
    /// see, so every gate uses the ICAN view.
    fn view(&self, now: Instant) -> Option<BusView> {
        self.ican().map(|m| BusView::new(m, &self.store, now))
    }

    /// DIS keys the router just produced drive the display.
    fn take_dis_keys(&mut self, from: usize, now: Instant) {
        let keys: Vec<carpi_input::DisKey> = self.outputs[from..]
            .iter()
            .filter_map(|o| match o {
                Output::Dis(k) => Some(*k),
                _ => None,
            })
            .collect();
        for k in keys {
            self.display.key(now, k);
        }
    }

    /// Display and diag timers and output. [`Car::tick`] calls this; call it
    /// at [`Car::next_deadline`] too.
    pub fn service(&mut self, now: Instant) {
        let view = self.view(now);
        let ican = self.ican_idx();
        let focus = self.controls.focus == carpi_input::Focus::Dis;
        self.display.set_focus(now, focus);
        let gate = ican.map(|i| (i, &mut self.controls.gates[i]));
        self.display
            .service(now, &mut self.store, view, gate, &mut self.outputs);
        self.sync_dis_subs(now);
        if let Some(d) = &mut self.diag {
            let gate = &mut self.controls.gates[d.iface];
            d.service(now, &mut self.store, view, gate, &mut self.outputs);
        }
    }

    /// Keep diag polling the values the DIS readings page shows (the old
    /// readings app's value requests), and only while it's on screen.
    fn sync_dis_subs(&mut self, now: Instant) {
        const PERIOD: Duration = Duration::from_millis(500);
        let want = self.display.wanted_values();
        if want != self.dis_subs.0 {
            let old = std::mem::take(&mut self.dis_subs.1);
            if !old.is_empty() {
                self.diag_unsubscribe(&old, now);
            }
            let ids = if want.is_empty() {
                Vec::new()
            } else {
                self.diag_subscribe(&want, PERIOD, now)
            };
            if !want.is_empty() {
                tracing::info!(
                    "dis: polling {} diag values for the readings page",
                    ids.len()
                );
            }
            self.dis_subs = (want, ids, Some(now));
        } else if !self.dis_subs.1.is_empty() && self.dis_subs.2.is_none_or(|t| now >= t + PERIOD) {
            let ids = self.dis_subs.1.clone();
            self.diag_renew(&ids, PERIOD, now);
            self.dis_subs.2 = Some(now);
        }
    }

    /// Earliest time the display or diag has something to do.
    pub fn next_deadline(&self, now: Instant) -> Option<Instant> {
        [
            self.display.next_deadline(now),
            self.diag.as_ref().and_then(|d| d.next_deadline()),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    fn pump_diag(&mut self, now: Instant) {
        let view = self.view(now);
        if let Some(d) = &mut self.diag {
            let gate = &mut self.controls.gates[d.iface];
            d.pump(now, &mut self.store, view, gate, &mut self.outputs);
        }
    }

    /// Diag subscriptions for every catalog value under `prefixes`, leased
    /// for 3x `period`. Empty when there's no diag bus or nothing matches.
    pub fn diag_subscribe(
        &mut self,
        prefixes: &[String],
        period: Duration,
        now: Instant,
    ) -> Vec<DiagSubId> {
        let Some(d) = &mut self.diag else {
            return Vec::new();
        };
        let ids = d.subscribe(now, prefixes, period);
        self.pump_diag(now);
        ids
    }

    /// Extends the leases to 3x `period` from now.
    pub fn diag_renew(&mut self, ids: &[DiagSubId], period: Duration, now: Instant) {
        if let Some(d) = &mut self.diag {
            d.renew(ids, now + period * diagnostics::LEASE_PERIODS);
        }
    }

    pub fn diag_unsubscribe(&mut self, ids: &[DiagSubId], now: Instant) {
        if let Some(d) = &mut self.diag {
            d.unsubscribe(now, ids);
        }
        self.pump_diag(now);
    }

    /// Starts a diag job. `Status` isn't a job: see [`Car::diag_status`].
    pub fn diag_start(
        &mut self,
        action: proto::DiagAction,
        now: Instant,
    ) -> Result<DiagJobId, String> {
        use carpi_diag::Job;
        let Some(d) = &mut self.diag else {
            return Err("no interface has the diag role".into());
        };
        let (job, confirm) = match action {
            proto::DiagAction::InstallList => (Job::InstallList, false),
            proto::DiagAction::ReadDtcs { module } => (Job::ReadDtcs { module }, false),
            proto::DiagAction::ClearDtcs { module, confirm } => {
                (Job::ClearDtcs { module }, confirm)
            }
            proto::DiagAction::Identify { module } => (Job::Identify { module }, false),
            proto::DiagAction::ReadGroup { module, group } => {
                (Job::ReadGroup { module, group }, false)
            }
            proto::DiagAction::Scan => {
                let r = d.scan(now);
                self.pump_diag(now);
                return r;
            }
            proto::DiagAction::ReadAdaptation { .. } => {
                return Err("Adaptation not supported".into());
            }
            proto::DiagAction::WriteCoding { .. } | proto::DiagAction::WriteAdaptation { .. } => {
                return Err(if diagnostics::WRITES_ENABLED {
                    "Writes not built"
                } else {
                    "Writes disabled"
                }
                .into());
            }
            proto::DiagAction::Status => return Err("status isn't a job".into()),
        };
        let r = d.start(now, job, confirm);
        self.pump_diag(now);
        r
    }

    pub fn diag_status(&self, now: Instant) -> Option<proto::DiagStatus> {
        self.diag.as_ref().map(|d| d.status(now))
    }

    /// The report of scan `id`, once it has finished (its `Finished` event
    /// is out). Once.
    pub fn take_scan_report(&mut self, id: DiagJobId) -> Option<diagnostics::ScanReport> {
        self.diag.as_mut().and_then(|d| d.take_report(id))
    }

    /// Diag job events since the last call, as API events.
    pub fn drain_diag_jobs(&mut self) -> Vec<(DiagJobId, Vec<proto::JobEvent>)> {
        self.diag
            .as_mut()
            .map(|d| d.drain_jobs())
            .unwrap_or_default()
    }

    /// The diag catalog, if there's a diag bus.
    pub fn diag_catalog(&self) -> Option<&carpi_diag::Catalog> {
        self.diag.as_ref().map(|d| d.diag().catalog())
    }

    pub fn ifaces(&self) -> impl Iterator<Item = &IfaceSpec> {
        self.ifaces.iter().map(|i| &i.spec)
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Values from outside the car (the A/V-over-SD stream), unknown after
    /// `timeout` without another set.
    pub fn set_values(
        &mut self,
        values: Vec<(String, Option<Value>)>,
        timeout: Duration,
        now: Instant,
    ) {
        for (name, value) in values {
            self.store.set(&name, value, now, timeout);
        }
    }

    /// Feeds one received frame from interface `idx`.
    pub fn ingest(&mut self, idx: usize, rx: &RxFrame) -> Option<BusEvent> {
        let iface = &mut self.ifaces[idx];
        iface.frames += 1;
        self.recorder.push(iface.rec, *rx);

        let mut event = None;
        if let Some(mon) = iface.monitor.as_mut()
            && let Some(t) = mon.on_frame(rx.at, rx.frame.id, rx.frame.data())
        {
            event = Some(BusEvent {
                iface: iface.spec.name.clone(),
                transition: t,
                detail: mon.take_detail(rx.at),
            });
        }
        if iface.spec.has_role(ROLE_ICAN)
            && !rx.frame.extended
            && let Some(msg) = carpi_signals::message(rx.frame.id)
        {
            let timeout = msg.timeout();
            let store = &mut self.store;
            (msg.decode)(rx.frame.data(), &mut |name, value| {
                store.set(name, value, rx.at, timeout)
            });
        }
        let is_ican = iface.spec.has_role(ROLE_ICAN);
        let from = self.outputs.len();
        if is_ican && !rx.frame.extended {
            self.controls.on_ican_frame(
                rx.at,
                rx.frame.id,
                rx.frame.data(),
                &self.store,
                self.bridge_conn.is_some(),
                &mut self.outputs,
            );
            self.take_dis_keys(from, rx.at);
        }
        let view = self.view(rx.at);
        if is_ican {
            self.display.on_frame(rx.at, &rx.frame);
            let gate = &mut self.controls.gates[idx];
            self.display
                .flush(rx.at, view, idx, gate, &mut self.outputs);
        }
        if let Some(d) = &mut self.diag
            && d.iface == idx
        {
            d.on_frame(rx.at, &rx.frame);
            let gate = &mut self.controls.gates[idx];
            d.pump(rx.at, &mut self.store, view, gate, &mut self.outputs);
        }
        event
    }

    /// Takes what the car model decided to do since the last call.
    pub fn drain_outputs(&mut self) -> Vec<Output> {
        std::mem::take(&mut self.outputs)
    }

    /// Call every ~100 ms.
    pub fn tick(&mut self, now: Instant) -> Vec<BusEvent> {
        let mut out = Vec::new();
        for iface in &mut self.ifaces {
            if let Some(mon) = iface.monitor.as_mut()
                && let Some(t) = mon.tick(now)
            {
                out.push(BusEvent {
                    iface: iface.spec.name.clone(),
                    transition: t,
                    detail: mon.take_detail(now),
                });
            }
        }
        let ican = self
            .ifaces
            .iter()
            .enumerate()
            .find_map(|(i, f)| f.monitor.as_ref().map(|m| (i, m)));
        let from = self.outputs.len();
        self.controls.tick(
            now,
            ican,
            &mut self.store,
            self.bridge_conn.is_some(),
            &mut self.outputs,
        );
        self.take_dis_keys(from, now);
        self.service(now);
        out
    }

    /// State of the ICAN ring, if an ICAN interface is configured.
    pub fn ican(&self) -> Option<&BusMonitor> {
        self.ifaces.iter().find_map(|i| i.monitor.as_ref())
    }

    /// Readings for every value under any of `prefixes`.
    pub fn readings(&self, prefixes: &[String], now: Instant) -> Vec<proto::Reading> {
        let mut out: Vec<proto::Reading> = Vec::new();
        for p in prefixes {
            for r in self.store.snapshot(p, now) {
                if out.iter().any(|o| o.name == r.name) {
                    continue;
                }
                out.push(proto::Reading {
                    name: r.name,
                    value: r.value.map(to_proto),
                    age_ms: r.age.as_millis() as u64,
                });
            }
        }
        out
    }

    /// An API connection closed. If it was LIVI's, its bridge state is gone.
    pub fn conn_closed(&mut self, conn: u64, now: Instant) {
        if self.bridge_conn == Some(conn) {
            self.bridge_conn = None;
            bridge::clear(&mut self.store, now);
        }
    }

    /// Answers one request from API connection `conn`. Subscriptions are
    /// run by the connection itself, using [`Car::readings`].
    pub fn handle(&mut self, conn: u64, req: proto::Request, now: Instant) -> proto::Response {
        use proto::{Request, Response};
        match req {
            Request::Bridge { update } => {
                self.bridge_conn = Some(conn);
                bridge::apply(&mut self.store, &update, now);
                Response::Ok
            }
            Request::Subscribe { .. }
            | Request::Unsubscribe { .. }
            | Request::Commands
            | Request::CommandDone { .. }
            | Request::Update { .. }
            | Request::Diag { .. }
            | Request::Settings { .. }
            // carpid's capture and datalog modules: they own the logs folder.
            | Request::Capture { .. }
            | Request::Wifi { .. }
            | Request::Storage { .. }
            | Request::DecoderCrc
            | Request::Firmware { .. }
            | Request::Datalog { .. } => Response::Error {
                message: "subscriptions, commands, jobs, captures and logging are handled per connection"
                    .into(),
            },
            Request::Hello { version, .. } => {
                if version == proto::PROTOCOL_VERSION {
                    Response::Hello {
                        version,
                        daemon: format!("carpid {}", env!("CARGO_PKG_VERSION")),
                    }
                } else {
                    Response::Error {
                        message: format!(
                            "protocol {version} not supported; carpid speaks {}",
                            proto::PROTOCOL_VERSION
                        ),
                    }
                }
            }
            Request::Light { action } => {
                match self.controls.light_action(action, &mut self.outputs) {
                    Ok(()) => Response::Ok,
                    Err(message) => Response::Error { message },
                }
            }
            Request::Values { prefix } => Response::Values {
                items: self.readings(&[prefix], now),
            },
            Request::Catalog => {
                static CATALOG: std::sync::OnceLock<carpi_diag::Catalog> =
                    std::sync::OnceLock::new();
                diagnostics::catalog_response(CATALOG.get_or_init(carpi_diag::Catalog::seed))
            }
            Request::Bus => Response::Bus {
                buses: self.buses(now),
            },
            Request::SaveCapture { path } => {
                let path = path
                    .map(PathBuf::from)
                    .unwrap_or_else(|| self.capture_dir.join(capture_name()));
                match self.save_capture(&path) {
                    Ok(frames) => Response::CaptureSaved {
                        path: path.display().to_string(),
                        frames,
                    },
                    Err(e) => Response::Error {
                        message: format!("saving capture to {}: {e}", path.display()),
                    },
                }
            }
        }
    }

    /// Every interface's counters and ring state.
    pub fn buses(&self, now: Instant) -> Vec<proto::BusInfo> {
        self.ifaces.iter().map(|i| bus_info(i, now)).collect()
    }

    /// Copies of the frames recorded in the last `back`, per interface.
    pub fn capture_window(&self, back: Duration, now: Instant) -> Vec<(String, Vec<RxFrame>)> {
        self.recorder.window(back, now)
    }

    /// Saves the rolling capture under the capture directory; returns a
    /// line for the log.
    pub fn save_default_capture(&self) -> String {
        let path = self.capture_dir.join(capture_name());
        match self.save_capture(&path) {
            Ok(n) => format!("{n} frames -> {}", path.display()),
            Err(e) => format!("saving {} failed: {e}", path.display()),
        }
    }

    pub fn save_capture(&self, path: &Path) -> std::io::Result<usize> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut w = std::io::BufWriter::new(std::fs::File::create(path)?);
        let n = self.recorder.dump(&mut w)?;
        std::io::Write::flush(&mut w)?;
        Ok(n)
    }
}

fn to_proto(v: Value) -> proto::Value {
    match v {
        Value::Bool(b) => proto::Value::Bool(b),
        Value::Num(n) => proto::Value::Num(n),
        Value::Text(s) => proto::Value::Text(s),
    }
}

fn bus_info(i: &Iface, now: Instant) -> proto::BusInfo {
    let ring = i
        .monitor
        .as_ref()
        .map(|m| {
            m.members(now)
                .map(|mem| proto::RingMember {
                    node: mem.last.node,
                    successor: mem.last.successor,
                    sleep_ind: mem.last.sleep_ind,
                    sleep_ack: mem.last.sleep_ack,
                    age_ms: now.saturating_duration_since(mem.at).as_millis() as u64,
                })
                .collect()
        })
        .unwrap_or_default();
    proto::BusInfo {
        role: i.spec.roles.join(","),
        iface: i.spec.name.clone(),
        state: i.monitor.as_ref().map(|m| m.state().to_string()),
        frames: i.frames,
        ring,
    }
}

fn capture_name() -> String {
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("capture-{t}.log")
}
