//! The phone portal (`carpi-portal`), run from carpid when `[portal]` is in
//! the config. Its status block reads a `watch` the main loop updates; its
//! config upload check is the loader's own `Config::parse`; its Wi-Fi page is
//! `wifi`'s list, add and forget (never scan or test: they drop the hotspot
//! the phone is on).

use crate::config::Config;
use carpi_portal::{PinCell, Portal, PortalConfig, Status, WifiNet, WifiOp};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::watch;

/// The portal's settings, with `config_file` set to the file carpid loaded.
/// `None` when `[portal]` is absent.
/// `logs` is the folder captures go to, the same one the portal lists.
pub fn config(cfg: &Config, loaded_from: &Path, logs: &Path) -> Option<PortalConfig> {
    let mut p = cfg.portal.clone()?;
    p.config_file = loaded_from.to_path_buf();
    p.logs = logs.to_path_buf();
    Some(p)
}

pub fn build(pcfg: PortalConfig, status: watch::Receiver<Status>, started: Instant) -> Portal {
    Portal::new(pcfg)
        .status(move || {
            let mut s = status.borrow().clone();
            async move {
                s.uptime_s = started.elapsed().as_secs();
                s
            }
        })
        .validate_config(|text| Config::parse(text).map(|_| ()))
}

/// The portal's Capture now button: the same capture as the key and the API.
pub fn with_captures(portal: Portal, captures: crate::capture::Captures) -> Portal {
    portal.capture(move |trigger| {
        let c = captures.clone();
        async move {
            match c.capture(&trigger).await {
                carpi_proto::Response::Captured { label, .. } => Ok(label),
                carpi_proto::Response::Error { message } => Err(message),
                other => Err(format!("unexpected reply: {other:?}")),
            }
        }
    })
}

/// The portal's Wi-Fi page: saved networks through NetworkManager, as the
/// API's `Request::Wifi` list, add and forget.
#[cfg_attr(not(unix), allow(dead_code))]
pub fn with_wifi<N: crate::wifi::Nmcli + 'static>(portal: Portal, nm: N) -> Portal {
    let nm = Arc::new(nm);
    portal.wifi(move |op| {
        let nm = nm.clone();
        async move {
            match op {
                WifiOp::List => {}
                WifiOp::Add {
                    ssid,
                    password,
                    hidden,
                } => crate::wifi::add(&*nm, &ssid, password.as_deref(), hidden).await?,
                WifiOp::Forget { ssid } => crate::wifi::forget(&*nm, &ssid).await?,
            }
            let (nets, connected) = crate::wifi::list(&*nm).await?;
            Ok(nets
                .into_iter()
                .map(|n| WifiNet {
                    connected: connected.as_deref() == Some(n.ssid.as_str()),
                    ssid: n.ssid,
                    secured: n.secured,
                    hidden: n.hidden,
                })
                .collect())
        }
    })
}

/// Serves until the process ends. Returns the live PIN cell.
pub fn spawn(portal: Portal) -> PinCell {
    let pin = portal.pin_cell();
    tokio::spawn(async move {
        if let Err(e) = portal.serve(std::future::pending()).await {
            tracing::warn!("portal stopped: {e}");
        }
    });
    pin
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn http(port: u16, req: String) -> String {
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        s.write_all(req.as_bytes()).await.unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        out
    }

    #[tokio::test]
    async fn starts_from_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("carpid.toml");
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let text = format!(
            "[bus.ican]\niface = \"can0\"\n[portal]\nbind = \"127.0.0.1:{port}\"\ncaptures = {:?}\n",
            dir.path().display().to_string()
        );
        std::fs::write(&path, &text).unwrap();
        let cfg = Config::load(&path).unwrap();
        assert!(
            config(
                &Config::parse("[bus.ican]\niface='x'\n").unwrap(),
                &path,
                dir.path()
            )
            .is_none()
        );
        let pcfg = config(&cfg, &path, dir.path()).unwrap();
        assert_eq!(pcfg.config_file, path);

        let (tx, rx) = watch::channel(Status {
            version: "9.9.9".into(),
            bus: "awake".into(),
            ..Default::default()
        });
        let _pin = spawn(build(pcfg, rx, Instant::now()));
        let get = "GET /api/status HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
        let mut reply = String::new();
        for _ in 0..50 {
            if tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_ok()
            {
                reply = http(port, get.into()).await;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
        assert!(
            reply.contains("9.9.9") && reply.contains("awake"),
            "{reply}"
        );
        tx.send_modify(|s| s.bus = "silent".into());
        assert!(http(port, get.into()).await.contains("silent"));

        // A config upload goes through carpid's own rules.
        let body = "--XyZ\r\nContent-Disposition: form-data; name=\"file\"; filename=\"carpid.toml\"\r\n\r\n[bus.diag]\niface = \"can0\"\n\r\n--XyZ--\r\n";
        let post = format!(
            "POST /up/config HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: multipart/form-data; boundary=XyZ\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let reply = http(port, post).await;
        assert!(reply.starts_with("HTTP/1.1 422"), "{reply}");
        assert!(reply.contains("[bus.ican] is required"), "{reply}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }

    /// Saved profiles: (uuid, ssid, password, hidden, mode).
    type Profiles = Vec<(String, String, Option<String>, bool, &'static str)>;

    /// A NetworkManager with state, enough for list, add and forget.
    #[derive(Clone, Default)]
    struct FakeNm(Arc<std::sync::Mutex<Profiles>>);

    impl crate::wifi::Nmcli for FakeNm {
        async fn run(&self, args: &[&str]) -> Result<String, String> {
            let mut ps = self.0.lock().unwrap();
            let arg = |k: &str| {
                args.iter()
                    .position(|a| *a == k)
                    .map(|i| args[i + 1].to_string())
            };
            match args {
                ["-t", "-f", "UUID,TYPE,TIMESTAMP,ACTIVE", ..] => Ok(ps
                    .iter()
                    .map(|p| format!("{}:802-11-wireless:0:no\n", p.0))
                    .collect()),
                ["-g", field, "connection", "show", "uuid", uuid] => {
                    let p = ps.iter().find(|p| p.0 == *uuid).ok_or("no such uuid")?;
                    Ok(match *field {
                        "802-11-wireless.mode" => p.4.to_string(),
                        "802-11-wireless.ssid" => p.1.clone(),
                        "802-11-wireless.hidden" => if p.3 { "yes" } else { "no" }.into(),
                        _ => if p.2.is_some() { "wpa-psk" } else { "" }.into(),
                    })
                }
                ["connection", "delete", "uuid", uuid] => {
                    ps.retain(|p| p.0 != *uuid);
                    Ok(String::new())
                }
                ["connection", "add", ..] => {
                    let uuid = format!("u{}", ps.len() + 10);
                    ps.push((
                        uuid,
                        arg("ssid").unwrap(),
                        arg("wifi-sec.psk"),
                        arg("802-11-wireless.hidden").is_some(),
                        "infrastructure",
                    ));
                    Ok(String::new())
                }
                _ => Err(format!("unexpected nmcli call: {}", args.len())),
            }
        }
    }

    fn post(path: &str, body: &str) -> String {
        format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\
             Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
    }

    fn get(path: &str) -> String {
        format!(
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Carpi-Pin: 4711\r\nConnection: close\r\n\r\n"
        )
    }

    #[tokio::test]
    async fn wifi_page_lists_adds_and_forgets() {
        let dir = tempfile::tempdir().unwrap();
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let pcfg = PortalConfig {
            bind: ([127, 0, 0, 1], port).into(),
            logs: dir.path().join("logs"),
            captures: dir.path().join("captures"),
            pin: Some("4711".into()),
            ..PortalConfig::default()
        };
        let nm = FakeNm::default();
        nm.0.lock().unwrap().extend([
            (
                "u1".into(),
                "Home".into(),
                Some("homehome1".into()),
                false,
                "infrastructure",
            ),
            (
                "u2".into(),
                "LIVI hotspot".into(),
                Some("livilivi".into()),
                false,
                "ap",
            ),
        ]);
        let (_tx, rx) = watch::channel(Status::default());
        let _pin = spawn(with_wifi(build(pcfg, rx, Instant::now()), nm.clone()));
        let mut page = String::new();
        for _ in 0..50 {
            if tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_ok()
            {
                page = http(port, get("/wifi")).await;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        // The hotspot's own profile is never listed.
        assert!(page.starts_with("HTTP/1.1 200"), "{page}");
        assert!(
            page.contains(">Home<") && !page.contains("LIVI hotspot"),
            "{page}"
        );
        let saved = |nm: &FakeNm| -> Vec<String> {
            nm.0.lock().unwrap().iter().map(|p| p.1.clone()).collect()
        };

        // Add: PIN first, then the name and password rules.
        let add = "ssid=Cafe+Wifi&password=hunter2hunter2&hidden=yes";
        let r = http(port, post("/wifi/add", add)).await;
        assert!(r.starts_with("HTTP/1.1 401"), "{r}");
        let r = http(
            port,
            post("/wifi/add", "ssid=Cafe&password=s3cr3t!&pin=4711"),
        )
        .await;
        assert!(
            r.starts_with("HTTP/1.1 400") && !r.contains("s3cr3t"),
            "{r}"
        );
        let long = "x".repeat(33);
        let r = http(port, post("/wifi/add", &format!("ssid={long}&pin=4711"))).await;
        assert!(r.starts_with("HTTP/1.1 400"), "{r}");
        assert_eq!(saved(&nm), ["Home", "LIVI hotspot"]);
        let r = http(port, post("/wifi/add", &format!("{add}&pin=4711"))).await;
        assert!(
            r.starts_with("HTTP/1.1 303") && !r.contains("hunter2"),
            "{r}"
        );
        {
            let ps = nm.0.lock().unwrap();
            let cafe = ps.iter().find(|p| p.1 == "Cafe Wifi").unwrap();
            assert_eq!(cafe.2.as_deref(), Some("hunter2hunter2"));
            assert!(cafe.3);
        }
        let r = http(port, get("/wifi")).await;
        assert!(r.contains(">Cafe Wifi<") && !r.contains("hunter2"), "{r}");

        // Forget: the confirm page, then only with confirm=yes.
        let r = http(port, get("/wifi/forget?ssid=Cafe%20Wifi")).await;
        assert!(
            r.starts_with("HTTP/1.1 200") && r.contains("Forget Cafe Wifi?"),
            "{r}"
        );
        let r = http(port, post("/wifi/forget", "ssid=Cafe+Wifi&pin=4711")).await;
        assert!(r.starts_with("HTTP/1.1 400"), "{r}");
        let r = http(port, post("/wifi/forget", "ssid=Cafe+Wifi&confirm=yes")).await;
        assert!(r.starts_with("HTTP/1.1 401"), "{r}");
        let r = http(
            port,
            post("/wifi/forget", "ssid=Cafe+Wifi&confirm=yes&pin=4711"),
        )
        .await;
        assert!(r.starts_with("HTTP/1.1 303"), "{r}");
        assert_eq!(saved(&nm), ["Home", "LIVI hotspot"]);
        // The hotspot can't be forgotten from here.
        let r = http(
            port,
            post("/wifi/forget", "ssid=LIVI+hotspot&confirm=yes&pin=4711"),
        )
        .await;
        assert!(r.starts_with("HTTP/1.1 500") && r.contains("isn"), "{r}");
        assert_eq!(saved(&nm), ["Home", "LIVI hotspot"]);
    }
}
