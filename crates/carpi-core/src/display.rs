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
}

impl Default for DisSetup {
    fn default() -> Self {
        DisSetup {
            enabled: false,
            readings: default_readings(),
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

/// Pages the DIS keys step through.
const PAGES: [ScreenId; 2] = [ScreenId::Readings, ScreenId::Nav];

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
        ..Default::default()
    })
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
        }
    }

    /// New settings. A change in readings restarts the driver.
    pub fn configure(&mut self, setup: DisSetup, now: Instant) {
        if setup.readings != self.setup.readings {
            if self.running {
                self.dis.stop(now);
            }
            // Frames from the old driver's stop are dropped; the cluster
            // times the session out by itself.
            self.dis = new_dis(&setup);
            self.running = false;
            self.nav = None;
            self.readings = None;
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

    pub fn key(&mut self, now: Instant, k: DisKey) {
        match k {
            DisKey::Up if self.user_page => self.page = (self.page + PAGES.len() - 1) % PAGES.len(),
            DisKey::Down if self.user_page => self.page = (self.page + 1) % PAGES.len(),
            DisKey::Up | DisKey::Down | DisKey::Select => self.user_page = true,
            DisKey::Back => self.user_page = false,
        }
        if self.running {
            self.dis
                .set_user_page(now, self.user_page.then_some(PAGES[self.page]));
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
        if want && !self.running {
            self.running = true;
            self.dis.start(now);
            self.dis
                .set_user_page(now, self.user_page.then_some(PAGES[self.page]));
        } else if !want && self.running {
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
            self.dis.tick(now);
            while let Some(e) = self.dis.poll_event() {
                self.on_event(e);
            }
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
            if let Some(v) = &view
                && gate.check(TxClass::DisplayDdp, &f, now, v).is_ok()
            {
                out.push(Output::Send { iface, frame: f });
            }
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
