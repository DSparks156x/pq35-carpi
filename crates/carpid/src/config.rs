use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// `/etc/carpi/carpid.toml` (or `--config`).
///
/// ```toml
/// [bus.ican]
/// iface = "ican"       # any netdev name; name interfaces with .link files
///
/// [bus.diag]
/// iface = "ican"       # same interface on the TT; "obd" where the gateway won't route diag
///
/// [api]
/// socket = "/run/user/1000/carpid.sock"
///
/// [recorder]
/// minutes = 10
/// dir = "/var/lib/carpi/captures"
/// ```
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub bus: BTreeMap<String, Bus>,
    #[serde(default)]
    pub api: Api,
    #[serde(default)]
    pub recorder: RecorderCfg,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bus {
    pub iface: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Api {
    #[serde(default = "default_socket")]
    pub socket: PathBuf,
}

impl Default for Api {
    fn default() -> Self {
        Api {
            socket: default_socket(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecorderCfg {
    #[serde(default = "default_minutes")]
    pub minutes: u64,
    #[serde(default = "default_capture_dir")]
    pub dir: PathBuf,
}

impl Default for RecorderCfg {
    fn default() -> Self {
        RecorderCfg {
            minutes: default_minutes(),
            dir: default_capture_dir(),
        }
    }
}

/// `$XDG_RUNTIME_DIR/carpid.sock`, so LIVI (same user) can reach it.
pub fn default_socket() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("carpid.sock")
}

fn default_minutes() -> u64 {
    10
}

fn default_capture_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("captures")
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let cfg: Config = toml::from_str(&text)?;
        if !cfg.bus.contains_key("ican") {
            anyhow::bail!("{}: [bus.ican] is required", path.display());
        }
        Ok(cfg)
    }

    /// Default when no file exists: ICAN and diag both on `can0`.
    pub fn fallback() -> Self {
        let mut bus = BTreeMap::new();
        bus.insert(
            "ican".into(),
            Bus {
                iface: "can0".into(),
            },
        );
        bus.insert(
            "diag".into(),
            Bus {
                iface: "can0".into(),
            },
        );
        Config {
            bus,
            api: Api::default(),
            recorder: RecorderCfg::default(),
        }
    }

    /// Interfaces with the roles each carries, in a stable order.
    pub fn ifaces(&self) -> Vec<carpi_core::IfaceSpec> {
        let mut out: Vec<carpi_core::IfaceSpec> = Vec::new();
        for (role, bus) in &self.bus {
            match out.iter_mut().find(|i| i.name == bus.iface) {
                Some(i) => i.roles.push(role.clone()),
                None => out.push(carpi_core::IfaceSpec {
                    name: bus.iface.clone(),
                    roles: vec![role.clone()],
                }),
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_and_separate_ifaces() {
        let cfg: Config =
            toml::from_str("[bus.ican]\niface='ican'\n[bus.diag]\niface='ican'\n").unwrap();
        let i = cfg.ifaces();
        assert_eq!(i.len(), 1);
        assert_eq!(i[0].roles, vec!["diag".to_string(), "ican".to_string()]);

        let cfg: Config =
            toml::from_str("[bus.ican]\niface='ican'\n[bus.diag]\niface='obd'\n").unwrap();
        assert_eq!(cfg.ifaces().len(), 2);
    }

    #[test]
    fn rejects_typos() {
        assert!(toml::from_str::<Config>("[bus.ican]\nifcae='x'\n").is_err());
    }
}
