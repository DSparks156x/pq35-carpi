//! Captures (docs/PAGES.md 3): the last few minutes of everything, as the
//! next `<date>/<NN>/` of the logs folder.
//!
//! ```text
//! entry.json      carpi_logs::Meta
//! can-<iface>.log the ring's window, candump log format, one per bus
//! journal.txt     journalctl for the same window, every unit
//! values.json     the value store with ages
//! config/         carpid.toml, LIVI's config.json, avsd.env, carpi-profile, manifest.json
//! info.txt        trigger, Pi time, car clock, odometer, versions, uptime, buses
//! ```
//!
//! One function makes them all: [`Captures::capture`]. The key gesture, the
//! API (`Request::Capture`, which `carpi capture` and the crash unit use)
//! and the portal call it. The CAN window is copied on the event loop (one
//! `Msg::Snapshot`); everything else runs on a blocking thread.
//!
//! Automatic captures (`crash:*`, `error:*`) are rate limited: one per
//! reason per 10 minutes, six an hour. The limit is kept on disk so a crash
//! loop can't fill the card.

use crate::Msg;
use crate::clock;
use crate::commands::CommandHub;
use carpi_can::RxFrame;
use carpi_logs::{Kind, Logs, Meta};
use carpi_proto::{BusInfo, Command, Reading, Response, ToastLevel};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::{mpsc, oneshot};
use tracing::{info, warn};

/// Newest journal lines kept; bytes as a backstop.
const JOURNAL_LINES: u32 = 50_000;
const JOURNAL_BYTES: u64 = 16 << 20;
/// Config files bigger than this aren't copied.
const CONFIG_BYTES: u64 = 1 << 20;

/// Automatic captures: per reason, and in any hour.
const REASON_GAP_S: u64 = 600;
const HOUR_MAX: usize = 6;

/// What the event loop hands over: copied, so writing never holds it up.
pub struct Snapshot {
    pub buses: Vec<(String, Vec<RxFrame>)>,
    pub values: Vec<Reading>,
    pub bus_info: Vec<BusInfo>,
}

impl Snapshot {
    /// The last `back` of the ring and the store as it is now.
    pub fn take(car: &carpi_core::Car, back: Duration, now: Instant) -> Self {
        Snapshot {
            buses: car.capture_window(back, now),
            values: car.readings(&[String::new()], now),
            bus_info: car.buses(now),
        }
    }

    fn value(&self, name: &str) -> Option<&Reading> {
        self.values.iter().find(|r| r.name == name)
    }
}

/// Where things are. [`Setup::new`] has the real paths.
#[derive(Clone, Debug)]
pub struct Setup {
    pub root: PathBuf,
    pub minutes: u64,
    pub max_pct: u8,
    /// (source, name under `config/`).
    pub configs: Vec<(PathBuf, String)>,
    /// carpid's own state (the rate limit).
    pub state_dir: PathBuf,
    /// The release manifest, for versions.
    pub manifest: PathBuf,
    /// Run `journalctl`, `ip`, `timedatectl`. Off in tests.
    pub system: bool,
}

impl Setup {
    pub fn new(cfg: &crate::config::Config, config_path: &Path) -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let manifest = PathBuf::from("/opt/carpi/current/manifest.json");
        let configs = vec![
            (config_path.to_path_buf(), "carpid.toml".into()),
            (
                home.join(".config/LIVI/config.json"),
                "livi-config.json".into(),
            ),
            (home.join("avsd/avsd.env"), "avsd.env".into()),
            ("/etc/carpi-profile".into(), "carpi-profile".into()),
            (manifest.clone(), "manifest.json".into()),
        ];
        Setup {
            root: cfg.logs.root(),
            minutes: cfg.logs.capture_minutes,
            max_pct: cfg.logs.max_pct,
            configs,
            state_dir: crate::runmark::state_dir(),
            manifest,
            system: true,
        }
    }
}

/// Automatic captures allowed: per reason and per hour. Times are epoch
/// seconds so the record survives a restart.
#[derive(Debug, Default)]
pub struct Limiter {
    recent: Vec<(u64, String)>,
}

impl Limiter {
    pub fn parse(text: &str) -> Self {
        let recent = text
            .lines()
            .filter_map(|l| {
                let (t, r) = l.split_once(' ')?;
                Some((t.parse().ok()?, r.to_string()))
            })
            .collect();
        Limiter { recent }
    }

    pub fn text(&self) -> String {
        self.recent
            .iter()
            .map(|(t, r)| format!("{t} {r}\n"))
            .collect()
    }

    /// True (and counted) if a capture for `reason` may go now.
    pub fn allow(&mut self, reason: &str, now: u64) -> bool {
        // An hour back; a clock that jumped back drops what's ahead of it.
        self.recent
            .retain(|(t, _)| *t <= now + REASON_GAP_S && now.saturating_sub(*t) < 3600);
        if self.recent.len() >= HOUR_MAX {
            return false;
        }
        if self
            .recent
            .iter()
            .any(|(t, r)| r == reason && now.saturating_sub(*t) < REASON_GAP_S)
        {
            return false;
        }
        self.recent.push((now, reason.to_string()));
        true
    }
}

/// `crash:...` and `error:...` are automatic.
pub fn is_auto(trigger: &str) -> bool {
    trigger.starts_with("crash:") || trigger.starts_with("error:")
}

/// One printable line, at most 64 characters; `crash:livi.service` ->
/// `crash:livi`.
pub fn clean_trigger(t: &str) -> String {
    let t: String = t
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(64)
        .collect();
    let t = match t.strip_prefix("crash:") {
        Some(unit) => format!("crash:{}", unit.strip_suffix(".service").unwrap_or(unit)),
        None => t,
    };
    if t.is_empty() { "unknown".into() } else { t }
}

/// What a crash capture knows about the run that ended.
#[derive(Clone, Debug)]
pub struct After {
    pub end: SystemTime,
    pub note: String,
}

#[derive(Clone)]
pub struct Captures {
    inner: Arc<Inner>,
}

struct Inner {
    setup: Setup,
    /// `minutes` and `max_pct`, live from the settings.
    limits: Mutex<(u64, u8)>,
    tx: mpsc::Sender<Msg>,
    hub: Arc<CommandHub>,
    limiter: Mutex<Limiter>,
    /// One capture at a time.
    busy: tokio::sync::Mutex<()>,
    started: Instant,
}

const LIMIT_FILE: &str = "auto-captures";

impl Captures {
    pub fn new(setup: Setup, tx: mpsc::Sender<Msg>, hub: Arc<CommandHub>) -> Self {
        let limiter = std::fs::read_to_string(setup.state_dir.join(LIMIT_FILE))
            .map(|t| Limiter::parse(&t))
            .unwrap_or_default();
        Captures {
            inner: Arc::new(Inner {
                limits: Mutex::new((setup.minutes, setup.max_pct)),
                setup,
                tx,
                hub,
                limiter: Mutex::new(limiter),
                busy: tokio::sync::Mutex::new(()),
                started: Instant::now(),
            }),
        }
    }

    /// Makes one capture and tells LIVI (a toast). `trigger`: `key`,
    /// `page`, `portal`, `cli`, `crash:<unit>`, `error:<what>`. Answers
    /// `Response::Captured`, or `Response::Error` (also when an automatic
    /// one is over its rate limit).
    /// `[logs] capture_minutes` and `max_pct`, live.
    pub fn set_limits(&self, minutes: u64, max_pct: u8) {
        *self.inner.limits.lock().unwrap_or_else(|e| e.into_inner()) = (minutes, max_pct);
    }

    fn setup(&self) -> Setup {
        let (minutes, max_pct) = *self.inner.limits.lock().unwrap_or_else(|e| e.into_inner());
        Setup {
            minutes,
            max_pct,
            ..self.inner.setup.clone()
        }
    }

    /// The logs folder.
    pub fn root(&self) -> &Path {
        &self.inner.setup.root
    }

    pub async fn capture(&self, trigger: &str) -> Response {
        self.run(trigger, None).await
    }

    /// An automatic capture in the background.
    pub fn auto(&self, reason: String) {
        let c = self.clone();
        tokio::spawn(async move {
            c.capture(&reason).await;
        });
    }

    /// `crash:carpid` for the run that ended at `after.end`: journal only.
    pub fn crash(&self, after: After) {
        let c = self.clone();
        tokio::spawn(async move {
            c.run("crash:carpid", Some(after)).await;
        });
    }

    /// Retention once at startup.
    pub fn prune_now(&self) {
        let root = self.inner.setup.root.clone();
        let pct = self.setup().max_pct;
        tokio::task::spawn_blocking(move || {
            let _ = std::fs::create_dir_all(&root);
            prune(&Logs::new(&root), pct, None);
        });
    }

    fn allowed(&self, trigger: &str) -> bool {
        let now = clock::epoch_secs(SystemTime::now()).max(0) as u64;
        let mut l = self.inner.limiter.lock().unwrap_or_else(|e| e.into_inner());
        if !l.allow(trigger, now) {
            return false;
        }
        let dir = &self.inner.setup.state_dir;
        let _ = std::fs::create_dir_all(dir)
            .and_then(|_| std::fs::write(dir.join(LIMIT_FILE), l.text()));
        true
    }

    async fn run(&self, trigger: &str, after: Option<After>) -> Response {
        let trigger = clean_trigger(trigger);
        let auto = is_auto(&trigger);
        if auto && !self.allowed(&trigger) {
            info!("capture ({trigger}) skipped: rate limit");
            return Response::Error {
                message: format!("{trigger}: automatic captures are rate limited"),
            };
        }
        let _busy = self.inner.busy.lock().await;
        let s = self.setup();
        let back = Duration::from_secs(s.minutes * 60);
        let (snapshot, end, note) = match after {
            Some(a) => (None, a.end, Some(a.note)),
            None => {
                let (rtx, rrx) = oneshot::channel();
                let snap = match self.inner.tx.send(Msg::Snapshot(back, rtx)).await {
                    Ok(()) => tokio::time::timeout(Duration::from_secs(5), rrx)
                        .await
                        .ok()
                        .and_then(Result::ok),
                    Err(_) => None,
                };
                let note = snap
                    .is_none()
                    .then(|| "the event loop didn't answer: no CAN or values".to_string());
                (snap, SystemTime::now(), note)
            }
        };
        let job = Job {
            setup: s,
            trigger: trigger.clone(),
            snapshot,
            start: end.checked_sub(back).unwrap_or(end),
            end,
            note,
            carpid_up: self.inner.started.elapsed(),
        };
        let result = tokio::task::spawn_blocking(move || job.run())
            .await
            .unwrap_or_else(|e| Err(format!("capture task: {e}")));
        let level = if auto {
            ToastLevel::Warn
        } else {
            ToastLevel::Info
        };
        match result {
            Ok((meta, dir)) => {
                info!("capture {} ({trigger}): {}", meta.id, meta.summary);
                self.toast(format!("Capture {} saved", meta.id), level);
                Response::Captured {
                    label: meta.id.to_string(),
                    date: meta.id.date,
                    number: meta.id.number,
                    dir: dir.display().to_string(),
                    summary: meta.summary,
                }
            }
            Err(message) => {
                warn!("capture ({trigger}) failed: {message}");
                self.toast(format!("Capture failed: {message}"), ToastLevel::Error);
                Response::Error { message }
            }
        }
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

/// Everything one capture writes, run on a blocking thread.
struct Job {
    setup: Setup,
    trigger: String,
    snapshot: Option<Snapshot>,
    start: SystemTime,
    end: SystemTime,
    /// For info.txt: why something is missing.
    note: Option<String>,
    carpid_up: Duration,
}

impl Job {
    fn run(self) -> Result<(Meta, PathBuf), String> {
        let s = &self.setup;
        let logs = Logs::new(&s.root);
        let now = clock::local_now();
        let (mut meta, dir) = logs
            .create(Kind::Capture, &now.date, &self.trigger, &now.datetime)
            .map_err(|e| format!("creating the entry in {}: {e}", s.root.display()))?;
        let mut summary = Vec::new();

        match &self.snapshot {
            Some(snap) => {
                let mut total = 0;
                for (iface, frames) in &snap.buses {
                    write_can(
                        &dir.join(format!("can-{}.log", safe_name(iface))),
                        iface,
                        frames,
                    )
                    .map_err(|e| format!("can-{iface}.log: {e}"))?;
                    total += frames.len();
                }
                let n = snap.buses.len();
                summary.push(format!(
                    "{n} bus{}, {} frames",
                    if n == 1 { "" } else { "es" },
                    group(total as u64)
                ));
                let values = serde_json::json!({ "taken": now.datetime, "values": snap.values });
                std::fs::write(
                    dir.join("values.json"),
                    serde_json::to_vec_pretty(&values).unwrap_or_default(),
                )
                .map_err(|e| format!("values.json: {e}"))?;
            }
            None => summary.push("no CAN".into()),
        }

        let journal = dir.join("journal.txt");
        match write_journal(
            s.system,
            clock::epoch_secs(self.start),
            clock::epoch_secs(self.end) + 1,
            &journal,
        ) {
            Ok(lines) => summary.push(format!("journal {} lines", group(lines))),
            Err(e) => {
                let _ = std::fs::write(&journal, format!("no journal: {e}\n"));
                summary.push("no journal".into());
            }
        }

        let copied = copy_configs(&dir.join("config"), &s.configs);
        std::fs::write(dir.join("info.txt"), self.info(&now, &copied))
            .map_err(|e| format!("info.txt: {e}"))?;

        meta.ended = Some(clock::local_now().datetime);
        meta.summary = summary.join(", ");
        logs.save_meta(&meta)
            .map_err(|e| format!("entry.json: {e}"))?;
        prune(&logs, s.max_pct, Some(&meta.id));
        Ok((meta, dir))
    }

    fn info(&self, now: &clock::Local, copied: &[String]) -> String {
        let s = &self.setup;
        let mut t = String::new();
        let mut line = |k: &str, v: String| t.push_str(&format!("{k:<11}{v}\n"));
        line("trigger", self.trigger.clone());
        line(
            "pi time",
            format!(
                "{} local, epoch {}{}",
                now.datetime,
                clock::epoch_secs(SystemTime::now()),
                if s.system { ntp_state() } else { String::new() }
            ),
        );
        line(
            "window",
            format!(
                "{} to {}",
                clock::local(self.start).datetime,
                clock::local(self.end).datetime
            ),
        );
        let snap = self.snapshot.as_ref();
        let shown = |name: &str, unit: &str| match snap.and_then(|s| s.value(name)) {
            Some(Reading {
                value: Some(v),
                age_ms,
                ..
            }) => format!("{}{unit} ({age_ms} ms old)", show_value(v)),
            Some(_) => "invalid".into(),
            None => "-".into(),
        };
        line("car clock", shown("clock.datetime", ""));
        line("odometer", shown("vehicle.odometer_km", " km"));
        line(
            "carpid",
            format!("{}, up {}", env!("CARGO_PKG_VERSION"), dur(self.carpid_up)),
        );
        line("release", release(&s.manifest));
        let profile = std::fs::read_to_string("/etc/carpi-profile")
            .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))
            .unwrap_or_else(|_| "-".into());
        line("os profile", profile);
        let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .map(|k| k.trim().to_string())
            .unwrap_or_else(|_| "-".into());
        line("kernel", kernel);
        let up = std::fs::read_to_string("/proc/uptime")
            .ok()
            .and_then(|u| u.split_whitespace().next()?.parse::<f64>().ok())
            .map_or("-".into(), |s| dur(Duration::from_secs_f64(s)));
        line("uptime", up);
        line(
            "config",
            if copied.is_empty() {
                "-".into()
            } else {
                copied.join(" ")
            },
        );
        if let Some(n) = &self.note {
            line("note", n.clone());
        }
        match snap {
            None => t.push_str("\nNo CAN in this capture.\n"),
            Some(snap) => {
                t.push_str("\nbuses\n");
                for b in &snap.bus_info {
                    let window = snap
                        .buses
                        .iter()
                        .find(|(n, _)| *n == b.iface)
                        .map_or(0, |(_, f)| f.len());
                    t.push_str(&format!(
                        "  {} ({}): {}, {} frames since start, {} in the window\n",
                        b.iface,
                        b.role,
                        b.state.as_deref().unwrap_or("no ring"),
                        group(b.frames),
                        group(window as u64)
                    ));
                }
                if s.system {
                    for b in &snap.bus_info {
                        if let Some(text) = run_text(
                            "ip",
                            &["-details", "-statistics", "link", "show", "dev", &b.iface],
                        ) {
                            t.push_str(&format!("\nip link {}\n{text}", b.iface));
                        }
                    }
                }
            }
        }
        t
    }
}

pub(crate) fn show_value(v: &carpi_proto::Value) -> String {
    match v {
        carpi_proto::Value::Bool(b) => b.to_string(),
        carpi_proto::Value::Num(n) => format!("{n}"),
        carpi_proto::Value::Text(s) => s.clone(),
    }
}

fn write_can(path: &Path, iface: &str, frames: &[RxFrame]) -> io::Result<()> {
    let mut w = io::BufWriter::new(std::fs::File::create(path)?);
    for f in frames {
        carpi_can::candump::write_line(&mut w, f.wall, iface, &f.frame)?;
    }
    w.flush()
}

/// The journal from `since` to `until` (epoch seconds), every unit: as a
/// member of `adm`, plain `journalctl` reads the system journal and every
/// user's, merged in time order. Returns the line count.
fn write_journal(system: bool, since: i64, until: i64, out: &Path) -> io::Result<u64> {
    use std::process::{Command, Stdio};
    if !system {
        std::fs::write(out, "")?;
        return Ok(0);
    }
    let err_path = out.with_file_name(".journal.err");
    let mut child = Command::new("journalctl")
        .args([
            "--since",
            &format!("@{since}"),
            "--until",
            &format!("@{until}"),
            "-o",
            "short-iso-precise",
            "--no-pager",
            "-n",
            &JOURNAL_LINES.to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(std::fs::File::create(&err_path)?)
        .spawn()?;
    let mut file = io::BufWriter::new(std::fs::File::create(out)?);
    let mut stdout = child.stdout.take().expect("piped");
    let mut buf = [0u8; 64 * 1024];
    let (mut bytes, mut lines) = (0u64, 0u64);
    let mut cut = false;
    loop {
        let n = stdout.read(&mut buf)?;
        if n == 0 {
            break;
        }
        let n = n.min((JOURNAL_BYTES - bytes) as usize);
        file.write_all(&buf[..n])?;
        lines += buf[..n].iter().filter(|b| **b == b'\n').count() as u64;
        bytes += n as u64;
        if bytes >= JOURNAL_BYTES {
            cut = true;
            let _ = child.kill();
            break;
        }
    }
    let status = child.wait()?;
    if cut {
        writeln!(file, "\n[cut at {} MB]", JOURNAL_BYTES >> 20)?;
    }
    let err = std::fs::read_to_string(&err_path).unwrap_or_default();
    let _ = std::fs::remove_file(&err_path);
    if !err.trim().is_empty() {
        writeln!(file, "\n--- journalctl stderr ---\n{}", err.trim())?;
    }
    file.flush()?;
    if !status.success() && !cut && lines == 0 {
        return Err(io::Error::other(format!(
            "journalctl {status}: {}",
            err.trim()
        )));
    }
    Ok(lines)
}

/// Copies the config files that exist; returns their names.
fn copy_configs(dir: &Path, files: &[(PathBuf, String)]) -> Vec<String> {
    let mut out = Vec::new();
    for (src, name) in files {
        let Ok(m) = std::fs::metadata(src) else {
            continue;
        };
        if !m.is_file() || m.len() > CONFIG_BYTES {
            continue;
        }
        if std::fs::create_dir_all(dir).is_ok() && std::fs::copy(src, dir.join(name)).is_ok() {
            out.push(name.clone());
        }
    }
    out
}

/// `version (channel), pq35-carpi <sha>, livi <sha>, sdlink <sha>`.
pub(crate) fn release(manifest: &Path) -> String {
    let Some(m) = std::fs::read(manifest)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
    else {
        return "- (no manifest)".into();
    };
    let s = |v: &serde_json::Value| v.as_str().unwrap_or("?").to_string();
    let mut out = format!("{} ({})", s(&m["version"]), s(&m["channel"]));
    if let Some(c) = m["commits"].as_object() {
        for (k, v) in c {
            out.push_str(&format!(
                ", {k} {}",
                v.as_str().unwrap_or("?").get(..10).unwrap_or("?")
            ));
        }
    }
    out
}

/// `, NTP on, not synced`, from timedatectl.
fn ntp_state() -> String {
    match run_text("timedatectl", &["show"]) {
        Some(t) => {
            let (ntp, synced) = clock::parse_show(&t);
            format!(
                ", NTP {}, {}",
                if ntp { "on" } else { "off" },
                if synced { "synced" } else { "not synced" }
            )
        }
        None => String::new(),
    }
}

fn run_text(program: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Logs over `max_pct` of their filesystem lose their oldest entries.
pub(crate) fn prune(logs: &Logs, max_pct: u8, keep: Option<&carpi_logs::EntryId>) {
    let Some(size) = fs_size(logs.root()) else {
        return;
    };
    let max = size / 100 * max_pct as u64;
    for id in logs.prune(max, keep) {
        info!("logs over {max_pct}% of the disk: removed {id}");
    }
}

#[cfg(unix)]
fn fs_size(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: statvfs fills the struct we pass; the path is NUL-terminated.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    Some(st.f_blocks as u64 * st.f_frsize as u64)
}

#[cfg(not(unix))]
fn fs_size(_path: &Path) -> Option<u64> {
    None
}

/// Interface names go into file names: `[A-Za-z0-9._-]` only.
fn safe_name(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// `31812` -> `31 812`.
pub fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

pub(crate) fn dur(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s} s"),
        60..3600 => format!("{} min", s / 60),
        _ => format!("{} h {:02} min", s / 3600, s / 60 % 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use carpi_can::Frame;

    #[test]
    fn rate_limit() {
        let mut l = Limiter::default();
        let t = 1_000_000;
        assert!(l.allow("crash:livi", t));
        assert!(!l.allow("crash:livi", t + 599), "same reason within 10 min");
        assert!(l.allow("crash:livi", t + 600));
        for (i, r) in ["error:a", "error:b", "error:c", "error:d"]
            .iter()
            .enumerate()
        {
            assert!(l.allow(r, t + 700 + i as u64));
        }
        assert!(!l.allow("error:e", t + 800), "six an hour");
        assert!(l.allow("error:e", t + 3600), "the first one aged out");
        // Survives a restart.
        let mut l2 = Limiter::parse(&l.text());
        assert!(!l2.allow("error:e", t + 3601));
        // A clock that jumped back doesn't block for a day.
        assert!(l2.allow("error:e", t - 86_400));
    }

    #[test]
    fn triggers() {
        assert_eq!(clean_trigger("crash:livi.service"), "crash:livi");
        assert_eq!(clean_trigger(" key\n"), "key");
        assert_eq!(clean_trigger(""), "unknown");
        assert!(is_auto("error:can0-bus-off") && !is_auto("portal"));
        assert_eq!(group(31812), "31 812");
        assert_eq!(group(999), "999");
        assert_eq!(group(1_234_567), "1 234 567");
    }

    #[test]
    fn capture_into_a_temp_folder() {
        let t = tempfile::tempdir().unwrap();
        let cfg_file = t.path().join("carpid.toml");
        std::fs::write(&cfg_file, "[bus.ican]\niface='can0'\n").unwrap();
        let manifest = t.path().join("manifest.json");
        std::fs::write(
            &manifest,
            r#"{"version":"1.2.3","channel":"beta","commits":{"pq35-carpi":"0123456789abcdef"}}"#,
        )
        .unwrap();
        let setup = Setup {
            root: t.path().join("logs"),
            minutes: 5,
            max_pct: 25,
            configs: vec![
                (cfg_file, "carpid.toml".into()),
                (t.path().join("missing.json"), "livi-config.json".into()),
            ],
            state_dir: t.path().join("state"),
            manifest,
            system: false,
        };
        let at = Instant::now();
        let wall = SystemTime::UNIX_EPOCH + Duration::from_secs(1_762_596_000);
        let rx = |id| RxFrame {
            frame: Frame::new(id, &[1, 2]).unwrap(),
            at,
            wall,
        };
        let snap = Snapshot {
            buses: vec![
                ("can0".into(), vec![rx(0x65D), rx(0x351)]),
                ("obd".into(), vec![]),
            ],
            values: vec![
                Reading {
                    name: "clock.datetime".into(),
                    value: Some(carpi_proto::Value::Text("2026-11-08T10:00:00".into())),
                    age_ms: 200,
                },
                Reading {
                    name: "vehicle.odometer_km".into(),
                    value: Some(carpi_proto::Value::Num(238600.0)),
                    age_ms: 200,
                },
            ],
            bus_info: vec![],
        };
        let job = Job {
            setup: setup.clone(),
            trigger: "key".into(),
            snapshot: Some(snap),
            start: SystemTime::now() - Duration::from_secs(300),
            end: SystemTime::now(),
            note: None,
            carpid_up: Duration::from_secs(65),
        };
        let (meta, dir) = job.run().unwrap();
        assert_eq!(meta.id.number, 1);
        assert_eq!(meta.summary, "2 buses, 2 frames, journal 0 lines");
        let logs = Logs::new(&setup.root);
        let names: Vec<_> = logs.files(&meta.id).into_iter().map(|f| f.path).collect();
        assert_eq!(
            names,
            [
                "can-can0.log",
                "can-obd.log",
                "config/carpid.toml",
                "entry.json",
                "info.txt",
                "journal.txt",
                "values.json"
            ]
        );
        let saved = logs.meta(&meta.id).unwrap();
        assert!(saved.ended.is_some());
        assert_eq!(saved.trigger, "key");
        assert_eq!(saved.kind, Kind::Capture);
        let can = std::fs::read_to_string(dir.join("can-can0.log")).unwrap();
        assert_eq!(
            can,
            "(1762596000.000000) can0 65D#0102\n(1762596000.000000) can0 351#0102\n"
        );
        let info = std::fs::read_to_string(dir.join("info.txt")).unwrap();
        assert!(info.contains("trigger    key"), "{info}");
        assert!(info.contains("2026-11-08T10:00:00 (200 ms old)"), "{info}");
        assert!(info.contains("238600 km (200 ms old)"), "{info}");
        assert!(
            info.contains("1.2.3 (beta), pq35-carpi 0123456789"),
            "{info}"
        );
        let values: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("values.json")).unwrap()).unwrap();
        assert_eq!(values["values"][1]["age_ms"], 200);

        // A crash capture: no CAN, said so.
        let job = Job {
            setup,
            trigger: "crash:carpid".into(),
            snapshot: None,
            start: SystemTime::now() - Duration::from_secs(300),
            end: SystemTime::now(),
            note: Some("previous run pid 42".into()),
            carpid_up: Duration::ZERO,
        };
        let (meta, dir) = job.run().unwrap();
        assert_eq!(meta.id.number, 2);
        assert_eq!(meta.summary, "no CAN, journal 0 lines");
        let info = std::fs::read_to_string(dir.join("info.txt")).unwrap();
        assert!(info.contains("No CAN in this capture."), "{info}");
        assert!(info.contains("previous run pid 42"), "{info}");
    }
}
