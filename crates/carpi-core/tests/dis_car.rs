//! The DIS end to end in the car model, against a simulated cluster: carpid's
//! own wiring (config, store, gate, bus view) must put something on the
//! cluster with the ignition on. Car test 2026-11-08 showed nothing at all.

use carpi_can::recorder::Recorder;
use carpi_can::{Frame, RxFrame};
use carpi_core::{Car, CoreConfig, DisSetup, IfaceSpec, Output};
use carpi_proto::{BridgeUpdate, MediaState, Request};
use carpi_sim::bus::Node;
use carpi_sim::ddp::{ClusterSpec, SimCluster};
use std::time::{Duration, Instant, SystemTime};

struct Rig {
    car: Car,
    cl: SimCluster,
    now: Instant,
    next_ring: Instant,
    on_tv: bool,
    /// Top-line frames (0x363 / 0x365) the car sent.
    top: Vec<Frame>,
}

impl Rig {
    fn new(dis: DisSetup) -> Self {
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
                dis,
                ..Default::default()
            },
            now,
        );
        Rig {
            car,
            cl: SimCluster::new(ClusterSpec::default()),
            now,
            next_ring: now,
            on_tv: false,
            top: Vec::new(),
        }
    }

    fn feed(&mut self, f: Frame) {
        let rx = RxFrame {
            frame: f,
            at: self.now,
            wall: SystemTime::now(),
        };
        self.car.ingest(0, &rx);
    }

    /// 5 ms steps: the awake ring with Kl15 on every 100 ms, the car, the
    /// cluster.
    fn run(&mut self, d: Duration) {
        let end = self.now + d;
        while self.now < end {
            self.now += Duration::from_millis(5);
            if self.now >= self.next_ring {
                self.next_ring = self.now + Duration::from_millis(100);
                for (id, data) in [
                    (0x42B, &[0x16, 0x01][..]),
                    (0x436, &[0x0B, 0x01]),
                    (0x575, &[0x02, 0, 0, 0]),
                    (0x2C3, &[0x02]),
                ] {
                    self.feed(Frame::new(id, data).unwrap());
                }
                let tv = if self.on_tv { 0x37 } else { 0x00 };
                self.feed(Frame::new(0x661, &[0x83, 0x01, 0x12, tv]).unwrap());
            }
            self.car.tick(self.now);
            for o in self.car.drain_outputs() {
                if let Output::Send { frame, .. } = o {
                    if frame.id == 0x363 || frame.id == 0x365 {
                        self.top.push(frame.clone());
                    }
                    self.cl.on_frame(self.now, &frame);
                }
            }
            self.cl.tick(self.now);
            while let Some(f) = self.cl.poll_transmit(self.now) {
                self.feed(f);
            }
        }
    }

    fn text(&self, name: &str) -> Option<String> {
        match self.car.store().get(name, self.now) {
            Some(carpi_state::Value::Text(s)) => Some(s.clone()),
            _ => None,
        }
    }
}

fn enabled() -> DisSetup {
    DisSetup {
        enabled: true,
        ..Default::default()
    }
}

#[test]
fn ignition_on_puts_readings_up_by_itself() {
    let mut r = Rig::new(enabled());
    r.run(Duration::from_secs(5));
    assert!(r.cl.owner, "cluster never granted the centre");
    assert_eq!(r.text("dis.shown").as_deref(), Some("readings"));
    assert!(!r.cl.drawn.is_empty());
}

#[test]
fn media_takes_over_and_gives_back() {
    let mut r = Rig::new(enabled());
    r.run(Duration::from_secs(4));
    r.car.handle(
        1,
        Request::Bridge {
            update: Box::new(BridgeUpdate::Media {
                media: Some(MediaState {
                    title: Some("Song".into()),
                    artist: Some("Band".into()),
                    playing: true,
                    position_ms: Some(10_000),
                    duration_ms: Some(200_000),
                    ..Default::default()
                }),
            }),
        },
        r.now,
    );
    r.run(Duration::from_secs(4));
    assert_eq!(r.text("dis.shown").as_deref(), Some("media"));
    r.car.handle(
        1,
        Request::Bridge {
            update: Box::new(BridgeUpdate::Media { media: None }),
        },
        r.now,
    );
    r.run(Duration::from_secs(4));
    assert_eq!(r.text("dis.shown").as_deref(), Some("readings"));
}

#[test]
fn tab_mode_leaves_the_centre_alone() {
    let mut r = Rig::new(DisSetup {
        claim_always: false,
        ..enabled()
    });
    r.run(Duration::from_secs(5));
    assert!(
        !r.cl.owner,
        "shown {:?} page {:?}",
        r.text("dis.shown"),
        r.text("dis.page")
    );
    // The link is up, ready for the cluster menu to pick us.
    assert_eq!(r.text("dis.state").as_deref(), Some("ready"));
    assert_eq!(r.text("dis.window").as_deref(), Some("idle"));
}

#[test]
fn media_as_the_default_page_like_the_old_client() {
    let mut r = Rig::new(DisSetup {
        default_page: carpi_core::DisScreen::Media,
        ..enabled()
    });
    r.run(Duration::from_secs(5));
    assert!(r.cl.owner);
    assert_eq!(r.text("dis.shown").as_deref(), Some("media"));
}

fn play(r: &mut Rig, title: &str) {
    r.car.handle(
        1,
        Request::Bridge {
            update: Box::new(BridgeUpdate::Media {
                media: Some(MediaState {
                    title: Some(title.into()),
                    artist: Some("Band".into()),
                    playing: true,
                    duration_ms: Some(200_000),
                    ..Default::default()
                }),
            }),
        },
        r.now,
    );
}

#[test]
fn top_lines_skip_the_centre_and_leave_line_2_to_the_cluster() {
    let mut r = Rig::new(DisSetup {
        auto_media: false,
        ..enabled()
    });
    r.on_tv = true;
    play(&mut r, "A Long Song Title");
    r.run(Duration::from_secs(3));
    // Before the grant line 2 was ours to use; from here the centre is ours.
    r.top.clear();
    r.run(Duration::from_secs(3));
    // Centre: readings (ours). Top: media title on line 1 only, marquee.
    assert_eq!(r.text("dis.shown").as_deref(), Some("readings"));
    assert_eq!(r.text("dis.top_text").as_deref(), Some("A Long Song Title"));
    assert!(r.top.iter().any(|f| f.id == 0x363));
    assert!(!r.top.iter().any(|f| f.id == 0x365), "line 2 is the cluster's");
    let l1: Vec<_> = r.top.iter().filter(|f| f.id == 0x363).map(|f| f.data().to_vec()).collect();
    assert!(l1.windows(2).any(|w| w[0] != w[1]), "line 1 marquees");
}

#[test]
fn top_lines_skip_media_when_the_centre_shows_it() {
    let mut r = Rig::new(enabled());
    r.on_tv = true;
    play(&mut r, "Song");
    r.run(Duration::from_secs(5));
    assert_eq!(r.text("dis.shown").as_deref(), Some("media"));
    assert_eq!(r.text("dis.top_text"), None, "media is in the centre already");
}
