//! Remote access (docs/IMAGE-TOOL.md): VNC on the desktop session, through the wayvnc user
//! unit os/install.sh puts in. ssh is always on and isn't controlled here. VNC at startup is on
//! for setup; off makes boot faster.

use carpi_proto::{RemotePatch, RemoteSettings};
use std::path::PathBuf;
use std::process::Command;

const UNIT: &str = "wayvnc.service";

fn units() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/systemd/user"))
}

fn systemctl(args: &[&str]) -> Option<String> {
    Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

pub fn read() -> RemoteSettings {
    let dir = units();
    let installed = dir.as_ref().is_some_and(|d| d.join(UNIT).exists());
    RemoteSettings {
        vnc_installed: installed,
        vnc_at_boot: installed
            && dir
                .as_ref()
                .is_some_and(|d| d.join("default.target.wants").join(UNIT).exists()),
        vnc_running: installed && systemctl(&["is-active", UNIT]).as_deref() == Some("active"),
    }
}

/// Starts, stops, enables or disables the unit. `--no-block`: starting waits for the desktop
/// session, and the settings reply shouldn't.
pub fn apply(p: &RemotePatch) -> Result<(), String> {
    if !read().vnc_installed {
        return Err("VNC isn't installed (no wayvnc unit)".into());
    }
    let run = |args: &[&str]| -> Result<(), String> {
        let st = Command::new("systemctl")
            .arg("--user")
            .args(args)
            .status()
            .map_err(|e| format!("systemctl: {e}"))?;
        if st.success() {
            Ok(())
        } else {
            Err(format!("systemctl --user {} failed", args.join(" ")))
        }
    };
    if let Some(on) = p.vnc_at_boot {
        run(&[if on { "enable" } else { "disable" }, UNIT])?;
    }
    if let Some(on) = p.vnc_running {
        run(&["--no-block", if on { "start" } else { "stop" }, UNIT])?;
    }
    Ok(())
}
