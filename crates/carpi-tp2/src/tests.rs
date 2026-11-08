use crate::*;
use carpi_can::Frame;
use std::time::{Duration, Instant};

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

struct T {
    ch: Channel,
    now: Instant,
}

impl T {
    fn new(ch: Channel) -> Self {
        T {
            ch,
            now: Instant::now(),
        }
    }
    fn rx(&mut self, id: u32, data: &[u8]) {
        self.ch.on_frame(self.now, &Frame::new(id, data).unwrap());
    }
    /// Advance time, ticking at every deadline on the way.
    fn advance(&mut self, d: Duration) {
        let end = self.now + d;
        while let Some(dl) = self.ch.next_deadline() {
            if dl > end {
                break;
            }
            self.now = self.now.max(dl);
            self.ch.tick(self.now);
            if self.ch.next_deadline() == Some(dl) {
                break;
            }
        }
        self.now = end;
        self.ch.tick(self.now);
    }
    fn tx(&mut self) -> Vec<(u32, Vec<u8>)> {
        std::iter::from_fn(|| self.ch.poll_transmit())
            .map(|f| (f.id, f.data().to_vec()))
            .collect()
    }
    fn events(&mut self) -> Vec<Event> {
        std::iter::from_fn(|| self.ch.poll_event()).collect()
    }
    fn send(&mut self, m: &[u8]) -> MsgId {
        self.ch.send(self.now, m.to_vec()).unwrap()
    }
}

/// Open the engine channel exactly as in tp2_worker_2.log (tester 0x300 only to
/// match the trace bytes; production never uses 0x300).
fn open_engine() -> T {
    let mut t = T::new(Channel::dynamic(0x01, 0x300, Config::diag()));
    t.ch.open(t.now);
    assert_eq!(
        t.tx(),
        vec![(0x200, vec![0x01, 0xC0, 0x00, 0x10, 0x00, 0x03, 0x01])]
    );
    t.advance(ms(12));
    t.rx(0x201, &[0x00, 0xD0, 0x00, 0x03, 0x40, 0x07, 0x01]);
    assert_eq!(
        t.tx(),
        vec![(0x740, vec![0xA0, 0x0F, 0x8A, 0xFF, 0x32, 0xFF])]
    );
    t.advance(ms(12));
    t.rx(0x300, &[0xA1, 0x0F, 0x8A, 0xFF, 0x4A, 0xFF]);
    assert_eq!(t.events(), vec![Event::Opened { generation: 1 }]);
    assert_eq!(t.ch.peer_params(), Params::DDP);
    t
}

#[test]
fn engine_trace_replay() {
    let mut t = open_engine();
    let id = t.send(&[0x10, 0x89]);
    assert_eq!(t.tx(), vec![(0x740, vec![0x10, 0x00, 0x02, 0x10, 0x89])]);
    t.advance(ms(8));
    t.rx(0x300, &[0xB1]);
    assert_eq!(t.events(), vec![Event::Sent(id)]);
    t.advance(ms(30));
    t.rx(0x300, &[0x10, 0x00, 0x02, 0x50, 0x89]);
    assert_eq!(t.tx(), vec![(0x740, vec![0xB1])]);
    assert_eq!(t.events(), vec![Event::Message(vec![0x50, 0x89])]);

    // 21 03, the four-frame reply from the log.
    t.send(&[0x21, 0x03]);
    assert_eq!(t.tx(), vec![(0x740, vec![0x11, 0x00, 0x02, 0x21, 0x03])]);
    t.rx(0x300, &[0xB2]);
    for f in [
        &[0x21, 0x00, 0x1A, 0x61, 0x03, 0x01, 0xC8, 0x15][..],
        &[0x22, 0x19, 0x20, 0x03, 0x21, 0xFF, 0x0D, 0x1B],
        &[0x23, 0x4B, 0x79, 0x05, 0x0A, 0xA0, 0x05, 0x0A],
    ] {
        t.rx(0x300, f);
        assert!(t.tx().is_empty(), "no ACK for 2n");
    }
    t.rx(0x300, &[0x14, 0x7D, 0x25, 0x00, 0x00, 0x25, 0x00, 0x00]);
    assert_eq!(t.tx(), vec![(0x740, vec![0xB5])]);
    let ev = t.events();
    let Some(Event::Message(m)) = ev.last() else {
        panic!("{ev:?}")
    };
    assert_eq!(m.len(), 26);
    assert_eq!(&m[..5], &[0x61, 0x03, 0x01, 0xC8, 0x15]);

    // Second read continues the sequence: TX 12, ACK B9 after 25..18.
    t.send(&[0x21, 0x03]);
    assert_eq!(t.tx(), vec![(0x740, vec![0x12, 0x00, 0x02, 0x21, 0x03])]);
    t.rx(0x300, &[0xB3]);
    t.rx(0x300, &[0x25, 0x00, 0x1A, 0x61, 0x03, 0x01, 0xC8, 0x15]);
    t.rx(0x300, &[0x26, 0x19, 0x25, 0x03, 0x21, 0xFF, 0x0D, 0x1B]);
    t.rx(0x300, &[0x27, 0x4B, 0x79, 0x05, 0x0A, 0xA0, 0x05, 0x0A]);
    t.rx(0x300, &[0x18, 0x7D, 0x25, 0x00, 0x00, 0x25, 0x00, 0x00]);
    assert_eq!(t.tx(), vec![(0x740, vec![0xB9])]);

    t.ch.close(t.now);
    assert_eq!(t.tx(), vec![(0x740, vec![0xA8])]);
    assert!(matches!(
        t.events().last(),
        Some(Event::Closed {
            reason: CloseReason::Local,
            ..
        })
    ));
}

#[test]
fn keepalive_and_timeout() {
    let mut t = open_engine();
    t.advance(ms(999));
    assert!(t.tx().is_empty());
    t.advance(ms(1));
    assert_eq!(t.tx(), vec![(0x740, vec![0xA3])]);
    t.advance(ms(8));
    t.rx(0x300, &[0xA1, 0x0F, 0x8A, 0xFF, 0x4A, 0xFF]);
    // Peer's own A3 gets our A1.
    t.rx(0x300, &[0xA3]);
    assert_eq!(
        t.tx(),
        vec![(0x740, vec![0xA1, 0x0F, 0x8A, 0xFF, 0x32, 0xFF])]
    );
    t.advance(ms(1000));
    assert_eq!(t.tx(), vec![(0x740, vec![0xA3])]);
    // No answer.
    t.advance(ms(500));
    assert_eq!(t.tx(), vec![(0x740, vec![0xA8])]);
    assert_eq!(
        t.events(),
        vec![Event::Closed {
            generation: 1,
            reason: CloseReason::KeepaliveTimeout
        }]
    );
    assert_eq!(t.ch.state(), State::Idle);
    assert_eq!(t.ch.tx_id(), 0);
}

#[test]
fn foreign_setup_reply_ignored_then_timeout() {
    let mut t = T::new(Channel::dynamic(0x01, 0x308, Config::diag()));
    t.ch.open(t.now);
    assert_eq!(t.tx()[0].1, vec![0x01, 0xC0, 0x00, 0x10, 0x08, 0x03, 0x01]);
    // VCDS's channel on 0x300.
    t.rx(0x201, &[0x00, 0xD0, 0x00, 0x03, 0x40, 0x07, 0x01]);
    assert!(t.tx().is_empty());
    t.advance(ms(500));
    assert_eq!(
        t.events(),
        vec![Event::Closed {
            generation: 0,
            reason: CloseReason::SetupTimeout
        }]
    );
}

#[test]
fn setup_refused() {
    let mut t = T::new(Channel::dynamic(0x0C, 0x308, Config::diag()));
    t.ch.open(t.now);
    t.tx();
    t.rx(0x20C, &[0x00, 0xD6, 0x00, 0x00, 0x00, 0x00, 0x00]);
    assert_eq!(
        t.events(),
        vec![Event::Closed {
            generation: 0,
            reason: CloseReason::SetupRefused(0xD6)
        }]
    );
}

fn ddp_open(peer_bs: u8) -> T {
    let mut t = T::new(Channel::fixed(0x6C0, 0x6C1, Config::ddp()));
    t.ch.open(t.now);
    assert_eq!(t.tx(), vec![(0x6C0, vec![0xA8])]);
    t.advance(ms(999));
    assert!(t.tx().is_empty());
    t.advance(ms(1));
    assert_eq!(
        t.tx(),
        vec![(0x6C0, vec![0xA0, 0x0F, 0x8A, 0xFF, 0x4A, 0xFF])]
    );
    t.rx(0x6C1, &[0xA1, peer_bs, 0x8A, 0xFF, 0x4A, 0xFF]);
    assert_eq!(t.events(), vec![Event::Opened { generation: 1 }]);
    t
}

#[test]
fn ddp_passive_open_and_stale() {
    let mut t = T::new(Channel::fixed(0x6C0, 0x6C1, Config::ddp()));
    // Stale session traffic before anything: A8 back.
    t.rx(0x6C1, &[0xA3]);
    assert_eq!(t.tx(), vec![(0x6C0, vec![0xA8])]);
    // Cluster opens first.
    t.rx(0x6C1, &[0xA0, 0x0F, 0x8A, 0xFF, 0x4A, 0xFF]);
    assert_eq!(
        t.tx(),
        vec![(0x6C0, vec![0xA1, 0x0F, 0x8A, 0xFF, 0x4A, 0xFF])]
    );
    assert_eq!(t.events(), vec![Event::Opened { generation: 1 }]);
    // Duplicate A0 straight after: answered, no reset.
    t.rx(0x6C1, &[0xA0, 0x0F, 0x8A, 0xFF, 0x4A, 0xFF]);
    assert_eq!(t.tx().len(), 1);
    assert!(t.events().is_empty());
    // Unprefixed message from the cluster: ends at 1n.
    t.rx(0x6C1, &[0x10, 0x00, 0x01]);
    assert_eq!(t.tx(), vec![(0x6C0, vec![0xB1])]);
    assert_eq!(t.events(), vec![Event::Message(vec![0x00, 0x01])]);
    // A0 after traffic: the cluster restarted.
    t.rx(0x6C1, &[0xA0, 0x0F, 0x8A, 0xFF, 0x4A, 0xFF]);
    assert_eq!(
        t.events(),
        vec![
            Event::Closed {
                generation: 1,
                reason: CloseReason::PeerRestart
            },
            Event::Opened { generation: 2 }
        ]
    );
}

#[test]
fn ddp_blocks_not_ready_and_replay() {
    let mut t = ddp_open(3);
    let msg: Vec<u8> = (0..30).collect();
    let id = t.send(&msg);
    // BS 3: 20 21 02 | 23 14.
    let f = t.tx();
    assert_eq!(
        f.iter().map(|(_, d)| d[0]).collect::<Vec<_>>(),
        vec![0x20, 0x21, 0x02]
    );
    assert_eq!(f[0].1[1..], msg[..7]);
    // Cluster didn't take frame 2: 92.
    t.rx(0x6C1, &[0x92]);
    assert_eq!(t.events(), vec![Event::NotReady]);
    t.advance(ms(99));
    assert!(t.tx().is_empty());
    t.advance(ms(1));
    assert_eq!(
        t.tx().iter().map(|(_, d)| d[0]).collect::<Vec<_>>(),
        vec![0x02]
    );
    t.rx(0x6C1, &[0xB3]);
    assert_eq!(
        t.tx().iter().map(|(_, d)| d[0]).collect::<Vec<_>>(),
        vec![0x23, 0x14]
    );
    t.rx(0x6C1, &[0xB5]);
    assert_eq!(t.events(), vec![Event::Sent(id)]);
    // Next message starts at seq 5.
    t.send(&[0x33]);
    assert_eq!(t.tx(), vec![(0x6C0, vec![0x15, 0x33])]);
}

#[test]
fn missing_ack_resends_last_frame_then_closes() {
    let mut t = ddp_open(15);
    t.send(&[1, 2, 3, 4, 5, 6, 7, 8, 9]);
    assert_eq!(t.tx().len(), 2);
    t.advance(ms(1000));
    assert_eq!(t.tx(), vec![(0x6C0, vec![0x11, 8, 9])]);
    t.advance(ms(1000));
    assert_eq!(t.tx(), vec![(0x6C0, vec![0x11, 8, 9])]);
    t.advance(ms(1000));
    assert_eq!(t.tx(), vec![(0x6C0, vec![0xA8])]);
    assert_eq!(
        t.events(),
        vec![Event::Closed {
            generation: 1,
            reason: CloseReason::AckTimeout
        }]
    );
    assert_eq!(t.ch.send(t.now, vec![1]), Err(SendError::NotOpen));
}

#[test]
fn not_ready_limit() {
    let mut t = ddp_open(15);
    t.send(&[1]);
    t.tx();
    for _ in 0..5 {
        t.rx(0x6C1, &[0x90]);
        t.advance(ms(100));
        assert_eq!(t.tx(), vec![(0x6C0, vec![0x10, 1])]);
    }
    t.rx(0x6C1, &[0x90]);
    assert_eq!(t.tx(), vec![(0x6C0, vec![0xA8])]);
    assert!(t.events().contains(&Event::Closed {
        generation: 1,
        reason: CloseReason::NotReadyLimit
    }));
}

#[test]
fn duplicate_frame_reacked_and_sequence_error() {
    let mut t = ddp_open(15);
    t.rx(0x6C1, &[0x10, 0xAA]);
    assert_eq!(t.tx(), vec![(0x6C0, vec![0xB1])]);
    // Our B1 was lost; the cluster resends.
    t.rx(0x6C1, &[0x10, 0xAA]);
    assert_eq!(t.tx(), vec![(0x6C0, vec![0xB1])]);
    assert_eq!(t.events(), vec![Event::Message(vec![0xAA])]);
    // Out of sequence.
    t.rx(0x6C1, &[0x13, 0xBB]);
    assert_eq!(t.tx(), vec![(0x6C0, vec![0xA8])]);
    assert_eq!(
        t.events(),
        vec![Event::Closed {
            generation: 1,
            reason: CloseReason::Sequence {
                expected: 1,
                got: 3
            }
        }]
    );
}

#[test]
fn length_flag_and_break() {
    let mut t = open_engine();
    // Pending reply with bit 15 set in the length word.
    t.rx(0x300, &[0x10, 0x80, 0x03, 0x7F, 0x21, 0x78]);
    t.tx();
    assert_eq!(t.events(), vec![Event::Message(vec![0x7F, 0x21, 0x78])]);
    // 0n block, then a frame, then A4: drop back to the 0n mark.
    t.rx(0x300, &[0x01, 0x00, 0x0A, 1, 2, 3, 4, 5]);
    assert_eq!(t.tx(), vec![(0x740, vec![0xB2])]);
    t.rx(0x300, &[0x22, 0xEE, 0xEE]);
    t.rx(0x300, &[0xA4]);
    t.rx(0x300, &[0x12, 6, 7, 8, 9, 10]);
    assert_eq!(t.tx(), vec![(0x740, vec![0xB3])]);
    assert_eq!(
        t.events(),
        vec![Event::Message(vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10])]
    );
    // Wrong length closes.
    t.rx(0x300, &[0x13, 0x00, 0x05, 1]);
    assert!(matches!(
        t.events().last(),
        Some(Event::Closed {
            reason: CloseReason::Length {
                declared: 5,
                got: 1
            },
            ..
        })
    ));
}

#[test]
fn peer_disconnect_and_reopen() {
    let mut t = ddp_open(15);
    t.rx(0x6C1, &[0xA8]);
    assert_eq!(t.tx(), vec![(0x6C0, vec![0xA8])]);
    assert_eq!(
        t.events(),
        vec![Event::Closed {
            generation: 1,
            reason: CloseReason::PeerDisconnect
        }]
    );
    t.ch.open(t.now);
    t.advance(ms(1000));
    t.tx();
    t.rx(0x6C1, &[0xA1, 0x0F, 0x8A, 0xFF, 0x4A, 0xFF]);
    assert_eq!(t.events(), vec![Event::Opened { generation: 2 }]);
}

#[test]
fn peer_t3_paces_frames() {
    let mut t = open_engine();
    // Engine T3 is 10 ms; a 19-byte request plus length is 3 frames.
    t.send(&[0x22; 19]);
    assert_eq!(t.tx().len(), 1);
    t.advance(ms(9));
    assert!(t.tx().is_empty());
    t.advance(ms(1));
    assert_eq!(t.tx().len(), 1);
    t.advance(ms(10));
    let f = t.tx();
    assert_eq!(f[0].1[0], 0x12);
}

#[test]
fn tester_ids_never_0x300() {
    let mut ids = TesterIds::new(0x300..=0x302);
    assert_eq!(ids.available(), 2);
    assert_eq!(ids.alloc(), Some(0x301));
    assert_eq!(ids.alloc(), Some(0x302));
    assert_eq!(ids.alloc(), None);
    ids.release(0x301);
    ids.release(0x300);
    assert_eq!(ids.alloc(), Some(0x301));
    let d = TesterIds::default();
    assert_eq!(d.available(), 8);
}
