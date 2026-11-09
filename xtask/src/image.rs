//! `cargo xtask image`: a stock Raspberry Pi OS Trixie image that turns
//! itself into a carpi on first boot. DEPLOY.md section 7.
//!
//! Writes cloud-init's `user-data`, `network-config` and `meta-data` and a
//! `carpi/` folder (bundle, carpi-update, os/firstboot.sh, profile.sh,
//! install.sh, setup.env) into the image's boot partition, or onto a card
//! already flashed with the stock image (`--boot E:\`). On first boot
//! cloud-init makes the user, joins the Wi-Fi and runs firstboot.sh, which
//! profiles the OS, installs the bundle and reboots.

use anyhow::{Context, bail};
use std::io::{BufRead, IsTerminal, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(clap::Args)]
pub struct Image {
    /// Raspberry Pi OS Trixie image, `.img` or `.img.xz`. It isn't changed:
    /// the result goes to `--out`.
    #[arg(long, required_unless_present = "boot", conflicts_with = "boot")]
    image: Option<PathBuf>,
    /// The patched image (default: `<input name>-carpi.img` next to the input).
    #[arg(long, conflicts_with = "boot")]
    out: Option<PathBuf>,
    /// Instead of an image: the boot partition of a card or NVMe already
    /// flashed with the stock image, e.g. `E:\`.
    #[arg(long)]
    boot: Option<PathBuf>,
    /// The bundle to install (default: the CI bundle of the current branch).
    #[arg(long)]
    bundle: Option<PathBuf>,
    /// CI bundle of a branch's newest successful release run, or of a run id.
    #[arg(long, value_name = "BRANCH|RUN-ID", conflicts_with = "bundle")]
    from_ci: Option<String>,
    #[arg(long, default_value = "carpi")]
    hostname: String,
    #[arg(long, default_value = "pi")]
    user: String,
    /// ssh public key for the user (default: the key of the `carpi` ssh alias).
    #[arg(long)]
    ssh_key: Option<PathBuf>,
    /// Ask for a login password. VNC and the console need one; ssh is
    /// key-only either way.
    #[arg(long)]
    password: bool,
    /// The Pi's timezone, e.g. America/Chicago (os/profile.sh --tz).
    #[arg(long)]
    tz: Option<String>,
    #[arg(long, default_value = "carpihat")]
    hat: String,
    /// Wi-Fi country (regulatory domain).
    #[arg(long, default_value = "US")]
    country: String,
    /// A netplan network-config to use as it is, instead of asking for Wi-Fi.
    #[arg(long, conflicts_with = "no_wifi")]
    network_config: Option<PathBuf>,
    /// No Wi-Fi; wired only.
    #[arg(long)]
    no_wifi: bool,
    /// Open network-config in an editor before it's written.
    #[arg(long)]
    edit: bool,
}

/// What goes on the boot partition: path (forward slashes) and contents.
type Files = Vec<(String, Vec<u8>)>;

pub fn run(a: Image, root: &Path) -> anyhow::Result<()> {
    let bundle = match (&a.bundle, &a.from_ci) {
        (Some(b), _) => b.clone(),
        (None, src) => {
            let src = match src {
                Some(s) => s.clone(),
                None => {
                    crate::output(Command::new("git").args(["rev-parse", "--abbrev-ref", "HEAD"]))?
                }
            };
            crate::from_ci(root, &src)?
        }
    };
    let key = ssh_key(a.ssh_key.as_deref())?;
    let files = files(&a, root, &bundle, &key)?;
    let total: usize = files.iter().map(|(_, d)| d.len()).sum();
    println!("{} files, {} MB", files.len(), total / 1_000_000);

    if let Some(boot) = &a.boot {
        if !boot.join("config.txt").exists() || !boot.join("cmdline.txt").exists() {
            bail!(
                "{} isn't a Raspberry Pi boot partition (no config.txt/cmdline.txt)",
                boot.display()
            );
        }
        for (p, d) in &files {
            let path = boot.join(p);
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&path, d).with_context(|| format!("writing {}", path.display()))?;
        }
        println!(
            "written to {}; eject it, then boot the Pi with network",
            boot.display()
        );
        return Ok(());
    }

    let input = a.image.as_deref().context("--image or --boot")?;
    let out = match &a.out {
        Some(o) => o.clone(),
        None => {
            let name = input.file_name().unwrap_or_default().to_string_lossy();
            let stem = name.trim_end_matches(".xz").trim_end_matches(".img");
            input.with_file_name(format!("{stem}-carpi.img"))
        }
    };
    if out == input {
        bail!("--out is the input image");
    }
    unpack(input, &out)?;
    let f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&out)?;
    patch(f, &files).with_context(|| format!("patching {}", out.display()))?;
    println!(
        "wrote {}\nflash it with Raspberry Pi Imager (\"Use custom\", and no OS customisation: the image has its own)",
        out.display()
    );
    Ok(())
}

// ---- What goes on the card ----

fn files(a: &Image, root: &Path, bundle: &Path, key: &str) -> anyhow::Result<Files> {
    let name = bundle
        .file_name()
        .context("bundle path")?
        .to_string_lossy()
        .to_string();
    let data = std::fs::read(bundle).with_context(|| format!("reading {}", bundle.display()))?;
    let inside = bundle_files(
        &data,
        &[
            "bin/carpi-update",
            "share/os/profile.sh",
            "share/os/install.sh",
            "share/os/firstboot.sh",
        ],
    )?;
    let from = |n: &str| -> anyhow::Result<Vec<u8>> {
        // Scripts from the bundle, so they match it; the repo's for older bundles.
        match inside.iter().find(|(p, _)| p == &format!("share/os/{n}")) {
            Some((_, d)) => Ok(lf(d)),
            None => Ok(lf(&std::fs::read(root.join("os").join(n))?)),
        }
    };
    let updater = inside
        .iter()
        .find(|(p, _)| p == "bin/carpi-update")
        .map(|(_, d)| d.clone())
        .context("the bundle has no bin/carpi-update")?;

    let password = if a.password {
        Some(ask_password()?)
    } else {
        None
    };
    let network = network_config(a, root)?;
    let mut setup = format!("CARPI_USER={}\nCARPI_HAT={}\n", a.user, a.hat);
    if let Some(tz) = &a.tz {
        setup.push_str(&format!("CARPI_TZ={tz}\n"));
    }
    Ok(vec![
        (
            "user-data".into(),
            user_data(a, key, password.as_deref())?.into_bytes(),
        ),
        ("network-config".into(), network.into_bytes()),
        ("meta-data".into(), meta_data().into_bytes()),
        ("carpi/setup.env".into(), setup.into_bytes()),
        ("carpi/firstboot.sh".into(), from("firstboot.sh")?),
        ("carpi/profile.sh".into(), from("profile.sh")?),
        ("carpi/install.sh".into(), from("install.sh")?),
        ("carpi/carpi-update".into(), updater),
        (format!("carpi/{name}"), data),
    ])
}

/// LF line endings: a Windows checkout may have turned them into CRLF.
fn lf(d: &[u8]) -> Vec<u8> {
    String::from_utf8_lossy(d)
        .replace("\r\n", "\n")
        .into_bytes()
}

/// The named files out of a `.tar.zst` bundle.
fn bundle_files(data: &[u8], want: &[&str]) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
    let zst = ruzstd::decoding::StreamingDecoder::new(data).context("bundle isn't zstd")?;
    let mut ar = tar::Archive::new(zst);
    let mut out = Vec::new();
    for e in ar.entries()? {
        let mut e = e?;
        let p = e
            .path()?
            .to_string_lossy()
            .trim_start_matches("./")
            .to_string();
        if want.contains(&p.as_str()) {
            let mut d = Vec::new();
            e.read_to_end(&mut d)?;
            out.push((p, d));
        }
    }
    Ok(out)
}

/// A YAML string (YAML reads JSON strings).
fn q(s: &str) -> String {
    serde_json::to_string(s).expect("string")
}

fn user_data(a: &Image, key: &str, password: Option<&str>) -> anyhow::Result<String> {
    let pw = match password {
        Some(hash) => format!("    lock_passwd: false\n    passwd: {}\n", q(hash)),
        None => "    lock_passwd: true\n".into(),
    };
    Ok(format!(
        "#cloud-config
# Written by `cargo xtask image` (pq35-carpi). First boot runs carpi/firstboot.sh.
hostname: {host}
manage_etc_hosts: true
# No RTC: the clock is wrong until NTP, and apt would call the archive's files not yet valid.
apt:
  conf: |
    Acquire {{
      Check-Date \"false\";
    }};
keyboard:
  model: pc105
  layout: \"us\"
users:
  - name: {user}
    groups: users,adm,dialout,audio,netdev,video,plugdev,input,gpio,spi,i2c,render,sudo
    shell: /bin/bash
{pw}    ssh_authorized_keys:
      - {key}
    sudo: ALL=(ALL) NOPASSWD:ALL
enable_ssh: true
ssh_pwauth: false
runcmd:
  - [systemctl, enable, --now, ssh]
  - [bash, /boot/firmware/carpi/firstboot.sh]
# Reboot only when firstboot.sh finished; on a failure the Pi stays up for ssh.
power_state:
  mode: reboot
  message: carpi first boot done
  condition: test -e /var/lib/carpi-firstboot.done
",
        host = a.hostname,
        user = a.user,
        key = q(key.trim()),
    ))
}

fn meta_data() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // A new instance id: cloud-init runs first-boot setup again for it.
    format!("dsmode: local\ninstance_id: carpi-{now}\n")
}

fn network_config(a: &Image, root: &Path) -> anyhow::Result<String> {
    let text = if let Some(p) = &a.network_config {
        std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?
    } else if a.no_wifi {
        "# Wired only (cargo xtask image --no-wifi).\nnetwork:\n  version: 2\n  ethernets:\n    eth0:\n      dhcp4: true\n      optional: true\n".into()
    } else {
        let (ssid, password) = ask_wifi()?;
        wifi_config(&ssid, &password, &a.country)
    };
    if !a.edit {
        return Ok(text);
    }
    let dir = root.join("target/image");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("network-config");
    std::fs::write(&path, &text)?;
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| {
            if cfg!(windows) {
                "notepad".into()
            } else {
                "nano".into()
            }
        });
    println!(
        "editing {} in {editor}; save and close it to go on",
        path.display()
    );
    crate::run(Command::new(&editor).arg(&path))?;
    let edited = std::fs::read_to_string(&path)?;
    // It holds the Wi-Fi password: don't leave it lying around.
    let _ = std::fs::remove_file(&path);
    if !edited.contains("network:") {
        bail!("network-config has no `network:`; nothing written");
    }
    Ok(edited)
}

fn wifi_config(ssid: &str, password: &str, country: &str) -> String {
    let ap = if password.is_empty() {
        format!("        {}: {{}}\n", q(ssid))
    } else {
        format!(
            "        {}:\n          password: {}\n",
            q(ssid),
            q(password)
        )
    };
    format!(
        "# Wi-Fi for first boot (it needs the internet for packages). Netplan v2.
network:
  version: 2
  wifis:
    renderer: NetworkManager
    wlan0:
      dhcp4: true
      optional: true
      regulatory-domain: {country}
      access-points:
{ap}",
        country = q(country),
    )
}

// ---- Asking ----

fn need_terminal(what: &str) -> anyhow::Result<()> {
    if !std::io::stdin().is_terminal() {
        bail!("{what} needs a terminal to ask in; pass --network-config FILE or --no-wifi");
    }
    Ok(())
}

fn ask_wifi() -> anyhow::Result<(String, String)> {
    need_terminal("Wi-Fi")?;
    print!("Wi-Fi network name (SSID) for first boot: ");
    std::io::stdout().flush()?;
    let mut ssid = String::new();
    std::io::stdin().lock().read_line(&mut ssid)?;
    let ssid = ssid.trim_end_matches(['\r', '\n']).to_string();
    if ssid.is_empty() {
        bail!("no SSID; pass --no-wifi for wired only");
    }
    let pw = rpassword::prompt_password("Wi-Fi password (empty for an open network): ")?;
    Ok((ssid, pw))
}

fn ask_password() -> anyhow::Result<String> {
    need_terminal("--password")?;
    let a = rpassword::prompt_password("Login password for the Pi: ")?;
    let b = rpassword::prompt_password("Again: ")?;
    if a != b {
        bail!("the passwords differ");
    }
    if a.is_empty() {
        bail!("empty password; leave out --password for key-only ssh");
    }
    let params = sha_crypt::Sha512Params::new(sha_crypt::ROUNDS_DEFAULT)
        .map_err(|e| anyhow::anyhow!("sha512 params: {e:?}"))?;
    sha_crypt::sha512_simple(&a, &params).map_err(|e| anyhow::anyhow!("hashing: {e:?}"))
}

/// The public key: `--ssh-key`, else the `carpi` alias's IdentityFile + .pub,
/// else ~/.ssh/id_ed25519.pub.
fn ssh_key(given: Option<&Path>) -> anyhow::Result<String> {
    let mut tried = Vec::new();
    let mut candidates: Vec<PathBuf> = given.map(Path::to_path_buf).into_iter().collect();
    if given.is_none() {
        let cfg = Command::new("ssh")
            .args(["-G", "carpi"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output();
        if let Ok(o) = cfg {
            for l in String::from_utf8_lossy(&o.stdout).lines() {
                if let Some(p) = l.strip_prefix("identityfile ") {
                    candidates.push(PathBuf::from(format!("{}.pub", expand(p))));
                }
            }
        }
        if let Some(h) = home() {
            candidates.push(h.join(".ssh/id_ed25519.pub"));
        }
    }
    for c in candidates {
        if let Ok(k) = std::fs::read_to_string(&c)
            && k.starts_with("ssh-")
        {
            println!("ssh key: {}", c.display());
            return Ok(k);
        }
        tried.push(c.display().to_string());
    }
    bail!(
        "no ssh public key (tried {}); pass --ssh-key",
        tried.join(", ")
    )
}

fn home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

fn expand(p: &str) -> String {
    match (p.strip_prefix("~/"), home()) {
        (Some(rest), Some(h)) => h.join(rest).to_string_lossy().into_owned(),
        _ => p.to_string(),
    }
}

// ---- The image ----

/// The input image, uncompressed, at `out`.
fn unpack(input: &Path, out: &Path) -> anyhow::Result<()> {
    let src = std::fs::File::open(input).with_context(|| format!("opening {}", input.display()))?;
    let mut r: Box<dyn Read> = if input.extension().is_some_and(|e| e == "xz") {
        println!("unpacking {}", input.display());
        Box::new(xz2::read::XzDecoder::new_multi_decoder(
            std::io::BufReader::new(src),
        ))
    } else {
        println!("copying {}", input.display());
        Box::new(src)
    };
    let mut w = std::io::BufWriter::new(std::fs::File::create(out)?);
    let mut buf = vec![0u8; 1 << 20];
    let (mut n, mut shown) = (0u64, 0u64);
    loop {
        let k = r.read(&mut buf)?;
        if k == 0 {
            break;
        }
        w.write_all(&buf[..k])?;
        n += k as u64;
        if n - shown >= 512 << 20 {
            shown = n;
            println!("  {} MB", n >> 20);
        }
    }
    w.flush()?;
    Ok(())
}

/// The first partition from the MBR: start and length in bytes. Pi OS images
/// put the FAT boot partition first.
fn boot_partition(mbr: &[u8; 512]) -> anyhow::Result<(u64, u64)> {
    if mbr[510..512] != [0x55, 0xAA] {
        bail!("no MBR partition table");
    }
    let e = &mbr[446..462];
    if !matches!(e[4], 0x0B | 0x0C | 0x0E | 0x06) {
        bail!("partition 1 isn't FAT (type {:#04x})", e[4]);
    }
    let lba = u32::from_le_bytes(e[8..12].try_into()?) as u64;
    let n = u32::from_le_bytes(e[12..16].try_into()?) as u64;
    Ok((lba * 512, n * 512))
}

fn patch(mut f: std::fs::File, files: &Files) -> anyhow::Result<()> {
    let mut mbr = [0u8; 512];
    f.seek(SeekFrom::Start(0))?;
    f.read_exact(&mut mbr)?;
    let (start, len) = boot_partition(&mbr)?;
    let part = fscommon::StreamSlice::new(f, start, start + len)?;
    let fs = fatfs::FileSystem::new(part, fatfs::FsOptions::new()).context("boot partition")?;
    let root = fs.root_dir();
    if root.open_file("config.txt").is_err() {
        bail!("partition 1 has no config.txt; is this a Raspberry Pi OS image?");
    }
    let stats = fs.stats()?;
    let free = stats.free_clusters() as u64 * stats.cluster_size() as u64;
    let need: u64 = files.iter().map(|(_, d)| d.len() as u64).sum();
    if need + (1 << 20) > free {
        bail!(
            "boot partition has {} MB free, needs {} MB",
            free >> 20,
            need >> 20
        );
    }
    for (p, d) in files {
        if let Some((dir, _)) = p.rsplit_once('/')
            && root.open_dir(dir).is_err()
        {
            root.create_dir(dir)?;
        }
        let mut file = root.create_file(p)?;
        file.truncate()?;
        file.write_all(d)?;
        file.flush()?;
    }
    drop(root);
    fs.unmount()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wifi_config_quotes_what_it_must() {
        let c = wifi_config("Stadium \"Walk\"", "p: w#d", "US");
        assert!(c.contains("        \"Stadium \\\"Walk\\\"\":\n          password: \"p: w#d\"\n"));
        assert!(wifi_config("open", "", "DE").contains("\"open\": {}"));
    }

    #[test]
    fn finds_the_fat_partition() {
        let mut mbr = [0u8; 512];
        mbr[510] = 0x55;
        mbr[511] = 0xAA;
        mbr[446 + 4] = 0x0C;
        mbr[446 + 8..446 + 12].copy_from_slice(&16384u32.to_le_bytes());
        mbr[446 + 12..446 + 16].copy_from_slice(&1048576u32.to_le_bytes());
        assert_eq!(boot_partition(&mbr).unwrap(), (16384 * 512, 1048576 * 512));
        mbr[446 + 4] = 0x83;
        assert!(boot_partition(&mbr).is_err());
    }

    /// A 64 MB disk: MBR, one FAT32 partition at 4 MiB with a config.txt.
    fn fake_disk(path: &Path) {
        let start = 4u64 << 20;
        let len = 60u64 << 20;
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .unwrap();
        f.set_len(start + len).unwrap();
        let mut mbr = [0u8; 512];
        mbr[510] = 0x55;
        mbr[511] = 0xAA;
        mbr[446 + 4] = 0x0C;
        mbr[446 + 8..446 + 12].copy_from_slice(&((start / 512) as u32).to_le_bytes());
        mbr[446 + 12..446 + 16].copy_from_slice(&((len / 512) as u32).to_le_bytes());
        let mut f2 = f.try_clone().unwrap();
        f2.write_all(&mbr).unwrap();
        let mut part = fscommon::StreamSlice::new(f, start, start + len).unwrap();
        fatfs::format_volume(
            &mut part,
            fatfs::FormatVolumeOptions::new().fat_type(fatfs::FatType::Fat32),
        )
        .unwrap();
        let fs = fatfs::FileSystem::new(part, fatfs::FsOptions::new()).unwrap();
        fs.root_dir()
            .create_file("config.txt")
            .unwrap()
            .write_all(
                b"[all]
",
            )
            .unwrap();
        fs.unmount().unwrap();
    }

    #[test]
    fn patches_the_boot_partition_and_reads_back() {
        let path =
            std::env::temp_dir().join(format!("carpi-image-test-{}.img", std::process::id()));
        fake_disk(&path);
        let big = vec![7u8; 5 << 20];
        let files: Files = vec![
            (
                "user-data".into(),
                b"#cloud-config
"
                .to_vec(),
            ),
            (
                "carpi/setup.env".into(),
                b"CARPI_USER=pi
"
                .to_vec(),
            ),
            ("carpi/bundle.tar.zst".into(), big.clone()),
        ];
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        patch(f, &files).unwrap();
        // Again: overwrites, doesn't fail on the existing folder.
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        patch(f, &files).unwrap();

        let f = std::fs::File::open(&path).unwrap();
        let part = fscommon::StreamSlice::new(f, 4 << 20, 64 << 20).unwrap();
        let fs = fatfs::FileSystem::new(part, fatfs::FsOptions::new()).unwrap();
        let read = |p: &str| {
            let mut d = Vec::new();
            fs.root_dir()
                .open_file(p)
                .unwrap()
                .read_to_end(&mut d)
                .unwrap();
            d
        };
        assert_eq!(
            read("user-data"),
            b"#cloud-config
"
        );
        assert_eq!(
            read("carpi/setup.env"),
            b"CARPI_USER=pi
"
        );
        assert_eq!(read("carpi/bundle.tar.zst"), big);
        assert_eq!(
            read("config.txt"),
            b"[all]
"
        );
        drop(fs);
        let _ = std::fs::remove_file(&path);
    }
}
