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
use carpi_dis::icons::Art;
use carpi_dis::image::{CoverParams, cover};
use carpi_dis::screens::call::CallInput;
use carpi_dis::screens::media::{ART as MEDIA_ART, MediaInput, MediaLayout};
use carpi_dis::screens::nav::{
    Clock, LaneLayout, NavConfig, NavInput, NavStyle, Units, format_distance_in,
};
use carpi_dis::screens::readings::{
    ReadingCell, ReadingSpec, ReadingsInput, ReadingsPage, unit_for,
};
use carpi_dis::session::{Phase, Window};
use carpi_dis::top::TopLines;
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
    /// Up to 8; left column top to bottom, then right. Used as one unnamed
    /// page when `pages` is empty.
    pub readings: Vec<Reading>,
    /// Named readings pages (the old readings app's DIS pages); the wheel
    /// steps through them.
    pub pages: Vec<DisPage>,
    /// Our default page holds the centre whenever nothing else wants it;
    /// off: we idle as a tab in the cluster menu until it's picked.
    pub claim_always: bool,
    /// What "always" shows and the menu tab opens to: readings, accel or media.
    pub default_page: ScreenId,
    /// Per source: take the centre when it gets content; give it back when
    /// the content goes (without release the screen stays up).
    pub claim_nav: bool,
    pub release_nav: bool,
    pub claim_phone: bool,
    pub release_phone: bool,
    pub claim_media: bool,
    pub release_media: bool,
    pub media_layout: MediaLayout,
    /// Hold for the full cover on a new track, after it's drawn. `None` = off.
    pub cover_flash: Option<Duration>,
    /// Distance units on the DIS (`Auto`: as the phone shows them).
    pub units: Units,
    /// ETA clock (`Auto`: 12 h if the phone's ETA has AM/PM).
    pub clock: Clock,
    /// Nav screen look: our icons and layout, or the stock RNS-E look.
    pub nav_style: NavStyle,
    /// Nav info, in either look: the maneuver distance column, the pocket
    /// (master switch) and its ETA / time left / distance left pages, and
    /// where lanes go. Stock with all off is the RNS-E's own layout.
    pub nav_distance: bool,
    pub nav_pocket: bool,
    pub nav_eta: bool,
    pub nav_time_left: bool,
    pub nav_dest_distance: bool,
    pub nav_lanes: LaneLayout,
    /// The accel test's lines as typed (`0-60mph`, `1/4mi`, `speed`), up to 4.
    pub accel: Vec<String>,
}

impl Default for DisSetup {
    fn default() -> Self {
        DisSetup {
            enabled: false,
            readings: default_readings(),
            pages: Vec::new(),
            claim_always: true,
            default_page: ScreenId::Readings,
            claim_nav: true,
            release_nav: true,
            claim_phone: true,
            release_phone: true,
            claim_media: true,
            release_media: true,
            media_layout: MediaLayout::Stack,
            cover_flash: Some(Duration::from_secs(4)),
            units: Units::Auto,
            clock: Clock::Auto,
            nav_style: NavStyle::Fancy,
            nav_distance: true,
            nav_pocket: true,
            nav_eta: true,
            nav_time_left: true,
            nav_dest_distance: true,
            nav_lanes: LaneLayout::Flip,
            accel: default_accel(),
        }
    }
}

/// A named readings page.
#[derive(Clone, Debug, PartialEq)]
pub struct DisPage {
    pub name: String,
    /// Up to 8; left column top to bottom, then right.
    pub cells: Vec<DisCell>,
}

/// One readings cell: any store value (ICAN, or a diag catalog value, which
/// is polled while its page is on screen).
#[derive(Clone, Debug, PartialEq)]
pub struct DisCell {
    pub key: String,
    pub decimals: u8,
    /// Shown value = value x scale, after any unit conversion.
    pub scale: f64,
    /// Show in this unit (`°F`, `mph`, `psi`, ...), converted from the key's
    /// own; `None`: as the key has it.
    pub unit: Option<String>,
    /// Icon name (`oil`, `boost`, ...); `None`: picked from the key.
    pub icon: Option<String>,
}

/// A key's own unit: the diag catalog's when it names one (`engine.boost.actual`
/// is mbar), else the key's suffix (`_c`, `_kmh`, ...).
pub fn key_unit(key: &str) -> &'static str {
    static CATALOG: std::sync::OnceLock<carpi_diag::Catalog> = std::sync::OnceLock::new();
    let cat = CATALOG.get_or_init(carpi_diag::Catalog::seed);
    match cat.get(key).map(|d| d.unit) {
        Some(carpi_diag::catalog::Unit::Expect(u)) => u,
        _ => unit_for(key),
    }
}

/// `v` in unit `from` (as the key's suffix gives it) expressed in `to`.
/// `None` if there's no conversion between them.
pub fn convert(v: f64, from: &str, to: &str) -> Option<f64> {
    if from == to || to.is_empty() {
        return Some(v);
    }
    Some(match (from, to) {
        ("°C", "°F") => v * 9.0 / 5.0 + 32.0,
        ("°F", "°C") => (v - 32.0) * 5.0 / 9.0,
        ("km/h", "mph") => v / 1.609_344,
        ("mph", "km/h") => v * 1.609_344,
        ("km", "mi") => v / 1.609_344,
        ("mi", "km") => v * 1.609_344,
        ("bar", "psi") => v * 14.503_77,
        ("bar", "kPa") => v * 100.0,
        ("bar", "mbar") => v * 1000.0,
        ("mbar", "bar") => v / 1000.0,
        ("mbar", "psi") => v * 0.014_503_77,
        ("mbar", "kPa") => v / 10.0,
        ("kPa", "psi") => v * 0.145_037_7,
        ("kPa", "bar") => v / 100.0,
        ("L", "gal") => v / 3.785_41,
        ("Nm", "lb-ft") => v * 0.737_562,
        _ => return None,
    })
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
/// `dis.*` values are rewritten every tick; let them lapse if ticks stop.
const DERIVED: Duration = Duration::from_secs(1);

pub(crate) struct Display {
    setup: DisSetup,
    dis: Dis,
    top: TopLines,
    running: bool,
    /// The page the driver picked, while `user_page`.
    picked: ScreenId,
    user_page: bool,
    nav: Option<NavInput>,
    readings: Option<ReadingsInput>,
    top_text: String,
    last_error: Option<String>,
    focus: bool,
    /// The current cover, decoded once per `media.art_id`.
    art: Option<(u64, Art, Art)>,
    /// The id last tried (so a cover that won't decode isn't retried every tick).
    art_tried: Option<u64>,
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

fn nav_config(setup: &DisSetup) -> NavConfig {
    NavConfig {
        units: setup.units,
        clock: setup.clock,
        style: setup.nav_style,
        show_distance: setup.nav_distance,
        pocket: setup.nav_pocket,
        show_eta: setup.nav_eta,
        show_time_left: setup.nav_time_left,
        show_dest_distance: setup.nav_dest_distance,
        lanes: setup.nav_lanes,
        ..NavConfig::default()
    }
}

fn readings_pages(setup: &DisSetup) -> Vec<ReadingsPage> {
    setup
        .pages
        .iter()
        .map(|p| ReadingsPage {
            name: p.name.clone(),
            cells: p
                .cells
                .iter()
                .take(8)
                .map(|c| ReadingCell {
                    key: c.key.clone(),
                    decimals: c.decimals,
                    icon: c.icon.clone(),
                    // The catalog knows units the key's name doesn't say
                    // (engine.maf_gs is g/s).
                    unit: c
                        .unit
                        .clone()
                        .or_else(|| Some(key_unit(&c.key).to_string()).filter(|u| !u.is_empty())),
                })
                .collect(),
        })
        .collect()
}

/// The old DIS client's lines (Drew's config), now with their unit.
pub fn default_accel() -> Vec<String> {
    ["0-60mph", "0-30mph", "40-70mph", "speed"]
        .map(String::from)
        .to_vec()
}

fn accel(setup: &DisSetup) -> carpi_dis::screens::accel::AccelConfig {
    let mph = setup.units == Units::Imperial;
    let (cfg, errors) = carpi_dis::screens::accel::accel_config(&setup.accel, mph);
    for e in errors {
        tracing::warn!("dis: accel line skipped: {e}");
    }
    cfg
}

fn new_dis(setup: &DisSetup) -> Dis {
    Dis::new(DisConfig {
        accel: accel(setup),
        readings_pages: readings_pages(setup),
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
        claim_nav: setup.claim_nav,
        release_nav: setup.release_nav,
        claim_call: setup.claim_phone,
        release_call: setup.release_phone,
        claim_media: setup.claim_media,
        release_media: setup.release_media,
        media_layout: setup.media_layout,
        cover_flash: setup.cover_flash,
        nav: nav_config(setup),
        ..Default::default()
    })
}

/// The cover in `media.art_*` (bridge 4a21f85) as the two arts the DIS uses:
/// the 48x48 px thumbnail for the art layout and the 128x96 native cover for
/// the full-centre screen and the flash, both through the cover pipeline
/// (Drew's old tester preset). `None` if there's no art or it won't decode.
pub fn decode_cover(base64_text: &str) -> Option<(Art, Art)> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64_text)
        .ok()?;
    let img = image::load_from_memory(&bytes).ok()?.to_rgb8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    if w == 0 || h == 0 {
        return None;
    }
    let raw = img.as_raw();
    let p = CoverParams::default();
    let (tw, th) = (usize::from(MEDIA_ART.w) * 2, usize::from(MEDIA_ART.h) * 2);
    Some((cover(raw, w, h, tw, th, &p), cover(raw, w, h, 128, 96, &p)))
}

fn media_input(store: &Store, now: Instant, art: Option<&(u64, Art, Art)>) -> MediaInput {
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
        art: art.map(|(id, thumb, _)| (*id, thumb.clone())),
        cover: art.map(|(id, _, full)| (*id, full.clone())),
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
    let ttd = store.get_num("nav.time_to_destination_s", now);
    NavInput {
        maneuver: text(store, "nav.maneuver", now),
        side: text(store, "nav.side", now),
        roundabout_angle: store.get_num("nav.roundabout_angle", now).map(|a| a as i32),
        distance_m: store.get_num("nav.distance_m", now),
        road: text(store, "nav.road", now),
        lanes: lanes(store, now),
        // AA sends the ETA as text; CarPlay only as Unix time, so it's the
        // car clock (local) plus the time left.
        eta: text(store, "nav.eta", now).or_else(|| eta_from_clock(store, now, ttd?)),
        distance_unit: text(store, "nav.distance_unit", now),
        time_to_destination_s: ttd,
        destination_distance_m: store.get_num("nav.destination_distance_m", now),
    }
}

/// `nav.lanes` (JSON of `carpi_proto::Lane`, left to right) as the screen's lanes.
fn lanes(store: &Store, now: Instant) -> Vec<carpi_dis::lanes::Lane> {
    let Some(json) = text(store, "nav.lanes", now) else {
        return Vec::new();
    };
    let Ok(lanes) = serde_json::from_str::<Vec<carpi_proto::Lane>>(&json) else {
        return Vec::new();
    };
    lanes
        .into_iter()
        .map(|l| {
            l.arrows
                .into_iter()
                .map(|a| carpi_dis::lanes::LaneArrow {
                    shape: a.shape,
                    highlighted: a.highlighted,
                })
                .collect()
        })
        .collect()
}

/// `HH:MM` local: the car clock (`clock.datetime`, local time) plus `secs`.
fn eta_from_clock(store: &Store, now: Instant, secs: f64) -> Option<String> {
    let clock = text(store, "clock.datetime", now)?;
    let hm = clock.get(11..16)?;
    let (h, m) = hm.split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    if !secs.is_finite() || secs < 0.0 {
        return None;
    }
    let total = (h * 60 + m + (secs / 60.0).round() as u32) % (24 * 60);
    Some(format!("{:02}:{:02}", total / 60, total % 60))
}

/// What a top-lines source shows: line 1, and line 2 when the cluster's own
/// line 2 (outside temperature) isn't there.
struct TopText {
    alone: String,
    line1: String,
    line2: String,
}

/// The top lines (0x363/0x365, 8 cells each, marquee when longer): the old
/// page's priority phone > nav > media, skipping whatever the centre shows,
/// so the driver gets that info anyway. One title never splits across both
/// lines. While the centre is ours the cluster puts the outside temperature
/// on line 2, so we leave 0x365 alone (`None`); otherwise line 2 carries the
/// second field. `None` overall: nothing to show.
pub fn top_lines(
    store: &Store,
    now: Instant,
    centre: Option<ScreenId>,
    centre_ours: bool,
    imperial: bool,
) -> Option<(String, Option<String>)> {
    let phone = || {
        let state = text(store, "call.state", now)?;
        let who = text(store, "call.name", now)
            .or_else(|| text(store, "call.number", now))
            .unwrap_or_else(|| "Unknown".into());
        let word = match state.as_str() {
            "ringing" => "Incoming",
            "dialing" => "Calling",
            "held" => "On hold",
            _ => "On call",
        };
        Some(TopText {
            alone: format!("{word}: {who}"),
            line1: who,
            line2: word.into(),
        })
    };
    let nav = || {
        if store.get_bool("nav.active", now) != Some(true) {
            return None;
        }
        let (v, unit) = format_distance_in(store.get_num("nav.distance_m", now), imperial);
        let dist = if unit.is_empty() {
            v
        } else {
            format!("{v} {unit}")
        };
        let road = text(store, "nav.road", now).unwrap_or_default();
        Some(TopText {
            alone: if road.is_empty() {
                dist.clone()
            } else {
                format!("{dist} {road}")
            },
            line1: dist,
            line2: road,
        })
    };
    let media = || {
        if store.get_bool("media.active", now) != Some(true) {
            return None;
        }
        let title = text(store, "media.title", now)?;
        let artist = text(store, "media.artist", now).unwrap_or_default();
        Some(TopText {
            alone: title.clone(),
            line1: title,
            line2: artist,
        })
    };
    let shows = |id: ScreenId| centre == Some(id);
    let pick = (!shows(ScreenId::Call))
        .then(phone)
        .flatten()
        .or_else(|| (!shows(ScreenId::Nav)).then(nav).flatten())
        .or_else(|| {
            let media_up = shows(ScreenId::Media) || shows(ScreenId::Cover) || shows(ScreenId::Egg);
            (!media_up).then(media).flatten()
        })?;
    Some(if centre_ours {
        (pick.alone, None)
    } else {
        (pick.line1, Some(pick.line2))
    })
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
            picked: ScreenId::Readings,
            user_page: false,
            nav: None,
            readings: None,
            top_text: String::new(),
            last_error: None,
            focus: false,
            art: None,
            art_tried: None,
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
        // Only the readings pages changed: swap them in, the session and the
        // page on screen stay (editing them used to drop our DIS tab).
        let rest = |d: &DisSetup| DisSetup {
            pages: Vec::new(),
            readings: Vec::new(),
            ..shape(d)
        };
        if shape(&setup) != shape(&self.setup) && rest(&setup) == rest(&self.setup) {
            let specs = setup
                .readings
                .iter()
                .take(8)
                .map(|r| ReadingSpec {
                    key: r.key.clone(),
                    label: r.label.clone(),
                    decimals: r.decimals,
                })
                .collect();
            self.dis
                .set_readings_pages(now, readings_pages(&setup), specs);
            self.readings = None;
            self.setup = setup;
            tracing::info!("dis: readings pages updated in place");
            return;
        }
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

    /// The stalk rocker (and the wheel while the DIS has focus): Up/Down
    /// step through the pages from the one on screen, the way the old DIS
    /// client did: media, nav while a route runs, the call while there is
    /// one, each readings page, accel. Only while our DIS owns the centre;
    /// otherwise ignored. Nav and calls still interrupt a picked page.
    /// Back unpicks it.
    pub fn key(&mut self, now: Instant, k: DisKey) {
        if !self.running || !self.dis.session().owned() {
            tracing::info!("dis: key {k:?} ignored (our DIS isn't up)");
            return;
        }
        let delta: i32 = match k {
            DisKey::Up => -1,
            DisKey::Down => 1,
            DisKey::Select => return,
            DisKey::Back => {
                self.user_page = false;
                tracing::info!("dis: key Back -> automatic");
                self.dis.set_user_page(now, None);
                return;
            }
        };
        // (screen, readings subpage)
        let mut list: Vec<(ScreenId, usize)> = vec![(ScreenId::Media, 0)];
        if self.dis.nav_available() {
            list.push((ScreenId::Nav, 0));
        }
        if self.dis.in_call() {
            list.push((ScreenId::Call, 0));
        }
        let (sub, subs) = (
            self.dis.readings().page_index(),
            self.dis.readings().page_count().max(1),
        );
        list.extend((0..subs).map(|i| (ScreenId::Readings, i)));
        list.push((ScreenId::Accel, 0));
        let here = match self.dis.shown().or(self.dis.wanted()) {
            Some(ScreenId::Readings) => Some((ScreenId::Readings, sub)),
            Some(ScreenId::Cover | ScreenId::Egg) => Some((ScreenId::Media, 0)),
            Some(s) => Some((s, 0)),
            None => None,
        };
        let n = list.len() as i32;
        let next = match here.and_then(|h| list.iter().position(|e| *e == h)) {
            Some(i) => (i as i32 + delta).rem_euclid(n) as usize,
            None if delta > 0 => 0,
            None => list.len() - 1,
        };
        let (screen, page) = list[next];
        if screen == ScreenId::Readings {
            self.dis.step_readings(now, page as i32 - sub as i32);
        }
        self.picked = screen;
        self.user_page = true;
        tracing::info!(
            "dis: key {k:?} -> {screen:?} {}",
            if screen == ScreenId::Readings {
                format!("page {}", page + 1)
            } else {
                String::new()
            }
        );
        self.dis.set_user_page(now, Some(screen));
    }

    /// Decode a new cover when `media.art_id` changes; drop it when it goes.
    fn update_art(&mut self, store: &Store, now: Instant) {
        let id = store.get_num("media.art_id", now).map(|v| v as u64);
        if id.is_none() {
            self.art = None;
            self.art_tried = None;
            return;
        }
        if id == self.art_tried {
            return;
        }
        self.art_tried = id;
        let decoded = text(store, "media.art_base64", now).and_then(|t| decode_cover(&t));
        match (id, decoded) {
            (Some(id), Some((thumb, full))) => {
                tracing::info!("dis: new cover {id:x}");
                self.art = Some((id, thumb, full));
            }
            (Some(id), None) => {
                tracing::warn!(
                    "dis: cover {id:x} ({:?}) didn't decode",
                    text(store, "media.art_mime", now)
                );
                self.art = None;
            }
            _ => {}
        }
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
                .set_user_page(now, self.user_page.then_some(self.picked));
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
            self.update_art(store, now);
            let m = media_input(store, now, self.art.as_ref());
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
            let ours = self.running && self.dis.session().owned();
            let centre = if ours { self.dis.shown() } else { None };
            let imperial =
                carpi_dis::screens::nav::imperial(&nav_input(store, now), &nav_config(&self.setup));
            let (l1, l2) = top_lines(store, now, centre, ours, imperial).unwrap_or_default();
            self.top_text = match &l2 {
                Some(l2) => format!("{l1}|{l2}"),
                None => l1.clone(),
            };
            self.top.set_lines(now, &l1, l2.as_deref());
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
        match self.setup.pages.get(self.dis.readings().page_index()) {
            Some(page) if !self.setup.pages.is_empty() => {
                for (v, c) in values.iter_mut().zip(&page.cells) {
                    let from = key_unit(&c.key);
                    let to = c.unit.as_deref().unwrap_or(from);
                    *v = store
                        .get_num(&c.key, now)
                        .and_then(|x| convert(x, from, to))
                        .map(|x| x * c.scale);
                }
            }
            _ => {
                for (v, r) in values.iter_mut().zip(&self.setup.readings) {
                    *v = store.get_num(&r.key, now).map(|x| x * r.scale);
                }
            }
        }
        ReadingsInput {
            title: "Readings".into(),
            values,
        }
    }

    /// Store keys the readings page on screen shows (for diag polling).
    /// Empty when the DIS isn't running or readings aren't up.
    pub fn wanted_values(&self) -> Vec<String> {
        if !self.running || self.dis.wanted() != Some(ScreenId::Readings) {
            return Vec::new();
        }
        match self.setup.pages.get(self.dis.readings().page_index()) {
            Some(p) => p.cells.iter().map(|c| c.key.clone()).collect(),
            None => self.setup.readings.iter().map(|r| r.key.clone()).collect(),
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
            Some(Value::Text(match (self.user_page, self.picked) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn store(now: Instant, kv: &[(&str, Value)]) -> Store {
        let mut s = Store::default();
        for (k, v) in kv {
            s.set(k, Some(v.clone()), now, Duration::from_secs(60));
        }
        s
    }

    #[test]
    fn nav_input_fills_eta_lanes_and_units() {
        let now = Instant::now();
        let t = |s: &str| Value::Text(s.into());
        let s = store(
            now,
            &[
                ("nav.active", Value::Bool(true)),
                ("nav.distance_m", Value::Num(400.0)),
                ("nav.distance_unit", t("mi")),
                ("nav.time_to_destination_s", Value::Num(25.0 * 60.0)),
                ("clock.datetime", t("2026-11-08T23:50:10")),
                (
                    "nav.lanes",
                    t(r#"[{"arrows":[{"shape":"straight","highlighted":true}]}]"#),
                ),
            ],
        );
        let n = nav_input(&s, now);
        // CarPlay-style: no ETA text, so car clock + time left, past midnight.
        assert_eq!(n.eta.as_deref(), Some("00:15"));
        assert_eq!(n.distance_unit.as_deref(), Some("mi"));
        assert_eq!(n.lanes.len(), 1);
        assert!(n.lanes[0][0].highlighted);
        // Units follow the phone: the top line reads in feet/miles.
        let imperial = carpi_dis::screens::nav::imperial(&n, &NavConfig::default());
        assert!(imperial);
        let (l1, _) = top_lines(&s, now, None, true, imperial).unwrap();
        assert!(l1.ends_with(" mi") || l1.ends_with(" ft"), "{l1}");
        // AA's ETA text wins over the clock.
        let s2 = store(
            now,
            &[
                ("nav.active", Value::Bool(true)),
                ("nav.eta", t("2:32 PM")),
                ("nav.time_to_destination_s", Value::Num(60.0)),
                ("clock.datetime", t("2026-11-08T14:00:00")),
            ],
        );
        assert_eq!(nav_input(&s2, now).eta.as_deref(), Some("2:32 PM"));
    }
}
