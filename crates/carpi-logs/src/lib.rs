//! The logs folder (docs/PAGES.md 3).
//!
//! ```text
//! <root>/2026-11-08/01/entry.json     what this is (see Meta)
//!                     /01/...          its files; subfolders allowed (config/)
//!                     /02/...
//! ```
//!
//! One *entry* per capture or data-log run. Numbers start at 01 each day and
//! are shared by both kinds. Dates are local, passed in by the caller (the
//! Pi clock is set from the car clock first). carpid writes; the portal
//! reads, stars and zips. Everything that takes a name from outside goes
//! through [`EntryId::parse`] / [`Logs::file`], which refuse anything but the
//! fixed shapes.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const META: &str = "entry.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Capture,
    Datalog,
}

/// `2026-11-08` and `3`: shown as `2026-11-08 #03`, folder `2026-11-08/03`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EntryId {
    pub date: String,
    pub number: u32,
}

impl EntryId {
    /// `2026-11-08/03`, `2026-11-08-03` or `2026-11-08 #03`.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        // ASCII only, so split_at can't land inside a character.
        if s.len() < 12 || !s.is_ascii() {
            return None;
        }
        let (date, rest) = s.split_at(10);
        let n = rest.trim_start_matches(['/', '-', ' ', '#']);
        if !valid_date(date) || !valid_number(n) {
            return None;
        }
        Some(EntryId {
            date: date.into(),
            number: n.parse().ok()?,
        })
    }

    /// Folder name inside the day: `03`.
    pub fn folder(&self) -> String {
        format!("{:02}", self.number)
    }

    /// For file names: `2026-11-08-03`.
    pub fn slug(&self) -> String {
        format!("{}-{:02}", self.date, self.number)
    }
}

impl fmt::Display for EntryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} #{:02}", self.date, self.number)
    }
}

/// `entry.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    pub kind: Kind,
    pub id: EntryId,
    /// What made it: `key`, `page`, `portal`, `cli`, `crash:<unit>`,
    /// `error:<what>`, or a data-log profile name.
    pub trigger: String,
    /// Local time, `YYYY-MM-DDTHH:MM:SS`.
    pub started: String,
    /// Set when the entry is complete. A capture without it was cut short.
    #[serde(default)]
    pub ended: Option<String>,
    #[serde(default)]
    pub starred: bool,
    /// One line for lists: "4 buses, 31 812 frames", "boost set, 10 Hz".
    #[serde(default)]
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Day {
    pub date: String,
    pub entries: usize,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: EntryId,
    /// `None`: no readable `entry.json` (still listed, still deletable).
    pub meta: Option<Meta>,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileInfo {
    /// Relative to the entry, `/`-separated: `journal.txt`, `config/carpid.toml`.
    pub path: String,
    pub bytes: u64,
}

#[derive(Clone, Debug)]
pub struct Logs {
    root: PathBuf,
}

impl Logs {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Logs { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Makes the next entry of `date` and writes its `entry.json` (no
    /// `ended`). The folder is claimed with a plain `mkdir`, so two writers
    /// never get the same number.
    pub fn create(
        &self,
        kind: Kind,
        date: &str,
        trigger: &str,
        started: &str,
    ) -> io::Result<(Meta, PathBuf)> {
        if !valid_date(date) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "bad date"));
        }
        let day = self.root.join(date);
        fs::create_dir_all(&day)?;
        let mut n = self.numbers(date).last().copied().unwrap_or(0) + 1;
        loop {
            let id = EntryId {
                date: date.into(),
                number: n,
            };
            let dir = day.join(id.folder());
            match fs::create_dir(&dir) {
                Ok(()) => {
                    let meta = Meta {
                        kind,
                        id,
                        trigger: trigger.into(),
                        started: started.into(),
                        ended: None,
                        starred: false,
                        summary: String::new(),
                    };
                    write_meta(&dir, &meta)?;
                    return Ok((meta, dir));
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists && n < 999 => n += 1,
                Err(e) => return Err(e),
            }
        }
    }

    /// Rewrites `entry.json` (set `ended`, `summary`, `starred`).
    pub fn save_meta(&self, meta: &Meta) -> io::Result<()> {
        let dir = self.dir(&meta.id).ok_or(io::ErrorKind::NotFound)?;
        write_meta(&dir, meta)
    }

    pub fn meta(&self, id: &EntryId) -> Option<Meta> {
        let dir = self.dir(id)?;
        serde_json::from_slice(&fs::read(dir.join(META)).ok()?).ok()
    }

    pub fn set_starred(&self, id: &EntryId, starred: bool) -> io::Result<()> {
        let mut m = self.meta(id).ok_or(io::ErrorKind::NotFound)?;
        m.starred = starred;
        self.save_meta(&m)
    }

    /// Newest day first.
    pub fn days(&self) -> Vec<Day> {
        let mut out: Vec<Day> = read_names(&self.root)
            .into_iter()
            .filter(|n| valid_date(n))
            .map(|date| {
                let numbers = self.numbers(&date);
                let bytes = dir_bytes(&self.root.join(&date));
                Day {
                    entries: numbers.len(),
                    date,
                    bytes,
                }
            })
            .filter(|d| d.entries > 0)
            .collect();
        out.sort_by(|a, b| b.date.cmp(&a.date));
        out
    }

    /// Entries of one day, newest first.
    pub fn entries(&self, date: &str) -> Vec<Entry> {
        if !valid_date(date) {
            return Vec::new();
        }
        let mut out: Vec<Entry> = self
            .numbers(date)
            .into_iter()
            .map(|number| {
                let id = EntryId {
                    date: date.into(),
                    number,
                };
                let dir = self.root.join(date).join(id.folder());
                Entry {
                    meta: self.meta(&id),
                    bytes: dir_bytes(&dir),
                    id,
                }
            })
            .collect();
        out.reverse();
        out
    }

    /// The entry's folder, if it exists and is a real folder under the root.
    pub fn dir(&self, id: &EntryId) -> Option<PathBuf> {
        if !valid_date(&id.date) || id.number == 0 || id.number > 999 {
            return None;
        }
        let dir = self.root.join(&id.date).join(id.folder());
        fs::symlink_metadata(&dir).ok()?.is_dir().then_some(dir)
    }

    /// Every regular file in the entry, sorted, `entry.json` included.
    /// Symlinks are skipped.
    pub fn files(&self, id: &EntryId) -> Vec<FileInfo> {
        let Some(dir) = self.dir(id) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        walk(&dir, "", &mut out);
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }

    /// One file of an entry by its relative path, checked: only names from
    /// [`Logs::files`] resolve.
    pub fn file(&self, id: &EntryId, rel: &str) -> Option<PathBuf> {
        let dir = self.dir(id)?;
        self.files(id)
            .into_iter()
            .find(|f| f.path == rel)
            .map(|f| dir.join(f.path))
    }

    pub fn remove(&self, id: &EntryId) -> io::Result<()> {
        let dir = self.dir(id).ok_or(io::ErrorKind::NotFound)?;
        fs::remove_dir_all(dir)?;
        let day = self.root.join(&id.date);
        if self.numbers(&id.date).is_empty() {
            let _ = fs::remove_dir(day);
        }
        Ok(())
    }

    pub fn total_bytes(&self) -> u64 {
        self.days().iter().map(|d| d.bytes).sum()
    }

    /// Deletes the oldest unstarred entries until the logs use at most
    /// `max_bytes`. `keep` (the entry being written) is never deleted.
    /// Returns what went.
    pub fn prune(&self, max_bytes: u64, keep: Option<&EntryId>) -> Vec<EntryId> {
        let mut total = self.total_bytes();
        let mut gone = Vec::new();
        if total <= max_bytes {
            return gone;
        }
        let mut days = self.days();
        days.reverse(); // oldest first
        for day in days {
            let mut entries = self.entries(&day.date);
            entries.reverse(); // oldest first
            for e in entries {
                if total <= max_bytes {
                    return gone;
                }
                let starred = e.meta.as_ref().is_some_and(|m| m.starred);
                if starred || Some(&e.id) == keep {
                    continue;
                }
                if self.remove(&e.id).is_ok() {
                    total = total.saturating_sub(e.bytes);
                    gone.push(e.id);
                }
            }
        }
        gone
    }

    fn numbers(&self, date: &str) -> Vec<u32> {
        let mut v: Vec<u32> = read_names(&self.root.join(date))
            .into_iter()
            .filter(|n| valid_number(n))
            .filter_map(|n| n.parse().ok())
            .filter(|n| *n > 0)
            .collect();
        v.sort_unstable();
        v
    }
}

fn write_meta(dir: &Path, meta: &Meta) -> io::Result<()> {
    let tmp = dir.join(".entry.json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(meta)?)?;
    fs::rename(tmp, dir.join(META))
}

fn valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            _ => c.is_ascii_digit(),
        })
}

fn valid_number(s: &str) -> bool {
    (2..=3).contains(&s.len()) && s.bytes().all(|c| c.is_ascii_digit())
}

fn read_names(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .filter_map(|e| e.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Plain names only: no leading dot, `[A-Za-z0-9._-]`.
fn safe_part(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 128
        && !n.starts_with('.')
        && n.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn walk(dir: &Path, prefix: &str, out: &mut Vec<FileInfo>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.filter_map(Result::ok) {
        let Ok(name) = e.file_name().into_string() else {
            continue;
        };
        if !safe_part(&name) {
            continue;
        }
        let Ok(t) = e.file_type() else { continue };
        let rel = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        if t.is_dir() && prefix.matches('/').count() < 3 {
            walk(&e.path(), &rel, out);
        } else if t.is_file() {
            let bytes = e.metadata().map_or(0, |m| m.len());
            out.push(FileInfo { path: rel, bytes });
        }
    }
}

fn dir_bytes(dir: &Path) -> u64 {
    let Ok(rd) = fs::read_dir(dir) else { return 0 };
    rd.filter_map(Result::ok)
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_bytes(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map_or(0, |m| m.len()),
            _ => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn logs() -> (tempfile::TempDir, Logs) {
        let t = tempfile::tempdir().unwrap();
        let l = Logs::new(t.path().join("logs"));
        (t, l)
    }

    #[test]
    fn numbers_restart_each_day_and_are_shared_by_kinds() {
        let (_t, l) = logs();
        let (a, _) = l
            .create(Kind::Capture, "2026-11-08", "key", "2026-11-08T10:00:00")
            .unwrap();
        let (b, _) = l
            .create(Kind::Datalog, "2026-11-08", "boost", "2026-11-08T10:05:00")
            .unwrap();
        let (c, _) = l
            .create(Kind::Capture, "2026-11-09", "portal", "2026-11-09T08:00:00")
            .unwrap();
        assert_eq!(a.id.to_string(), "2026-11-08 #01");
        assert_eq!(b.id.number, 2);
        assert_eq!(c.id.number, 1);
        assert_eq!(
            l.days().iter().map(|d| d.date.as_str()).collect::<Vec<_>>(),
            ["2026-11-09", "2026-11-08"]
        );
        assert_eq!(l.entries("2026-11-08")[0].id.number, 2);
        assert!(l.create(Kind::Capture, "../x", "key", "").is_err());
    }

    #[test]
    fn ids_parse_three_ways_and_refuse_the_rest() {
        let want = Some(EntryId {
            date: "2026-11-08".into(),
            number: 3,
        });
        for s in ["2026-11-08/03", "2026-11-08-03", "2026-11-08 #03"] {
            assert_eq!(EntryId::parse(s), want, "{s}");
        }
        for s in [
            "2026-11-08/3",
            "2026-11-08/../x",
            "2026/11/08/03",
            "x",
            "2026-11-08/0003",
        ] {
            assert_eq!(EntryId::parse(s), None, "{s}");
        }
        assert_eq!(want.map(|w| w.slug()).as_deref(), Some("2026-11-08-03"));
    }

    #[test]
    fn files_list_and_resolve_only_what_is_there() {
        let (_t, l) = logs();
        let (m, dir) = l.create(Kind::Capture, "2026-11-08", "key", "t").unwrap();
        fs::create_dir(dir.join("config")).unwrap();
        fs::write(dir.join("config/carpid.toml"), "x").unwrap();
        fs::write(dir.join("journal.txt"), "hello").unwrap();
        fs::write(dir.join(".hidden"), "no").unwrap();
        let names: Vec<_> = l.files(&m.id).into_iter().map(|f| f.path).collect();
        assert_eq!(names, ["config/carpid.toml", "entry.json", "journal.txt"]);
        assert!(l.file(&m.id, "journal.txt").is_some());
        assert!(l.file(&m.id, "../../x").is_none());
        assert!(l.file(&m.id, ".hidden").is_none());
    }

    #[test]
    fn prune_drops_oldest_unstarred_first() {
        let (_t, l) = logs();
        let mut ids = Vec::new();
        for (d, n) in [("2026-11-07", 2), ("2026-11-08", 2)] {
            for _ in 0..n {
                let (m, dir) = l.create(Kind::Capture, d, "key", "t").unwrap();
                fs::write(dir.join("can.log"), vec![0u8; 1000]).unwrap();
                ids.push(m.id);
            }
        }
        l.set_starred(&ids[0], true).unwrap();
        let per = l.entries("2026-11-08")[0].bytes;
        let gone = l.prune(per * 2 + 10, Some(&ids[3]));
        assert_eq!(gone, [ids[1].clone(), ids[2].clone()]);
        assert!(l.meta(&ids[0]).unwrap().starred);
        assert_eq!(l.days().len(), 2);
        assert!(l.prune(u64::MAX, None).is_empty());
    }
}
