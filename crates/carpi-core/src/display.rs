//! The cluster display on ICAN: the `carpi-dis` driver fed from the store,
//! plus the 0x363/0x365 top lines.
//!
//! - Session frames (0x6C0) go through the ICAN gate as `DisplayDdp`. The
//!   session only runs while the gate would pass them (bus awake, Kl15 on);
//!   it's stopped when that goes, and anything it still sends is refused.
//! - Inputs are read from the store every tick: `nav.*` from the LIVI bridge,
//!   and the readings page from its configured store names.
//! - DIS keys (wheel keys while the DIS has focus) pick the user page.
//! - Top lines (`DisplayTop`) only while `rnse.on_tv`: on TV the firmware
//!   redirects the RNS-E's own text, so the lines are ours (PLAN 6). Nav
//!   distance and road while navigating, else media title and artist.
//! - Session state and errors land in the store as `dis.*` for the pages.

use carpi_can::Frame;
use carpi_dis::screens::call::CallInput;
use carpi_dis::screens::media::{MediaInput, MediaLayout};
use carpi_dis::screens::nav::{NavInput, format_distance};
use carpi_dis::screens::readings::{ReadingSpec, ReadingsInput};
use carpi_dis::session::{Phase, Window};
use carpi_dis::top::{self, TopLines};
use carpi_dis::{Dis, DisConfig, ScreenId, SessionEvent};
use carpi_gate::{BusView, Gate, TxClass};
use carpi_input::DisKey;
use carpi_nm::BusState;
use carpi_state::{Store, Value};
use std::time::{Duration, Instant};

use crate::control::Output;

/// One cell of the readings page.
#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    /// Store name.
    pub key: String,
    pub label: String,
    pub decimals: u8,
    /// Shown value = store value x scale.
    pub scale: f64,
}

impl Reading {
    pub fn new(key: &str, label: &str, decimals: u8, scale: f64) -> Self {
        Reading {
            key: key.into(),
            label: label.into(),
            decimals,
            scale,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DisSetup {
    /// Off by default: the first runs on the car send nothing to the cluster.
    pub enabled: bool,
    /// Up to 8; left column top to bottom, then right.
    pub readings: Vec<Reading>,
    /// Our default page holds the centre whenever nothing else wants it;
    /// off: we idle as a tab in the cluster menu until it's picked.
    pub claim_always: bool,
    /// What "always" shows and the menu tab opens to: readings or accel.
    pub default_page: ScreenId,
    /// Each takes the centre while it has content, and gives it back.
    pub auto_nav: bool,
    pub auto_phone: bool,
    pub auto_media: bool,
    pub media_layout: MediaLayout,
    /// Hold for the full cover on a new track, after it's drawn. `None` = off.
    pub cover_flash: Option<Duration>,
}

impl Default for DisSetup {
    fn default() -> Self {
        DisSetup {
            enabled: false,
            readings: default_readings(),
            claim_always: true,
            default_page: ScreenId::Readings,
            auto_nav: true,
            auto_phone: true,
            auto_media: true,
            media_layout: MediaLayout::Stack,
            cover_flash: Some(Duration::from_secs(4)),
        }
    }
}

/// ICAN values only, so the page needs no diag polling. The odometer is
/// shown in 1000 km: six digits don't fit a cell.
pub fn default_readings() -> Vec<Reading> {
    vec![
        Reading::new("vehicle.speed_kmh", "Spd", 0, 1.0),
        Reading::new("engine.rpm", "RPM", 0, 1.0),
        Reading::new("engine.coolant_c", "H2O", 0, 1.0),
        Reading::new("ambient.outside_temp_c", "Out", 1, 1.0),
        Reading::new("power.battery_v", "Bat", 1, 1.0),
        Reading::new("fuel.level_l", "Ful", 0, 1.0),
        Reading::new("vehicle.odometer_km", "Odo", 0, 0.001),
        Reading::new("light.dim_display_pct", "Dim", 0, 1.0),
    ]
}

/// Pages the DIS keys step through; always available (Drew). Nav, phone and
/// media come and go with their content instead.
const PAGES: [ScreenId; 2] = [ScreenId::Readings, ScreenId::Accel];

/// `dis.*` values are rewritten every tick; let them lapse if ticks stop.
const DERIVED: Duration = Duration::from_secs(1);

pub(crate) struct Display {
    setup: DisSetup,
    dis: Dis,
    top: TopLines,
    running: bool,
    /// Index into [`PAGES`], and whether a user page is shown at all.
    page: usize,
    user_page: bool,
    nav: Option<NavInput>,
    readings: Option<ReadingsInput>,
    top_text: String,
    last_error: Option<String>,
    focus: bool,
    media: Option<MediaInput>,
    call: Option<CallInput>,
    /// Logged state, so the journal shows each change once (car test
    /// 2026-11-08: the DIS did nothing and logged nothing).
    log: LogState,
}

#[derive(Default)]
struct LogState {
    wait: Option<String>,
    phase: Option<Phase>,
    window: &'static str,
    want: Option<Option<ScreenId>>,
    /// Gate refusals of DDP frames since the last log line, and the reason.
    refused: u32,
    refusal: Option<String>,
}

fn new_dis(setup: &DisSetup) -> Dis {
    Dis::new(DisConfig {
        readings: setup
            .readings
            .iter()
            .take(8)
            .map(|r| ReadingSpec {
                key: r.key.clone(),
                label: r.label.clone(),
                decimals: r.decimals,
            })
            .collect(),
        claim_always: setup.claim_always,
        default_page: setup.default_page,
        auto_nav: setup.auto_nav,
        auto_call: setup.auto_phone,
        auto_media: setup.auto_media,
        media_layout: setup.media_layout,
        cover_flash: setup.cover_flash,
        ..Default::default()
    })
}

fn media_input(store: &Store, now: Instant) -> MediaInput {
    if store.get_bool("media.active", now) != Some(true) {
        return MediaInput::default();
    }
    let ms = |k| {
        store
            .get_num(k, now)
            .filter(|v| *v >= 0.0)
            .map(|v| v as u64)
    };
    MediaInput {
        title: text(store, "media.title", now),
        artist: text(store, "media.artist", now),
        album: text(store, "media.album", now),
        playing: store.get_bool("media.playing", now) == Some(true),
        position_ms: ms("media.position_ms"),
        duration_ms: ms("media.duration_ms"),
        // No album art reaches carpid yet (LIVI doesn't forward it).
        art: None,
        cover: None,
    }
}

fn call_input(store: &Store, now: Instant) -> CallInput {
    let pct = |k| store.get_num(k, now).map(|v| v.clamp(0.0, 100.0) as u8);
    CallInput {
        state: text(store, "call.state", now),
        name: text(store, "call.name", now),
        number: text(store, "call.number", now),
        signal_pct: pct("phone.signal_pct"),
        battery_pct: pct("phone.battery_pct"),
    }
}

fn text(store: &Store, name: &str, now: Instant) -> Option<String> {
    match store.get(name, now) {
        Some(Value::Text(s)) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

fn nav_input(store: &Store, now: Instant) -> NavInput {
    if store.get_bool("nav.active", now) != Some(true) {
        return NavInput::default();
    }
    NavInput {
        maneuver: text(store, "nav.maneuver", now),
        side: text(store, "nav.side", now),
        roundabout_angle: store.get_num("nav.roundabout_angle", now).map(|a| a as i32),
        distance_m: store.get_num("nav.distance_m", now),
        road: text(store, "nav.road", now),
        ..Default::default()
    }
}

/// The two 8-cell lines: nav distance and road while navigating, else
/// media title and artist, else blank.
pub fn top_lines(store: &Store, now: Instant) -> (String, String) {
    if store.get_bool("nav.active", now) == Some(true) {
        let (v, unit) = format_distance(store.get_num("nav.distance_m", now));
        let dist = if unit.is_empty() {
            v
        } else {
            format!("{v} {unit}")
        };
        return (dist, text(store, "nav.road", now).unwrap_or_default());
    }
    if store.get_bool("media.active", now) == Some(true) {
        return (
            text(store, "media.title", now).unwrap_or_default(),
            text(store, "media.artist", now).unwrap_or_default(),
        );
    }
    (String::new(), String::new())
}

fn cells(l1: &str, l2: &str) -> [u8; 16] {
    let (a, b) = (top::encode(l1), top::encode(l2));
    let mut out = [0x20; 16];
    out[..8].copy_from_slice(&a[..8]);
    out[8..].copy_from_slice(&b[..8]);
    out
}

fn may_draw(view: &BusView) -> bool {
    view.bus == BusState::Awake && view.kl15 == Some(true)
}

fn phase_name(p: Phase) -> &'static str {
    match p {
        Phase::Down => "down",
        Phase::Linking => "linking",
        Phase::AwaitMode | Phase::AwaitCaps | Phase::AwaitSetup => "handshake",
        Phase::Ready => "ready",
    }
}

fn window_name(w: Window) -> &'static str {
    match w {
        Window::Idle => "idle",
        Window::Claiming { .. } => "claiming",
        Window::Owned => "owned",
        Window::Busy => "busy",
        Window::Free { .. } => "free",
        Window::Retry { .. } => "retry",
    }
}

impl Display {
    pub fn new(setup: DisSetup) -> Self {
        Display {
            dis: new_dis(&setup),
            setup,
            top: TopLines::new(),
            running: false,
            page: 0,
            user_page: false,
            nav: None,
            readings: None,
            top_text: String::new(),
            last_error: None,
            focus: false,
            media: None,
            call: None,
            log: LogState::default(),
        }
    }

    /// New settings. Any change to what the driver shows (everything but
    /// `enabled`) rebuilds it; `enabled` alone just starts or stops it.
    pub fn configure(&mut self, setup: DisSetup, now: Instant) {
        let shape = |d: &DisSetup| DisSetup {
            enabled: false,
            ..d.clone()
        };
        if shape(&setup) != shape(&self.setup) {
            if self.running {
                self.dis.stop(now);
            }
            // Frames from the old driver's stop are dropped; the cluster
            // times the session out by itself.
            self.dis = new_dis(&setup);
            self.running = false;
            self.nav = None;
            self.readings = None;
            self.media = None;
            self.call = None;
        }
        self.setup = setup;
    }

    pub fn session(&self) -> &carpi_dis::Session {
        self.dis.session()
    }

    pub fn on_frame(&mut self, now: Instant, f: &Frame) {
        if self.running && !f.extended && f.id == carpi_signals::ids::DDP_RX {
            self.dis.on_frame(now, f);
        }
    }

    /// Wheel keys while the DIS has focus. Up/Down walk one list: the
    /// readings subpages, then accel, then round again. The first key picks
    /// the page on screen (or the default). Back unpicks it.
    pub fn key(&mut self, now: Instant, k: DisKey) {
        if !self.user_page {
            if k == DisKey::Back {
                return;
            }
            let on = self.dis.wanted().filter(|id| PAGES.contains(id));
            let page = on.unwrap_or(self.setup.default_page);
            self.page = PAGES.iter().position(|p| *p == page).unwrap_or(0);
            self.user_page = true;
        } else {
            match k {
                DisKey::Up | DisKey::Down => {
                    let down = k == DisKey::Down;
                    let (i, n) = (
                        self.dis.readings().page_index(),
                        self.dis.readings().page_count(),
                    );
                    match PAGES[self.page] {
                        ScreenId::Readings if down && i + 1 < n => {
                            self.dis.step_readings(now, 1);
                        }
                        ScreenId::Readings if !down && i > 0 => {
                            self.dis.step_readings(now, -1);
                        }
                        ScreenId::Readings => {
                            self.page = PAGES
                                .iter()
                                .position(|p| *p == ScreenId::Accel)
                                .unwrap_or(0);
                        }
                        _ => {
                            // Into readings at its first (down) or last (up) subpage.
                            let target = if down { 0 } else { n.saturating_sub(1) };
                            self.dis.step_readings(now, target as i32 - i as i32);
                            self.page = 0;
                        }
                    }
                }
                DisKey::Select => {}
                DisKey::Back => self.user_page = false,
            }
        }
        let page = self.user_page.then_some(PAGES[self.page]);
        tracing::info!("dis: key {k:?} -> page {page:?}");
        self.dis.set_user_page(now, page);
    }

    /// DIS focus on or off (the wheel drives the DIS).
    pub fn set_focus(&mut self, now: Instant, on: bool) {
        if on != self.focus {
            self.focus = on;
            self.dis.set_focus(now, on);
        }
    }

    pub fn next_deadline(&self, now: Instant) -> Option<Instant> {
        if !self.running {
            return None;
        }
        [self.dis.next_deadline(now), self.top.next_deadline()]
            .into_iter()
            .flatten()
            .min()
    }

    /// Timers, inputs and output. `view`: the ICAN gate's view, if there's an
    /// ICAN interface.
    pub fn service(
        &mut self,
        now: Instant,
        store: &mut Store,
        view: Option<BusView>,
        gate: Option<(usize, &mut Gate)>,
        out: &mut Vec<Output>,
    ) {
        let (Some(view), Some((iface, gate))) = (view, gate) else {
            self.publish(store, now);
            return;
        };
        let want = self.setup.enabled && may_draw(&view);
        let wait = (!want).then(|| {
            if !self.setup.enabled {
                "disabled in config".to_string()
            } else if view.bus != BusState::Awake {
                format!("bus {:?}", view.bus)
            } else {
                format!("kl15 {:?}", view.kl15)
            }
        });
        if wait != self.log.wait {
            match &wait {
                Some(w) => tracing::info!("dis: waiting ({w})"),
                None => tracing::info!("dis: starting (bus awake, kl15 on)"),
            }
            self.log.wait = wait;
        }
        if want && !self.running {
            self.running = true;
            self.dis.start(now);
            self.dis
                .set_user_page(now, self.user_page.then_some(PAGES[self.page]));
        } else if !want && self.running {
            tracing::info!("dis: stopped");
            self.running = false;
            self.dis.stop(now);
            self.nav = None;
            self.readings = None;
        }

        if self.running {
            let nav = nav_input(store, now);
            if self.nav.as_ref() != Some(&nav) {
                self.dis.set_nav(now, nav.clone());
                self.nav = Some(nav);
            }
            let r = self.readings_input(store, now);
            if self.readings.as_ref() != Some(&r) {
                self.dis.set_readings(now, r.clone());
                self.readings = Some(r);
            }
            let m = media_input(store, now);
            if self.media.as_ref() != Some(&m) {
                self.dis.set_media(now, m.clone());
                self.media = Some(m);
            }
            let c = call_input(store, now);
            if self.call.as_ref() != Some(&c) {
                self.dis.set_call(now, c.clone());
                self.call = Some(c);
            }
            if let Some(v) = store.get_num("vehicle.speed_kmh", now) {
                self.dis.set_speed(now, v);
            }
            self.dis.tick(now);
            while let Some(e) = self.dis.poll_event() {
                self.on_event(e);
            }
            self.log_state();
            if let Some(e) = self.dis.last_error.take() {
                self.last_error = Some(format!("frame skipped: {e:?}"));
            }
        }
        self.flush(now, Some(view), iface, gate, out);

        // Top lines.
        let on_tv = store.get_bool("rnse.on_tv", now) == Some(true);
        if self.setup.enabled && on_tv {
            let (l1, l2) = top_lines(store, now);
            self.top_text = format!("{l1}|{l2}");
            self.top.set_cells(now, cells(&l1, &l2));
            self.top.tick(now);
            let mut refused = false;
            while let Some(f) = self.top.poll_transmit() {
                match gate.check(TxClass::DisplayTop, &f, now, &view) {
                    Ok(()) => out.push(Output::Send { iface, frame: f }),
                    Err(_) => refused = true,
                }
            }
            if refused {
                // Send again once the gate lets it through.
                self.top.invalidate();
            }
        } else {
            while self.top.poll_transmit().is_some() {}
            self.top.invalidate();
            self.top_text.clear();
        }
        self.publish(store, now);
    }

    /// Session frames out through the gate. With no view nothing passes.
    pub fn flush(
        &mut self,
        now: Instant,
        view: Option<BusView>,
        iface: usize,
        gate: &mut Gate,
        out: &mut Vec<Output>,
    ) {
        while let Some(f) = self.dis.poll_transmit() {
            let Some(v) = &view else { continue };
            match gate.check(TxClass::DisplayDdp, &f, now, v) {
                Ok(()) => out.push(Output::Send { iface, frame: f }),
                Err(e) => {
                    // A dropped DDP frame stalls the TP2 link; say so.
                    let why = format!("{e:?}");
                    if self.log.refusal.as_deref() != Some(why.as_str()) {
                        tracing::warn!(
                            "dis: gate refused DDP frame ({why}); {} refused before",
                            self.log.refused
                        );
                        self.log.refusal = Some(why);
                        self.log.refused = 0;
                    }
                    self.log.refused += 1;
                }
            }
        }
    }

    /// Phase, window and wanted screen, each logged when it changes.
    fn log_state(&mut self) {
        let s = self.dis.session();
        let (phase, window) = (s.phase(), window_name(s.window()));
        if self.log.phase != Some(phase) {
            tracing::info!("dis: session {}", phase_name(phase));
            self.log.phase = Some(phase);
        }
        if self.log.window != window {
            tracing::info!("dis: window {window} (generation {})", s.generation());
            self.log.window = window;
        }
        let shown = self.dis.wanted();
        if self.log.want != Some(shown) {
            match shown {
                Some(id) => tracing::info!("dis: wants the centre for {id:?}"),
                None => tracing::info!(
                    "dis: nothing to show (no nav guidance, no page picked{})",
                    if self.user_page {
                        ""
                    } else {
                        "; wheel keys with DIS focus pick one"
                    }
                ),
            }
            self.log.want = Some(shown);
        }
    }

    fn readings_input(&self, store: &Store, now: Instant) -> ReadingsInput {
        let mut values = [None; 8];
        for (v, r) in values.iter_mut().zip(&self.setup.readings) {
            *v = store.get_num(&r.key, now).map(|x| x * r.scale);
        }
        ReadingsInput {
            title: "Readings".into(),
            values,
        }
    }

    fn on_event(&mut self, e: SessionEvent) {
        match &e {
            SessionEvent::FrameDone => {}
            SessionEvent::Ready(p) => tracing::info!("dis: handshake done: {p:?}"),
            other => tracing::info!("dis: {other:?}"),
        }
        let msg = match e {
            SessionEvent::Unsupported(rec) => format!("cluster record not supported: {rec:02X?}"),
            SessionEvent::Fault => "claim refused (53 C0)".into(),
            SessionEvent::AppError(b) => format!("cluster error {b:02X?}"),
            SessionEvent::LinkClosed(r) => format!("link closed: {r:?}"),
            SessionEvent::FrameDropped => "frame dropped".into(),
            _ => return,
        };
        self.last_error = Some(msg);
    }

    fn publish(&self, store: &mut Store, now: Instant) {
        let s = self.dis.session();
        let mut set = |name: &str, v: Option<Value>| store.set(name, v, now, DERIVED);
        set("dis.enabled", Some(Value::Bool(self.setup.enabled)));
        let state = if !self.setup.enabled {
            "off"
        } else if !self.running {
            "waiting"
        } else {
            phase_name(s.phase())
        };
        set("dis.state", Some(Value::Text(state.into())));
        set(
            "dis.window",
            (self.running && s.phase() == Phase::Ready)
                .then(|| Value::Text(window_name(s.window()).into())),
        );
        set("dis.generation", Some(Value::Num(s.generation() as f64)));
        set(
            "dis.page",
            Some(Value::Text(match (self.user_page, PAGES[self.page]) {
                (false, _) => "none".into(),
                (true, ScreenId::Readings) => "readings".into(),
                (true, ScreenId::Nav) => "nav".into(),
                (true, ScreenId::Media) => "media".into(),
                (true, ScreenId::Call) => "call".into(),
                (true, ScreenId::Cover) => "cover".into(),
                (true, ScreenId::Accel) => "accel".into(),
                (true, ScreenId::Egg) => "egg".into(),
            })),
        );
        set(
            "dis.shown",
            self.dis.shown().map(|id| {
                Value::Text(match id {
                    ScreenId::Readings => "readings".into(),
                    ScreenId::Nav => "nav".into(),
                    ScreenId::Media => "media".into(),
                    ScreenId::Call => "call".into(),
                    ScreenId::Cover => "cover".into(),
                    ScreenId::Accel => "accel".into(),
                    ScreenId::Egg => "egg".into(),
                })
            }),
        );
        set("dis.last_error", self.last_error.clone().map(Value::Text));
        set(
            "dis.top_text",
            (!self.top_text.is_empty()).then(|| Value::Text(self.top_text.clone())),
        );
    }
}
