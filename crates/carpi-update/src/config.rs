//! The `[update]` and `[api]` sections of carpid's config. Other sections
//! are carpid's business and ignored here.

use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Deserialize)]
struct File {
    #[serde(default)]
    update: Update,
    #[serde(default)]
    api: Api,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Update {
    /// `owner/name` on GitHub. A fork points this at itself.
    pub repo: String,
    /// `stable` or `beta`.
    pub channel: String,
    /// minisign public key file. Bundles must be signed by it when it exists.
    pub pubkey: PathBuf,
    /// User units restarted after a switch (`systemctl --user restart`).
    pub restart_user: Vec<String>,
    /// System units restarted after a switch (`sudo -n systemctl restart`).
    pub restart_system: Vec<String>,
    /// Health check also wants CAN frames to arrive. Off by default: a
    /// parked car's bus is silent.
    pub check_frames: bool,
}

impl Default for Update {
    fn default() -> Self {
        Update {
            repo: "DSparks156x/rnse-carpi".into(),
            channel: "stable".into(),
            pubkey: "/etc/carpi/minisign.pub".into(),
            restart_user: vec!["carpid.service".into()],
            restart_system: vec!["livi-kiosk.service".into()],
            check_frames: false,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct Api {
    socket: Option<PathBuf>,
}

#[derive(Debug)]
pub struct Config {
    pub update: Update,
    pub socket: PathBuf,
}

impl Config {
    /// Reads `path`; a missing file gives the defaults.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let file: File = match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => File::default(),
            Err(e) => return Err(e.into()),
        };
        Self::from_file(file)
    }

    fn from_file(file: File) -> anyhow::Result<Self> {
        let u = &file.update;
        if !valid_repo(&u.repo) {
            anyhow::bail!("[update] repo {:?} is not owner/name", u.repo);
        }
        if u.channel != "stable" && u.channel != "beta" {
            anyhow::bail!(
                "[update] channel must be stable or beta, not {:?}",
                u.channel
            );
        }
        let socket = file.api.socket.clone().unwrap_or_else(|| {
            std::env::var_os("XDG_RUNTIME_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir)
                .join("carpid.sock")
        });
        Ok(Config {
            update: file.update,
            socket,
        })
    }
}

/// `~/.config/carpi/carpid.toml`, else `/etc/carpi/carpid.toml`.
pub fn default_path() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        let p = PathBuf::from(home).join(".config/carpi/carpid.toml");
        if p.exists() {
            return p;
        }
    }
    "/etc/carpi/carpid.toml".into()
}

fn valid_repo(repo: &str) -> bool {
    let ok = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    matches!(repo.split_once('/'), Some((o, n)) if ok(o) && ok(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_update_and_ignores_carpid_sections() {
        let file: File = toml::from_str(
            "[bus.ican]\niface='can0'\n[update]\nrepo='someone/pq35-fork'\nchannel='beta'\n",
        )
        .unwrap();
        let c = Config::from_file(file).unwrap();
        assert_eq!(c.update.repo, "someone/pq35-fork");
        assert_eq!(c.update.channel, "beta");
        assert_eq!(c.update.restart_user, ["carpid.service"]);
    }

    #[test]
    fn refuses_bad_repo_and_channel() {
        let bad = |t: &str| Config::from_file(toml::from_str(t).unwrap()).is_err();
        assert!(bad("[update]\nrepo='nope'\n"));
        assert!(bad("[update]\nrepo='a/b/../c'\n"));
        assert!(bad("[update]\nchannel='nightly'\n"));
    }
}
