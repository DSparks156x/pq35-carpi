//! `carpi nav-trace`: nav as carpid holds it, one JSON line per change.
//!
//! Each line is the whole `nav.*` / `projection.*` picture after the change,
//! so a trace can be read (or replayed) from any line on:
//!
//! ```text
//! {"unix_ms":1762600000123,"t_ms":4100,"changed":["nav.distance_m"],"state":{"nav.distance_m":250.0,...}}
//! ```
//!
//! `nav.lanes` is written as JSON rather than a string, and the turn icon as its
//! size only (`nav.icon_png_base64_len`).

#![cfg_attr(not(unix), allow(dead_code))]

use carpi_proto::{Reading, Value};
use serde_json::{Map, Value as Json, json};
use std::collections::BTreeMap;

#[cfg_attr(not(unix), allow(dead_code))]
pub const PREFIXES: [&str; 2] = ["nav.", "projection."];

const ICON: &str = "nav.icon_png_base64";

/// What carpid has told us so far.
#[derive(Default)]
pub struct Trace {
    state: BTreeMap<String, Json>,
}

impl Trace {
    /// Folds in one update; the line to write, or `None` when nothing changed.
    pub fn line(&mut self, items: &[Reading], unix_ms: u128, t_ms: u128) -> Option<String> {
        let mut changed = Vec::new();
        for r in items {
            let (name, value) = shown(r);
            let before = self.state.get(&name);
            if before == value.as_ref() {
                continue;
            }
            match value {
                Some(v) => self.state.insert(name.clone(), v),
                None => self.state.remove(&name),
            };
            changed.push(name);
        }
        if changed.is_empty() {
            return None;
        }
        let state: Map<String, Json> = self.state.clone().into_iter().collect();
        Some(
            json!({
                "unix_ms": unix_ms as u64,
                "t_ms": t_ms as u64,
                "changed": changed,
                "state": state,
            })
            .to_string(),
        )
    }
}

fn shown(r: &Reading) -> (String, Option<Json>) {
    let value = r.value.as_ref().map(|v| match v {
        Value::Bool(b) => json!(b),
        Value::Num(n) => json!(n),
        Value::Text(s) if r.name == "nav.lanes" => {
            serde_json::from_str(s).unwrap_or_else(|_| json!(s))
        }
        Value::Text(s) if r.name == ICON => json!(s.len()),
        Value::Text(s) => json!(s),
    });
    let name = if r.name == ICON {
        format!("{ICON}_len")
    } else {
        r.name.clone()
    };
    (name, value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(name: &str, value: Option<Value>) -> Reading {
        Reading {
            name: name.into(),
            value,
            age_ms: 0,
        }
    }

    #[test]
    fn a_line_per_change_with_the_whole_picture() {
        let mut t = Trace::default();
        let first = t
            .line(
                &[
                    reading("nav.active", Some(Value::Bool(true))),
                    reading("nav.distance_m", Some(Value::Num(350.0))),
                    reading(
                        "nav.lanes",
                        Some(Value::Text(
                            r#"[{"arrows":[{"shape":"straight","highlighted":true}],"status":null}]"#
                                .into(),
                        )),
                    ),
                    reading(ICON, Some(Value::Text("iVBORw0K".into()))),
                    reading("nav.eta", None),
                ],
                1_000,
                0,
            )
            .unwrap();
        let v: Json = serde_json::from_str(&first).unwrap();
        assert_eq!(v["state"]["nav.distance_m"], json!(350.0));
        assert_eq!(
            v["state"]["nav.lanes"][0]["arrows"][0]["shape"],
            json!("straight")
        );
        assert_eq!(v["state"]["nav.icon_png_base64_len"], json!(8));
        assert!(v["state"].get("nav.eta").is_none());
        assert_eq!(v["changed"].as_array().unwrap().len(), 4);

        // Same values again: nothing to write.
        assert!(
            t.line(
                &[reading("nav.distance_m", Some(Value::Num(350.0)))],
                1_100,
                100
            )
            .is_none()
        );
        let next = t
            .line(
                &[
                    reading("nav.distance_m", Some(Value::Num(300.0))),
                    reading("nav.active", None),
                ],
                1_200,
                200,
            )
            .unwrap();
        let v: Json = serde_json::from_str(&next).unwrap();
        assert_eq!(v["changed"], json!(["nav.distance_m", "nav.active"]));
        assert_eq!(v["state"]["nav.distance_m"], json!(300.0));
        assert!(v["state"].get("nav.active").is_none());
        assert_eq!(v["t_ms"], json!(200));
    }
}
