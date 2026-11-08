//! Client tests against a hand-scripted engine peer (bytes from the car trace).

use crate::*;
use carpi_can::Frame;
use carpi_tp2::{Channel, CloseReason, Config};
use std::time::{Duration, Instant};

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

struct T {
    c: KwpClient,
    now: Instant,
    /// Peer's next TX sequence.
    seq: u8,
}

impl T {
    fn engine(profiles: &Profiles) -> T {
        let mut t = T {
            // Keepalive off: this scripted peer doesn't answer A3 (the sim does).
            c: KwpClient::new(
                Channel::dynamic(
                    0x01,
                    0x308,
                    Config {
                        keepalive: None,
                        ..Config::diag()
                    },
                ),
                profiles,
            ),
            now: Instant::now(),
            seq: 0,
        };
        t.c.open(t.now).unwrap();
        t.rx(0x201, &[0x00, 0xD0, 0x08, 0x03, 0x40, 0x07, 0x01]);
        t.rx(0x308, &[0xA1, 0x0F, 0x8A, 0xFF, 0x4A, 0xFF]);
        t.tx();
        assert_eq!(t.events(), vec![KwpEvent::Opened { generation: 1 }]);
        t
    }
    fn rx(&mut self, id: u32, d: &[u8]) {
        self.c.on_frame(self.now, &Frame::new(id, d).unwrap());
    }
    /// Peer sends one short (single-frame) KWP message.
    fn reply(&mut self, msg: &[u8]) {
        let mut f = vec![0x10 | self.seq, 0x00, msg.len() as u8];
        f.extend_from_slice(msg);
        self.seq = (self.seq + 1) & 0x0F;
        self.rx(0x308, &f);
    }
    fn tx(&mut self) -> Vec<Vec<u8>> {
        std::iter::from_fn(|| self.c.poll_transmit())
            .map(|f| f.data().to_vec())
            .collect()
    }
    fn events(&mut self) -> Vec<KwpEvent> {
        std::iter::from_fn(|| self.c.poll_event()).collect()
    }
    fn advance(&mut self, d: Duration) {
        let end = self.now + d;
        while let Some(dl) = self.c.next_deadline() {
            if dl > end {
                break;
            }
            self.now = self.now.max(dl);
            self.c.tick(self.now);
            if self.c.next_deadline() == Some(dl) {
                break;
            }
        }
        self.now = end;
        self.c.tick(self.now);
    }
}

#[test]
fn read_group_positive() {
    let mut t = T::engine(&Profiles::default());
    let id = t.c.request(t.now, group::read_group(3)).unwrap();
    assert_eq!(t.tx(), vec![vec![0x10, 0x00, 0x02, 0x21, 0x03]]);
    t.rx(0x308, &[0xB1]);
    t.reply(&[0x61, 0x03, 0x01, 0xC8, 0x15]);
    let ev = t.events();
    assert_eq!(
        ev,
        vec![KwpEvent::Done {
            id,
            result: Ok(vec![0x61, 0x03, 0x01, 0xC8, 0x15])
        }]
    );
}

#[test]
fn response_pending_then_positive() {
    let mut t = T::engine(&Profiles::default());
    let id = t.c.request(t.now, services::read_dtcs()).unwrap();
    t.rx(0x308, &[0xB1]);
    for _ in 0..2 {
        t.advance(ms(1900));
        // Pending reply with length flag bit set.
        let s = t.seq;
        t.seq += 1;
        t.rx(0x308, &[0x10 | s, 0x80, 0x03, 0x7F, 0x18, 0x78]);
        assert_eq!(t.events(), vec![KwpEvent::Pending(id)]);
    }
    // Past the 2 s first-response timeout, still alive thanks to the 78s.
    t.advance(ms(4000));
    assert!(t.events().is_empty());
    t.reply(&[0x58, 0x00]);
    assert_eq!(
        t.events(),
        vec![KwpEvent::Done {
            id,
            result: Ok(vec![0x58, 0x00])
        }]
    );
}

#[test]
fn timeout_negative_and_mismatch() {
    let mut t = T::engine(&Profiles::default());
    let id = t.c.request(t.now, services::read_ident()).unwrap();
    assert_eq!(
        t.c.request(t.now, services::read_ident()),
        Err(KwpError::Busy)
    );
    t.rx(0x308, &[0xB1]);
    t.advance(ms(2000));
    let ev = t.events();
    assert!(ev.contains(&KwpEvent::Done {
        id,
        result: Err(KwpError::Timeout)
    }));

    let id = t.c.request(t.now, services::read_ident()).unwrap();
    t.reply(&[0x7F, 0x1A, 0x11]);
    assert!(t.events().contains(&KwpEvent::Done {
        id,
        result: Err(KwpError::Negative { nrc: 0x11 })
    }));

    let id = t.c.request(t.now, group::read_group(3)).unwrap();
    t.reply(&[0x61, 0x04]);
    assert!(t.events().contains(&KwpEvent::Done {
        id,
        result: Err(KwpError::Mismatch(vec![0x61, 0x04]))
    }));
}

#[test]
fn busy_retry() {
    let mut p = Profiles::default();
    p.default.busy_retries = 1;
    let mut t = T::engine(&p);
    let id = t.c.request(t.now, group::read_group(1)).unwrap();
    t.tx();
    t.rx(0x308, &[0xB1]);
    t.reply(&[0x7F, 0x21, 0x21]);
    t.advance(ms(200));
    // Our ACK of the 7F, then the resend.
    assert_eq!(t.tx(), vec![vec![0xB1], vec![0x11, 0x00, 0x02, 0x21, 0x01]]);
    t.reply(&[0x7F, 0x21, 0x21]);
    assert!(t.events().contains(&KwpEvent::Done {
        id,
        result: Err(KwpError::Negative { nrc: 0x21 })
    }));
}

#[test]
fn link_close_fails_request() {
    let mut t = T::engine(&Profiles::default());
    let id = t.c.request(t.now, group::read_group(1)).unwrap();
    t.rx(0x308, &[0xA8]);
    assert_eq!(
        t.events(),
        vec![
            KwpEvent::Done {
                id,
                result: Err(KwpError::LinkClosed(CloseReason::PeerDisconnect))
            },
            KwpEvent::Closed {
                generation: 1,
                reason: CloseReason::PeerDisconnect
            }
        ]
    );
    assert_eq!(
        t.c.request(t.now, group::read_group(1)),
        Err(KwpError::NotOpen)
    );
}

#[test]
fn abs_denied_by_default() {
    let p = Profiles::default();
    assert!(p.is_denied(ABS));
    assert!(!p.is_denied(0x01));
    let mut c = KwpClient::new(Channel::dynamic(ABS, 0x309, Config::diag()), &p);
    let now = Instant::now();
    assert_eq!(c.open(now), Err(KwpError::Denied));
    assert!(c.poll_transmit().is_none());
    // A deliberate job under a token can still talk to it.
    let tok = ExclusiveToken::mint(1, Class::ReadOnly);
    c.open_exclusive(now, &tok);
    assert_eq!(c.poll_transmit().unwrap().data()[..2], [0x03, 0xC0]);
    let r = Request::exclusive(vec![0x18, 0x02, 0xFF, 0x00], &tok).unwrap();
    assert_eq!(c.request(now, group::read_group(1)), Err(KwpError::Denied));
    // Not open yet, but no longer denied.
    assert_eq!(c.request(now, r), Err(KwpError::NotOpen));
}
