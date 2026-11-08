//! The real [`System`]: systemd and the health probes.

use crate::apply::{Health, System};
use crate::config::Config;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub struct Real {
    restart_user: Vec<String>,
    restart_system: Vec<String>,
    socket: PathBuf,
    check_frames: bool,
    /// Frame count at the last probe, to see it move.
    frames: Option<u64>,
    start: Instant,
}

impl Real {
    pub fn new(cfg: &Config) -> Self {
        Real {
            restart_user: cfg.update.restart_user.clone(),
            restart_system: cfg.update.restart_system.clone(),
            socket: cfg.socket.clone(),
            check_frames: cfg.update.check_frames,
            frames: None,
            start: Instant::now(),
        }
    }
}

fn run(program: &str, args: &[&str]) -> Result<(), String> {
    let status = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .map_err(|e| format!("{program}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} {} exited {status}", args.join(" ")))
    }
}

impl System for Real {
    fn restart(&mut self) -> Result<(), String> {
        let mut errors = Vec::new();
        // System units first: LIVI comes back while carpid restarts.
        for unit in &self.restart_system {
            if let Err(e) = run("sudo", &["-n", "systemctl", "restart", unit]) {
                errors.push(e);
            }
        }
        for unit in &self.restart_user {
            if let Err(e) = run("systemctl", &["--user", "restart", unit]) {
                errors.push(e);
            }
        }
        self.frames = None;
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    fn health(&mut self) -> Health {
        let (carpid, frames) = probe_carpid(&self.socket, self.check_frames);
        let moving = match (self.check_frames, frames) {
            (false, _) => None,
            (true, None) => Some(false),
            (true, Some(n)) => {
                let moved = self.frames.is_some_and(|before| n > before);
                self.frames = Some(n);
                Some(moved)
            }
        };
        Health {
            carpid,
            livi: process_running("livi-core"),
            frames: moving,
        }
    }

    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }

    fn now(&self) -> Duration {
        self.start.elapsed()
    }
}

/// Hello answered, and with `frames` the total frame count from `Bus`.
#[cfg(unix)]
pub fn probe_carpid(socket: &std::path::Path, frames: bool) -> (bool, Option<u64>) {
    use carpi_proto::{Request, Response};
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;

    fn call(s: &mut UnixStream, req: &Request) -> Option<Response> {
        s.write_all(&carpi_proto::encode(req)).ok()?;
        let mut prefix = [0u8; 4];
        s.read_exact(&mut prefix).ok()?;
        let mut body = vec![0u8; carpi_proto::body_len(prefix).ok()?];
        s.read_exact(&mut body).ok()?;
        carpi_proto::decode(&body).ok()
    }

    let Ok(mut s) = UnixStream::connect(socket) else {
        return (false, None);
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
    let hello = Request::Hello {
        version: carpi_proto::PROTOCOL_VERSION,
        client: "carpi-update".into(),
    };
    if !matches!(call(&mut s, &hello), Some(Response::Hello { .. })) {
        return (false, None);
    }
    if !frames {
        return (true, None);
    }
    match call(&mut s, &Request::Bus) {
        Some(Response::Bus { buses }) => (true, Some(buses.iter().map(|b| b.frames).sum())),
        _ => (true, None),
    }
}

#[cfg(not(unix))]
pub fn probe_carpid(_socket: &std::path::Path, _frames: bool) -> (bool, Option<u64>) {
    (false, None)
}

/// A process whose `comm` is `name` (Linux `/proc`).
pub fn process_running(name: &str) -> bool {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return false;
    };
    dir.flatten().any(|e| {
        e.file_name()
            .to_string_lossy()
            .bytes()
            .all(|b| b.is_ascii_digit())
            && std::fs::read_to_string(e.path().join("comm")).is_ok_and(|c| c.trim() == name)
    })
}
