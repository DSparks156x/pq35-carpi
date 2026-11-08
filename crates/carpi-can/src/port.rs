use crate::{Frame, RxFrame, candump};
use std::io;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// A receive-only CAN source. Sending is added with the TX gate.
pub enum Port {
    #[cfg(target_os = "linux")]
    Socket(socket::SocketPort),
    Replay(ReplayPort),
}

impl Port {
    /// Opens a SocketCAN interface by name (e.g. `ican`, `can0`).
    #[cfg(target_os = "linux")]
    pub fn open(iface: &str) -> io::Result<Self> {
        Ok(Port::Socket(socket::SocketPort::open(iface)?))
    }

    #[cfg(not(target_os = "linux"))]
    pub fn open(iface: &str) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("SocketCAN ({iface}) needs Linux; use a replay file"),
        ))
    }

    /// Next frame. `Ok(None)` means the source ended (replay only).
    pub async fn recv(&mut self) -> io::Result<Option<RxFrame>> {
        match self {
            #[cfg(target_os = "linux")]
            Port::Socket(s) => s.recv().await.map(Some),
            Port::Replay(r) => Ok(r.recv().await),
        }
    }
}

/// Plays a candump file back as if it were a bus.
pub struct ReplayPort {
    frames: Vec<(Duration, Frame)>,
    next: usize,
    start: Instant,
    start_wall: SystemTime,
    realtime: bool,
}

impl ReplayPort {
    /// Loads every frame on `iface` (or every frame, if `iface` is `None`).
    /// Files without timestamps get `gap` between frames.
    pub fn load(
        path: &Path,
        iface: Option<&str>,
        gap: Duration,
        realtime: bool,
    ) -> io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Ok(Self::from_text(&text, iface, gap, realtime))
    }

    pub fn from_text(text: &str, iface: Option<&str>, gap: Duration, realtime: bool) -> Self {
        let mut frames = Vec::new();
        let mut first_ts = None;
        let mut n = 0u32;
        for line in text.lines().filter_map(candump::parse_line) {
            if iface.is_some_and(|i| i != line.iface) {
                continue;
            }
            let offset = match line.ts {
                Some(ts) => {
                    let first = *first_ts.get_or_insert(ts);
                    Duration::from_secs_f64((ts - first).max(0.0))
                }
                None => gap * n,
            };
            n += 1;
            frames.push((offset, line.frame));
        }
        ReplayPort {
            frames,
            next: 0,
            start: Instant::now(),
            start_wall: SystemTime::now(),
            realtime,
        }
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Synchronous iteration for offline tools and tests: yields frames with
    /// `at` = `base` + file offset, without sleeping.
    pub fn iter_from(&self, base: Instant) -> impl Iterator<Item = RxFrame> + '_ {
        self.frames.iter().map(move |(off, frame)| RxFrame {
            frame: *frame,
            at: base + *off,
            wall: UNIX_EPOCH + *off,
        })
    }

    async fn recv(&mut self) -> Option<RxFrame> {
        let (off, frame) = *self.frames.get(self.next)?;
        self.next += 1;
        let at = self.start + off;
        if self.realtime {
            tokio::time::sleep_until(at.into()).await;
        }
        Some(RxFrame {
            frame,
            at,
            wall: self.start_wall + off,
        })
    }
}

#[cfg(target_os = "linux")]
mod socket {
    use super::*;
    use socketcan::{CanFrame, EmbeddedFrame, Frame as _};

    pub struct SocketPort {
        sock: socketcan::tokio::CanSocket,
    }

    impl SocketPort {
        pub fn open(iface: &str) -> io::Result<Self> {
            Ok(SocketPort {
                sock: socketcan::tokio::CanSocket::open(iface)?,
            })
        }

        pub async fn recv(&mut self) -> io::Result<RxFrame> {
            loop {
                let f = self.sock.read_frame().await?;
                let at = Instant::now();
                let wall = SystemTime::now();
                // Remote and error frames carry no data we use.
                let CanFrame::Data(d) = f else { continue };
                let frame = if d.is_extended() {
                    Frame::new_extended(d.raw_id(), d.data())
                } else {
                    Frame::new(d.raw_id(), d.data())
                };
                if let Some(frame) = frame {
                    return Ok(RxFrame { frame, at, wall });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_spacing() {
        let text = "  can0  42B   [2]  16 01\n  can1  436   [2]  0B 01\n  can0  5C3   [2]  39 0B\n";
        let r = ReplayPort::from_text(text, Some("can0"), Duration::from_millis(10), false);
        assert_eq!(r.len(), 2);
        let base = Instant::now();
        let v: Vec<_> = r.iter_from(base).collect();
        assert_eq!(v[1].at - v[0].at, Duration::from_millis(10));

        let text = "(10.0) ican 42B#1601\n(10.25) ican 436#0B01\n";
        let r = ReplayPort::from_text(text, None, Duration::from_millis(10), false);
        let v: Vec<_> = r.iter_from(base).collect();
        assert_eq!(v[1].at - v[0].at, Duration::from_millis(250));
    }

    #[tokio::test]
    async fn replay_recv_ends() {
        let r = ReplayPort::from_text("  can0  42B   [2]  16 01\n", None, Duration::ZERO, false);
        let mut p = Port::Replay(r);
        assert!(p.recv().await.unwrap().is_some());
        assert!(p.recv().await.unwrap().is_none());
    }
}
