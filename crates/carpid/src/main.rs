//! carpid: the car daemon.
//!
//! Opens every configured interface, decodes ICAN into the value store,
//! watches the NM ring, keeps a rolling capture and answers the local API.
//! The car model decides what to send (RNS-E label/brightness, the cluster
//! display and top lines, diagnostics), which keys go to LIVI, and when to
//! power off; `act` carries that out. Sending and powering off stay off until
//! enabled in the config. Update jobs run `carpi-update` and ask LIVI to drop
//! its hotspot while they fetch. With `[portal]`, the phone portal runs too.
//! Captures go to the logs folder (`capture`), automatically on crashes and
//! bad CAN or update faults (`runmark`, `faults`); the Pi clock follows the
//! car's until network time syncs (`clock`). Data-log runs go there too
//! (`datalog`).

mod act;
mod api;
mod avsd;
mod avsd_status;
pub mod capture;
mod clock;
mod commands;
mod config;
mod datalog;
mod faults;
mod portal;
mod runmark;
mod settings;
mod update;
mod wifi;

use anyhow::Context;
use carpi_can::recorder::Recorder;
use carpi_can::{Port, ReplayPort, RxFrame};
use carpi_core::{Car, DiagJobId, DiagSubId};
use carpi_proto::{DiagAction, JobEvent, Response, SettingsAction};
use clap::Parser;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{info, warn};

#[derive(Parser)]
#[command(version, about = "pq35-carpi car daemon")]
struct Args {
    /// Config file. Without one, ICAN and diag are both on can0.
    #[arg(long, default_value = "/etc/carpi/carpid.toml")]
    config: PathBuf,
    /// Play a candump file into the ICAN interface instead of opening sockets.
    #[arg(long)]
    replay: Option<PathBuf>,
    /// With --replay: play at recorded speed rather than as fast as possible.
    #[arg(long)]
    realtime: bool,
}

type JobEvents = mpsc::UnboundedSender<JobEvent>;

#[cfg_attr(not(unix), allow(dead_code))]
pub enum Msg {
    Frame(usize, RxFrame),
    PortEnded(usize, Option<std::io::Error>),
    /// A request from API connection `conn`.
    Api(
        u64,
        carpi_proto::Request,
        oneshot::Sender<carpi_proto::Response>,
    ),
    /// Current readings under these prefixes, for a subscription; renews
    /// its diag leases (`renew`) for another 3x `period`.
    Readings {
        prefixes: Vec<String>,
        renew: Vec<DiagSubId>,
        period: Duration,
        reply: oneshot::Sender<Vec<carpi_proto::Reading>>,
    },
    /// Diag subscriptions for a new API subscription.
    DiagSubscribe(Vec<String>, Duration, oneshot::Sender<Vec<DiagSubId>>),
    DiagUnsubscribe(Vec<DiagSubId>),
    /// Start a diag job; its events go to the sender.
    DiagJob(DiagAction, JobEvents, oneshot::Sender<Result<(), String>>),
    /// Diag status, sent as one job event.
    DiagStatus(oneshot::Sender<Result<(), String>>, JobEvents),
    Settings(SettingsAction, oneshot::Sender<Response>),
    ConnClosed(u64),
    /// The ring's last `Duration` and the value store, for a capture.
    Snapshot(Duration, oneshot::Sender<capture::Snapshot>),
    /// Values from outside the car, unknown after the timeout.
    Values(Vec<(String, Option<carpi_state::Value>)>, Duration),
}

/// What the main loop owns besides the car model.
struct Daemon {
    cfg: config::Config,
    config_path: PathBuf,
    /// False when running on the built-in fallback (no file to write).
    have_file: bool,
    act: act::Actuators,
    portal_pin: Option<carpi_portal::PinCell>,
    diag_jobs: HashMap<DiagJobId, JobEvents>,
    portal_status: watch::Sender<carpi_portal::Status>,
    /// Learned light points not yet in the file, and when they changed.
    learned_dirty: Option<Instant>,
    captures: capture::Captures,
    datalog: datalog::Datalog,
    replay: bool,
    /// This run's crash marker (not in replay) and its last heartbeat.
    mark: Option<Arc<runmark::RunMark>>,
    beat_at: Instant,
    /// Next car-clock comparison; true while one is setting the clock.
    clock_at: Instant,
    clock_busy: Arc<AtomicBool>,
    /// The CAN ring's length, fixed at start.
    ring_minutes: u64,
}

/// Nudges are saved this long after the last one, so a run of key presses
/// is one write.
const LEARNED_SAVE_DELAY: Duration = Duration::from_secs(3);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args = Args::parse();
    let started = Instant::now();

    let have_file = args.config.exists();
    let cfg = if have_file {
        config::Config::load(&args.config)
            .with_context(|| format!("loading {}", args.config.display()))?
    } else {
        warn!(
            "{} not found; using can0 for ican and diag",
            args.config.display()
        );
        config::Config::fallback()
    };
    if cfg.recorder.dir.is_some() {
        warn!(
            "[recorder] dir is no longer used; captures go to [logs] dir ({})",
            cfg.logs.root().display()
        );
    }
    let replay = args.replay.is_some();
    // Not in replay: a dev run mustn't touch the car's crash marker.
    let (mark, previous) = if replay {
        (None, runmark::Previous::Clean)
    } else {
        let (m, p) = runmark::RunMark::start(&runmark::state_dir());
        (Some(Arc::new(m)), p)
    };
    let (panic_tx, mut panic_rx) = mpsc::unbounded_channel::<String>();
    if let Some(m) = &mark {
        runmark::install_panic_hook(m.path().to_path_buf(), panic_tx);
    }

    let ifaces = cfg.ifaces();
    // The ring always covers a capture.
    let ring_min = cfg.recorder.minutes.max(cfg.logs.capture_minutes);
    let recorder = Recorder::new(Duration::from_secs(ring_min * 60), 2_000_000);
    let mut car = Car::new(ifaces.clone(), recorder, config::raw_capture_dir());
    car.configure(cfg.core(), Instant::now());
    car.set_power(cfg.power_policy());
    car.set_gestures(cfg.gestures());
    let mut act = act::Actuators::new(
        &cfg,
        ifaces.iter().map(|i| i.name.clone()).collect(),
        args.replay.is_some(),
    );

    let (tx, mut rx) = mpsc::channel::<Msg>(4096);

    for (idx, spec) in ifaces.iter().enumerate() {
        let port = match &args.replay {
            Some(path) if spec.has_role(carpi_core::ROLE_ICAN) => {
                let r = ReplayPort::load(path, None, Duration::from_micros(3500), args.realtime)
                    .with_context(|| format!("loading {}", path.display()))?;
                info!(
                    "replaying {} frames from {} as {}",
                    r.len(),
                    path.display(),
                    spec.name
                );
                Port::Replay(r)
            }
            Some(_) => continue,
            None => Port::open(&spec.name).with_context(|| format!("opening {}", spec.name))?,
        };
        info!("listening on {} ({})", spec.name, spec.roles.join(","));
        tokio::spawn(read_port(idx, spec.name.clone(), port, tx.clone()));
    }

    let jobs = std::sync::Arc::new(update::Jobs::new(
        Default::default(),
        cfg.update.updater.clone(),
        args.config.clone(),
    ));
    act.set_hub(jobs.hub.clone());
    let captures = capture::Captures::new(
        capture::Setup::new(&cfg, &args.config),
        tx.clone(),
        jobs.hub.clone(),
    );
    act.set_captures(captures.clone());
    let datalog = datalog::Datalog::new(
        datalog::Setup::new(&cfg, &args.config),
        tx.clone(),
        jobs.hub.clone(),
    );
    act.set_datalog(datalog.clone());
    api::spawn(
        &cfg.api.socket,
        tx.clone(),
        jobs,
        captures.clone(),
        datalog.clone(),
    )?;
    captures.prune_now();
    match previous {
        runmark::Previous::Clean => {}
        runmark::Previous::PowerLoss => info!("the last run ended with the power; no capture"),
        runmark::Previous::Crashed { pid, end, panic } => {
            warn!("the last carpid run (pid {pid:?}) didn't stop cleanly");
            let mut note = format!(
                "carpid crashed; the journal ends where the last run (pid {}) did",
                pid.map_or("?".into(), |p| p.to_string())
            );
            if let Some(p) = panic {
                note.push_str(&format!("; {p}"));
            }
            captures.crash(capture::After { end, note });
        }
    }
    {
        let c = captures.clone();
        tokio::spawn(async move {
            while let Some(reason) = panic_rx.recv().await {
                c.auto(reason);
            }
        });
    }
    if !replay {
        faults::spawn(
            ifaces.iter().map(|i| i.name.clone()).collect(),
            captures.clone(),
            runmark::state_dir(),
        );
        avsd_status::spawn(cfg.avsd.unit.clone(), tx.clone());
    }

    let (portal_status, status_rx) = watch::channel(carpi_portal::Status {
        version: env!("CARGO_PKG_VERSION").into(),
        ..Default::default()
    });
    let portal_pin = portal::config(&cfg, &args.config, captures.root()).map(|p| {
        portal::spawn(portal::with_captures(
            portal::build(p, status_rx, started),
            captures.clone(),
        ))
    });
    if portal_pin.is_none() {
        info!("no [portal] in the config; phone portal off");
    }

    // The timezone list, read once off the loop.
    tokio::task::spawn_blocking(clock::timezones);
    let socket = cfg.api.socket.clone();
    let mut d = Daemon {
        cfg,
        config_path: args.config.clone(),
        have_file,
        act,
        portal_pin,
        diag_jobs: HashMap::new(),
        portal_status,
        learned_dirty: None,
        captures,
        datalog,
        replay,
        mark,
        beat_at: Instant::now(),
        clock_at: Instant::now() + Duration::from_secs(10),
        clock_busy: Arc::new(AtomicBool::new(false)),
        ring_minutes: ring_min,
    };

    let mut tick = tokio::time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let stop = stop_signal();
    tokio::pin!(stop);
    loop {
        // DIS and diag timers can be finer than the 100 ms tick.
        let now = Instant::now();
        let deadline = car
            .next_deadline(now)
            .unwrap_or(now + Duration::from_secs(3600));
        tokio::select! {
            msg = rx.recv() => {
                let Some(msg) = msg else { break };
                d.handle(&mut car, msg, &ifaces);
            }
            _ = tick.tick() => {
                for ev in car.tick(Instant::now()) {
                    info!(
                        "{}: bus {} -> {} ({})",
                        ev.iface, ev.transition.from, ev.transition.to, ev.detail
                    );
                }
                d.publish_status(&car);
                d.save_learned(false);
                d.housekeeping(&car);
            }
            _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
                car.service(Instant::now());
            }
            _ = &mut stop => {
                info!("stopping");
                break;
            }
        }
        d.flush(&mut car);
    }
    d.datalog.shutdown().await;
    api::cleanup(&socket);
    if let Some(m) = &d.mark {
        m.clean();
    }
    Ok(())
}

/// Ctrl-C, or SIGTERM from systemd.
async fn stop_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut term) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

impl Daemon {
    fn handle(&mut self, car: &mut Car, msg: Msg, ifaces: &[carpi_core::IfaceSpec]) {
        let now = Instant::now();
        match msg {
            Msg::Frame(idx, frame) => {
                if let Some(ev) = car.ingest(idx, &frame) {
                    info!(
                        "{}: bus {} -> {} ({})",
                        ev.iface, ev.transition.from, ev.transition.to, ev.detail
                    );
                }
            }
            Msg::PortEnded(idx, err) => {
                let name = &ifaces[idx].name;
                match err {
                    Some(e) => {
                        warn!("{name}: read failed: {e}");
                        if !self.replay {
                            self.captures.auto(format!("error:{name}-gone"));
                        }
                    }
                    None => info!("{name}: replay finished"),
                }
            }
            Msg::Api(conn, req, reply) => {
                let _ = reply.send(car.handle(conn, req, now));
            }
            Msg::Readings {
                prefixes,
                renew,
                period,
                reply,
            } => {
                car.diag_renew(&renew, period, now);
                let _ = reply.send(car.readings(&prefixes, now));
            }
            Msg::DiagSubscribe(prefixes, period, reply) => {
                let _ = reply.send(car.diag_subscribe(&prefixes, period, now));
            }
            Msg::DiagUnsubscribe(ids) => car.diag_unsubscribe(&ids, now),
            Msg::DiagJob(action, events, reply) => match car.diag_start(action, now) {
                Ok(id) => {
                    self.diag_jobs.insert(id, events);
                    let _ = reply.send(Ok(()));
                }
                Err(e) => {
                    let _ = reply.send(Err(e));
                }
            },
            Msg::DiagStatus(reply, events) => match car.diag_status(now) {
                Some(st) => {
                    let _ = events.send(JobEvent::DiagStatus(st));
                    let _ = events.send(JobEvent::Finished {
                        ok: true,
                        message: "status".into(),
                    });
                    let _ = reply.send(Ok(()));
                }
                None => {
                    let _ = reply.send(Err("no interface has the diag role".into()));
                }
            },
            Msg::Settings(action, reply) => {
                let (mut resp, tz) = self.settings(car, action, now);
                match tz {
                    None => {
                        let _ = reply.send(resp);
                    }
                    // timedatectl runs off the loop; the reply waits for it.
                    Some(cmd) => {
                        tokio::spawn(async move {
                            match clock::run(cmd).await {
                                Ok(()) => {
                                    if let Response::Settings { settings, .. } = &mut resp {
                                        settings.clock.timezone = clock::timezone();
                                    }
                                }
                                Err(e) => {
                                    warn!("timezone: {e}");
                                    resp = Response::Error {
                                        message: format!(
                                            "settings saved, but the timezone wasn't set: {e}"
                                        ),
                                    };
                                }
                            }
                            let _ = reply.send(resp);
                        });
                    }
                }
            }
            Msg::ConnClosed(conn) => car.conn_closed(conn, now),
            Msg::Values(values, timeout) => car.set_values(values, timeout, now),
            Msg::Snapshot(back, reply) => {
                let _ = reply.send(capture::Snapshot::take(car, back, now));
            }
        }
    }

    /// Heartbeat for the crash marker, and the car clock. Both run off the
    /// loop.
    fn housekeeping(&mut self, car: &Car) {
        let now = Instant::now();
        if let Some(m) = &self.mark
            && now.duration_since(self.beat_at) >= runmark::BEAT
        {
            self.beat_at = now;
            let m = m.clone();
            tokio::task::spawn_blocking(move || m.beat());
        }
        if self.replay || !self.cfg.clock.set_from_car || now < self.clock_at {
            return;
        }
        self.clock_at = now + clock::CHECK_EVERY;
        if self.clock_busy.load(Ordering::Relaxed) {
            return;
        }
        let Some(carpi_proto::Reading {
            value: Some(carpi_proto::Value::Text(dt)),
            age_ms,
            ..
        }) = car
            .readings(&["clock.datetime".to_string()], now)
            .into_iter()
            .find(|r| r.name == "clock.datetime")
        else {
            return;
        };
        let Some(car_t) = clock::car_now(&dt, age_ms) else {
            return;
        };
        let pi = clock::local_now().naive;
        let Some(target) = clock::needs_set(pi, car_t) else {
            return;
        };
        let busy = self.clock_busy.clone();
        busy.store(true, Ordering::Relaxed);
        tokio::spawn(async move {
            let r = tokio::task::spawn_blocking(move || clock::set_from_car(target, pi)).await;
            // Synced or failing: look again in 10 minutes, not every minute.
            let wait = match r {
                Ok(Ok(Some(changed))) => {
                    warn!("clock: {changed}");
                    Duration::ZERO
                }
                Ok(Ok(None)) => {
                    info!("clock: car and Pi differ, but network time is synced; leaving it");
                    Duration::from_secs(600)
                }
                Ok(Err(e)) => {
                    warn!("clock: {e}");
                    Duration::from_secs(600)
                }
                Err(e) => {
                    warn!("clock: {e}");
                    Duration::from_secs(600)
                }
            };
            tokio::time::sleep(wait).await;
            busy.store(false, Ordering::Relaxed);
        });
    }

    fn current(&self) -> carpi_proto::Settings {
        let mut s = settings::get(&self.cfg);
        settings::system(&mut s, &self.cfg);
        s
    }

    /// The reply, and a timezone command to run before sending it.
    fn settings(
        &mut self,
        car: &mut Car,
        action: SettingsAction,
        now: Instant,
    ) -> (Response, Option<Vec<String>>) {
        let patch = match action {
            SettingsAction::Get => {
                let r = Response::Settings {
                    settings: self.current(),
                    restart_needed: Vec::new(),
                };
                return (r, None);
            }
            SettingsAction::Set { patch } => *patch,
        };
        match self.set(car, &patch, now) {
            Ok(r) => r,
            Err(message) => (Response::Error { message }, None),
        }
    }

    fn set(
        &mut self,
        car: &mut Car,
        patch: &carpi_proto::SettingsPatch,
        now: Instant,
    ) -> Result<(Response, Option<Vec<String>>), String> {
        // Checked before anything is saved.
        let tz = match patch.clock.as_ref().and_then(|c| c.timezone.as_deref()) {
            Some(tz) if tz != clock::timezone() => {
                Some(clock::set_timezone_command(tz, clock::timezones())?)
            }
            _ => None,
        };
        let avsd_env = config::expand_home(&self.cfg.avsd.env);
        let avsd = patch.avsd.as_ref();
        if let Some(a) = avsd.filter(|a| !a.values.is_empty()) {
            let text = std::fs::read_to_string(&avsd_env)
                .map_err(|e| format!("can't read {}: {e}", avsd_env.display()))?;
            avsd::edit(&text, &a.values)?;
        }
        // The file must have the latest learned points before it's patched.
        self.save_learned(true);
        if !self.have_file {
            return Err(format!(
                "no config file at {}; settings can't be saved",
                self.config_path.display()
            ));
        }
        let new = settings::set(&self.config_path, patch)?;
        car.configure(new.core(), now);
        car.set_power(new.power_policy());
        car.set_gestures(new.gestures());
        if new.tx.enabled != self.cfg.tx.enabled {
            self.act.set_tx_enabled(new.tx.enabled);
        }
        self.act.set_shutdown_enabled(new.power.shutdown);
        self.captures
            .set_limits(new.logs.capture_minutes, new.logs.max_pct);
        self.datalog.set_max_pct(new.logs.max_pct);
        if let Some(cell) = &self.portal_pin {
            *cell.write().unwrap_or_else(|e| e.into_inner()) =
                new.portal.as_ref().and_then(|p| p.pin.clone());
        }
        info!("settings saved to {}", self.config_path.display());
        let mut restart_needed = Vec::new();
        // The portal starts and stops with carpid, and reads its config once.
        if new.portal.is_some() != self.portal_pin.is_some() {
            restart_needed.push("portal".into());
        }
        let protect = |c: &config::Config| c.portal.as_ref().map(|p| p.protect_downloads);
        if self.portal_pin.is_some() && protect(&new) != protect(&self.cfg) {
            restart_needed.push("portal.protect_downloads".into());
        }
        // The keyboard is opened at start.
        if new.input.uinput != self.cfg.input.uinput {
            restart_needed.push("input.uinput".into());
        }
        // The CAN ring is sized at start.
        if new.logs.capture_minutes > self.ring_minutes {
            restart_needed.push("logs.capture_minutes".into());
        }
        // glibc reads the timezone once.
        if tz.is_some() {
            restart_needed.push("clock.timezone".into());
        }
        self.cfg = new;
        if let Some(a) = avsd {
            let changed = if a.values.is_empty() {
                false
            } else {
                // Checked above; it can only fail on the write now.
                match avsd::save(&avsd_env, &a.values) {
                    Ok(c) => c,
                    Err(e) => return Err(format!("carpid.toml saved, but {e}")),
                }
            };
            if changed || a.restart {
                avsd::restart(self.cfg.avsd.unit.clone());
            }
        }
        let r = Response::Settings {
            settings: self.current(),
            restart_needed,
        };
        Ok((r, tz))
    }

    /// Writes learned light points once they've settled (`now`: at once).
    fn save_learned(&mut self, now: bool) {
        let Some(at) = self.learned_dirty else {
            return;
        };
        if !now && at.elapsed() < LEARNED_SAVE_DELAY {
            return;
        }
        self.learned_dirty = None;
        if !self.have_file {
            warn!("no config file; learned brightness points are kept until restart only");
            return;
        }
        match settings::save_learned(&self.config_path, &self.cfg.light.learned) {
            Ok(c) => {
                self.cfg = c;
                info!("learned brightness points saved");
            }
            Err(e) => warn!("saving learned brightness points: {e}"),
        }
    }

    /// Outputs to the actuators, diag job events to their connections.
    fn flush(&mut self, car: &mut Car) {
        for o in car.drain_outputs() {
            if let carpi_core::Output::LightLearned(points) = &o {
                self.cfg.light.learned = points
                    .iter()
                    .map(|&(s, ambient, pct)| carpi_proto::LearnedPoint {
                        source: config::to_proto_source(s),
                        ambient,
                        pct,
                    })
                    .collect();
                self.learned_dirty = Some(Instant::now());
                continue;
            }
            self.act.act(o, car);
        }
        for (id, events) in car.drain_diag_jobs() {
            let done = events
                .iter()
                .any(|e| matches!(e, JobEvent::Finished { .. }));
            if let Some(tx) = self.diag_jobs.get(&id) {
                for e in events {
                    let _ = tx.send(e);
                }
            }
            if done {
                self.diag_jobs.remove(&id);
            }
        }
    }

    fn publish_status(&self, car: &Car) {
        let bus = car
            .ican()
            .map(|m| m.state().to_string())
            .unwrap_or_else(|| "no ICAN".into());
        let now = Instant::now();
        let text = |n: &str| match car.readings(&[n.to_string()], now).pop() {
            Some(carpi_proto::Reading {
                value: Some(carpi_proto::Value::Text(t)),
                ..
            }) => t,
            _ => "-".into(),
        };
        let extra = vec![
            (
                "TX".into(),
                if self.cfg.tx.enabled { "on" } else { "off" }.into(),
            ),
            ("DIS".into(), text("dis.state")),
            ("Diag".into(), text("diag.mode")),
        ];
        self.portal_status.send_if_modified(|s| {
            let changed = s.bus != bus || s.extra != extra;
            s.bus = bus;
            s.extra = extra;
            changed
        });
    }
}

/// Reads one interface forever. On a read error it waits and reopens the
/// interface by name, so a bus-off or a link bounce doesn't kill the daemon.
/// (Replay ports never error.)
async fn read_port(idx: usize, name: String, mut port: Port, tx: mpsc::Sender<Msg>) {
    loop {
        match port.recv().await {
            Ok(Some(frame)) => {
                if tx.send(Msg::Frame(idx, frame)).await.is_err() {
                    return;
                }
            }
            Ok(None) => {
                let _ = tx.send(Msg::PortEnded(idx, None)).await;
                return;
            }
            Err(e) => {
                let _ = tx.send(Msg::PortEnded(idx, Some(e))).await;
                loop {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    if let Ok(p) = Port::open(&name) {
                        port = p;
                        break;
                    }
                }
            }
        }
    }
}
