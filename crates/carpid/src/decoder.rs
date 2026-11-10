//! Read-only sector 107 fingerprint, using the firmware's B1 CAN service.
//! Sends through the daemon's gate and TX worker; no flash/RAM writes or reset.
use crate::rnse_bus::{RnseBus, crc32};
use carpi_core::rnse_service::{SECTOR_SIZE, SECTOR_XIP};
use carpi_proto::JobEvent;
use std::time::Duration;
use tokio::sync::mpsc;

const CHUNK: u32 = 64;

/// CRC-32 of the whole live sector 107, with progress under `stage`.
pub async fn sector_crc(
    bus: &mut RnseBus,
    stage: &str,
    events: &mpsc::UnboundedSender<JobEvent>,
) -> Result<u32, String> {
    let mut crc = !0;
    for offset in (0..SECTOR_SIZE).step_by(CHUNK as usize) {
        if events.is_closed() {
            return Err("decoder read cancelled: UI disconnected".into());
        }
        let data = bus.read(SECTOR_XIP + offset, CHUNK as u8).await?;
        crc = crc32(crc, &data);
        let done = offset + CHUNK;
        if done.is_multiple_of(4096) || done == SECTOR_SIZE {
            let _ = events.send(JobEvent::Progress {
                stage: stage.into(),
                done: done as u64,
                total: Some(SECTOR_SIZE as u64),
            });
        }
    }
    Ok(!crc)
}

async fn read(
    interface: &str,
    tx: mpsc::Sender<crate::Msg>,
    events: &mpsc::UnboundedSender<JobEvent>,
) -> Result<u32, String> {
    let mut bus = RnseBus::open(interface, tx)?;
    sector_crc(&mut bus, "decoder", events).await
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
