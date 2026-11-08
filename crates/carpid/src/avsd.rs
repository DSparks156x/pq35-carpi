//! The A/V-over-SD chain's settings: `~/avsd/avsd.env`, plain `NAME=value`
//! lines that `hud2.sh` sources as shell at chain start. Ranges and meanings
//! are sdlink's docs/DEPLOYMENT.md, "Settings"; defaults are `hud2.sh`'s.
//!
//! An edit writes only known keys and keeps comments and every other line.
//! `TONEPAL` and `NURELAX` are flags inside `ENCEXTRA`; its other flags stay.
//! The chain reads the file only at start, so a change restarts its unit.

use carpi_proto::{AvsdRange, AvsdSettings};
use std::collections::BTreeMap;
use std::path::Path;

struct Key {
    key: &'static str,
    label: &'static str,
    unit: &'static str,
    min: f64,
    max: f64,
    step: f64,
    /// The shipped avsd.env value.
    default: f64,
    /// What the chain uses when the file doesn't set it.
    missing: f64,
    zero_off: bool,
    help: &'static str,
}

const KEYS: &[Key] = &[
    Key {
        key: "GAIN",
        label: "Audio gain",
        unit: "dB",
        min: -30.0,
        max: -3.0,
        step: 0.5,
        default: -3.0,
        missing: -3.0,
        zero_off: false,
        help: "Audio level sent to the RNS-E. Use -6 with any tone or EQ boost",
    },
    Key {
        key: "LINKCAP",
        label: "Video rate",
        unit: "B/s",
        min: 3.2e6,
        max: 4.5e6,
        step: 0.1e6,
        default: 4.5e6,
        missing: 4.5e6,
        zero_off: false,
        help: "Video rate target. Over 4.5e6: stutter or audio gaps",
    },
    Key {
        key: "MAXVID",
        label: "Largest video frame",
        unit: "sectors",
        min: 130.0,
        max: 161.0,
        step: 1.0,
        default: 161.0,
        missing: 161.0,
        zero_off: false,
        help: "Largest video frame per superframe (x512 B). Over 161 breaks the stream",
    },
    Key {
        key: "CYC",
        label: "Decode-time cap",
        unit: "cycles",
        min: 3.0e6,
        max: 4.05e6,
        step: 0.05e6,
        default: 4.05e6,
        missing: 4.05e6,
        zero_off: false,
        help: "Lower is steadier audio, higher a better heavy-screen picture. Lower it first for skips",
    },
    Key {
        key: "KEYINT",
        label: "Full refresh every",
        unit: "frames",
        min: 240.0,
        max: 3600.0,
        step: 60.0,
        default: 0.0,
        missing: 0.0,
        zero_off: true,
        help: "Periodic full refresh. 0: only when the link asks",
    },
    Key {
        key: "ENCTHREADS",
        label: "Encoder threads",
        unit: "",
        min: 2.0,
        max: 3.0,
        step: 1.0,
        default: 2.0,
        missing: 2.0,
        zero_off: false,
        help: "Encoder threads at real-time priority. 3 gains little and takes CPU from LIVI",
    },
    Key {
        key: "DITHER",
        label: "Dither",
        unit: "",
        min: 0.0,
        max: 1.0,
        step: 1.0,
        default: 0.0,
        missing: 0.0,
        zero_off: false,
        help: "Screen-locked dither on smooth areas. Costs decode time",
    },
    Key {
        key: "SRCDEAD",
        label: "Ignore changes under",
        unit: "levels",
        min: 0.0,
        max: 6.0,
        step: 1.0,
        default: 3.0,
        missing: 3.0,
        zero_off: false,
        help: "Source changes this small are ignored, which stops noise flicker",
    },
    Key {
        key: "SRCMEAN",
        label: "Noise mean",
        unit: "1/64 level",
        min: 12.0,
        max: 64.0,
        step: 1.0,
        default: 48.0,
        missing: 48.0,
        zero_off: false,
        help: "The block mean must stay within this to count as noise. Low: more updates",
    },
    Key {
        key: "FILLPEN",
        label: "Flat fill penalty",
        unit: "",
        min: 1.0,
        max: 4.0,
        step: 1.0,
        default: 2.0,
        missing: 2.0,
        zero_off: false,
        help: "Higher: fewer mosaics; lower: better worst frames",
    },
    Key {
        key: "TONEPAL",
        label: "Palette snap",
        unit: "codes",
        min: 2.0,
        max: 4.0,
        step: 1.0,
        default: 3.0,
        missing: 0.0,
        zero_off: true,
        help: "Flat pale colours (Maps land) lose the phone stream's speckle. 3 is tested, 0 is off",
    },
    Key {
        key: "NURELAX",
        label: "Smoother transitions",
        unit: "",
        min: 0.0,
        max: 1.0,
        step: 1.0,
        default: 1.0,
        missing: 0.0,
        zero_off: false,
        help: "Frames may use spare decode cycles, for fewer flat fills on transitions",
    },
];

/// Flags `NURELAX` on writes.
const NURELAX_ON: [&str; 4] = ["--nurelax", "0.5", "--nurelaxthr", "0.97"];

fn find(key: &str) -> Option<&'static Key> {
    KEYS.iter().find(|k| k.key == key)
}

pub fn ranges() -> Vec<AvsdRange> {
    KEYS.iter()
        .map(|k| AvsdRange {
            key: k.key.into(),
            label: k.label.into(),
            unit: k.unit.into(),
            min: k.min,
            max: k.max,
            step: k.step,
            default: k.default,
            zero_off: k.zero_off,
            help: k.help.into(),
        })
        .collect()
}

/// `NAME=value` on this line: (name, raw value, trailing comment).
fn assignment(line: &str) -> Option<(&str, &str, &str)> {
    let t = line.trim_start();
    if t.starts_with('#') {
        return None;
    }
    let (name, rest) = t.split_once('=')?;
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    // A quoted value runs to its closing quote; a bare one to whitespace.
    let end = match rest.chars().next() {
        Some(q @ ('"' | '\'')) => rest[1..].find(q).map_or(rest.len(), |i| i + 2),
        _ => rest.find(char::is_whitespace).unwrap_or(rest.len()),
    };
    Some((name, &rest[..end], &rest[end..]))
}

fn unquote(v: &str) -> &str {
    let b = v.as_bytes();
    if b.len() >= 2 && (b[0] == b'"' || b[0] == b'\'') && b[b.len() - 1] == b[0] {
        &v[1..v.len() - 1]
    } else {
        v
    }
}

/// The last line setting `name` (the one the shell keeps).
fn last(text: &str, name: &str) -> Option<String> {
    text.lines()
        .rev()
        .filter_map(assignment)
        .find(|(n, _, _)| *n == name)
        .map(|(_, v, _)| unquote(v).to_string())
}

fn flag(tokens: &[String], name: &str) -> Option<f64> {
    let i = tokens.iter().position(|t| t == name)?;
    tokens.get(i + 1)?.parse().ok()
}

/// Every key's value as the chain would use it.
pub fn values(text: &str) -> BTreeMap<String, f64> {
    let extra: Vec<String> = last(text, "ENCEXTRA")
        .unwrap_or_default()
        .split_whitespace()
        .map(String::from)
        .collect();
    KEYS.iter()
        .map(|k| {
            let v = match k.key {
                "TONEPAL" => flag(&extra, "--tonepal").unwrap_or(0.0),
                "NURELAX" => match flag(&extra, "--nurelax") {
                    Some(x) if x != 0.0 => 1.0,
                    _ => 0.0,
                },
                _ => last(text, k.key)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(k.missing),
            };
            (k.key.to_string(), v)
        })
        .collect()
}

pub fn check(key: &str, v: f64) -> Result<(), String> {
    let k = find(key).ok_or_else(|| format!("avsd: unknown setting {key}"))?;
    if !v.is_finite() {
        return Err(format!("avsd {key}: not a number"));
    }
    if k.zero_off && v == 0.0 {
        return Ok(());
    }
    let steps = (v - k.min) / k.step;
    if v < k.min - 1e-9 || v > k.max + 1e-9 || (steps - steps.round()).abs() > 1e-6 {
        let off = if k.zero_off { " or 0" } else { "" };
        return Err(format!(
            "avsd {key}: {}-{} in steps of {}{off}, got {}",
            num(k.min),
            num(k.max),
            num(k.step),
            num(v)
        ));
    }
    Ok(())
}

/// Shell-safe, and in the file's own style (`4.5e6`, `-3`).
fn num(v: f64) -> String {
    if v.abs() >= 1e5 {
        let m = (v / 1e6 * 1e4).round() / 1e4;
        format!("{m}e6")
    } else if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{}", (v * 1e4).round() / 1e4)
    }
}

/// Sets `name` on its last line, or appends a line.
fn set_line(lines: &mut Vec<String>, name: &str, value: &str) {
    let at = lines
        .iter()
        .rposition(|l| assignment(l).is_some_and(|(n, _, _)| n == name));
    match at {
        Some(i) => {
            let (_, _, tail) = assignment(&lines[i]).unwrap_or_default();
            let indent = &lines[i][..lines[i].len() - lines[i].trim_start().len()];
            lines[i] = format!("{indent}{name}={value}{tail}");
        }
        None => lines.push(format!("{name}={value}")),
    }
}

/// Drops `name` and the value after it.
fn drop_flag(tokens: &mut Vec<String>, name: &str) {
    while let Some(i) = tokens.iter().position(|t| t == name) {
        let n = if tokens.get(i + 1).is_some_and(|t| !t.starts_with("--")) {
            2
        } else {
            1
        };
        tokens.drain(i..(i + n).min(tokens.len()));
    }
}

/// `text` with `changes` applied. Checks every value first.
pub fn edit(text: &str, changes: &BTreeMap<String, f64>) -> Result<String, String> {
    for (k, &v) in changes {
        check(k, v)?;
    }
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    let mut extra: Option<Vec<String>> = None;
    for (k, &v) in changes {
        match k.as_str() {
            "TONEPAL" | "NURELAX" => {
                let t = extra.get_or_insert_with(|| {
                    last(text, "ENCEXTRA")
                        .unwrap_or_default()
                        .split_whitespace()
                        .map(String::from)
                        .collect()
                });
                if k == "TONEPAL" {
                    if v == 0.0 {
                        drop_flag(t, "--tonepal");
                    } else if let Some(i) = t.iter().position(|x| x == "--tonepal") {
                        if t.get(i + 1).is_some_and(|x| !x.starts_with("--")) {
                            t[i + 1] = num(v);
                        } else {
                            t.insert(i + 1, num(v));
                        }
                    } else {
                        t.extend(["--tonepal".to_string(), num(v)]);
                    }
                } else if v == 0.0 {
                    drop_flag(t, "--nurelax");
                    drop_flag(t, "--nurelaxthr");
                } else if flag(t, "--nurelax").is_none_or(|x| x == 0.0) {
                    drop_flag(t, "--nurelax");
                    drop_flag(t, "--nurelaxthr");
                    t.extend(NURELAX_ON.iter().map(|s| s.to_string()));
                }
            }
            _ => set_line(&mut lines, k, &num(v)),
        }
    }
    if let Some(t) = extra {
        let joined = t.join(" ");
        if joined.contains(['"', '$', '`', '\\']) {
            return Err("avsd: ENCEXTRA has quotes or $ in it; edit it by hand".into());
        }
        set_line(&mut lines, "ENCEXTRA", &format!("\"{joined}\""));
    }
    let mut out = lines.join("\n");
    if text.ends_with('\n') || text.is_empty() {
        out.push('\n');
    }
    Ok(out)
}

/// The settings for the API: the file's values, or the defaults without one.
pub fn read(path: &Path) -> AvsdSettings {
    let text = std::fs::read_to_string(path);
    AvsdSettings {
        path: path.display().to_string(),
        found: text.is_ok(),
        values: values(text.as_deref().unwrap_or("")),
        ranges: ranges(),
    }
}

/// Edit the file in place (atomic, with a backup). Unchanged: no write.
/// True if it changed.
pub fn save(path: &Path, changes: &BTreeMap<String, f64>) -> Result<bool, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("can't read {}: {e}", path.display()))?;
    let new = edit(&text, changes)?;
    if new == text {
        return Ok(false);
    }
    carpi_portal::replace_with_backup(path, &new)
        .map_err(|e| format!("saving {}: {e}", path.display()))?;
    Ok(true)
}

/// Restarts the chain's user unit, off the event loop.
pub fn restart(unit: String) {
    tokio::spawn(async move {
        let r = tokio::process::Command::new("systemctl")
            .args(["--user", "restart", &unit])
            .status()
            .await;
        match r {
            Ok(s) if s.success() => tracing::info!("avsd: restarted {unit}"),
            Ok(s) => tracing::warn!("avsd: systemctl --user restart {unit}: {s}"),
            Err(e) => tracing::warn!("avsd: systemctl --user restart {unit}: {e}"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// sdlink's pi/run/avsd.env, plus an unknown line.
    const FILE: &str = "# Settings for the A/V-over-SD chain.\n\
GAIN=-3\n\
LINKCAP=4.5e6\n\
MAXVID=161\n\
CYC=4.05e6\n\
KEYINT=0\n\
ENCTHREADS=2\n\
DITHER=0\n\
SRCDEAD=3\n\
SRCMEAN=48\n\
FILLPEN=2\n\
# Flat pale colours\n\
ENCEXTRA=\"--tonepal 3 --nurelax 0.5 --nurelaxthr 0.97\"\n\
# Front end. Unset = Hudiy. For LIVI:\n\
APP_PROC=livi-core\n\
#CAP_SINK=avsd_out\n";

    fn ch(kv: &[(&str, f64)]) -> BTreeMap<String, f64> {
        kv.iter().map(|&(k, v)| (k.to_string(), v)).collect()
    }

    #[test]
    fn reads_the_shipped_file() {
        let v = values(FILE);
        assert_eq!(v["GAIN"], -3.0);
        assert_eq!(v["LINKCAP"], 4.5e6);
        assert_eq!(v["TONEPAL"], 3.0);
        assert_eq!(v["NURELAX"], 1.0);
        // Every shipped value is its range's default and valid.
        for r in ranges() {
            assert_eq!(v[&r.key], r.default, "{}", r.key);
            check(&r.key, r.default).unwrap();
        }
        // Missing lines: hud2.sh's defaults; no ENCEXTRA: both flags off.
        let v = values("APP_PROC=livi-core\n");
        assert_eq!((v["CYC"], v["TONEPAL"], v["NURELAX"]), (4.05e6, 0.0, 0.0));
    }

    #[test]
    fn edit_keeps_comments_and_unknown_lines() {
        let out = edit(
            FILE,
            &ch(&[
                ("GAIN", -6.0),
                ("CYC", 3.5e6),
                ("TONEPAL", 0.0),
                ("NURELAX", 0.0),
            ]),
        )
        .unwrap();
        assert!(out.contains("GAIN=-6\n"), "{out}");
        assert!(out.contains("CYC=3.5e6\n"), "{out}");
        assert!(out.contains("ENCEXTRA=\"\"\n"), "{out}");
        for keep in [
            "# Settings for the A/V-over-SD chain.\n",
            "# Flat pale colours\n",
            "APP_PROC=livi-core\n",
            "#CAP_SINK=avsd_out\n",
            "LINKCAP=4.5e6\n",
        ] {
            assert!(out.contains(keep), "{keep:?} lost:\n{out}");
        }
        assert_eq!(out.lines().count(), FILE.lines().count());
        let v = values(&out);
        assert_eq!(
            (v["GAIN"], v["CYC"], v["TONEPAL"], v["NURELAX"]),
            (-6.0, 3.5e6, 0.0, 0.0)
        );
        // Back on: the other ENCEXTRA flags stay, the line is added if gone.
        let out = edit(
            "GAIN=-3 # car\nENCEXTRA=\"--foo 1\"\n",
            &ch(&[("TONEPAL", 2.0), ("NURELAX", 1.0), ("KEYINT", 240.0)]),
        )
        .unwrap();
        assert_eq!(
            out,
            "GAIN=-3 # car\nENCEXTRA=\"--foo 1 --nurelax 0.5 --nurelaxthr 0.97 --tonepal 2\"\nKEYINT=240\n"
        );
        // No change, same text.
        assert_eq!(edit(FILE, &ch(&[("GAIN", -3.0)])).unwrap(), FILE);
    }

    #[test]
    fn refuses_bad_values() {
        for (k, v) in [
            ("GAIN", 0.0),
            ("GAIN", -2.5),
            ("MAXVID", 170.0),
            ("KEYINT", 100.0),
            ("ENCTHREADS", 2.5),
            ("TONEPAL", 1.0),
            ("RTPRIO", 5.0),
            ("ENCFLAGS", 1.0),
            ("LINKCAP", f64::NAN),
        ] {
            assert!(edit(FILE, &ch(&[(k, v)])).is_err(), "{k}={v}");
        }
        check("KEYINT", 0.0).unwrap();
        check("TONEPAL", 0.0).unwrap();
    }

    #[test]
    fn save_writes_only_on_change() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("avsd.env");
        std::fs::write(&p, FILE).unwrap();
        assert!(!save(&p, &ch(&[("DITHER", 0.0)])).unwrap());
        assert!(save(&p, &ch(&[("DITHER", 1.0)])).unwrap());
        let s = read(&p);
        assert!(s.found);
        assert_eq!(s.values["DITHER"], 1.0);
        assert!(!read(&d.path().join("none")).found);
    }
}
