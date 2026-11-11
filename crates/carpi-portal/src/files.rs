//! Names, paths and writes. Everything that touches the disk on behalf of a
//! request goes through here.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// `[A-Za-z0-9._-]`, 1..=128 chars, no leading dot. Rules out `/`, `\`,
/// `..`, drive letters and hidden files.
pub fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// `dir/name` if it is a regular file (not a symlink) that really lives in `dir`.
pub fn resolve(dir: &Path, name: &str) -> Option<PathBuf> {
    if !safe_name(name) {
        return None;
    }
    let path = dir.join(name);
    if !fs::symlink_metadata(&path).ok()?.is_file() {
        return None;
    }
    let root = fs::canonicalize(dir).ok()?;
    let real = fs::canonicalize(&path).ok()?;
    (real.parent() == Some(root.as_path())).then_some(path)
}

pub struct Entry {
    pub name: String,
    pub size: u64,
    pub modified: SystemTime,
}

/// Regular files in `dir` with safe names, newest first. Missing dir: empty.
pub fn list(dir: &Path) -> Vec<Entry> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Entry> = rd
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let meta = e.metadata().ok()?; // DirEntry::metadata doesn't follow symlinks
            (safe_name(&name) && meta.is_file()).then(|| Entry {
                name,
                size: meta.len(),
                modified: meta.modified().unwrap_or(UNIX_EPOCH),
            })
        })
        .collect();
    out.sort_by(|a, b| b.modified.cmp(&a.modified).then(a.name.cmp(&b.name)));
    out
}

/// Replaces `target` with `text`, copying the old file to
/// `<name>.<stamp>.bak` beside it first. Returns the backup's name.
pub fn replace_with_backup(target: &Path, text: &str) -> io::Result<Option<String>> {
    // Write through a symlinked config rather than replacing the link.
    let real = fs::canonicalize(target);
    let target = real.as_deref().unwrap_or(target);
    let dir = target.parent().unwrap_or(Path::new("."));
    let base = target
        .file_name()
        .ok_or_else(|| io::Error::other("config path has no file name"))?
        .to_string_lossy()
        .into_owned();
    fs::create_dir_all(dir)?;
    let backup = match fs::metadata(target) {
        Ok(m) if m.is_file() => {
            let old = fs::read(target)?;
            let stamp = stamp();
            let mut n = 0;
            loop {
                let name = match n {
                    0 => format!("{base}.{stamp}.bak"),
                    n => format!("{base}.{stamp}-{n}.bak"),
                };
                match write_new(&dir.join(&name), &old) {
                    Ok(()) => break Some(name),
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists && n < 100 => n += 1,
                    Err(e) => return Err(e),
                }
            }
        }
        _ => None,
    };
    let tmp = dir.join(format!(".{base}.upload"));
    let mut f = fs::File::create(&tmp)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()?;
    drop(f);
    fs::rename(&tmp, target)?;
    Ok(backup)
}

/// Writes `dir/name` only if it doesn't exist. The file appears complete or
/// not at all.
pub fn store_new(dir: &Path, name: &str, data: &[u8]) -> io::Result<()> {
    static SEQ: AtomicU32 = AtomicU32::new(0);
    fs::create_dir_all(dir)?;
    let dest = dir.join(name);
    if fs::symlink_metadata(&dest).is_ok() {
        return Err(io::ErrorKind::AlreadyExists.into());
    }
    let tmp = dir.join(format!(
        ".upload-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    write_new(&tmp, data)?;
    let linked = fs::hard_link(&tmp, &dest);
    let result = match linked {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(e),
        // No hard links on this filesystem: plain rename after the check above.
        Err(_) => return fs::rename(&tmp, &dest),
    };
    let _ = fs::remove_file(&tmp);
    result
}

fn write_new(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut f = OpenOptions::new().write(true).create_new(true).open(path)?;
    f.write_all(data)?;
    f.sync_all()
}

/// One RNS-E flash sector, 128 KiB.
pub const SECTOR_BIN_SIZE: usize = 0x20000;

/// A `.bin` upload is an RNS-E sector image: `[prefix]sector_<N>[-tag].bin`,
/// exactly one sector long. carpid checks the rest.
pub fn check_sector_bin(name: &str, len: usize) -> Result<(), String> {
    let lower = name.to_ascii_lowercase();
    let named = lower
        .rfind("sector_")
        .and_then(|at| lower[at + "sector_".len()..].strip_suffix(".bin"))
        .is_some_and(|r| {
            let d = r.bytes().take_while(u8::is_ascii_digit).count();
            (1..=3).contains(&d) && (r.len() == d || r[d..].starts_with(['-', '_', '.']))
        });
    if !named {
        return Err("Sector image name: [prefix]sector_<N>[-tag].bin.".into());
    }
    if len != SECTOR_BIN_SIZE {
        return Err(format!(
            "Sector image must be {SECTOR_BIN_SIZE} bytes (128 KiB), got {len}."
        ));
    }
    Ok(())
}

/// Intel HEX sanity: every record well formed with a good checksum, known
/// record types, ends with an EOF record. Says nothing about the target chip.
pub fn check_ihex(data: &[u8]) -> Result<(), String> {
    let text = std::str::from_utf8(data).map_err(|_| "Not an Intel HEX file (not text).")?;
    let mut eof = false;
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let n = i + 1;
        if eof {
            return Err(format!("Line {n}: data after the end-of-file record."));
        }
        let rec = line
            .strip_prefix(':')
            .and_then(unhex_bytes)
            .ok_or_else(|| format!("Line {n}: not an Intel HEX record."))?;
        if rec.len() < 5 || rec.len() != rec[0] as usize + 5 {
            return Err(format!("Line {n}: wrong record length."));
        }
        if rec.iter().fold(0u8, |a, b| a.wrapping_add(*b)) != 0 {
            return Err(format!("Line {n}: bad checksum."));
        }
        match rec[3] {
            0 | 2..=5 => {}
            1 => eof = true,
            t => return Err(format!("Line {n}: unknown record type {t:02X}.")),
        }
    }
    if !eof {
        return Err("No end-of-file record; the file is cut short.".into());
    }
    Ok(())
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex_bytes(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || !s.is_ascii() {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

pub fn unhex(s: &str) -> Option<String> {
    String::from_utf8(unhex_bytes(s)?).ok()
}

/// UTC `YYYYMMDDTHHMMSSZ` for now.
pub fn stamp() -> String {
    let (y, mo, d, h, mi, s) = utc(SystemTime::now());
    format!("{y:04}{mo:02}{d:02}T{h:02}{mi:02}{s:02}Z")
}

/// UTC `YYYY-MM-DD HH:MM`.
pub fn short_time(t: SystemTime) -> String {
    let (y, mo, d, h, mi, _) = utc(t);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}")
}

fn utc(t: SystemTime) -> (i64, u32, u32, u32, u32, u32) {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400) as u32);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d, rem / 3600, rem / 60 % 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn names() {
        assert!(safe_name("capture-20261107T120000Z.log"));
        assert!(safe_name("teensy_4.1.hex"));
        for bad in [
            "", ".", "..", ".hidden", "a/b", "a\\b", "../x", "c:x", "a b", "é",
        ] {
            assert!(!safe_name(bad), "{bad}");
        }
        assert!(!safe_name(&"a".repeat(129)));
    }

    #[test]
    fn dates() {
        let t = UNIX_EPOCH + Duration::from_secs(1_794_052_800); // 2026-11-07 12:00:00
        assert_eq!(utc(t), (2026, 11, 7, 12, 0, 0));
        assert_eq!(utc(UNIX_EPOCH + Duration::from_secs(951_782_400)).0, 2000); // 2000-02-29
        assert_eq!(short_time(UNIX_EPOCH), "1970-01-01 00:00");
    }

    #[test]
    fn ihex() {
        let good = b":0400000001020304F2\n:00000001FF\n";
        assert_eq!(check_ihex(good), Ok(()));
        assert_eq!(
            check_ihex(b":0400000001020304F2\r\n:00000001FF\r\n"),
            Ok(())
        );
        assert!(check_ihex(b":0400000001020304F3\n:00000001FF\n").is_err()); // checksum
        assert!(check_ihex(b":0400000001020304F2\n").is_err()); // no EOF
        assert!(check_ihex(b"hello\n:00000001FF\n").is_err());
        assert!(check_ihex(b":00000001FF\n:0400000001020304F2\n").is_err()); // after EOF
        assert!(check_ihex(b":0500000001020304F2\n:00000001FF\n").is_err()); // length
    }

    #[test]
    fn sector_bin() {
        assert_eq!(check_sector_bin("sector_96.bin", SECTOR_BIN_SIZE), Ok(()));
        assert_eq!(
            check_sector_bin("Sector_107-try2.BIN", SECTOR_BIN_SIZE),
            Ok(())
        );
        assert_eq!(
            check_sector_bin("r52_sector_33.bin", SECTOR_BIN_SIZE),
            Ok(())
        );
        assert!(check_sector_bin("sector_96.bin", 10).is_err());
        assert!(check_sector_bin("dump.bin", SECTOR_BIN_SIZE).is_err());
        assert!(check_sector_bin("sector_96x.bin", SECTOR_BIN_SIZE).is_err());
    }

    #[test]
    fn hex_roundtrip() {
        assert_eq!(unhex(&hex(b"12;ab")).as_deref(), Some("12;ab"));
        assert_eq!(unhex("zz"), None);
    }
}
