//! `/opt/carpi` (or a test root):
//!
//! ```text
//! releases/<version>/      unpacked bundles, at most current and previous
//! current -> releases/<v>  what services run
//! previous -> releases/<v> what a rollback goes back to
//! downloads/<version>/     fetched, verified, not applied yet
//! state.json               last download and how the last apply ended
//! ```
//!
//! `current` and `previous` are symlinks swapped with rename(2), so a
//! reader sees the old or the new target, never neither. Off Unix (tests on
//! Windows) they are small files holding the same relative path.

use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

pub const CURRENT: &str = "current";
pub const PREVIOUS: &str = "previous";
const RELEASES: &str = "releases";
const STAGING: &str = ".staging";

pub struct Layout {
    pub root: PathBuf,
}

#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct State {
    pub downloaded: Option<String>,
    pub last_apply: Option<String>,
}

impl Layout {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Layout { root: root.into() }
    }

    pub fn releases(&self) -> PathBuf {
        self.root.join(RELEASES)
    }

    pub fn release(&self, version: &str) -> PathBuf {
        self.releases().join(version)
    }

    pub fn staging(&self) -> PathBuf {
        self.releases().join(STAGING)
    }

    pub fn download_dir(&self, version: &str) -> PathBuf {
        self.root.join("downloads").join(version)
    }

    pub fn bundle_name(version: &str) -> String {
        format!("carpi-{version}-arm64.tar.zst")
    }

    pub fn downloaded_bundle(&self, version: &str) -> PathBuf {
        self.download_dir(version).join(Self::bundle_name(version))
    }

    pub fn current(&self) -> Option<String> {
        self.pointer(CURRENT)
    }

    pub fn previous(&self) -> Option<String> {
        self.pointer(PREVIOUS)
    }

    /// The version a pointer names, if it names an existing release.
    fn pointer(&self, name: &str) -> Option<String> {
        let target = read_pointer(&self.root.join(name))?;
        let version = target.strip_prefix("releases/")?.to_string();
        (crate::manifest::valid_version(&version) && self.release(&version).is_dir())
            .then_some(version)
    }

    fn set_pointer(&self, name: &str, version: &str) -> io::Result<()> {
        let tmp = self.root.join(format!(".{name}.tmp"));
        let _ = std::fs::remove_file(&tmp);
        write_pointer(&tmp, &format!("{RELEASES}/{version}"))?;
        std::fs::rename(&tmp, self.root.join(name))
    }

    fn clear_pointer(&self, name: &str) -> io::Result<()> {
        match std::fs::remove_file(self.root.join(name)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// `current` to `version`; the old current becomes `previous`. Switching
    /// to what's already current leaves `previous` alone.
    pub fn switch_to(&self, version: &str) -> io::Result<()> {
        if !self.release(version).is_dir() {
            return Err(io::Error::other(format!("no release {version}")));
        }
        match self.current() {
            Some(cur) if cur != version => self.set_pointer(PREVIOUS, &cur)?,
            _ => {}
        }
        self.set_pointer(CURRENT, version)
    }

    /// Swaps `current` and `previous`. Returns the version now current.
    pub fn rollback(&self) -> io::Result<String> {
        let prev = self
            .previous()
            .ok_or_else(|| io::Error::other("no previous release to go back to"))?;
        match self.current() {
            Some(cur) => self.set_pointer(PREVIOUS, &cur)?,
            None => self.clear_pointer(PREVIOUS)?,
        }
        self.set_pointer(CURRENT, &prev)?;
        Ok(prev)
    }

    /// Removes every release that isn't current or previous, and leftovers.
    pub fn prune(&self) -> io::Result<Vec<String>> {
        let keep = [self.current(), self.previous()];
        let mut removed = Vec::new();
        let Ok(dir) = std::fs::read_dir(self.releases()) else {
            return Ok(removed);
        };
        for e in dir {
            let e = e?;
            let name = e.file_name().to_string_lossy().into_owned();
            if keep.iter().flatten().any(|k| *k == name) {
                continue;
            }
            std::fs::remove_dir_all(e.path())?;
            removed.push(name);
        }
        Ok(removed)
    }

    pub fn state(&self) -> State {
        std::fs::read(self.root.join("state.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save_state(&self, s: &State) -> io::Result<()> {
        let tmp = self.root.join(".state.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(s)?)?;
        std::fs::rename(tmp, self.root.join("state.json"))
    }
}

#[cfg(unix)]
fn write_pointer(path: &Path, target: &str) -> io::Result<()> {
    std::os::unix::fs::symlink(target, path)
}

#[cfg(unix)]
fn read_pointer(path: &Path) -> Option<String> {
    std::fs::read_link(path)
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

#[cfg(not(unix))]
fn write_pointer(path: &Path, target: &str) -> io::Result<()> {
    std::fs::write(path, target)
}

#[cfg(not(unix))]
fn read_pointer(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// Holds `<root>/update.lock` while alive, so two updaters don't interleave.
pub struct Lock(PathBuf);

impl Lock {
    pub fn take(root: &Path) -> io::Result<Lock> {
        std::fs::create_dir_all(root)?;
        let path = root.join("update.lock");
        for _ in 0..2 {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut f) => {
                    use std::io::Write;
                    let _ = write!(f, "{}", std::process::id());
                    return Ok(Lock(path));
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    let pid = std::fs::read_to_string(&path).unwrap_or_default();
                    if alive(pid.trim()) {
                        return Err(io::Error::other(format!(
                            "another carpi-update (pid {}) is running",
                            pid.trim()
                        )));
                    }
                    std::fs::remove_file(&path)?;
                }
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::other("can't take update.lock"))
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn alive(pid: &str) -> bool {
    if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    if cfg!(target_os = "linux") {
        Path::new("/proc").join(pid).exists()
    } else {
        // Can't tell; trust the lock.
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root_with(versions: &[&str]) -> (tempfile::TempDir, Layout) {
        let dir = tempfile::tempdir().unwrap();
        let l = Layout::new(dir.path());
        for v in versions {
            std::fs::create_dir_all(l.release(v).join("bin")).unwrap();
        }
        (dir, l)
    }

    #[test]
    fn switch_keeps_one_previous_and_rolls_back() {
        let (_d, l) = root_with(&["1", "2", "3"]);
        assert_eq!(l.current(), None);
        l.switch_to("1").unwrap();
        assert_eq!((l.current(), l.previous()), (Some("1".into()), None));
        l.switch_to("2").unwrap();
        assert_eq!(
            (l.current(), l.previous()),
            (Some("2".into()), Some("1".into()))
        );
        // Reinstalling the current one leaves previous alone.
        l.switch_to("2").unwrap();
        assert_eq!(l.previous(), Some("1".into()));
        l.switch_to("3").unwrap();
        assert_eq!(l.rollback().unwrap(), "2");
        assert_eq!(
            (l.current(), l.previous()),
            (Some("2".into()), Some("3".into()))
        );
        assert!(l.switch_to("9").is_err());
        #[cfg(unix)]
        assert!(
            std::fs::symlink_metadata(l.root.join(CURRENT))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn prune_keeps_two() {
        let (_d, l) = root_with(&["1", "2", "3"]);
        std::fs::create_dir_all(l.staging()).unwrap();
        l.switch_to("2").unwrap();
        l.switch_to("3").unwrap();
        let mut removed = l.prune().unwrap();
        removed.sort();
        assert_eq!(removed, [".staging", "1"]);
        assert!(l.release("2").is_dir() && l.release("3").is_dir());
    }

    #[test]
    fn no_rollback_without_previous() {
        let (_d, l) = root_with(&["1"]);
        l.switch_to("1").unwrap();
        assert!(l.rollback().is_err());
        assert_eq!(l.current(), Some("1".into()));
    }

    #[test]
    fn lock_is_exclusive_and_released() {
        let d = tempfile::tempdir().unwrap();
        let a = Lock::take(d.path()).unwrap();
        assert!(Lock::take(d.path()).is_err());
        drop(a);
        assert!(Lock::take(d.path()).is_ok());
    }
}
