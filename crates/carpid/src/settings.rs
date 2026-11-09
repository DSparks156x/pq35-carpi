//! `Request::Settings`: a typed view of the parts of `carpid.toml` the pages
//! may change. `Set` edits the file in place (comments and other keys kept),
//! checks the result with the loader's own rules, then replaces the file
//! atomically with a backup beside it. A rejected patch leaves the file alone.
//!
//! Most of it applies live (see `main.rs`); `restart_needed` names what
//! can't. The timezone (timedated) and the A/V-over-SD settings (avsd.env)
//! aren't in carpid.toml: `main.rs` sets those, and [`system`] reads them.

use crate::config::Config;
use carpi_proto::{
    AmbientSource, Binding, LearnedPoint, LightPatch, PinChange, ReadingSetting, Settings,
    SettingsPatch, UpdateChannel,
};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;
use toml_edit::{Array, DocumentMut, InlineTable, Item, Value, value};

pub fn get(cfg: &Config) -> Settings {
    Settings {
        tx_enabled: cfg.tx.enabled,
        power_shutdown: cfg.power.shutdown,
        light: carpi_proto::LightSettings {
            auto_brightness: cfg.light.auto_brightness,
            scroll_trim: cfg.light.scroll_trim,
            ambient_source: cfg.light.ambient_source,
            curve_mode: cfg.light.curve_mode,
            manual_curve: cfg.light.manual_curve.clone(),
            learned: cfg.light.learned.clone(),
            dark_mode_source: cfg.light.dark_mode_source,
            night_threshold: cfg.light.night_threshold,
            night_hysteresis: cfg.light.night_hysteresis,
            night_dwell_s: cfg.light.night_dwell_s,
            latitude: cfg.light.latitude,
            longitude: cfg.light.longitude,
            utc_offset_hours: cfg.light.utc_offset_hours,
            night_from: cfg.light.night_from.clone(),
            day_from: cfg.light.day_from.clone(),
        },
        dis: carpi_proto::DisSettings {
            enabled: cfg.dis.enabled,
            claim: match cfg.dis.claim {
                crate::config::Claim::Always => carpi_proto::DisClaim::Always,
                crate::config::Claim::Tab => carpi_proto::DisClaim::Tab,
            },
            default_page: match cfg.dis.default_page {
                crate::config::DisPage::Readings => carpi_proto::DisPage::Readings,
                crate::config::DisPage::Accel => carpi_proto::DisPage::Accel,
                crate::config::DisPage::Media => carpi_proto::DisPage::Media,
            },
            pages: cfg
                .dis
                .pages
                .iter()
                .map(|p| carpi_proto::DisPageSetting {
                    name: p.name.clone(),
                    cells: p
                        .cells
                        .iter()
                        .map(|c| carpi_proto::DisCellSetting {
                            key: c.key.clone(),
                            decimals: c.decimals,
                            scale: c.scale,
                            unit: c.unit.clone(),
                            icon: c.icon.clone(),
                        })
                        .collect(),
                })
                .collect(),
            claim_nav: cfg.dis.claim_nav,
            release_nav: cfg.dis.release_nav,
            claim_phone: cfg.dis.claim_phone,
            release_phone: cfg.dis.release_phone,
            claim_media: cfg.dis.claim_media,
            release_media: cfg.dis.release_media,
            media_layout: match cfg.dis.media_layout {
                crate::config::DisMediaLayout::Stack => carpi_proto::DisMediaLayout::Stack,
                crate::config::DisMediaLayout::Art => carpi_proto::DisMediaLayout::Art,
            },
            cover_flash_s: cfg.dis.cover_flash_s,
            units: match cfg.dis.units {
                crate::config::DisUnits::Auto => carpi_proto::DisUnits::Auto,
                crate::config::DisUnits::Metric => carpi_proto::DisUnits::Metric,
                crate::config::DisUnits::Imperial => carpi_proto::DisUnits::Imperial,
            },
            clock: match cfg.dis.clock {
                crate::config::DisClock::Auto => carpi_proto::DisClock::Auto,
                crate::config::DisClock::H12 => carpi_proto::DisClock::H12,
                crate::config::DisClock::H24 => carpi_proto::DisClock::H24,
            },
            nav_style: match cfg.dis.nav_style {
                crate::config::DisNavStyle::Fancy => carpi_proto::DisNavStyle::Fancy,
                crate::config::DisNavStyle::Stock => carpi_proto::DisNavStyle::Stock,
            },
            nav_distance: cfg.dis.nav_distance,
            nav_pocket: cfg.dis.nav_pocket,
            nav_eta: cfg.dis.nav_eta,
            nav_time_left: cfg.dis.nav_time_left,
            nav_dest_distance: cfg.dis.nav_dest_distance,
            nav_lanes: match cfg.dis.nav_lanes {
                crate::config::DisLanes::Off => carpi_proto::DisLanes::Off,
                crate::config::DisLanes::Swap => carpi_proto::DisLanes::Swap,
                crate::config::DisLanes::Flip => carpi_proto::DisLanes::Flip,
            },
            accel: cfg.dis.accel.clone(),
            readings: cfg
                .readings()
                .into_iter()
                .map(|r| ReadingSetting {
                    key: r.key,
                    label: r.label,
                    decimals: r.decimals,
                    scale: r.scale,
                })
                .collect(),
        },
        bindings: cfg
            .routes()
            .unwrap_or_default()
            .to_rows()
            .into_iter()
            .map(|r| Binding {
                table: r.table,
                key: r.key,
                gesture: r.gesture,
                action: r.action,
            })
            .collect(),
        bindings_custom: cfg.input.bindings.is_some(),
        portal_pin_set: cfg
            .portal
            .as_ref()
            .and_then(|p| p.pin.as_deref())
            .is_some_and(|p| !p.is_empty()),
        logs: carpi_proto::LogsSettings {
            capture_minutes: cfg.logs.capture_minutes as u32,
            max_pct: cfg.logs.max_pct,
        },
        clock: carpi_proto::ClockSettings {
            set_from_car: cfg.clock.set_from_car,
            ..Default::default()
        },
        power: carpi_proto::PowerSettings {
            shutdown: cfg.power.shutdown,
            silent_s: cfg.power.silent_s,
            radio_hold_min: cfg.power.radio_hold_min,
        },
        input: carpi_proto::InputSettings {
            uinput: cfg.input.uinput,
            long_ms: cfg.input.long_ms,
            extended_s: cfg.input.extended_s,
        },
        portal: carpi_proto::PortalSettings {
            protect_downloads: cfg.portal.as_ref().is_some_and(|p| p.protect_downloads),
            address: cfg.portal.as_ref().map(|p| portal_address(p.bind, None)),
        },
        update: carpi_proto::UpdateSettings {
            channel: match cfg.update.channel.as_deref() {
                Some("beta") => UpdateChannel::Beta,
                _ => UpdateChannel::Stable,
            },
            version: String::new(),
        },
        avsd: carpi_proto::AvsdSettings {
            path: crate::config::expand_home(&cfg.avsd.env)
                .display()
                .to_string(),
            found: false,
            values: crate::avsd::values(""),
            ranges: crate::avsd::ranges(),
        },
    }
}

/// The parts [`get`] can't know from carpid.toml: timezone, portal address
/// on the hotspot, release, avsd.env.
pub fn system(s: &mut Settings, cfg: &Config) {
    s.clock.timezone = crate::clock::timezone();
    s.clock.timezones = crate::clock::timezones().to_vec();
    if let Some(p) = &cfg.portal {
        s.portal.address = Some(portal_address(p.bind, hotspot_ip()));
    }
    s.update.version = version(Path::new(MANIFEST));
    s.avsd = crate::avsd::read(&crate::config::expand_home(&cfg.avsd.env));
}

const MANIFEST: &str = "/opt/carpi/current/manifest.json";

/// LIVI's hotspot address when wlan0 has none (its default).
const HOTSPOT_IP: Ipv4Addr = Ipv4Addr::new(10, 10, 0, 1);

/// The URL the phone uses: the bind address, or the hotspot's when bound to
/// every interface.
pub fn portal_address(bind: SocketAddr, wlan: Option<Ipv4Addr>) -> String {
    let ip = if bind.ip().is_unspecified() {
        IpAddr::V4(wlan.unwrap_or(HOTSPOT_IP))
    } else {
        bind.ip()
    };
    format!("http://{}", SocketAddr::new(ip, bind.port()))
}

/// `carpid 0.1.0, release 2026.11.08 (stable)`.
pub fn version(manifest: &Path) -> String {
    let carpid = format!("carpid {}", env!("CARGO_PKG_VERSION"));
    let m = std::fs::read(manifest)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    match m {
        Some(m) => format!(
            "{carpid}, release {} ({})",
            m["version"].as_str().unwrap_or("?"),
            m["channel"].as_str().unwrap_or("?")
        ),
        None => format!("{carpid} (no release manifest)"),
    }
}

/// wlan0's IPv4 address (the hotspot's while it runs).
#[cfg(unix)]
fn hotspot_ip() -> Option<Ipv4Addr> {
    let mut found = None;
    // SAFETY: getifaddrs fills a list we only read, then free once.
    unsafe {
        let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut list) != 0 {
            return None;
        }
        let mut p = list;
        while !p.is_null() {
            let a = &*p;
            if !a.ifa_addr.is_null()
                && i32::from((*a.ifa_addr).sa_family) == libc::AF_INET
                && std::ffi::CStr::from_ptr(a.ifa_name).to_bytes() == b"wlan0"
            {
                let sin = &*(a.ifa_addr as *const libc::sockaddr_in);
                found = Some(Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr)));
                break;
            }
            p = a.ifa_next;
        }
        libc::freeifaddrs(list);
    }
    found
}

#[cfg(not(unix))]
fn hotspot_ip() -> Option<Ipv4Addr> {
    None
}

/// One element per line, so the file stays readable.
fn array(items: impl IntoIterator<Item = InlineTable>) -> Item {
    let mut a = Array::new();
    for t in items {
        let mut v = Value::InlineTable(t);
        v.decor_mut().set_prefix("\n    ");
        a.push_formatted(v);
    }
    a.set_trailing("\n");
    a.set_trailing_comma(true);
    value(a)
}

fn table(doc: &mut DocumentMut, name: &str) -> Result<(), String> {
    match doc.get(name) {
        None => {
            doc[name] = toml_edit::table();
            Ok(())
        }
        Some(i) if i.is_table() => Ok(()),
        Some(_) => Err(format!("[{name}] in the file isn't a table")),
    }
}

fn check_pin(pin: &str) -> Result<(), String> {
    let n = pin.chars().count();
    if !(4..=32).contains(&n) {
        return Err("the PIN must be 4-32 characters".into());
    }
    if pin.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("the PIN can't contain spaces or control characters".into());
    }
    Ok(())
}

fn name<T: serde::Serialize>(v: T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

fn light(doc: &mut DocumentMut, l: &LightPatch) -> Result<(), String> {
    table(doc, "light")?;
    let t = &mut doc["light"];
    if let Some(v) = l.auto_brightness {
        t["auto_brightness"] = value(v);
    }
    if let Some(v) = l.scroll_trim {
        t["scroll_trim"] = value(v);
    }
    if let Some(v) = l.ambient_source {
        t["ambient_source"] = value(name(v));
    }
    if let Some(v) = l.curve_mode {
        t["curve_mode"] = value(name(v));
    }
    if let Some(c) = &l.manual_curve {
        t["manual_curve"] = array(c.iter().map(|p| {
            let mut i = InlineTable::new();
            i.insert("ambient", p.ambient.into());
            i.insert("pct", i64::from(p.pct).into());
            i
        }));
    }
    if let Some(v) = l.night_threshold {
        t["night_threshold"] = value(v);
    }
    if let Some(v) = l.night_hysteresis {
        t["night_hysteresis"] = value(v);
    }
    if let Some(v) = l.dark_mode_source {
        t["dark_mode_source"] = value(name(v));
    }
    if let Some(v) = l.night_dwell_s {
        t["night_dwell_s"] = value(v);
    }
    if l.clear_location
        && let Some(tt) = t.as_table_mut()
    {
        for k in ["latitude", "longitude", "utc_offset_hours"] {
            tt.remove(k);
        }
    }
    if let Some(v) = l.latitude {
        t["latitude"] = value(v);
    }
    if let Some(v) = l.longitude {
        t["longitude"] = value(v);
    }
    if let Some(v) = l.utc_offset_hours {
        t["utc_offset_hours"] = value(v);
    }
    if let Some(v) = &l.night_from {
        t["night_from"] = value(v.as_str());
    }
    if let Some(v) = &l.day_from {
        t["day_from"] = value(v.as_str());
    }
    if l.reset_learned {
        // The source the points are for: the patch's, else the file's.
        let src = match l.ambient_source {
            Some(s) => name(s),
            None => t
                .get("ambient_source")
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_else(|| name(AmbientSource::Cluster)),
        };
        if let Some(arr) = t.get_mut("learned").and_then(|v| v.as_array_mut()) {
            arr.retain(|p| {
                p.as_inline_table()
                    .and_then(|p| p.get("source"))
                    .and_then(|s| s.as_str())
                    != Some(src.as_str())
            });
        }
    }
    Ok(())
}

fn learned_array(points: &[LearnedPoint]) -> Item {
    array(points.iter().map(|p| {
        let mut i = InlineTable::new();
        i.insert("source", name(p.source).into());
        // Two decimals are plenty on the log scale and keep the file tidy.
        i.insert("ambient", ((p.ambient * 100.0).round() / 100.0).into());
        i.insert("pct", i64::from(p.pct).into());
        i
    }))
}

/// Replaces `[light] learned` (carpid-owned: the light model's nudges).
/// Same checks and backup as [`set`].
pub fn save_learned(path: &Path, points: &[LearnedPoint]) -> Result<Config, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("can't read {}: {e}", path.display()))?;
    let mut doc: DocumentMut = text.parse().map_err(|e| format!("current file: {e}"))?;
    table(&mut doc, "light")?;
    doc["light"]["learned"] = learned_array(points);
    let new = doc.to_string();
    let cfg = Config::parse(&new).map_err(|e| format!("rejected, nothing saved: {e}"))?;
    if new != text {
        carpi_portal::replace_with_backup(path, &new)
            .map_err(|e| format!("saving {}: {e}", path.display()))?;
    }
    Ok(cfg)
}

/// The file's text with `patch` applied. Doesn't check the result.
pub fn apply(text: &str, patch: &SettingsPatch) -> Result<String, String> {
    let mut doc: DocumentMut = text.parse().map_err(|e| format!("current file: {e}"))?;
    if let Some(v) = patch.tx_enabled {
        table(&mut doc, "tx")?;
        doc["tx"]["enabled"] = value(v);
    }
    // The old flat field first: `power.shutdown` wins.
    if let Some(v) = patch.power_shutdown {
        table(&mut doc, "power")?;
        doc["power"]["shutdown"] = value(v);
    }
    if let Some(p) = &patch.power {
        table(&mut doc, "power")?;
        if let Some(v) = p.shutdown {
            doc["power"]["shutdown"] = value(v);
        }
        if let Some(v) = p.silent_s {
            doc["power"]["silent_s"] = value(i64::from(v));
        }
        if let Some(v) = p.radio_hold_min {
            doc["power"]["radio_hold_min"] = value(i64::from(v));
        }
    }
    if let Some(l) = &patch.logs {
        table(&mut doc, "logs")?;
        if let Some(v) = l.capture_minutes {
            if !(1..=30).contains(&v) {
                return Err("capture length must be 1-30 minutes".into());
            }
            doc["logs"]["capture_minutes"] = value(i64::from(v));
        }
        if let Some(v) = l.max_pct {
            if !(5..=75).contains(&v) {
                return Err("space for logs must be 5-75 % of the disk".into());
            }
            doc["logs"]["max_pct"] = value(i64::from(v));
        }
    }
    // The timezone isn't in the file (main.rs sets it).
    if let Some(v) = patch.clock.as_ref().and_then(|c| c.set_from_car) {
        table(&mut doc, "clock")?;
        doc["clock"]["set_from_car"] = value(v);
    }
    if let Some(i) = &patch.input {
        table(&mut doc, "input")?;
        if let Some(v) = i.uinput {
            doc["input"]["uinput"] = value(v);
        }
        if let Some(v) = i.long_ms {
            doc["input"]["long_ms"] = value(i64::from(v));
        }
        if let Some(v) = i.extended_s {
            doc["input"]["extended_s"] = value(i64::from(v));
        }
    }
    if let Some(v) = patch.portal.as_ref().and_then(|p| p.protect_downloads) {
        if !doc.get("portal").is_some_and(Item::is_table) {
            return Err("the portal is off ([portal] isn't in carpid.toml)".into());
        }
        doc["portal"]["protect_downloads"] = value(v);
    }
    if let Some(v) = patch.update.as_ref().and_then(|u| u.channel) {
        table(&mut doc, "update")?;
        doc["update"]["channel"] = value(name(v));
    }
    if let Some(l) = &patch.light {
        light(&mut doc, l)?;
    }
    if let Some(v) = patch.dis_enabled {
        table(&mut doc, "dis")?;
        doc["dis"]["enabled"] = value(v);
    }
    if let Some(r) = &patch.dis_readings {
        table(&mut doc, "dis")?;
        doc["dis"]["readings"] = array(r.iter().map(|r| {
            let mut t = InlineTable::new();
            t.insert("key", r.key.as_str().into());
            t.insert("label", r.label.as_str().into());
            t.insert("decimals", i64::from(r.decimals).into());
            t.insert("scale", r.scale.into());
            t
        }));
    }
    if let Some(d) = &patch.dis {
        table(&mut doc, "dis")?;
        let snake = |v: &dyn erased::Snake| v.snake();
        if let Some(v) = d.claim {
            doc["dis"]["claim"] = value(snake(&v));
        }
        if let Some(v) = d.default_page {
            doc["dis"]["default_page"] = value(snake(&v));
        }
        if let Some(v) = d.claim_nav {
            // The old name would shadow it on the next read.
            doc["dis"].as_table_like_mut().map(|t| t.remove("auto_nav"));
            doc["dis"]["claim_nav"] = value(v);
        }
        if let Some(v) = d.release_nav {
            doc["dis"]["release_nav"] = value(v);
        }
        if let Some(v) = d.claim_phone {
            // The old name would shadow it on the next read.
            doc["dis"]
                .as_table_like_mut()
                .map(|t| t.remove("auto_phone"));
            doc["dis"]["claim_phone"] = value(v);
        }
        if let Some(v) = d.release_phone {
            doc["dis"]["release_phone"] = value(v);
        }
        if let Some(v) = d.claim_media {
            // The old name would shadow it on the next read.
            doc["dis"]
                .as_table_like_mut()
                .map(|t| t.remove("auto_media"));
            doc["dis"]["claim_media"] = value(v);
        }
        if let Some(v) = d.release_media {
            doc["dis"]["release_media"] = value(v);
        }
        if let Some(v) = d.media_layout {
            doc["dis"]["media_layout"] = value(snake(&v));
        }
        if let Some(v) = d.cover_flash_s {
            doc["dis"]["cover_flash_s"] = value(v);
        }
        if let Some(pages) = &d.pages {
            doc["dis"]["pages"] = array(pages.iter().map(|p| {
                let mut t = InlineTable::new();
                t.insert("name", p.name.as_str().into());
                let mut cells = Array::new();
                for c in &p.cells {
                    let mut ct = InlineTable::new();
                    ct.insert("key", c.key.as_str().into());
                    ct.insert("decimals", i64::from(c.decimals).into());
                    ct.insert("scale", c.scale.into());
                    if let Some(u) = &c.unit {
                        ct.insert("unit", u.as_str().into());
                    }
                    if let Some(i) = &c.icon {
                        ct.insert("icon", i.as_str().into());
                    }
                    cells.push(Value::InlineTable(ct));
                }
                t.insert("cells", Value::Array(cells));
                t
            }));
        }
        if let Some(v) = d.units {
            doc["dis"]["units"] = value(snake(&v));
        }
        if let Some(v) = d.clock {
            doc["dis"]["clock"] = value(snake(&v));
        }
        if let Some(v) = d.nav_style {
            doc["dis"]["nav_style"] = value(snake(&v));
        }
        if let Some(v) = d.nav_distance {
            doc["dis"]["nav_distance"] = value(v);
        }
        if let Some(v) = d.nav_pocket {
            doc["dis"]["nav_pocket"] = value(v);
        }
        if let Some(v) = d.nav_eta {
            doc["dis"]["nav_eta"] = value(v);
        }
        if let Some(v) = d.nav_time_left {
            doc["dis"]["nav_time_left"] = value(v);
        }
        if let Some(v) = d.nav_dest_distance {
            doc["dis"]["nav_dest_distance"] = value(v);
        }
        if let Some(v) = d.nav_lanes {
            doc["dis"]["nav_lanes"] = value(snake(&v));
        }
        if let Some(lines) = &d.accel {
            doc["dis"]["accel"] =
                value(toml_edit::Array::from_iter(lines.iter().map(|l| l.trim())));
        }
    }
    if patch.bindings_reset {
        if let Some(t) = doc.get_mut("input").and_then(Item::as_table_like_mut) {
            t.remove("bindings");
        }
    } else if let Some(b) = &patch.bindings {
        table(&mut doc, "input")?;
        doc["input"]["bindings"] = array(b.iter().map(|b| {
            let mut t = InlineTable::new();
            t.insert("table", b.table.as_str().into());
            t.insert("key", b.key.as_str().into());
            t.insert("gesture", b.gesture.as_str().into());
            t.insert("action", b.action.as_str().into());
            t
        }));
    }
    match &patch.portal_pin {
        None => {}
        Some(change) => {
            // Setting a PIN never turns the portal on by itself.
            if !doc.get("portal").is_some_and(Item::is_table) {
                return Err("the portal is off ([portal] isn't in carpid.toml)".into());
            }
            match change {
                PinChange::Set { pin } => {
                    check_pin(pin)?;
                    doc["portal"]["pin"] = value(pin.as_str());
                }
                PinChange::Clear => {
                    if let Some(t) = doc["portal"].as_table_mut() {
                        t.remove("pin");
                    }
                }
            }
        }
    }
    Ok(doc.to_string())
}

/// Patch the file at `path`, check it, save it with a backup. Returns the
/// new config; on any error the file is unchanged.
pub fn set(path: &Path, patch: &SettingsPatch) -> Result<Config, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("can't read {}: {e}", path.display()))?;
    let new = apply(&text, patch)?;
    let cfg = Config::parse(&new).map_err(|e| format!("rejected, nothing saved: {e}"))?;
    if new != text {
        carpi_portal::replace_with_backup(path, &new)
            .map_err(|e| format!("saving {}: {e}", path.display()))?;
    }
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use carpi_proto::DarkModeSource;
    use std::time::Duration;

    const FILE: &str = "# my car\n[bus.ican]\niface = \"can0\" # TT\n\n[tx]\nenabled = false\n\n[portal]\nbind = \"0.0.0.0:5003\"\n";

    fn file() -> (tempfile::TempDir, std::path::PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("carpid.toml");
        std::fs::write(&p, FILE).unwrap();
        (d, p)
    }

    fn backups(dir: &Path) -> usize {
        std::fs::read_dir(dir)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".bak")
            })
            .count()
    }

    #[test]
    fn round_trip() {
        let (d, p) = file();
        let before = get(&Config::parse(FILE).unwrap());
        assert!(!before.tx_enabled);
        assert!(!before.portal_pin_set);
        assert_eq!(before.dis.readings.len(), 8);
        assert_eq!(before.bindings[0].table, "dis_toggle");

        let mut bindings = before.bindings.clone();
        bindings.retain(|b| !(b.table == "livi" && b.key == "setup"));
        bindings.push(Binding {
            table: "livi".into(),
            key: "setup".into(),
            gesture: "short".into(),
            action: "code:59".into(),
        });
        let mut readings = before.dis.readings.clone();
        readings.truncate(2);
        readings[1].label = "Oil".into();
        readings[1].key = "engine.oil_temp_c".into();
        let patch = SettingsPatch {
            tx_enabled: Some(true),
            light: Some(LightPatch {
                auto_brightness: Some(false),
                ambient_source: Some(AmbientSource::Climatronic),
                manual_curve: Some(vec![
                    carpi_proto::CurvePoint {
                        ambient: 0.5,
                        pct: 10,
                    },
                    carpi_proto::CurvePoint {
                        ambient: 2.5,
                        pct: 90,
                    },
                ]),
                night_threshold: Some(0.8),
                ..Default::default()
            }),
            dis_enabled: Some(true),
            dis_readings: Some(readings.clone()),
            bindings: Some(bindings),
            portal_pin: Some(PinChange::Set { pin: "4711".into() }),
            ..Default::default()
        };
        let cfg = set(&p, &patch).unwrap();
        let after = get(&cfg);
        assert!(after.tx_enabled);
        assert!(after.dis.enabled);
        assert_eq!(after.dis.readings, readings);
        assert!(!after.light.auto_brightness);
        assert_eq!(after.light.ambient_source, AmbientSource::Climatronic);
        assert_eq!(after.light.manual_curve.len(), 2);
        assert_eq!(after.light.night_threshold, 0.8);
        assert!(after.light.scroll_trim, "untouched");
        assert!(after.portal_pin_set);
        assert!(
            after
                .bindings
                .iter()
                .any(|b| b.key == "setup" && b.action == "code:59")
        );
        // What's on disk says the same, comments kept, old file backed up.
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("# my car") && text.contains("# TT"));
        assert_eq!(get(&Config::parse(&text).unwrap()), after);
        assert_eq!(backups(d.path()), 1);

        let cfg = set(
            &p,
            &SettingsPatch {
                portal_pin: Some(PinChange::Clear),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!get(&cfg).portal_pin_set);

        // Reset: the built-in table again, the rest of [input] kept.
        assert!(get(&cfg).bindings_custom);
        let cfg = set(
            &p,
            &SettingsPatch {
                bindings_reset: true,
                ..Default::default()
            },
        )
        .unwrap();
        let reset = get(&cfg);
        assert!(!reset.bindings_custom);
        assert_eq!(reset.bindings, before.bindings);
        assert!(!std::fs::read_to_string(&p).unwrap().contains("bindings ="));
    }

    #[test]
    fn dis_behaviour_round_trip() {
        use carpi_proto::{DisClaim, DisMediaLayout, DisPage, DisPatch};
        let (_d, p) = file();
        let before = get(&Config::parse(FILE).unwrap());
        assert_eq!(before.dis.claim, DisClaim::Always);
        assert_eq!(before.dis.default_page, DisPage::Readings);
        assert!(before.dis.claim_media && before.dis.release_media);
        let cfg = set(
            &p,
            &SettingsPatch {
                dis: Some(DisPatch {
                    claim: Some(DisClaim::Tab),
                    default_page: Some(DisPage::Media),
                    claim_phone: Some(false),
                    release_media: Some(false),
                    media_layout: Some(DisMediaLayout::Art),
                    cover_flash_s: Some(0.0),
                    units: Some(carpi_proto::DisUnits::Imperial),
                    pages: Some(vec![carpi_proto::DisPageSetting {
                        name: "Daily".into(),
                        cells: vec![carpi_proto::DisCellSetting {
                            key: "engine.boost.actual".into(),
                            decimals: 1,
                            scale: 1.0,
                            unit: Some("psi".into()),
                            icon: Some("boost".into()),
                        }],
                    }]),
                    clock: Some(carpi_proto::DisClock::H12),
                    nav_style: Some(carpi_proto::DisNavStyle::Stock),
                    nav_eta: Some(false),
                    nav_lanes: Some(carpi_proto::DisLanes::Swap),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap();
        let after = get(&cfg);
        assert_eq!(after.dis.claim, DisClaim::Tab);
        assert_eq!(after.dis.default_page, DisPage::Media);
        assert!(
            !after.dis.claim_phone && after.dis.claim_nav,
            "only phone changed"
        );
        assert!(!after.dis.release_media && after.dis.release_nav);
        assert_eq!(after.dis.media_layout, DisMediaLayout::Art);
        assert_eq!(after.dis.units, carpi_proto::DisUnits::Imperial);
        assert_eq!(after.dis.pages.len(), 1);
        assert_eq!(after.dis.pages[0].cells[0].unit.as_deref(), Some("psi"));
        assert_eq!(
            cfg.core().dis.pages[0].cells[0].icon.as_deref(),
            Some("boost")
        );
        assert_eq!(after.dis.clock, carpi_proto::DisClock::H12);
        assert_eq!(after.dis.nav_style, carpi_proto::DisNavStyle::Stock);
        assert!(!after.dis.nav_eta && after.dis.nav_time_left);
        assert_eq!(after.dis.nav_lanes, carpi_proto::DisLanes::Swap);
        assert_eq!(cfg.core().dis.nav_lanes, carpi_core::DisLanes::Swap);
        assert_eq!(cfg.core().dis.nav_style, carpi_core::DisNavStyle::Stock);
        // And what the car model gets.
        let core = cfg.core().dis;
        assert!(!core.claim_always);
        assert_eq!(core.default_page, carpi_core::DisScreen::Media);
        assert!(core.cover_flash.is_none());
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("claim = \"tab\""), "{text}");
        // Out of range is refused.
        let bad = set(
            &p,
            &SettingsPatch {
                dis: Some(DisPatch {
                    cover_flash_s: Some(-1.0),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        assert!(bad.is_err());
        // Accel lines as typed; one that doesn't parse is refused.
        let cfg = set(
            &p,
            &SettingsPatch {
                dis: Some(DisPatch {
                    accel: Some(vec!["0-60mph".into(), "1/4mi".into(), "speed".into()]),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(get(&cfg).dis.accel, ["0-60mph", "1/4mi", "speed"]);
        assert_eq!(cfg.core().dis.accel.len(), 3);
        let bad = set(
            &p,
            &SettingsPatch {
                dis: Some(DisPatch {
                    accel: Some(vec!["1000".into()]),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        assert!(bad.unwrap_err().contains("add a unit"));
    }

    #[test]
    fn bad_patch_keeps_the_old_file() {
        let (d, p) = file();
        let bad = SettingsPatch {
            tx_enabled: Some(true),
            bindings: Some(vec![Binding {
                table: "livi".into(),
                key: "wheel_up".into(),
                gesture: "sideways".into(),
                action: "prev".into(),
            }]),
            ..Default::default()
        };
        let e = set(&p, &bad).unwrap_err();
        assert!(e.contains("rejected"), "{e}");
        let light = SettingsPatch {
            light: Some(LightPatch {
                manual_curve: Some(vec![carpi_proto::CurvePoint {
                    ambient: 1.0,
                    pct: 150,
                }]),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(set(&p, &light).is_err());
        let pin = SettingsPatch {
            portal_pin: Some(PinChange::Set { pin: "1 2".into() }),
            ..Default::default()
        };
        assert!(set(&p, &pin).is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), FILE);
        assert_eq!(backups(d.path()), 0);
    }

    #[test]
    fn reset_learned_drops_only_the_current_source() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("carpid.toml");
        std::fs::write(
            &p,
            "[bus.ican]\niface = \"can0\"\n[light]\nambient_source = \"cluster\"\nlearned = [\n    { source = \"cluster\", ambient = 1.0, pct = 20 },\n    { source = \"climatronic\", ambient = 2.0, pct = 60 },\n]\n",
        )
        .unwrap();
        let cfg = set(
            &p,
            &SettingsPatch {
                light: Some(LightPatch {
                    reset_learned: true,
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap();
        let l = get(&cfg).light.learned;
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].source, AmbientSource::Climatronic);
    }

    #[test]
    fn learned_points_persist() {
        let (_d, p) = file();
        let pts = vec![
            LearnedPoint {
                source: AmbientSource::Cluster,
                ambient: 1.556_302,
                pct: 68,
            },
            LearnedPoint {
                source: AmbientSource::Climatronic,
                ambient: 2.0,
                pct: 40,
            },
        ];
        let cfg = save_learned(&p, &pts).unwrap();
        let got = get(&cfg).light.learned;
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].ambient, 1.56);
        assert_eq!(got[0].pct, 68);
        // Into the model: the learned point moves the curve there.
        let mut m = carpi_light::LightModel::new(cfg.core().light);
        let at = std::time::Instant::now();
        let l = m.update(
            at,
            &carpi_light::Inputs {
                cluster_sensor: Some(35.0),
                ..Default::default()
            },
        );
        assert!((l.target_pct.unwrap() - 68.0).abs() < 1.5, "{l:?}");
        // Comments kept, a later settings patch keeps them.
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("# my car"));
        let cfg = set(
            &p,
            &SettingsPatch {
                tx_enabled: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(get(&cfg).light.learned.len(), 2);
    }

    #[test]
    fn dark_mode_settings() {
        let (_d, p) = file();
        let before = get(&Config::parse(FILE).unwrap()).light;
        assert_eq!(before.dark_mode_source, DarkModeSource::Headlights);
        assert_eq!(
            (before.night_from.as_str(), before.day_from.as_str()),
            ("19:00", "07:00")
        );
        assert_eq!(before.latitude, None);
        let patch = |l: LightPatch| SettingsPatch {
            light: Some(l),
            ..Default::default()
        };
        let cfg = set(
            &p,
            &patch(LightPatch {
                dark_mode_source: Some(DarkModeSource::Ambient),
                night_dwell_s: Some(20.0),
                latitude: Some(51.5),
                longitude: Some(-0.13),
                utc_offset_hours: Some(0.0),
                night_from: Some("18:30".into()),
                ..Default::default()
            }),
        )
        .unwrap();
        let l = get(&cfg).light;
        assert_eq!(l.dark_mode_source, DarkModeSource::Ambient);
        assert_eq!(l.night_dwell_s, 20.0);
        assert_eq!((l.latitude, l.longitude), (Some(51.5), Some(-0.13)));
        assert_eq!(l.night_from, "18:30");
        // Half a location, or a bad time, is refused.
        assert!(
            set(
                &p,
                &patch(LightPatch {
                    clear_location: true,
                    latitude: Some(10.0),
                    ..Default::default()
                })
            )
            .is_err()
        );
        assert!(
            set(
                &p,
                &patch(LightPatch {
                    day_from: Some("7am".into()),
                    ..Default::default()
                })
            )
            .is_err()
        );
        let cfg = set(
            &p,
            &patch(LightPatch {
                clear_location: true,
                ..Default::default()
            }),
        )
        .unwrap();
        let l = get(&cfg).light;
        assert_eq!(
            (l.latitude, l.longitude, l.utc_offset_hours),
            (None, None, None)
        );
    }

    #[test]
    fn new_sections_round_trip() {
        use carpi_proto::{
            ClockPatch, InputPatch, LogsPatch, PortalPatch, PowerPatch, UpdatePatch,
        };
        let (_d, p) = file();
        let before = get(&Config::parse(FILE).unwrap());
        assert_eq!(before.logs, carpi_proto::LogsSettings::default());
        assert_eq!(before.power, carpi_proto::PowerSettings::default());
        assert_eq!(before.input, carpi_proto::InputSettings::default());
        assert_eq!(before.update.channel, UpdateChannel::Stable);
        assert_eq!(
            before.portal.address.as_deref(),
            Some("http://10.10.0.1:5003")
        );
        assert_eq!(before.avsd.ranges.len(), before.avsd.values.len());
        let cfg = set(
            &p,
            &SettingsPatch {
                logs: Some(LogsPatch {
                    capture_minutes: Some(20),
                    max_pct: Some(50),
                }),
                clock: Some(ClockPatch {
                    set_from_car: Some(false),
                    timezone: Some("Europe/Berlin".into()),
                }),
                power: Some(PowerPatch {
                    shutdown: Some(true),
                    silent_s: Some(90),
                    radio_hold_min: Some(0),
                }),
                input: Some(InputPatch {
                    uinput: Some(false),
                    long_ms: Some(600),
                    extended_s: Some(5),
                }),
                portal: Some(PortalPatch {
                    protect_downloads: Some(true),
                }),
                update: Some(UpdatePatch {
                    channel: Some(UpdateChannel::Beta),
                }),
                ..Default::default()
            },
        )
        .unwrap();
        let after = get(&cfg);
        assert_eq!((after.logs.capture_minutes, after.logs.max_pct), (20, 50));
        assert!(!after.clock.set_from_car);
        assert_eq!(
            after.power,
            carpi_proto::PowerSettings {
                shutdown: true,
                silent_s: 90,
                radio_hold_min: 0,
            }
        );
        assert!(after.power_shutdown, "the old field follows");
        assert_eq!(
            after.input,
            carpi_proto::InputSettings {
                uinput: false,
                long_ms: 600,
                extended_s: 5,
            }
        );
        assert!(after.portal.protect_downloads);
        assert_eq!(after.update.channel, UpdateChannel::Beta);
        let text = std::fs::read_to_string(&p).unwrap();
        assert_eq!(get(&Config::parse(&text).unwrap()), after);
        assert!(text.contains("channel = \"beta\""), "{text}");
        assert!(!text.contains("Europe"), "the timezone isn't in the file");
        assert!(text.contains("# my car") && text.contains("# TT"));
        // What the car model gets.
        assert_eq!(cfg.power_policy().silent_for, Duration::from_secs(90));
        assert_eq!(cfg.power_policy().radio_hold, Duration::ZERO);
        assert_eq!(
            cfg.gestures().timing(carpi_input::Key::Next).long,
            Some(Duration::from_millis(600))
        );

        // The old flat field still works, and power.shutdown wins over it.
        let cfg = set(
            &p,
            &SettingsPatch {
                power_shutdown: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!get(&cfg).power.shutdown);
        let cfg = set(
            &p,
            &SettingsPatch {
                power_shutdown: Some(false),
                power: Some(PowerPatch {
                    shutdown: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(get(&cfg).power.shutdown);

        // Out of range is refused, the file kept.
        let text = std::fs::read_to_string(&p).unwrap();
        for bad in [
            SettingsPatch {
                logs: Some(LogsPatch {
                    capture_minutes: Some(31),
                    ..Default::default()
                }),
                ..Default::default()
            },
            SettingsPatch {
                logs: Some(LogsPatch {
                    max_pct: Some(4),
                    ..Default::default()
                }),
                ..Default::default()
            },
            SettingsPatch {
                power: Some(PowerPatch {
                    silent_s: Some(14),
                    ..Default::default()
                }),
                ..Default::default()
            },
            SettingsPatch {
                power: Some(PowerPatch {
                    radio_hold_min: Some(121),
                    ..Default::default()
                }),
                ..Default::default()
            },
            SettingsPatch {
                input: Some(InputPatch {
                    long_ms: Some(1501),
                    ..Default::default()
                }),
                ..Default::default()
            },
            SettingsPatch {
                input: Some(InputPatch {
                    extended_s: Some(2),
                    ..Default::default()
                }),
                ..Default::default()
            },
        ] {
            assert!(set(&p, &bad).is_err(), "{bad:?}");
        }
        assert_eq!(std::fs::read_to_string(&p).unwrap(), text);
    }

    #[test]
    fn portal_address_and_version() {
        let any: SocketAddr = "0.0.0.0:5003".parse().unwrap();
        assert_eq!(portal_address(any, None), "http://10.10.0.1:5003");
        assert_eq!(
            portal_address(any, Some(Ipv4Addr::new(10, 42, 0, 1))),
            "http://10.42.0.1:5003"
        );
        let one: SocketAddr = "192.168.1.5:8080".parse().unwrap();
        assert_eq!(portal_address(one, None), "http://192.168.1.5:8080");
        let d = tempfile::tempdir().unwrap();
        let m = d.path().join("manifest.json");
        assert!(version(&m).ends_with("(no release manifest)"));
        std::fs::write(&m, r#"{"version":"2026.11.08","channel":"beta"}"#).unwrap();
        assert!(
            version(&m).ends_with(", release 2026.11.08 (beta)"),
            "{}",
            version(&m)
        );
    }

    #[test]
    fn protect_downloads_needs_the_portal_section() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("carpid.toml");
        std::fs::write(&p, "[bus.ican]\niface = \"can0\"\n").unwrap();
        let e = set(
            &p,
            &SettingsPatch {
                portal: Some(carpi_proto::PortalPatch {
                    protect_downloads: Some(true),
                }),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(e.contains("portal is off"), "{e}");
        assert_eq!(
            get(&Config::parse("[bus.ican]\niface='x'\n").unwrap())
                .portal
                .address,
            None
        );
    }

    #[test]
    fn pin_needs_the_portal_section() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("carpid.toml");
        std::fs::write(&p, "[bus.ican]\niface = \"can0\"\n").unwrap();
        let e = set(
            &p,
            &SettingsPatch {
                portal_pin: Some(PinChange::Set { pin: "4711".into() }),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(e.contains("portal is off"), "{e}");
    }
}

/// The TOML spelling of the `[dis]` enums (their serde snake_case names).
mod erased {
    pub trait Snake {
        fn snake(&self) -> String;
    }
    impl<T: serde::Serialize> Snake for T {
        fn snake(&self) -> String {
            serde_json::to_value(self)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default()
        }
    }
}
