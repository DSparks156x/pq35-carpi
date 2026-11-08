//! The driver against the simulated cluster on a 100 kbit/s virtual bus:
//! startup busy, `9n`, reinit, menu busy, a repeated mode request, and loss.

use carpi_can::Frame;
use carpi_dis::fb::Fb;
use carpi_dis::font::Metrics;
use carpi_dis::proto::Variant;
use carpi_dis::screens::nav::NavInput;
use carpi_dis::screens::readings::{ReadingSpec, ReadingsInput};
use carpi_dis::{Dis, DisConfig, ScreenId, SessionEvent};
use carpi_sim::bus::{Bus, Node};
use carpi_sim::ddp::{ClusterSpec, SimCluster};
use std::time::{Duration, Instant};

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

struct Hu {
    dis: Dis,
    now: Instant,
    ev: Vec<SessionEvent>,
}

impl Hu {
    fn drain(&mut self) {
        while let Some(e) = self.dis.poll_event() {
            self.ev.push(e);
        }
    }
}

impl Node for Hu {
    fn on_frame(&mut self, now: Instant, f: &Frame) {
        self.now = now;
        self.dis.on_frame(now, f);
        self.drain();
    }
    fn tick(&mut self, now: Instant) {
        self.now = now;
        self.dis.tick(now);
        self.drain();
    }
    fn poll_transmit(&mut self, _now: Instant) -> Option<Frame> {
        self.dis.poll_transmit()
    }
    fn next_deadline(&self) -> Option<Instant> {
        self.dis.next_deadline(self.now)
    }
}

struct World {
    bus: Bus,
    hu: Hu,
    cl: SimCluster,
}

fn specs() -> Vec<ReadingSpec> {
    ["Oil", "H2O", "Bst", "IAT", "Bat", "RPM", "Hdx", "AFR"]
        .iter()
        .map(|l| ReadingSpec {
            key: format!("x.{l}"),
            label: (*l).into(),
            decimals: 0,
        })
        .collect()
}

fn values(v: f64) -> ReadingsInput {
    ReadingsInput {
        title: "Engine".into(),
        values: [Some(v); 8],
    }
}

impl World {
    fn new(spec: ClusterSpec) -> Self {
        let t0 = Instant::now();
        let mut dis = Dis::new(DisConfig {
            readings: specs(),
            ..DisConfig::default()
        });
        dis.set_user_page(t0, Some(ScreenId::Readings));
        dis.set_readings(t0, values(90.0));
        dis.start(t0);
        World {
            bus: Bus::new(t0),
            hu: Hu {
                dis,
                now: t0,
                ev: Vec::new(),
            },
            cl: SimCluster::new(spec),
        }
    }

    fn now(&self) -> Instant {
        self.bus.now
    }

    fn run(&mut self, d: Duration) {
        let mut nodes: [&mut dyn Node; 2] = [&mut self.hu, &mut self.cl];
        self.bus.run_for(&mut nodes, d);
    }

    fn owned(&self) -> Vec<u32> {
        self.hu
            .ev
            .iter()
            .filter_map(|e| match e {
                SessionEvent::Owned { generation } => Some(*generation),
                _ => None,
            })
            .collect()
    }

    fn count(&self, f: impl Fn(&SessionEvent) -> bool) -> usize {
        self.hu.ev.iter().filter(|e| f(e)).count()
    }

    /// Run until no frame is in flight (refine slices keep coming for a while).
    fn settle(&mut self) {
        for _ in 0..1000 {
            if self.hu.dis.session().frame_idle() {
                return;
            }
            self.run(Duration::from_millis(10));
        }
        panic!("frames never went idle");
    }

    /// What the cluster got, replayed into a model, must match the driver's belief.
    fn check_screen(&self) {
        let mut fb = Fb::new(Metrics::for_variant(Variant::White));
        for m in &self.cl.drawn {
            fb.apply(m);
        }
        let belief = self.hu.dis.belief().expect("drew something");
        assert_eq!(fb.ascii(), belief.ascii(), "cluster and belief disagree");
        assert!(self.cl.violations.is_empty(), "{:?}", self.cl.violations);
        assert!(!self.cl.frozen);
    }

    fn sent_app(&self, msg: &[u8]) -> bool {
        self.cl.received.iter().any(|(_, m)| m == msg)
    }
}

#[test]
fn startup_busy_then_grant_and_draw() {
    let mut w = World::new(ClusterSpec {
        startup_busy: Some(Duration::from_secs(3)),
        ..ClusterSpec::default()
    });
    w.run(Duration::from_secs(2));
    assert!(w.sent_app(&[0x15, 0x01, 0x01, 0x02, 0x00, 0x00]));
    assert!(w.sent_app(&[0x01, 0x01, 0x00]));
    assert!(w.sent_app(&[0x20, 0x3B, 0xA0, 0x00]));
    assert!(w.sent_app(&[0x33]));
    assert!(matches!(w.hu.ev.first(), Some(SessionEvent::Ready(_))));
    assert_eq!(w.count(|e| *e == SessionEvent::Busy), 1);
    assert!(w.owned().is_empty() && w.cl.drawn.is_empty());

    w.run(Duration::from_secs(3));
    assert_eq!(w.count(|e| *e == SessionEvent::Free), 1);
    assert!(w.sent_app(&[0x2F]), "2E answered");
    assert_eq!(w.owned(), vec![1]);
    assert!(w.cl.owner);
    assert_eq!(w.hu.dis.shown(), Some(ScreenId::Readings));
    assert_eq!(w.cl.drawn_unowned, 0);
    w.settle();
    w.check_screen();
    assert_eq!(w.count(|e| *e == SessionEvent::ClusterSelected), 0);
}

#[test]
fn stuck_busy_reclaims_then_relinks() {
    let mut w = World::new(ClusterSpec::default());
    w.cl.faults.stuck_busy = true;
    w.run(Duration::from_secs(20));
    // Re-claimed every 5 s, still nothing.
    assert!(w.count(|e| *e == SessionEvent::Busy) >= 4, "{:?}", w.hu.ev);
    assert!(w.owned().is_empty());
    // After 30 s of busy the link drops, the handshake reruns and we get it.
    w.run(Duration::from_secs(20));
    assert!(w.count(|e| matches!(e, SessionEvent::LinkClosed(_))) >= 1);
    assert_eq!(w.owned().len(), 1);
    w.settle();
    w.check_screen();
}

#[test]
fn not_ready_mid_frame_recovers() {
    let mut w = World::new(ClusterSpec::default());
    w.run(Duration::from_secs(3));
    assert_eq!(w.owned(), vec![1]);
    let done = w.count(|e| *e == SessionEvent::FrameDone);
    w.cl.faults.not_ready = 2;
    let now = w.now();
    w.hu.dis.set_readings(now, values(1234.0));
    w.run(Duration::from_secs(2));
    assert_eq!(w.cl.not_ready_sent, 2);
    assert!(w.count(|e| *e == SessionEvent::FrameDone) > done);
    assert_eq!(w.count(|e| matches!(e, SessionEvent::LinkClosed(_))), 0);
    w.settle();
    w.check_screen();
}

#[test]
fn reinit_menu_busy_and_mode_repeat_repaint() {
    let mut w = World::new(ClusterSpec::default());
    w.run(Duration::from_secs(3));
    assert_eq!(w.owned(), vec![1]);

    // Cluster asks for a reinit: 2F, re-claim, full repaint.
    let n = w.cl.drawn.len();
    let now = w.now();
    w.cl.reinit(now);
    w.run(Duration::from_secs(1));
    assert_eq!(w.owned(), vec![1, 3]);
    assert!(w.cl.received.iter().filter(|(_, m)| m == &[0x2F]).count() >= 1);
    assert!(w.cl.drawn[n].starts_with(&[0x52, 0x05, 0x02, 0x00, 0x1B, 0x40, 0x30]));
    w.settle();
    w.check_screen();

    // Menu takes the centre, then gives it back.
    let now = w.now();
    w.cl.busy(now);
    w.run(ms(500));
    assert!(!w.hu.dis.session().owned());
    let now = w.now();
    w.hu.dis.set_readings(now, values(55.0));
    w.run(ms(500));
    let now = w.now();
    w.cl.free(now);
    w.run(Duration::from_secs(1));
    assert_eq!(w.owned(), vec![1, 3, 5]);
    w.settle();
    w.check_screen();

    // A repeated mode request after setup: caps and setup again, then re-claim.
    let now = w.now();
    w.cl.mode_request(now);
    w.run(Duration::from_secs(1));
    assert_eq!(w.owned(), vec![1, 3, 5, 7]);
    w.settle();
    w.check_screen();
}

#[test]
fn loss_closes_link_and_recovers() {
    let mut w = World::new(ClusterSpec::default());
    w.run(Duration::from_secs(3));
    assert_eq!(w.owned(), vec![1]);
    w.cl.faults.silent = true;
    let now = w.now();
    w.hu.dis.set_readings(now, values(77.0));
    w.run(Duration::from_secs(6));
    assert!(w.count(|e| matches!(e, SessionEvent::LinkClosed(_))) >= 1);
    assert!(w.count(|e| matches!(e, SessionEvent::Lost { .. })) == 1);
    assert!(!w.hu.dis.session().owned());

    w.cl.faults.silent = false;
    w.run(Duration::from_secs(8));
    assert_eq!(w.owned().len(), 2, "{:?}", w.hu.ev);
    assert!(w.cl.owner);
    w.settle();
    w.check_screen();
}

#[test]
fn nav_with_refine_paces_strokes() {
    let mut w = World::new(ClusterSpec::default());
    w.run(Duration::from_secs(3));
    let now = w.now();
    w.hu.dis.set_nav(
        now,
        NavInput {
            maneuver: Some("roundabout_enter_and_exit".into()),
            side: Some("right".into()),
            roundabout_angle: Some(45),
            distance_m: Some(420.0),
            road: Some("Very long road name for the marquee".into()),
        },
    );
    w.run(ms(100));
    assert_eq!(w.hu.dis.shown(), Some(ScreenId::Nav));
    w.run(Duration::from_secs(4));
    // Refine strokes went out, and the sim saw no stroke spacing under 10 ms.
    assert!(w.cl.drawn.iter().any(|m| m.contains(&0x63)));
    assert!(!w.hu.dis.nav().refining(), "refine should finish in 4 s");
    w.settle();
    w.check_screen();
}

#[test]
fn media_takes_the_centre_without_a_user_page() {
    use carpi_dis::screens::media::MediaInput;
    let mut w = World::new(ClusterSpec::default());
    w.hu.dis.set_user_page(w.now(), None);
    w.run(Duration::from_secs(2));
    assert_eq!(w.hu.dis.shown(), None);
    let now = w.now();
    w.hu.dis.set_media(
        now,
        MediaInput {
            title: Some("Song".into()),
            duration_ms: Some(180_000),
            position_ms: Some(10_000),
            playing: true,
            ..Default::default()
        },
    );
    w.run(Duration::from_secs(2));
    assert_eq!(w.hu.dis.shown(), Some(ScreenId::Media));
    w.settle();
    w.check_screen();
}

#[test]
fn new_cover_flashes_full_centre_then_back_to_media() {
    use carpi_dis::icons::Art;
    use carpi_dis::screens::media::MediaInput;
    let cover = |id: u64| {
        let a = Art {
            w: 128,
            h: 96,
            px: (0..128 * 96).map(|i| (i as u64 + id) % 5 == 0).collect(),
        };
        Some((id, a))
    };
    let media = |id: u64| MediaInput {
        title: Some(format!("Song {id}")),
        duration_ms: Some(180_000),
        position_ms: Some(1_000),
        playing: true,
        cover: cover(id),
        ..Default::default()
    };
    let mut w = World::new(ClusterSpec::default());
    w.hu.dis.set_user_page(w.now(), None);
    let now = w.now();
    w.hu.dis.set_media(now, media(1));
    w.run(Duration::from_secs(3));
    // First cover arrived before media had the centre: no flash.
    assert_eq!(w.hu.dis.shown(), Some(ScreenId::Media));
    let now = w.now();
    w.hu.dis.set_media(now, media(2));
    w.run(Duration::from_secs(1));
    assert_eq!(w.hu.dis.shown(), Some(ScreenId::Cover));
    // Native refine runs, then the 4 s hold starts; back to media before the
    // 20 s cap.
    let start = w.now();
    let mut back = None;
    for _ in 0..250 {
        w.run(Duration::from_millis(100));
        if w.hu.dis.shown() == Some(ScreenId::Media) {
            back = Some(w.now() - start);
            break;
        }
    }
    let back = back.expect("flash ended");
    assert!(back >= Duration::from_secs(4), "{back:?}");
    assert!(back < Duration::from_secs(20), "{back:?}");
    w.settle();
    w.check_screen();
}
