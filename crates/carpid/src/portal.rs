//! The phone portal (`carpi-portal`), run from carpid when `[portal]` is in
//! the config. Its status block reads a `watch` the main loop updates; its
//! config upload check is the loader's own `Config::parse`.

use crate::config::Config;
use carpi_portal::{PinCell, Portal, PortalConfig, Status};
use std::path::Path;
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
}
