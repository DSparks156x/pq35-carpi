//! Read-only sector 96 fingerprint, using the firmware's B1 CAN service.
//! Sends through the daemon's gate and TX worker; no flash/RAM writes or reset.
use carpi_can::{Frame, Port};
use carpi_proto::JobEvent;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

const START: u32 = 0x80C00000;
const SIZE: u32 = 0x20000;
const CHUNK: u32 = 64;

/// One reply consists of a correlated header and eleven checksummed data frames.
#[derive(Default)]
struct Reply {
    header: bool,
    frames: [Option<[u8; 6]>; 11],
}

impl Reply {
    fn accept(&mut self, address: u32, frame: &Frame) -> Result<Option<[u8; 64]>, String> {
        if frame.extended || frame.id != 0x7B6 || frame.len() != 8 {
            return Ok(None);
        }
        let d = frame.data();
        if d[0] == 0x80 {
            if d[2..6] != address.to_be_bytes() || d[6] != 64 {
                return Ok(None);
            }
            if d[1] != 0 {
                return Err(format!(
                    "RNS-E rejected read at {address:08X} (status {})",
                    d[1]
                ));
            }
            if d[7] != 11 {
                return Ok(None);
            }
            self.header = true;
        } else if self.header
            && (0x81..=0x8B).contains(&d[0])
            && d[1..7].iter().fold(0u8, |a, b| a.wrapping_add(*b)) == d[7]
        {
            self.frames[(d[0] - 0x81) as usize] = Some(d[1..7].try_into().unwrap());
        }
        if self.header && self.frames.iter().all(Option::is_some) {
            let mut bytes = [0; 64];
            for (i, slot) in bytes.iter_mut().enumerate() {
                *slot = self.frames[i / 6].unwrap()[i % 6];
            }
            Ok(Some(bytes))
        } else {
            Ok(None)
        }
    }
}

fn crc32(mut crc: u32, bytes: &[u8]) -> u32 {
    for byte in bytes {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB88320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    crc
}

async fn read(
    interface: &str,
    tx: mpsc::Sender<crate::Msg>,
    events: &mpsc::UnboundedSender<JobEvent>,
) -> Result<u32, String> {
    let mut port = Port::open(interface).map_err(|e| format!("opening {interface}: {e}"))?;
    let mut crc = !0;
    for offset in (0..SIZE).step_by(CHUNK as usize) {
        if events.is_closed() {
            return Err("decoder read cancelled: UI disconnected".into());
        }
        let address = START + offset;
        let mut data = None;
        for _ in 0..4 {
            let (reply, accepted) = oneshot::channel();
            tx.send(crate::Msg::DecoderRead(address, reply))
                .await
                .map_err(|_| "carpid stopped")?;
            accepted.await.map_err(|_| "carpid stopped")??;
            let received = tokio::time::timeout(Duration::from_secs(1), async {
                let mut reply = Reply::default();
                loop {
                    let frame = port
                        .recv()
                        .await
                        .map_err(|e| format!("CAN receive: {e}"))?
                        .ok_or("CAN receive ended")?;
                    if let Some(data) = reply.accept(address, &frame.frame)? {
                        return Ok::<_, String>(data);
                    }
                }
            })
            .await;
            match received {
                Ok(result) => {
                    data = Some(result?);
                    break;
                }
                Err(_) => continue,
            }
        }
        let data = data.ok_or_else(|| format!("No complete reply at {address:08X}. Keep the RNS-E awake with B1 read support available"))?;
        crc = crc32(crc, &data);
        let done = offset + CHUNK;
        if done.is_multiple_of(4096) || done == SIZE {
            let _ = events.send(JobEvent::Progress {
                stage: "decoder".into(),
                done: done as u64,
                total: Some(SIZE as u64),
            });
        }
    }
    Ok(!crc)
}

pub async fn run(
    interface: &str,
    tx: mpsc::Sender<crate::Msg>,
    events: &mpsc::UnboundedSender<JobEvent>,
) {
    let _ = events.send(JobEvent::Stage {
        stage: "decoder".into(),
        message: "Reading RNS-E decoder; this can take several minutes".into(),
    });
    let result = tokio::time::timeout(Duration::from_secs(900), read(interface, tx, events)).await;
    let (ok, message) = match result {
        Ok(Ok(crc)) => (true, format!("{crc:08X}")),
        Ok(Err(message)) => (false, message),
        Err(_) => (false, "Decoder read timed out after 15 minutes".into()),
    };
    let _ = events.send(JobEvent::Finished { ok, message });
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(d: &[u8]) -> Frame {
        Frame::new(0x7B6, d).unwrap()
    }

    #[test]
    fn crc_matches_standard_and_incremental_vector() {
        assert_eq!(!crc32(!0, b"123456789"), 0xCBF43926);
        assert_eq!(!crc32(crc32(!0, b"1234"), b"56789"), 0xCBF43926);
    }

    #[test]
    fn reply_requires_matching_header_all_frames_and_valid_checksums() {
        let mut reply = Reply::default();
        assert_eq!(
            reply
                .accept(START, &frame(&[0x80, 0, 0x80, 0xC0, 0, 64, 64, 11]))
                .unwrap(),
            None
        );
        assert_eq!(
            reply
                .accept(START, &frame(&[0x80, 0, 0x80, 0xC0, 0, 0, 64, 11]))
                .unwrap(),
            None
        );
        for i in 0u8..11 {
            let d = [0x81 + i, i, i, i, i, i, i, i.wrapping_mul(6)];
            let mut bad = d;
            bad[7] ^= 1;
            assert_eq!(reply.accept(START, &frame(&bad)).unwrap(), None);
            let result = reply.accept(START, &frame(&d)).unwrap();
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
    fn rejection_never_produces_crc() {
        assert!(
            Reply::default()
                .accept(START, &frame(&[0x80, 1, 0x80, 0xC0, 0, 0, 64, 0]))
                .is_err()
        );
    }

    #[tokio::test]
    async fn unavailable_can_interface_finishes_with_error_not_a_crc() {
        let (tx, _requests) = mpsc::channel(1);
        let (events, mut received) = mpsc::unbounded_channel();
        run("carpi-test-no-such-interface", tx, &events).await;
        assert!(matches!(received.try_recv(), Ok(JobEvent::Stage { .. })));
        match received.try_recv().unwrap() {
            JobEvent::Finished { ok, message } => {
                assert!(!ok);
                assert!(message.contains("opening carpi-test-no-such-interface"));
            }
            other => panic!("expected terminal error: {other:?}"),
        }
    }
}
