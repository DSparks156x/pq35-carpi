//! The binding table as data: every row is `table, key, gesture, action`
//! with snake_case names, so it can live in `carpid.toml` and travel over
//! the API without the API crate knowing these enums.

use crate::route::{CarAction, DisKey, Page, RouteConfig, SystemAction};
use crate::{GestureKind, Key, LiviKey};
use std::collections::HashMap;

/// One row of [`RouteConfig`].
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct BindingRow {
    pub table: String,
    pub key: String,
    pub gesture: String,
    pub action: String,
}

/// Tables in the order [`crate::route`] checks them.
pub const TABLES: [&str; 7] = [
    "dis_toggle",
    "system",
    "call_ringing",
    "call_active",
    "dis",
    "car",
    "livi",
];

const KEYS: [(Key, &str); 23] = [
    (Key::WheelUp, "wheel_up"),
    (Key::WheelDown, "wheel_down"),
    (Key::WheelClick, "wheel_click"),
    (Key::Mode, "mode"),
    (Key::Ptt, "ptt"),
    (Key::VolUp, "vol_up"),
    (Key::VolDown, "vol_down"),
    (Key::VolClick, "vol_click"),
    (Key::Prev, "prev"),
    (Key::Next, "next"),
    (Key::Up, "up"),
    (Key::Down, "down"),
    (Key::Enter, "enter"),
    (Key::Return, "return"),
    (Key::Setup, "setup"),
    (Key::Nav, "nav"),
    (Key::Tel, "tel"),
    (Key::Media, "media"),
    (Key::Name, "name"),
    (Key::Info, "info"),
    (Key::Car, "car"),
    (Key::Knob(1), "knob_cw"),
    (Key::Knob(-1), "knob_ccw"),
];

const GESTURES: [(GestureKind, &str); 5] = [
    (GestureKind::Short, "short"),
    (GestureKind::Long, "long"),
    (GestureKind::Extended, "extended"),
    (GestureKind::Double, "double"),
    (GestureKind::Detent, "detent"),
];

const SYSTEM: [(SystemAction, &str); 7] = [
    (SystemAction::ToggleDisFocus, "toggle_dis_focus"),
    (SystemAction::Shutdown, "shutdown"),
    (SystemAction::Reboot, "reboot"),
    (SystemAction::SaveCapture, "save_capture"),
    (SystemAction::BrightnessUp, "brightness_up"),
    (SystemAction::BrightnessDown, "brightness_down"),
    (SystemAction::ToggleLogging, "toggle_logging"),
];

const CAR: [(CarAction, &str); 7] = [
    (CarAction::OpenNav, "open_nav"),
    (CarAction::OpenPhone, "open_phone"),
    (CarAction::OpenMedia, "open_media"),
    (CarAction::OpenPage(Page::Data), "open_page_data"),
    (CarAction::OpenPage(Page::Car), "open_page_car"),
    (CarAction::CallAccept, "call_accept"),
    (CarAction::CallEnd, "call_end"),
];

const DIS: [(DisKey, &str); 4] = [
    (DisKey::Up, "up"),
    (DisKey::Down, "down"),
    (DisKey::Select, "select"),
    (DisKey::Back, "back"),
];

const LIVI: [(LiviKey, &str); 16] = [
    (LiviKey::Up, "up"),
    (LiviKey::Down, "down"),
    (LiviKey::Left, "left"),
    (LiviKey::Right, "right"),
    (LiviKey::Select, "select"),
    (LiviKey::Back, "back"),
    (LiviKey::Home, "home"),
    (LiviKey::PlayPause, "play_pause"),
    (LiviKey::Next, "next"),
    (LiviKey::Prev, "prev"),
    (LiviKey::AcceptPhone, "accept_phone"),
    (LiviKey::RejectPhone, "reject_phone"),
    (LiviKey::Voice, "voice"),
    (LiviKey::CycleSession, "cycle_session"),
    (LiviKey::KnobLeft, "knob_left"),
    (LiviKey::KnobRight, "knob_right"),
];

fn name_of<T: PartialEq + Copy>(table: &[(T, &'static str)], v: T) -> &'static str {
    table
        .iter()
        .find(|(x, _)| *x == v)
        .map(|(_, n)| *n)
        .expect("every variant is named")
}

fn parse<T: Copy>(table: &[(T, &'static str)], what: &str, s: &str) -> Result<T, String> {
    table
        .iter()
        .find(|(_, n)| *n == s)
        .map(|(v, _)| *v)
        .ok_or_else(|| {
            let names: Vec<&str> = table.iter().map(|(_, n)| *n).collect();
            format!("unknown {what} '{s}' (one of: {})", names.join(", "))
        })
}

fn key_name(k: Key) -> String {
    match KEYS.iter().find(|(x, _)| *x == k) {
        Some((_, n)) => (*n).into(),
        // Multi-detent steps never come out of the recogniser; named anyway.
        None => match k {
            Key::Knob(n) => format!("knob:{n}"),
            _ => unreachable!("every key is named"),
        },
    }
}

fn parse_key(s: &str) -> Result<Key, String> {
    parse(&KEYS, "key", s)
}

fn livi_name(k: LiviKey) -> String {
    match k {
        LiviKey::Code(c) => format!("code:{c}"),
        k => name_of(&LIVI, k).into(),
    }
}

fn parse_livi(s: &str) -> Result<LiviKey, String> {
    if let Some(c) = s.strip_prefix("code:") {
        return c
            .parse()
            .map(LiviKey::Code)
            .map_err(|_| format!("bad key code '{s}'"));
    }
    parse(&LIVI, "LIVI key", s)
}

fn rows<T: Copy>(
    out: &mut Vec<BindingRow>,
    table: &str,
    map: &HashMap<(Key, GestureKind), T>,
    name: impl Fn(T) -> String,
) {
    for (&(k, g), &a) in map {
        out.push(BindingRow {
            table: table.into(),
            key: key_name(k),
            gesture: name_of(&GESTURES, g).into(),
            action: name(a),
        });
    }
}

impl RouteConfig {
    /// Every binding as a row, in table order, then key and gesture name.
    pub fn to_rows(&self) -> Vec<BindingRow> {
        let mut out = vec![BindingRow {
            table: "dis_toggle".into(),
            key: key_name(self.dis_toggle.0),
            gesture: name_of(&GESTURES, self.dis_toggle.1).into(),
            action: "toggle".into(),
        }];
        rows(&mut out, "system", &self.system, |a| {
            name_of(&SYSTEM, a).into()
        });
        rows(&mut out, "call_ringing", &self.call_ringing, |a| {
            name_of(&CAR, a).into()
        });
        rows(&mut out, "call_active", &self.call_active, |a| {
            name_of(&CAR, a).into()
        });
        rows(&mut out, "dis", &self.dis, |a| name_of(&DIS, a).into());
        rows(&mut out, "car", &self.car, |a| name_of(&CAR, a).into());
        rows(&mut out, "livi", &self.livi, livi_name);
        let order = |t: &str| TABLES.iter().position(|x| *x == t).unwrap_or(usize::MAX);
        out.sort_by(|a, b| {
            (order(&a.table), &a.key, &a.gesture).cmp(&(order(&b.table), &b.key, &b.gesture))
        });
        out
    }

    /// Builds a table from rows. Needs exactly one `dis_toggle` row; a
    /// trigger may appear once per table.
    pub fn from_rows(rows: &[BindingRow]) -> Result<RouteConfig, String> {
        let mut cfg = RouteConfig {
            dis_toggle: (Key::Mode, GestureKind::Long),
            system: HashMap::new(),
            call_ringing: HashMap::new(),
            call_active: HashMap::new(),
            dis: HashMap::new(),
            car: HashMap::new(),
            livi: HashMap::new(),
        };
        let mut toggles = 0;
        for (i, r) in rows.iter().enumerate() {
            let at = |e: String| {
                format!(
                    "binding {} ({} {} {}): {e}",
                    i + 1,
                    r.table,
                    r.key,
                    r.gesture
                )
            };
            let t = (
                parse_key(&r.key).map_err(at)?,
                parse(&GESTURES, "gesture", &r.gesture).map_err(at)?,
            );
            let dup = |e: bool| {
                if e {
                    Err(at("trigger already bound in this table".into()))
                } else {
                    Ok(())
                }
            };
            match r.table.as_str() {
                "dis_toggle" => {
                    if r.action != "toggle" {
                        return Err(at("the dis_toggle action is 'toggle'".into()));
                    }
                    toggles += 1;
                    cfg.dis_toggle = t;
                }
                "system" => dup(cfg
                    .system
                    .insert(t, parse(&SYSTEM, "action", &r.action).map_err(at)?)
                    .is_some())?,
                "call_ringing" => dup(cfg
                    .call_ringing
                    .insert(t, parse(&CAR, "action", &r.action).map_err(at)?)
                    .is_some())?,
                "call_active" => dup(cfg
                    .call_active
                    .insert(t, parse(&CAR, "action", &r.action).map_err(at)?)
                    .is_some())?,
                "dis" => dup(cfg
                    .dis
                    .insert(t, parse(&DIS, "action", &r.action).map_err(at)?)
                    .is_some())?,
                "car" => dup(cfg
                    .car
                    .insert(t, parse(&CAR, "action", &r.action).map_err(at)?)
                    .is_some())?,
                "livi" => dup(cfg
                    .livi
                    .insert(t, parse_livi(&r.action).map_err(at)?)
                    .is_some())?,
                other => {
                    return Err(at(format!(
                        "unknown table '{other}' (one of: {})",
                        TABLES.join(", ")
                    )));
                }
            }
        }
        if toggles != 1 {
            return Err(format!(
                "need exactly one dis_toggle binding, got {toggles}"
            ));
        }
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_table_roundtrips() {
        let d = RouteConfig::default();
        let rows = d.to_rows();
        assert_eq!(rows[0].table, "dis_toggle");
        let back = RouteConfig::from_rows(&rows).unwrap();
        assert_eq!(back.to_rows(), rows);
        assert_eq!(back.livi, d.livi);
        assert_eq!(back.system, d.system);
    }

    #[test]
    fn rejects_bad_rows() {
        let mut rows = RouteConfig::default().to_rows();
        rows.push(BindingRow {
            table: "livi".into(),
            key: "wheel_sideways".into(),
            gesture: "short".into(),
            action: "home".into(),
        });
        let e = RouteConfig::from_rows(&rows).unwrap_err();
        assert!(e.contains("unknown key 'wheel_sideways'"), "{e}");
        let mut rows = RouteConfig::default().to_rows();
        rows.remove(0);
        assert!(RouteConfig::from_rows(&rows).is_err());
        let rows = vec![
            BindingRow {
                table: "dis_toggle".into(),
                key: "mode".into(),
                gesture: "long".into(),
                action: "toggle".into(),
            },
            BindingRow {
                table: "livi".into(),
                key: "setup".into(),
                gesture: "short".into(),
                action: "code:59".into(),
            },
        ];
        let c = RouteConfig::from_rows(&rows).unwrap();
        assert_eq!(
            c.livi.get(&(Key::Setup, GestureKind::Short)),
            Some(&LiviKey::Code(59))
        );
    }

    #[test]
    fn brightness_actions_bind_but_not_by_default() {
        let d = RouteConfig::default();
        assert!(
            d.system
                .values()
                .all(|a| !matches!(a, SystemAction::BrightnessUp | SystemAction::BrightnessDown))
        );
        let mut rows = d.to_rows();
        rows.push(BindingRow {
            table: "system".into(),
            key: "wheel_up".into(),
            gesture: "long".into(),
            action: "brightness_up".into(),
        });
        let c = RouteConfig::from_rows(&rows).unwrap();
        assert_eq!(
            c.system.get(&(Key::WheelUp, GestureKind::Long)),
            Some(&SystemAction::BrightnessUp)
        );
        assert!(c.to_rows().iter().any(|r| r.action == "brightness_up"));
    }
}
