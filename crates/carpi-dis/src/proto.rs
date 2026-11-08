//! DDP protocol facts: opcodes, the claim, status masks, the capability record
//! and the per-variant cluster profile.
//!
//! Tags: **bench** = measured on the A3 white cluster at 500 kbit/s, **car** =
//! seen in Drew's TT, **guess** = ours. `dc/` = RNS-E-Hudiy `dis_client/`,
//! `[dis]` = `docs/research/dis.md`.

/// TP2 IDs. Head unit sends on 0x6C0, cluster on 0x6C1. DBC + bench `[dis]` §1.
pub const TX_ID: u16 = 0x6C0;
pub const RX_ID: u16 = 0x6C1;

/// Our session announcement. Car + bench (`dc/NATIVE_HANDSHAKE_CONTROLLER.txt:61-71`).
pub const ANNOUNCE: [u8; 6] = [0x15, 0x01, 0x01, 0x02, 0x00, 0x00];
/// Capability query. Car + bench.
pub const CAPS_QUERY: [u8; 1] = [0x08];
/// Setup, priority A0. Car + bench.
pub const SETUP: [u8; 4] = [0x20, 0x3B, 0xA0, 0x00];
/// Release: no display intent. Car + bench (`dc/ddp_protocol.py:1421-1441`).
pub const RELEASE: [u8; 1] = [0x33];
/// Claim + clear of the central window (0,27,64,48). Bench (`dc/dis_service.py:280-361`).
pub const CLAIM: [u8; 7] = [0x52, 0x05, 0x82, 0x00, 0x1B, 0x40, 0x30];
/// Cluster asks us to reinitialise; we confirm with 2F. Bench (`[dis]` §1, BENCH_RESULTS:181-185).
pub const REINIT_REQ: u8 = 0x2E;
pub const REINIT_ACK: [u8; 1] = [0x2F];
/// Our answer when the cluster sends `08`: describes the RNS-E, never echoes the
/// cluster's record. Old code (`dc/ddp_protocol.py:550-552`); not seen on the wire.
pub const OUR_CAPS: [u8; 11] = [
    0x09, 0x20, 0x00, 0x50, 0x00, 0x10, 0x20, 0x21, 0x22, 0x23, 0x00,
];
/// The A3 bench cluster's `09` record (8P0920982G). Bench. The TT's is uncaptured.
pub const WHITE_BENCH_CAPS: [u8; 12] = [
    0x09, 0x20, 0x0B, 0x50, 0x08, 0x0B, 0x50, 0x30, 0x39, 0x00, 0x32, 0x00,
];

pub const OP_REGION: u8 = 0x52;
pub const OP_STATUS: u8 = 0x53;
pub const OP_BITMAP: u8 = 0x55;
pub const OP_TEXT: u8 = 0x57;
pub const OP_LINE: u8 = 0x63;
pub const OP_COMMIT: u8 = 0x39;
pub const OP_ERROR: u8 = 0x0B;

/// Rectangle in centre-relative logical units (the centre is 64x48).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rect {
    pub x: u8,
    pub y: u8,
    pub w: u8,
    pub h: u8,
}

impl Rect {
    pub const fn new(x: u8, y: u8, w: u8, h: u8) -> Self {
        Rect { x, y, w, h }
    }
    pub fn right(&self) -> u16 {
        u16::from(self.x) + u16::from(self.w)
    }
    pub fn bottom(&self) -> u16 {
        u16::from(self.y) + u16::from(self.h)
    }
    pub fn contains(&self, o: &Rect) -> bool {
        o.x >= self.x && o.y >= self.y && o.right() <= self.right() && o.bottom() <= self.bottom()
    }
    pub fn intersects(&self, o: &Rect) -> bool {
        u16::from(self.x) < o.right()
            && u16::from(o.x) < self.right()
            && u16::from(self.y) < o.bottom()
            && u16::from(o.y) < self.bottom()
    }
}

/// Logical centre size. Bench (`dc/native_bitmap.py:10-13`).
pub const CENTRE_W: u8 = 64;
pub const CENTRE_H: u8 = 48;
/// Absolute y of the centre in the 64x88 logical display. Bench.
pub const CENTRE_Y: u8 = 27;
/// The whole centre as a clip.
pub const FULL: Rect = Rect::new(0, 0, CENTRE_W, CENTRE_H);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    /// 128x96 physical, 64x48 logical protocol, native strokes reach every pixel.
    White,
    /// 64x48 real. No hi-res. Untested on red hardware.
    Red,
}

/// Limits and behaviour of one cluster variant. Everything the packer and
/// encoder must respect lives here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Profile {
    pub variant: Variant,
    /// Largest application message. White 105: bench policy; 195 worked, 208
    /// froze (`[dis]` §1). Red 42: old-code policy (`dc/dis_service.py:641,677`), guess.
    pub max_msg: usize,
    /// Bytes the cluster buffers before it applies them on its own. White:
    /// one message, bench 2026-11-08 (no-`39` bar probe grew per message), so
    /// `max_msg`. Red: same, guess.
    pub cmd_buffer: usize,
    /// Glyphs per `57`. Stock 49-byte record budget (`dc/native_nav_glyphs.py:44`). Bench.
    pub max_glyphs: usize,
    /// Rows per `55` record. White: up to 12 verified, 22 froze (bench,
    /// `dc/bitmap_rows_profile.txt:21-25`). Red: 1 (`dc/dis_service.py:685`).
    pub max_bitmap_rows: usize,
    /// Native strokes (2x2 sub-pixel) allowed. White only.
    pub hires: bool,
}

impl Profile {
    pub const WHITE: Profile = Profile {
        variant: Variant::White,
        max_msg: 105,
        cmd_buffer: 105,
        max_glyphs: 44,
        max_bitmap_rows: 12,
        hires: true,
    };
    pub const RED: Profile = Profile {
        variant: Variant::Red,
        max_msg: 42,
        cmd_buffer: 42,
        max_glyphs: 44,
        max_bitmap_rows: 1,
        hires: false,
    };
}

/// Parsed `09` capability record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caps {
    pub raw: Vec<u8>,
    /// Byte 1. 0x20 = mono `52` command family (white TT/A3). Bench
    /// (`dc/ddp_application.py:16-38`). 0x10 = colour/other: we don't render.
    pub format: u8,
}

impl Caps {
    pub fn parse(raw: &[u8]) -> Option<Caps> {
        if raw.len() < 6 || raw[0] != 0x09 {
            return None;
        }
        Some(Caps {
            raw: raw.to_vec(),
            format: raw[1],
        })
    }

    /// Profile for this record, or `None` if we can't draw on it.
    ///
    /// White vs red is a guess: no red `09` record exists. The bench record is
    /// white; any other mono-format record takes `unknown_mono`. The old code told
    /// them apart at TP2 open instead (red broadcasts `A0 07 00`,
    /// `dc/ddp_protocol.py:1280-1316`); the cluster variant is therefore also
    /// settable from config.
    pub fn profile(&self, unknown_mono: Variant) -> Option<Profile> {
        if self.format != 0x20 {
            return None;
        }
        let v = if self.raw == WHITE_BENCH_CAPS {
            Variant::White
        } else {
            unknown_mono
        };
        Some(match v {
            Variant::White => Profile::WHITE,
            Variant::Red => Profile::RED,
        })
    }
}

/// `53 xx` window status, by the stock mask order (`dc/ddp_application.py:71-87`). Bench.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowStatus {
    /// Bit 0x40. `53 C0` answers non-central claims.
    Fault,
    /// `xx & 3 == 0`: busy (cluster warning, menu): `53 84`, `53 04`.
    Busy,
    /// Bit 0x80: `53 85` (central). The only proof of ownership.
    Granted,
    /// Free again: `53 05`. A `2E` usually follows.
    Free,
}

impl WindowStatus {
    pub fn parse(v: u8) -> WindowStatus {
        if v & 0x40 != 0 {
            WindowStatus::Fault
        } else if v & 3 == 0 {
            WindowStatus::Busy
        } else if v & 0x80 != 0 {
            WindowStatus::Granted
        } else {
            WindowStatus::Free
        }
    }
}
