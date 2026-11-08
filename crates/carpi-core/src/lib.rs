//! The car model: everything the daemon knows, fed one frame at a time.
//!
//! Sans-IO apart from writing capture files. The daemon owns the sockets and
//! the clock; tests and `carpi replay` drive this directly.

pub mod bridge;
mod control;

pub use control::Output;

use carpi_can::RxFrame;
use carpi_can::recorder::Recorder;
use carpi_nm::{BusMonitor, Transition};
use carpi_proto as proto;
use carpi_state::{Store, Value};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

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

/// A bus-state change, tagged with the interface it happened on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BusEvent {
    pub iface: String,
    pub transition: Transition,
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
            .collect();
        Car {
            ifaces,
            store: Store::new(),
            recorder,
            capture_dir,
            bridge_conn: None,
            controls: control::Controls::new(&names),
            outputs: Vec::new(),
        }
    }

    pub fn ifaces(&self) -> impl Iterator<Item = &IfaceSpec> {
        self.ifaces.iter().map(|i| &i.spec)
    }

    pub fn store(&self) -> &Store {
        &self.store
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
        if iface.spec.has_role(ROLE_ICAN) && !rx.frame.extended {
            self.controls.on_ican_frame(
                rx.at,
                rx.frame.id,
                rx.frame.data(),
                &self.store,
                self.bridge_conn.is_some(),
                &mut self.outputs,
            );
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
            if let Some(t) = iface.monitor.as_mut().and_then(|m| m.tick(now)) {
                out.push(BusEvent {
                    iface: iface.spec.name.clone(),
                    transition: t,
                });
            }
        }
        let ican = self
            .ifaces
            .iter()
            .enumerate()
            .find_map(|(i, f)| f.monitor.as_ref().map(|m| (i, m)));
        self.controls.tick(
            now,
            ican,
            &mut self.store,
            self.bridge_conn.is_some(),
            &mut self.outputs,
        );
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
            | Request::Update { .. } => Response::Error {
                message: "subscriptions, commands and jobs are handled per connection".into(),
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
            Request::Values { prefix } => Response::Values {
                items: self.readings(&[prefix], now),
            },
            Request::Bus => Response::Bus {
                buses: self.ifaces.iter().map(|i| bus_info(i, now)).collect(),
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
