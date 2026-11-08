//! carpid local API.
//!
//! One Unix socket. Each message is a little-endian `u32` length followed by
//! that many bytes of JSON. The client sends `Hello` first; a version
//! mismatch is refused (daemon and LIVI fork ship in lockstep).
//!
//! Mostly request/response, in order. Three kinds of response arrive
//! unsolicited and may come between a request and its reply, so clients
//! dispatch on the type rather than pairing replies blindly:
//! - `Update` for a `Subscribe`;
//! - `Job` for an update or diag job the connection started;
//! - `Command` to the connection that sent `Commands` (the LIVI fork),
//!   answered with `CommandDone`. This is the daemon-to-LIVI direction of
//!   the bridge.
//!
//! Licensed MIT OR Apache-2.0 so the GPL LIVI fork can link it. Keep this
//! crate to message types and framing only.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

/// Largest message either side will accept.
pub const MAX_MESSAGE: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Hello {
        version: u32,
        client: String,
    },
    /// Every value whose name starts with `prefix` ("" for all).
    Values {
        prefix: String,
    },
    Bus,
    /// Write the rolling capture to a file. `None` picks a name in the
    /// daemon's capture directory.
    SaveCapture {
        path: Option<String>,
    },
    /// Save a capture into the logs folder (docs/PAGES.md 3): the last few
    /// minutes of CAN, the journal, values and config, as the next
    /// `<date> #NN`. Answers [`Response::Captured`]; LIVI also gets a
    /// [`Command::Toast`].
    Capture {
        /// `key`, `page`, `portal`, `cli`, ...
        trigger: String,
    },
    /// Stream values whose names start with any of `prefixes`. The daemon
    /// sends `Update { sub }` at most every `period_ms`, and only when
    /// something changed (a value, or valid <-> invalid). The first update
    /// is a full snapshot. Replies `Ok`.
    Subscribe {
        sub: u32,
        prefixes: Vec<String>,
        period_ms: u32,
    },
    Unsubscribe {
        sub: u32,
    },
    /// State pushed by the LIVI fork. Lands in the value store under
    /// `projection.*`, `nav.*`, `media.*`, `call.*`, `phone.*`, `ui.*`, and
    /// is cleared when the pushing connection closes. Replies `Ok`.
    Bridge {
        update: Box<BridgeUpdate>,
    },
    /// Makes this connection the one carpid sends [`Response::Command`] to
    /// (the LIVI fork). The latest connection to ask wins. Replies `Ok`.
    Commands,
    /// Answer to [`Response::Command`] `id`: `error` is `None` when it
    /// worked. Replies `Ok`.
    CommandDone {
        id: u32,
        error: Option<String>,
    },
    /// Starts an update job. Replies [`Response::JobStarted`], then sends
    /// [`Response::Job`] events on this connection until one with
    /// [`JobEvent::Finished`]. One update job at a time.
    Update {
        action: UpdateAction,
        /// With `fetch`: apply the bundle once it's verified and the AP is
        /// back. Without it, `apply` does that later.
        #[serde(default)]
        and_apply: bool,
    },
    /// Starts a diagnostics job. Replies [`Response::JobStarted`], then
    /// [`Response::Job`] events until [`JobEvent::Finished`], like `Update`.
    /// Job numbers are shared with update jobs.
    Diag {
        action: DiagAction,
    },
    /// Daemon settings, typed. `Set` validates, saves `carpid.toml` (old one
    /// backed up) and applies what it can live. Replies [`Response::Settings`].
    Settings {
        action: SettingsAction,
    },
    /// The light model: brightness nudges and "reset learned". Replies `Ok`;
    /// the learned points land in `carpid.toml` a few seconds later.
    Light {
        action: LightAction,
    },
    /// Data logging (docs/PAGES.md 4): profiles, and one run at a time into
    /// the logs folder. Replies [`Response::Datalog`]; starting and stopping
    /// also toast LIVI.
    Datalog {
        action: DatalogAction,
    },
}

/// Rates a data-log profile can run at, Hz.
pub const DATALOG_RATES: [u32; 5] = [1, 2, 5, 10, 20];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DatalogAction {
    /// Profiles, what each costs, and the run.
    Status,
    /// Starts logging `profile`. Refused while a run is going.
    Start {
        profile: String,
    },
    /// Stops the run and saves it.
    Stop,
    /// Adds or replaces a profile. With `replaces`, the profile of that name
    /// is replaced (a rename).
    Save {
        profile: DatalogProfile,
        #[serde(default)]
        replaces: Option<String>,
    },
    Delete {
        name: String,
    },
}

/// A channel set and a rate. carpid keeps these in `datalog.toml` next to
/// its config.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatalogProfile {
    pub name: String,
    /// Store names, in column order.
    pub values: Vec<String>,
    /// One of [`DATALOG_RATES`].
    pub rate_hz: u32,
}

/// What a profile costs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DatalogLoad {
    /// Values the bus or the bridge sends anyway.
    pub bus_values: u32,
    /// Values only diagnostics give, and the measuring-block reads and
    /// modules they take.
    pub diag_values: u32,
    pub diag_groups: u32,
    pub diag_modules: u32,
    /// The rate each diag group can be read at with this set, Hz, at most the
    /// profile's. `None` without diag values.
    pub diag_hz: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DatalogProfileStatus {
    pub profile: DatalogProfile,
    pub load: DatalogLoad,
}

/// One run, going or ended.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DatalogRun {
    /// `2026-11-08 #04`.
    pub label: String,
    pub date: String,
    pub number: u32,
    pub profile: String,
    pub values: u32,
    pub rate_hz: u32,
    /// Local time, `YYYY-MM-DDTHH:MM:SS`.
    pub started: String,
    pub elapsed_ms: u64,
    pub rows: u64,
    /// The entry's folder on the Pi.
    pub dir: String,
    /// Set when it ended: "12 values, 10 Hz, 4 812 rows".
    pub summary: String,
    /// Why it stopped by itself, if it did.
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DatalogStatus {
    pub profiles: Vec<DatalogProfileStatus>,
    /// The profile a key press starts: the last one started.
    pub selected: Option<String>,
    pub running: Option<DatalogRun>,
    /// The last run that ended since carpid started.
    pub last: Option<DatalogRun>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LightAction {
    /// Brighter (+) or darker (-) by this many panel % at the current
    /// ambient. Learned mode stores it as a learned point.
    Nudge { delta_pct: i8 },
    /// Forget the learned points of `source` (`None`: the current one).
    ResetLearned {
        #[serde(default)]
        source: Option<AmbientSource>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiagAction {
    /// Gateway installation list (`1A 9F`).
    InstallList,
    /// Fault codes of one module (TP2 logical ID, e.g. 1 = engine).
    ReadDtcs { module: u8 },
    /// Clear fault codes, then read what's left. Takes the exclusive diag
    /// token, so it only runs with `confirm: true`.
    ClearDtcs {
        module: u8,
        #[serde(default)]
        confirm: bool,
    },
    /// Mode, channels and subscriptions, as one [`JobEvent::DiagStatus`].
    Status,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SettingsAction {
    Get,
    /// Boxed: a patch is large and rare.
    Set {
        patch: Box<SettingsPatch>,
    },
}

/// Everything settable through [`Request::Settings`]. The portal PIN is
/// never returned, only whether one is set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// `[tx] enabled`: frames the gate passes are sent, not just logged.
    pub tx_enabled: bool,
    /// `[power] shutdown`: act on shutdown requests.
    pub power_shutdown: bool,
    pub light: LightSettings,
    pub dis: DisSettings,
    /// The key binding table, every row. See [`Binding`].
    pub bindings: Vec<Binding>,
    pub portal_pin_set: bool,
}

/// Light model (`[light]`, PLAN 2.6). `ambient` is the model's log-scale
/// estimate; the page plots the curve against the live `light.ambient` and
/// `light.panel_pct` values.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LightSettings {
    /// Off: `BB 00` once to hand the panel back to the RNS-E, then nothing.
    pub auto_brightness: bool,
    /// The car's dimming output shifts the curve.
    pub scroll_trim: bool,
    /// Where `ambient` comes from.
    pub ambient_source: AmbientSource,
    pub curve_mode: CurveMode,
    /// Ambient -> panel %, ascending ambient.
    pub manual_curve: Vec<CurvePoint>,
    /// Points learned from manual nudges, per ambient source (a new source
    /// learns afresh; the old points stay). carpid owns these; only
    /// [`LightPatch::reset_learned`] changes them over the API.
    pub learned: Vec<LearnedPoint>,
    /// What decides night (dark theme).
    #[serde(default)]
    pub dark_mode_source: DarkModeSource,
    /// Ambient dark mode: night below this ambient...
    pub night_threshold: f64,
    /// ...and day again above threshold + hysteresis...
    pub night_hysteresis: f64,
    /// ...each after holding this long, seconds.
    #[serde(default)]
    pub night_dwell_s: f64,
    /// Rough location for sunset/sunrise while the ignition is off. Both or
    /// neither.
    #[serde(default)]
    pub latitude: Option<f64>,
    #[serde(default)]
    pub longitude: Option<f64>,
    /// The car clock's offset from UTC. `None`: from the longitude.
    #[serde(default)]
    pub utc_offset_hours: Option<f64>,
    /// Without a location: night from this local time ("19:00")...
    #[serde(default)]
    pub night_from: String,
    /// ...until this one ("07:00").
    #[serde(default)]
    pub day_from: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DarkModeSource {
    /// The car's lights while Kl15 is on, the car clock while it's off.
    #[default]
    Headlights,
    /// The brightness model's ambient estimate; the clock while it's stale.
    Ambient,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AmbientSource {
    /// 0x621 `KO1_Lichtsensor`: in a dial, sees light most like the RNS-E.
    Cluster,
    /// 0x3E3 sun sensor on the dash top: open sky, reads brighter.
    Climatronic,
    /// Cluster first; climatronic fills in the bright end if the cluster
    /// sensor saturates.
    Blend,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LearnedPoint {
    pub source: AmbientSource,
    pub ambient: f64,
    pub pct: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CurveMode {
    Manual,
    Learned,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CurvePoint {
    pub ambient: f64,
    /// RNS-E panel level, 0-100.
    pub pct: u8,
}

/// Fields left out stay as they are.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LightPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_brightness: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scroll_trim: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ambient_source: Option<AmbientSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curve_mode: Option<CurveMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_curve: Option<Vec<CurvePoint>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub night_threshold: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub night_hysteresis: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dark_mode_source: Option<DarkModeSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub night_dwell_s: Option<f64>,
    /// Sets the location; `clear_location` removes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latitude: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub longitude: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utc_offset_hours: Option<f64>,
    /// Removes latitude, longitude and UTC offset.
    #[serde(default)]
    pub clear_location: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub night_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day_from: Option<String>,
    /// Forget the learned points of the (new) ambient source.
    #[serde(default)]
    pub reset_learned: bool,
}

/// Cluster display (`[dis]`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DisSettings {
    pub enabled: bool,
    /// The readings page, up to 8 cells: left column top to bottom, then right.
    /// Used when `pages` is empty.
    pub readings: Vec<ReadingSetting>,
    /// Named readings pages (the wheel steps through them).
    #[serde(default)]
    pub pages: Vec<DisPageSetting>,
    /// When we hold the centre with nothing else to show.
    #[serde(default)]
    pub claim: DisClaim,
    /// The page "always" shows, and the cluster-menu tab opens to.
    #[serde(default)]
    pub default_page: DisPage,
    /// Per source: take the centre when it gets content (`claim_*`), give
    /// it back when the content goes (`release_*`; off: the screen stays up).
    #[serde(default = "yes")]
    pub claim_nav: bool,
    #[serde(default = "yes")]
    pub release_nav: bool,
    #[serde(default = "yes")]
    pub claim_phone: bool,
    #[serde(default = "yes")]
    pub release_phone: bool,
    #[serde(default = "yes")]
    pub claim_media: bool,
    #[serde(default = "yes")]
    pub release_media: bool,
    #[serde(default)]
    pub media_layout: DisMediaLayout,
    /// Full cover on a new track, held this long once drawn; 0 = off.
    #[serde(default = "four")]
    pub cover_flash_s: f64,
    #[serde(default)]
    pub units: DisUnits,
    #[serde(default)]
    pub clock: DisClock,
    #[serde(default)]
    pub nav_style: DisNavStyle,
}

/// Nav screen look.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisNavStyle {
    /// Our icons, pocket and lanes.
    #[default]
    Fancy,
    /// The RNS-E's own look (ROM glyph arrows).
    Stock,
}

/// A named DIS readings page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DisPageSetting {
    pub name: String,
    /// Up to 8: left column top to bottom, then right.
    pub cells: Vec<DisCellSetting>,
}

/// A readings cell: any store or diag catalog value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DisCellSetting {
    pub key: String,
    #[serde(default)]
    pub decimals: u8,
    #[serde(default = "one")]
    pub scale: f64,
    /// Show in this unit (`°F`, `mph`, `psi`, ...); `None`: the key's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Icon name; `None`: picked from the key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// DIS distance units. `Auto`: as the phone shows them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisUnits {
    #[default]
    Auto,
    Metric,
    Imperial,
}

/// DIS ETA clock. `Auto`: 12 h if the phone's ETA has AM/PM.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DisClock {
    #[default]
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "12h")]
    H12,
    #[serde(rename = "24h")]
    H24,
}

fn yes() -> bool {
    true
}

fn four() -> f64 {
    4.0
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisClaim {
    /// Our default page holds the centre whenever nothing else wants it.
    #[default]
    Always,
    /// Idle as a cluster-menu tab until the driver picks it.
    Tab,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisPage {
    #[default]
    Readings,
    Accel,
    Media,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisMediaLayout {
    /// Title, artist, album, bar, times.
    #[default]
    Stack,
    /// A small cover beside the text.
    Art,
}

/// `[dis]` fields a patch can change; left out = unchanged.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DisPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim: Option<DisClaim>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_page: Option<DisPage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_nav: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_nav: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_phone: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_phone: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_media: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_media: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_layout: Option<DisMediaLayout>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_flash_s: Option<f64>,
    /// Replaces all pages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pages: Option<Vec<DisPageSetting>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub units: Option<DisUnits>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock: Option<DisClock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nav_style: Option<DisNavStyle>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReadingSetting {
    /// Store name, e.g. `engine.coolant_c`.
    pub key: String,
    /// About 3 characters fit.
    pub label: String,
    #[serde(default)]
    pub decimals: u8,
    /// Shown value = value x scale (e.g. 0.001 for km -> 1000 km).
    #[serde(default = "one")]
    pub scale: f64,
}

fn one() -> f64 {
    1.0
}

/// One key binding: in `table`, `key` + `gesture` does `action`.
///
/// Tables, in the order the router checks them: `dis_toggle` (one row,
/// action `toggle`), `system`, `call_ringing`, `call_active`, `dis`, `car`,
/// `livi`. Names are snake_case (`wheel_up`, `knob_cw`, `long`,
/// `play_pause`, `open_page_data`, `code:30` for a raw Linux key code).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub table: String,
    pub key: String,
    pub gesture: String,
    pub action: String,
}

/// Fields left out stay as they are.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SettingsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_shutdown: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub light: Option<LightPatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dis_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dis_readings: Option<Vec<ReadingSetting>>,
    /// The rest of `[dis]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dis: Option<DisPatch>,
    /// Replaces the whole table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bindings: Option<Vec<Binding>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub portal_pin: Option<PinChange>,
}

/// The PIN never shows in `Debug` output, so it can't reach a log.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PinChange {
    Set { pin: String },
    Clear,
}

impl std::fmt::Debug for PinChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PinChange::Set { .. } => f.write_str("Set { pin: <hidden> }"),
            PinChange::Clear => f.write_str("Clear"),
        }
    }
}

/// What an update job does. `check` and `fetch` need the internet: the AP
/// is suspended while they run (see docs/DEPLOY.md, section 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateAction {
    /// List releases on the configured channel.
    Check,
    /// Download and verify the newest release on the channel.
    Fetch,
    /// Install the downloaded bundle (offline).
    Apply,
    /// What's installed and downloaded (offline).
    Status,
}

/// Something carpid asks the command connection (LIVI) to do.
///
/// The server-to-client direction: carpid sends `Response::Command { id,
/// command }` to the connection that sent [`Request::Commands`], unsolicited
/// and interleaved with other responses. The client does it and answers
/// [`Request::CommandDone`] with the same `id`. carpid gives up waiting
/// after a timeout of its own choosing; the client must still finish or undo
/// safely.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Command {
    /// Hotspot: `suspend` hands the Wi-Fi interface to NetworkManager,
    /// `resume` restores whatever was running before `suspend`. The client
    /// resumes by itself when carpid's connection drops or after
    /// [`AP_SUSPEND_MAX_S`].
    Ap { ap: ApAction },
    /// A projection-neutral input action from the car's keys. The fork maps
    /// it to Android Auto keycodes or CarPlay commands for the active session
    /// (PLAN 2.4). Sent only while the projection has focus.
    Input { action: InputAction },
    /// A short message over whatever is on screen ("Capture 2026-11-08 #03").
    Toast {
        text: String,
        #[serde(default)]
        level: ToastLevel,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToastLevel {
    #[default]
    Info,
    Warn,
    Error,
}

/// Actions the fork maps per session (AA keycode / CarPlay command).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum InputAction {
    Up,
    Down,
    Left,
    Right,
    /// Rotary step; positive is clockwise.
    Rotate {
        steps: i32,
    },
    /// Select press/release.
    Select {
        down: bool,
    },
    Back,
    Home,
    PlayPause,
    Next,
    Prev,
    CallAccept,
    CallEnd,
    /// Voice assistant hold (PTT): down starts, up releases.
    Voice {
        down: bool,
    },
    /// AA Navigation keycode / CarPlay `maps:`.
    OpenNav,
    /// AA Tel / CarPlay `mobilephone:`.
    OpenPhone,
    /// AA Media / CarPlay `nowplaying:`.
    OpenMedia,
    /// One of our pages in the fork (`data`, `diagnostics`, `car`, `light`).
    OpenPage {
        page: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApAction {
    Suspend,
    Resume,
}

/// Longest the AP may stay suspended. carpid resumes after 3 minutes; the
/// client resumes on its own after this, in case carpid can't.
pub const AP_SUSPEND_MAX_S: u64 = 240;

/// One step of an update job. `carpi-update` prints these as JSON lines on
/// stdout; carpid relays them as [`Response::Job`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum JobEvent {
    /// A new stage started: `ap`, `network`, `check`, `download`,
    /// `verify`, `unpack`, `profile`, `switch`, `restart`, `health`,
    /// `rollback`, `cleanup`, `status`.
    Stage { stage: String, message: String },
    /// Bytes or items done in the current stage.
    Progress {
        stage: String,
        done: u64,
        total: Option<u64>,
    },
    /// Releases on the channel, newest first.
    Releases { items: Vec<ReleaseInfo> },
    /// What's on disk.
    Installed(Installed),
    /// Diag: the gateway's installation list.
    InstallList { modules: Vec<InstalledModule> },
    /// Diag: fault codes read from `module`. After a clear, what's left.
    Dtcs { module: u8, dtcs: Vec<Dtc> },
    /// Diag: mode, channels and subscriptions.
    DiagStatus(DiagStatus),
    /// Last event of a job.
    Finished { ok: bool, message: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledModule {
    /// Workshop (VCDS) address.
    pub vcds: u8,
    /// TP2 logical ID: what `ReadDtcs { module }` takes.
    pub tp2: u8,
    pub coded: bool,
    pub status: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dtc {
    /// VAG code, shown as 5 decimal digits.
    pub code: u16,
    /// Raw status byte; meaning unknown.
    pub status: u8,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DiagStatus {
    /// `normal`, `exclusive`, `vcds_pause` or `bus_lease`.
    pub mode: String,
    /// Paused because another tester (VCDS) was seen.
    pub vcds_auto_pause: bool,
    pub channels: Vec<DiagChannel>,
    pub subscriptions: Vec<DiagSubscription>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DiagChannel {
    pub module: u8,
    /// `closed`, `opening`, `open`, `grace`, `failed`.
    pub state: String,
    pub failures: u32,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DiagSubscription {
    pub value: String,
    /// `ican`, `diag:<module>:<group>` or `unavailable`.
    pub source: String,
    pub requested_ms: u64,
    pub granted_ms: Option<u64>,
    pub achieved_hz: Option<f64>,
    /// Why it's below what was asked, if it is.
    pub degraded: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReleaseInfo {
    pub version: String,
    /// `stable` or `beta`.
    pub channel: String,
    pub published: String,
    /// Bundle size in bytes.
    pub size: u64,
    /// Same version as the one running.
    pub installed: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Installed {
    pub current: Option<String>,
    pub previous: Option<String>,
    /// Downloaded and verified, not applied yet.
    pub downloaded: Option<String>,
    /// How the last apply ended, if one ran: `ok`, `rolled_back`, `failed`.
    pub last_apply: Option<String>,
    pub os_profile: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Hello {
        version: u32,
        daemon: String,
    },
    Ok,
    Values {
        items: Vec<Reading>,
    },
    /// Unsolicited, for a `Subscribe`. Only changed readings after the first.
    Update {
        sub: u32,
        items: Vec<Reading>,
    },
    Bus {
        buses: Vec<BusInfo>,
    },
    CaptureSaved {
        path: String,
        frames: usize,
    },
    /// Reply to `Capture`.
    Captured {
        /// `2026-11-08`.
        date: String,
        number: u32,
        /// `2026-11-08 #03`.
        label: String,
        /// The entry's folder on the Pi.
        dir: String,
        /// One line: what went in.
        summary: String,
    },
    Error {
        message: String,
    },
    /// Unsolicited, to the command connection. See [`Command`].
    Command {
        id: u32,
        command: Command,
    },
    /// Reply to `Update`: the job's events carry this number.
    JobStarted {
        job: u32,
    },
    /// Unsolicited, for a job this connection started.
    Job {
        job: u32,
        event: JobEvent,
    },
    /// Reply to `Settings`, after a `Set` too. `restart_needed` names
    /// settings that were saved but only take effect after a restart.
    Settings {
        settings: Settings,
        restart_needed: Vec<String>,
    },
    /// Reply to `Datalog`.
    Datalog {
        status: Box<DatalogStatus>,
    },
}

/// What LIVI knows that the car needs: projection, nav, media, calls.
/// Each variant replaces that whole section; `None` clears it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
// A few updates a second, each moved once: boxing `Nav` would buy nothing.
#[allow(clippy::large_enum_variant)]
pub enum BridgeUpdate {
    Projection {
        projection: Option<Projection>,
    },
    Nav {
        nav: Option<NavState>,
    },
    /// Turn icon as sent by the phone (AA asks for 256x256 PNG), base64.
    NavIcon {
        png_base64: Option<String>,
    },
    Media {
        media: Option<MediaState>,
    },
    Call {
        call: Option<CallState>,
    },
    Phone {
        phone: Option<PhoneState>,
    },
    /// What has keyboard focus in LIVI: `"projection"`, `"livi"` (its own
    /// menus) or `"page:<name>"` for one of our pages.
    Focus {
        focus: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Projection {
    /// `"android_auto"` or `"carplay"`.
    pub kind: String,
    pub wireless: bool,
}

/// Route guidance as the phone sends it, the same shape for Android Auto and
/// CarPlay. A field is `None` when the active protocol doesn't carry it.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NavState {
    /// Maneuver type in AA's vocabulary, lower snake case
    /// (`turn`, `slight_turn`, `roundabout_enter_and_exit`, `destination`, ...).
    pub maneuver: Option<String>,
    /// `"left"`, `"right"`, or none.
    pub side: Option<String>,
    /// Roundabout exit number (1 = first) and exit angle where the phone gives them.
    pub roundabout_exit: Option<u32>,
    pub roundabout_angle: Option<i32>,
    /// To the next maneuver.
    pub distance_m: Option<f64>,
    /// Distance to the maneuver as the phone shows it ("350 m", "0.2 mi");
    /// LIVI's own rendering of `distance_m` when the phone sends none.
    pub distance_text: Option<String>,
    /// Unit the phone shows the distance in: `m`, `km`, `mi`, `ft`, `yd`.
    pub distance_unit: Option<String>,
    /// Android Auto only.
    pub time_to_maneuver_s: Option<f64>,
    pub road: Option<String>,
    pub cue: Option<String>,
    pub destination: Option<String>,
    pub destination_distance_m: Option<f64>,
    /// As for `distance_text` / `distance_unit`.
    pub destination_distance_text: Option<String>,
    pub destination_distance_unit: Option<String>,
    pub time_to_destination_s: Option<f64>,
    /// ETA as the phone formats it (Android Auto).
    pub eta: Option<String>,
    /// ETA as Unix seconds (CarPlay).
    pub eta_unix_s: Option<i64>,
    /// Left to right. Empty when the phone sends no lanes.
    pub lanes: Vec<Lane>,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Lane {
    /// The lane's arrows as painted on the road.
    pub arrows: Vec<LaneArrow>,
    /// CarPlay's verdict on the lane: `not_good`, `good`, `preferred`.
    pub status: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LaneArrow {
    /// AA lane shape (`straight`, `slight_left`, `normal_right`, `uturn_left`, ...).
    pub shape: String,
    /// The phone highlights the arrows to follow; AA may highlight several.
    pub highlighted: bool,
}

impl Lane {
    /// Any arrow of this lane is one to follow.
    pub fn highlighted(&self) -> bool {
        self.arrows.iter().any(|a| a.highlighted)
    }
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct MediaState {
    pub source: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub playing: bool,
    pub position_ms: Option<u64>,
    pub duration_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct CallState {
    /// `"ringing"`, `"dialing"`, `"active"`, `"held"`.
    pub state: String,
    pub number: Option<String>,
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct PhoneState {
    pub battery_pct: Option<u8>,
    pub signal_pct: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Bool(bool),
    Num(f64),
    Text(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reading {
    pub name: String,
    /// `None` when the source sent an error/init value or it timed out.
    pub value: Option<Value>,
    pub age_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BusInfo {
    /// Role name from the config, e.g. `ican`, `diag`.
    pub role: String,
    pub iface: String,
    /// `silent`, `waking`, `awake`, `sleep-pending`; `None` for buses
    /// without an NM ring the Pi can see.
    pub state: Option<String>,
    pub frames: u64,
    pub ring: Vec<RingMember>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RingMember {
    pub node: u8,
    pub successor: u8,
    pub sleep_ind: bool,
    pub sleep_ack: bool,
    pub age_ms: u64,
}

/// Length-prefixed JSON.
pub fn encode<T: Serialize>(msg: &T) -> Vec<u8> {
    let body = serde_json::to_vec(msg).expect("API types always serialize");
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// Reads the length prefix. Errors if it's over [`MAX_MESSAGE`].
pub fn body_len(prefix: [u8; 4]) -> Result<usize, String> {
    let n = u32::from_le_bytes(prefix) as usize;
    if n > MAX_MESSAGE {
        return Err(format!(
            "message of {n} bytes is over the {MAX_MESSAGE} byte limit"
        ));
    }
    Ok(n)
}

pub fn decode<'a, T: Deserialize<'a>>(body: &'a [u8]) -> Result<T, serde_json::Error> {
    serde_json::from_slice(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let r = Request::Values {
            prefix: "light.".into(),
        };
        let bytes = encode(&r);
        let n = body_len(bytes[..4].try_into().unwrap()).unwrap();
        assert_eq!(n, bytes.len() - 4);
        assert_eq!(decode::<Request>(&bytes[4..]).unwrap(), r);
    }

    #[test]
    fn light_request_shape() {
        let r: Request =
            serde_json::from_str(r#"{"type":"light","action":{"kind":"nudge","delta_pct":-5}}"#)
                .unwrap();
        assert_eq!(
            r,
            Request::Light {
                action: LightAction::Nudge { delta_pct: -5 }
            }
        );
        let r: Request =
            serde_json::from_str(r#"{"type":"light","action":{"kind":"reset_learned"}}"#).unwrap();
        assert_eq!(
            r,
            Request::Light {
                action: LightAction::ResetLearned { source: None }
            }
        );
    }

    #[test]
    fn wire_shape() {
        let r = Response::Values {
            items: vec![Reading {
                name: "vehicle.speed_kmh".into(),
                value: Some(Value::Num(0.0)),
                age_ms: 12,
            }],
        };
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(
            json,
            r#"{"type":"values","items":[{"name":"vehicle.speed_kmh","value":0.0,"age_ms":12}]}"#
        );
    }

    #[test]
    fn command_wire_shape() {
        let c = Response::Command {
            id: 7,
            command: Command::Ap {
                ap: ApAction::Suspend,
            },
        };
        assert_eq!(
            serde_json::to_string(&c).unwrap(),
            r#"{"type":"command","id":7,"command":{"kind":"ap","ap":"suspend"}}"#
        );
        let u: Request = decode(br#"{"type":"update","action":"fetch"}"#).unwrap();
        assert_eq!(
            u,
            Request::Update {
                action: UpdateAction::Fetch,
                and_apply: false
            }
        );
        let e = JobEvent::Finished {
            ok: true,
            message: "done".into(),
        };
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            r#"{"event":"finished","ok":true,"message":"done"}"#
        );
        let i = JobEvent::Installed(Installed::default());
        let back: JobEvent = serde_json::from_str(&serde_json::to_string(&i).unwrap()).unwrap();
        assert_eq!(back, i);
    }

    #[test]
    fn diag_and_settings_wire_shape() {
        let r: Request =
            decode(br#"{"type":"diag","action":{"kind":"clear_dtcs","module":1,"confirm":true}}"#)
                .unwrap();
        assert_eq!(
            r,
            Request::Diag {
                action: DiagAction::ClearDtcs {
                    module: 1,
                    confirm: true
                }
            }
        );
        let r: Request =
            decode(br#"{"type":"diag","action":{"kind":"clear_dtcs","module":1}}"#).unwrap();
        assert!(matches!(
            r,
            Request::Diag {
                action: DiagAction::ClearDtcs { confirm: false, .. }
            }
        ));
        let r: Request = decode(
            br#"{"type":"settings","action":{"kind":"set","patch":{"tx_enabled":true,"portal_pin":{"kind":"set","pin":"4711"}}}}"#,
        )
        .unwrap();
        let Request::Settings {
            action: SettingsAction::Set { patch },
        } = &r
        else {
            panic!("{r:?}")
        };
        assert_eq!(patch.tx_enabled, Some(true));
        assert!(!format!("{r:?}").contains("4711"));
        let g: Request = decode(br#"{"type":"settings","action":{"kind":"get"}}"#).unwrap();
        assert_eq!(
            g,
            Request::Settings {
                action: SettingsAction::Get
            }
        );
    }

    #[test]
    fn datalog_wire_shape() {
        let r: Request =
            decode(br#"{"type":"datalog","action":{"kind":"start","profile":"Boost"}}"#).unwrap();
        assert_eq!(
            r,
            Request::Datalog {
                action: DatalogAction::Start {
                    profile: "Boost".into()
                }
            }
        );
        let r: Request = decode(
            br#"{"type":"datalog","action":{"kind":"save","profile":{"name":"Boost","values":["engine.rpm"],"rate_hz":10}}}"#,
        )
        .unwrap();
        assert!(matches!(
            r,
            Request::Datalog {
                action: DatalogAction::Save { replaces: None, .. }
            }
        ));
        let s = Response::Datalog {
            status: Box::default(),
        };
        assert_eq!(
            serde_json::to_string(&s).unwrap(),
            r#"{"type":"datalog","status":{"profiles":[],"selected":null,"running":null,"last":null}}"#
        );
    }

    #[test]
    fn rejects_huge() {
        assert!(body_len((MAX_MESSAGE as u32 + 1).to_le_bytes()).is_err());
    }
}
