//! Super bad errors that make an automatic capture (`error:<what>`):
//! - a CAN controller going bus-off, its interface vanishing, or a storm of
//!   error-passive transitions, from `ip -details -statistics -json link`
//!   every [`CAN_POLL`] (error frames never reach carpid's sockets);
//! - an update that rolled back, from `/opt/carpi/state.json`.
//!
//! Crashes are elsewhere: carpid's own in `runmark`, LIVI's through
//! `carpi-capture@.service`.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const CAN_POLL: Duration = Duration::from_secs(5);
pub const ROLLBACK_POLL: Duration = Duration::from_secs(60);

/// This many error-passive transitions within [`STORM_WINDOW`] is a storm.
const STORM_COUNT: u64 = 10;
const STORM_WINDOW: Duration = Duration::from_secs(60);

/// What `ip` says about one CAN interface.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct CanStats {
    pub state: Option<String>,
    pub bus_off: u64,
    pub error_passive: u64,
}

/// `ip -details -statistics -json link show dev X`. Keys are looked up
/// anywhere under `linkinfo` (`info_data.state`, `info_xstats.bus_off`).
pub fn parse_ip(json: &str) -> Option<CanStats> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let link = v.get(0)?.get("linkinfo")?;
    fn find<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
        match v {
            serde_json::Value::Object(m) => {
                m.get(key).or_else(|| m.values().find_map(|c| find(c, key)))
            }
            _ => None,
        }
    }
    let n = |k: &str| find(link, k).and_then(|x| x.as_u64()).unwrap_or(0);
    Some(CanStats {
        state: find(link, "state")
            .and_then(|s| s.as_str())
            .map(String::from),
        bus_off: n("bus_off"),
        error_passive: n("error_passive"),
    })
}

/// Turns successive polls of one interface into fault names.
#[derive(Default)]
pub struct CanWatch {
    last: Option<CanStats>,
    /// Seen present since it was last reported gone.
    present: bool,
    /// (when, error-passive transitions since the poll before).
    passive: VecDeque<(Instant, u64)>,
}

impl CanWatch {
    /// `stats`: `None` when the interface doesn't exist.
    pub fn step(&mut self, now: Instant, stats: Option<CanStats>) -> Option<&'static str> {
        let Some(s) = stats else {
            self.last = None;
            if std::mem::take(&mut self.present) {
                return Some("gone");
            }
            return None;
        };
        self.present = true;
        let prev = self.last.replace(s.clone())?;
        let off_now = s.state.as_deref() == Some("BUS-OFF");
        let off_before = prev.state.as_deref() == Some("BUS-OFF");
        if s.bus_off > prev.bus_off || (off_now && !off_before) {
            return Some("bus-off");
        }
        let added = s.error_passive.saturating_sub(prev.error_passive);
        if added > 0 {
            self.passive.push_back((now, added));
        }
        while self
            .passive
            .front()
            .is_some_and(|(t, _)| now.saturating_duration_since(*t) > STORM_WINDOW)
        {
            self.passive.pop_front();
        }
        if self.passive.iter().map(|(_, n)| n).sum::<u64>() >= STORM_COUNT {
            self.passive.clear();
            return Some("error-passive");
        }
        None
    }
}

/// `last_apply` in carpi-update's state file.
pub fn rolled_back(state_json: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(state_json)
        .ok()
        .and_then(|v| v.get("last_apply")?.as_str().map(|s| s == "rolled_back"))
        .unwrap_or(false)
}

/// Capture a rollback once: the seen flag stays until a later apply
/// changes `last_apply`.
pub fn rollback_step(rolled: bool, seen: bool) -> (bool, bool) {
    match (rolled, seen) {
        (true, false) => (true, true),
        (true, true) => (false, true),
        (false, _) => (false, false),
    }
}

const SEEN: &str = "rollback.seen";

/// Checks the update state once; true if a rollback is new.
pub fn check_rollback(state_file: &Path, state_dir: &Path) -> bool {
    let rolled = std::fs::read_to_string(state_file).is_ok_and(|t| rolled_back(&t));
    let seen_path = state_dir.join(SEEN);
    let (capture, seen) = rollback_step(rolled, seen_path.exists());
    if seen {
        let _ = std::fs::create_dir_all(state_dir).and_then(|_| std::fs::write(&seen_path, ""));
    } else {
        let _ = std::fs::remove_file(&seen_path);
    }
    capture
}

/// Watches the interfaces and the update state; asks for captures.
#[cfg(target_os = "linux")]
pub fn spawn(ifaces: Vec<String>, captures: crate::capture::Captures, state_dir: PathBuf) {
    tokio::spawn(async move {
        let state_file = PathBuf::from("/opt/carpi/state.json");
        let mut watch: Vec<CanWatch> = ifaces.iter().map(|_| CanWatch::default()).collect();
        let mut tick = tokio::time::interval(CAN_POLL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last_rollback: Option<Instant> = None;
        loop {
            tick.tick().await;
            let now = Instant::now();
            for (iface, w) in ifaces.iter().zip(&mut watch) {
                let stats = ip_stats(iface).await;
                if let Some(what) = w.step(now, stats) {
                    tracing::warn!("{iface}: {what}");
                    captures.auto(format!("error:{iface}-{what}"));
                }
            }
            if last_rollback.is_none_or(|t| now.duration_since(t) >= ROLLBACK_POLL) {
                last_rollback = Some(now);
                let (f, d) = (state_file.clone(), state_dir.clone());
                let new = tokio::task::spawn_blocking(move || check_rollback(&f, &d))
                    .await
                    .unwrap_or(false);
                if new {
                    tracing::warn!("the last update rolled back");
                    captures.auto("error:update-rollback".into());
                }
            }
        }
    });
}

#[cfg(not(target_os = "linux"))]
pub fn spawn(_ifaces: Vec<String>, _captures: crate::capture::Captures, _state_dir: PathBuf) {}

/// `None` when the interface doesn't exist. An `ip` that won't run counts
/// as no news (the last state stands).
#[cfg(target_os = "linux")]
async fn ip_stats(iface: &str) -> Option<CanStats> {
    let out = tokio::process::Command::new("ip")
        .args([
            "-details",
            "-statistics",
            "-json",
            "link",
            "show",
            "dev",
            iface,
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .await;
    match out {
        Ok(o) if o.status.success() => {
            parse_ip(&String::from_utf8_lossy(&o.stdout)).or(Some(CanStats::default()))
        }
        // ip exits 1 with "Device ... does not exist".
        Ok(o) if String::from_utf8_lossy(&o.stderr).contains("does not exist") => None,
        _ => Some(CanStats::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IP: &str = r#"[{"ifindex":3,"ifname":"can0","operstate":"UP","linkinfo":{"info_kind":"can",
        "info_data":{"ctrlmode":[],"state":"ERROR-ACTIVE","berr_counter":{"tx":0,"rx":0},"restart_ms":100},
        "info_xstats":{"restarts":0,"bus_error":0,"arbitration_lost":0,"error_warning":1,"error_passive":2,"bus_off":0}}}]"#;

    #[test]
    fn ip_json() {
        let s = parse_ip(IP).unwrap();
        assert_eq!(s.state.as_deref(), Some("ERROR-ACTIVE"));
        assert_eq!(s.error_passive, 2);
        assert_eq!(s.bus_off, 0);
        assert_eq!(parse_ip("not json"), None);
    }

    fn st(state: &str, bus_off: u64, passive: u64) -> Option<CanStats> {
        Some(CanStats {
            state: Some(state.into()),
            bus_off,
            error_passive: passive,
        })
    }

    #[test]
    fn can_faults() {
        let t0 = Instant::now();
        let s = |n| t0 + Duration::from_secs(n);
        let mut w = CanWatch::default();
        assert_eq!(w.step(s(0), st("ERROR-ACTIVE", 0, 0)), None);
        assert_eq!(w.step(s(5), st("ERROR-ACTIVE", 1, 0)), Some("bus-off"));
        assert_eq!(w.step(s(10), st("BUS-OFF", 1, 0)), Some("bus-off"));
        assert_eq!(w.step(s(15), st("BUS-OFF", 1, 0)), None);
        // Passive transitions: a few is normal, ten in a minute is a storm.
        assert_eq!(w.step(s(20), st("ERROR-ACTIVE", 1, 4)), None);
        assert_eq!(
            w.step(s(90), st("ERROR-ACTIVE", 1, 9)),
            None,
            "the 4 aged out"
        );
        assert_eq!(
            w.step(s(95), st("ERROR-ACTIVE", 1, 14)),
            Some("error-passive")
        );
        // Gone once, then back.
        assert_eq!(w.step(s(100), None), Some("gone"));
        assert_eq!(w.step(s(105), None), None);
        assert_eq!(w.step(s(110), st("ERROR-ACTIVE", 0, 0)), None);
    }

    #[test]
    fn rollback_once() {
        assert!(rolled_back(
            r#"{"downloaded":null,"last_apply":"rolled_back"}"#
        ));
        assert!(!rolled_back(r#"{"last_apply":"ok"}"#));
        assert_eq!(rollback_step(true, false), (true, true));
        assert_eq!(rollback_step(true, true), (false, true));
        assert_eq!(rollback_step(false, true), (false, false));
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("state.json");
        std::fs::write(&f, r#"{"last_apply":"rolled_back"}"#).unwrap();
        assert!(check_rollback(&f, t.path()));
        assert!(!check_rollback(&f, t.path()));
        std::fs::write(&f, r#"{"last_apply":"ok"}"#).unwrap();
        assert!(!check_rollback(&f, t.path()));
        std::fs::write(&f, r#"{"last_apply":"rolled_back"}"#).unwrap();
        assert!(check_rollback(&f, t.path()));
    }
}
