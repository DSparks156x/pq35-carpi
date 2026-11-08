//! `carpi dis-bench`: the DIS driver straight against the bench cluster. No
//! daemon, gate or NM. Sans-IO here; `main.rs` wires it to a port.
//!
//! The bench cluster (A3 8P0920982G, own 500 kbit/s CAN) only shows the DIS
//! while it sees wake and ignition traffic. From the old bench owner
//! (`dis_emulator/bench_cluster/j2534_service.py`, `BENCH_FINDINGS_20261007`):
//! 0x575 `FF x8` every 500 ms and 0x2C3 `07` (Kl15 on) every 100 ms, sent
//! from the start and for the whole run.

use carpi_can::Frame;
use carpi_dis::screens::nav::NavInput;
use carpi_dis::screens::readings::{ReadingCell, ReadingsInput, ReadingsPage};
use carpi_dis::session::{Phase, Window};
use carpi_dis::{Dis, DisConfig, ScreenId, SessionEvent};
use carpi_tp2::wire::{Pdu, parse};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub const WAKE_ID: u32 = 0x575;
pub const KL15_ID: u32 = 0x2C3;
const WAKE_EVERY: Duration = Duration::from_millis(500);
const KL15_EVERY: Duration = Duration::from_millis(100);
const INPUT_EVERY: Duration = Duration::from_millis(250);
const STATS_EVERY: Duration = Duration::from_secs(5);
/// `both`: switch the user page this often.
const FLIP_EVERY: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Screens {
    Nav,
    Readings,
    Both,
}

/// Scripted legs: maneuver, side, roundabout angle, road.
const LEGS: [(&str, Option<&str>, Option<i32>, &str); 6] = [
    ("turn", Some("right"), None, "Main Street"),
    ("slight_turn", Some("left"), None, "A38 Bristol Road"),
    (
        "roundabout_enter_and_exit",
        Some("right"),
        Some(90),
        "Ring Road",
    ),
    ("turn", Some("left"), None, "High Street"),
    ("u_turn", Some("left"), None, "Back Lane"),
    ("destination", None, None, "Home"),
];
const LEG_M: f64 = 600.0;
const SPEED_MS: f64 = 30.0;

/// Nav input `t` into the script: each leg counts down from 600 m at 30 m/s.
pub fn nav_at(t: Duration) -> NavInput {
    let leg_secs = LEG_M / SPEED_MS;
    let s = t.as_secs_f64();
    let leg = (s / leg_secs) as usize % LEGS.len();
    let d = LEG_M - (s % leg_secs) * SPEED_MS;
    let (m, side, angle, road) = LEGS[leg];
    NavInput {
        maneuver: Some(m.into()),
        side: side.map(Into::into),
        roundabout_angle: angle,
        distance_m: Some(d.max(0.0)),
        road: Some(road.into()),
    }
}

/// Readings subpages: (name, [(key, decimals)]). Keys carry unit suffixes so
/// the screen picks icons and units on its own.
const PAGES: [(&str, [(&str, u8); 8]); 3] = [
    (
        "Engine",
        [
            ("bench.speed_kmh", 0),
            ("bench.rpm", 0),
            ("bench.coolant_c", 0),
            ("bench.oil_temp_c", 0),
            ("bench.battery_v", 1),
            ("bench.boost_bar", 2),
            ("bench.iat_c", 0),
            ("bench.afr", 1),
        ],
    ),
    (
        "Temps",
        [
            ("bench.coolant_c", 0),
            ("bench.oil_temp_c", 0),
            ("bench.iat_c", 0),
            ("bench.outside_temp_c", 1),
            ("bench.haldex_oil_temperature_c", 0),
            ("bench.transmission.oil_temperature_c", 0),
            ("bench.exhaust_c", 0),
            ("bench.ambient.cabin_c", 1),
        ],
    ),
    (
        "Driver",
        [
            ("bench.throttle_pct", 0),
            ("bench.pedal_pct", 0),
            ("bench.torque_nm", 0),
            ("bench.ignition_timing_deg", 1),
            ("bench.fuel_level_l", 0),
            ("bench.odometer_km", 0),
            ("bench.steering_deg", 0),
            ("bench.dim_display_pct", 0),
        ],
    ),
];

pub fn reading_pages() -> Vec<ReadingsPage> {
    PAGES
        .iter()
        .map(|(name, cells)| ReadingsPage {
            name: (*name).into(),
            cells: cells
                .iter()
                .map(|(k, d)| ReadingCell {
                    key: (*k).into(),
                    decimals: *d,
                    icon: None,
                    unit: None,
                })
                .collect(),
        })
        .collect()
}

/// Values for `page` at `t` into the script: slow ramps and waves, all different.
pub fn readings_at(t: Duration, page: usize) -> ReadingsInput {
    let s = t.as_secs_f64();
    let w = |p: f64| (s * std::f64::consts::TAU / p).sin();
    let all: [[f64; 8]; 3] = [
        [
            (60.0 + 60.0 * w(20.0)).round(),
            (2500.0 + 2000.0 * w(7.0)).round(),
            (40.0 + s).min(95.0),
            (30.0 + s * 0.8).min(110.0),
            13.8 + 0.4 * w(3.0),
            0.8 * w(5.0),
            25.0 + 5.0 * w(30.0),
            14.7 + 2.0 * w(4.0),
        ],
        [
            (40.0 + s).min(95.0),
            (30.0 + s * 0.8).min(110.0),
            25.0 + 5.0 * w(30.0),
            8.0 + 2.0 * w(60.0),
            (20.0 + s * 0.3).min(70.0),
            (25.0 + s * 0.5).min(90.0),
            450.0 + 300.0 * w(9.0),
            21.0 + w(40.0),
        ],
        [
            (50.0 + 50.0 * w(6.0)).round(),
            (50.0 + 50.0 * w(6.5)).round(),
            (150.0 + 150.0 * w(8.0)).round(),
            10.0 + 20.0 * w(5.0),
            (40.0 - s * 0.01).max(0.0),
            123_456.0 / 1000.0,
            (90.0 * w(4.0)).round(),
            (50.0 + 50.0 * w(30.0)).round(),
        ],
    ];
    ReadingsInput {
        title: String::new(),
        values: all[page % 3].map(Some),
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    /// DDP payload bytes we sent in data frames (TP2 header excluded).
    pub tx_bytes: u64,
    pub not_ready: u32,
    pub reinits: u32,
    pub frames_done: u32,
}

pub struct Bench {
    pub dis: Dis,
    screens: Screens,
    t0: Instant,
    end: Instant,
    next_wake: Instant,
    next_kl15: Instant,
    next_input: Instant,
    next_stats: Instant,
    stats_mark: (Instant, u64),
    page: Option<ScreenId>,
    phase: Phase,
    window: Window,
    out: VecDeque<Frame>,
    log: VecDeque<String>,
    pub stats: Stats,
    stopped: bool,
    tx_mid: bool,
    now: Instant,
    readings_cycle: u64,
    focus: bool,
}

impl Bench {
    pub fn new(now: Instant, screens: Screens, secs: f64) -> Self {
        let dis = Dis::new(DisConfig {
            readings_pages: reading_pages(),
            ..DisConfig::default()
        });
        let mut b = Bench {
            phase: dis.session().phase(),
            window: dis.session().window(),
            dis,
            screens,
            t0: now,
            end: now + Duration::from_secs_f64(secs),
            next_wake: now,
            next_kl15: now,
            next_input: now,
            next_stats: now + STATS_EVERY,
            stats_mark: (now, 0),
            page: None,
            out: VecDeque::new(),
            log: VecDeque::new(),
            stats: Stats::default(),
            stopped: false,
            tx_mid: false,
            now,
            readings_cycle: 0,
            focus: false,
        };
        b.dis.start(now);
        b.tick(now);
        b
    }

    pub fn done(&self, now: Instant) -> bool {
        now >= self.end
    }

    fn say(&mut self, now: Instant, s: String) {
        let t = now.saturating_duration_since(self.t0).as_secs_f64();
        self.log.push_back(format!("{t:8.3}s  {s}"));
    }

    pub fn poll_log(&mut self) -> Option<String> {
        self.log.pop_front()
    }

    pub fn on_frame(&mut self, now: Instant, f: &Frame) {
        self.now = now;
        if !f.extended && f.id == u32::from(carpi_dis::proto::RX_ID) {
            if let Some(Pdu::Ack { ready: false, .. }) = parse(f.data()) {
                self.stats.not_ready += 1;
                self.say(now, "9n (cluster not ready)".into());
            }
            self.dis.on_frame(now, f);
        }
        self.after(now);
    }

    pub fn tick(&mut self, now: Instant) {
        self.now = now;
        if !self.stopped {
            if now >= self.next_wake {
                self.out.push_back(Frame::new(WAKE_ID, &[0xFF; 8]).unwrap());
                self.next_wake = now + WAKE_EVERY;
            }
            if now >= self.next_kl15 {
                self.out.push_back(Frame::new(KL15_ID, &[0x07]).unwrap());
                self.next_kl15 = now + KL15_EVERY;
            }
            if now >= self.next_input {
                self.inputs(now);
                self.next_input = now + INPUT_EVERY;
            }
        }
        if now >= self.next_stats {
            self.report(now);
            self.next_stats = now + STATS_EVERY;
        }
        self.dis.tick(now);
        self.after(now);
    }

    fn inputs(&mut self, now: Instant) {
        let t = now - self.t0;
        let page = match self.screens {
            Screens::Nav => ScreenId::Nav,
            Screens::Readings => ScreenId::Readings,
            Screens::Both if (t.as_secs() / FLIP_EVERY.as_secs()).is_multiple_of(2) => {
                ScreenId::Nav
            }
            Screens::Both => ScreenId::Readings,
        };
        if self.screens != Screens::Readings {
            self.dis.set_nav(now, nav_at(t));
        }
        if self.screens != Screens::Nav {
            // 10 s cycle: focus off, on at 4 s, step the subpage at 10 s.
            let cycle = t.as_secs() / 10;
            let focus = t.as_secs() % 10 >= 4;
            if cycle != self.readings_cycle {
                self.readings_cycle = cycle;
                self.dis.step_readings(now, 1);
                let r = self.dis.readings();
                let msg = format!("readings page {}/{}", r.page_index() + 1, r.page_count());
                self.say(now, msg);
            }
            if focus != self.focus {
                self.focus = focus;
                self.say(now, format!("focus {}", if focus { "on" } else { "off" }));
                self.dis.set_focus(now, focus);
            }
            let page = self.dis.readings().page_index();
            self.dis.set_readings(now, readings_at(t, page));
        }
        if self.page != Some(page) {
            self.page = Some(page);
            self.say(now, format!("page {page:?}"));
            self.dis.set_user_page(now, Some(page));
        }
    }

    /// Log state changes and session events.
    fn after(&mut self, now: Instant) {
        let s = self.dis.session();
        let (phase, window, generation) = (s.phase(), s.window(), s.generation());
        if phase != self.phase {
            self.phase = phase;
            self.say(now, format!("phase {phase:?}"));
        }
        if std::mem::discriminant(&window) != std::mem::discriminant(&self.window) {
            self.window = window;
            let w = match window {
                Window::Idle => "idle",
                Window::Claiming { .. } => "claim sent",
                Window::Owned => "owned",
                Window::Busy => "busy",
                Window::Free { .. } => "free",
                Window::Retry { .. } => "retry",
            };
            self.say(now, format!("window {w} (generation {generation})"));
        }
        while let Some(e) = self.dis.poll_event() {
            match e {
                SessionEvent::FrameDone => self.stats.frames_done += 1,
                SessionEvent::Owned { generation } => {
                    self.say(now, format!("granted (53 85), generation {generation}"))
                }
                SessionEvent::Ready(p) => self.say(now, format!("handshake done: {p:?}")),
                e => self.say(now, format!("{e:?}")),
            }
        }
        if let Some(e) = self.dis.last_error.take() {
            self.say(now, format!("pack error: {e:?}"));
        }
    }

    pub fn poll_transmit(&mut self) -> Option<Frame> {
        if let Some(f) = self.out.pop_front() {
            return Some(f);
        }
        let f = self.dis.poll_transmit()?;
        if let Some(Pdu::Data { payload, last, .. }) = parse(f.data()) {
            self.stats.tx_bytes += payload.len() as u64;
            // A one-frame `2F` is our answer to the cluster's `2E` reinit.
            if last && !self.tx_mid && payload == [0x2F] {
                self.stats.reinits += 1;
                self.say(self.now, "reinit (2E, answered 2F)".into());
            }
            self.tx_mid = !last;
        }
        Some(f)
    }

    pub fn next_deadline(&self, now: Instant) -> Instant {
        let mut d = [self.next_stats, self.end]
            .into_iter()
            .chain(self.dis.next_deadline(now))
            .min()
            .unwrap();
        if !self.stopped {
            d = d
                .min(self.next_wake)
                .min(self.next_kl15)
                .min(self.next_input);
        }
        d
    }

    fn report(&mut self, now: Instant) {
        let (at, bytes) = self.stats_mark;
        let secs = (now - at).as_secs_f64().max(1e-3);
        let rate = (self.stats.tx_bytes - bytes) as f64 / secs;
        self.stats_mark = (now, self.stats.tx_bytes);
        let st = self.stats;
        self.say(
            now,
            format!(
                "{rate:.0} B/s DDP out; total {} B, {} frames done, {} 9n, {} reinit",
                st.tx_bytes, st.frames_done, st.not_ready, st.reinits
            ),
        );
    }

    /// Release and close the link (A8). Drain `poll_transmit` after this.
    pub fn stop(&mut self, now: Instant) {
        self.now = now;
        self.stopped = true;
        self.dis.stop(now);
        self.after(now);
        let secs = (now - self.t0).as_secs_f64().max(1e-3);
        let st = self.stats;
        self.say(
            now,
            format!(
                "end: {:.0} B/s average, {} B, {} frames done, {} 9n, {} reinit",
                st.tx_bytes as f64 / secs,
                st.tx_bytes,
                st.frames_done,
                st.not_ready,
                st.reinits
            ),
        );
    }

    /// The driver's belief of the centre, one char per logical cell.
    pub fn belief_ascii(&self) -> Option<String> {
        self.dis.belief().map(|f| f.ascii())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use carpi_sim::bus::{Bus, Node};
    use carpi_sim::ddp::{ClusterSpec, SimCluster};

    #[test]
    fn nav_script_counts_down_and_moves_on() {
        let a = nav_at(Duration::ZERO);
        assert_eq!(a.distance_m, Some(600.0));
        assert_eq!(a.road.as_deref(), Some("Main Street"));
        let b = nav_at(Duration::from_secs(10));
        assert_eq!(b.distance_m, Some(300.0));
        assert_eq!(b.maneuver, a.maneuver);
        let c = nav_at(Duration::from_secs(20));
        assert_eq!(c.distance_m, Some(600.0));
        assert_eq!(c.road.as_deref(), Some("A38 Bristol Road"));
        // Wraps after six legs.
        assert_eq!(nav_at(Duration::from_secs(120)), a);
    }

    #[test]
    fn readings_change() {
        let a = readings_at(Duration::ZERO, 0);
        let b = readings_at(Duration::from_millis(1500), 0);
        assert!(a.values.iter().all(Option::is_some));
        assert!(
            a.values
                .iter()
                .zip(&b.values)
                .filter(|(x, y)| x != y)
                .count()
                >= 6
        );
    }

    struct Hu {
        b: Bench,
        now: Instant,
        sent: Vec<Frame>,
    }

    impl Node for Hu {
        fn on_frame(&mut self, now: Instant, f: &Frame) {
            self.now = now;
            self.b.on_frame(now, f);
        }
        fn tick(&mut self, now: Instant) {
            self.now = now;
            self.b.tick(now);
        }
        fn poll_transmit(&mut self, _now: Instant) -> Option<Frame> {
            let f = self.b.poll_transmit()?;
            self.sent.push(f);
            Some(f)
        }
        fn next_deadline(&self) -> Option<Instant> {
            Some(self.b.next_deadline(self.now))
        }
    }

    #[test]
    fn against_sim_cluster_at_500k() {
        let t0 = Instant::now();
        let mut hu = Hu {
            b: Bench::new(t0, Screens::Both, 30.0),
            now: t0,
            sent: Vec::new(),
        };
        let mut cl = SimCluster::new(ClusterSpec {
            startup_busy: Some(Duration::from_secs(3)),
            ..ClusterSpec::default()
        });
        let mut bus = Bus::new(t0);
        bus.bit_time = Duration::from_nanos(2200);
        {
            let mut nodes: [&mut dyn Node; 2] = [&mut hu, &mut cl];
            bus.run_for(&mut nodes, Duration::from_secs(25));
        }
        let now = bus.now;
        hu.b.stop(now);
        let log: Vec<String> = std::iter::from_fn(|| hu.b.poll_log()).collect();
        let text = log.join("\n");
        for want in [
            "handshake done",
            "Busy",
            "Free",
            "granted",
            "page Nav",
            "page Readings",
            "B/s",
        ] {
            assert!(text.contains(want), "no {want:?} in\n{text}");
        }
        assert!(hu.b.stats.reinits >= 1, "startup 2E answered");
        assert!(hu.b.stats.frames_done > 10);
        assert!(cl.violations.is_empty(), "{:?}", cl.violations);
        assert!(!cl.frozen);
        // Wake and Kl15 kept going the whole run.
        let wake = hu.sent.iter().filter(|f| f.id == WAKE_ID).count();
        let kl15 = hu.sent.iter().filter(|f| f.id == KL15_ID).count();
        assert!((49..=51).contains(&wake), "{wake}");
        assert!((245..=251).contains(&kl15), "{kl15}");
        assert!(hu.b.belief_ascii().is_some());
    }
}
