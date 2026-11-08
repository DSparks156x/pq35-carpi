//! `carpi dis-bench`: the DIS driver straight against the bench cluster. No
//! daemon, gate or NM. Sans-IO here; `main.rs` wires it to a port.
//!
//! The bench cluster (A3 8P0920982G, own 500 kbit/s CAN) only shows the DIS
//! while it sees wake and ignition traffic. From the old bench owner
//! (`dis_emulator/bench_cluster/j2534_service.py`, `BENCH_FINDINGS_20261007`):
//! 0x575 `FF x8` every 500 ms and 0x2C3 `07` (Kl15 on) every 100 ms, sent
//! from the start and for the whole run.

use carpi_can::Frame;
use carpi_dis::icons::Art;
use carpi_dis::image::dither;
use carpi_dis::screens::call::CallInput;
use carpi_dis::screens::egg::Egg;
use carpi_dis::screens::media::{ART, MediaInput, MediaLayout};
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
    /// Command-buffer probe: no screens, raw frames, Drew watches.
    Probe,
    /// Media, text stack layout (bench-driven; not in the arbiter yet).
    MediaStack,
    /// Media, cover thumbnail + text.
    MediaArt,
    /// A scripted call cycle.
    Call,
    /// Acceleration test against a scripted speed trace.
    Accel,
    /// Media with an easter egg per track, from `$CARPI_BENCH_EGGS` (`.egg`
    /// files from `cargo run -p carpi-dis --example eggs`).
    Eggs,
}

fn bench_eggs() -> &'static [Egg] {
    static C: std::sync::OnceLock<Vec<Egg>> = std::sync::OnceLock::new();
    C.get_or_init(|| {
        let Some(dir) = std::env::var_os("CARPI_BENCH_EGGS") else {
            return Vec::new();
        };
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "egg"))
            .collect();
        files.sort();
        files
            .iter()
            .filter_map(|f| Egg::parse(&std::fs::read(f).ok()?))
            .collect()
    })
}

/// Speed trace, 40 s cycle, km/h: stopped 0-3 s; a pull to ~170 easing off
/// (0-100 in ~6.5 s); lift at 21 s (aborts the quarter mile if still
/// running); brake to 0 by 29 s; stopped until 40 s. 20 Hz with jitter.
pub fn speed_at(t: Duration) -> f64 {
    let s = t.as_secs_f64() % 40.0;
    match s {
        s if s < 3.0 => 0.0,
        s if s < 21.0 => 210.0 * (1.0 - (-(s - 3.0) / 11.0).exp()),
        s if s < 29.0 => {
            let top = 210.0 * (1.0 - (-18.0f64 / 11.0).exp());
            (top * (1.0 - (s - 21.0) / 8.0)).max(0.0)
        }
        _ => 0.0,
    }
}

/// Call script, 45 s cycle: ringing 0-6 s, active 6-30, held 30-34, dialing
/// an unnamed long number 34-40, no call 40-45. Signal and battery wander.
pub fn call_at(t: Duration) -> CallInput {
    let s = t.as_secs() % 45;
    let named = |st: &str| CallInput {
        state: Some(st.into()),
        name: Some("Mum".into()),
        number: Some("+44 7700 900123".into()),
        // Signal wanders through all four bars; battery drains.
        signal_pct: Some(((t.as_secs() * 7) % 101) as u8),
        battery_pct: Some(100 - ((t.as_secs() / 3) % 101) as u8),
    };
    match s {
        0..6 => named("ringing"),
        6..30 => named("active"),
        30..34 => named("held"),
        34..40 => CallInput {
            state: Some("dialing".into()),
            name: None,
            number: Some("+44 20 7946 0958 ext 4471 9902".into()),
            signal_pct: Some(20),
            battery_pct: Some(10),
        },
        _ => CallInput::default(),
    }
}

/// Scripted tracks: title, artist, album, length in s.
const TRACKS: [(&str, &str, &str, u64); 3] = [
    ("Short One", "Some Band", "First Album", 95),
    (
        "A Much Longer Track Title That Has To Scroll Along",
        "An Artist With A Long Name",
        "Greatest Hits Volume Two",
        245,
    ),
    ("Interlude", "Ensemble", "Live", 3725),
];
const TRACK_SECS: u64 = 25;

/// Real covers for the bench: `<name>.<kind>.bin` files (1-bit MSB rows) in
/// `$CARPI_BENCH_COVERS`, from `cargo run -p carpi-dis --example covers`:
/// `thumb` 48x48 for the media art layout, `cover` 128x96 native for the
/// full-centre cover and its flash.
fn bench_covers(kind: &str, w: usize, h: usize) -> Vec<Art> {
    let Some(dir) = std::env::var_os("CARPI_BENCH_COVERS") else {
        return Vec::new();
    };
    let suffix = format!(".{kind}.bin");
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().ends_with(&suffix))
        .collect();
    files.sort();
    files
        .iter()
        .filter_map(|f| std::fs::read(f).ok())
        .filter(|b| b.len() == w.div_ceil(8) * h)
        .map(|b| Art {
            w,
            h,
            px: (0..w * h)
                .map(|i| b[(i / w) * w.div_ceil(8) + (i % w) / 8] & (0x80 >> (i % 8)) != 0)
                .collect(),
        })
        .collect()
}

fn thumbs() -> &'static [Art] {
    static C: std::sync::OnceLock<Vec<Art>> = std::sync::OnceLock::new();
    C.get_or_init(|| bench_covers("thumb", usize::from(ART.w) * 2, usize::from(ART.h) * 2))
}

fn full_covers() -> &'static [Art] {
    static C: std::sync::OnceLock<Vec<Art>> = std::sync::OnceLock::new();
    C.get_or_init(|| bench_covers("cover", 128, 96))
}

/// The full-centre cover for a track: real if there are any, else the
/// synthetic pattern at 128x96.
pub fn full_cover(track: usize) -> Art {
    let real = full_covers();
    if !real.is_empty() {
        return real[track % real.len()].clone();
    }
    pattern(track, 128, 96)
}

/// A cover per track: the next real one if `CARPI_BENCH_COVERS` has any, else
/// a synthetic grey pattern dithered to the thumbnail.
pub fn cover(track: usize) -> Art {
    let real = thumbs();
    if !real.is_empty() {
        return real[track % real.len()].clone();
    }
    pattern(track, usize::from(ART.w) * 2, usize::from(ART.h) * 2)
}

/// Synthetic grey test pattern per track, dithered to `w` x `h`.
fn pattern(track: usize, ow: usize, oh: usize) -> Art {
    let n = 96usize;
    let mut g = vec![0u8; n * n];
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = (x as f64 / n as f64 - 0.5, y as f64 / n as f64 - 0.5);
            let r = (fx * fx + fy * fy).sqrt();
            let v = match track % 3 {
                0 => (1.0 - r * 2.0).clamp(0.0, 1.0),
                1 => {
                    let stripe = if ((x + y) / 8) % 2 == 0 { 0.8 } else { 0.2 };
                    if r < 0.3 { 1.0 - stripe } else { stripe }
                }
                _ => (x as f64 / n as f64) * if (x / 24 + y / 24) % 2 == 0 { 1.0 } else { 0.5 },
            };
            g[y * n + x] = (v * 255.0) as u8;
        }
    }
    dither(&g, n, n, ow, oh)
}

/// Media `t` into the script: a track every 25 s; paused 8..11 s in; a seek
/// back 30 s at 15 s.
pub fn media_at(t: Duration) -> MediaInput {
    let s = t.as_secs();
    let k = (s / TRACK_SECS) as usize % TRACKS.len();
    let (title, artist, album, len) = TRACKS[k];
    let into = s % TRACK_SECS;
    let played = if into < 8 {
        into
    } else if into < 11 {
        8
    } else {
        into - 3
    };
    let base = 20 + (k as u64) * 40;
    let pos = if into >= 15 {
        (base + played).saturating_sub(30)
    } else {
        base + played
    };
    MediaInput {
        title: Some(title.into()),
        artist: Some(artist.into()),
        album: Some(album.into()),
        playing: !(8..11).contains(&into),
        position_ms: Some(pos.min(len) * 1000),
        duration_ms: Some(len * 1000),
        art: Some((s / TRACK_SECS, cover((s / TRACK_SECS) as usize))),
        cover: Some((s / TRACK_SECS, full_cover((s / TRACK_SECS) as usize))),
    }
}

pub fn stop_file() -> std::path::PathBuf {
    std::env::var_os("CARPI_BENCH_STOP").map_or_else(|| "dis-bench.stop".into(), Into::into)
}

/// Bar probe steps: one message each (see [`probe_segment`]), no commit.
const PROBE_SEGMENTS: u8 = 60;
const PROBE_EVERY: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProbeStage {
    Blank,
    Bar(u8),
    Commit,
}

/// Find the cluster's auto-commit size. Blank + `39`, then a bar grows one
/// step per message every 250 ms with a step count beside it and *no* `39`,
/// then a `39`, repeat. If the cluster buffers until full, the bar and count
/// jump together; the counts Drew sees at each jump give the size.
struct Probe {
    stage: ProbeStage,
    at: Instant,
    round: u32,
}

/// Step `i` of the bar: full clip, the step count (1-based, 2 digits),
/// segment `i`. 21 B, so a jump after n steps means a ~21n B buffer.
pub fn probe_segment(i: u8) -> Vec<u8> {
    use carpi_dis::cmd::{Ink, Op, text_flags};
    use carpi_dis::font::{Font, audscii};
    use carpi_dis::proto::{FULL, Rect};
    let mut b = Op::Clip(FULL).encode();
    b.extend(
        Op::Text {
            clip: FULL,
            x: 24,
            y: 36,
            flags: text_flags(Font::Fixed, Ink::Opaque),
            bytes: audscii(&format!("{:02}", i + 1)),
        }
        .encode(),
    );
    b.extend(Op::Fill(Rect::new(2 + i, 12, 1, 16)).encode());
    b
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
        lanes: bench_lanes(m, side),
        // Trip info for the pocket: 12 km / 15 min out. Drew's phone is
        // imperial with a 12 h clock, so the bench says so too.
        eta: Some("2:32 PM".into()),
        distance_unit: Some("mi".into()),
        time_to_destination_s: Some((900.0 - s).max(0.0)),
        destination_distance_m: Some((12_400.0 - s * SPEED_MS).max(0.0)),
        ..Default::default()
    }
}

/// Plausible lanes per scripted maneuver, left to right.
pub fn bench_lanes(m: &str, side: Option<&str>) -> Vec<carpi_dis::lanes::Lane> {
    let a = |shape: &str, hl: bool| carpi_dis::lanes::LaneArrow {
        shape: shape.into(),
        highlighted: hl,
    };
    let right = side == Some("right");
    match m {
        "turn" if right => vec![
            vec![a("normal_left", false)],
            vec![a("straight", false)],
            vec![a("straight", false), a("normal_right", true)],
            vec![a("normal_right", true)],
        ],
        "turn" => vec![
            vec![a("uturn_left", false), a("normal_left", true)],
            vec![a("normal_left", true)],
            vec![a("straight", false)],
            vec![a("straight", false)],
            vec![a("normal_right", false)],
        ],
        "slight_turn" => vec![
            vec![a("straight", false)],
            vec![a("straight", false)],
            vec![a("slight_left", true)],
        ],
        _ => Vec::new(),
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
    next_speed: Instant,
    speed_n: u32,
    egg_track: Option<u64>,
    /// Top line 1 with nav: a media title, as a marquee (line 2 left alone).
    top: carpi_dis::top::TopLines,
    probe: Option<Probe>,
}

impl Bench {
    pub fn new(now: Instant, screens: Screens, secs: f64) -> Self {
        let dis = Dis::new(DisConfig {
            readings_pages: reading_pages(),
            media_layout: if screens == Screens::MediaArt {
                MediaLayout::Art
            } else {
                MediaLayout::Stack
            },
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
            next_speed: now,
            speed_n: 1,
            egg_track: None,
            top: carpi_dis::top::TopLines::new(),
            probe: (screens == Screens::Probe).then_some(Probe {
                stage: ProbeStage::Blank,
                at: now,
                round: 1,
            }),
        };
        if screens == Screens::Probe {
            b.dis.set_probe(now, true);
        }
        b.dis.start(now);
        b.tick(now);
        b
    }

    /// Time's up, or the stop file exists (`CARPI_BENCH_STOP`, default
    /// `dis-bench.stop` in the working dir), so runs end cleanly instead of
    /// being killed with the link open.
    pub fn done(&self, now: Instant) -> bool {
        now >= self.end || stop_file().exists()
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
            if self.screens == Screens::Nav {
                self.top.set_lines(now, "One More Time - Daft Punk", None);
                self.top.tick(now);
                while let Some(f) = self.top.poll_transmit() {
                    self.out.push_back(f);
                }
            }
            if now >= self.next_input {
                self.inputs(now);
                self.next_input = now + INPUT_EVERY;
            }
            if self.screens == Screens::Accel && now >= self.next_speed {
                self.dis.set_speed(now, speed_at(now - self.t0));
                // 50 ms +- 10 ms, like a real bus rate with jitter.
                self.speed_n = self
                    .speed_n
                    .wrapping_mul(1_103_515_245)
                    .wrapping_add(12_345);
                let jitter = (self.speed_n >> 16) % 21;
                self.next_speed = now + Duration::from_millis(40 + u64::from(jitter));
            }
        }
        if now >= self.next_stats {
            self.report(now);
            self.next_stats = now + STATS_EVERY;
        }
        self.dis.tick(now);
        self.after(now);
    }

    fn probe_step(&mut self, now: Instant) {
        use carpi_dis::cmd::Op;
        use carpi_dis::pack::Message;
        let Some(p) = &self.probe else { return };
        if now < p.at || !self.dis.session().owned() || !self.dis.session().frame_idle() {
            return;
        }
        let (stage, round) = (p.stage, p.round);
        let msg = |bytes: Vec<u8>| Message {
            bytes,
            gap: Duration::ZERO,
        };
        let (frame, next, wait, say) = match stage {
            ProbeStage::Blank => {
                let mut b = Op::Clear(carpi_dis::proto::FULL).encode();
                b.push(0x39);
                let say = Some(format!("probe round {round}: blank"));
                (
                    vec![msg(b)],
                    ProbeStage::Bar(0),
                    Duration::from_secs(2),
                    say,
                )
            }
            ProbeStage::Bar(i) => {
                let next = if i + 1 >= PROBE_SEGMENTS {
                    ProbeStage::Commit
                } else {
                    ProbeStage::Bar(i + 1)
                };
                let say = (i == 0).then(|| format!("probe round {round}: bar, no commits"));
                (vec![msg(probe_segment(i))], next, PROBE_EVERY, say)
            }
            ProbeStage::Commit => (
                vec![msg(vec![0x39])],
                ProbeStage::Blank,
                Duration::from_secs(3),
                Some(format!("probe round {round}: commit")),
            ),
        };
        if self.dis.submit_raw(now, frame) {
            if let Some(s) = say {
                self.say(now, s);
            }
            let p = self.probe.as_mut().unwrap();
            p.stage = next;
            p.at = now + wait;
            if stage == ProbeStage::Commit {
                p.round += 1;
            }
        }
    }

    fn inputs(&mut self, now: Instant) {
        if self.probe.is_some() {
            self.probe_step(now);
            return;
        }
        if self.screens == Screens::Accel {
            if self.page != Some(ScreenId::Accel) {
                self.page = Some(ScreenId::Accel);
                self.dis.set_user_page(now, Some(ScreenId::Accel));
            }
            return;
        }
        if self.screens == Screens::Call {
            self.dis.set_call(now, call_at(now - self.t0));
            return;
        }
        if self.screens == Screens::Eggs {
            let t = now - self.t0;
            self.dis.set_media(now, media_at(t));
            let track = t.as_secs() / TRACK_SECS;
            let eggs = bench_eggs();
            if !eggs.is_empty() && self.egg_track != Some(track) {
                self.egg_track = Some(track);
                let egg = eggs[track as usize % eggs.len()].clone();
                // Loop for about 15 s of the 25 s track.
                let len: Duration = egg.frames.iter().map(|(_, d)| *d).sum();
                let loops = (15.0 / len.as_secs_f64().max(0.1)).ceil() as u32;
                let msg = format!(
                    "egg {} ({} frames, {loops} loops)",
                    track as usize % eggs.len() + 1,
                    egg.frames.len()
                );
                self.say(now, msg);
                self.dis.play_egg(now, egg, loops);
            }
            return;
        }
        if matches!(self.screens, Screens::MediaStack | Screens::MediaArt) {
            // No user page: media wins the centre through the arbiter.
            self.dis.set_media(now, media_at(now - self.t0));
            return;
        }
        let t = now - self.t0;
        let page = match self.screens {
            Screens::Nav => ScreenId::Nav,
            Screens::Readings => ScreenId::Readings,
            Screens::Both if (t.as_secs() / FLIP_EVERY.as_secs()).is_multiple_of(2) => {
                ScreenId::Nav
            }
            Screens::Both
            | Screens::Probe
            | Screens::MediaStack
            | Screens::MediaArt
            | Screens::Call
            | Screens::Accel
            | Screens::Eggs => ScreenId::Readings,
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
            .chain(self.top.next_deadline())
            .min()
            .unwrap();
        if !self.stopped {
            d = d
                .min(self.next_wake)
                .min(self.next_kl15)
                .min(self.next_input);
            if self.screens == Screens::Accel {
                d = d.min(self.next_speed);
            }
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
        // Wraps after six legs (the trip info keeps counting down).
        let w = nav_at(Duration::from_secs(120));
        assert_eq!((&w.maneuver, w.distance_m, &w.road, &w.lanes), (&a.maneuver, a.distance_m, &a.road, &a.lanes));
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
