//! The disks: where the system runs from, and Move to NVMe (docs/IMAGE-TOOL.md, NVMe and
//! post-setup).
//!
//! The move itself runs as root: os/move-nvme.sh in the system unit `carpi-move-nvme.service`,
//! which the car user may start through a polkit rule (os/profile.sh). carpid starts it
//! (`--no-block`) and follows the progress file the script writes, one line each:
//!
//! ```text
//! start|<unix time>
//! stage|<name>|<message>
//! progress|<stage>|<done>|<total>
//! finished|ok|<message>      finished|fail|<message>
//! ```
//!
//! A file whose `start` is older than the job is the last run's and is ignored. If the unit
//! stops without a `finished` line, the job ends failed and points at the unit's journal.

#![cfg_attr(not(unix), allow(dead_code))]

use carpi_proto::{JobEvent, NvmeDisk, RootDisk, StorageStatus};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

pub const UNIT: &str = "carpi-move-nvme.service";
pub const PROGRESS: &str = "/run/carpi-move-nvme/progress";
/// The unit os/install.sh puts in; without it there's nothing to start.
pub const UNIT_FILE: &str = "/etc/systemd/system/carpi-move-nvme.service";
/// Room the boot partition takes, with the script's margin on top of what's in use.
const BOOT_BYTES: u64 = 512 << 20;

pub trait Sys: Send + Sync {
    /// Runs `prog args`; stdout, or the error text.
    fn run(&self, prog: &str, args: &[&str])
    -> impl Future<Output = Result<String, String>> + Send;
    /// A file's text; `None` when it's missing.
    fn read(&self, path: &Path) -> Option<String>;
    fn exists(&self, path: &Path) -> bool;
}

/// The real machine.
pub struct Real;

impl Sys for Real {
    async fn run(&self, prog: &str, args: &[&str]) -> Result<String, String> {
        let out = tokio::time::timeout(
            Duration::from_secs(20),
            tokio::process::Command::new(prog)
                .args(args)
                .stdin(std::process::Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| format!("{prog} took over 20 s"))?
        .map_err(|e| format!("{prog}: {e}"))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            Err(if err.is_empty() {
                format!("{prog} failed ({})", out.status)
            } else {
                err
            })
        }
    }

    fn read(&self, path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
}

/// What kind of disk a device is on, by its name.
pub fn root_disk(dev: &str) -> RootDisk {
    let name = dev.trim_start_matches("/dev/");
    if name.starts_with("mmcblk") {
        RootDisk::Sd
    } else if name.starts_with("nvme") {
        RootDisk::Nvme
    } else if name.starts_with("sd") {
        RootDisk::Usb
    } else {
        RootDisk::Other
    }
}

/// The first NVMe disk in `lsblk -J -b -d -o NAME,SIZE,MODEL,TYPE`. Older lsblk prints sizes
/// as strings; both are taken.
pub fn parse_lsblk(json: &str) -> Option<NvmeDisk> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    v.get("blockdevices")?.as_array()?.iter().find_map(|d| {
        let name = d.get("name")?.as_str()?;
        if !name.starts_with("nvme") || d.get("type").and_then(|t| t.as_str()) != Some("disk") {
            return None;
        }
        let size = match d.get("size")? {
            serde_json::Value::Number(n) => n.as_u64()?,
            serde_json::Value::String(s) => s.trim().parse().ok()?,
            _ => return None,
        };
        let model = d
            .get("model")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        Some(NvmeDisk {
            path: format!("/dev/{name}"),
            model,
            size,
        })
    })
}

/// `df -B1 --output=used <paths>`: the used bytes, added up.
pub fn parse_df(text: &str) -> u64 {
    text.lines()
        .skip(1)
        .filter_map(|l| l.trim().parse::<u64>().ok())
        .sum()
}

/// Where the system runs from, the NVMe, and whether a move can start.
pub async fn status(sys: &impl Sys) -> StorageStatus {
    let root = sys
        .run("findmnt", &["-n", "-o", "SOURCE", "/"])
        .await
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let nvme = sys
        .run("lsblk", &["-J", "-b", "-d", "-o", "NAME,SIZE,MODEL,TYPE"])
        .await
        .ok()
        .and_then(|j| parse_lsblk(&j));
    let used = sys
        .run("df", &["-B1", "--output=used", "/", "/boot/firmware"])
        .await
        .map(|t| parse_df(&t))
        .unwrap_or(0);
    let root_disk = root_disk(&root);
    let blocked = match &nvme {
        None => Some("No NVMe drive found".to_string()),
        Some(_) if root_disk == RootDisk::Nvme => Some("Already running from the NVMe".into()),
        Some(n) if used > 0 && n.size < used + BOOT_BYTES => Some(format!(
            "The NVMe is smaller than what's in use ({})",
            gb(used)
        )),
        Some(_) if !sys.exists(Path::new(UNIT_FILE)) => {
            Some("The move isn't installed: run os/install.sh again".into())
        }
        Some(_) => None,
    };
    StorageStatus {
        root,
        root_disk,
        nvme,
        used,
        blocked,
    }
}

fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

/// The progress file so far, as job events. `None` while it isn't this run's: no `start`
/// line yet, or one from before `since` (unix seconds; a few seconds' slack). Only whole lines
/// count, so a line half written isn't read.
pub fn parse_progress(text: &str, since: u64) -> Option<Vec<JobEvent>> {
    let whole = &text[..text.rfind('\n').map_or(0, |i| i + 1)];
    let mut lines = whole.lines();
    let start: u64 = lines.next()?.strip_prefix("start|")?.trim().parse().ok()?;
    if start + 5 < since {
        return None;
    }
    let mut out = Vec::new();
    for line in lines {
        let Some((kind, rest)) = line.split_once('|') else {
            continue;
        };
        // The message is the last field and may hold anything.
        let (a, b) = rest.split_once('|').unwrap_or((rest, ""));
        let event = match kind {
            "stage" => JobEvent::Stage {
                stage: a.into(),
                message: b.into(),
            },
            "progress" => {
                let (done, total) = b.split_once('|').unwrap_or((b, ""));
                let Ok(done) = done.trim().parse() else {
                    continue;
                };
                JobEvent::Progress {
                    stage: a.into(),
                    done,
                    total: total.trim().parse().ok(),
                }
            }
            "finished" => JobEvent::Finished {
                ok: a == "ok",
                message: b.into(),
            },
            _ => continue,
        };
        out.push(event);
    }
    Some(out)
}

#[derive(Clone, Copy)]
pub struct Timing {
    pub poll: Duration,
    /// For the script's `start` line after asking systemd.
    pub start: Duration,
    /// No new line for this long: stop watching.
    pub stall: Duration,
}

pub const TIMING: Timing = Timing {
    poll: Duration::from_secs(1),
    start: Duration::from_secs(30),
    stall: Duration::from_secs(20 * 60),
};

fn now_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn finish(events: &mpsc::UnboundedSender<JobEvent>, ok: bool, message: impl Into<String>) {
    let _ = events.send(JobEvent::Finished {
        ok,
        message: message.into(),
    });
}

/// The move job: checks, starts the unit, relays the script's progress. Ends with `Finished`;
/// on success the script restarts the Pi a few seconds later.
pub async fn move_job(
    sys: &impl Sys,
    progress: &Path,
    t: Timing,
    events: &mpsc::UnboundedSender<JobEvent>,
) {
    let st = status(sys).await;
    if let Some(why) = st.blocked {
        return finish(events, false, why);
    }
    let since = now_s();
    let _ = events.send(JobEvent::Stage {
        stage: "start".into(),
        message: "Starting".into(),
    });
    if let Err(e) = sys.run("systemctl", &["--no-block", "start", UNIT]).await {
        return finish(events, false, format!("couldn't start the move: {e}"));
    }
    let failed = format!("The move stopped without finishing; see journalctl -u {UNIT}");
    let begun = tokio::time::Instant::now();
    let mut last_new = begun;
    let mut sent = 0;
    // Relays new lines; true once `Finished` went out.
    let relay = |sent: &mut usize| -> Option<bool> {
        let evs = parse_progress(&sys.read(progress)?, since)?;
        let mut done = false;
        for e in evs.into_iter().skip(*sent) {
            *sent += 1;
            done |= matches!(e, JobEvent::Finished { .. });
            let _ = events.send(e);
            if done {
                break;
            }
        }
        Some(done)
    };
    loop {
        tokio::time::sleep(t.poll).await;
        let before = sent;
        let seen = relay(&mut sent);
        if seen == Some(true) {
            return;
        }
        if sent != before {
            last_new = tokio::time::Instant::now();
        }
        let state = sys
            .run(
                "systemctl",
                &["show", "--property=ActiveState", "--value", UNIT],
            )
            .await
            .unwrap_or_else(|e| e);
        let running = matches!(state.trim(), "active" | "activating" | "reloading");
        if !running && (seen.is_some() || begun.elapsed() >= t.start) {
            // It may have written its last line just before stopping.
            if relay(&mut sent) == Some(true) {
                return;
            }
            return finish(events, false, failed);
        }
        if last_new.elapsed() >= t.stall {
            return finish(
                events,
                false,
                format!(
                    "No word from the move for {} min; see journalctl -u {UNIT}",
                    t.stall.as_secs() / 60
                ),
            );
        }
    }
}

/// Where the script writes its progress.
pub fn progress_path() -> PathBuf {
    PathBuf::from(PROGRESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    const LSBLK: &str = r#"{"blockdevices":[
        {"name":"mmcblk0","size":31914983424,"model":null,"type":"disk"},
        {"name":"nvme0n1","size":"500107862016","model":"WD Blue SN580 500GB  ","type":"disk"}
    ]}"#;

    #[test]
    fn finds_the_nvme_and_the_root() {
        let n = parse_lsblk(LSBLK).unwrap();
        assert_eq!(n.path, "/dev/nvme0n1");
        assert_eq!(n.model, "WD Blue SN580 500GB");
        assert_eq!(n.size, 500_107_862_016);
        assert!(
            parse_lsblk(r#"{"blockdevices":[{"name":"sda","size":1,"type":"disk"}]}"#).is_none()
        );
        assert!(parse_lsblk("nope").is_none());
        assert_eq!(root_disk("/dev/mmcblk0p2"), RootDisk::Sd);
        assert_eq!(root_disk("/dev/nvme0n1p2"), RootDisk::Nvme);
        assert_eq!(root_disk("/dev/sda2"), RootDisk::Usb);
        assert_eq!(root_disk(""), RootDisk::Other);
        assert_eq!(parse_df("  Used\n5000000000\n 70000000\n"), 5_070_000_000);
    }

    #[test]
    fn progress_is_this_runs_and_whole_lines_only() {
        let text = "start|1000\nstage|copy|Copying the system\nprogress|copy|42|100\nprogress|co";
        let evs = parse_progress(text, 1002).unwrap();
        assert_eq!(
            evs,
            [
                JobEvent::Stage {
                    stage: "copy".into(),
                    message: "Copying the system".into()
                },
                JobEvent::Progress {
                    stage: "copy".into(),
                    done: 42,
                    total: Some(100)
                }
            ]
        );
        assert!(parse_progress(text, 2000).is_none(), "last run's file");
        assert!(parse_progress("", 0).is_none());
        assert!(parse_progress("start|10", 0).is_none(), "half a line");
        let end = parse_progress("start|10\nfinished|fail|no room | really\n", 10).unwrap();
        assert_eq!(
            end,
            [JobEvent::Finished {
                ok: false,
                message: "no room | really".into()
            }]
        );
    }

    /// A machine with an NVMe, running from the SD; the script's progress is whatever
    /// `file` holds, the unit's state whatever `state` says.
    #[derive(Clone)]
    struct Fake {
        calls: Arc<Mutex<Vec<String>>>,
        file: Arc<Mutex<Option<String>>>,
        state: Arc<Mutex<&'static str>>,
        root: &'static str,
    }

    impl Fake {
        fn new() -> Self {
            Fake {
                calls: Arc::default(),
                file: Arc::default(),
                state: Arc::new(Mutex::new("activating")),
                root: "/dev/mmcblk0p2",
            }
        }
    }

    impl Sys for Fake {
        async fn run(&self, prog: &str, args: &[&str]) -> Result<String, String> {
            let line = format!("{prog} {}", args.join(" "));
            self.calls.lock().unwrap().push(line.clone());
            if line == format!("systemctl is-active {UNIT}")
                && *self.state.lock().unwrap() != "active"
            {
                return Err(self.state.lock().unwrap().to_string());
            }
            Ok(match prog {
                "findmnt" => format!("{}\n", self.root),
                "lsblk" => LSBLK.into(),
                "df" => "Used\n6000000000\n70000000\n".into(),
                "systemctl" if line.contains("show --property=ActiveState --value") => {
                    format!("{}\n", self.state.lock().unwrap())
                }
                _ => String::new(),
            })
        }
        fn read(&self, _: &Path) -> Option<String> {
            self.file.lock().unwrap().clone()
        }
        fn exists(&self, _: &Path) -> bool {
            true
        }
    }

    const T: Timing = Timing {
        poll: Duration::from_millis(10),
        start: Duration::from_millis(100),
        stall: Duration::from_secs(60),
    };

    async fn drain(mut rx: mpsc::UnboundedReceiver<JobEvent>) -> Vec<JobEvent> {
        let mut out = Vec::new();
        while let Some(e) = rx.recv().await {
            out.push(e);
        }
        out
    }

    #[tokio::test]
    async fn status_says_whether_a_move_can_start() {
        let mut f = Fake::new();
        let s = status(&f).await;
        assert_eq!(s.root_disk, RootDisk::Sd);
        assert_eq!(s.used, 6_070_000_000);
        assert_eq!(s.blocked, None);
        f.root = "/dev/nvme0n1p2";
        assert_eq!(
            status(&f).await.blocked.as_deref(),
            Some("Already running from the NVMe")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_move_relays_the_scripts_lines() {
        let f = Fake::new();
        *f.file.lock().unwrap() = Some(format!(
            "start|{}\nstage|copy|Copying the system\nprogress|copy|50|100\nfinished|ok|Copied\n",
            now_s()
        ));
        let (tx, rx) = mpsc::unbounded_channel();
        move_job(&f, Path::new("p"), T, &tx).await;
        drop(tx);
        let evs = drain(rx).await;
        assert!(
            f.calls
                .lock()
                .unwrap()
                .iter()
                .any(|c| c == "systemctl --no-block start carpi-move-nvme.service")
        );
        assert_eq!(evs.len(), 4, "{evs:?}");
        assert_eq!(
            evs.last(),
            Some(&JobEvent::Finished {
                ok: true,
                message: "Copied".into()
            })
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_activating_oneshot_is_not_mistaken_for_a_stopped_unit() {
        let f = Fake::new();
        let (tx, rx) = mpsc::unbounded_channel();
        let job = tokio::spawn({
            let f = f.clone();
            async move { move_job(&f, Path::new("p"), T, &tx).await }
        });

        tokio::task::yield_now().await;
        tokio::time::advance(T.poll).await;
        tokio::task::yield_now().await;
        let calls = f.calls.lock().unwrap().clone();
        assert!(
            calls.iter().any(|c| {
                c == "systemctl show --property=ActiveState --value carpi-move-nvme.service"
            }),
            "the active state query succeeds while the oneshot is activating: {calls:?}"
        );
        assert!(
            !calls
                .iter()
                .any(|c| c == "systemctl is-active carpi-move-nvme.service"),
            "is-active exits unsuccessfully for activating oneshots"
        );

        *f.file.lock().unwrap() = Some(format!("start|{}\nfinished|ok|Copied\n", now_s()));
        tokio::time::advance(T.poll).await;
        tokio::task::yield_now().await;
        job.await.unwrap();
        drop(rx);
    }

    #[tokio::test(start_paused = true)]
    async fn a_unit_that_stops_silently_fails_the_job() {
        let f = Fake::new();
        // Last run's file, and the unit never gets going.
        *f.file.lock().unwrap() = Some("start|10\nfinished|ok|old\n".into());
        *f.state.lock().unwrap() = "failed";
        let (tx, rx) = mpsc::unbounded_channel();
        move_job(&f, Path::new("p"), T, &tx).await;
        drop(tx);
        let evs = drain(rx).await;
        assert!(
            matches!(evs.last(), Some(JobEvent::Finished { ok: false, message }) if message.contains("journalctl")),
            "{evs:?}"
        );
        assert!(
            !evs.iter()
                .any(|e| matches!(e, JobEvent::Finished { ok: true, .. }))
        );
        assert!(
            f.calls.lock().unwrap().iter().any(|c| {
                c == "systemctl show --property=ActiveState --value carpi-move-nvme.service"
            }),
            "a genuine failed state is still detected"
        );
    }

    #[tokio::test]
    async fn a_blocked_move_never_starts_the_unit() {
        let mut f = Fake::new();
        f.root = "/dev/nvme0n1p2";
        let (tx, rx) = mpsc::unbounded_channel();
        move_job(&f, Path::new("p"), T, &tx).await;
        drop(tx);
        assert!(matches!(
            drain(rx).await.last(),
            Some(JobEvent::Finished { ok: false, .. })
        ));
        assert!(!f.calls.lock().unwrap().iter().any(|c| c.contains("start")));
    }
}
