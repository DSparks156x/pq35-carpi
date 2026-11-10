//! SDLink firmware: what the RNS-E (sector 107 module) and the Teensy run,
//! what this release bundles, and installing it. The procedure is sdlink's
//! docs/PI_UPDATER.md, the layout docs/FIRMWARE_CONTRACT.md.
//!
//! Where the installed identity comes from:
//! - RNS-E: the module's announcement (avlink's `rnse.txt`, while the chain
//!   runs), or a probe: a B1 read of the sector 107 header and the firmware
//!   table. Whichever is newer.
//! - Teensy: `TEENSY_VERSION` in `rnse.txt`, or a probe asking it `ver`
//!   when the chain isn't using the port.
//!
//! Comparison: same build = same; else the stream format orders them; else
//! the bundle's build history (module.json `history`, oldest first). A table
//! version the image doesn't match is `incompatible`. The Teensy's version
//! string `sdemu X.Y (YYYY-MM-DD ...)` orders by date then version.
//!
//! RNS-E install: check the image, probe the table and gate on it, skip if
//! the live sector already matches, stop the chain, take the bus lease,
//! auto start off and wait for task 159 to be dormant, stage over CAN (B3,
//! the unit CRCs it), B1 spot checks, B7 + B6 commit (no ack; erase,
//! program, cold reset), then verify: restart count 0, live CRC equal,
//! table status `OK2F`, auto start re-armed, chain restarted.

use crate::rnse_bus::{RnseBus, crc32};
use carpi_core::rnse_service as svc;
use carpi_proto::{
    FirmwareComparison as Cmp, FirmwareIdentity, FirmwareInstalled, FirmwareProtocol,
    FirmwareStatus, FirmwareTarget, JobEvent,
};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::mpsc;

// ---------------------------------------------------------------- unit layout (FIRMWARE_CONTRACT.md)
const DEST: u32 = 0x00D6_0000;
const MAGIC_TVA2: u32 = 0x5456_4132;
/// TVA RAM block: word 1 auto start off, 25 table address, 26 table status.
const TVA: u32 = 0x8F72_F000;
const TVA_OFF: u32 = 0x4F46_4621; // 'OFF!'
const OK2F: u32 = 0x4F4B_3246; // 'OK2F'
/// Where current firmware builds keep the table: r64 line, PCBBC NA, PCBBC EU.
const TABLE_CANDIDATES: [u32; 3] = [0x806A_A900, 0x806A_CC00, 0x806B_AC00];
const SDFW: u32 = 0x5344_4657;
const RAM_MAP_1: u32 = 1;
/// fwtab.h entry numbers (table word = 5 + entry): TCB_BASE and TASK_TV.
const FWE_TCB_BASE: u32 = 20;
const FWE_TASK_TV: u32 = 61;
const TCB_STRIDE: u32 = 0x40;
const DORMANT: u16 = 0x0010;
/// Boot ROM restart count (signed 16-bit).
const RESTART_COUNT: u32 = 0xB800_001A;
/// B3 staging RAM, P2.
const STAGE_BASE: u32 = 0xACE0_0000;
const HEADER: usize = 0xA0;

const B6_STATUS: [&str; 6] = [
    "committing",
    "CRC mismatch",
    "range",
    "not armed",
    "?",
    "forbidden",
];

// ---------------------------------------------------------------- module image
#[derive(Clone, Debug, PartialEq)]
pub struct Module {
    pub build_id: u32,
    pub table_version: u32,
    pub fmt: u8,
    pub fmt_min: u8,
    pub declared: u32,
    pub sum: u32,
}

fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(b[at..at + 4].try_into().unwrap())
}

impl Module {
    /// The header (first 0xA0 bytes of the sector); `None` when there is no
    /// SDLink v2 module there.
    pub fn parse(h: &[u8]) -> Option<Module> {
        if h.len() < HEADER || be32(h, 0x80) != MAGIC_TVA2 || be32(h, 0x70) != DEST {
            return None;
        }
        let f = be32(h, 0x9C);
        Some(Module {
            build_id: be32(h, 0x94),
            table_version: be32(h, 0x98),
            fmt: f as u8,
            fmt_min: (f >> 8) as u8,
            declared: be32(h, 0x74),
            sum: be32(h, 0x78),
        })
    }

    pub fn build(&self) -> String {
        format!("{:08X}", self.build_id)
    }

    /// `"stream format 1"`, `"stream formats 1-2"`, `"no stream format"`
    /// for a module from before the handshake.
    pub fn formats(&self) -> String {
        formats(self.fmt_min, self.fmt)
    }
}

fn formats(min: u8, max: u8) -> String {
    match (min, max) {
        (_, 0) => "stream format unannounced".into(),
        (a, b) if a == b || a == 0 => format!("stream format {b}"),
        (a, b) => format!("stream formats {a}-{b}"),
    }
}

/// The whole-sector image is what the hook will accept: header, size, word
/// sum, zero tail.
pub fn check_image(img: &[u8]) -> Result<Module, String> {
    if img.len() != svc::SECTOR_SIZE as usize {
        return Err(format!(
            "image is {} bytes, not one 128 KiB sector",
            img.len()
        ));
    }
    let m = Module::parse(img).ok_or("image has no SDLink module header for sector 107")?;
    let n = m.declared as usize;
    if n <= 0x80 || n > img.len() || !n.is_multiple_of(4) {
        return Err(format!("image declares {n} bytes"));
    }
    let sum = img[0x80..n].chunks(4).fold(0u32, |a, w| {
        a.wrapping_add(u32::from_be_bytes(w.try_into().unwrap()))
    });
    if sum != m.sum {
        return Err(format!(
            "image word sum {sum:08X}, header says {:08X}",
            m.sum
        ));
    }
    if img[n..].iter().any(|&b| b != 0) {
        return Err("image tail is not zero".into());
    }
    Ok(m)
}

fn sha256_hex(b: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(b))
}

// ---------------------------------------------------------------- the bundle
#[derive(Clone, Debug, Default)]
pub struct Bundle {
    pub rnse: Option<RnseBundle>,
    pub teensy: Option<TeensyBundle>,
    /// Stream formats this release's encoder and avlink send.
    pub chain_fmt: Option<(u8, u8)>,
}

#[derive(Clone, Debug)]
pub struct RnseBundle {
    pub path: PathBuf,
    pub build_id: String,
    pub fmt: u8,
    pub fmt_min: u8,
    pub table_version: u32,
    pub sha256: String,
    /// Build ids, oldest first, ending with this one.
    pub history: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct TeensyBundle {
    pub path: PathBuf,
    pub version: String,
}

#[derive(Deserialize)]
struct ManifestFw {
    #[serde(default)]
    firmware: Vec<FwEntry>,
}

#[derive(Deserialize)]
struct FwEntry {
    target: String,
    path: String,
    version: String,
    #[serde(default)]
    chain_fmt: Option<[u8; 2]>,
}

#[derive(Deserialize)]
struct ModuleJson {
    build_id: String,
    sha256: String,
    table_version: u32,
    stream_fmt: u8,
    stream_fmt_min: u8,
    #[serde(default)]
    history: Vec<String>,
}

/// The firmware this release carries: `manifest.json` `firmware[]` under
/// `root` (`/opt/carpi/current`), RNS-E details from the `module.json`
/// beside its image.
pub fn load_bundle(root: &Path) -> Bundle {
    let mut b = Bundle::default();
    let Some(m) = std::fs::read(root.join("manifest.json"))
        .ok()
        .and_then(|t| serde_json::from_slice::<ManifestFw>(&t).ok())
    else {
        return b;
    };
    for fw in m.firmware {
        let path = root.join(&fw.path);
        match fw.target.as_str() {
            "teensy" => {
                b.teensy = Some(TeensyBundle {
                    path,
                    version: fw.version,
                })
            }
            "rnse" => {
                let json = path.with_file_name("module.json");
                let Some(mj) = std::fs::read(&json)
                    .ok()
                    .and_then(|t| serde_json::from_slice::<ModuleJson>(&t).ok())
                else {
                    tracing::warn!("firmware: {} missing or unreadable", json.display());
                    continue;
                };
                b.chain_fmt = fw.chain_fmt.map(|[a, z]| (a, z));
                b.rnse = Some(RnseBundle {
                    path,
                    build_id: mj.build_id.to_ascii_uppercase(),
                    fmt: mj.stream_fmt,
                    fmt_min: mj.stream_fmt_min,
                    table_version: mj.table_version,
                    sha256: mj.sha256.to_ascii_lowercase(),
                    history: mj.history.iter().map(|h| h.to_ascii_uppercase()).collect(),
                });
            }
            other => tracing::info!("firmware: unknown target {other:?} in the manifest"),
        }
    }
    b
}

// ---------------------------------------------------------------- avlink's rnse.txt
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Live {
    pub announced: bool,
    pub build: Option<String>,
    pub fmt: u8,
    pub fmt_min: u8,
    pub fw_id: Option<String>,
    pub table: Option<u32>,
    /// The format avlink sends; 0 = none in common.
    pub stream_fmt: i32,
    pub teensy: Option<String>,
}

/// `NAME=value` lines as avlink writes them (`rnse_ver_write`).
pub fn parse_rnse_txt(text: &str) -> Live {
    let mut l = Live::default();
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = v.trim();
        let v = v
            .strip_prefix('\'')
            .and_then(|v| v.strip_suffix('\''))
            .unwrap_or(v);
        match k.trim() {
            "RNSE_ANNOUNCED" => l.announced = v == "1",
            "RNSE_BUILD" => l.build = Some(v.to_ascii_uppercase()),
            "RNSE_FMT" => l.fmt = v.parse().unwrap_or(0),
            "RNSE_FMT_MIN" => l.fmt_min = v.parse().unwrap_or(0),
            "RNSE_FW_ID" => l.fw_id = Some(v.into()),
            "RNSE_TABLE" => l.table = v.parse().ok(),
            "STREAM_FMT" => l.stream_fmt = v.parse().unwrap_or(0),
            "TEENSY_VERSION" if !v.is_empty() => l.teensy = Some(v.into()),
            _ => {}
        }
    }
    l
}

// ---------------------------------------------------------------- comparisons
/// The bundled module against an installed one. `installed`: build id and
/// newest stream format, `None` = no module in sector 107. `table`: the
/// unit's table version and ram map, when known.
pub fn compare_rnse(
    installed: Option<(&str, u8)>,
    b: &RnseBundle,
    table: Option<(u32, u32)>,
) -> Cmp {
    if let Some((ver, map)) = table
        && (ver != b.table_version || map != RAM_MAP_1)
    {
        return Cmp::Incompatible;
    }
    let Some((build, fmt)) = installed else {
        // Nothing there yet: the bundle is the newer one, on a known table.
        return if table.is_some() {
            Cmp::Newer
        } else {
            Cmp::Unknown
        };
    };
    if build.eq_ignore_ascii_case(&b.build_id) {
        return Cmp::Same;
    }
    if fmt != b.fmt {
        return if fmt < b.fmt { Cmp::Newer } else { Cmp::Older };
    }
    let at = |id: &str| b.history.iter().position(|h| h.eq_ignore_ascii_case(id));
    match (at(build), at(&b.build_id)) {
        (Some(i), Some(j)) if i < j => Cmp::Newer,
        (Some(i), Some(j)) if i > j => Cmp::Older,
        _ => Cmp::Unknown,
    }
}

/// `sdemu 0.5 (2026-11-10, ...)` -> (date, major, minor).
fn teensy_key(v: &str) -> Option<(String, u32, u32)> {
    let rest = v.strip_prefix("sdemu ")?;
    let (ver, rest) = rest.split_once(' ')?;
    let (maj, min) = ver.split_once('.')?;
    let date: String = rest.strip_prefix('(')?.chars().take(10).collect();
    let ok = date.len() == 10
        && date.chars().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                c == '-'
            } else {
                c.is_ascii_digit()
            }
        });
    ok.then(|| (date, maj.parse().ok(), min.parse().ok()))
        .and_then(|(d, a, b)| Some((d, a?, b?)))
}

pub fn compare_teensy(installed: &str, bundled: &str) -> Cmp {
    if installed.trim() == bundled.trim() {
        return Cmp::Same;
    }
    match (teensy_key(installed), teensy_key(bundled)) {
        (Some(i), Some(b)) if i < b => Cmp::Newer,
        (Some(i), Some(b)) if i > b => Cmp::Older,
        _ => Cmp::Unknown,
    }
}

// ---------------------------------------------------------------- what carpid has seen
#[derive(Clone, Debug)]
pub struct Table {
    pub addr: u32,
    pub version: u32,
    pub fw_id: u32,
    pub ram_map: u32,
    pub tcb_base: u32,
    pub task_tv: u32,
}

#[derive(Clone, Debug)]
pub struct RnseSeen {
    /// `None`: the sector has no SDLink v2 module.
    pub module: Option<Module>,
    pub table: Option<Table>,
    /// The module validated the table and runs (`OK2F`).
    pub running: bool,
    pub at: SystemTime,
}

#[derive(Default)]
pub struct Seen {
    pub rnse: Option<RnseSeen>,
    pub teensy: Option<(String, SystemTime)>,
}

/// The chain as it is now.
#[derive(Clone, Debug, Default)]
pub struct Chain {
    pub active: bool,
    pub live: Option<(Live, SystemTime)>,
    pub teensy_on_usb: bool,
}

fn chain_now(unit: &str) -> Chain {
    let show = std::process::Command::new("systemctl")
        .args(["--user", "show", "-p", "LoadState,ActiveState", unit])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let active = crate::avsd_status::Unit::parse(&show) == crate::avsd_status::Unit::Active;
    let live = crate::avsd_status::chain_dir().and_then(|d| {
        let p = d.join("rnse.txt");
        let t = std::fs::metadata(&p).and_then(|m| m.modified()).ok()?;
        Some((parse_rnse_txt(&std::fs::read_to_string(&p).ok()?), t))
    });
    Chain {
        active,
        live: live.filter(|_| active),
        teensy_on_usb: crate::teensy::port().is_some(),
    }
}

/// Everything the status needs that isn't on disk.
pub struct Inputs<'a> {
    pub seen: &'a Seen,
    pub bundle: &'a Bundle,
    pub chain: &'a Chain,
    pub busy: bool,
    pub can: bool,
}

fn identity(version: String, build: Option<String>) -> FirmwareIdentity {
    FirmwareIdentity {
        version,
        build_id: build,
    }
}

/// The status LIVI shows. Pure: tests drive it directly.
pub fn status(i: &Inputs) -> FirmwareStatus {
    // RNS-E: the newer of the announcement and the probe.
    let live = i.chain.live.as_ref().filter(|(l, _)| l.announced);
    let probe = i.seen.rnse.as_ref();
    let use_live = match (live, probe) {
        (Some((_, lt)), Some(p)) => *lt >= p.at,
        (Some(_), None) => true,
        _ => false,
    };
    let table = probe.and_then(|p| p.table.as_ref());
    let table_key = if use_live {
        live.and_then(|(l, _)| l.table).map(|v| (v, RAM_MAP_1))
    } else {
        table.map(|t| (t.version, t.ram_map))
    };
    let (installed, inst_key, mut detail) = if use_live {
        let (l, _) = live.unwrap();
        let build = l.build.clone().unwrap_or_default();
        (
            FirmwareInstalled::Reported(identity(formats(l.fmt_min, l.fmt), Some(build.clone()))),
            Some((build, l.fmt)),
            Some(format!(
                "Announced by the running module; firmware table {}/{}",
                l.fw_id.as_deref().unwrap_or("?"),
                l.table.map_or("?".into(), |t| t.to_string())
            )),
        )
    } else if let Some(p) = probe {
        let tdesc = match &p.table {
            Some(t) => format!(
                "firmware table {:X}/{} at {:08X}",
                t.fw_id, t.version, t.addr
            ),
            None => "no SDLink firmware table found".into(),
        };
        let run = if p.running { "running" } else { "not running" };
        match &p.module {
            Some(m) => (
                FirmwareInstalled::Reported(identity(m.formats(), Some(m.build()))),
                Some((m.build(), m.fmt)),
                Some(format!("Read from sector 107 ({run}); {tdesc}")),
            ),
            None => (
                FirmwareInstalled::Legacy,
                None,
                Some(format!("Sector 107 holds no SDLink module; {tdesc}")),
            ),
        }
    } else {
        (FirmwareInstalled::Unknown, None, None)
    };
    let known = !matches!(installed, FirmwareInstalled::Unknown);
    let rnse = match &i.bundle.rnse {
        None => FirmwareTarget {
            installed,
            detail,
            blocked: Some("This release carries no RNS-E image".into()),
            ..Default::default()
        },
        Some(b) => {
            let comparison = if known {
                compare_rnse(
                    inst_key.as_ref().map(|(s, f)| (s.as_str(), *f)),
                    b,
                    table_key,
                )
            } else {
                Cmp::Unknown
            };
            if let (Some((lo, hi)), true) = (i.bundle.chain_fmt, b.fmt != 0)
                && (b.fmt < lo || b.fmt_min > hi)
            {
                detail = Some(format!(
                    "{}. The bundled module plays {} but this release sends {lo}-{hi}",
                    detail.unwrap_or_default(),
                    formats(b.fmt_min, b.fmt)
                ));
            }
            let blocked = if i.busy {
                Some("A firmware job is running".into())
            } else if !i.can {
                Some("No live ICAN interface".into())
            } else if comparison == Cmp::Same {
                Some("Already installed".into())
            } else if comparison == Cmp::Incompatible {
                Some(format!(
                    "This unit's firmware table doesn't match the bundled module (needs table version {})",
                    b.table_version
                ))
            } else {
                None
            };
            FirmwareTarget {
                installed,
                bundled: Some(identity(
                    formats(b.fmt_min, b.fmt),
                    Some(b.build_id.clone()),
                )),
                comparison,
                detail,
                blocked,
            }
        }
    };

    // Teensy: the newer of rnse.txt and the probe.
    let from_live = i
        .chain
        .live
        .as_ref()
        .and_then(|(l, t)| l.teensy.clone().map(|v| (v, *t)));
    let tv = match (from_live, i.seen.teensy.clone()) {
        (Some(a), Some(b)) => Some(if a.1 >= b.1 { a } else { b }),
        (a, b) => a.or(b),
    };
    let t_installed = match &tv {
        Some((v, _)) => FirmwareInstalled::Reported(identity(v.clone(), None)),
        None => FirmwareInstalled::Unknown,
    };
    let teensy = match &i.bundle.teensy {
        None => FirmwareTarget {
            installed: t_installed,
            blocked: Some("This release carries no Teensy image".into()),
            ..Default::default()
        },
        Some(b) => {
            let comparison = tv
                .as_ref()
                .map_or(Cmp::Unknown, |(v, _)| compare_teensy(v, &b.version));
            let blocked = if i.busy {
                Some("A firmware job is running".into())
            } else if !i.chain.teensy_on_usb {
                Some("The Teensy isn't on USB".into())
            } else if comparison == Cmp::Same {
                Some("Already installed".into())
            } else {
                None
            };
            FirmwareTarget {
                installed: t_installed,
                bundled: Some(identity(b.version.clone(), None)),
                comparison,
                detail: (!i.chain.teensy_on_usb).then(|| "Not on USB".into()),
                blocked,
            }
        }
    };

    let protocol = match i.chain.live.as_ref() {
        Some((l, _)) if l.announced => FirmwareProtocol::Negotiated(if l.stream_fmt > 0 {
            l.stream_fmt.to_string()
        } else {
            "none".into()
        }),
        Some(_) => FirmwareProtocol::Legacy,
        None => FirmwareProtocol::Unknown,
    };
    FirmwareStatus {
        rnse,
        teensy,
        protocol,
        busy: i.busy,
    }
}

// ---------------------------------------------------------------- the service
pub struct Firmware {
    pub root: PathBuf,
    pub unit: String,
    pub interface: Option<String>,
    seen: Mutex<Seen>,
    busy: std::sync::atomic::AtomicBool,
}

type Events = mpsc::UnboundedSender<JobEvent>;

fn stage(ev: &Events, stage: &str, msg: impl Into<String>) {
    let _ = ev.send(JobEvent::Stage {
        stage: stage.into(),
        message: msg.into(),
    });
}

fn progress(ev: &Events, stage: &str, done: u64, total: u64) {
    let _ = ev.send(JobEvent::Progress {
        stage: stage.into(),
        done,
        total: Some(total),
    });
}

impl Firmware {
    pub fn new(root: PathBuf, unit: String, interface: Option<String>) -> Self {
        Firmware {
            root,
            unit,
            interface,
            seen: Mutex::new(Seen::default()),
            busy: Default::default(),
        }
    }

    pub fn busy(&self) -> bool {
        self.busy.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Takes the one firmware job slot.
    pub fn claim(&self) -> Result<(), String> {
        if self.busy.swap(true, std::sync::atomic::Ordering::SeqCst) {
            Err("a firmware job is running".into())
        } else {
            Ok(())
        }
    }

    pub fn done(&self) {
        self.busy.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// Status now. Blocking (systemctl, files): call it off the loop.
    pub fn status(&self) -> FirmwareStatus {
        let bundle = load_bundle(&self.root);
        let chain = chain_now(&self.unit);
        let seen = self.seen.lock().unwrap();
        status(&Inputs {
            seen: &seen,
            bundle: &bundle,
            chain: &chain,
            busy: self.busy(),
            can: self.interface.is_some(),
        })
    }

    async fn status_async(self: &std::sync::Arc<Self>) -> FirmwareStatus {
        let me = self.clone();
        tokio::task::spawn_blocking(move || me.status())
            .await
            .unwrap_or_default()
    }

    async fn emit_status(self: &std::sync::Arc<Self>, ev: &Events) {
        let s = self.status_async().await;
        let _ = ev.send(JobEvent::Firmware(Box::new(s)));
    }

    // ------------------------------------------------------------ probe
    /// The RNS-E module header, table and run state. Read only.
    async fn probe_rnse(&self, bus: &mut RnseBus) -> Result<RnseSeen, String> {
        let h = bus.read_all(svc::SECTOR_XIP, HEADER).await?;
        let module = Module::parse(&h);
        let running = bus.u32(TVA + 26 * 4).await.unwrap_or(0) == OK2F;
        let mut addrs = Vec::new();
        if running {
            addrs.push(bus.u32(TVA + 25 * 4).await?);
        }
        addrs.extend(TABLE_CANDIDATES);
        let mut table = None;
        for a in addrs {
            let w = bus.read(a, 20).await?;
            if be32(&w, 0) != SDFW {
                continue;
            }
            let word = |e: u32| a + 4 * (5 + e);
            table = Some(Table {
                addr: a,
                version: be32(&w, 4),
                fw_id: be32(&w, 12),
                ram_map: be32(&w, 16),
                tcb_base: bus.u32(word(FWE_TCB_BASE)).await?,
                task_tv: bus.u32(word(FWE_TASK_TV)).await?,
            });
            break;
        }
        Ok(RnseSeen {
            module,
            table,
            running,
            at: SystemTime::now(),
        })
    }

    async fn probe_teensy(&self, chain_active: bool) -> Result<String, String> {
        if chain_active {
            return Err("the chain has the Teensy's port; its version comes from avlink".into());
        }
        let port = crate::teensy::port().ok_or("the Teensy isn't on USB")?;
        tokio::task::spawn_blocking(move || crate::teensy::query_version(&port))
            .await
            .map_err(|e| e.to_string())?
    }

    pub async fn probe(self: &std::sync::Arc<Self>, tx: mpsc::Sender<crate::Msg>, ev: &Events) {
        let unit = self.unit.clone();
        let chain = tokio::task::spawn_blocking(move || chain_now(&unit))
            .await
            .unwrap_or_default();
        let mut notes = Vec::new();
        stage(
            ev,
            "probe",
            "Reading the RNS-E's SDLink module and firmware table",
        );
        let rnse = match &self.interface {
            None => Err("no live ICAN interface".to_string()),
            Some(i) => match RnseBus::open(i, tx) {
                Ok(mut bus) => self.probe_rnse(&mut bus).await,
                Err(e) => Err(e),
            },
        };
        match rnse {
            Ok(r) => {
                notes.push(match &r.module {
                    Some(m) => format!("RNS-E module {} ({})", m.build(), m.formats()),
                    None => "RNS-E: no SDLink module in sector 107".into(),
                });
                self.seen.lock().unwrap().rnse = Some(r);
            }
            Err(e) => notes.push(format!("RNS-E: {e}")),
        }
        stage(ev, "probe", "Asking the Teensy its version");
        match self.probe_teensy(chain.active).await {
            Ok(v) => {
                notes.push(format!("Teensy {v}"));
                self.seen.lock().unwrap().teensy = Some((v, SystemTime::now()));
            }
            Err(e) if chain.active => {
                if chain
                    .live
                    .as_ref()
                    .and_then(|(l, _)| l.teensy.as_ref())
                    .is_none()
                {
                    notes.push(format!("Teensy: {e}"));
                }
            }
            Err(e) => notes.push(format!("Teensy: {e}")),
        }
        self.emit_status(ev).await;
        let _ = ev.send(JobEvent::Finished {
            ok: true,
            message: notes.join("; "),
        });
    }

    // ------------------------------------------------------------ chain
    async fn systemctl(&self, verb: &str) -> Result<(), String> {
        let out = tokio::process::Command::new("systemctl")
            .args(["--user", verb, &self.unit])
            .stdin(std::process::Stdio::null())
            .output()
            .await
            .map_err(|e| format!("systemctl: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!(
                "systemctl --user {verb} {}: {}",
                self.unit,
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }

    /// Stops the chain; true when it was running (and so must be restarted).
    async fn stop_chain(&self, ev: &Events) -> Result<bool, String> {
        let unit = self.unit.clone();
        let active = tokio::task::spawn_blocking(move || chain_now(&unit).active)
            .await
            .unwrap_or(false);
        if active {
            stage(ev, "chain", "Stopping the A/V over SD chain");
            self.systemctl("stop").await?;
        }
        Ok(active)
    }

    async fn start_chain(&self, ev: &Events) {
        stage(ev, "chain", "Starting the A/V over SD chain");
        if let Err(e) = self.systemctl("start").await {
            stage(ev, "chain", e);
        }
    }

    // ------------------------------------------------------------ RNS-E install
    pub async fn install_rnse(
        self: &std::sync::Arc<Self>,
        tx: mpsc::Sender<crate::Msg>,
        ev: &Events,
    ) {
        let r = tokio::time::timeout(
            Duration::from_secs(25 * 60),
            self.install_rnse_inner(tx, ev),
        )
        .await
        .unwrap_or_else(|_| Err("stopped after 25 minutes".into()));
        self.emit_status(ev).await;
        let (ok, message) = match r {
            Ok(m) => (true, m),
            Err(m) => (false, m),
        };
        let _ = ev.send(JobEvent::Finished { ok, message });
    }

    async fn install_rnse_inner(
        &self,
        tx: mpsc::Sender<crate::Msg>,
        ev: &Events,
    ) -> Result<String, String> {
        // 1. The image.
        stage(ev, "check", "Checking the bundled sector 107 image");
        let root = self.root.clone();
        let bundle = tokio::task::spawn_blocking(move || load_bundle(&root))
            .await
            .map_err(|e| e.to_string())?;
        let b = bundle.rnse.ok_or("this release carries no RNS-E image")?;
        let img = tokio::fs::read(&b.path)
            .await
            .map_err(|e| format!("{}: {e}", b.path.display()))?;
        let module = check_image(&img)?;
        let sha = sha256_hex(&img);
        let sha_file = b.path.with_file_name("sector_107.bin.sha256");
        let listed = tokio::fs::read_to_string(&sha_file)
            .await
            .unwrap_or_default();
        if sha != b.sha256 || listed.split_whitespace().next() != Some(sha.as_str()) {
            return Err(format!(
                "image sha256 {sha} doesn't match module.json / {}",
                sha_file.display()
            ));
        }
        if module.build() != b.build_id {
            return Err(format!(
                "image build {} but module.json says {}",
                module.build(),
                b.build_id
            ));
        }
        if let Some((lo, hi)) = bundle.chain_fmt
            && (module.fmt < lo || module.fmt_min > hi)
        {
            return Err(format!(
                "the bundled module plays {} but this release's encoder sends {lo}-{hi}",
                module.formats()
            ));
        }
        let crc = !crc32(!0, &img);

        // 2. The unit: table gate, and is it already there?
        let iface = self.interface.clone().ok_or("no live ICAN interface")?;
        let mut bus = RnseBus::open(&iface, tx)?;
        stage(ev, "gate", "Reading the RNS-E's firmware table");
        let seen = self.probe_rnse(&mut bus).await?;
        self.seen.lock().unwrap().rnse = Some(seen.clone());
        let t = seen.table.clone().ok_or(
            "this RNS-E has no SDLink firmware table: its firmware needs the SDLink hook first",
        )?;
        if t.version != module.table_version || t.ram_map != RAM_MAP_1 {
            return Err(format!(
                "firmware table {:X} is version {} (ram map {}); the module needs version {}",
                t.fw_id, t.version, t.ram_map, module.table_version
            ));
        }
        let live_header = bus.read_all(svc::SECTOR_XIP, HEADER).await?;
        if live_header == img[..HEADER] {
            stage(
                ev,
                "verify",
                "The header matches; reading the whole sector to confirm",
            );
            if crate::decoder::sector_crc(&mut bus, "verify", ev).await? == crc {
                return Ok(format!("Module {} is already installed", module.build()));
            }
        }

        // 3. Write, with cleanup whatever happens.
        let restart = self.stop_chain(ev).await?;
        let r = self.write_rnse(&mut bus, &img, crc, &t, ev).await;
        bus.release().await;
        let r = match r {
            Ok(()) => self.verify_rnse(&mut bus, &img, crc, &module, ev).await,
            Err(Pre(e)) => {
                // Nothing was committed: re-arm auto start.
                if bus.lease(Duration::from_secs(10)).await.is_ok() {
                    let _ = bus.send(svc::poke(svc::AUTO_START_WORD, 0)).await;
                    bus.release().await;
                }
                Err(e)
            }
        };
        if restart {
            self.start_chain(ev).await;
        }
        r
    }

    /// Steps 4-6. `Err` here means nothing was committed.
    async fn write_rnse(
        &self,
        bus: &mut RnseBus,
        img: &[u8],
        crc: u32,
        t: &Table,
        ev: &Events,
    ) -> Result<(), Pre> {
        stage(ev, "prepare", "Taking the bus; auto start off");
        bus.lease(Duration::from_secs(15 * 60)).await.map_err(Pre)?;
        bus.send(svc::poke(svc::AUTO_START_WORD, TVA_OFF))
            .await
            .map_err(Pre)?;
        if bus.u32(svc::AUTO_START_WORD).await.map_err(Pre)? != TVA_OFF {
            return Err(Pre("the auto start word didn't take 'OFF!'".into()));
        }
        let state = t.tcb_base + t.task_tv * TCB_STRIDE + 4;
        let end = Instant::now() + Duration::from_secs(30);
        loop {
            let s = bus.u16(state).await.map_err(Pre)?;
            if s == DORMANT {
                break;
            }
            if Instant::now() > end {
                return Err(Pre(format!(
                    "task {} stayed busy (state {s:04X})",
                    t.task_tv
                )));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }

        // Stage: B3 frames, slower on a retry; the unit CRCs what it got.
        let n = img.len() as u32;
        let frames = img.len().div_ceil(5);
        let mut staged = false;
        for (attempt, per_tick) in [4usize, 2, 1].into_iter().enumerate() {
            stage(
                ev,
                "stage",
                format!(
                    "Sending the image to the RNS-E (try {}, {} frames)",
                    attempt + 1,
                    frames
                ),
            );
            let mut tick = tokio::time::interval(Duration::from_millis(10));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            for (seq, chunk) in img.chunks(5).enumerate() {
                if seq % per_tick == 0 {
                    tick.tick().await;
                }
                let mut c = [0u8; 5];
                c[..chunk.len()].copy_from_slice(chunk);
                bus.send(svc::stage(seq as u16, c)).await.map_err(Pre)?;
                if seq % 1024 == 0 || seq + 1 == frames {
                    progress(ev, "stage", (seq + 1) as u64, frames as u64);
                }
            }
            let mut end_reply = None;
            for _ in 0..6 {
                bus.drain().await;
                bus.send(svc::stage_end(n)).await.map_err(Pre)?;
                if let Some(r) = bus.reply(0x83, Duration::from_secs(1)).await.map_err(Pre)? {
                    end_reply = Some(r);
                    break;
                }
            }
            let r = end_reply.ok_or(Pre("no reply to B3 END".into()))?;
            let got = u32::from_be_bytes([r[1], r[2], r[3], r[4]]);
            if r[5] == 0 && got == crc {
                staged = true;
                break;
            }
            stage(
                ev,
                "stage",
                format!("staged CRC {got:08X} (status {}), want {crc:08X}", r[5]),
            );
        }
        if !staged {
            return Err(Pre(
                "the staged image never matched; nothing was written".into()
            ));
        }
        for off in [0usize, 0x7FC0, 0x10000, img.len() - 48] {
            let got = bus.read(STAGE_BASE + off as u32, 48).await.map_err(Pre)?;
            if got != img[off..off + 48] {
                return Err(Pre(format!(
                    "staged RAM differs at +{off:05X}; nothing was written"
                )));
            }
        }

        // Commit: no ack expected; a refusal comes back as 84 st.
        stage(
            ev,
            "commit",
            "Writing sector 107: the RNS-E erases, programs and restarts (about 25 s)",
        );
        bus.send(svc::set_target(crc)).await.map_err(Pre)?;
        tokio::time::sleep(Duration::from_millis(20)).await;
        bus.drain().await;
        bus.send(svc::commit()).await.map_err(Pre)?;
        if let Some(r) = bus.reply(0x84, Duration::from_secs(3)).await.map_err(Pre)?
            && r[1] != 0
        {
            let why = B6_STATUS.get(r[1] as usize).copied().unwrap_or("?");
            return Err(Pre(format!(
                "the RNS-E refused the commit ({why}); nothing was written"
            )));
        }
        Ok(())
    }

    /// Step 7.
    async fn verify_rnse(
        &self,
        bus: &mut RnseBus,
        img: &[u8],
        crc: u32,
        m: &Module,
        ev: &Events,
    ) -> Result<String, String> {
        stage(ev, "restart", "Waiting for the RNS-E to restart");
        tokio::time::sleep(Duration::from_secs(25)).await;
        let end = Instant::now() + Duration::from_secs(120);
        let count = loop {
            match bus.u16(RESTART_COUNT).await {
                Ok(c) => break c as i16,
                Err(_) if Instant::now() < end => tokio::time::sleep(Duration::from_secs(2)).await,
                Err(e) => {
                    return Err(format!(
                        "the RNS-E didn't answer after the commit: {e}. Check it, then run Check versions"
                    ));
                }
            }
        };
        if count != 0 {
            return Err(format!("the RNS-E's restart count is {count}, expected 0"));
        }
        stage(ev, "verify", "Reading sector 107 back");
        if bus.read_all(svc::SECTOR_XIP, HEADER).await? != img[..HEADER] {
            return Err("sector 107's header doesn't match the image after the commit".into());
        }
        let live = crate::decoder::sector_crc(bus, "verify", ev).await?;
        if live != crc {
            return Err(format!(
                "sector 107 CRC {live:08X} after the commit, image {crc:08X}"
            ));
        }
        stage(
            ev,
            "verify",
            "Waiting for the module to validate the firmware table",
        );
        let end = Instant::now() + Duration::from_secs(60);
        let running = loop {
            if bus.u32(TVA + 26 * 4).await.unwrap_or(0) == OK2F {
                break true;
            }
            if Instant::now() > end {
                break false;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        };
        // The cold reset cleared the off word; writing 0 also covers a module that kept it.
        if bus.lease(Duration::from_secs(10)).await.is_ok() {
            let _ = bus.send(svc::poke(svc::AUTO_START_WORD, 0)).await;
            bus.release().await;
        }
        if let Ok(s) = self.probe_rnse(bus).await {
            self.seen.lock().unwrap().rnse = Some(s);
        }
        if !running {
            return Err(format!(
                "module {} is written and verified, but it hasn't reported OK2F yet; check the firmware table",
                m.build()
            ));
        }
        Ok(format!("Module {} installed and running", m.build()))
    }

    // ------------------------------------------------------------ Teensy install
    pub async fn install_teensy(self: &std::sync::Arc<Self>, ev: &Events) {
        let r = tokio::time::timeout(Duration::from_secs(10 * 60), self.install_teensy_inner(ev))
            .await
            .unwrap_or_else(|_| Err("stopped after 10 minutes".into()));
        self.emit_status(ev).await;
        let (ok, message) = match r {
            Ok(m) => (true, m),
            Err(m) => (false, m),
        };
        let _ = ev.send(JobEvent::Finished { ok, message });
    }

    async fn install_teensy_inner(&self, ev: &Events) -> Result<String, String> {
        let root = self.root.clone();
        let bundle = tokio::task::spawn_blocking(move || load_bundle(&root))
            .await
            .map_err(|e| e.to_string())?;
        let b = bundle
            .teensy
            .ok_or("this release carries no Teensy image")?;
        let text = tokio::fs::read_to_string(&b.path)
            .await
            .map_err(|e| format!("{}: {e}", b.path.display()))?;
        let mem = crate::teensy::read_hex(&text)?;
        let port = crate::teensy::port();
        if port.is_none() {
            return Err("the Teensy isn't on USB".into());
        }
        let restart = self.stop_chain(ev).await?;
        stage(ev, "flash", "Flashing the Teensy");
        let (say_tx, mut say_rx) = mpsc::unbounded_channel::<String>();
        let job = tokio::task::spawn_blocking(move || {
            crate::teensy::flash(&mem, port.as_deref(), &mut |m| {
                let _ = say_tx.send(m);
            })
        });
        let fwd = {
            let ev = ev.clone();
            tokio::spawn(async move {
                while let Some(m) = say_rx.recv().await {
                    stage(&ev, "flash", m);
                }
            })
        };
        let flashed = job.await.map_err(|e| e.to_string()).and_then(|r| r);
        let _ = fwd.await;
        let r = match flashed {
            Err(e) => Err(e),
            Ok(()) => {
                stage(ev, "verify", "Waiting for the Teensy to come back");
                let end = Instant::now() + Duration::from_secs(20);
                let mut v = Err("the Teensy didn't come back on USB".to_string());
                while Instant::now() < end {
                    if let Some(p) = crate::teensy::port() {
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        v = tokio::task::spawn_blocking(move || crate::teensy::query_version(&p))
                            .await
                            .map_err(|e| e.to_string())
                            .and_then(|r| r);
                        if v.is_ok() {
                            break;
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                v.and_then(|v| {
                    self.seen.lock().unwrap().teensy = Some((v.clone(), SystemTime::now()));
                    if v.trim() == b.version.trim() {
                        Ok(format!("Teensy runs {v}"))
                    } else {
                        Err(format!(
                            "the Teensy reports {v:?}, the bundle {:?}",
                            b.version
                        ))
                    }
                })
            }
        };
        if restart {
            self.start_chain(ev).await;
        }
        r
    }
}

/// An install error before the commit: nothing was written to flash.
struct Pre(String);

#[cfg(test)]
mod tests {
    use super::*;

    fn image(build: u32, fmt: u32) -> Vec<u8> {
        let mut img = vec![0u8; svc::SECTOR_SIZE as usize];
        img[..4].copy_from_slice(b".SDS");
        let payload: Vec<u8> = {
            let mut p = vec![0u8; 0x40];
            p[..4].copy_from_slice(&MAGIC_TVA2.to_be_bytes());
            p[0x14..0x18].copy_from_slice(&build.to_be_bytes());
            p[0x18..0x1C].copy_from_slice(&1u32.to_be_bytes());
            p[0x1C..0x20].copy_from_slice(&fmt.to_be_bytes());
            p[0x30..0x34].copy_from_slice(&0xDEADBEEFu32.to_be_bytes());
            p
        };
        let sum = payload.chunks(4).fold(0u32, |a, w| {
            a.wrapping_add(u32::from_be_bytes(w.try_into().unwrap()))
        });
        img[0x70..0x74].copy_from_slice(&DEST.to_be_bytes());
        img[0x74..0x78].copy_from_slice(&(0x80 + payload.len() as u32).to_be_bytes());
        img[0x78..0x7C].copy_from_slice(&sum.to_be_bytes());
        img[0x80..0x80 + payload.len()].copy_from_slice(&payload);
        img
    }

    fn bundle(build: &str, fmt: u8, history: &[&str]) -> RnseBundle {
        RnseBundle {
            path: PathBuf::new(),
            build_id: build.into(),
            fmt,
            fmt_min: 1,
            table_version: 1,
            sha256: String::new(),
            history: history.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn image_checks() {
        let img = image(0xF46C419D, 0x0101);
        let m = check_image(&img).unwrap();
        assert_eq!(
            (m.build(), m.fmt, m.fmt_min, m.table_version),
            ("F46C419D".into(), 1, 1, 1)
        );
        let mut bad = img.clone();
        bad[0x90] ^= 1;
        assert!(check_image(&bad).unwrap_err().contains("word sum"));
        let mut bad = img.clone();
        bad[0x1FFFF] = 1;
        assert!(check_image(&bad).unwrap_err().contains("tail"));
        let mut bad = img.clone();
        bad[0x70..0x74].copy_from_slice(&0x00C0_0000u32.to_be_bytes());
        assert!(check_image(&bad).is_err());
        assert!(check_image(&img[..0x1000]).is_err());
        assert_eq!(Module::parse(&[0xFF; HEADER]), None);
    }

    #[test]
    fn rnse_comparison() {
        let b = bundle("BBBB0002", 1, &["AAAA0001", "BBBB0002"]);
        let t = Some((1, 1));
        assert_eq!(compare_rnse(Some(("bbbb0002", 1)), &b, t), Cmp::Same);
        assert_eq!(compare_rnse(Some(("AAAA0001", 1)), &b, t), Cmp::Newer);
        assert_eq!(compare_rnse(Some(("CCCC0003", 1)), &b, t), Cmp::Unknown);
        // Stream format orders before history; pre-handshake modules (fmt 0) are older.
        assert_eq!(compare_rnse(Some(("CCCC0003", 0)), &b, t), Cmp::Newer);
        assert_eq!(compare_rnse(Some(("CCCC0003", 2)), &b, t), Cmp::Older);
        let later = bundle("AAAA0001", 1, &["AAAA0001", "BBBB0002"]);
        assert_eq!(compare_rnse(Some(("BBBB0002", 1)), &later, t), Cmp::Older);
        // Empty sector: newer on a known table, unknown without one.
        assert_eq!(compare_rnse(None, &b, t), Cmp::Newer);
        assert_eq!(compare_rnse(None, &b, None), Cmp::Unknown);
        // Table version or RAM map the module doesn't take.
        assert_eq!(
            compare_rnse(Some(("AAAA0001", 1)), &b, Some((2, 1))),
            Cmp::Incompatible
        );
        assert_eq!(compare_rnse(None, &b, Some((1, 2))), Cmp::Incompatible);
    }

    #[test]
    fn teensy_comparison() {
        let a = "sdemu 0.4 (2026-11-05, touch, live stream, pool 1500)";
        let b = "sdemu 0.5 (2026-11-10, touch, live stream, pool 1500, version handshake)";
        assert_eq!(compare_teensy(b, b), Cmp::Same);
        assert_eq!(compare_teensy(a, b), Cmp::Newer);
        assert_eq!(compare_teensy(b, a), Cmp::Older);
        assert_eq!(
            compare_teensy("sdemu 0.5 (2026-11-10, x)", "sdemu 0.5 (2026-11-10, y)"),
            Cmp::Unknown
        );
        assert_eq!(compare_teensy("bench build", b), Cmp::Unknown);
    }

    #[test]
    fn rnse_txt() {
        let l = parse_rnse_txt(
            "# avlink\nRNSE_ANNOUNCED=1\nRNSE_BUILD=f46c419d\nRNSE_FMT=1\nRNSE_FMT_MIN=1\nRNSE_FW_ID=54B\nRNSE_TABLE=1\nRNSE_HS=1\nSTREAM_FMT=1\nTEENSY_VERSION='sdemu 0.5 (2026-11-10, touch)'\n",
        );
        assert!(l.announced);
        assert_eq!(l.build.as_deref(), Some("F46C419D"));
        assert_eq!(
            (l.fmt, l.fmt_min, l.table, l.stream_fmt),
            (1, 1, Some(1), 1)
        );
        assert_eq!(l.teensy.as_deref(), Some("sdemu 0.5 (2026-11-10, touch)"));
        let none = parse_rnse_txt("RNSE_ANNOUNCED=0\nSTREAM_FMT=1\n");
        assert!(!none.announced && none.build.is_none() && none.teensy.is_none());
    }

    fn base_bundle() -> Bundle {
        Bundle {
            rnse: Some(bundle("BBBB0002", 1, &["AAAA0001", "BBBB0002"])),
            teensy: Some(TeensyBundle {
                path: PathBuf::new(),
                version: "sdemu 0.5 (2026-11-10, x)".into(),
            }),
            chain_fmt: Some((1, 1)),
        }
    }

    #[test]
    fn status_without_anything_seen_claims_nothing() {
        let (seen, bundle, chain) = (Seen::default(), base_bundle(), Chain::default());
        let s = status(&Inputs {
            seen: &seen,
            bundle: &bundle,
            chain: &chain,
            busy: false,
            can: true,
        });
        assert_eq!(s.rnse.installed, FirmwareInstalled::Unknown);
        assert_eq!(s.rnse.comparison, Cmp::Unknown);
        assert_eq!(s.rnse.blocked, None);
        assert_eq!(s.protocol, FirmwareProtocol::Unknown);
        assert_eq!(s.teensy.blocked.as_deref(), Some("The Teensy isn't on USB"));
        let empty = Bundle::default();
        let s = status(&Inputs {
            seen: &seen,
            bundle: &empty,
            chain: &chain,
            busy: false,
            can: true,
        });
        assert!(s.rnse.bundled.is_none() && s.rnse.blocked.is_some());
    }

    #[test]
    fn status_from_a_probe_and_from_the_announcement() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
        let mut seen = Seen {
            rnse: Some(RnseSeen {
                module: Module::parse(&image(0xAAAA0001, 0x0101)),
                table: Some(Table {
                    addr: 0x806ACC00,
                    version: 1,
                    fw_id: 0x54B,
                    ram_map: 1,
                    tcb_base: 0,
                    task_tv: 159,
                }),
                running: true,
                at: t0,
            }),
            ..Default::default()
        };
        let bundle = base_bundle();
        let mut chain = Chain {
            teensy_on_usb: true,
            ..Default::default()
        };
        let s = status(&Inputs {
            seen: &seen,
            bundle: &bundle,
            chain: &chain,
            busy: false,
            can: true,
        });
        assert_eq!(
            s.rnse.installed,
            FirmwareInstalled::Reported(identity(
                "stream format 1".into(),
                Some("AAAA0001".into())
            ))
        );
        assert_eq!(s.rnse.comparison, Cmp::Newer);
        assert!(s.rnse.detail.unwrap().contains("54B/1"));
        // A newer announcement of the bundled build wins: same, blocked.
        let live = parse_rnse_txt(
            "RNSE_ANNOUNCED=1\nRNSE_BUILD=BBBB0002\nRNSE_FMT=1\nRNSE_FMT_MIN=1\nRNSE_FW_ID=54B\nRNSE_TABLE=1\nSTREAM_FMT=1\nTEENSY_VERSION='sdemu 0.5 (2026-11-10, x)'\n",
        );
        chain.active = true;
        chain.live = Some((live, t0 + Duration::from_secs(5)));
        let s = status(&Inputs {
            seen: &seen,
            bundle: &bundle,
            chain: &chain,
            busy: false,
            can: true,
        });
        assert_eq!(s.rnse.comparison, Cmp::Same);
        assert_eq!(s.rnse.blocked.as_deref(), Some("Already installed"));
        assert_eq!(s.protocol, FirmwareProtocol::Negotiated("1".into()));
        assert_eq!(s.teensy.comparison, Cmp::Same);
        // Busy blocks everything.
        let s = status(&Inputs {
            seen: &seen,
            bundle: &bundle,
            chain: &chain,
            busy: true,
            can: true,
        });
        assert!(s.busy && s.rnse.blocked.is_some() && s.teensy.blocked.is_some());
        // An unannounced unit: legacy protocol.
        chain.live = Some((parse_rnse_txt("RNSE_ANNOUNCED=0\nSTREAM_FMT=1\n"), t0));
        let s = status(&Inputs {
            seen: &seen,
            bundle: &bundle,
            chain: &chain,
            busy: false,
            can: true,
        });
        assert_eq!(s.protocol, FirmwareProtocol::Legacy);
        // An empty sector on a good table is legacy, and the bundle is newer.
        seen.rnse.as_mut().unwrap().module = None;
        let s = status(&Inputs {
            seen: &seen,
            bundle: &bundle,
            chain: &chain,
            busy: false,
            can: true,
        });
        assert_eq!(s.rnse.installed, FirmwareInstalled::Legacy);
        assert_eq!(s.rnse.comparison, Cmp::Newer);
    }

    #[test]
    fn bundle_from_manifest_and_module_json() {
        let dir = std::env::temp_dir().join(format!("carpi-fw-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("firmware/rnse")).unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            r#"{"format_version":1,"files":[],"firmware":[
                {"target":"teensy","path":"firmware/teensy/sdemu.hex","version":"sdemu 0.5 (2026-11-10)"},
                {"target":"rnse","path":"firmware/rnse/sector_107.bin","version":"F46C419D","chain_fmt":[1,1]}]}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("firmware/rnse/module.json"),
            r#"{"build_id":"f46c419d","sha256":"AB","table_version":1,"stream_fmt":1,"stream_fmt_min":1,"history":["F46C419D"],"sector":107}"#,
        )
        .unwrap();
        let b = load_bundle(&dir);
        let r = b.rnse.unwrap();
        assert_eq!(
            (r.build_id.as_str(), r.sha256.as_str(), r.history.len()),
            ("F46C419D", "ab", 1)
        );
        assert!(r.path.ends_with("firmware/rnse/sector_107.bin"));
        assert_eq!(b.teensy.unwrap().version, "sdemu 0.5 (2026-11-10)");
        assert_eq!(b.chain_fmt, Some((1, 1)));
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(load_bundle(&dir).rnse.is_none());
    }

    /// The entry numbers above against sdlink's fwtab.h, when the submodule is checked out.
    #[test]
    fn fwtab_entry_numbers_match_sdlink() {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sdlink/rnse/src/fwtab.h");
        let Ok(h) = std::fs::read_to_string(&p) else {
            eprintln!("skipped: {} not checked out", p.display());
            return;
        };
        let names: Vec<&str> = h
            .lines()
            .filter_map(|l| l.trim().strip_prefix("X(")?.split(')').next())
            .collect();
        let at = |n: &str| names.iter().position(|x| *x == n).unwrap() as u32;
        assert_eq!(at("TCB_BASE"), FWE_TCB_BASE);
        assert_eq!(at("TASK_TV"), FWE_TASK_TV);
        assert!(h.contains("#define FWT_MAGIC    0x53444657u"));
    }
}
