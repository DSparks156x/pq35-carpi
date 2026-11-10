//! The SDLink Teensy 4.1: its firmware version (console `ver`) and flashing
//! it through the HalfKay bootloader, the same blocks as sdlink's
//! `teensy/tools/bench/hkflash_linux.py` and `teensy_loader_cli`.
//!
//! The port is avlink's while the chain runs: stop the chain first.
//! HalfKay's hidraw node needs the bundle's udev rule
//! (`share/udev/99-carpi-teensy.rules`) to be writable without root.

#![cfg_attr(not(unix), allow(dead_code))]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::time::{Duration, Instant};

/// `/dev/serial/by-id` name prefix of the SDLink Teensy (USB serial + touch).
pub const BY_ID: &str = "usb-Teensyduino_SDemu_Touch_";
const FLASH_BASE: u32 = 0x6000_0000;
const FLASH_SIZE: u32 = 8_126_464;
const BLOCK: usize = 1024;
/// HalfKay's USB id, as hidraw's uevent spells it.
const HALFKAY: &str = "HID_ID=0003:000016C0:00000478";

/// The Teensy's serial port, if it is on USB.
pub fn port() -> Option<PathBuf> {
    std::fs::read_dir("/dev/serial/by-id")
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy())
                .is_some_and(|n| n.starts_with(BY_ID) && n.ends_with("-if00"))
        })
}

/// The version from one console line: `VER sdemu 0.5 (...) flexio2_param=...`,
/// or `BOOT`/`HELP` followed by the version. Same rule as avlink's.
pub fn version_from_line(line: &str) -> Option<String> {
    let line = line.trim_end_matches(['\r', '\n', ' ']);
    let rest = ["VER ", "BOOT ", "HELP ", "ST begin "]
        .iter()
        .find_map(|p| line.strip_prefix(p))?;
    let v = rest.split(" flexio2_param=").next()?.trim_end();
    v.starts_with("sdemu").then(|| v.to_string())
}

/// A Teensy HEX image: flash address -> byte.
pub fn read_hex(text: &str) -> Result<BTreeMap<u32, u8>, String> {
    let mut mem = BTreeMap::new();
    let mut base = 0u32;
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        let Some(hex) = line.strip_prefix(':') else {
            continue;
        };
        let b = (0..hex.len() / 2)
            .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16))
            .collect::<Result<Vec<u8>, _>>()
            .map_err(|_| format!("hex line {}: not hex", n + 1))?;
        if b.len() < 5 || b.len() != b[0] as usize + 5 {
            return Err(format!("hex line {}: bad length", n + 1));
        }
        if b.iter().fold(0u8, |a, x| a.wrapping_add(*x)) != 0 {
            return Err(format!("hex line {}: bad checksum", n + 1));
        }
        let (len, addr, kind) = (b[0] as usize, u16::from_be_bytes([b[1], b[2]]) as u32, b[3]);
        let data = &b[4..4 + len];
        match kind {
            0 => {
                for (i, x) in data.iter().enumerate() {
                    mem.insert(base + addr + i as u32, *x);
                }
            }
            1 => break,
            2 if len == 2 => base = (u16::from_be_bytes([data[0], data[1]]) as u32) << 4,
            4 if len == 2 => base = (u16::from_be_bytes([data[0], data[1]]) as u32) << 16,
            3 | 5 => {}
            _ => return Err(format!("hex line {}: record type {kind}", n + 1)),
        }
    }
    let (Some((&lo, _)), Some((&hi, _))) = (mem.first_key_value(), mem.last_key_value()) else {
        return Err("hex file is empty".into());
    };
    if lo < FLASH_BASE || hi >= FLASH_BASE + FLASH_SIZE {
        return Err(format!(
            "hex spans {lo:08X}-{hi:08X}: not a Teensy 4.1 flash image"
        ));
    }
    Ok(mem)
}

/// HalfKay reports: 64-byte header (address) + 1024 data bytes, behind a
/// report id byte; then the boot report.
pub fn blocks(mem: &BTreeMap<u32, u8>) -> Vec<Vec<u8>> {
    let mut offs: Vec<u32> = mem
        .keys()
        .map(|a| (a - FLASH_BASE) / BLOCK as u32)
        .collect();
    offs.dedup();
    offs.iter()
        .map(|blk| {
            let addr = blk * BLOCK as u32;
            let mut pkt = vec![0u8, addr as u8, (addr >> 8) as u8, (addr >> 16) as u8];
            pkt.resize(65, 0);
            pkt.extend(
                (0..BLOCK as u32).map(|i| *mem.get(&(FLASH_BASE + addr + i)).unwrap_or(&0xFF)),
            );
            pkt
        })
        .collect()
}

fn boot_report() -> Vec<u8> {
    let mut pkt = vec![0u8, 0xFF, 0xFF, 0xFF];
    pkt.resize(65 + BLOCK, 0);
    pkt
}

fn find_halfkay() -> Option<PathBuf> {
    for e in std::fs::read_dir("/sys/class/hidraw").ok()?.flatten() {
        let uevent = std::fs::read_to_string(e.path().join("device/uevent")).unwrap_or_default();
        if uevent.to_ascii_uppercase().contains(HALFKAY) {
            return Some(Path::new("/dev").join(e.file_name()));
        }
    }
    None
}

#[cfg(unix)]
mod tty {
    use std::os::fd::AsRawFd;

    /// Raw, no echo; reads return after `tenths` of a second without data.
    pub fn raw(f: &std::fs::File, speed: libc::speed_t, tenths: u8) -> std::io::Result<()> {
        let fd = f.as_raw_fd();
        // SAFETY: tcgetattr/tcsetattr on an open descriptor with a zeroed struct it fills.
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(fd, &mut t) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            libc::cfmakeraw(&mut t);
            t.c_cflag |= libc::CLOCAL | libc::CREAD;
            t.c_cc[libc::VMIN] = 0;
            t.c_cc[libc::VTIME] = tenths;
            libc::cfsetispeed(&mut t, speed);
            libc::cfsetospeed(&mut t, speed);
            if libc::tcsetattr(fd, libc::TCSANOW, &t) != 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
        Ok(())
    }
}

/// Asks the Teensy its version on its console. Blocking (about 2 s at most);
/// only while nothing else has the port open.
#[cfg(unix)]
pub fn query_version(port: &Path) -> Result<String, String> {
    use std::io::{Read, Write};
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY)
        .open(port)
        .map_err(|e| format!("{}: {e}", port.display()))?;
    tty::raw(&f, libc::B115200, 2).map_err(|e| format!("{}: {e}", port.display()))?;
    // The Teensy drops bytes that come within its DTR debounce after open.
    std::thread::sleep(Duration::from_millis(50));
    let end = Instant::now() + Duration::from_secs(2);
    let mut text = String::new();
    let mut asked = Instant::now() - Duration::from_secs(1);
    while Instant::now() < end {
        if asked.elapsed() >= Duration::from_millis(500) {
            f.write_all(b"\nver\n")
                .map_err(|e| format!("{}: {e}", port.display()))?;
            asked = Instant::now();
        }
        let mut buf = [0u8; 512];
        let n = f.read(&mut buf).unwrap_or(0);
        text.push_str(&String::from_utf8_lossy(&buf[..n]));
        if let Some(v) = text
            .lines()
            .filter(|_| text.contains('\n'))
            .find_map(version_from_line)
        {
            return Ok(v);
        }
    }
    Err("the Teensy didn't answer 'ver'".into())
}

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

/// Flashes `hex` (already parsed). `say` gets progress lines. Blocking, up to
/// a few minutes (the first block waits for the erase).
#[cfg(unix)]
pub fn flash(
    mem: &BTreeMap<u32, u8>,
    port: Option<&Path>,
    say: &mut dyn FnMut(String),
) -> Result<(), String> {
    use std::io::Write;
    let blocks = blocks(mem);
    if find_halfkay().is_none() {
        // 134 baud on the serial port asks the running firmware to reboot into HalfKay.
        let Some(port) = port else {
            return Err("the Teensy isn't on USB".into());
        };
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOCTTY | libc::O_NONBLOCK)
            .open(port)
            .map_err(|e| format!("{}: {e}", port.display()))?;
        tty::raw(&f, libc::B134, 0).map_err(|e| format!("reboot request: {e}"))?;
        drop(f);
    }
    let start = Instant::now();
    let dev = loop {
        if let Some(d) = find_halfkay() {
            break d;
        }
        if start.elapsed() > Duration::from_secs(15) {
            return Err(
                "the Teensy bootloader did not appear; press the Teensy's button and try again"
                    .into(),
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    say(format!(
        "bootloader at {}; writing {} blocks",
        dev.display(),
        blocks.len()
    ));
    std::thread::sleep(Duration::from_millis(200));
    let mut hid = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&dev)
        .map_err(|e| format!("{}: {e} (is the udev rule installed?)", dev.display()))?;
    for (k, pkt) in blocks.iter().enumerate() {
        // The first block erases the flash and is refused until that's done.
        let limit = Instant::now() + Duration::from_secs(if k == 0 { 180 } else { 10 });
        loop {
            match hid.write(pkt) {
                Ok(_) => break,
                Err(e) if Instant::now() > limit => {
                    return Err(format!(
                        "block {k} refused: {e}; the Teensy may need its button pressed"
                    ));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
        if (k + 1) % 64 == 0 {
            say(format!("{} of {} blocks", k + 1, blocks.len()));
        }
    }
    let _ = hid.write(&boot_report());
    say(format!(
        "{} blocks in {:.1} s; restarting",
        blocks.len(),
        start.elapsed().as_secs_f32()
    ));
    Ok(())
}

#[cfg(not(unix))]
pub fn query_version(_: &Path) -> Result<String, String> {
    Err("not supported on this OS".into())
}

#[cfg(not(unix))]
pub fn flash(
    _: &BTreeMap<u32, u8>,
    _: Option<&Path>,
    _: &mut dyn FnMut(String),
) -> Result<(), String> {
    Err("not supported on this OS".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_lines() {
        assert_eq!(
            version_from_line(
                "VER sdemu 0.5 (2026-11-10, touch) flexio2_param=00 f_cpu=600000000\r"
            )
            .as_deref(),
            Some("sdemu 0.5 (2026-11-10, touch)")
        );
        assert_eq!(
            version_from_line("BOOT sdemu 0.4 (x)").as_deref(),
            Some("sdemu 0.4 (x)")
        );
        assert_eq!(version_from_line("LINK active=1"), None);
        assert_eq!(version_from_line("VER junk"), None);
    }

    #[test]
    fn hex_parses_into_halfkay_blocks() {
        // extended linear address 0x6000, 4 bytes at 0x60000000, 1 byte at 0x60000400, EOF
        let hex = ":020000046000\u{39}A\n:0400000001020304F2\n:01040000AA51\n:00000001FF\n";
        let mem = read_hex(hex).unwrap();
        assert_eq!(mem.len(), 5);
        assert_eq!(mem[&0x6000_0000], 1);
        assert_eq!(mem[&0x6000_0400], 0xAA);
        let b = blocks(&mem);
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].len(), 65 + BLOCK);
        assert_eq!(&b[0][..4], &[0, 0, 0, 0]);
        assert_eq!(&b[0][65..70], &[1, 2, 3, 4, 0xFF]);
        assert_eq!(&b[1][..4], &[0, 0x00, 0x04, 0]);
        assert_eq!(b[1][65], 0xAA);
        assert_eq!(boot_report()[1..4], [0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn hex_outside_flash_or_corrupt_is_refused() {
        assert!(read_hex(":0400000001020304F2\n:00000001FF\n").is_err()); // address 0
        assert!(read_hex(":020000046000\u{39}A\n:0400000001020304F3\n").is_err()); // checksum
        assert!(read_hex("").is_err());
    }
}
