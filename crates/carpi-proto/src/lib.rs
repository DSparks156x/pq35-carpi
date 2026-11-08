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
    Set { patch: SettingsPatch },
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

/// Light model thresholds (`[light]`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LightSettings {
    /// Lights on this long before night.
    pub night_after_s: f64,
    /// Lights off this long before day.
    pub day_after_s: f64,
    /// RNS-E panel level by day: 0 follows the car's dimmer, 1-100 fixed.
    pub day_panel: u8,
    pub night_panel: u8,
}

/// Cluster display (`[dis]`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DisSettings {
    pub enabled: bool,
    /// The readings page, up to 8 cells: left column top to bottom, then right.
    pub readings: Vec<ReadingSetting>,
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
    pub light: Option<LightSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dis_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dis_readings: Option<Vec<ReadingSetting>>,
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
}

/// What LIVI knows that the car needs: projection, nav, media, calls.
/// Each variant replaces that whole section; `None` clears it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
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

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct NavState {
    /// Maneuver type in AA's vocabulary, lower snake case
    /// (`turn`, `slight_turn`, `roundabout_enter_and_exit`, `destination`, ...).
    pub maneuver: Option<String>,
    /// `"left"`, `"right"`, or none.
    pub side: Option<String>,
    /// Roundabout exit number or turn angle where AA gives one.
    pub roundabout_exit: Option<u32>,
    pub roundabout_angle: Option<i32>,
    pub distance_m: Option<f64>,
    /// Distance as the phone formats it for display ("350 m", "0.2 mi").
    pub distance_text: Option<String>,
    pub time_to_maneuver_s: Option<f64>,
    pub road: Option<String>,
    pub cue: Option<String>,
    pub destination: Option<String>,
    pub eta: Option<String>,
    /// Left to right. Empty when the phone sends no lanes.
    pub lanes: Vec<Lane>,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Lane {
    /// AA lane shapes (`straight`, `slight_left`, `uturn_right`, ...).
    pub shapes: Vec<String>,
    pub highlighted: bool,
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
    fn rejects_huge() {
        assert!(body_len((MAX_MESSAGE as u32 + 1).to_le_bytes()).is_err());
    }
}
