//! carpi-update: installs carpi bundles (docs/DEPLOY.md section 3,
//! docs/BUNDLE.md).
//!
//! Separate from carpid because it replaces carpid. Progress goes to stdout
//! as one JSON [`carpi_proto::JobEvent`] per line, ending with `finished`;
//! carpid relays them to the UI. Write errors on stdout are ignored: when
//! the apply restarts carpid, the reader goes away and the update carries on.
//!
//! Never touches user config (`~/.config/...`); `share/` in a bundle is
//! reference only.

mod apply;
mod bundle;
mod config;
mod github;
mod layout;
mod manifest;
mod profile;
mod system;

use carpi_proto::{Installed, JobEvent, ReleaseInfo};
use clap::{Parser, Subcommand};
use layout::{Layout, Lock};
use minisign_verify::PublicKey;
use std::io::Write;
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about = "Install carpi bundles")]
struct Args {
    /// carpid's config, for `[update]` and `[api]` (default:
    /// ~/.config/carpi/carpid.toml, else /etc/carpi/carpid.toml).
    #[arg(long)]
    config: Option<PathBuf>,
    /// Install root.
    #[arg(long, default_value = "/opt/carpi")]
    root: PathBuf,
    /// OS profile stamp written by os/profile.sh.
    #[arg(long, default_value = "/etc/carpi-profile")]
    profile: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List releases on the configured repo and channel.
    Check,
    /// Download and verify the newest release on the channel.
    Fetch {
        /// Even if it's installed or downloaded already.
        #[arg(long)]
        force: bool,
    },
    /// Install a bundle: the downloaded one, or `--file`.
    Apply {
        #[arg(long)]
        file: Option<PathBuf>,
        /// Accept an unsigned `--file` although this device has a key.
        #[arg(long, requires = "file")]
        allow_unsigned: bool,
        /// Files the bundle lists but doesn't carry come from the current
        /// release (dev bundles from `cargo xtask deploy --carpid-only`).
        #[arg(long, requires = "file")]
        reuse_current: bool,
    },
    /// Go back to the previous release.
    Rollback,
    /// What's installed and downloaded.
    Status,
}

fn emit(e: JobEvent) {
    if let Ok(line) = serde_json::to_string(&e) {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    }
}

fn stage(stage: &str, message: impl Into<String>) {
    emit(JobEvent::Stage {
        stage: stage.into(),
        message: message.into(),
    });
}

fn main() -> std::process::ExitCode {
    let args = Args::parse();
    let result = run(&args);
    let ok = result.is_ok();
    emit(JobEvent::Finished {
        ok,
        message: result.unwrap_or_else(|e| e),
    });
    if ok {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}

fn run(args: &Args) -> Result<String, String> {
    let path = args.config.clone().unwrap_or_else(config::default_path);
    let cfg = config::Config::load(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let layout = Layout::new(&args.root);
    let os_profile = profile::read(&args.profile)?;
    let opts = apply::Opts {
        window: apply::HEALTH_WINDOW,
        poll: apply::HEALTH_POLL,
        os_profile,
    };
    match &args.cmd {
        Cmd::Status => {
            emit(JobEvent::Installed(installed(&layout, os_profile)));
            Ok("status".into())
        }
        Cmd::Check => check(&cfg, &layout),
        Cmd::Fetch { force } => {
            let _lock = lock(&layout)?;
            fetch(&cfg, &layout, os_profile, *force)
        }
        Cmd::Apply {
            file,
            allow_unsigned,
            reuse_current,
        } => {
            let _lock = lock(&layout)?;
            let file = match file {
                Some(f) => f.clone(),
                None => {
                    let v = layout
                        .state()
                        .downloaded
                        .ok_or("nothing downloaded; fetch first or pass --file")?;
                    layout.downloaded_bundle(&v)
                }
            };
            let key = public_key(&cfg)?;
            let base = match reuse_current {
                false => None,
                true => Some(
                    layout.release(
                        &layout
                            .current()
                            .ok_or("--reuse-current: nothing is installed")?,
                    ),
                ),
            };
            let policy = bundle::Policy {
                key: key.as_ref(),
                allow_unsigned: *allow_unsigned,
                expect_manifest: None,
                fill_from: base.as_deref(),
            };
            let mut sys = system::Real::new(&cfg);
            match apply::apply(&layout, &file, &policy, &mut sys, &opts, &mut emit)? {
                apply::Outcome::Applied(v) => Ok(format!("{v} installed")),
                apply::Outcome::RolledBack {
                    failed,
                    back_to,
                    why,
                } => Err(format!("{failed} rolled back to {back_to}: {why}")),
            }
        }
        Cmd::Rollback => {
            let _lock = lock(&layout)?;
            let back = layout.rollback().map_err(|e| e.to_string())?;
            stage("rollback", format!("current -> {back}"));
            let mut sys = system::Real::new(&cfg);
            apply::System::restart(&mut sys)?;
            apply::wait_healthy(&mut sys, &opts)?;
            Ok(format!("back on {back}"))
        }
    }
}

fn lock(layout: &Layout) -> Result<Lock, String> {
    Lock::take(&layout.root).map_err(|e| format!("{}: {e}", layout.root.display()))
}

fn installed(layout: &Layout, os_profile: Option<u32>) -> Installed {
    let st = layout.state();
    Installed {
        current: layout.current(),
        previous: layout.previous(),
        downloaded: st
            .downloaded
            .filter(|v| layout.downloaded_bundle(v).is_file()),
        last_apply: st.last_apply,
        os_profile,
    }
}

/// The configured key, if its file exists.
fn public_key(cfg: &config::Config) -> Result<Option<PublicKey>, String> {
    let path = &cfg.update.pubkey;
    if !path.exists() {
        stage(
            "verify",
            format!("no public key at {}: checksums only", path.display()),
        );
        return Ok(None);
    }
    PublicKey::from_file(path)
        .map(Some)
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn check(cfg: &config::Config, layout: &Layout) -> Result<String, String> {
    let u = &cfg.update;
    stage("check", format!("{} ({})", u.repo, u.channel));
    let releases = github::fetch_releases(&u.repo)?;
    let current = layout.current();
    let items: Vec<ReleaseInfo> = github::candidates(&releases, &u.channel)
        .into_iter()
        .map(|c| ReleaseInfo {
            installed: current.as_deref() == Some(c.version.as_str()),
            version: c.version,
            channel: c.channel.into(),
            published: c.published,
            size: c.bundle.size,
        })
        .collect();
    let summary = match items.first() {
        None => format!("no {} releases in {}", u.channel, u.repo),
        Some(r) if r.installed => format!("{} is installed, up to date", r.version),
        Some(r) => format!("{} available", r.version),
    };
    emit(JobEvent::Releases { items });
    Ok(summary)
}

fn fetch(
    cfg: &config::Config,
    layout: &Layout,
    os_profile: Option<u32>,
    force: bool,
) -> Result<String, String> {
    let u = &cfg.update;
    stage("check", format!("{} ({})", u.repo, u.channel));
    let releases = github::fetch_releases(&u.repo)?;
    let c = github::candidates(&releases, &u.channel)
        .into_iter()
        .next()
        .ok_or_else(|| format!("no {} releases in {}", u.channel, u.repo))?;
    let v = c.version.clone();
    if !force && layout.current().as_deref() == Some(v.as_str()) {
        return Ok(format!("{v} is installed, up to date"));
    }
    let bundle_path = layout.downloaded_bundle(&v);
    let mut state = layout.state();
    if !force && state.downloaded.as_deref() == Some(v.as_str()) && bundle_path.is_file() {
        return Ok(format!("{v} is downloaded already"));
    }

    let key = public_key(cfg)?;
    let dir = layout.download_dir(&v);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;

    // The release's own manifest first: signature and OS profile before
    // spending the bandwidth.
    let outer = match &c.manifest {
        Some(a) => {
            let bytes = github::fetch_small(&a.browser_download_url, manifest::MAX_META)?;
            let m = manifest::Manifest::parse(&bytes)?;
            let sig = match &c.signature {
                Some(s) => Some(
                    String::from_utf8(github::fetch_small(
                        &s.browser_download_url,
                        manifest::MAX_META,
                    )?)
                    .map_err(|_| "signature is not text")?,
                ),
                None => None,
            };
            let trust =
                manifest::verify_signature(&bytes, &m, sig.as_deref(), key.as_ref(), false)?;
            profile::check(m.min_os_profile, os_profile)?;
            stage("verify", format!("{v}: manifest {trust:?}"));
            Some(bytes)
        }
        None => None,
    };

    stage(
        "download",
        format!("{} ({} bytes)", c.bundle.name, c.bundle.size),
    );
    let downloaded = github::download(
        &c.bundle.browser_download_url,
        &bundle_path,
        &mut |done, total| {
            emit(JobEvent::Progress {
                stage: "download".into(),
                done,
                total,
            })
        },
    );
    if let Err(e) = downloaded {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(e);
    }

    stage("verify", format!("checking {}", c.bundle.name));
    let policy = bundle::Policy {
        key: key.as_ref(),
        allow_unsigned: false,
        expect_manifest: outer.as_deref(),
        fill_from: None,
    };
    let verified = std::fs::File::open(&bundle_path)
        .map_err(|e| e.to_string())
        .and_then(|file| {
            bundle::read(
                std::io::BufReader::new(file),
                &policy,
                &mut bundle::Verify,
                &mut |done, total| {
                    emit(JobEvent::Progress {
                        stage: "verify".into(),
                        done,
                        total: Some(total),
                    })
                },
            )
        })
        .and_then(|r| {
            profile::check(r.manifest.min_os_profile, os_profile)?;
            if r.manifest.version != v {
                return Err(format!(
                    "bundle says {}, release says {v}",
                    r.manifest.version
                ));
            }
            Ok(r)
        });
    if let Err(e) = verified {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(e);
    }

    // Only the newest download is kept.
    if let Some(old) = state.downloaded.replace(v.clone())
        && old != v
    {
        let _ = std::fs::remove_dir_all(layout.download_dir(&old));
    }
    layout.save_state(&state).map_err(|e| e.to_string())?;
    Ok(format!("{v} downloaded and verified"))
}
