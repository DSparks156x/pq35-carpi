//! `/etc/carpi-profile`, written by `os/profile.sh`. Holds a line
//! `PROFILE_VERSION=<n>`; a bundle names the lowest `n` it runs on.

use std::path::Path;

/// The profile version in the file's text, if any.
pub fn parse(text: &str) -> Option<u32> {
    text.lines().find_map(|l| {
        let l = l.trim();
        let v = l
            .strip_prefix("PROFILE_VERSION")?
            .trim_start()
            .strip_prefix('=')?;
        v.trim().trim_matches(['"', '\'']).parse().ok()
    })
}

/// `Ok(None)` when the file doesn't exist.
pub fn read(path: &Path) -> Result<Option<u32>, String> {
    match std::fs::read_to_string(path) {
        Ok(t) => parse(&t)
            .map(Some)
            .ok_or_else(|| format!("{}: no PROFILE_VERSION line", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Refuses a bundle that needs a newer OS profile than this device has.
pub fn check(needs: u32, have: Option<u32>) -> Result<(), String> {
    match have {
        _ if needs == 0 => Ok(()),
        Some(h) if h >= needs => Ok(()),
        Some(h) => Err(format!(
            "this bundle needs OS profile {needs}, the device has {h}. \
             Run os/profile.sh from the bundle's pq35-carpi commit first."
        )),
        None => Err(format!(
            "this bundle needs OS profile {needs} and the device has none \
             (no /etc/carpi-profile). Run os/profile.sh first."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shell_style() {
        assert_eq!(parse("# carpi\nPROFILE_VERSION=3\nHAT=carpihat\n"), Some(3));
        assert_eq!(parse("PROFILE_VERSION = \"12\"\n"), Some(12));
        assert_eq!(parse("HAT=x\n"), None);
    }

    #[test]
    fn version_check() {
        assert!(check(2, Some(2)).is_ok());
        assert!(check(2, Some(5)).is_ok());
        assert!(
            check(3, Some(2))
                .unwrap_err()
                .contains("needs OS profile 3")
        );
        assert!(check(1, None).is_err());
        assert!(check(0, None).is_ok());
    }

    #[test]
    fn missing_file_is_none() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(read(&d.path().join("carpi-profile")), Ok(None));
        std::fs::write(d.path().join("p"), "PROFILE_VERSION=4\n").unwrap();
        assert_eq!(read(&d.path().join("p")), Ok(Some(4)));
    }
}
