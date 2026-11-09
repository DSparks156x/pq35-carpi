//! The A/V-over-SD stream's state, as `avsd.*` values: the chain's user
//! unit, the Teensy on USB, and avlink's STAT line (every 5 s in
//! `$XDG_RUNTIME_DIR/avsd/live.log`).
//!
//! | name                | value                                                  |
//! |---------------------|--------------------------------------------------------|
//! | `avsd.state`        | Off, Failed, Waiting for Teensy, Starting, Streaming, Stalled |
//! | `avsd.teensy`       | the Teensy is on USB                                   |
//! | `avsd.fps`          | video frames in per second, since the last STAT        |
//! | `avsd.link_kbs`     | kB/s the Teensy served to the RNS-E                    |
//! | `avsd.cap_kbs`      | the encoder's link cap now, kB/s                       |
//! | `avsd.dropped`      | frames dropped since the stream started                |
//! | `avsd.stalls`       | reader stalls since the stream started                 |
//!
//! Nothing is published when the unit isn't installed.

use carpi_state::Value;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How often it's read. avlink writes every 5 s.
pub const EVERY: Duration = Duration::from_secs(3);
/// Values go stale after this without a read.
pub const TIMEOUT: Duration = Duration::from_secs(15);
/// The chain restarts itself after 20 s without a STAT line; past this, Stalled.
const STALE: Duration = Duration::from_secs(12);
const TEENSY: &str = "usb-Teensyduino_SDemu_Touch_";

/// One parsed STAT line.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stat {
    pub frames: u64,
    pub drop: u64,
    pub srv_sect: u64,
    pub stalls: u64,
    pub rd_stalled: bool,
    pub cap_kb: u64,
}

impl Stat {
    /// `STAT sf built=... frames=N drop=N ... srv_sect=N ... rd_stalled=0 stalls=N ... cap_kb=N ...`
    pub fn parse(line: &str) -> Option<Stat> {
        let rest = line.strip_prefix("STAT ")?;
        let mut s = Stat::default();
        let mut seen = 0;
        for word in rest.split_whitespace() {
            let Some((k, v)) = word.split_once('=') else {
                continue;
            };
            let n = || v.parse::<u64>().ok();
            match k {
                "frames" => s.frames = n()?,
                "drop" => s.drop = n()?,
                "srv_sect" => s.srv_sect = n()?,
                "stalls" => s.stalls = n()?,
                "rd_stalled" => s.rd_stalled = v != "0",
                "cap_kb" => s.cap_kb = n()?,
                _ => continue,
            }
            seen += 1;
        }
        (seen == 6).then_some(s)
    }
}

/// The last two STAT lines in `text`, newest last.
fn last_stats(text: &str) -> (Option<Stat>, Option<Stat>) {
    let mut it = text.lines().rev().filter_map(Stat::parse);
    let newest = it.next();
    let before = it.next();
    (before, newest)
}

/// The unit's state from `systemctl --user show -p LoadState,ActiveState`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Unit {
    Missing,
    Active,
    Failed,
    Inactive,
}

impl Unit {
    pub fn parse(show: &str) -> Unit {
        let get = |k: &str| {
            show.lines()
                .find_map(|l| l.strip_prefix(k)?.strip_prefix('='))
                .unwrap_or("")
        };
        match (get("LoadState"), get("ActiveState")) {
            ("not-found", _) | ("", _) => Unit::Missing,
            (_, "active" | "activating" | "reloading") => Unit::Active,
            (_, "failed") => Unit::Failed,
            _ => Unit::Inactive,
        }
    }
}

/// What one read saw.
#[derive(Clone, Debug, PartialEq)]
pub struct Seen {
    pub unit: Unit,
    pub teensy: bool,
    /// live.log's age, if it exists.
    pub log_age: Option<Duration>,
    pub before: Option<Stat>,
    pub newest: Option<Stat>,
    /// avlink's `--stats` period.
    pub period_s: f64,
}

impl Seen {
    /// The `avsd.*` values; empty when the unit isn't installed.
    pub fn values(&self) -> Vec<(String, Option<Value>)> {
        if self.unit == Unit::Missing {
            return Vec::new();
        }
        let fresh = self.unit == Unit::Active && self.log_age.is_some_and(|a| a <= STALE);
        let state = match self.unit {
            Unit::Missing | Unit::Inactive => "Off",
            Unit::Failed => "Failed",
            Unit::Active if !self.teensy => "Waiting for Teensy",
            Unit::Active => match (&self.newest, self.log_age) {
                (Some(s), Some(_)) if fresh && !s.rd_stalled => "Streaming",
                (Some(_), Some(_)) => "Stalled",
                _ => "Starting",
            },
        };
        let mut out = vec![
            ("avsd.state".to_string(), Some(Value::Text(state.into()))),
            ("avsd.teensy".to_string(), Some(Value::Bool(self.teensy))),
        ];
        let live = |f: &dyn Fn(&Stat) -> f64| {
            self.newest
                .as_ref()
                .filter(|_| fresh)
                .map(|s| Value::Num(f(s)))
        };
        let rate = |f: &dyn Fn(&Stat) -> u64| match (&self.before, &self.newest) {
            (Some(a), Some(b)) if fresh && f(b) >= f(a) && self.period_s > 0.0 => Some(Value::Num(
                ((f(b) - f(a)) as f64 / self.period_s * 10.0).round() / 10.0,
            )),
            _ => None,
        };
        out.push(("avsd.fps".into(), rate(&|s| s.frames)));
        out.push((
            "avsd.link_kbs".into(),
            rate(&|s| s.srv_sect).map(|v| match v {
                Value::Num(n) => Value::Num((n * 512.0 / 1000.0).round()),
                v => v,
            }),
        ));
        out.push(("avsd.cap_kbs".into(), live(&|s| s.cap_kb as f64)));
        out.push(("avsd.dropped".into(), live(&|s| s.drop as f64)));
        out.push(("avsd.stalls".into(), live(&|s| s.stalls as f64)));
        out
    }
}

/// Where the chain keeps its logs.
pub fn chain_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").map(|d| PathBuf::from(d).join("avsd"))
}

/// The tail of a file: enough for a few STAT lines between console text.
fn tail(path: &Path, bytes: u64) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(bytes))).ok()?;
    let mut buf = Vec::new();
    f.take(bytes).read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

fn teensy_present() -> bool {
    std::fs::read_dir("/dev/serial/by-id")
        .map(|d| {
            d.flatten()
                .any(|e| e.file_name().to_string_lossy().starts_with(TEENSY))
        })
        .unwrap_or(false)
}

/// One read. Blocking: run it off the loop.
pub fn read(unit: &str, dir: &Path) -> Seen {
    let show = std::process::Command::new("systemctl")
        .args(["--user", "show", "-p", "LoadState,ActiveState", unit])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let log = dir.join("live.log");
    let log_age = std::fs::metadata(&log)
        .and_then(|m| m.modified())
        .ok()
        .map(|t| SystemTime::now().duration_since(t).unwrap_or_default());
    let (before, newest) = tail(&log, 16 * 1024)
        .map(|t| last_stats(&t))
        .unwrap_or((None, None));
    Seen {
        unit: Unit::parse(&show),
        teensy: teensy_present(),
        log_age,
        before,
        newest,
        period_s: 5.0,
    }
}

/// Reads every [`EVERY`] and hands the values to the loop.
pub fn spawn(unit: String, tx: tokio::sync::mpsc::Sender<crate::Msg>) {
    let Some(dir) = chain_dir() else {
        tracing::info!("avsd status: no XDG_RUNTIME_DIR; not reading the stream state");
        return;
    };
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last_state = None;
        loop {
            tick.tick().await;
            let (u, d) = (unit.clone(), dir.clone());
            let Ok(seen) = tokio::task::spawn_blocking(move || read(&u, &d)).await else {
                continue;
            };
            let values = seen.values();
            let state = values
                .first()
                .and_then(|(_, v)| v.as_ref())
                .map(|v| v.to_string());
            if state != last_state {
                tracing::info!("avsd: {}", state.as_deref().unwrap_or("unit not installed"));
                last_state = state;
            }
            if values.is_empty() {
                continue;
            }
            if tx.send(crate::Msg::Values(values, TIMEOUT)).await.is_err() {
                break;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str = "STAT sf built=10 sent=10 repeat=0 late=0 frames=300 drop=2 audio_in=1 silence=0 adrop=0 raw=0 adj=0 credit_waits=0 keyreq=0 | teensy status=5 free=10/20 slots=3 newest=4 served=4 srv_sect=10000 holds=0 timeouts=0 notready=0 expired=0 idle=0 evicted=0 conv=0 rxerr=0 bus=4 junk=0 | rd_stalled=0 stalls=1 discards=0 vdiscarded=0 waitkey=0 window_waits=0 away_ticks=0 cap_kb=4500 backoffs=0 catchups=0";

    fn get<'a>(v: &'a [(String, Option<Value>)], n: &str) -> Option<&'a Value> {
        v.iter().find(|(k, _)| k == n).and_then(|(_, v)| v.as_ref())
    }

    #[test]
    fn parses_a_stat_line() {
        let s = Stat::parse(LINE).unwrap();
        assert_eq!(
            s,
            Stat {
                frames: 300,
                drop: 2,
                srv_sect: 10000,
                stalls: 1,
                rd_stalled: false,
                cap_kb: 4500
            }
        );
        assert_eq!(Stat::parse("END sf built=1"), None);
        assert_eq!(Stat::parse("STAT sf built=1 frames=2"), None);
    }

    #[test]
    fn rates_come_from_the_last_two_lines() {
        let later = LINE
            .replace("frames=300", "frames=450")
            .replace("srv_sect=10000", "srv_sect=17000");
        let text = format!("teensy: hello\n{LINE}\nteensy: console\n{later}\n");
        let (before, newest) = last_stats(&text);
        let seen = Seen {
            unit: Unit::Active,
            teensy: true,
            log_age: Some(Duration::from_secs(1)),
            before,
            newest,
            period_s: 5.0,
        };
        let v = seen.values();
        assert_eq!(
            get(&v, "avsd.state"),
            Some(&Value::Text("Streaming".into()))
        );
        assert_eq!(get(&v, "avsd.fps"), Some(&Value::Num(30.0)));
        // 7000 sectors in 5 s = 1400 sectors/s x 512 B.
        assert_eq!(get(&v, "avsd.link_kbs"), Some(&Value::Num(717.0)));
        assert_eq!(get(&v, "avsd.cap_kbs"), Some(&Value::Num(4500.0)));
        assert_eq!(get(&v, "avsd.dropped"), Some(&Value::Num(2.0)));
    }

    #[test]
    fn states() {
        let mut seen = Seen {
            unit: Unit::Active,
            teensy: false,
            log_age: None,
            before: None,
            newest: None,
            period_s: 5.0,
        };
        let state = |s: &Seen| get(&s.values(), "avsd.state").map(|v| v.to_string());
        assert_eq!(state(&seen).as_deref(), Some("Waiting for Teensy"));
        seen.teensy = true;
        assert_eq!(state(&seen).as_deref(), Some("Starting"));
        seen.newest = Stat::parse(LINE);
        seen.log_age = Some(Duration::from_secs(30));
        assert_eq!(state(&seen).as_deref(), Some("Stalled"));
        // Stale numbers aren't shown as live.
        assert_eq!(get(&seen.values(), "avsd.fps"), None);
        assert_eq!(get(&seen.values(), "avsd.dropped"), None);
        seen.unit = Unit::Failed;
        assert_eq!(state(&seen).as_deref(), Some("Failed"));
        seen.unit = Unit::Missing;
        assert!(seen.values().is_empty());
    }

    #[test]
    fn unit_states() {
        assert_eq!(
            Unit::parse("LoadState=loaded\nActiveState=active\n"),
            Unit::Active
        );
        assert_eq!(
            Unit::parse("LoadState=not-found\nActiveState=inactive\n"),
            Unit::Missing
        );
        assert_eq!(
            Unit::parse("LoadState=loaded\nActiveState=failed\n"),
            Unit::Failed
        );
        assert_eq!(Unit::parse(""), Unit::Missing);
    }
}
