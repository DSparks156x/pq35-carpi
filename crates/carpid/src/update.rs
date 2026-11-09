//! Update jobs: carpid runs `carpi-update` as a child and relays its JSON
//! lines as `Job` events (docs/DEPLOY.md sections 3 and 4).
//!
//! `check` and `fetch` need the internet, and LIVI holds the Wi-Fi as its
//! hotspot. The order:
//! 1. ask LIVI to suspend the AP (skipped when already online, e.g. Ethernet);
//! 2. wait up to 30 s for NetworkManager to join a known network, else
//!    resume and report;
//! 3. run `carpi-update check|fetch`;
//! 4. resume the AP whatever happened; a 3 min hard timeout around 2-3
//!    makes sure of it;
//! 5. with `and_apply`, run `carpi-update apply`, offline.
//!
//! `apply` restarts carpid, so it runs outside carpid's cgroup
//! (`systemd-run --user`) and outlives this process; its result is in
//! `carpi-update status` afterwards.

#![cfg_attr(not(unix), allow(dead_code))]

use crate::commands::CommandHub;
use carpi_proto::{ApAction, Command, JobEvent, UpdateAction};
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;

pub trait Bridge: Send + Sync {
    fn ap(&self, action: ApAction) -> impl Future<Output = Result<(), String>> + Send;
}

pub trait Network: Send + Sync {
    /// NetworkManager has a full connection.
    fn online(&self) -> impl Future<Output = bool> + Send;
}

pub trait Updater: Send + Sync {
    /// Runs `carpi-update <args>`, forwarding its events except the last;
    /// returns that `finished` message. `detach`: run outside carpid.
    fn run(
        &self,
        args: &[&str],
        detach: bool,
        events: &mpsc::UnboundedSender<JobEvent>,
    ) -> impl Future<Output = Result<String, String>> + Send;
}

#[derive(Clone, Copy)]
pub struct Timing {
    /// For NetworkManager to join a known network.
    pub network: Duration,
    pub poll: Duration,
    /// AP down for at most this long.
    pub hard: Duration,
}

pub const TIMING: Timing = Timing {
    network: Duration::from_secs(30),
    poll: Duration::from_secs(1),
    hard: Duration::from_secs(180),
};

fn stage(events: &mpsc::UnboundedSender<JobEvent>, stage: &str, message: impl Into<String>) {
    let _ = events.send(JobEvent::Stage {
        stage: stage.into(),
        message: message.into(),
    });
}

/// One job, start to end. Always ends with `Finished`.
pub async fn run_job(
    action: UpdateAction,
    and_apply: bool,
    bridge: &impl Bridge,
    net: &impl Network,
    updater: &impl Updater,
    t: Timing,
    events: &mpsc::UnboundedSender<JobEvent>,
) {
    let result = match action {
        UpdateAction::Status => updater.run(&["status"], false, events).await,
        UpdateAction::Apply => updater.run(&["apply"], true, events).await,
        UpdateAction::Check => online(bridge, net, updater, t, "check", events).await,
        UpdateAction::Fetch => match online(bridge, net, updater, t, "fetch", events).await {
            Ok(msg) if and_apply => {
                stage(events, "fetch", msg);
                updater.run(&["apply"], true, events).await
            }
            other => other,
        },
    };
    let (ok, message) = match result {
        Ok(m) => (true, m),
        Err(m) => (false, m),
    };
    let _ = events.send(JobEvent::Finished { ok, message });
}

/// Steps 1-4: `carpi-update <sub>` with the internet, AP back afterwards.
async fn online(
    bridge: &impl Bridge,
    net: &impl Network,
    updater: &impl Updater,
    t: Timing,
    sub: &str,
    events: &mpsc::UnboundedSender<JobEvent>,
) -> Result<String, String> {
    if net.online().await {
        stage(events, "network", "online already; the hotspot stays up");
        return run_bounded(updater, sub, t.hard, events).await;
    }
    stage(
        events,
        "ap",
        "suspending the hotspot; the phone link drops until it's back",
    );
    if let Err(e) = bridge.ap(ApAction::Suspend).await {
        // LIVI may not run; the AP may be down already. Try anyway.
        stage(events, "ap", format!("suspend: {e}"));
    }
    let result = tokio::time::timeout(t.hard, async {
        stage(events, "network", "waiting for a known Wi-Fi");
        let joined = tokio::time::timeout(t.network, async {
            while !net.online().await {
                tokio::time::sleep(t.poll).await;
            }
        })
        .await;
        if joined.is_err() {
            return Err(format!(
                "no known network within {} s; add one on the Car page",
                t.network.as_secs()
            ));
        }
        stage(events, "network", "online");
        updater.run(&[sub], false, events).await
    })
    .await
    .unwrap_or_else(|_| Err(format!("{sub} took over {} s; stopped", t.hard.as_secs())));
    stage(events, "ap", "restoring the hotspot");
    if let Err(e) = bridge.ap(ApAction::Resume).await {
        stage(events, "ap", format!("resume: {e}"));
    }
    result
}

async fn run_bounded(
    updater: &impl Updater,
    sub: &str,
    hard: Duration,
    events: &mpsc::UnboundedSender<JobEvent>,
) -> Result<String, String> {
    tokio::time::timeout(hard, updater.run(&[sub], false, events))
        .await
        .unwrap_or_else(|_| Err(format!("{sub} took over {} s; stopped", hard.as_secs())))
}

/// Bridge through LIVI's command connection.
pub struct Livi(pub Arc<CommandHub>);

impl Bridge for Livi {
    async fn ap(&self, action: ApAction) -> Result<(), String> {
        // Stopping or starting the AP unit takes a few seconds.
        self.0
            .send(Command::Ap { ap: action }, Duration::from_secs(30))
            .await
    }
}

/// `nmcli -t -f STATE general` says `connected` (not local or site only).
pub struct Nm;

impl Network for Nm {
    async fn online(&self) -> bool {
        tokio::process::Command::new("nmcli")
            .args(["-t", "-f", "STATE", "general"])
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output()
            .await
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "connected")
    }
}

/// The real `carpi-update`.
pub struct Child {
    pub program: PathBuf,
    /// carpid's config, which carries `[update]`.
    pub config: PathBuf,
}

impl Updater for Child {
    async fn run(
        &self,
        args: &[&str],
        detach: bool,
        events: &mpsc::UnboundedSender<JobEvent>,
    ) -> Result<String, String> {
        use tokio::io::AsyncBufReadExt;

        let mut cmd = if detach && cfg!(target_os = "linux") {
            let mut c = tokio::process::Command::new("systemd-run");
            c.args([
                "--user",
                "--collect",
                "--quiet",
                "--wait",
                "--pipe",
                "--unit=carpi-update-apply",
                "--",
            ])
            .arg(&self.program);
            c
        } else {
            tokio::process::Command::new(&self.program)
        };
        cmd.arg("--config")
            .arg(&self.config)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("{}: {e}", self.program.display()))?;
        let stdout = child.stdout.take().expect("piped");
        let mut lines = tokio::io::BufReader::new(stdout).lines();
        let mut finished = None;
        while let Ok(Some(line)) = lines.next_line().await {
            match serde_json::from_str::<JobEvent>(&line) {
                Ok(JobEvent::Finished { ok, message }) => finished = Some((ok, message)),
                Ok(e) => {
                    let _ = events.send(e);
                }
                Err(_) => tracing::info!("carpi-update: {line}"),
            }
        }
        let status = child.wait().await.map_err(|e| e.to_string())?;
        match finished {
            Some((true, m)) => Ok(m),
            Some((false, m)) => Err(m),
            None => Err(format!("carpi-update exited ({status}) without a result")),
        }
    }
}

/// One update job at a time, across connections.
pub struct Jobs {
    busy: AtomicBool,
    next: AtomicU32,
    pub hub: Arc<CommandHub>,
    pub updater: PathBuf,
    pub config: PathBuf,
}

impl Jobs {
    pub fn new(hub: Arc<CommandHub>, updater: PathBuf, config: PathBuf) -> Self {
        Jobs {
            busy: AtomicBool::new(false),
            next: AtomicU32::new(0),
            hub,
            updater,
            config,
        }
    }

    /// A job number, unique across update and diag jobs.
    pub fn next_job(&self) -> u32 {
        self.next.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Starts a job; its events go to `events` and it ends with `Finished`.
    /// Runs to the end even if the asking connection closes.
    pub fn start(
        self: &Arc<Self>,
        action: UpdateAction,
        and_apply: bool,
        events: mpsc::UnboundedSender<JobEvent>,
    ) -> Result<u32, String> {
        if self.busy.swap(true, Ordering::SeqCst) {
            return Err("an update or Wi-Fi job is running".into());
        }
        let job = self.next_job();
        let me = self.clone();
        tokio::spawn(async move {
            let updater = Child {
                program: me.updater.clone(),
                config: me.config.clone(),
            };
            run_job(
                action,
                and_apply,
                &Livi(me.hub.clone()),
                &Nm,
                &updater,
                TIMING,
                &events,
            )
            .await;
            me.busy.store(false, Ordering::SeqCst);
        });
        Ok(job)
    }
}

impl Jobs {
    /// A Wi-Fi scan or test: it takes the radio from the hotspot, so it
    /// shares the update's one-at-a-time.
    pub fn start_wifi(
        self: &Arc<Self>,
        action: carpi_proto::WifiAction,
        events: mpsc::UnboundedSender<JobEvent>,
    ) -> Result<u32, String> {
        use carpi_proto::WifiAction as W;
        if !matches!(action, W::Scan | W::Test { .. }) {
            return Err("not a Wi-Fi job".into());
        }
        if self.busy.swap(true, Ordering::SeqCst) {
            return Err("an update or Wi-Fi job is running".into());
        }
        let job = self.next_job();
        let me = self.clone();
        tokio::spawn(async move {
            let bridge = Livi(me.hub.clone());
            match action {
                W::Test { ssid } => {
                    crate::wifi::test(
                        &crate::wifi::Real,
                        &bridge,
                        &ssid,
                        crate::wifi::TIMING,
                        &events,
                    )
                    .await
                }
                _ => {
                    crate::wifi::scan(&crate::wifi::Real, &bridge, crate::wifi::TIMING, &events)
                        .await
                }
            }
            me.busy.store(false, Ordering::SeqCst);
        });
        Ok(job)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default, Clone)]
    struct Log(Arc<Mutex<Vec<String>>>);

    impl Log {
        fn push(&self, s: impl Into<String>) {
            self.0.lock().unwrap().push(s.into());
        }
        fn take(&self) -> Vec<String> {
            std::mem::take(&mut self.0.lock().unwrap())
        }
    }

    struct FakeBridge(Log);

    impl Bridge for FakeBridge {
        async fn ap(&self, action: ApAction) -> Result<(), String> {
            self.0.push(format!("ap {action:?}"));
            Ok(())
        }
    }

    /// Online after `after` checks; `None` never; `Some(0)` already.
    struct FakeNet {
        log: Log,
        after: Option<u32>,
        checks: Mutex<u32>,
    }

    impl Network for FakeNet {
        async fn online(&self) -> bool {
            let mut n = self.checks.lock().unwrap();
            let on = self.after.is_some_and(|a| *n >= a);
            *n += 1;
            if on {
                self.log.push("online");
            }
            on
        }
    }

    #[derive(Clone, Copy)]
    enum Behave {
        Ok,
        Fail,
        Hang,
    }

    struct FakeUpdater {
        log: Log,
        fetch: Behave,
    }

    impl Updater for FakeUpdater {
        async fn run(
            &self,
            args: &[&str],
            detach: bool,
            events: &mpsc::UnboundedSender<JobEvent>,
        ) -> Result<String, String> {
            let what = args.join(" ");
            self.log.push(format!(
                "run {what}{}",
                if detach { " detached" } else { "" }
            ));
            let _ = events.send(JobEvent::Stage {
                stage: what.clone(),
                message: String::new(),
            });
            match (what.as_str(), self.fetch) {
                ("fetch" | "check", Behave::Fail) => Err("404".into()),
                ("fetch" | "check", Behave::Hang) => {
                    std::future::pending::<()>().await;
                    unreachable!()
                }
                _ => Ok(format!("{what} done")),
            }
        }
    }

    async fn job(
        action: UpdateAction,
        and_apply: bool,
        net_after: Option<u32>,
        fetch: Behave,
    ) -> (Vec<String>, Vec<JobEvent>) {
        let log = Log::default();
        let (tx, mut rx) = mpsc::unbounded_channel();
        run_job(
            action,
            and_apply,
            &FakeBridge(log.clone()),
            &FakeNet {
                log: log.clone(),
                after: net_after,
                checks: Mutex::new(0),
            },
            &FakeUpdater {
                log: log.clone(),
                fetch,
            },
            TIMING,
            &tx,
        )
        .await;
        drop(tx);
        let mut events = Vec::new();
        while let Some(e) = rx.recv().await {
            events.push(e);
        }
        (log.take(), events)
    }

    fn finished(events: &[JobEvent]) -> (bool, String) {
        match events.last() {
            Some(JobEvent::Finished { ok, message }) => (*ok, message.clone()),
            other => panic!("last event {other:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn fetch_order_is_suspend_wait_fetch_resume_apply() {
        let (log, events) = job(UpdateAction::Fetch, true, Some(3), Behave::Ok).await;
        assert_eq!(
            log,
            [
                "ap Suspend",
                "online",
                "run fetch",
                "ap Resume",
                "run apply detached"
            ]
        );
        assert_eq!(finished(&events), (true, "apply done".into()));
        // Only one Finished, at the end.
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, JobEvent::Finished { .. }))
                .count(),
            1
        );
    }

    #[tokio::test(start_paused = true)]
    async fn fetch_alone_does_not_apply() {
        let (log, _) = job(UpdateAction::Fetch, false, Some(1), Behave::Ok).await;
        assert_eq!(log, ["ap Suspend", "online", "run fetch", "ap Resume"]);
    }

    #[tokio::test(start_paused = true)]
    async fn ap_resumes_when_fetch_fails() {
        let (log, events) = job(UpdateAction::Fetch, true, Some(1), Behave::Fail).await;
        assert_eq!(log, ["ap Suspend", "online", "run fetch", "ap Resume"]);
        assert_eq!(finished(&events), (false, "404".into()));
    }

    #[tokio::test(start_paused = true)]
    async fn ap_resumes_when_no_network_appears() {
        let start = tokio::time::Instant::now();
        let (log, events) = job(UpdateAction::Check, false, None, Behave::Ok).await;
        assert_eq!(log, ["ap Suspend", "ap Resume"]);
        let (ok, msg) = finished(&events);
        assert!(!ok && msg.contains("no known network"), "{msg}");
        let took = start.elapsed();
        assert!(took >= TIMING.network && took < TIMING.network + Duration::from_secs(2));
    }

    #[tokio::test(start_paused = true)]
    async fn ap_resumes_at_the_hard_timeout() {
        let start = tokio::time::Instant::now();
        let (log, events) = job(UpdateAction::Fetch, true, Some(1), Behave::Hang).await;
        assert_eq!(log, ["ap Suspend", "online", "run fetch", "ap Resume"]);
        assert!(!finished(&events).0);
        assert_eq!(start.elapsed(), TIMING.hard);
    }

    #[tokio::test(start_paused = true)]
    async fn online_already_keeps_the_ap_up() {
        let (log, _) = job(UpdateAction::Fetch, true, Some(0), Behave::Ok).await;
        assert_eq!(log, ["online", "run fetch", "run apply detached"]);
    }

    #[tokio::test(start_paused = true)]
    async fn offline_actions_skip_the_network() {
        let (log, _) = job(UpdateAction::Status, false, None, Behave::Ok).await;
        assert_eq!(log, ["run status"]);
        let (log, _) = job(UpdateAction::Apply, false, None, Behave::Ok).await;
        assert_eq!(log, ["run apply detached"]);
    }
}
