//! `Request::Settings`: a typed view of the parts of `carpid.toml` the pages
//! may change. `Set` edits the file in place (comments and other keys kept),
//! checks the result with the loader's own rules, then replaces the file
//! atomically with a backup beside it. A rejected patch leaves the file alone.
//!
//! Everything here applies live (see `main.rs`); `restart_needed` stays for
//! settings that can't.

use crate::config::Config;
use carpi_proto::{
    AmbientSource, Binding, LearnedPoint, LightPatch, PinChange, ReadingSetting, Settings,
    SettingsPatch,
};
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
        portal_pin_set: cfg
            .portal
            .as_ref()
            .and_then(|p| p.pin.as_deref())
            .is_some_and(|p| !p.is_empty()),
    }
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
    if let Some(v) = patch.power_shutdown {
        table(&mut doc, "power")?;
        doc["power"]["shutdown"] = value(v);
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
    if let Some(b) = &patch.bindings {
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
