//! The RNS-E service commands for jobs (decoder read, firmware probe and
//! install): frames go out through the main loop, so through the car's gate
//! (`carpi_core::rnse_service`) and TX worker; replies come in on a port of
//! the job's own.
//!
//! B1 read: `B1 a3 a2 a1 a0 n 00 00` on 0x7B0. Reply on 0x7B6: header
//! `80 st a3 a2 a1 a0 n k`, then `k = ceil(n/6)` frames `80+i d0..d5 sum`.

use crate::{Msg, RnseOp};
use carpi_can::{Frame, Port};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

pub const REPLY_ID: u32 = 0x7B6;

/// One B1 reply being put together.
#[derive(Default)]
pub struct Reply {
    header: bool,
    frames: Vec<Option<[u8; 6]>>,
}

impl Reply {
    /// Feeds one received frame; `Some` once the whole reply to a read of
    /// `len` bytes at `address` is in. `Err` when the unit refused the read.
    pub fn accept(
        &mut self,
        address: u32,
        len: u8,
        frame: &Frame,
    ) -> Result<Option<Vec<u8>>, String> {
        if frame.extended || frame.id != REPLY_ID || frame.len() != 8 {
            return Ok(None);
        }
        let d = frame.data();
        let k = (len as usize).div_ceil(6);
        if d[0] == 0x80 {
            if d[2..6] != address.to_be_bytes() || d[6] != len {
                return Ok(None);
            }
            if d[1] != 0 {
                return Err(format!(
                    "RNS-E refused the read at {address:08X} (status {})",
                    d[1]
                ));
            }
            if d[7] as usize != k {
                return Ok(None);
            }
            self.header = true;
            self.frames = vec![None; k];
        } else if self.header
            && (0x81..0x81 + k as u8).contains(&d[0])
            && d[1..7].iter().fold(0u8, |a, b| a.wrapping_add(*b)) == d[7]
        {
            self.frames[(d[0] - 0x81) as usize] = Some(d[1..7].try_into().unwrap());
        }
        if self.header && self.frames.iter().all(Option::is_some) {
            let mut out: Vec<u8> = self.frames.iter().flat_map(|f| f.unwrap()).collect();
            out.truncate(len as usize);
            Ok(Some(out))
        } else {
            Ok(None)
        }
    }
}

pub struct RnseBus {
    port: Port,
    tx: mpsc::Sender<Msg>,
}

impl RnseBus {
    pub fn open(interface: &str, tx: mpsc::Sender<Msg>) -> Result<Self, String> {
        let port = Port::open(interface).map_err(|e| format!("opening {interface}: {e}"))?;
        Ok(RnseBus { port, tx })
    }

    async fn op(&self, op: RnseOp) -> Result<(), String> {
        let (reply, done) = oneshot::channel();
        self.tx
            .send(Msg::Rnse(op, reply))
            .await
            .map_err(|_| "carpid stopped")?;
        done.await.map_err(|_| "carpid stopped")?
    }

    /// One frame, through the gate. A full TX queue is waited out (100 ms).
    pub async fn send(&self, d: [u8; 8]) -> Result<(), String> {
        for _ in 0..50 {
            match self.op(RnseOp::Send(d)).await {
                Err(e) if e.contains("queue is full") => {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
                other => return other,
            }
        }
        Err("CAN transmit queue stayed full".into())
    }

    pub async fn lease(&self, max: Duration) -> Result<(), String> {
        self.op(RnseOp::Lease(max)).await
    }

    pub async fn release(&self) {
        let _ = self.op(RnseOp::Release).await;
    }

    /// Frames already received are of no interest to the next request.
    pub async fn drain(&mut self) {
        while let Ok(Ok(Some(_))) =
            tokio::time::timeout(Duration::from_millis(1), self.port.recv()).await
        {}
    }

    /// The next 0x7B6 frame whose first byte is `op`, within `wait`.
    pub async fn reply(&mut self, op: u8, wait: Duration) -> Result<Option<[u8; 8]>, String> {
        let r = tokio::time::timeout(wait, async {
            loop {
                let f = self
                    .port
                    .recv()
                    .await
                    .map_err(|e| format!("CAN receive: {e}"))?
                    .ok_or("CAN receive ended")?;
                let f = f.frame;
                if !f.extended && f.id == REPLY_ID && f.len() == 8 && f.data()[0] == op {
                    return Ok::<_, String>(f.data().try_into().unwrap());
                }
            }
        })
        .await;
        match r {
            Ok(r) => r.map(Some),
            Err(_) => Ok(None),
        }
    }

    /// `len` (1..=64) bytes at `address`, four tries of a second each.
    pub async fn read(&mut self, address: u32, len: u8) -> Result<Vec<u8>, String> {
        let req = carpi_core::rnse_service::read(address, len);
        for _ in 0..4 {
            self.send(req).await?;
            let got = tokio::time::timeout(Duration::from_secs(1), async {
                let mut reply = Reply::default();
                loop {
                    let f = self
                        .port
                        .recv()
                        .await
                        .map_err(|e| format!("CAN receive: {e}"))?
                        .ok_or("CAN receive ended")?;
                    if let Some(data) = reply.accept(address, len, &f.frame)? {
                        return Ok::<_, String>(data);
                    }
                }
            })
            .await;
            if let Ok(r) = got {
                return r;
            }
        }
        Err(format!(
            "no complete reply to a read at {address:08X}: is the RNS-E awake?"
        ))
    }

    /// Any length, in 64-byte reads.
    pub async fn read_all(&mut self, address: u32, len: usize) -> Result<Vec<u8>, String> {
        let mut out = Vec::with_capacity(len);
        while out.len() < len {
            let n = (len - out.len()).min(64) as u8;
            out.extend(self.read(address + out.len() as u32, n).await?);
        }
        Ok(out)
    }

    pub async fn u32(&mut self, address: u32) -> Result<u32, String> {
        let b = self.read(address, 4).await?;
        Ok(u32::from_be_bytes(b.try_into().unwrap()))
    }

    pub async fn u16(&mut self, address: u32) -> Result<u16, String> {
        let b = self.read(address, 2).await?;
        Ok(u16::from_be_bytes(b.try_into().unwrap()))
    }
}

/// The firmware's CRC-32 (zlib), incrementally: start with `!0`, end with `!`.
pub fn crc32(mut crc: u32, bytes: &[u8]) -> u32 {
    for byte in bytes {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB88320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(d: &[u8]) -> Frame {
        Frame::new(REPLY_ID, d).unwrap()
    }

    const A: u32 = 0x80D60000;

    #[test]
    fn crc_matches_standard_and_incremental_vector() {
        assert_eq!(!crc32(!0, b"123456789"), 0xCBF43926);
        assert_eq!(!crc32(crc32(!0, b"1234"), b"56789"), 0xCBF43926);
    }

    #[test]
    fn reply_requires_matching_header_all_frames_and_valid_checksums() {
        let mut reply = Reply::default();
        // wrong length, then the right header
        assert_eq!(
            reply
                .accept(A, 64, &frame(&[0x80, 0, 0x80, 0xD6, 0, 0, 48, 8]))
                .unwrap(),
            None
        );
        assert_eq!(
            reply
                .accept(A, 64, &frame(&[0x80, 0, 0x80, 0xD6, 0, 0, 64, 11]))
                .unwrap(),
            None
        );
        for i in 0u8..11 {
            let d = [0x81 + i, i, i, i, i, i, i, i.wrapping_mul(6)];
            let mut bad = d;
            bad[7] ^= 1;
            assert_eq!(reply.accept(A, 64, &frame(&bad)).unwrap(), None);
            let result = reply.accept(A, 64, &frame(&d)).unwrap();
            if i == 10 {
                let data = result.unwrap();
                assert_eq!(data.len(), 64);
                assert_eq!(data[63], 10);
            } else {
                assert_eq!(result, None);
            }
        }
    }

    #[test]
    fn short_reads_are_cut_to_length() {
        let mut reply = Reply::default();
        assert_eq!(
            reply
                .accept(A, 4, &frame(&[0x80, 0, 0x80, 0xD6, 0, 0, 4, 1]))
                .unwrap(),
            None
        );
        let d = [0x81, 1, 2, 3, 4, 9, 9, 28];
        assert_eq!(
            reply.accept(A, 4, &frame(&d)).unwrap(),
            Some(vec![1, 2, 3, 4])
        );
    }

    #[test]
    fn rejection_never_produces_data() {
        assert!(
            Reply::default()
                .accept(A, 64, &frame(&[0x80, 1, 0x80, 0xD6, 0, 0, 64, 0]))
                .is_err()
        );
    }
}
