//! LIVI bridge state into the value store.
//!
//! Every section lands as plain values so the DIS, light model and pages read
//! nav/media/calls the same way they read CAN signals. Bridge values don't
//! time out; they are cleared when LIVI's connection closes or it pushes
//! `None`.

use carpi_proto::BridgeUpdate;
use carpi_state::{Store, Value};
use std::time::{Duration, Instant};

/// Bridge values stay until replaced or cleared.
const FOREVER: Duration = Duration::from_secs(365 * 24 * 3600);

pub const SECTIONS: [&str; 6] = ["projection.", "nav.", "media.", "call.", "phone.", "ui."];

fn text(v: &Option<String>) -> Option<Value> {
    v.clone().map(Value::Text)
}

fn num<T: Into<f64> + Copy>(v: Option<T>) -> Option<Value> {
    v.map(|n| Value::Num(n.into()))
}

pub fn apply(store: &mut Store, update: &BridgeUpdate, now: Instant) {
    let mut set = |name: &str, v: Option<Value>| store.set(name, v, now, FOREVER);
    match update {
        BridgeUpdate::Projection { projection } => {
            set(
                "projection.connected",
                Some(Value::Bool(projection.is_some())),
            );
            set(
                "projection.kind",
                projection.as_ref().map(|p| Value::Text(p.kind.clone())),
            );
            set(
                "projection.wireless",
                projection.as_ref().map(|p| Value::Bool(p.wireless)),
            );
        }
        BridgeUpdate::Nav { nav } => {
            set("nav.active", Some(Value::Bool(nav.is_some())));
            let n = nav.clone().unwrap_or_default();
            set("nav.maneuver", text(&n.maneuver));
            set("nav.side", text(&n.side));
            set("nav.roundabout_exit", num(n.roundabout_exit));
            set("nav.roundabout_angle", num(n.roundabout_angle));
            set("nav.distance_m", num(n.distance_m));
            set("nav.distance_text", text(&n.distance_text));
            set("nav.time_to_maneuver_s", num(n.time_to_maneuver_s));
            set("nav.road", text(&n.road));
            set("nav.cue", text(&n.cue));
            set("nav.destination", text(&n.destination));
            set("nav.eta", text(&n.eta));
            let lanes = (!n.lanes.is_empty())
                .then(|| serde_json::to_string(&n.lanes).ok())
                .flatten()
                .map(Value::Text);
            set("nav.lanes", lanes);
        }
        BridgeUpdate::NavIcon { png_base64 } => set("nav.icon_png_base64", text(png_base64)),
        BridgeUpdate::Media { media } => {
            let m = media.clone().unwrap_or_default();
            set("media.active", Some(Value::Bool(media.is_some())));
            set("media.source", text(&m.source));
            set("media.title", text(&m.title));
            set("media.artist", text(&m.artist));
            set("media.album", text(&m.album));
            set(
                "media.playing",
                media.as_ref().map(|m| Value::Bool(m.playing)),
            );
            set(
                "media.position_ms",
                m.position_ms.map(|v| Value::Num(v as f64)),
            );
            set(
                "media.duration_ms",
                m.duration_ms.map(|v| Value::Num(v as f64)),
            );
        }
        BridgeUpdate::Call { call } => {
            set(
                "call.state",
                call.as_ref().map(|c| Value::Text(c.state.clone())),
            );
            set("call.number", call.as_ref().and_then(|c| text(&c.number)));
            set("call.name", call.as_ref().and_then(|c| text(&c.name)));
        }
        BridgeUpdate::Phone { phone } => {
            let p = phone.clone().unwrap_or_default();
            set("phone.battery_pct", num(p.battery_pct));
            set("phone.signal_pct", num(p.signal_pct));
        }
        BridgeUpdate::Focus { focus } => set("ui.focus", Some(Value::Text(focus.clone()))),
    }
}

/// LIVI went away: everything it told us is unknown now.
pub fn clear(store: &mut Store, now: Instant) {
    let names: Vec<String> = SECTIONS
        .iter()
        .flat_map(|p| store.snapshot(p, now).into_iter().map(|r| r.name))
        .collect();
    for name in names {
        store.set(&name, None, now, FOREVER);
    }
    store.set(
        "projection.connected",
        Some(Value::Bool(false)),
        now,
        FOREVER,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use carpi_proto::{Lane, NavState};

    #[test]
    fn nav_lands_and_clears() {
        let now = Instant::now();
        let mut s = Store::new();
        let nav = NavState {
            maneuver: Some("turn".into()),
            side: Some("left".into()),
            distance_m: Some(350.0),
            road: Some("Hauptstrasse".into()),
            lanes: vec![Lane {
                shapes: vec!["straight".into()],
                highlighted: true,
            }],
            ..Default::default()
        };
        apply(&mut s, &BridgeUpdate::Nav { nav: Some(nav) }, now);
        assert_eq!(s.get_num("nav.distance_m", now), Some(350.0));
        assert_eq!(s.get_bool("nav.active", now), Some(true));
        assert!(s.get("nav.lanes", now).is_some());
        // Doesn't time out.
        assert_eq!(
            s.get_num("nav.distance_m", now + Duration::from_secs(3600)),
            Some(350.0)
        );

        apply(&mut s, &BridgeUpdate::Nav { nav: None }, now);
        assert_eq!(s.get_bool("nav.active", now), Some(false));
        assert_eq!(s.get("nav.distance_m", now), None);

        apply(
            &mut s,
            &BridgeUpdate::Focus {
                focus: "projection".into(),
            },
            now,
        );
        clear(&mut s, now);
        assert_eq!(s.get("ui.focus", now), None);
        assert_eq!(s.get_bool("projection.connected", now), Some(false));
    }
}
