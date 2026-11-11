//! Data logging (docs/PAGES.md 4): profiles, and runs into the logs folder.
//!
//! ```text
//! entry.json    carpi_logs::Meta, kind datalog, trigger = the profile's name
//! data.csv      Time (s), a column per value (the profile's label), Mark;
//!               a row per tick, each value held until the next sample
//! profile.json  the profile as it ran, and what it cost
//! info.txt      start and end, Pi time, car clock, odometer, versions
//! ```
//!
//! A mark (`DatalogAction::Mark`) puts its number (1, 2, 3...) in the next
//! row's `Mark` cell and toasts "Mark N".
//!
//! Profiles are kept in `datalog.toml` next to carpid's config. One run at a
//! time. The run's task asks the event loop for its values once a tick
//! (`Msg::Readings`, like a subscription, which also renews the diag leases
//! that keep its diag values polled) and hands each row to a writer thread.
//! The writer flushes and syncs every second, so a power cut loses about a
//! second.

// The API that drives it is Unix only.
#![cfg_attr(not(unix), allow(dead_code))]

use crate::Msg;
use crate::capture;
use crate::clock;
use crate::commands::CommandHub;
use carpi_diag::{Catalog, Provider};
use carpi_logs::{Kind, Logs, Meta};
use carpi_proto::{
    Command, DATALOG_RATES, DatalogAction, DatalogLoad, DatalogProfile, DatalogProfileStatus,
    DatalogRun, DatalogStatus, Reading, Response, ToastLevel, Value,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::{mpsc, oneshot};
use tracing::{info, warn};

/// The profiles file, next to carpid's config.
pub const FILE: &str = "datalog.toml";
/// Rows reach the card at least this often.
const FLUSH: Duration = Duration::from_secs(1);
const MAX_VALUES: usize = 200;
const MAX_NAME: usize = 32;
/// Longest column label, in characters.
const MAX_LABEL: usize = 64;
/// Read for info.txt at the start and the end.
const CLOCK: &str = "clock.datetime";
const ODOMETER: &str = "vehicle.odometer_km";

#[derive(Clone, Debug)]
pub struct Setup {
    /// The logs folder.
    pub root: PathBuf,
    pub max_pct: u8,
    /// `datalog.toml`.
    pub profiles: PathBuf,
    /// The release manifest, for versions.
    pub manifest: PathBuf,
}

impl Setup {
    pub fn new(cfg: &crate::config::Config, config_path: &Path) -> Self {
        Setup {
            root: cfg.logs.root(),
            max_pct: cfg.logs.max_pct,
            profiles: config_path.with_file_name(FILE),
            manifest: PathBuf::from("/opt/carpi/current/manifest.json"),
        }
    }
}

/// `datalog.toml`.
#[derive(Debug, Default, Serialize, Deserialize)]
struct ProfilesFile {
    /// The profile a key press starts.
    #[serde(default)]
    selected: Option<String>,
    #[serde(default, rename = "profile")]
    profiles: Vec<DatalogProfile>,
}

/// What there is before anything is saved.
fn defaults() -> Vec<DatalogProfile> {
    vec![DatalogProfile {
        name: "Boost".into(),
        values: [
            "engine.rpm",
            "engine.load_pct",
            "engine.throttle_pct",
            "engine.pedal_pct",
            "engine.boost.actual",
            "engine.boost.specified",
            "engine.n75_duty_pct",
            "engine.ignition_timing_deg",
            "engine.intake_temp_c",
            "vehicle.speed_kmh",
        ]
        .map(String::from)
        .to_vec(),
        rate_hz: 10,
        labels: Vec::new(),
    }]
}

/// Profiles from `path`: the defaults when there's no file. A file that
/// doesn't parse is reported and set aside on the next save.
fn read_profiles(path: &Path) -> (ProfilesFile, bool) {
    match std::fs::read_to_string(path) {
        Ok(text) => match toml::from_str::<ProfilesFile>(&text) {
            Ok(mut f) => {
                f.profiles = f
                    .profiles
                    .into_iter()
                    .filter_map(|p| match check(p) {
                        Ok(p) => Some(p),
                        Err(e) => {
                            warn!("{}: profile skipped: {e}", path.display());
                            None
                        }
                    })
                    .collect();
                (f, false)
            }
            Err(e) => {
                warn!("{}: {e}; using the default profiles", path.display());
                (
                    ProfilesFile {
                        selected: None,
                        profiles: defaults(),
                    },
                    true,
                )
            }
        },
        Err(_) => (
            ProfilesFile {
                selected: None,
                profiles: defaults(),
            },
            false,
        ),
    }
}

fn write_profiles(path: &Path, f: &ProfilesFile, broken: bool) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if broken && path.exists() {
        std::fs::rename(path, path.with_extension("toml.bad"))?;
    }
    let text = toml::to_string(f).map_err(io::Error::other)?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, path)
}

/// A profile as carpid keeps it: trimmed name, a known rate, plain value
/// names without repeats, a one-line label (or none) per value.
pub fn check(mut p: DatalogProfile) -> Result<DatalogProfile, String> {
    p.name = p.name.trim().to_string();
    if p.name.is_empty() || p.name.chars().count() > MAX_NAME {
        return Err(format!("a profile name is 1 to {MAX_NAME} characters"));
    }
    if p.name.chars().any(char::is_control) {
        return Err("a profile name is one plain line".into());
    }
    if !DATALOG_RATES.contains(&p.rate_hz) {
        return Err(format!(
            "rate {} Hz: use one of {}",
            p.rate_hz,
            DATALOG_RATES.map(|r| r.to_string()).join(", ")
        ));
    }
    let mut seen = BTreeSet::new();
    let mut labels = std::mem::take(&mut p.labels).into_iter();
    let (values, labels): (Vec<String>, Vec<String>) = std::mem::take(&mut p.values)
        .into_iter()
        .map(|v| {
            let l = labels.next().unwrap_or_default();
            let l: String = l
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect();
            (v, l.trim().chars().take(MAX_LABEL).collect())
        })
        .filter(|(v, _)| seen.insert(v.clone()))
        .unzip();
    p.values = values;
    // No labels at all stays an empty list.
    p.labels = if labels.iter().all(String::is_empty) {
        Vec::new()
    } else {
        labels
    };
    if p.values.len() > MAX_VALUES {
        return Err(format!("at most {MAX_VALUES} values"));
    }
    if let Some(bad) = p.values.iter().find(|v| !value_name(v)) {
        return Err(format!("{bad:?} isn't a value name"));
    }
    Ok(p)
}

fn value_name(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 96
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// What logging `values` at `rate_hz` costs: diag values are packed into
/// measuring-block reads the way the scheduler does it (most values per read
/// first), and each module's reads share its channel.
pub fn load(catalog: &Catalog, values: &[String], rate_hz: u32) -> DatalogLoad {
    let cfg = carpi_diag::Config::default();
    let mut bus = 0;
    let mut need: Vec<Vec<(u8, u8, usize)>> = Vec::new();
    for v in values {
        // A module-group name is one measuring-block read of its own.
        if let Some(f) = carpi_diag::GroupField::parse(v) {
            match carpi_diag::modules::default_tp2(f.address) {
                Some(m) if !cfg.profiles.is_denied(m) => need.push(vec![(m, f.group, 0)]),
                _ => bus += 1,
            }
            continue;
        }
        let Some(def) = catalog.get(v).filter(|d| d.ican_name().is_none()) else {
            bus += 1;
            continue;
        };
        let reads: Vec<_> = def
            .providers
            .iter()
            .enumerate()
            .filter_map(|(rank, p)| match *p {
                Provider::Diag { module, group, .. } if !cfg.profiles.is_denied(module) => {
                    Some((module, group, rank))
                }
                _ => None,
            })
            .collect();
        if !reads.is_empty() {
            need.push(reads);
        }
    }
    let diag_values = need.len() as u32;
    let mut groups = BTreeSet::new();
    while !need.is_empty() {
        let mut score: BTreeMap<(u8, u8), (usize, usize)> = BTreeMap::new();
        for reads in &need {
            for &(m, g, rank) in reads {
                let e = score.entry((m, g)).or_insert((0, usize::MAX));
                e.0 += 1;
                e.1 = e.1.min(rank);
            }
        }
        let best = *score
            .iter()
            .max_by(|a, b| a.1.0.cmp(&b.1.0).then(b.1.1.cmp(&a.1.1)).then(b.0.cmp(a.0)))
            .expect("need is non-empty")
            .0;
        need.retain(|reads| !reads.iter().any(|r| (r.0, r.1) == best));
        groups.insert(best);
    }
    let mut per_module: BTreeMap<u8, u32> = BTreeMap::new();
    for (m, _) in &groups {
        *per_module.entry(*m).or_default() += 1;
    }
    let diag_hz = per_module.values().max().map(|&n| {
        let fits = cfg.utilisation / (n as f64 * cfg.default_cost.as_secs_f64());
        let hz = fits
            .min(rate_hz as f64)
            .min(1.0 / cfg.min_period.as_secs_f64());
        (hz * 10.0).floor() / 10.0
    });
    DatalogLoad {
        bus_values: bus,
        diag_values,
        diag_groups: groups.len() as u32,
        diag_modules: per_module.len() as u32,
        diag_hz,
    }
}

/// The CSV's header line: `Time (s)`, each value's label (its name when the
/// label is missing or empty), `Mark`.
pub fn header(p: &DatalogProfile) -> String {
    let mut s = String::from("Time (s)");
    for (i, n) in p.values.iter().enumerate() {
        let l = p.labels.get(i).filter(|l| !l.is_empty()).unwrap_or(n);
        s.push(',');
        s.push_str(&field(l));
    }
    s.push_str(",Mark\n");
    s
}

/// One row: the time in ms as seconds, then each value, blank when there
/// is none, then the mark's number if one was made.
pub fn line(ms: u64, values: &[Option<Value>], mark: Option<u32>) -> String {
    let mut s = format!("{}.{:03}", ms / 1000, ms % 1000);
    for v in values {
        s.push(',');
        match v {
            None => {}
            Some(Value::Bool(b)) => s.push(if *b { '1' } else { '0' }),
            Some(Value::Num(n)) => s.push_str(&number(*n)),
            Some(Value::Text(t)) => s.push_str(&field(t)),
        }
    }
    s.push(',');
    if let Some(m) = mark {
        s.push_str(&m.to_string());
    }
    s.push('\n');
    s
}

/// Values for the next row: what came in, else the last value seen, so a
/// cell is blank only before a value's first sample.
pub fn hold(last: &mut [Option<Value>], now: Vec<Option<Value>>) {
    for (l, v) in last.iter_mut().zip(now) {
        if v.is_some() {
            *l = v;
        }
    }
}

/// At most 6 decimals, no trailing zeros; blank for NaN and infinities.
fn number(n: f64) -> String {
    if !n.is_finite() {
        return String::new();
    }
    if n.fract() == 0.0 && n.abs() < 1e15 {
        return format!("{n:.0}");
    }
    let s = format!("{n:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

/// One line, quoted when it has to be.
fn field(s: &str) -> String {
    let s: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if s.contains([',', '"']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

/// The tick a row belongs to, as ms since the start: rows sit on the
/// profile's grid even when a tick runs late.
pub fn tick_ms(since_start: Duration, rate_hz: u32) -> u64 {
    let period = 1000 / rate_hz.max(1) as u64;
    let n = (since_start.as_millis() as u64 + period / 2) / period;
    n * period
}

pub struct Row {
    pub ms: u64,
    pub values: Vec<Option<Value>>,
    pub mark: Option<u32>,
}

/// The writer thread: rows into `out`, flushed and synced every `flush`, and
/// at the end. Counts what it wrote into `rows`.
pub fn write_rows(
    out: std::fs::File,
    rx: std::sync::mpsc::Receiver<Row>,
    rows: &AtomicU64,
    flush: Duration,
) -> io::Result<()> {
    use std::sync::mpsc::RecvTimeoutError;
    let mut w = io::BufWriter::new(out);
    let mut flushed = Instant::now();
    loop {
        let done = match rx.recv_timeout(flush) {
            Ok(row) => {
                w.write_all(line(row.ms, &row.values, row.mark).as_bytes())?;
                rows.fetch_add(1, Ordering::Relaxed);
                false
            }
            Err(RecvTimeoutError::Timeout) => false,
            Err(RecvTimeoutError::Disconnected) => true,
        };
        if done || flushed.elapsed() >= flush {
            w.flush()?;
            w.get_ref().sync_data()?;
            flushed = Instant::now();
        }
        if done {
            return Ok(());
        }
    }
}

#[derive(Clone)]
pub struct Datalog {
    inner: Arc<Inner>,
}

struct Inner {
    setup: Setup,
    /// `[logs] max_pct`, live from the settings.
    max_pct: Mutex<u8>,
    tx: mpsc::Sender<Msg>,
    hub: Arc<CommandHub>,
    catalog: Catalog,
    state: Mutex<State>,
    /// Start and stop one at a time.
    busy: tokio::sync::Mutex<()>,
    started: Instant,
}

struct State {
    profiles: Vec<DatalogProfile>,
    selected: Option<String>,
    /// The file didn't parse: set it aside before writing.
    broken: bool,
    run: Option<Live>,
    last: Option<DatalogRun>,
}

struct Live {
    run: DatalogRun,
    t0: Instant,
    rows: Arc<AtomicU64>,
    /// Marks made, and the one the next row carries (0: none).
    marks: Arc<Marks>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<DatalogRun>>,
}

impl Live {
    fn now(&self) -> DatalogRun {
        let mut r = self.run.clone();
        r.elapsed_ms = self.t0.elapsed().as_millis() as u64;
        r.rows = self.rows.load(Ordering::Relaxed);
        r.marks = self.marks.made.load(Ordering::Relaxed);
        r
    }
}

/// A run's marks, between the API and the run's task.
#[derive(Default)]
struct Marks {
    made: AtomicU32,
    /// For the next row; 0 when there's none.
    next: AtomicU32,
}

/// What a run's task needs.
struct Plan {
    meta: Meta,
    dir: PathBuf,
    profile: DatalogProfile,
    load: DatalogLoad,
    ids: Vec<carpi_core::DiagSubId>,
    rows_tx: std::sync::mpsc::Sender<Row>,
    writer: std::thread::JoinHandle<io::Result<()>>,
    rows: Arc<AtomicU64>,
    marks: Arc<Marks>,
    stop: oneshot::Receiver<()>,
    t0: Instant,
    start_values: Vec<Reading>,
    start_time: SystemTime,
}

impl Datalog {
    /// `[logs] max_pct`, live.
    pub fn set_max_pct(&self, pct: u8) {
        *self.inner.max_pct.lock().unwrap_or_else(|e| e.into_inner()) = pct;
    }

    fn setup(&self) -> Setup {
        Setup {
            max_pct: *self.inner.max_pct.lock().unwrap_or_else(|e| e.into_inner()),
            ..self.inner.setup.clone()
        }
    }

    pub fn new(setup: Setup, tx: mpsc::Sender<Msg>, hub: Arc<CommandHub>) -> Self {
        let (file, broken) = read_profiles(&setup.profiles);
        Datalog {
            inner: Arc::new(Inner {
                max_pct: Mutex::new(setup.max_pct),
                setup,
                tx,
                hub,
                catalog: Catalog::seed(),
                state: Mutex::new(State {
                    profiles: file.profiles,
                    selected: file.selected,
                    broken,
                    run: None,
                    last: None,
                }),
                busy: tokio::sync::Mutex::new(()),
                started: Instant::now(),
            }),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.inner.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Answers `Request::Datalog`.
    pub async fn handle(&self, action: DatalogAction) -> Response {
        let r = match action {
            DatalogAction::Status => Ok(()),
            DatalogAction::Start { profile } => self.start(&profile).await.map(|_| ()),
            DatalogAction::Stop => self.stop().await.map(|_| ()),
            DatalogAction::Save { profile, replaces } => self.save(profile, replaces),
            DatalogAction::Delete { name } => self.delete(&name),
            DatalogAction::Mark => self.mark().map(|_| ()),
        };
        match r {
            Ok(()) => Response::Datalog {
                status: Box::new(self.status()),
            },
            Err(message) => Response::Error { message },
        }
    }

    pub fn status(&self) -> DatalogStatus {
        let st = self.state();
        DatalogStatus {
            profiles: st
                .profiles
                .iter()
                .map(|p| DatalogProfileStatus {
                    load: load(&self.inner.catalog, &p.values, p.rate_hz),
                    profile: p.clone(),
                })
                .collect(),
            selected: st.selected.clone(),
            running: st.run.as_ref().map(Live::now),
            last: st.last.clone(),
        }
    }

    /// The key binding: stops the run, or starts the selected profile (the
    /// first one if none is).
    pub fn toggle(&self) {
        let d = self.clone();
        tokio::spawn(async move {
            let running = d.state().run.is_some();
            let r = if running {
                d.stop().await.map(|_| ())
            } else {
                let name = {
                    let st = d.state();
                    st.selected
                        .clone()
                        .filter(|s| st.profiles.iter().any(|p| p.name == *s))
                        .or_else(|| st.profiles.first().map(|p| p.name.clone()))
                };
                match name {
                    Some(n) => d.start(&n).await.map(|_| ()),
                    None => Err("no data-log profiles".into()),
                }
            };
            if let Err(e) = r {
                d.toast(format!("Logging: {e}"), ToastLevel::Error);
            }
        });
    }

    /// Marks the next row of the run; its number.
    pub fn mark(&self) -> Result<u32, String> {
        let n = {
            let st = self.state();
            let Some(live) = &st.run else {
                return Err("not logging".into());
            };
            let n = live.marks.made.fetch_add(1, Ordering::Relaxed) + 1;
            live.marks.next.store(n, Ordering::Relaxed);
            n
        };
        self.toast(format!("Mark {n}"), ToastLevel::Info);
        Ok(n)
    }

    /// Stops a run before carpid exits, so it gets its end.
    pub async fn shutdown(&self) {
        if self.state().run.is_some() {
            let _ = tokio::time::timeout(Duration::from_secs(5), self.stop()).await;
        }
    }

    fn save(&self, profile: DatalogProfile, replaces: Option<String>) -> Result<(), String> {
        let p = check(profile)?;
        let mut st = self.state();
        let at = replaces
            .as_deref()
            .and_then(|r| st.profiles.iter().position(|x| x.name == r))
            .or_else(|| st.profiles.iter().position(|x| x.name == p.name));
        if st
            .profiles
            .iter()
            .enumerate()
            .any(|(i, x)| x.name == p.name && Some(i) != at)
        {
            return Err(format!("there's already a profile called {}", p.name));
        }
        let mut profiles = st.profiles.clone();
        let mut selected = st.selected.clone();
        match at {
            Some(i) => {
                if selected.as_deref() == Some(profiles[i].name.as_str()) {
                    selected = Some(p.name.clone());
                }
                profiles[i] = p;
            }
            None => profiles.push(p),
        }
        self.write(&mut st, profiles, selected)
    }

    fn delete(&self, name: &str) -> Result<(), String> {
        let mut st = self.state();
        let mut profiles = st.profiles.clone();
        let before = profiles.len();
        profiles.retain(|p| p.name != name);
        if profiles.len() == before {
            return Err(format!("no profile called {name}"));
        }
        let selected = st.selected.clone().filter(|s| s != name);
        self.write(&mut st, profiles, selected)
    }

    /// Saves the file, then takes the new list.
    fn write(
        &self,
        st: &mut State,
        profiles: Vec<DatalogProfile>,
        selected: Option<String>,
    ) -> Result<(), String> {
        let f = ProfilesFile { selected, profiles };
        let path = &self.inner.setup.profiles;
        write_profiles(path, &f, st.broken)
            .map_err(|e| format!("saving {}: {e}", path.display()))?;
        st.broken = false;
        st.profiles = f.profiles;
        st.selected = f.selected;
        Ok(())
    }

    /// Current readings of `names`, from the event loop.
    async fn readings(&self, names: &[&str]) -> Vec<Reading> {
        let (rtx, rrx) = oneshot::channel();
        let msg = Msg::Readings {
            prefixes: names.iter().map(|n| n.to_string()).collect(),
            renew: Vec::new(),
            period: Duration::from_secs(1),
            reply: rtx,
        };
        let ask = async {
            self.inner.tx.send(msg).await.ok()?;
            rrx.await.ok()
        };
        tokio::time::timeout(Duration::from_secs(1), ask)
            .await
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    async fn start(&self, name: &str) -> Result<DatalogRun, String> {
        let _busy = self.inner.busy.lock().await;
        let profile = {
            let st = self.state();
            if let Some(l) = &st.run {
                return Err(format!("already logging {}", l.run.label));
            }
            st.profiles
                .iter()
                .find(|p| p.name == name)
                .cloned()
                .ok_or_else(|| format!("no profile called {name}"))?
        };
        if profile.values.is_empty() {
            return Err(format!("{} has no values to log", profile.name));
        }
        let load = load(&self.inner.catalog, &profile.values, profile.rate_hz);
        let start_values = self.readings(&[CLOCK, ODOMETER]).await;
        let start_time = SystemTime::now();

        let setup = self.setup();
        let p = profile.clone();
        let l = load.clone();
        let (meta, dir, file) = tokio::task::spawn_blocking(move || open(&setup, &p, &l))
            .await
            .map_err(|e| format!("datalog task: {e}"))??;

        let period = period(profile.rate_hz);
        let (rtx, rrx) = oneshot::channel();
        let ids = match self
            .inner
            .tx
            .send(Msg::DiagSubscribe(profile.values.clone(), period, rtx))
            .await
        {
            Ok(()) => rrx.await.unwrap_or_default(),
            Err(_) => Vec::new(),
        };

        let rows = Arc::new(AtomicU64::new(0));
        let (rows_tx, rows_rx) = std::sync::mpsc::channel();
        let counter = rows.clone();
        let writer = std::thread::Builder::new()
            .name("datalog".into())
            .spawn(move || write_rows(file, rows_rx, &counter, FLUSH))
            .map_err(|e| format!("datalog thread: {e}"))?;
        let (stop_tx, stop_rx) = oneshot::channel();
        let t0 = Instant::now();
        let run = DatalogRun {
            label: meta.id.to_string(),
            date: meta.id.date.clone(),
            number: meta.id.number,
            profile: profile.name.clone(),
            values: profile.values.len() as u32,
            rate_hz: profile.rate_hz,
            started: meta.started.clone(),
            elapsed_ms: 0,
            rows: 0,
            dir: dir.display().to_string(),
            summary: String::new(),
            error: None,
            marks: 0,
        };
        let marks = Arc::new(Marks::default());
        let plan = Plan {
            meta,
            dir,
            profile,
            load,
            ids,
            rows_tx,
            writer,
            rows: rows.clone(),
            marks: marks.clone(),
            stop: stop_rx,
            t0,
            start_values,
            start_time,
        };
        let task = tokio::spawn(self.clone().run(plan));
        let selected = {
            let mut st = self.state();
            st.run = Some(Live {
                run: run.clone(),
                t0,
                rows,
                marks,
                stop: Some(stop_tx),
                task: Some(task),
            });
            (st.selected.as_deref() != Some(name)).then(|| {
                st.selected = Some(name.to_string());
                ProfilesFile {
                    selected: st.selected.clone(),
                    profiles: st.profiles.clone(),
                }
            })
        };
        if let Some(f) = selected {
            let path = self.inner.setup.profiles.clone();
            let broken = self.state().broken;
            if let Err(e) = write_profiles(&path, &f, broken) {
                warn!("{}: {e}", path.display());
            } else {
                self.state().broken = false;
            }
        }
        info!(
            "logging {} ({}, {} values, {} Hz)",
            run.label, run.profile, run.values, run.rate_hz
        );
        self.toast(format!("Logging {}", run.label), ToastLevel::Info);
        Ok(run)
    }

    async fn stop(&self) -> Result<DatalogRun, String> {
        let _busy = self.inner.busy.lock().await;
        let (stop, task) = {
            let mut st = self.state();
            let Some(live) = st.run.as_mut() else {
                return Err("not logging".into());
            };
            (live.stop.take(), live.task.take())
        };
        if let Some(s) = stop {
            let _ = s.send(());
        }
        match task {
            Some(t) => t.await.map_err(|e| format!("datalog task: {e}")),
            None => Err("the run is already stopping".into()),
        }
    }

    /// One run, start to end.
    async fn run(self, mut plan: Plan) -> DatalogRun {
        let names = plan.profile.values.clone();
        let rate = plan.profile.rate_hz;
        let period = period(rate);
        let mut tick = tokio::time::interval_at(tokio::time::Instant::from_std(plan.t0), period);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut error = None;
        let mut last: Vec<Option<Value>> = vec![None; names.len()];
        loop {
            let at = tokio::select! {
                _ = &mut plan.stop => break,
                at = tick.tick() => at,
            };
            let (rtx, rrx) = oneshot::channel();
            let msg = Msg::Readings {
                prefixes: names.clone(),
                renew: plan.ids.clone(),
                period,
                reply: rtx,
            };
            let items = match self.inner.tx.send(msg).await {
                Ok(()) => rrx.await.ok(),
                Err(_) => None,
            };
            let Some(items) = items else {
                error = Some("carpid's event loop stopped".to_string());
                break;
            };
            let now = names
                .iter()
                .map(|n| {
                    items
                        .iter()
                        .find(|r| r.name == *n)
                        .and_then(|r| r.value.clone())
                })
                .collect();
            hold(&mut last, now);
            let mark = Some(plan.marks.next.swap(0, Ordering::Relaxed)).filter(|&m| m > 0);
            let ms = tick_ms(at.into_std().saturating_duration_since(plan.t0), rate);
            let row = Row {
                ms,
                values: last.clone(),
                mark,
            };
            if plan.rows_tx.send(row).is_err() {
                break;
            }
        }
        drop(plan.rows_tx);
        let writer = plan.writer;
        let written = tokio::task::spawn_blocking(move || writer.join())
            .await
            .map_err(|e| e.to_string())
            .and_then(|j| j.map_err(|_| "the writer panicked".to_string()))
            .and_then(|r| r.map_err(|e| format!("data.csv: {e}")));
        if let Err(e) = written {
            error.get_or_insert(e);
        }
        if !plan.ids.is_empty() {
            let _ = self
                .inner
                .tx
                .try_send(Msg::DiagUnsubscribe(std::mem::take(&mut plan.ids)));
        }
        let end_values = self.readings(&[CLOCK, ODOMETER]).await;
        let rows = plan.rows.load(Ordering::Relaxed);
        let elapsed = plan.t0.elapsed();
        let summary = format!(
            "{} value{}, {} Hz, {} row{}",
            names.len(),
            if names.len() == 1 { "" } else { "s" },
            rate,
            capture::group(rows),
            if rows == 1 { "" } else { "s" }
        );
        let end = End {
            setup: self.setup(),
            meta: plan.meta,
            dir: plan.dir,
            profile: plan.profile,
            load: plan.load,
            summary: summary.clone(),
            rows,
            elapsed,
            start_values: plan.start_values,
            end_values,
            start_time: plan.start_time,
            error: error.clone(),
            carpid_up: self.inner.started.elapsed(),
        };
        let label = end.meta.id.to_string();
        let saved = tokio::task::spawn_blocking(move || end.finish())
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
        if let Err(e) = &saved {
            error.get_or_insert(e.clone());
        }
        let run = {
            let mut st = self.state();
            let mut run = st
                .run
                .take()
                .map(|l| l.run)
                .expect("the run is in the state until it ends");
            run.elapsed_ms = elapsed.as_millis() as u64;
            run.rows = rows;
            run.summary = summary;
            run.error = error.clone();
            run.marks = plan.marks.made.load(Ordering::Relaxed);
            st.last = Some(run.clone());
            run
        };
        match &error {
            None => {
                info!("log {label} saved: {}", run.summary);
                self.toast(format!("Log {label} saved"), ToastLevel::Info);
            }
            Some(e) => {
                warn!("log {label} stopped: {e}");
                self.toast(format!("Log {label} stopped: {e}"), ToastLevel::Error);
            }
        }
        run
    }

    fn toast(&self, text: String, level: ToastLevel) {
        let hub = self.inner.hub.clone();
        tokio::spawn(async move {
            if let Err(e) = hub
                .send(Command::Toast { text, level }, Duration::from_secs(2))
                .await
            {
                tracing::debug!("toast: {e}");
            }
        });
    }
}

fn period(rate_hz: u32) -> Duration {
    Duration::from_millis(1000 / rate_hz.max(1) as u64)
}

/// Makes the entry: `entry.json`, `profile.json`, `info.txt` and the CSV's
/// header. Returns the CSV to append to.
fn open(
    setup: &Setup,
    profile: &DatalogProfile,
    load: &DatalogLoad,
) -> Result<(Meta, PathBuf, std::fs::File), String> {
    let logs = Logs::new(&setup.root);
    let now = clock::local_now();
    let (meta, dir) = logs
        .create(Kind::Datalog, &now.date, &profile.name, &now.datetime)
        .map_err(|e| format!("creating the entry in {}: {e}", setup.root.display()))?;
    capture::prune(&logs, setup.max_pct, Some(&meta.id));
    let json = serde_json::json!({ "profile": profile, "load": load });
    std::fs::write(
        dir.join("profile.json"),
        serde_json::to_vec_pretty(&json).unwrap_or_default(),
    )
    .map_err(|e| format!("profile.json: {e}"))?;
    let mut f =
        std::fs::File::create(dir.join("data.csv")).map_err(|e| format!("data.csv: {e}"))?;
    f.write_all(header(profile).as_bytes())
        .and_then(|_| f.sync_data())
        .map_err(|e| format!("data.csv: {e}"))?;
    Ok((meta, dir, f))
}

/// The end of a run, written on a blocking thread.
struct End {
    setup: Setup,
    meta: Meta,
    dir: PathBuf,
    profile: DatalogProfile,
    load: DatalogLoad,
    summary: String,
    rows: u64,
    elapsed: Duration,
    start_values: Vec<Reading>,
    end_values: Vec<Reading>,
    start_time: SystemTime,
    error: Option<String>,
    carpid_up: Duration,
}

impl End {
    fn finish(mut self) -> Result<(), String> {
        let logs = Logs::new(&self.setup.root);
        std::fs::write(self.dir.join("info.txt"), self.info())
            .map_err(|e| format!("info.txt: {e}"))?;
        self.meta.ended = Some(clock::local_now().datetime);
        self.meta.summary = self.summary.clone();
        logs.save_meta(&self.meta)
            .map_err(|e| format!("entry.json: {e}"))?;
        capture::prune(&logs, self.setup.max_pct, Some(&self.meta.id));
        Ok(())
    }

    fn info(&self) -> String {
        let mut t = String::new();
        let mut line = |k: &str, v: String| t.push_str(&format!("{k:<11}{v}\n"));
        line("profile", self.profile.name.clone());
        line("rate", format!("{} Hz", self.profile.rate_hz));
        line("values", self.profile.values.join(" "));
        if !self.profile.labels.is_empty() {
            line("labels", self.profile.labels.join(" | "));
        }
        line("load", load_text(&self.load));
        line(
            "started",
            format!(
                "{} local, epoch {}",
                self.meta.started,
                clock::epoch_secs(self.start_time)
            ),
        );
        let end = SystemTime::now();
        line(
            "ended",
            format!(
                "{} local, epoch {}",
                clock::local(end).datetime,
                clock::epoch_secs(end)
            ),
        );
        line(
            "length",
            format!(
                "{}, {} rows",
                capture::dur(self.elapsed),
                capture::group(self.rows)
            ),
        );
        line("car clock", shown(&self.start_values, CLOCK, ""));
        line("at the end", shown(&self.end_values, CLOCK, ""));
        line("odometer", shown(&self.start_values, ODOMETER, " km"));
        line("at the end", shown(&self.end_values, ODOMETER, " km"));
        line(
            "carpid",
            format!(
                "{}, up {}",
                env!("CARGO_PKG_VERSION"),
                capture::dur(self.carpid_up)
            ),
        );
        line("release", capture::release(&self.setup.manifest));
        if let Some(e) = &self.error {
            line("stopped by", e.clone());
        }
        t
    }
}

fn shown(values: &[Reading], name: &str, unit: &str) -> String {
    match values.iter().find(|r| r.name == name) {
        Some(Reading {
            value: Some(v),
            age_ms,
            ..
        }) => format!("{}{unit} ({age_ms} ms old)", capture::show_value(v)),
        Some(_) => "invalid".into(),
        None => "-".into(),
    }
}

/// `7 bus, 5 diag in 3 groups on 1 module, about 4.4 Hz each`.
pub fn load_text(l: &DatalogLoad) -> String {
    let mut s = format!("{} bus, {} diag", l.bus_values, l.diag_values);
    if l.diag_groups > 0 {
        s.push_str(&format!(
            " in {} group{} on {} module{}",
            l.diag_groups,
            if l.diag_groups == 1 { "" } else { "s" },
            l.diag_modules,
            if l.diag_modules == 1 { "" } else { "s" }
        ));
    }
    if let Some(hz) = l.diag_hz {
        s.push_str(&format!(", about {hz} Hz each"));
    }
    s
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn profile(name: &str, values: &[&str], rate_hz: u32) -> DatalogProfile {
        DatalogProfile {
            name: name.into(),
            values: values.iter().map(|v| v.to_string()).collect(),
            rate_hz,
            labels: Vec::new(),
        }
    }

    #[test]
    fn header_rows_and_marks() {
        let mut p = profile("x", &["engine.rpm", "a,b", "engine.boost.actual"], 1);
        assert_eq!(
            header(&p),
            "Time (s),engine.rpm,\"a,b\",engine.boost.actual,Mark\n"
        );
        p.labels = vec!["RPM".into(), String::new()];
        assert_eq!(
            header(&p),
            "Time (s),RPM,\"a,b\",engine.boost.actual,Mark\n"
        );
        let row = line(
            1250,
            &[
                Some(Value::Num(840.0)),
                None,
                Some(Value::Num(0.9921875)),
                Some(Value::Bool(true)),
                Some(Value::Text("say \"hi\"\nthere".into())),
                Some(Value::Num(f64::NAN)),
                Some(Value::Num(-0.0000001)),
            ],
            None,
        );
        assert_eq!(row, "1.250,840,,0.992188,1,\"say \"\"hi\"\" there\",,0,\n");
        assert_eq!(line(0, &[None, None], Some(3)), "0.000,,,3\n");
    }

    #[test]
    fn values_hold_between_samples() {
        let mut last = vec![None, None];
        hold(&mut last, vec![Some(Value::Num(1.0)), None]);
        assert_eq!(last, [Some(Value::Num(1.0)), None]);
        hold(&mut last, vec![None, Some(Value::Num(2.0))]);
        assert_eq!(last, [Some(Value::Num(1.0)), Some(Value::Num(2.0))]);
        hold(&mut last, vec![Some(Value::Num(3.0)), None]);
        assert_eq!(last, [Some(Value::Num(3.0)), Some(Value::Num(2.0))]);
    }

    #[test]
    fn labels_follow_their_values() {
        let mut p = profile("x", &["a", "b", "a", "c"], 1);
        p.labels = vec!["A".into(), " B\n2 ".into(), "A again".into()];
        let p = check(p).unwrap();
        assert_eq!(p.values, ["a", "b", "c"]);
        assert_eq!(p.labels, ["A", "B 2", ""]);
        let mut q = profile("x", &["a", "b"], 1);
        q.labels = vec![String::new(), "  ".into(), "extra".into()];
        assert!(check(q).unwrap().labels.is_empty());
    }

    #[test]
    fn rows_sit_on_the_rate_grid() {
        assert_eq!(tick_ms(Duration::ZERO, 10), 0);
        assert_eq!(tick_ms(Duration::from_millis(149), 10), 100);
        assert_eq!(tick_ms(Duration::from_millis(151), 10), 200);
        assert_eq!(tick_ms(Duration::from_millis(1013), 20), 1000);
        assert_eq!(tick_ms(Duration::from_millis(2600), 1), 3000);
        assert_eq!(tick_ms(Duration::from_millis(410), 5), 400);
    }

    #[test]
    fn profiles_check_and_round_trip() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join(FILE);
        // No file: the defaults.
        let (f, broken) = read_profiles(&path);
        assert!(!broken);
        assert_eq!(f.profiles[0].name, "Boost");

        let p = check(profile(
            "  Haldex ",
            &["awd.oil_temp_c", "engine.rpm", "awd.oil_temp_c"],
            5,
        ))
        .unwrap();
        assert_eq!(p.name, "Haldex");
        assert_eq!(p.values, ["awd.oil_temp_c", "engine.rpm"]);
        assert!(check(profile("x", &[], 3)).is_err());
        assert!(check(profile("", &[], 1)).is_err());
        assert!(check(profile("a\nb", &[], 1)).is_err());
        assert!(check(profile("x", &["../etc"], 1)).is_err());
        assert!(check(profile("x", &["a b"], 1)).is_err());

        let file = ProfilesFile {
            selected: Some("Haldex".into()),
            profiles: vec![p.clone(), profile("Trip", &[], 1)],
        };
        write_profiles(&path, &file, false).unwrap();
        let (back, broken) = read_profiles(&path);
        assert!(!broken);
        assert_eq!(back.selected.as_deref(), Some("Haldex"));
        assert_eq!(back.profiles, file.profiles);

        // A broken file: defaults now, set aside on the next save.
        std::fs::write(&path, "[[profile]\nname=").unwrap();
        let (f, broken) = read_profiles(&path);
        assert!(broken);
        assert_eq!(f.profiles, defaults());
        write_profiles(&path, &file, true).unwrap();
        assert!(path.with_extension("toml.bad").exists());
        // A bad profile in a good file is skipped.
        std::fs::write(
            &path,
            "[[profile]]\nname='A'\nvalues=[]\nrate_hz=3\n[[profile]]\nname='B'\nvalues=['engine.rpm']\nrate_hz=2\n",
        )
        .unwrap();
        let (f, _) = read_profiles(&path);
        assert_eq!(f.profiles.len(), 1);
        assert_eq!(f.profiles[0].name, "B");
    }

    #[test]
    fn load_counts_reads_like_the_scheduler() {
        let c = Catalog::seed();
        let v = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let l = load(&c, &v(&["engine.rpm", "nav.road"]), 10);
        assert_eq!((l.bus_values, l.diag_values, l.diag_groups), (2, 0, 0));
        assert_eq!(l.diag_hz, None);
        // Boost actual and specified share group 115.
        let l = load(
            &c,
            &v(&[
                "engine.boost.actual",
                "engine.boost.specified",
                "engine.rpm",
            ]),
            10,
        );
        assert_eq!((l.diag_values, l.diag_groups, l.diag_modules), (2, 1, 1));
        assert_eq!(l.diag_hz, Some(10.0));
        // Engine and Haldex: two modules; four engine groups at 60 ms fill
        // 80% at 3.3 Hz.
        let l = load(
            &c,
            &v(&[
                "engine.boost.actual",
                "engine.timing_retard.cyl1_deg",
                "engine.fuel_rail.actual_bar",
                "engine.lambda.actual",
                "awd.oil_temp_c",
            ]),
            20,
        );
        assert_eq!((l.diag_groups, l.diag_modules), (5, 2));
        assert_eq!(l.diag_hz, Some(3.3));
        // Module groups: two fields of one block are one read.
        let g = load(
            &c,
            &v(&[
                "diag.01.103.1",
                "diag.01.103.2",
                "diag.22.5.1",
                "diag.03.1.1",
            ]),
            10,
        );
        assert_eq!((g.bus_values, g.diag_values), (1, 3));
        assert_eq!((g.diag_groups, g.diag_modules), (2, 2));

        assert_eq!(
            load_text(&l),
            "0 bus, 5 diag in 5 groups on 2 modules, about 3.3 Hz each"
        );
    }

    /// Answers the event loop's part: readings from `values`, diag subscribe
    /// and unsubscribe. Returns what was unsubscribed.
    pub(crate) fn fake_loop(
        mut rx: mpsc::Receiver<Msg>,
        values: Arc<Mutex<Vec<Reading>>>,
    ) -> tokio::task::JoinHandle<Vec<Vec<carpi_core::DiagSubId>>> {
        tokio::spawn(async move {
            let mut gone = Vec::new();
            while let Some(m) = rx.recv().await {
                match m {
                    Msg::Readings {
                        prefixes, reply, ..
                    } => {
                        let v = values.lock().unwrap().clone();
                        let _ = reply.send(
                            v.into_iter()
                                .filter(|r| prefixes.iter().any(|p| r.name.starts_with(p)))
                                .collect(),
                        );
                    }
                    Msg::DiagSubscribe(names, _, reply) => {
                        let _ = reply.send(
                            names
                                .iter()
                                .filter(|n| n.starts_with("awd."))
                                .map(|_| carpi_core::DiagSubId(7))
                                .collect(),
                        );
                    }
                    Msg::DiagUnsubscribe(ids) => gone.push(ids),
                    Msg::Api(_, carpi_proto::Request::Hello { version, .. }, reply) => {
                        let _ = reply.send(Response::Hello {
                            version,
                            daemon: "test".into(),
                        });
                    }
                    _ => {}
                }
            }
            gone
        })
    }

    pub(crate) fn setup(dir: &Path) -> Setup {
        Setup {
            root: dir.join("logs"),
            max_pct: 25,
            profiles: dir.join(FILE),
            manifest: dir.join("manifest.json"),
        }
    }

    #[tokio::test]
    async fn a_run_writes_its_entry() {
        let t = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel(64);
        let values = Arc::new(Mutex::new(vec![
            Reading {
                name: "engine.rpm".into(),
                value: Some(Value::Num(900.0)),
                age_ms: 10,
            },
            Reading {
                name: "engine.rpm_target".into(),
                value: Some(Value::Num(1.0)),
                age_ms: 10,
            },
            Reading {
                name: "awd.oil_temp_c".into(),
                value: None,
                age_ms: 3000,
            },
            Reading {
                name: CLOCK.into(),
                value: Some(Value::Text("2026-11-08T10:00:00".into())),
                age_ms: 200,
            },
        ]));
        let lp = fake_loop(rx, values.clone());
        let d = Datalog::new(setup(t.path()), tx, Arc::new(CommandHub::default()));
        let p = profile("Haldex", &["engine.rpm", "awd.oil_temp_c", "nav.road"], 20);
        let r = d
            .handle(DatalogAction::Save {
                profile: p.clone(),
                replaces: None,
            })
            .await;
        assert!(matches!(r, Response::Datalog { .. }), "{r:?}");

        let run = d.start("Haldex").await.unwrap();
        assert_eq!(run.label, format!("{} #01", clock::local_now().date));
        let again = d
            .handle(DatalogAction::Start {
                profile: "Boost".into(),
            })
            .await;
        assert!(
            matches!(&again, Response::Error { message } if message.starts_with("already logging")),
            "{again:?}"
        );
        tokio::time::sleep(Duration::from_millis(520)).await;
        values.lock().unwrap()[0].value = Some(Value::Num(2500.0));
        assert_eq!(d.mark(), Ok(1));
        tokio::time::sleep(Duration::from_millis(200)).await;
        // Gone stale: the row keeps the last value.
        values.lock().unwrap()[0].value = None;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let st = d.status();
        assert!(st.running.as_ref().unwrap().rows >= 10);
        assert_eq!(st.running.as_ref().unwrap().marks, 1);
        let done = d.stop().await.unwrap();
        assert_eq!(done.marks, 1);
        assert!(d.status().running.is_none());
        assert_eq!(d.status().last.as_ref(), Some(&done));
        assert!(matches!(d.stop().await, Err(e) if e == "not logging"));
        assert!(matches!(
            d.handle(DatalogAction::Mark).await,
            Response::Error { message } if message == "not logging"
        ));

        let logs = Logs::new(t.path().join("logs"));
        let id = carpi_logs::EntryId::parse(&done.label).unwrap();
        let meta = logs.meta(&id).unwrap();
        assert_eq!(meta.kind, Kind::Datalog);
        assert_eq!(meta.trigger, "Haldex");
        assert!(meta.ended.is_some());
        assert_eq!(
            meta.summary,
            format!("3 values, 20 Hz, {} rows", capture::group(done.rows))
        );
        let names: Vec<_> = logs.files(&id).into_iter().map(|f| f.path).collect();
        assert_eq!(
            names,
            ["data.csv", "entry.json", "info.txt", "profile.json"]
        );
        let csv = std::fs::read_to_string(Path::new(&done.dir).join("data.csv")).unwrap();
        let mut lines = csv.lines();
        assert_eq!(
            lines.next(),
            Some("Time (s),engine.rpm,awd.oil_temp_c,nav.road,Mark")
        );
        let rows: Vec<&str> = lines.collect();
        assert_eq!(rows.len() as u64, done.rows);
        assert!((15..=25).contains(&rows.len()), "{}", rows.len());
        assert_eq!(rows[0], "0.000,900,,,");
        let marked: Vec<&&str> = rows.iter().filter(|r| !r.ends_with(',')).collect();
        assert_eq!(marked.len(), 1, "{rows:?}");
        assert!(marked[0].ends_with(",2500,,,1"), "{marked:?}");
        let times: Vec<u64> = rows
            .iter()
            .map(|r| {
                let t = r.split(',').next().unwrap().replace('.', "");
                t.parse().unwrap()
            })
            .collect();
        assert!(times.windows(2).all(|w| w[1] > w[0]), "{times:?}");
        assert!(times.iter().all(|t| t % 50 == 0), "{times:?}");
        assert!(rows.last().unwrap().ends_with(",2500,,,"));

        let info = std::fs::read_to_string(Path::new(&done.dir).join("info.txt")).unwrap();
        assert!(info.contains("profile    Haldex"), "{info}");
        assert!(info.contains("2026-11-08T10:00:00 (200 ms old)"), "{info}");
        assert!(
            info.contains("load       2 bus, 1 diag in 1 group on 1 module"),
            "{info}"
        );
        let pj: serde_json::Value = serde_json::from_slice(
            &std::fs::read(Path::new(&done.dir).join("profile.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(pj["profile"]["rate_hz"], 20);

        // The diag value was let go, and the profile is the one a key starts.
        drop(d);
        let gone = lp.await.unwrap();
        assert_eq!(gone, [vec![carpi_core::DiagSubId(7)]]);
        let (f, _) = read_profiles(&t.path().join(FILE));
        assert_eq!(f.selected.as_deref(), Some("Haldex"));
    }

    #[tokio::test]
    async fn profiles_save_rename_and_delete() {
        let t = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel(64);
        let _lp = fake_loop(rx, Arc::default());
        let d = Datalog::new(setup(t.path()), tx, Arc::new(CommandHub::default()));
        let names = |d: &Datalog| {
            d.status()
                .profiles
                .into_iter()
                .map(|p| p.profile.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&d), ["Boost"]);
        d.save(profile("Trip", &["fuel.level_l"], 1), None).unwrap();
        assert!(
            d.save(profile("Trip", &[], 1), Some("Boost".into()))
                .is_err()
        );
        d.save(profile("Engine", &["engine.rpm"], 2), Some("Boost".into()))
            .unwrap();
        assert_eq!(names(&d), ["Engine", "Trip"]);
        assert!(d.start("Nope").await.is_err());
        d.save(profile("Empty", &[], 1), None).unwrap();
        assert!(
            d.start("Empty")
                .await
                .is_err_and(|e| e.contains("no values"))
        );
        d.delete("Trip").unwrap();
        assert!(d.delete("Trip").is_err());
        let again = Datalog::new(
            setup(t.path()),
            mpsc::channel(1).0,
            Arc::new(CommandHub::default()),
        );
        assert_eq!(names(&again), ["Engine", "Empty"]);
        assert_eq!(again.status().profiles[0].load.bus_values, 1);
    }
}
