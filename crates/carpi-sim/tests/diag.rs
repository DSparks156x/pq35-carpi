//! carpi-tp2 + carpi-kwp against the simulated gateway, on virtual time.

use carpi_can::Frame;
use carpi_kwp::group::{self, Value};
use carpi_kwp::services;
use carpi_kwp::{KwpClient, KwpError, KwpEvent, Profiles, Request};
use carpi_sim::bus::{Bus, Node};
use carpi_sim::tp2::Gateway;
use carpi_tp2::{Channel, CloseReason, Config};
use std::time::{Duration, Instant};

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

struct Tester {
    c: KwpClient,
    ev: Vec<KwpEvent>,
}

impl Tester {
    fn new(module: u8, tester_rx: u16) -> Self {
        Tester {
            c: KwpClient::new(
                Channel::dynamic(module, tester_rx, Config::diag()),
                &Profiles::default(),
            ),
            ev: Vec::new(),
        }
    }
    fn drain(&mut self) {
        while let Some(e) = self.c.poll_event() {
            self.ev.push(e);
        }
    }
    fn opened(&self) -> Option<u32> {
        self.ev.iter().rev().find_map(|e| match e {
            KwpEvent::Opened { generation } => Some(*generation),
            _ => None,
        })
    }
    fn done(&mut self) -> Option<Result<Vec<u8>, KwpError>> {
        let i = self
            .ev
            .iter()
            .position(|e| matches!(e, KwpEvent::Done { .. }))?;
        match self.ev.remove(i) {
            KwpEvent::Done { result, .. } => Some(result),
            _ => unreachable!(),
        }
    }
}

impl Node for Tester {
    fn on_frame(&mut self, now: Instant, f: &Frame) {
        self.c.on_frame(now, f);
        self.drain();
    }
    fn tick(&mut self, now: Instant) {
        self.c.tick(now);
        self.drain();
    }
    fn poll_transmit(&mut self, _now: Instant) -> Option<Frame> {
        self.c.poll_transmit()
    }
    fn next_deadline(&self) -> Option<Instant> {
        self.c.next_deadline()
    }
}

struct World {
    bus: Bus,
    gw: Gateway,
}

impl World {
    fn new() -> Self {
        World {
            bus: Bus::new(Instant::now()),
            gw: Gateway::tt(),
        }
    }

    fn run(&mut self, t: &mut Tester, d: Duration) {
        self.bus.run_for(&mut [t, &mut self.gw], d);
    }

    fn open(&mut self, t: &mut Tester) -> u32 {
        t.c.open(self.bus.now).unwrap();
        self.run(t, ms(60));
        t.opened().expect("opened")
    }

    /// Run until the request completes; returns the result and the wall time.
    fn request(&mut self, t: &mut Tester, r: Request) -> (Result<Vec<u8>, KwpError>, Duration) {
        let start = self.bus.now;
        if let Err(e) = t.c.request(start, r) {
            return (Err(e), Duration::ZERO);
        }
        for _ in 0..10_000 {
            self.run(t, ms(1));
            if let Some(res) = t.done() {
                return (res, self.bus.now - start);
            }
        }
        panic!("request never finished");
    }
}

fn num(v: &Value) -> f64 {
    match v {
        Value::Number(n) => *n,
        Value::Text(t) => panic!("{t}"),
    }
}

#[test]
fn open_session_and_read_groups() {
    let mut w = World::new();
    let mut t = Tester::new(0x01, 0x308);
    assert_eq!(w.open(&mut t), 1);
    assert_eq!(w.gw.setups, vec![(0x01, 0x308)]);

    let (r, _) = w.request(&mut t, services::start_session());
    assert_eq!(r.unwrap(), vec![0x50, 0x89]);

    let (r, took) = w.request(&mut t, group::read_group(3));
    let g = group::parse_group(&r.unwrap(), 3).unwrap();
    // Car: request to complete ~48 ms; first frame 23-40 ms.
    assert!(took >= ms(30) && took <= ms(80), "{took:?}");
    let rpm = g.records[0].decode().unwrap();
    assert_eq!((num(&rpm.value), rpm.unit), (840.0, "rpm"));

    let (r, _) = w.request(&mut t, group::read_group(113));
    let g = group::parse_group(&r.unwrap(), 113).unwrap();
    assert_eq!(num(&g.records[3].decode().unwrap().value), 795.0);

    let (r, _) = w.request(&mut t, group::read_group(99));
    assert_eq!(r, Err(KwpError::Negative { nrc: 0x31 }));

    // Keepalive keeps it open across idle time.
    w.run(&mut t, ms(5000));
    assert!(t.c.is_open());
    let (r, _) = w.request(&mut t, services::read_dtcs());
    assert_eq!(services::parse_dtcs(&r.unwrap()).unwrap()[0].code, 17965);
}

#[test]
fn install_list_from_gateway() {
    let mut w = World::new();
    let mut t = Tester::new(services::GATEWAY, 0x309);
    w.open(&mut t);
    let (r, _) = w.request(&mut t, services::read_install_list());
    let l = services::parse_install_list(&r.unwrap()).unwrap();
    assert_eq!(l.len(), 6);
    let awd = l.iter().find(|e| e.vcds == 0x22).unwrap();
    assert_eq!(awd.tp2, 0x0A);
    let ksg = l.iter().find(|e| e.vcds == 0x46).unwrap();
    assert!(ksg.coded() && ksg.unreachable());
}

#[test]
fn survives_not_ready() {
    let mut w = World::new();
    let mut t = Tester::new(0x01, 0x308);
    w.open(&mut t);
    w.gw.module(0x01).faults.not_ready = 2;
    let (r, took) = w.request(&mut t, group::read_group(3));
    assert!(r.is_ok());
    // Two 100 ms waits before the request frame was taken.
    assert!(took >= ms(200), "{took:?}");
    let nines = w
        .bus
        .log
        .iter()
        .filter(|(_, _, f)| f.id == 0x308 && f.data()[0] >> 4 == 0x9)
        .count();
    assert_eq!(nines, 2);
    assert_eq!(w.gw.module(0x01).requests, vec![vec![0x21, 0x03]]);
}

#[test]
fn survives_lost_ack() {
    let mut w = World::new();
    let mut t = Tester::new(0x01, 0x308);
    w.open(&mut t);
    w.gw.module(0x01).faults.lose_acks = 1;
    let (r, _) = w.request(&mut t, group::read_group(3));
    assert!(r.is_ok());
    // Our request frame goes out again after the ACK timeout and gets re-ACKed.
    w.run(&mut t, ms(600));
    let req_frames = w
        .bus
        .sent_by(0)
        .filter(|(_, id, d)| *id == 0x740 && d[0] == 0x10)
        .count();
    assert_eq!(req_frames, 2);
    assert!(t.c.is_open());
    // Only one request reached the module.
    assert_eq!(w.gw.module(0x01).requests.len(), 1);
    let (r, _) = w.request(&mut t, group::read_group(113));
    assert!(r.is_ok());
}

#[test]
fn response_pending() {
    let mut w = World::new();
    let mut t = Tester::new(0x01, 0x308);
    w.open(&mut t);
    w.gw.module(0x01).faults.pending = 5;
    let (r, took) = w.request(&mut t, services::read_dtcs());
    assert!(r.is_ok());
    assert!(took > ms(2500), "{took:?}");
    let pend =
        t.ev.iter()
            .filter(|e| matches!(e, KwpEvent::Pending(_)))
            .count();
    assert_eq!(pend, 5);
}

#[test]
fn dropout_then_reopen() {
    let mut w = World::new();
    let mut t = Tester::new(0x01, 0x308);
    assert_eq!(w.open(&mut t), 1);
    w.gw.module(0x01).faults.silent = true;
    let (r, _) = w.request(&mut t, group::read_group(3));
    // The request frame's ACK never comes: 3 x 500 ms, then the link closes.
    assert!(
        matches!(r, Err(KwpError::LinkClosed(CloseReason::AckTimeout))),
        "{r:?}"
    );
    w.run(&mut t, ms(3000));
    assert!(!t.c.is_open());

    // Still gone: setup times out.
    t.c.open(w.bus.now).unwrap();
    w.run(&mut t, ms(600));
    assert!(t.ev.iter().any(|e| matches!(
        e,
        KwpEvent::Closed {
            reason: CloseReason::SetupTimeout,
            ..
        }
    )));

    // Back: new generation, reads work.
    w.gw.module(0x01).faults.silent = false;
    assert_eq!(w.open(&mut t), 2);
    let (r, _) = w.request(&mut t, group::read_group(3));
    assert!(r.is_ok());
}

#[test]
fn refused_setup() {
    let mut w = World::new();
    let mut t = Tester::new(0x01, 0x308);
    w.gw.module(0x01).faults.refuse_setup = Some(0xD6);
    t.c.open(w.bus.now).unwrap();
    w.run(&mut t, ms(50));
    assert!(t.ev.contains(&KwpEvent::Closed {
        generation: 0,
        reason: CloseReason::SetupRefused(0xD6)
    }));
}

#[test]
fn multi_block_request() {
    let mut w = World::new();
    // Module asks for ACKs every 2 frames.
    w.gw.module(0x01).spec.params[0] = 0x02;
    let mut t = Tester::new(0x01, 0x308);
    w.open(&mut t);
    let mut req = vec![0x22];
    req.extend(1..=20u8);
    let (r, _) = w.request(&mut t, Request::read_only(req.clone()).unwrap());
    assert_eq!(r, Err(KwpError::Negative { nrc: 0x11 }));
    assert_eq!(w.gw.module(0x01).requests, vec![req]);
    let heads: Vec<u8> = w
        .bus
        .sent_by(0)
        .filter(|(_, id, d)| *id == 0x740 && d[0] >> 4 <= 3)
        .map(|(_, _, d)| d[0])
        .collect();
    assert_eq!(heads, vec![0x20, 0x01, 0x22, 0x13]);
}

#[test]
fn two_channels_interleave() {
    let mut bus = Bus::new(Instant::now());
    let mut gw = Gateway::tt();
    let mut eng = Tester::new(0x01, 0x308);
    let mut hal = Tester::new(0x0A, 0x309);
    eng.c.open(bus.now).unwrap();
    hal.c.open(bus.now).unwrap();
    bus.run_for(&mut [&mut eng, &mut hal, &mut gw], ms(60));
    assert!(eng.opened().is_some() && hal.opened().is_some());

    let start = bus.now;
    let (mut ne, mut nh) = (0, 0);
    while ne < 10 || nh < 10 {
        if ne < 10 && !eng.c.busy() {
            eng.c.request(bus.now, group::read_group(3)).unwrap();
        }
        if nh < 10 && !hal.c.busy() {
            hal.c.request(bus.now, group::read_group(1)).unwrap();
        }
        bus.run_for(&mut [&mut eng, &mut hal, &mut gw], ms(1));
        if let Some(r) = eng.done() {
            r.unwrap();
            ne += 1;
        }
        if let Some(r) = hal.done() {
            r.unwrap();
            nh += 1;
        }
        assert!(bus.now - start < ms(5000));
    }
    // 20 reads at ~40-60 ms each would be ~1 s back to back; interleaved is ~half.
    assert!(bus.now - start < ms(700), "{:?}", bus.now - start);
}

#[test]
fn abs_never_opened() {
    let mut w = World::new();
    let mut t = Tester::new(carpi_kwp::ABS, 0x30A);
    assert_eq!(t.c.open(w.bus.now), Err(KwpError::Denied));
    w.run(&mut t, ms(100));
    assert!(w.gw.setups.is_empty());
}
