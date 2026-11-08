use carpi_core::{CoreConfig, DisSetup, Reading};
use carpi_input::{BindingRow, RouteConfig};
use carpi_proto::{AmbientSource, CurveMode, CurvePoint, DarkModeSource, LearnedPoint};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// `/etc/carpi/carpid.toml` (or `--config`).
///
/// ```toml
/// [bus.ican]
/// iface = "ican"       # any netdev name; name interfaces with .link files
///
/// [bus.diag]
/// iface = "ican"       # same interface on the TT; "obd" where the gateway won't route diag
///
/// [api]
/// socket = "/run/user/1000/carpid.sock"
///
/// [recorder]
/// minutes = 10         # RAM ring; at least [logs] capture_minutes
///
/// [logs]               # captures (docs/PAGES.md 3)
/// dir = "~/logs"       # <dir>/<date>/<NN>/
/// capture_minutes = 5  # how far back a capture goes
/// max_pct = 25         # oldest unstarred entries go above this % of the disk
///
/// [clock]
/// set_from_car = true  # set the Pi clock from the car's (0x65D) when off by > 2 min
///
/// [tx]
/// enabled = false      # off: gate-approved frames are logged, not sent
///
/// [power]
/// shutdown = false     # off: a shutdown request is logged, not acted on
///
/// [input]
/// uinput = true        # keys to LIVI through a virtual keyboard
/// # bindings = [ { table = "livi", key = "wheel_up", gesture = "short", action = "prev" }, ... ]
///
/// [light]              # see deploy/carpid.toml and docs/PLAN.md 2.6
/// auto_brightness = true
/// dark_mode_source = "headlights"   # or "ambient"
/// # latitude = 52.5    # rough location for sunset with the ignition off
/// # longitude = 13.4
/// # utc_offset_hours = 1.0
/// night_from = "19:00" # without a location
/// day_from = "07:00"
///
/// [dis]
/// enabled = false      # off: nothing is sent to the cluster
/// # readings = [ { key = "engine.rpm", label = "RPM", decimals = 0, scale = 1.0 }, ... ]
///
/// [portal]             # phone portal; absent = off. Keys: carpi_portal::PortalConfig
/// bind = "0.0.0.0:5003"
///
/// [update]             # read by carpi-update too; see docs/DEPLOY.md
/// repo = "DSparks156x/pq35-carpi"
/// channel = "stable"   # or "beta"
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub bus: BTreeMap<String, Bus>,
    #[serde(default)]
    pub api: Api,
    #[serde(default)]
    pub recorder: RecorderCfg,
    #[serde(default)]
    pub logs: LogsCfg,
    #[serde(default)]
    pub clock: ClockCfg,
    #[serde(default)]
    pub tx: Tx,
    #[serde(default)]
    pub power: Power,
    #[serde(default)]
    pub input: Input,
    #[serde(default)]
    pub update: Update,
    #[serde(default)]
    pub light: Light,
    #[serde(default)]
    pub dis: Dis,
    /// Absent: no portal. `config_file` is always the file carpid loaded.
    pub portal: Option<carpi_portal::PortalConfig>,
}

/// `[light]`: the light model (PLAN 2.6); see `carpi_light`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Light {
    pub auto_brightness: bool,
    pub scroll_trim: bool,
    pub ambient_source: AmbientSource,
    pub curve_mode: CurveMode,
    pub manual_curve: Vec<CurvePoint>,
    /// carpid-owned: written by the model, cleared by "reset learned".
    pub learned: Vec<LearnedPoint>,
    pub dark_mode_source: DarkModeSource,
    pub night_threshold: f64,
    pub night_hysteresis: f64,
    pub night_dwell_s: f64,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub utc_offset_hours: Option<f64>,
    /// "HH:MM", local car clock.
    pub night_from: String,
    pub day_from: String,
}

/// "HH:MM" -> minute of the day.
pub fn parse_hhmm(s: &str) -> Option<u16> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (u16, u16) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

fn hhmm(min: u16) -> String {
    format!("{:02}:{:02}", min / 60, min % 60)
}

impl Default for Light {
    fn default() -> Self {
        let d = carpi_core::LightSetup::default();
        Light {
            auto_brightness: d.auto_brightness,
            scroll_trim: d.scroll_trim,
            ambient_source: to_proto_source(d.ambient_source),
            curve_mode: match d.curve_mode {
                carpi_core::CurveMode::Manual => CurveMode::Manual,
                carpi_core::CurveMode::Learned => CurveMode::Learned,
            },
            manual_curve: d
                .manual_curve
                .iter()
                .map(|&(ambient, pct)| CurvePoint { ambient, pct })
                .collect(),
            learned: Vec::new(),
            dark_mode_source: DarkModeSource::Headlights,
            night_threshold: d.dark.threshold,
            night_hysteresis: d.dark.hysteresis,
            night_dwell_s: d.dark.dwell.as_secs_f64(),
            latitude: None,
            longitude: None,
            utc_offset_hours: None,
            night_from: hhmm(d.dark.night_from),
            day_from: hhmm(d.dark.day_from),
        }
    }
}

pub fn to_proto_source(s: carpi_core::AmbientSource) -> AmbientSource {
    match s {
        carpi_core::AmbientSource::Cluster => AmbientSource::Cluster,
        carpi_core::AmbientSource::Climatronic => AmbientSource::Climatronic,
        carpi_core::AmbientSource::Blend => AmbientSource::Blend,
    }
}

fn to_core_source(s: AmbientSource) -> carpi_core::AmbientSource {
    match s {
        AmbientSource::Cluster => carpi_core::AmbientSource::Cluster,
        AmbientSource::Climatronic => carpi_core::AmbientSource::Climatronic,
        AmbientSource::Blend => carpi_core::AmbientSource::Blend,
    }
}

fn check_curve(name: &str, pts: &[(f64, u8)], max: usize) -> Result<(), String> {
    if pts.len() > max {
        return Err(format!("[light] {name}: at most {max} points"));
    }
    for &(a, p) in pts {
        if !a.is_finite() || p > 100 {
            return Err(format!(
                "[light] {name}: ambient must be a number, pct 0-100"
            ));
        }
    }
    Ok(())
}

/// `[dis]`: the cluster display.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dis {
    #[serde(default)]
    pub enabled: bool,
    /// Absent: the default 2x4 set of ICAN values.
    pub readings: Option<Vec<ReadingCfg>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadingCfg {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub decimals: u8,
    #[serde(default = "one")]
    pub scale: f64,
}

fn one() -> f64 {
    1.0
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingCfg {
    pub table: String,
    pub key: String,
    pub gesture: String,
    pub action: String,
}

/// `[update]`. carpi-update reads the same section; carpid only needs to
/// know where carpi-update is, but checks every key so typos fail here.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)] // carpi-update's keys: only checked here
pub struct Update {
    #[serde(default = "default_updater")]
    pub updater: PathBuf,
    pub repo: Option<String>,
    pub channel: Option<String>,
    pub pubkey: Option<PathBuf>,
    pub restart_user: Option<Vec<String>>,
    pub restart_system: Option<Vec<String>>,
    pub check_frames: Option<bool>,
}

impl Default for Update {
    fn default() -> Self {
        Update {
            updater: default_updater(),
            repo: None,
            channel: None,
            pubkey: None,
            restart_user: None,
            restart_system: None,
            check_frames: None,
        }
    }
}

fn default_updater() -> PathBuf {
    "/opt/carpi/current/bin/carpi-update".into()
}

/// Off by default: the first runs on the car log what they would send.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tx {
    #[serde(default)]
    pub enabled: bool,
}

/// Off by default: the first runs on the car log the shutdown request.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Power {
    #[serde(default)]
    pub shutdown: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    #[serde(default = "yes")]
    pub uinput: bool,
    /// The whole binding table. Absent: the built-in one.
    pub bindings: Option<Vec<BindingCfg>>,
}

impl Default for Input {
    fn default() -> Self {
        Input {
            uinput: true,
            bindings: None,
        }
    }
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bus {
    pub iface: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Api {
    #[serde(default = "default_socket")]
    pub socket: PathBuf,
}

impl Default for Api {
    fn default() -> Self {
        Api {
            socket: default_socket(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecorderCfg {
    #[serde(default = "default_minutes")]
    pub minutes: u64,
    /// Old key, ignored: captures go to `[logs] dir`. Still parses so old
    /// files load; carpid warns once.
    pub dir: Option<PathBuf>,
}

impl Default for RecorderCfg {
    fn default() -> Self {
        RecorderCfg {
            minutes: default_minutes(),
            dir: None,
        }
    }
}

/// `[logs]`: the logs folder and what a capture takes.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LogsCfg {
    /// `~/` is the home folder.
    pub dir: PathBuf,
    pub capture_minutes: u64,
    /// Logs use at most this much of the filesystem they're on.
    pub max_pct: u8,
}

impl Default for LogsCfg {
    fn default() -> Self {
        LogsCfg {
            dir: "~/logs".into(),
            capture_minutes: 5,
            max_pct: 25,
        }
    }
}

impl LogsCfg {
    /// `dir` with `~/` expanded.
    pub fn root(&self) -> PathBuf {
        let home = || {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir)
        };
        match self.dir.to_str() {
            Some("~") => home(),
            Some(s) if s.starts_with("~/") => home().join(&s[2..]),
            _ => self.dir.clone(),
        }
    }
}

/// `[clock]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ClockCfg {
    /// Set the Pi clock from the car's when they differ by over 2 minutes
    /// and network time hasn't synced.
    pub set_from_car: bool,
}

impl Default for ClockCfg {
    fn default() -> Self {
        ClockCfg { set_from_car: true }
    }
}

/// `$XDG_RUNTIME_DIR/carpid.sock`, so LIVI (same user) can reach it.
pub fn default_socket() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("carpid.sock")
}

fn default_minutes() -> u64 {
    10
}

/// Where `SaveCapture` without a path writes (the raw ring dump).
pub fn raw_capture_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("captures")
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Self::parse(&text).map_err(anyhow::Error::msg)
    }

    /// The loader's rules, on text: the portal's upload check and settings
    /// writes use this too.
    pub fn parse(text: &str) -> Result<Self, String> {
        let cfg: Config = toml::from_str(text).map_err(|e| e.to_string())?;
        if !cfg.bus.contains_key("ican") {
            return Err("[bus.ican] is required".into());
        }
        cfg.check()?;
        Ok(cfg)
    }

    fn check(&self) -> Result<(), String> {
        let l = &self.light;
        let manual: Vec<(f64, u8)> = l.manual_curve.iter().map(|p| (p.ambient, p.pct)).collect();
        check_curve("manual_curve", &manual, 32)?;
        if manual.windows(2).any(|w| w[1].0 <= w[0].0) {
            return Err("[light] manual_curve: ambient must go up point by point".into());
        }
        let learned: Vec<(f64, u8)> = l.learned.iter().map(|p| (p.ambient, p.pct)).collect();
        check_curve("learned", &learned, 256)?;
        if !l.night_threshold.is_finite() {
            return Err("[light] night_threshold must be a number".into());
        }
        if !(l.night_hysteresis.is_finite() && l.night_hysteresis >= 0.0) {
            return Err("[light] night_hysteresis must be 0 or more".into());
        }
        if !(l.night_dwell_s.is_finite() && (0.0..=3600.0).contains(&l.night_dwell_s)) {
            return Err("[light] night_dwell_s must be 0-3600".into());
        }
        match (l.latitude, l.longitude) {
            (None, None) => {}
            (Some(la), Some(lo))
                if (-90.0..=90.0).contains(&la) && (-180.0..=180.0).contains(&lo) => {}
            (Some(_), Some(_)) => {
                return Err("[light] latitude must be -90 to 90, longitude -180 to 180".into());
            }
            _ => return Err("[light] latitude and longitude go together".into()),
        }
        if l.utc_offset_hours
            .is_some_and(|o| !(-14.0..=14.0).contains(&o))
        {
            return Err("[light] utc_offset_hours must be -14 to 14".into());
        }
        for (name, v) in [("night_from", &l.night_from), ("day_from", &l.day_from)] {
            if parse_hhmm(v).is_none() {
                return Err(format!("[light] {name}: \"HH:MM\", got \"{v}\""));
            }
        }
        if !(1..=60).contains(&self.logs.capture_minutes) {
            return Err("[logs] capture_minutes must be 1-60".into());
        }
        if !(1..=90).contains(&self.logs.max_pct) {
            return Err("[logs] max_pct must be 1-90".into());
        }
        if let Some(r) = &self.dis.readings {
            if r.len() > 8 {
                return Err(format!("[dis] readings: at most 8, got {}", r.len()));
            }
            for (i, c) in r.iter().enumerate() {
                if c.key.is_empty() {
                    return Err(format!("[dis] readings {}: key is empty", i + 1));
                }
                if c.label.chars().count() > 4 {
                    return Err(format!(
                        "[dis] readings {}: label '{}' is over 4 characters",
                        i + 1,
                        c.label
                    ));
                }
                if c.decimals > 3 {
                    return Err(format!("[dis] readings {}: decimals must be 0-3", i + 1));
                }
                if !c.scale.is_finite() || c.scale == 0.0 {
                    return Err(format!("[dis] readings {}: bad scale", i + 1));
                }
            }
        }
        self.routes().map(|_| ())
    }

    /// The binding table: configured, or the built-in one.
    pub fn routes(&self) -> Result<RouteConfig, String> {
        match &self.input.bindings {
            None => Ok(RouteConfig::default()),
            Some(b) => RouteConfig::from_rows(
                &b.iter()
                    .map(|b| BindingRow {
                        table: b.table.clone(),
                        key: b.key.clone(),
                        gesture: b.gesture.clone(),
                        action: b.action.clone(),
                    })
                    .collect::<Vec<_>>(),
            )
            .map_err(|e| format!("[input] bindings: {e}")),
        }
    }

    pub fn readings(&self) -> Vec<Reading> {
        match &self.dis.readings {
            None => carpi_core::display::default_readings(),
            Some(r) => r
                .iter()
                .map(|c| Reading::new(&c.key, &c.label, c.decimals, c.scale))
                .collect(),
        }
    }

    /// What the car model takes from this file. `parse` checked it.
    pub fn core(&self) -> CoreConfig {
        let l = &self.light;
        CoreConfig {
            dis: DisSetup {
                enabled: self.dis.enabled,
                readings: self.readings(),
            },
            light: carpi_core::LightSetup {
                auto_brightness: l.auto_brightness,
                scroll_trim: l.scroll_trim,
                ambient_source: to_core_source(l.ambient_source),
                curve_mode: match l.curve_mode {
                    CurveMode::Manual => carpi_core::CurveMode::Manual,
                    CurveMode::Learned => carpi_core::CurveMode::Learned,
                },
                manual_curve: l.manual_curve.iter().map(|p| (p.ambient, p.pct)).collect(),
                learned: l
                    .learned
                    .iter()
                    .map(|p| (to_core_source(p.source), p.ambient, p.pct))
                    .collect(),
                dark: carpi_core::DarkSetup {
                    source: match l.dark_mode_source {
                        DarkModeSource::Headlights => carpi_core::DarkSource::Headlights,
                        DarkModeSource::Ambient => carpi_core::DarkSource::Ambient,
                    },
                    threshold: l.night_threshold,
                    hysteresis: l.night_hysteresis,
                    dwell: std::time::Duration::from_secs_f64(l.night_dwell_s),
                    location: match (l.latitude, l.longitude) {
                        (Some(latitude), Some(longitude)) => Some(carpi_core::Location {
                            latitude,
                            longitude,
                            utc_offset_hours: l.utc_offset_hours,
                        }),
                        _ => None,
                    },
                    night_from: parse_hhmm(&l.night_from).unwrap_or(19 * 60),
                    day_from: parse_hhmm(&l.day_from).unwrap_or(7 * 60),
                },
            },
            routes: self.routes().unwrap_or_default(),
        }
    }

    /// Default when no file exists: ICAN and diag both on `can0`.
    pub fn fallback() -> Self {
        let mut bus = BTreeMap::new();
        bus.insert(
            "ican".into(),
            Bus {
                iface: "can0".into(),
            },
        );
        bus.insert(
            "diag".into(),
            Bus {
                iface: "can0".into(),
            },
        );
        Config {
            bus,
            api: Api::default(),
            recorder: RecorderCfg::default(),
            logs: LogsCfg::default(),
            clock: ClockCfg::default(),
            tx: Tx::default(),
            power: Power::default(),
            input: Input::default(),
            update: Update::default(),
            light: Light::default(),
            dis: Dis::default(),
            portal: None,
        }
    }

    /// Interfaces with the roles each carries, in a stable order.
    pub fn ifaces(&self) -> Vec<carpi_core::IfaceSpec> {
        let mut out: Vec<carpi_core::IfaceSpec> = Vec::new();
        for (role, bus) in &self.bus {
            match out.iter_mut().find(|i| i.name == bus.iface) {
                Some(i) => i.roles.push(role.clone()),
                None => out.push(carpi_core::IfaceSpec {
                    name: bus.iface.clone(),
                    roles: vec![role.clone()],
                }),
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_and_separate_ifaces() {
        let cfg: Config =
            toml::from_str("[bus.ican]\niface='ican'\n[bus.diag]\niface='ican'\n").unwrap();
        let i = cfg.ifaces();
        assert_eq!(i.len(), 1);
        assert_eq!(i[0].roles, vec!["diag".to_string(), "ican".to_string()]);

        let cfg: Config =
            toml::from_str("[bus.ican]\niface='ican'\n[bus.diag]\niface='obd'\n").unwrap();
        assert_eq!(cfg.ifaces().len(), 2);
    }

    #[test]
    fn rejects_typos() {
        assert!(toml::from_str::<Config>("[bus.ican]\nifcae='x'\n").is_err());
    }

    #[test]
    fn deploy_file_parses_with_safe_defaults() {
        let text = include_str!("../../../deploy/carpid.toml");
        let cfg = Config::parse(text).unwrap();
        assert!(!cfg.tx.enabled);
        assert!(!cfg.power.shutdown);
        assert!(!cfg.dis.enabled);
        let p = cfg.portal.as_ref().expect("[portal] in the deploy file");
        assert_eq!(p.bind.port(), 5003);
        assert!(p.pin.is_none());
        let core = cfg.core();
        assert_eq!(core.dis.readings.len(), 8);
    }

    #[test]
    fn checks_new_sections() {
        let base = "[bus.ican]\niface='can0'\n";
        assert!(Config::parse(&format!("{base}[light]\nnight_hysteresis = -1.0\n")).is_err());
        assert!(
            Config::parse(&format!(
                "{base}[light]\nmanual_curve = [{{ ambient = 2.0, pct = 10 }}, {{ ambient = 1.0, pct = 20 }}]\n"
            ))
            .is_err()
        );
        assert!(
            Config::parse(&format!(
                "{base}[light]\nmanual_curve = [{{ ambient = 1.0, pct = 101 }}]\n"
            ))
            .is_err()
        );
        let c = Config::parse(&format!("{base}[light]\nambient_source = \"blend\"\n")).unwrap();
        assert_eq!(c.light.ambient_source, AmbientSource::Blend);
        assert!(c.light.auto_brightness, "Drew's default");
        let bad = format!(
            "{base}[input]\nbindings = [{{ table = 'livi', key = 'x', gesture = 'short', action = 'home' }}]\n"
        );
        let e = Config::parse(&bad).unwrap_err();
        assert!(e.contains("[input] bindings"), "{e}");
        let nine: Vec<String> = (0..9)
            .map(|i| format!("{{ key = 'k{i}', label = 'L' }}"))
            .collect();
        let e =
            Config::parse(&format!("{base}[dis]\nreadings = [{}]\n", nine.join(","))).unwrap_err();
        assert!(e.contains("at most 8"), "{e}");
        assert!(Config::parse("[bus.diag]\niface='can0'\n").is_err());
    }

    #[test]
    fn logs_section_and_old_recorder_dir() {
        let base = "[bus.ican]\niface='can0'\n";
        let c = Config::parse(base).unwrap();
        assert_eq!(c.logs.capture_minutes, 5);
        assert_eq!(c.logs.max_pct, 25);
        assert!(c.clock.set_from_car);
        assert!(c.logs.root().ends_with("logs"));
        // Old files still load; the key is ignored.
        let c = Config::parse(&format!("{base}[recorder]\nminutes = 10\ndir = '/x'\n")).unwrap();
        assert_eq!(c.recorder.dir.as_deref(), Some(Path::new("/x")));
        let c = Config::parse(&format!(
            "{base}[logs]\ndir = '/data/logs'\ncapture_minutes = 2\n[clock]\nset_from_car = false\n"
        ))
        .unwrap();
        assert_eq!(c.logs.root(), PathBuf::from("/data/logs"));
        assert!(!c.clock.set_from_car);
        assert!(Config::parse(&format!("{base}[logs]\nmax_pct = 0\n")).is_err());
        assert!(Config::parse(&format!("{base}[logs]\ncapture_minutes = 0\n")).is_err());
    }
}
