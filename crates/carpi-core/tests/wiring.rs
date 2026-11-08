//! The DIS, top lines and diag as wired into the car model: everything they
//! send goes through the gate, top lines only on TV, API subscriptions map
//! onto diag subscriptions.

use carpi_can::recorder::Recorder;
use carpi_can::{Frame, RxFrame};
use carpi_core::{Car, CoreConfig, DisSetup, IfaceSpec, Output};
use carpi_gate::{Refused, TxClass};
use carpi_proto::{BridgeUpdate, DiagAction, MediaState, NavState, Request};
use std::time::{Duration, Instant, SystemTime};

fn text(r: &Rig, name: &str) -> Option<String> {
    match r.car.store().get(name, r.now) {
        Some(carpi_state::Value::Text(s)) => Some(s.clone()),
        _ => None,
    }
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

struct Rig {
    car: Car,
    now: Instant,
    kl15: bool,
    on_tv: bool,
    ring: bool,
    sent: Vec<Frame>,
}

impl Rig {
    fn new(dis: bool) -> Self {
        let mut car = Car::new(
            vec![IfaceSpec {
                name: "can0".into(),
                roles: vec!["ican".into(), "diag".into()],
            }],
            Recorder::new(Duration::from_secs(1), 100),
            std::env::temp_dir(),
        );
        let now = Instant::now();
        car.configure(
            CoreConfig {
                dis: DisSetup {
                    enabled: dis,
                    ..Default::default()
                },
                ..Default::default()
            },
            now,
        );
        Rig {
            car,
            now,
            kl15: false,
            on_tv: false,
            ring: true,
            sent: Vec::new(),
        }
    }

    fn feed(&mut self, id: u32, data: &[u8]) {
        let rx = RxFrame {
            frame: Frame::new(id, data).unwrap(),
            at: self.now,
            wall: SystemTime::now(),
        };
        self.car.ingest(0, &rx);
    }

    /// `n` steps of 100 ms: ring, Kl15, radio status, then a tick.
    fn run(&mut self, n: u32) {
        for _ in 0..n {
            self.now += ms(100);
            if self.ring {
                self.feed(0x42B, &[0x16, 0x01]);
                self.feed(0x436, &[0x0B, 0x01]);
                let k = if self.kl15 { 0x02 } else { 0x00 };
                self.feed(0x575, &[k, 0, 0, 0]);
                self.feed(0x2C3, &[k]);
                let tv = if self.on_tv { 0x37 } else { 0x00 };
                self.feed(0x661, &[0x83, 0x01, 0x12, tv]);
            }
            self.car.tick(self.now);
            for o in self.car.drain_outputs() {
                if let Output::Send { frame, .. } = o {
                    self.sent.push(frame);
                }
            }
        }
    }

    fn take(&mut self, ids: &[u32]) -> Vec<Frame> {
        let (hit, rest) = std::mem::take(&mut self.sent)
            .into_iter()
            .partition(|f| ids.contains(&f.id));
        self.sent = rest;
        hit
    }

    fn bridge(&mut self, update: BridgeUpdate) {
        self.car.handle(
            1,
            Request::Bridge {
                update: Box::new(update),
            },
            self.now,
        );
    }
}

#[test]
fn dis_off_by_default_sends_nothing() {
    let mut r = Rig::new(false);
    r.kl15 = true;
    r.on_tv = true;
    r.run(30);
    assert!(r.take(&[0x6C0, 0x363, 0x365]).is_empty());
    assert_eq!(text(&r, "dis.state").as_deref(), Some("off"));
}

#[test]
fn dis_frames_go_through_the_gate() {
    let mut r = Rig::new(true);
    // Awake ring, Kl15 off: the session doesn't even start.
    r.run(20);
    assert!(r.take(&[0x6C0]).is_empty());
    assert_eq!(text(&r, "dis.state").as_deref(), Some("waiting"));

    // Kl15 on: the link opens on 0x6C0.
    r.kl15 = true;
    r.run(5);
    assert!(!r.take(&[0x6C0]).is_empty(), "expected TP2 setup on 0x6C0");
    let passed = r.car.gate(0).stats(TxClass::DisplayDdp).passed;
    assert!(passed > 0);

    // Kl15 off: the session stops, and its close is refused by the gate.
    r.kl15 = false;
    r.run(5);
    assert!(r.take(&[0x6C0]).is_empty());
    let st = r.car.gate(0).stats(TxClass::DisplayDdp);
    assert!(st.refused(Refused::Kl15Off) > 0, "{st:?}");

    // Bus goes silent: still nothing.
    r.kl15 = true;
    r.ring = false;
    r.run(40);
    assert!(r.take(&[0x6C0]).is_empty());
}

#[test]
fn top_lines_only_on_tv() {
    let mut r = Rig::new(true);
    r.kl15 = true;
    r.bridge(BridgeUpdate::Media {
        media: Some(MediaState {
            title: Some("Song".into()),
            artist: Some("Band".into()),
            playing: true,
            ..Default::default()
        }),
    });
    r.run(10);
    assert!(r.take(&[0x363, 0x365]).is_empty(), "not on TV");

    r.on_tv = true;
    r.run(3);
    let top = r.take(&[0x363, 0x365]);
    assert_eq!(top.len(), 2, "{top:?}");
    assert_eq!(top[0].id, 0x363);
    assert_eq!(&top[0].data()[..4], &carpi_dis::top::encode("Song")[..4]);
    assert_eq!(&top[1].data()[..4], &carpi_dis::top::encode("Band")[..4]);
    // Change-driven: nothing more while it stays the same.
    r.run(10);
    assert!(r.take(&[0x363, 0x365]).is_empty());

    // Navigating: distance and road.
    r.bridge(BridgeUpdate::Nav {
        nav: Some(NavState {
            maneuver: Some("turn".into()),
            side: Some("left".into()),
            distance_m: Some(350.0),
            road: Some("Hauptstr".into()),
            ..Default::default()
        }),
    });
    r.run(2);
    let top = r.take(&[0x363, 0x365]);
    assert_eq!(top.len(), 2);
    assert_eq!(top[0].data(), &carpi_dis::top::encode("350 m")[..8]);
    assert_eq!(top[1].data(), &carpi_dis::top::encode("Hauptstr")[..8]);

    // Off TV: silent again.
    r.on_tv = false;
    r.bridge(BridgeUpdate::Nav { nav: None });
    r.run(10);
    assert!(r.take(&[0x363, 0x365]).is_empty());
}

#[test]
fn api_prefixes_map_to_diag_subscriptions() {
    let mut r = Rig::new(false);
    let period = ms(500);
    let t0 = r.now;
    // "" maps to nothing: a page asking for everything never starts polling.
    assert!(r.car.diag_subscribe(&["".into()], period, t0).is_empty());
    let ids = r.car.diag_subscribe(&["engine.boost.".into()], period, t0);
    assert_eq!(ids.len(), 2);
    let rpm = r.car.diag_subscribe(&["engine.rpm".into()], period, t0);
    assert_eq!(rpm.len(), 1);
    let st = r.car.diag_status(t0).unwrap();
    assert_eq!(st.subscriptions.len(), 3);
    assert!(
        st.subscriptions
            .iter()
            .any(|s| s.value == "engine.rpm" && s.source == "ican")
    );
    assert!(
        st.subscriptions
            .iter()
            .any(|s| s.value == "engine.boost.actual" && s.source.starts_with("diag:1:"))
    );

    // Kl15 off: the engine channel's setup on 0x200 is refused.
    r.run(5);
    assert!(r.take(&[0x200]).is_empty());
    assert!(r.car.gate(0).stats(TxClass::Diag).refused_total() > 0);

    // Leases are 3x the period: boost is renewed, rpm isn't.
    for _ in 0..4 {
        r.car.diag_renew(&ids, period, r.now);
        r.run(5);
    }
    let st = r.car.diag_status(r.now).unwrap();
    let names: Vec<&str> = st.subscriptions.iter().map(|s| s.value.as_str()).collect();
    assert_eq!(names, ["engine.boost.actual", "engine.boost.specified"]);

    // Kl15 on: the setup goes out once the channel's backoff from the
    // refused attempts is over (at most 30 s).
    r.kl15 = true;
    for _ in 0..80 {
        r.car.diag_renew(&ids, period, r.now);
        r.run(5);
    }
    let setup = r.take(&[0x200]);
    assert!(!setup.is_empty(), "expected TP2 setup on 0x200");
    assert_eq!(setup[0].data()[0], 0x01, "engine");

    // API subscription ends: diag drops it.
    r.car.diag_unsubscribe(&ids, r.now);
    assert!(r.car.diag_status(r.now).unwrap().subscriptions.is_empty());
}

#[test]
fn diag_jobs_and_clear_needs_confirm() {
    let mut r = Rig::new(false);
    r.run(1);
    let e = r
        .car
        .diag_start(
            DiagAction::ClearDtcs {
                module: 1,
                confirm: false,
            },
            r.now,
        )
        .unwrap_err();
    assert!(e.contains("confirm"), "{e}");
    assert_eq!(text(&r, "diag.mode").as_deref(), Some("normal"));

    let id = r.car.diag_start(DiagAction::InstallList, r.now).unwrap();
    let ev = r.car.drain_diag_jobs();
    assert!(ev.iter().any(|(j, e)| *j == id && !e.is_empty()));

    r.car
        .diag_start(
            DiagAction::ClearDtcs {
                module: 1,
                confirm: true,
            },
            r.now,
        )
        .unwrap();
    r.run(1);
    assert_eq!(text(&r, "diag.mode").as_deref(), Some("exclusive"));
    assert_eq!(
        r.car.store().get_bool("diag.vcds_auto_pause", r.now),
        Some(false)
    );

    // Another tester sets up a channel: auto pause, and the clear fails.
    r.feed(0x200, &[0x01, 0xC0, 0x00, 0x10, 0x00, 0x03, 0x01]);
    r.run(1);
    assert_eq!(
        r.car.store().get_bool("diag.vcds_auto_pause", r.now),
        Some(true)
    );
    let done = r.car.drain_diag_jobs();
    assert!(done.iter().any(|(_, evs)| {
        evs.iter()
            .any(|e| matches!(e, carpi_proto::JobEvent::Finished { ok: false, .. }))
    }));
}
