use crate::Frame;
use std::io;

/// Sending side of an interface. Only the daemon's TX task owns one, and only
/// gate-approved frames reach it. The bench tool also gets one from
/// `j2534::open`.
pub struct TxPort {
    inner: Inner,
}

enum Inner {
    #[cfg(target_os = "linux")]
    Socket(socketcan::tokio::CanSocket),
    #[cfg(windows)]
    J2534(std::sync::Arc<crate::j2534::Device>),
    #[cfg(not(target_os = "linux"))]
    None,
}

impl TxPort {
    #[cfg(target_os = "linux")]
    pub fn open(iface: &str) -> io::Result<Self> {
        Ok(TxPort {
            inner: Inner::Socket(socketcan::tokio::CanSocket::open(iface)?),
        })
    }

    #[cfg(not(target_os = "linux"))]
    pub fn open(iface: &str) -> io::Result<Self> {
        let _ = Inner::None;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("SocketCAN ({iface}) needs Linux"),
        ))
    }

    #[cfg(windows)]
    pub(crate) fn j2534(dev: std::sync::Arc<crate::j2534::Device>) -> Self {
        TxPort {
            inner: Inner::J2534(dev),
        }
    }

    /// On J2534 this blocks until the device takes the frame (<= 50 ms).
    pub async fn send(&self, frame: &Frame) -> io::Result<()> {
        match &self.inner {
            #[cfg(target_os = "linux")]
            Inner::Socket(sock) => {
                use socketcan::{CanFrame, EmbeddedFrame, ExtendedId, StandardId};
                let f = if frame.extended {
                    ExtendedId::new(frame.id).and_then(|id| CanFrame::new(id, frame.data()))
                } else {
                    StandardId::new(frame.id as u16).and_then(|id| CanFrame::new(id, frame.data()))
                };
                let f =
                    f.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bad frame"))?;
                sock.write_frame(f).await
            }
            #[cfg(windows)]
            Inner::J2534(dev) => dev.write(frame),
            #[cfg(not(target_os = "linux"))]
            Inner::None => {
                let _ = frame;
                Err(io::Error::new(io::ErrorKind::Unsupported, "no CAN port"))
            }
        }
    }
}
