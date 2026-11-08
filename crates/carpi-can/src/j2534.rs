//! J2534 (PassThru 04.04) CAN port, Windows only. For the bench cluster on
//! Drew's PC; the car uses SocketCAN.
//!
//! The vendor DLL is loaded at runtime. Most are 32-bit (Drew's Scanmatik SM2
//! `smj2534.dll` is), and a 64-bit process can't load those: build the tool
//! for `i686-pc-windows-msvc`. [`open`] checks the DLL's machine type first.
//!
//! One reader thread polls `PassThruReadMsgs` and feeds [`Port::recv`];
//! [`TxPort::send`] writes from the caller. Both go through one lock, since the
//! spec doesn't promise the DLL is thread-safe.

use crate::{Frame, Port, RxFrame, TxPort};
use std::ffi::c_void;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::mpsc;

const PROTOCOL_CAN: u32 = 5;
const CAN_29BIT_ID: u32 = 0x100;
/// RxStatus bits that mark an echo or an indication rather than a bus frame.
const RX_NOT_DATA: u32 = 0x01 | 0x02 | 0x08;
const FILTER_PASS: u32 = 1;
const IOCTL_CLEAR_TX: u32 = 7;
const IOCTL_CLEAR_RX: u32 = 8;
const ERR_TIMEOUT: i32 = 0x09;
const ERR_BUFFER_EMPTY: i32 = 0x10;
const ERR_BUFFER_FULL: i32 = 0x11;
const ERR_BUFFER_OVERFLOW: i32 = 0x12;
const READ_BATCH: usize = 32;
const READ_TIMEOUT_MS: u32 = 5;
const WRITE_TIMEOUT_MS: u32 = 50;
const REG_KEY: &str = r"SOFTWARE\PassThruSupport.04.04";

/// `PASSTHRU_MSG`.
#[repr(C)]
#[derive(Clone)]
pub struct PassThruMsg {
    pub protocol_id: u32,
    pub rx_status: u32,
    pub tx_flags: u32,
    pub timestamp: u32,
    pub data_size: u32,
    pub extra_data_index: u32,
    pub data: [u8; 4128],
}

impl Default for PassThruMsg {
    fn default() -> Self {
        PassThruMsg {
            protocol_id: 0,
            rx_status: 0,
            tx_flags: 0,
            timestamp: 0,
            data_size: 0,
            extra_data_index: 0,
            data: [0; 4128],
        }
    }
}

/// CAN frame → message: 4-byte big-endian ID, then the data.
pub fn to_msg(f: &Frame) -> PassThruMsg {
    let mut m = PassThruMsg {
        protocol_id: PROTOCOL_CAN,
        tx_flags: if f.extended { CAN_29BIT_ID } else { 0 },
        data_size: 4 + f.len() as u32,
        ..Default::default()
    };
    m.data[..4].copy_from_slice(&f.id.to_be_bytes());
    m.data[4..4 + f.len()].copy_from_slice(f.data());
    m
}

/// Message → CAN frame. `None` for echoes, indications and bad sizes.
pub fn from_msg(m: &PassThruMsg) -> Option<Frame> {
    if m.rx_status & RX_NOT_DATA != 0 || !(4..=12).contains(&m.data_size) {
        return None;
    }
    let id = u32::from_be_bytes([m.data[0], m.data[1], m.data[2], m.data[3]]);
    let data = &m.data[4..m.data_size as usize];
    if m.rx_status & CAN_29BIT_ID != 0 {
        Frame::new_extended(id, data)
    } else {
        Frame::new(id, data)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    PassAll,
    /// Pass when `id & mask == pattern`.
    Mask {
        mask: u32,
        pattern: u32,
    },
}

#[derive(Clone, Debug)]
pub struct Options {
    /// DLL path. If `None`, from the registry: the entry whose name contains
    /// `device`, or the only entry.
    pub dll: Option<PathBuf>,
    pub device: Option<String>,
    pub bitrate: u32,
    pub filter: Filter,
}

/// `(registry entry name, FunctionLibrary)` for every 04.04 device.
pub fn installed() -> Vec<(String, PathBuf)> {
    reg::list(REG_KEY)
}

fn pick_dll(o: &Options) -> io::Result<PathBuf> {
    if let Some(p) = &o.dll {
        return Ok(p.clone());
    }
    let all = installed();
    let hits: Vec<_> = all
        .iter()
        .filter(|(name, _)| {
            o.device
                .as_ref()
                .is_none_or(|d| name.to_lowercase().contains(&d.to_lowercase()))
        })
        .collect();
    match hits.as_slice() {
        [(_, p)] => Ok(p.clone()),
        _ => Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "pick one J2534 device ({} match); installed: {:?}",
                hits.len(),
                all
            ),
        )),
    }
}

/// PE machine type of a DLL: 0x14C x86, 0x8664 x64.
fn pe_machine(path: &Path) -> io::Result<u16> {
    let b = std::fs::read(path)?;
    let bad = || io::Error::new(io::ErrorKind::InvalidData, "not a PE file");
    let off = u32::from_le_bytes(b.get(0x3C..0x40).ok_or_else(bad)?.try_into().unwrap()) as usize;
    if b.get(off..off + 4) != Some(b"PE\0\0") {
        return Err(bad());
    }
    Ok(u16::from_le_bytes(
        b.get(off + 4..off + 6).ok_or_else(bad)?.try_into().unwrap(),
    ))
}

type FnOpen = unsafe extern "system" fn(*const c_void, *mut u32) -> i32;
type FnId = unsafe extern "system" fn(u32) -> i32;
type FnConnect = unsafe extern "system" fn(u32, u32, u32, u32, *mut u32) -> i32;
type FnMsgs = unsafe extern "system" fn(u32, *mut PassThruMsg, *mut u32, u32) -> i32;
type FnStartFilter = unsafe extern "system" fn(
    u32,
    u32,
    *const PassThruMsg,
    *const PassThruMsg,
    *const PassThruMsg,
    *mut u32,
) -> i32;
type FnStopFilter = unsafe extern "system" fn(u32, u32) -> i32;
type FnIoctl = unsafe extern "system" fn(u32, u32, *mut c_void, *mut c_void) -> i32;
type FnLastError = unsafe extern "system" fn(*mut u8) -> i32;

struct Api {
    close: FnId,
    disconnect: FnId,
    read: FnMsgs,
    write: FnMsgs,
    stop_filter: FnStopFilter,
    ioctl: FnIoctl,
    last_error: FnLastError,
}

/// An open device + CAN channel. Closed on drop.
pub struct Device {
    api: Api,
    device: u32,
    channel: u32,
    filter: u32,
    lock: Mutex<()>,
    /// Writers waiting for the lock; the reader backs off for them.
    writers: AtomicUsize,
    pub dll: PathBuf,
    _lib: libloading::Library,
}

impl Device {
    fn err(&self, what: &str, code: i32) -> io::Error {
        let mut buf = [0u8; 256];
        // SAFETY: the spec's buffer is 80 chars; ours is bigger.
        unsafe { (self.api.last_error)(buf.as_mut_ptr()) };
        let text = buf.split(|b| *b == 0).next().unwrap_or(&[]);
        io::Error::other(format!(
            "{what}: J2534 error 0x{code:02X} {}",
            String::from_utf8_lossy(text)
        ))
    }

    fn open(o: &Options) -> io::Result<Device> {
        let dll = pick_dll(o)?;
        let machine = pe_machine(&dll)?;
        let want = if cfg!(target_pointer_width = "64") {
            0x8664
        } else {
            0x14C
        };
        if machine != want {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "{} is {}-bit and this build isn't; build with --target {}",
                    dll.display(),
                    if machine == 0x14C { 32 } else { 64 },
                    if machine == 0x14C {
                        "i686-pc-windows-msvc"
                    } else {
                        "x86_64-pc-windows-msvc"
                    }
                ),
            ));
        }
        // SAFETY: a J2534 DLL; loading runs its DllMain.
        let lib = unsafe { libloading::Library::new(&dll) }.map_err(io::Error::other)?;
        macro_rules! sym {
            ($name:literal) => {
                // SAFETY: the signature is the J2534 04.04 one for this name.
                *unsafe { lib.get(concat!($name, "\0").as_bytes()) }
                    .map_err(|e| io::Error::other(format!("{}: {e}", $name)))?
            };
        }
        let open: FnOpen = sym!("PassThruOpen");
        let connect: FnConnect = sym!("PassThruConnect");
        let start_filter: FnStartFilter = sym!("PassThruStartMsgFilter");
        let api = Api {
            close: sym!("PassThruClose"),
            disconnect: sym!("PassThruDisconnect"),
            read: sym!("PassThruReadMsgs"),
            write: sym!("PassThruWriteMsgs"),
            stop_filter: sym!("PassThruStopMsgFilter"),
            ioctl: sym!("PassThruIoctl"),
            last_error: sym!("PassThruGetLastError"),
        };
        let mut d = Device {
            api,
            device: 0,
            channel: 0,
            filter: 0,
            lock: Mutex::new(()),
            writers: AtomicUsize::new(0),
            dll,
            _lib: lib,
        };
        // From here on, drop undoes whatever succeeded.
        let mut id = 0;
        // SAFETY: valid out-pointer; null name = first device.
        let r = unsafe { open(std::ptr::null(), &mut id) };
        if r != 0 {
            return Err(d.err("PassThruOpen", r));
        }
        d.device = id;
        let mut ch = 0;
        // SAFETY: valid out-pointer.
        let r = unsafe { connect(d.device, PROTOCOL_CAN, 0, o.bitrate, &mut ch) };
        if r != 0 {
            return Err(d.err("PassThruConnect", r));
        }
        d.channel = ch;
        let (mask, pattern) = match o.filter {
            Filter::PassAll => (0, 0),
            Filter::Mask { mask, pattern } => (mask, pattern),
        };
        let id_msg = |v: u32| PassThruMsg {
            protocol_id: PROTOCOL_CAN,
            data_size: 4,
            data: {
                let mut a = [0; 4128];
                a[..4].copy_from_slice(&v.to_be_bytes());
                a
            },
            ..Default::default()
        };
        let (m, p) = (id_msg(mask), id_msg(pattern));
        let mut fid = 0;
        // SAFETY: valid message pointers; no flow control for a pass filter.
        let r = unsafe { start_filter(d.channel, FILTER_PASS, &m, &p, std::ptr::null(), &mut fid) };
        if r != 0 {
            return Err(d.err("PassThruStartMsgFilter", r));
        }
        d.filter = fid;
        // SAFETY: these ioctls take no arguments.
        unsafe {
            (d.api.ioctl)(
                d.channel,
                IOCTL_CLEAR_TX,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            (d.api.ioctl)(
                d.channel,
                IOCTL_CLEAR_RX,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
        }
        Ok(d)
    }

    /// Up to `buf.len()` frames' worth of messages; returns how many arrived.
    fn read(&self, buf: &mut [PassThruMsg]) -> io::Result<usize> {
        while self.writers.load(Ordering::Acquire) > 0 {
            std::thread::yield_now();
        }
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut n = buf.len() as u32;
        // SAFETY: `buf` holds `n` messages.
        let r = unsafe { (self.api.read)(self.channel, buf.as_mut_ptr(), &mut n, READ_TIMEOUT_MS) };
        match r {
            0 | ERR_TIMEOUT | ERR_BUFFER_EMPTY | ERR_BUFFER_OVERFLOW => Ok(n as usize),
            _ => Err(self.err("PassThruReadMsgs", r)),
        }
    }

    pub fn write(&self, f: &Frame) -> io::Result<()> {
        let mut m = to_msg(f);
        self.writers.fetch_add(1, Ordering::AcqRel);
        let g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.writers.fetch_sub(1, Ordering::AcqRel);
        let mut n = 1;
        // SAFETY: one valid message.
        let r = unsafe { (self.api.write)(self.channel, &mut m, &mut n, WRITE_TIMEOUT_MS) };
        if r == ERR_BUFFER_FULL || r == ERR_BUFFER_OVERFLOW {
            // A full queue (no ACK on the bus) would block every later send.
            // SAFETY: no arguments.
            unsafe {
                (self.api.ioctl)(
                    self.channel,
                    IOCTL_CLEAR_TX,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
        }
        drop(g);
        if r != 0 {
            return Err(self.err("PassThruWriteMsgs", r));
        }
        Ok(())
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        // SAFETY: ids came from the DLL; zero means that step never happened.
        unsafe {
            if self.filter != 0 {
                (self.api.stop_filter)(self.channel, self.filter);
            }
            if self.channel != 0 {
                (self.api.disconnect)(self.channel);
            }
            if self.device != 0 {
                (self.api.close)(self.device);
            }
        }
    }
}

/// Receive side: a reader thread feeding a channel, so `recv` is cancel-safe.
pub struct J2534Port {
    dev: Arc<Device>,
    rx: mpsc::UnboundedReceiver<io::Result<RxFrame>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl J2534Port {
    pub fn device(&self) -> &Arc<Device> {
        &self.dev
    }

    pub(crate) async fn recv(&mut self) -> io::Result<Option<RxFrame>> {
        match self.rx.recv().await {
            Some(r) => r.map(Some),
            None => Ok(None),
        }
    }
}

impl Drop for J2534Port {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Opens the device and channel. Disconnect and close happen when both
/// halves are dropped.
pub fn open(o: &Options) -> io::Result<(Port, TxPort)> {
    let dev = Arc::new(Device::open(o)?);
    let (tx, rx) = mpsc::unbounded_channel();
    let stop = Arc::new(AtomicBool::new(false));
    let thread = {
        let (dev, stop) = (dev.clone(), stop.clone());
        std::thread::Builder::new()
            .name("j2534-rx".into())
            .spawn(move || {
                let mut buf = vec![PassThruMsg::default(); READ_BATCH];
                while !stop.load(Ordering::Acquire) {
                    match dev.read(&mut buf) {
                        Ok(n) => {
                            let (at, wall) = (Instant::now(), SystemTime::now());
                            for m in &buf[..n] {
                                if let Some(frame) = from_msg(m) {
                                    let _ = tx.send(Ok(RxFrame { frame, at, wall }));
                                }
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(Err(e));
                            std::thread::sleep(Duration::from_millis(100));
                        }
                    }
                }
            })?
    };
    let txp = TxPort::j2534(dev.clone());
    Ok((
        Port::J2534(J2534Port {
            dev,
            rx,
            stop,
            thread: Some(thread),
        }),
        txp,
    ))
}

mod reg {
    use std::path::PathBuf;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY, RRF_RT_REG_SZ,
        RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    /// Subkeys of HKLM\`key` with their `FunctionLibrary`, 32- then 64-bit view.
    pub fn list(key: &str) -> Vec<(String, PathBuf)> {
        let mut out = Vec::new();
        for view in [KEY_WOW64_32KEY, KEY_WOW64_64KEY] {
            let mut h: HKEY = std::ptr::null_mut();
            // SAFETY: valid strings and out-pointer.
            if unsafe {
                RegOpenKeyExW(
                    HKEY_LOCAL_MACHINE,
                    wide(key).as_ptr(),
                    0,
                    KEY_READ | view,
                    &mut h,
                )
            } != 0
            {
                continue;
            }
            for i in 0.. {
                let mut name = [0u16; 256];
                let mut len = name.len() as u32;
                // SAFETY: `name` holds `len` chars.
                let r = unsafe {
                    RegEnumKeyExW(
                        h,
                        i,
                        name.as_mut_ptr(),
                        &mut len,
                        std::ptr::null(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                };
                if r != 0 {
                    break;
                }
                let sub = &name[..len as usize];
                let mut val = [0u16; 520];
                let mut bytes = (val.len() * 2) as u32;
                let mut subz = sub.to_vec();
                subz.push(0);
                // SAFETY: `val` holds `bytes` bytes.
                let r = unsafe {
                    RegGetValueW(
                        h,
                        subz.as_ptr(),
                        wide("FunctionLibrary").as_ptr(),
                        RRF_RT_REG_SZ,
                        std::ptr::null_mut(),
                        val.as_mut_ptr().cast(),
                        &mut bytes,
                    )
                };
                if r == 0 {
                    let n = (bytes as usize / 2).saturating_sub(1);
                    let entry = (
                        String::from_utf16_lossy(sub),
                        PathBuf::from(String::from_utf16_lossy(&val[..n])),
                    );
                    if !out.contains(&entry) {
                        out.push(entry);
                    }
                }
            }
            // SAFETY: opened above.
            unsafe { RegCloseKey(h) };
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_round_trip() {
        let f = Frame::new(0x6C0, &[0x10, 0x00, 0x01, 0x2F]).unwrap();
        let m = to_msg(&f);
        assert_eq!(m.protocol_id, PROTOCOL_CAN);
        assert_eq!(m.tx_flags, 0);
        assert_eq!(m.data_size, 8);
        assert_eq!(&m.data[..8], &[0, 0, 0x06, 0xC0, 0x10, 0x00, 0x01, 0x2F]);
        assert_eq!(from_msg(&m), Some(f));
    }

    #[test]
    fn extended_and_empty() {
        let f = Frame::new_extended(0x17F0_0010, &[]).unwrap();
        let mut m = to_msg(&f);
        assert_eq!(m.tx_flags, CAN_29BIT_ID);
        m.rx_status = CAN_29BIT_ID;
        assert_eq!(from_msg(&m), Some(f));
    }

    #[test]
    fn drops_echoes_and_bad_sizes() {
        let mut m = to_msg(&Frame::new(0x575, &[0xFF; 8]).unwrap());
        assert!(from_msg(&m).is_some());
        for s in [0x01, 0x02, 0x08] {
            m.rx_status = s;
            assert_eq!(from_msg(&m), None);
        }
        m.rx_status = 0;
        m.data_size = 13;
        assert_eq!(from_msg(&m), None);
        m.data_size = 3;
        assert_eq!(from_msg(&m), None);
        // An 11-bit frame with an out-of-range ID is refused by Frame::new.
        m.data_size = 4;
        m.data[..4].copy_from_slice(&0x800u32.to_be_bytes());
        assert_eq!(from_msg(&m), None);
    }

    #[test]
    fn msg_layout() {
        assert_eq!(std::mem::size_of::<PassThruMsg>(), 24 + 4128);
    }
}
