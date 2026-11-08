//! Did the last carpid run end cleanly?
//!
//! While carpid runs, `~/.local/state/carpi/carpid.running` holds its pid,
//! the boot id, and a heartbeat rewritten every [`BEAT`]. A clean stop
//! (SIGTERM, Ctrl-C) removes it; a panic appends `panic=...` first. Found
//! at startup from the same boot, it means carpid crashed: the previous run
//! ended within a beat of the last heartbeat. Found from an earlier boot, the
//! Pi lost power, which isn't a carpid crash.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const BEAT: Duration = Duration::from_secs(60);
const FILE: &str = "carpid.running";

/// carpid's own state: `$XDG_STATE_HOME/carpi`, else `~/.local/state/carpi`.
pub fn state_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("XDG_STATE_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(d).join("carpi");
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(".local/state/carpi")
}

#[derive(Clone, Debug, PartialEq)]
pub enum Previous {
    Clean,
    /// The marker is from another boot: power was cut.
    PowerLoss,
    Crashed {
        pid: Option<u32>,
        /// Roughly when it ended: the last heartbeat plus one beat, at most now.
        end: SystemTime,
        panic: Option<String>,
    },
}

/// Reads an old marker. `boot` is this boot's id (`None`: unknown, taken as
/// the same boot).
pub fn judge(marker: Option<&str>, boot: Option<&str>, now: SystemTime) -> Previous {
    let Some(text) = marker else {
        return Previous::Clean;
    };
    let get = |k: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(k)?.strip_prefix('='))
            .map(str::trim)
    };
    if let (Some(then), Some(now_boot)) = (get("boot"), boot)
        && then != now_boot
    {
        return Previous::PowerLoss;
    }
    let secs = |k: &str| get(k).and_then(|v| v.parse::<u64>().ok());
    let end = secs("beat")
        .or_else(|| secs("started"))
        .map(|s| UNIX_EPOCH + Duration::from_secs(s) + BEAT)
        .map_or(now, |e| e.min(now));
    Previous::Crashed {
        pid: get("pid").and_then(|v| v.parse().ok()),
        end,
        panic: get("panic").map(String::from),
    }
}

pub fn boot_id() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub struct RunMark {
    path: PathBuf,
    started: u64,
    boot: Option<String>,
}

fn epoch(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl RunMark {
    /// Judges the last run and marks this one as running.
    pub fn start(dir: &Path) -> (Self, Previous) {
        let path = dir.join(FILE);
        let boot = boot_id();
        let old = std::fs::read_to_string(&path).ok();
        let prev = judge(old.as_deref(), boot.as_deref(), SystemTime::now());
        let mark = RunMark {
            path,
            started: epoch(SystemTime::now()),
            boot,
        };
        if let Err(e) = std::fs::create_dir_all(dir).and_then(|_| mark.write()) {
            tracing::warn!("{}: {e}; crashes won't be noticed", mark.path.display());
        }
        (mark, prev)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn write(&self) -> std::io::Result<()> {
        let mut text = format!(
            "pid={}\nstarted={}\nbeat={}\n",
            std::process::id(),
            self.started,
            epoch(SystemTime::now())
        );
        if let Some(b) = &self.boot {
            text.push_str(&format!("boot={b}\n"));
        }
        std::fs::write(&self.path, text)
    }

    /// Heartbeat. Small write; call off the event loop.
    pub fn beat(&self) {
        let _ = self.write();
    }

    /// Clean stop.
    pub fn clean(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Panics are written into the marker. One on another thread than main
/// (a tokio task: carpid keeps running) also asks for an `error:panic`
/// capture through `notify`; one on main ends carpid, and the next start
/// captures it as `crash:carpid`.
pub fn install_panic_hook(path: PathBuf, notify: tokio::sync::mpsc::UnboundedSender<String>) {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let line = format!("{info}").replace('\n', " ");
        let line: String = line.chars().take(400).collect();
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&path) {
            use std::io::Write;
            let _ = writeln!(f, "panic={} thread {}", line, thread.name().unwrap_or("?"));
        }
        if thread.name() != Some("main") {
            let _ = notify.send("error:panic".into());
        }
        default(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unclean_exit_detection() {
        let now = UNIX_EPOCH + Duration::from_secs(10_000);
        assert_eq!(judge(None, Some("b1"), now), Previous::Clean);
        let text = "pid=42\nstarted=9000\nbeat=9500\nboot=b1\npanic=boom at x.rs:1 thread tokio\n";
        assert_eq!(
            judge(Some(text), Some("b1"), now),
            Previous::Crashed {
                pid: Some(42),
                end: UNIX_EPOCH + Duration::from_secs(9560),
                panic: Some("boom at x.rs:1 thread tokio".into()),
            }
        );
        assert_eq!(judge(Some(text), Some("b2"), now), Previous::PowerLoss);
        // A heartbeat just now: the end is now, not in the future.
        let fresh = "pid=1\nstarted=9990\nbeat=9990\nboot=b1\n";
        let Previous::Crashed { end, panic, .. } = judge(Some(fresh), None, now) else {
            panic!("crash expected");
        };
        assert_eq!(end, now);
        assert_eq!(panic, None);
    }

    #[test]
    fn marker_round_trip() {
        let t = tempfile::tempdir().unwrap();
        let (m, prev) = RunMark::start(t.path());
        assert_eq!(prev, Previous::Clean);
        // Still there at the next start: a crash (same boot).
        let (m2, prev) = RunMark::start(t.path());
        assert!(matches!(prev, Previous::Crashed { .. }), "{prev:?}");
        m2.clean();
        assert!(!m.path().exists());
        let (_m3, prev) = RunMark::start(t.path());
        assert_eq!(prev, Previous::Clean);
    }
}
