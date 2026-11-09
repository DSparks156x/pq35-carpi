//! Wi-Fi networks the Pi joins when LIVI's hotspot steps aside, through
//! NetworkManager (`nmcli`); docs/PAGES.md 5b.
//!
//! `wlan0` is the hotspot while LIVI runs, so adding a network only saves a
//! NetworkManager profile (autoconnect, so the update's network dance joins
//! it). Scanning and testing need the radio: they suspend the hotspot through
//! LIVI, like an update does, and always resume it.
//!
//! The car user may change system connections and scan through a polkit rule
//! (os/profile.sh). Passwords go to NetworkManager, which keeps them in
//! root-only files; they're never logged.

#![cfg_attr(not(unix), allow(dead_code))]

use crate::update::Bridge;
use carpi_proto::{ApAction, JobEvent, WifiNetwork, WifiSeen};
use std::future::Future;
use std::time::Duration;
use tokio::sync::mpsc;

pub trait Nmcli: Send + Sync {
    /// Runs `nmcli <args>`; stdout, or the error text.
    fn run(&self, args: &[&str]) -> impl Future<Output = Result<String, String>> + Send;
}

/// The real `nmcli`.
pub struct Real;

impl Nmcli for Real {
    async fn run(&self, args: &[&str]) -> Result<String, String> {
        let out = tokio::time::timeout(
            Duration::from_secs(45),
            tokio::process::Command::new("nmcli")
                .args(args)
                .stdin(std::process::Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| "nmcli took over 45 s".to_string())?
        .map_err(|e| format!("nmcli: {e}"))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            // Never echo the arguments: `add` carries the password.
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    }
}

/// One `nmcli -t` line into fields: `:` separates, `\:` and `\\` escape.
pub fn fields(line: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut esc = false;
    for c in line.chars() {
        match (esc, c) {
            (true, c) => {
                out.last_mut().unwrap().push(c);
                esc = false;
            }
            (false, '\\') => esc = true,
            (false, ':') => out.push(String::new()),
            (false, c) => out.last_mut().unwrap().push(c),
        }
    }
    out
}

/// A saved Wi-Fi profile.
#[derive(Clone, Debug, PartialEq)]
struct Profile {
    uuid: String,
    ssid: String,
    mode: String,
    key_mgmt: String,
    hidden: bool,
    last_used: Option<u64>,
    active: bool,
}

async fn get(nm: &impl Nmcli, field: &str, uuid: &str) -> String {
    nm.run(&["-g", field, "connection", "show", "uuid", uuid])
        .await
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Client (infrastructure) Wi-Fi profiles; LIVI's hotspot and any other AP
/// profile left out.
async fn profiles(nm: &impl Nmcli) -> Result<Vec<Profile>, String> {
    let text = nm
        .run(&[
            "-t",
            "-f",
            "UUID,TYPE,TIMESTAMP,ACTIVE",
            "connection",
            "show",
        ])
        .await?;
    let mut out = Vec::new();
    for line in text.lines() {
        let f = fields(line);
        if f.len() < 4 || f[1] != "802-11-wireless" {
            continue;
        }
        let uuid = f[0].clone();
        let mode = get(nm, "802-11-wireless.mode", &uuid).await;
        if mode == "ap" {
            continue;
        }
        out.push(Profile {
            ssid: get(nm, "802-11-wireless.ssid", &uuid).await,
            key_mgmt: get(nm, "802-11-wireless-security.key-mgmt", &uuid).await,
            hidden: get(nm, "802-11-wireless.hidden", &uuid).await == "yes",
            last_used: f[2].parse().ok().filter(|t| *t > 0),
            active: f[3] == "yes",
            mode,
            uuid,
        });
    }
    Ok(out)
}

/// The saved networks, and the one in use.
pub async fn list(nm: &impl Nmcli) -> Result<(Vec<WifiNetwork>, Option<String>), String> {
    let ps = profiles(nm).await?;
    let connected = ps.iter().find(|p| p.active).map(|p| p.ssid.clone());
    let mut nets: Vec<WifiNetwork> = ps
        .into_iter()
        .map(|p| WifiNetwork {
            secured: !p.key_mgmt.is_empty() && p.key_mgmt != "none",
            hidden: p.hidden,
            last_used: p.last_used,
            ssid: p.ssid,
        })
        .collect();
    nets.sort_by(|a, b| b.last_used.cmp(&a.last_used).then(a.ssid.cmp(&b.ssid)));
    Ok((nets, connected))
}

fn check_ssid(ssid: &str) -> Result<(), String> {
    if ssid.is_empty() || ssid.len() > 32 {
        return Err("a network name is 1-32 characters".into());
    }
    Ok(())
}

/// Saves `ssid` (replacing a saved one with that name). WPA passwords are
/// 8-63 characters.
pub async fn add(
    nm: &impl Nmcli,
    ssid: &str,
    password: Option<&str>,
    hidden: bool,
) -> Result<(), String> {
    check_ssid(ssid)?;
    if let Some(p) = password
        && !(8..=63).contains(&p.chars().count())
    {
        return Err("a Wi-Fi password is 8-63 characters".into());
    }
    for p in profiles(nm).await? {
        if p.ssid == ssid {
            nm.run(&["connection", "delete", "uuid", &p.uuid]).await?;
        }
    }
    let mut args: Vec<&str> = vec![
        "connection",
        "add",
        "type",
        "wifi",
        "ifname",
        "wlan0",
        "con-name",
        ssid,
        "ssid",
        ssid,
        "connection.autoconnect",
        "yes",
    ];
    if hidden {
        args.extend(["802-11-wireless.hidden", "yes"]);
    }
    if let Some(p) = password {
        args.extend(["wifi-sec.key-mgmt", "wpa-psk", "wifi-sec.psk", p]);
    }
    nm.run(&args).await.map(|_| ())
}

pub async fn forget(nm: &impl Nmcli, ssid: &str) -> Result<(), String> {
    let ps = profiles(nm).await?;
    let found: Vec<_> = ps.iter().filter(|p| p.ssid == ssid).collect();
    if found.is_empty() {
        return Err(format!("{ssid:?} isn't saved"));
    }
    for p in found {
        nm.run(&["connection", "delete", "uuid", &p.uuid]).await?;
    }
    Ok(())
}

/// `nmcli -t -f SSID,SIGNAL,SECURITY device wifi list`, strongest first, one
/// entry per name, hidden networks (no name) left out.
pub fn parse_scan(text: &str, saved: &[String]) -> Vec<WifiSeen> {
    let mut out: Vec<WifiSeen> = Vec::new();
    for line in text.lines() {
        let f = fields(line);
        if f.len() < 3 || f[0].is_empty() {
            continue;
        }
        let signal = f[1].parse::<u8>().unwrap_or(0).min(100);
        let secured = !f[2].is_empty() && f[2] != "--";
        match out.iter_mut().find(|s| s.ssid == f[0]) {
            Some(s) => s.signal = s.signal.max(signal),
            None => out.push(WifiSeen {
                saved: saved.contains(&f[0]),
                ssid: f[0].clone(),
                signal,
                secured,
            }),
        }
    }
    out.sort_by(|a, b| b.signal.cmp(&a.signal).then(a.ssid.cmp(&b.ssid)));
    out
}

fn stage(events: &mpsc::UnboundedSender<JobEvent>, stage: &str, message: impl Into<String>) {
    let _ = events.send(JobEvent::Stage {
        stage: stage.into(),
        message: message.into(),
    });
}

#[derive(Clone, Copy)]
pub struct Timing {
    /// After the hotspot stops, for NetworkManager to take the radio.
    pub settle: Duration,
    /// For a test's join.
    pub join: Duration,
}

pub const TIMING: Timing = Timing {
    settle: Duration::from_secs(3),
    join: Duration::from_secs(30),
};

/// Runs `f` with the hotspot suspended, then always resumes it.
async fn without_ap<T>(
    bridge: &impl Bridge,
    events: &mpsc::UnboundedSender<JobEvent>,
    f: impl Future<Output = T>,
) -> T {
    stage(
        events,
        "ap",
        "hotspot off for a moment; the phone link drops",
    );
    if let Err(e) = bridge.ap(ApAction::Suspend).await {
        stage(events, "ap", format!("suspend: {e}"));
    }
    let out = f.await;
    stage(events, "ap", "hotspot back on");
    if let Err(e) = bridge.ap(ApAction::Resume).await {
        stage(events, "ap", format!("resume: {e}"));
    }
    out
}

/// Scan job: hotspot down, rescan, list, hotspot back. Ends with `Finished`.
pub async fn scan(
    nm: &impl Nmcli,
    bridge: &impl Bridge,
    t: Timing,
    events: &mpsc::UnboundedSender<JobEvent>,
) {
    let saved: Vec<String> = match list(nm).await {
        Ok((n, _)) => n.into_iter().map(|n| n.ssid).collect(),
        Err(_) => Vec::new(),
    };
    let result = without_ap(bridge, events, async {
        tokio::time::sleep(t.settle).await;
        stage(events, "scan", "looking for networks");
        nm.run(&[
            "-t",
            "-f",
            "SSID,SIGNAL,SECURITY",
            "device",
            "wifi",
            "list",
            "--rescan",
            "yes",
        ])
        .await
    })
    .await;
    let (ok, message) = match result {
        Ok(text) => {
            let seen = parse_scan(&text, &saved);
            let n = seen.len();
            let _ = events.send(JobEvent::WifiSeen { networks: seen });
            (
                true,
                format!("{n} network{} in range", if n == 1 { "" } else { "s" }),
            )
        }
        Err(e) => (false, format!("scan failed: {e}")),
    };
    let _ = events.send(JobEvent::Finished { ok, message });
}

/// Test job: hotspot down, join `ssid`, see if NetworkManager calls it
/// online, leave it, hotspot back. Ends with `Finished`.
pub async fn test(
    nm: &impl Nmcli,
    bridge: &impl Bridge,
    ssid: &str,
    t: Timing,
    events: &mpsc::UnboundedSender<JobEvent>,
) {
    let uuid = match profiles(nm).await {
        Ok(ps) => ps.into_iter().find(|p| p.ssid == ssid).map(|p| p.uuid),
        Err(e) => {
            let _ = events.send(JobEvent::Finished {
                ok: false,
                message: e,
            });
            return;
        }
    };
    let Some(uuid) = uuid else {
        let _ = events.send(JobEvent::Finished {
            ok: false,
            message: format!("{ssid:?} isn't saved"),
        });
        return;
    };
    let result = without_ap(bridge, events, async {
        tokio::time::sleep(t.settle).await;
        stage(events, "join", format!("joining {ssid}"));
        let wait = format!("{}", t.join.as_secs());
        let up = nm
            .run(&["--wait", &wait, "connection", "up", "uuid", &uuid])
            .await;
        let state = nm
            .run(&["-t", "-f", "STATE", "general"])
            .await
            .unwrap_or_default();
        let _ = nm.run(&["connection", "down", "uuid", &uuid]).await;
        match up {
            Err(e) => Err(format!("couldn't join {ssid}: {e}")),
            Ok(_) if state.trim() == "connected" => Ok(format!("{ssid} works, with the internet")),
            Ok(_) => Ok(format!("joined {ssid}, but no internet ({})", state.trim())),
        }
    })
    .await;
    let (ok, message) = match result {
        Ok(m) => (true, m),
        Err(m) => (false, m),
    };
    let _ = events.send(JobEvent::Finished { ok, message });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Answers by the first matching argument pattern; records every call.
    #[derive(Clone, Default)]
    struct Fake {
        calls: Arc<Mutex<Vec<String>>>,
        answers: Vec<(&'static str, Result<String, String>)>,
    }

    impl Nmcli for Fake {
        async fn run(&self, args: &[&str]) -> Result<String, String> {
            let line = args.join(" ");
            self.calls.lock().unwrap().push(line.clone());
            self.answers
                .iter()
                .find(|(p, _)| line.contains(p))
                .map(|(_, r)| r.clone())
                .unwrap_or(Ok(String::new()))
        }
    }

    struct Ap(Arc<Mutex<Vec<String>>>);

    impl Bridge for Ap {
        async fn ap(&self, action: ApAction) -> Result<(), String> {
            self.0.lock().unwrap().push(format!("{action:?}"));
            Ok(())
        }
    }

    fn fake() -> Fake {
        Fake {
            answers: vec![
                (
                    "UUID,TYPE,TIMESTAMP,ACTIVE",
                    Ok("u1:802-11-wireless:1762600000:no\nu2:802-3-ethernet:0:yes\nu3:802-11-wireless:0:no\nu4:802-11-wireless:1762700000:yes\n".into()),
                ),
                ("802-11-wireless.mode connection show uuid u3", Ok("ap\n".into())),
                ("802-11-wireless.mode", Ok("infrastructure\n".into())),
                ("802-11-wireless.ssid connection show uuid u1", Ok("Home\\:5G\n".into())),
                ("802-11-wireless.ssid connection show uuid u4", Ok("Pixel hotspot\n".into())),
                ("key-mgmt connection show uuid u1", Ok("wpa-psk\n".into())),
                ("802-11-wireless.hidden", Ok("no\n".into())),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn terse_fields_unescape() {
        assert_eq!(fields(r"a\:b:c\\d:"), ["a:b", r"c\d", ""]);
    }

    #[tokio::test]
    async fn lists_client_networks_only() {
        let (nets, connected) = list(&fake()).await.unwrap();
        // The AP profile and Ethernet are left out; newest first.
        let names: Vec<_> = nets.iter().map(|n| n.ssid.as_str()).collect();
        assert_eq!(names, ["Pixel hotspot", r"Home\:5G"]);
        assert!(!nets[0].secured && nets[1].secured);
        assert_eq!(connected.as_deref(), Some("Pixel hotspot"));
    }

    #[tokio::test]
    async fn add_replaces_and_checks_the_password() {
        let nm = fake();
        assert!(add(&nm, "x", Some("short"), false).await.is_err());
        assert!(add(&nm, "", None, false).await.is_err());
        add(&nm, "Pixel hotspot", Some("hunter2hunter2"), true)
            .await
            .unwrap();
        let calls = nm.calls.lock().unwrap().clone();
        assert!(calls.iter().any(|c| c == "connection delete uuid u4"));
        let addc = calls
            .iter()
            .find(|c| c.starts_with("connection add"))
            .unwrap();
        assert!(addc.contains("ssid Pixel hotspot") && addc.contains("802-11-wireless.hidden yes"));
        assert!(addc.contains("wifi-sec.key-mgmt wpa-psk"));
    }

    #[test]
    fn scan_merges_and_sorts() {
        let text = "Home:40:WPA2\nCafe:80:\nHome:70:WPA2\n:90:WPA2\nCafe\\:2:10:WPA1 WPA2\n";
        let seen = parse_scan(text, &["Home".into()]);
        let names: Vec<_> = seen
            .iter()
            .map(|s| (s.ssid.as_str(), s.signal, s.secured, s.saved))
            .collect();
        assert_eq!(
            names,
            [
                ("Cafe", 80, false, false),
                ("Home", 70, true, true),
                ("Cafe:2", 10, true, false)
            ]
        );
    }

    #[tokio::test]
    async fn scan_always_brings_the_hotspot_back() {
        let ap = Arc::new(Mutex::new(Vec::new()));
        let mut nm = fake();
        nm.answers
            .insert(0, ("device wifi list", Err("radio busy".into())));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let t = Timing {
            settle: Duration::ZERO,
            join: Duration::from_secs(1),
        };
        scan(&nm, &Ap(ap.clone()), t, &tx).await;
        assert_eq!(*ap.lock().unwrap(), ["Suspend", "Resume"]);
        let mut last = None;
        while let Ok(e) = rx.try_recv() {
            last = Some(e);
        }
        assert!(matches!(last, Some(JobEvent::Finished { ok: false, .. })));
    }

    #[tokio::test]
    async fn test_joins_reports_and_leaves() {
        let ap = Arc::new(Mutex::new(Vec::new()));
        let mut nm = fake();
        nm.answers
            .insert(0, ("STATE general", Ok("connected\n".into())));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let t = Timing {
            settle: Duration::ZERO,
            join: Duration::from_secs(5),
        };
        test(&nm, &Ap(ap.clone()), "Pixel hotspot", t, &tx).await;
        let calls = nm.calls.lock().unwrap().clone();
        assert!(calls.iter().any(|c| c == "--wait 5 connection up uuid u4"));
        assert!(calls.iter().any(|c| c == "connection down uuid u4"));
        assert_eq!(*ap.lock().unwrap(), ["Suspend", "Resume"]);
        let mut last = None;
        while let Ok(e) = rx.try_recv() {
            last = Some(e);
        }
        assert!(matches!(last, Some(JobEvent::Finished { ok: true, .. })));
    }
}
