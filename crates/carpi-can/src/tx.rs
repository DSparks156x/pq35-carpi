use crate::Frame;
use std::io;

/// Sending side of an interface. Only the daemon's TX task owns one, and only
/// gate-approved frames reach it.
pub struct TxPort {
    #[cfg(target_os = "linux")]
    sock: socketcan::tokio::CanSocket,
}

impl TxPort {
    #[cfg(target_os = "linux")]
    pub fn open(iface: &str) -> io::Result<Self> {
        Ok(TxPort {
            sock: socketcan::tokio::CanSocket::open(iface)?,
        })
    }

    #[cfg(not(target_os = "linux"))]
    pub fn open(iface: &str) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("SocketCAN ({iface}) needs Linux"),
        ))
    }

    #[cfg(target_os = "linux")]
    pub async fn send(&self, frame: &Frame) -> io::Result<()> {
        use socketcan::{CanFrame, EmbeddedFrame, ExtendedId, StandardId};
        let f = if frame.extended {
            ExtendedId::new(frame.id).and_then(|id| CanFrame::new(id, frame.data()))
        } else {
            StandardId::new(frame.id as u16).and_then(|id| CanFrame::new(id, frame.data()))
        };
        let f = f.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bad frame"))?;
        self.sock.write_frame(f).await
    }

    #[cfg(not(target_os = "linux"))]
    pub async fn send(&self, _frame: &Frame) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "no SocketCAN here",
        ))
    }
}
