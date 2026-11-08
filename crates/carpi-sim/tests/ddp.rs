//! The cluster peer against a bare carpi-tp2 channel: handshake replies, claim
//! rules and drawing checks, without carpi-dis in the loop.

use carpi_can::Frame;
use carpi_sim::bus::{Bus, Node};
use carpi_sim::ddp::{ClusterSpec, SimCluster, WHITE_CAPS};
use carpi_tp2::{Channel, Config, Event};
use std::time::{Duration, Instant};

struct Hu {
    ch: Channel,
    got: Vec<Vec<u8>>,
}

impl Node for Hu {
    fn on_frame(&mut self, now: Instant, f: &Frame) {
        self.ch.on_frame(now, f);
        self.drain();
    }
    fn tick(&mut self, now: Instant) {
        self.ch.tick(now);
        self.drain();
    }
    fn poll_transmit(&mut self, _now: Instant) -> Option<Frame> {
        self.ch.poll_transmit()
    }
    fn next_deadline(&self) -> Option<Instant> {
        self.ch.next_deadline()
    }
}

impl Hu {
    fn drain(&mut self) {
        while let Some(e) = self.ch.poll_event() {
            if let Event::Message(m) = e {
                self.got.push(m);
            }
        }
    }
}

fn run(bus: &mut Bus, hu: &mut Hu, cl: &mut SimCluster, d: Duration) {
    let mut nodes: [&mut dyn Node; 2] = [hu, cl];
    bus.run_for(&mut nodes, d);
}

#[test]
fn handshake_claim_and_checks() {
    let t0 = Instant::now();
    let mut bus = Bus::new(t0);
    let mut hu = Hu {
        ch: Channel::fixed(0x6C0, 0x6C1, Config::ddp()),
        got: Vec::new(),
    };
    let mut cl = SimCluster::new(ClusterSpec::default());
    hu.ch.open(t0);
    run(&mut bus, &mut hu, &mut cl, Duration::from_millis(1200));
    assert!(hu.ch.is_open() && cl.is_open());

    let say = |hu: &mut Hu, cl: &mut SimCluster, bus: &mut Bus, m: &[u8]| {
        let now = bus.now;
        hu.ch.send(now, m.to_vec()).unwrap();
        run(bus, hu, cl, Duration::from_millis(200));
    };
    say(
        &mut hu,
        &mut cl,
        &mut bus,
        &[0x15, 0x01, 0x01, 0x02, 0x00, 0x00],
    );
    say(&mut hu, &mut cl, &mut bus, &[0x08]);
    say(&mut hu, &mut cl, &mut bus, &[0x20, 0x3B, 0xA0, 0x00]);
    assert_eq!(hu.got[0], vec![0x00, 0x01]);
    assert_eq!(hu.got[1], WHITE_CAPS.to_vec());
    assert_eq!(hu.got[2], vec![0x21, 0x3B, 0xA0, 0x00]);

    say(
        &mut hu,
        &mut cl,
        &mut bus,
        &[0x52, 0x05, 0x82, 0x00, 0x00, 0x40, 0x58],
    );
    assert_eq!(hu.got[3], vec![0x53, 0xC0], "full-display claim refused");
    say(
        &mut hu,
        &mut cl,
        &mut bus,
        &[0x52, 0x05, 0x82, 0x00, 0x1B, 0x40, 0x30],
    );
    assert_eq!(hu.got[4], vec![0x53, 0x85]);
    assert!(cl.owner);

    let mut big = vec![0x57, 48, 0x06, 0, 0];
    big.extend([0x30; 45]);
    say(&mut hu, &mut cl, &mut bus, &big);
    assert_eq!(hu.got[5], vec![0x0B, 0x03, 0x57]);
    assert_eq!(cl.violations.len(), 1);
}
