//! `cargo xtask deploy` (and `image`, in image.rs): put a bundle on the Pi and install it with
//! `carpi-update apply`, the same path (rollback included) a release takes.
//! See docs/DEPLOY.md section 5.
//!
//! Sources:
//! - `--from-ci <branch|run-id>` (default: the current branch): the bundle
//!   artifact of the newest successful `release` run, via `gh run download`;
//! - `--file <bundle>`: a local bundle;
//! - `--carpid-only`: carpid and carpi cross-built here with cargo-zigbuild;
//!   the rest is reused from what's installed on the Pi.

mod image;

use anyhow::{Context, bail};
use carpi_proto::JobEvent;
use clap::{Parser, Subcommand};
use sha2::Digest;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Parser)]
#[command(about = "rnse-carpi developer tasks")]
struct Args {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Install a bundle on the Pi.
    Deploy(Deploy),
    /// A Raspberry Pi OS image (or flashed card) that sets itself up as a
    /// carpi on first boot.
    Image(Box<image::Image>),
}

#[derive(clap::Args)]
struct Deploy {
    /// CI bundle of a branch's newest successful release run, or of a run id.
    #[arg(long, value_name = "BRANCH|RUN-ID", conflicts_with_all = ["file", "carpid_only"])]
    from_ci: Option<String>,
    /// A local bundle.
    #[arg(long, conflicts_with = "carpid_only")]
    file: Option<PathBuf>,
    /// Cross-build carpid and carpi only; reuse the rest of the Pi's install.
    #[arg(long)]
    carpid_only: bool,
    /// ssh host (an alias from ~/.ssh/config works).
    #[arg(long, default_value = "carpi")]
    host: String,
    /// carpi-update on the Pi.
    #[arg(long, default_value = "/opt/carpi/current/bin/carpi-update")]
    updater: String,
}

const TARGET: &str = "aarch64-unknown-linux-gnu";
/// Trixie's glibc.
const GLIBC: &str = "2.41";
const WORKFLOW: &str = "release.yml";

fn main() -> anyhow::Result<()> {
    let root = workspace_root()?;
    let d = match Args::parse().cmd {
        Cmd::Deploy(d) => d,
        Cmd::Image(i) => return image::run(*i, &root),
    };
    let (bundle, reuse) = if let Some(file) = &d.file {
        (file.clone(), false)
    } else if d.carpid_only {
        (dev_bundle(&root, &d.host)?, true)
    } else {
        let src = match &d.from_ci {
            Some(s) => s.clone(),
            None => output(Command::new("git").args(["rev-parse", "--abbrev-ref", "HEAD"]))?,
        };
        (from_ci(&root, &src)?, false)
    };
    deliver(&bundle, &d.host, &d.updater, reuse)
}

fn workspace_root() -> anyhow::Result<PathBuf> {
    // xtask/ sits in the workspace root.
    Ok(Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("no workspace root")?
        .to_path_buf())
}

/// Runs and returns trimmed stdout; fails on a non-zero exit.
fn output(cmd: &mut Command) -> anyhow::Result<String> {
    let name = format!("{cmd:?}");
    let out = cmd
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("running {name}"))?;
    if !out.status.success() {
        bail!("{name} failed ({})", out.status);
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn run(cmd: &mut Command) -> anyhow::Result<()> {
    let name = format!("{cmd:?}");
    let status = cmd.status().with_context(|| format!("running {name}"))?;
    if !status.success() {
        bail!("{name} failed ({status})");
    }
    Ok(())
}

fn have(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn from_ci(root: &Path, src: &str) -> anyhow::Result<PathBuf> {
    if !have("gh", &["--version"]) {
        bail!("needs the GitHub CLI: https://cli.github.com, then `gh auth login`");
    }
    let run_id = if !src.is_empty() && src.bytes().all(|b| b.is_ascii_digit()) {
        src.to_string()
    } else {
        let id = output(Command::new("gh").current_dir(root).args([
            "run",
            "list",
            "--workflow",
            WORKFLOW,
            "--branch",
            src,
            "--status",
            "success",
            "--limit",
            "1",
            "--json",
            "databaseId",
            "--jq",
            ".[0].databaseId // empty",
        ]))?;
        if id.is_empty() {
            bail!("no successful {WORKFLOW} run on {src}; push it or pass --file");
        }
        id
    };
    let dir = root.join("target/deploy").join(format!("run-{run_id}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    println!("downloading the bundle of run {run_id}");
    run(Command::new("gh").current_dir(root).args([
        "run",
        "download",
        &run_id,
        "--pattern",
        "carpi-*-arm64",
        "--dir",
        &dir.to_string_lossy(),
    ]))?;
    find_bundle(&dir)?.with_context(|| format!("run {run_id} has no carpi-*-arm64.tar.zst"))
}

fn find_bundle(dir: &Path) -> anyhow::Result<Option<PathBuf>> {
    for e in std::fs::read_dir(dir)? {
        let p = e?.path();
        if p.is_dir() {
            if let Some(b) = find_bundle(&p)? {
                return Ok(Some(b));
            }
        } else if p.to_string_lossy().ends_with("-arm64.tar.zst") {
            return Ok(Some(p));
        }
    }
    Ok(None)
}

fn zigbuild_help() -> String {
    format!(
        "--carpid-only needs zig and cargo-zigbuild. Install them, then run this again:\n\
         \x20 winget install zig.zig        (or: pip install ziglang)\n\
         \x20 cargo install --locked cargo-zigbuild\n\
         \x20 rustup target add {TARGET}"
    )
}

/// carpid and carpi for the Pi, plus a manifest naming everything else as
/// it is on the Pi now.
fn dev_bundle(root: &Path, host: &str) -> anyhow::Result<PathBuf> {
    if !have("zig", &["version"]) || !have("cargo", &["zigbuild", "--version"]) {
        bail!(zigbuild_help());
    }
    let installed =
        output(Command::new("rustup").args(["target", "list", "--installed"])).unwrap_or_default();
    if !installed.lines().any(|l| l.trim() == TARGET) {
        bail!(zigbuild_help());
    }

    println!("reading the installed manifest from {host}");
    let base = output(Command::new("ssh").args([host, "cat", "/opt/carpi/current/manifest.json"]))
        .context("no install on the Pi to reuse; deploy a full bundle first")?;

    println!("building carpid and carpi for {TARGET}.{GLIBC}");
    run(Command::new("cargo").current_dir(root).args([
        "zigbuild",
        "--release",
        "--target",
        &format!("{TARGET}.{GLIBC}"),
        "-p",
        "carpid",
        "-p",
        "carpi-cli",
    ]))?;
    let out = root.join("target").join(TARGET).join("release");
    let bins: Vec<(String, Vec<u8>)> = ["carpi", "carpid"]
        .iter()
        .map(|b| {
            let p = out.join(b);
            std::fs::read(&p)
                .with_context(|| p.display().to_string())
                .map(|data| (format!("bin/{b}"), data))
        })
        .collect::<anyhow::Result<_>>()?;

    let git = |args: &[&str]| {
        output(
            Command::new("git")
                .current_dir(root)
                .env("TZ", "UTC")
                .args(args),
        )
    };
    let sha = git(&["rev-parse", "HEAD"])?;
    let dirty = !git(&["status", "--porcelain"])?.is_empty();
    let date = git(&[
        "log",
        "-1",
        "--format=%cd",
        "--date=format-local:%Y-%m-%dT%H:%M:%SZ",
    ])?;
    let version = format!("dev-{}{}", &sha[..7], if dirty { "-dirty" } else { "" });

    let manifest = dev_manifest(&base, &version, &sha, &date, &bins)?;
    let mut entries: Vec<(&str, &[u8])> = vec![("manifest.json", &manifest)];
    entries.extend(bins.iter().map(|(p, d)| (p.as_str(), d.as_slice())));
    let dir = root.join("target/deploy");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("carpi-{version}-arm64.tar.zst"));
    std::fs::write(&path, tarball(&entries)?)?;
    println!("{} (carpid and carpi only)", path.display());
    Ok(path)
}

/// The installed manifest with `bins` swapped in and this commit's identity.
fn dev_manifest(
    base: &str,
    version: &str,
    sha: &str,
    date: &str,
    bins: &[(String, Vec<u8>)],
) -> anyhow::Result<Vec<u8>> {
    let mut m: serde_json::Value = serde_json::from_str(base).context("installed manifest")?;
    if m["format_version"] != 1 {
        bail!("installed manifest has format {}", m["format_version"]);
    }
    m["version"] = version.into();
    m["channel"] = "dev".into();
    m["date"] = date.into();
    m["commits"]["rnse-carpi"] = sha.into();
    let files = m["files"]
        .as_array_mut()
        .context("installed manifest has no files")?;
    for (path, data) in bins {
        files.retain(|f| f["path"] != path.as_str());
        files.push(serde_json::json!({
            "path": path,
            "sha256": hex::encode(sha2::Sha256::digest(data)),
            "size": data.len(),
            "mode": "0755",
        }));
    }
    files.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    let mut out = serde_json::to_vec_pretty(&m)?;
    out.push(b'\n');
    Ok(out)
}

fn tarball(entries: &[(&str, &[u8])]) -> anyhow::Result<Vec<u8>> {
    let mut tar = tar::Builder::new(Vec::new());
    for (path, data) in entries {
        let mut h = tar::Header::new_ustar();
        h.set_size(data.len() as u64);
        h.set_mode(if path.starts_with("bin/") {
            0o755
        } else {
            0o644
        });
        h.set_entry_type(tar::EntryType::Regular);
        h.set_mtime(0);
        tar.append_data(&mut h, path, *data)?;
    }
    Ok(ruzstd::encoding::compress_to_vec(
        &tar.into_inner()?[..],
        ruzstd::encoding::CompressionLevel::Fastest,
    ))
}

/// Copies the bundle to the Pi and applies it, printing the progress.
fn deliver(bundle: &Path, host: &str, updater: &str, reuse: bool) -> anyhow::Result<()> {
    let name = bundle
        .file_name()
        .context("bundle has no file name")?
        .to_string_lossy()
        .into_owned();
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        bail!("odd bundle name {name:?}");
    }
    println!("copying {name} to {host}");
    // A bare name: scp on Windows may read `C:` as a host.
    let dir = bundle.parent().filter(|p| !p.as_os_str().is_empty());
    let mut scp = Command::new("scp");
    if let Some(dir) = dir {
        scp.current_dir(dir);
    }
    run(scp.args(["-q", &name, &format!("{host}:{name}")]))?;

    // Dev bundles are unsigned; direct deploy accepts them on purpose.
    let mut remote = format!("{updater} apply --file \"$HOME/{name}\" --allow-unsigned");
    if reuse {
        remote.push_str(" --reuse-current");
    }
    remote.push_str(&format!("; s=$?; rm -f \"$HOME/{name}\"; exit $s"));
    let mut child = Command::new("ssh")
        .args([host, &remote])
        .stdout(Stdio::piped())
        .spawn()
        .context("running ssh")?;
    let mut ok = None;
    let mut shown = 0u64;
    for line in std::io::BufReader::new(child.stdout.take().expect("piped")).lines() {
        let line = line?;
        match serde_json::from_str::<JobEvent>(&line) {
            Ok(e) => {
                if let JobEvent::Finished { ok: o, .. } = &e {
                    ok = Some(*o);
                }
                show(&e, &mut shown);
            }
            Err(_) => println!("{line}"),
        }
        std::io::stdout().flush()?;
    }
    let status = child.wait()?;
    match ok {
        Some(true) => Ok(()),
        Some(false) => bail!("apply failed"),
        None => bail!("ssh {host} ended ({status}) without a result"),
    }
}

/// One line per stage; progress every tenth.
fn show(e: &JobEvent, shown: &mut u64) {
    match e {
        JobEvent::Stage { stage, message } => {
            *shown = 0;
            println!("{stage:>9}  {message}");
        }
        JobEvent::Progress {
            done,
            total: Some(total),
            ..
        } if *total > 0 => {
            let tenth = done * 10 / total;
            if tenth > *shown {
                *shown = tenth;
                println!("{:>9}  {}%", "", tenth * 10);
            }
        }
        JobEvent::Progress { .. } => {}
        JobEvent::Finished { ok, message } => {
            println!("{:>9}  {message}", if *ok { "done" } else { "FAILED" });
        }
        other => println!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_manifest_swaps_binaries_and_keeps_the_rest() {
        let base = r#"{"format_version":1,"version":"beta-20261107-e7e3f15","channel":"beta",
            "arch":"arm64","date":"x","commits":{"rnse-carpi":"old","livi":"l"},
            "min_os_profile":1,"firmware":[],
            "files":[{"path":"bin/LIVI","sha256":"aa","size":1,"mode":"0755"},
                     {"path":"bin/carpid","sha256":"bb","size":2,"mode":"0755"}]}"#;
        let bins = vec![
            ("bin/carpi".to_string(), b"cli".to_vec()),
            ("bin/carpid".to_string(), b"daemon".to_vec()),
        ];
        let m = dev_manifest(
            base,
            "dev-1234567",
            "1234567abc",
            "2026-11-08T00:00:00Z",
            &bins,
        )
        .unwrap();
        let m: serde_json::Value = serde_json::from_slice(&m).unwrap();
        assert_eq!(m["version"], "dev-1234567");
        assert_eq!(m["channel"], "dev");
        assert_eq!(m["commits"]["livi"], "l");
        assert_eq!(m["commits"]["rnse-carpi"], "1234567abc");
        let paths: Vec<_> = m["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["path"].as_str().unwrap())
            .collect();
        assert_eq!(paths, ["bin/LIVI", "bin/carpi", "bin/carpid"]);
        assert_eq!(m["files"][2]["size"], 6);
        assert_eq!(m["files"][0]["sha256"], "aa");
    }

    #[test]
    fn finds_the_bundle_in_an_artifact_dir() {
        let d = std::env::temp_dir().join(format!("xtask-{}", std::process::id()));
        let sub = d.join("carpi-dev-1234567-arm64");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("manifest.json"), "{}").unwrap();
        std::fs::write(sub.join("carpi-dev-1234567-arm64.tar.zst"), "x").unwrap();
        let found = find_bundle(&d).unwrap().unwrap();
        assert!(found.ends_with("carpi-dev-1234567-arm64.tar.zst"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
