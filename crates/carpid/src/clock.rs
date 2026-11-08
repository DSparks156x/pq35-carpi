//! Local time, and setting the Pi clock from the car's.
//!
//! Log folders are named by the Pi's local date, so the Pi clock has to be
//! right before a capture is named. Network time wins when it has synced.
//! Otherwise, when the car clock (0x65D, `clock.datetime`, local time) and
//! the Pi's local time differ by more than [`MAX_DRIFT_S`], carpid sets the
//! Pi clock through timedated (`timedatectl`; os/profile.sh grants the car
//! user `set-time` and `set-ntp` through polkit).
//!
//! timedated refuses `set-time` while NTP is enabled, synced or not, so the
//! sequence is: `set-ntp false`, `set-time`, `set-ntp true`. Turning NTP back
//! on restarts timesyncd, which still corrects the clock once it reaches a
//! server. The car clock is local, so the Pi's timezone must match it
//! (os/profile.sh --tz, or the settings API: `set-timezone`, also granted).
//! glibc reads the timezone once, so carpid needs a restart after a change.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Pi and car may differ by this much before the Pi is set.
pub const MAX_DRIFT_S: i64 = 120;

/// How often carpid compares the clocks.
pub const CHECK_EVERY: Duration = Duration::from_secs(60);

/// A car clock before this is unset, not a time.
const MIN_YEAR: i64 = 2024;

/// A moment as local wall time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Local {
    /// `2026-11-08`.
    pub date: String,
    /// `2026-11-08T10:00:00`.
    pub datetime: String,
    /// Seconds since 1970-01-01T00:00:00 of the local wall time (not UTC),
    /// to compare with the car clock.
    pub naive: i64,
}

pub fn local(t: SystemTime) -> Local {
    let (y, mo, d, h, mi, s) = local_fields(t);
    Local {
        date: format!("{y:04}-{mo:02}-{d:02}"),
        datetime: format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}"),
        naive: naive_secs(y, mo, d, h, mi, s),
    }
}

pub fn local_now() -> Local {
    local(SystemTime::now())
}

#[cfg(unix)]
fn local_fields(t: SystemTime) -> (i64, i64, i64, i64, i64, i64) {
    let secs = epoch_secs(t) as libc::time_t;
    // SAFETY: localtime_r only writes the tm we hand it.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ok = unsafe { !libc::localtime_r(&secs, &mut tm).is_null() };
    if !ok {
        return utc_fields(t);
    }
    (
        tm.tm_year as i64 + 1900,
        tm.tm_mon as i64 + 1,
        tm.tm_mday as i64,
        tm.tm_hour as i64,
        tm.tm_min as i64,
        tm.tm_sec as i64,
    )
}

/// No timezone database off Unix: UTC.
#[cfg(not(unix))]
fn local_fields(t: SystemTime) -> (i64, i64, i64, i64, i64, i64) {
    utc_fields(t)
}

fn utc_fields(t: SystemTime) -> (i64, i64, i64, i64, i64, i64) {
    let secs = epoch_secs(t);
    let (y, mo, d) = civil_from_days(secs.div_euclid(86_400));
    let r = secs.rem_euclid(86_400);
    (y, mo, d, r / 3600, r / 60 % 60, r % 60)
}

pub fn epoch_secs(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian date (H. Hinnant).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (
        if m <= 2 {
            yoe + era * 400 + 1
        } else {
            yoe + era * 400
        },
        m,
        d,
    )
}

fn naive_secs(y: i64, mo: i64, d: i64, h: i64, mi: i64, s: i64) -> i64 {
    days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + s
}

/// `2026-11-07T19:05:30` (as `clock.datetime` has it) -> naive seconds.
pub fn parse_naive(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 19
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let (h, mi, sec) = (n(11..13)?, n(14..16)?, n(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 59 {
        return None;
    }
    Some(naive_secs(y, mo, d, h, mi, sec))
}

/// Naive seconds -> `2026-11-07 19:05:30`, the form `timedatectl set-time`
/// takes (read as local time).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn format_naive(n: i64) -> String {
    let (y, mo, d) = civil_from_days(n.div_euclid(86_400));
    let r = n.rem_euclid(86_400);
    format!(
        "{y:04}-{mo:02}-{d:02} {:02}:{:02}:{:02}",
        r / 3600,
        r / 60 % 60,
        r % 60
    )
}

/// The car's local time now, from a `clock.datetime` reading `age_ms` old.
/// `None` for an unset car clock.
pub fn car_now(datetime: &str, age_ms: u64) -> Option<i64> {
    let n = parse_naive(datetime)?;
    let year = civil_from_days(n.div_euclid(86_400)).0;
    (year >= MIN_YEAR).then_some(n + (age_ms / 1000) as i64)
}

/// The car's time to set, if the Pi is off by more than [`MAX_DRIFT_S`].
pub fn needs_set(pi_naive: i64, car_naive: i64) -> Option<i64> {
    ((pi_naive - car_naive).abs() > MAX_DRIFT_S).then_some(car_naive)
}

/// `timedatectl show` -> (NTP enabled, NTP synchronized).
pub fn parse_show(text: &str) -> (bool, bool) {
    let get = |k: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(k)?.strip_prefix('='))
            .is_some_and(|v| v.trim() == "yes")
    };
    (get("NTP"), get("NTPSynchronized"))
}

/// Sets the Pi clock to the car's `car` (naive local seconds), unless
/// network time has synced. Blocking: runs `timedatectl`. Returns the
/// change for the log; `None` when network time is synced.
#[cfg(target_os = "linux")]
pub fn set_from_car(car: i64, pi: i64) -> Result<Option<String>, String> {
    use std::process::Command;
    let run = |args: &[&str]| -> Result<String, String> {
        let out = Command::new("timedatectl")
            .args(args)
            .output()
            .map_err(|e| format!("timedatectl: {e}"))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(format!(
                "timedatectl {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    };
    let (ntp, synced) = parse_show(&run(&["show"])?);
    if synced {
        return Ok(None);
    }
    let when = format_naive(car);
    if ntp {
        run(&["set-ntp", "false"])?;
    }
    // timedated sees timesyncd stop a moment after set-ntp returns.
    let mut set = run(&["set-time", &when]);
    for _ in 0..5 {
        match &set {
            Err(e) if e.contains("synchronization is enabled") => {
                std::thread::sleep(Duration::from_secs(1));
                set = run(&["set-time", &when]);
            }
            _ => break,
        }
    }
    if ntp {
        // Back on even when set-time failed, so network time still works.
        if let Err(e) = run(&["set-ntp", "true"]) {
            tracing::warn!("clock: turning NTP back on: {e}");
        }
    }
    set?;
    Ok(Some(format!(
        "Pi clock {} -> {when} (car clock; off by {} s)",
        format_naive(pi),
        pi - car
    )))
}

#[cfg(not(target_os = "linux"))]
pub fn set_from_car(_car: i64, _pi: i64) -> Result<Option<String>, String> {
    Err("setting the clock needs Linux (timedated)".into())
}

const ZONEINFO: &str = "/usr/share/zoneinfo";

/// Zones offered besides every `America/*` and `Europe/*`.
const OTHER_ZONES: &[&str] = &[
    "UTC",
    "Africa/Johannesburg",
    "Asia/Dubai",
    "Asia/Kolkata",
    "Asia/Shanghai",
    "Asia/Singapore",
    "Asia/Tokyo",
    "Australia/Perth",
    "Australia/Sydney",
    "Pacific/Auckland",
    "Pacific/Honolulu",
];

/// The Pi's timezone: `/etc/localtime`'s link into the zoneinfo tree.
pub fn timezone() -> String {
    std::fs::read_link("/etc/localtime")
        .ok()
        .and_then(|p| {
            let s = p.to_string_lossy().into_owned();
            s.split_once("zoneinfo/").map(|(_, z)| z.to_string())
        })
        .unwrap_or_default()
}

/// Timezone choices from the zoneinfo folder, sorted. Read once.
pub fn timezones() -> &'static [String] {
    static LIST: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    LIST.get_or_init(|| zones_in(std::path::Path::new(ZONEINFO)))
}

fn zones_in(root: &std::path::Path) -> Vec<String> {
    fn walk(dir: &std::path::Path, name: &str, out: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let n = format!("{name}/{}", e.file_name().to_string_lossy());
            match e.file_type() {
                Ok(t) if t.is_dir() => walk(&e.path(), &n, out),
                Ok(_) => out.push(n),
                Err(_) => {}
            }
        }
    }
    let mut out = Vec::new();
    for area in ["America", "Europe"] {
        walk(&root.join(area), area, &mut out);
    }
    out.extend(
        OTHER_ZONES
            .iter()
            .filter(|z| root.join(z).is_file())
            .map(|z| z.to_string()),
    );
    out.retain(|z| {
        z.chars()
            .all(|c| c.is_ascii_alphanumeric() || "/_+-".contains(c))
    });
    out.sort();
    out.dedup();
    out
}

/// The command that sets `tz`, if it's one of `choices`.
pub fn set_timezone_command(tz: &str, choices: &[String]) -> Result<Vec<String>, String> {
    if !choices.iter().any(|c| c == tz) {
        return Err(if choices.is_empty() {
            "no timezone database on this machine".into()
        } else {
            format!("unknown timezone {tz:?}")
        });
    }
    Ok(["timedatectl", "set-timezone", tz]
        .iter()
        .map(|s| s.to_string())
        .collect())
}

/// Runs `cmd` with a time limit. Async: carpid's loop keeps going.
pub async fn run(cmd: Vec<String>) -> Result<(), String> {
    let (prog, args) = cmd.split_first().ok_or("empty command")?;
    let out = tokio::time::timeout(
        Duration::from_secs(20),
        tokio::process::Command::new(prog).args(args).output(),
    )
    .await
    .map_err(|_| format!("{} timed out", cmd.join(" ")))?
    .map_err(|e| format!("{prog}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{}: {}",
            cmd.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_round_trip() {
        for (y, m, d) in [(1970, 1, 1), (2000, 2, 29), (2026, 11, 8), (2099, 12, 31)] {
            assert_eq!(civil_from_days(days_from_civil(y, m, d)), (y, m, d));
        }
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        let n = parse_naive("2026-11-07T19:05:30").unwrap();
        assert_eq!(format_naive(n), "2026-11-07 19:05:30");
        assert_eq!(parse_naive("2026-13-07T19:05:30"), None);
        assert_eq!(parse_naive("2026-11-07 19:05:30"), None);
    }

    #[test]
    fn utc_fallback_formats() {
        let t = UNIX_EPOCH + Duration::from_secs(1_762_596_000); // 2025-11-08T10:00:00Z
        let (y, mo, d, h, mi, s) = utc_fields(t);
        assert_eq!((y, mo, d, h, mi, s), (2025, 11, 8, 10, 0, 0));
    }

    #[test]
    fn drift_rule() {
        let car = car_now("2026-11-08T10:00:00", 1500).unwrap();
        assert_eq!(format_naive(car), "2026-11-08 10:00:01");
        assert_eq!(needs_set(car + 120, car), None);
        assert_eq!(needs_set(car - 121, car), Some(car));
        assert_eq!(needs_set(car + 5 * 3600, car), Some(car));
        // An unset car clock is no time at all.
        assert_eq!(car_now("2000-01-01T00:00:00", 0), None);
    }

    #[test]
    fn timezone_command_and_choices() {
        let d = tempfile::tempdir().unwrap();
        for z in [
            "Europe/Berlin",
            "America/Chicago",
            "America/Indiana/Knox",
            "Asia/Tokyo",
            "Asia/Omsk",
        ] {
            let p = d.path().join(z);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "TZif").unwrap();
        }
        let zones = zones_in(d.path());
        assert_eq!(
            zones,
            [
                "America/Chicago",
                "America/Indiana/Knox",
                "Asia/Tokyo",
                "Europe/Berlin"
            ]
        );
        assert_eq!(
            set_timezone_command("Europe/Berlin", &zones).unwrap(),
            ["timedatectl", "set-timezone", "Europe/Berlin"]
        );
        assert!(set_timezone_command("Asia/Omsk", &zones).is_err());
        assert!(set_timezone_command("Europe/Berlin; reboot", &zones).is_err());
        assert!(set_timezone_command("UTC", &[]).is_err());
    }

    #[test]
    fn show_output() {
        let text = "Timezone=America/Chicago\nNTP=yes\nNTPSynchronized=no\nTimeUSec=x\n";
        assert_eq!(parse_show(text), (true, false));
        assert_eq!(parse_show("NTP=no\nNTPSynchronized=yes\n"), (false, true));
    }
}
