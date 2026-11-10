//! What carpid may send to the RNS-E's service commands on 0x7B0 (sdlink
//! docs/PI_UPDATER.md): B1 memory reads, and for a sector 107 update only,
//! B9 to the auto start word, B3 staging, B7 + B6 commit.
//!
//! Reads pass through the `Diag` class, or `Lease` while the bus is leased.
//! Everything that writes needs the lease, which the updater takes for the
//! write and which stops every other sender on ICAN meanwhile. Each frame is
//! checked against a narrow shape: B9 only to the one word, B7/B6 only to
//! sector 107, one target, armed.

use carpi_can::Frame;

/// The SDLink module's sector, the only one an update writes.
pub const SECTOR: u16 = 107;
/// Its XIP address and size.
pub const SECTOR_XIP: u32 = 0x80D60000;
pub const SECTOR_SIZE: u32 = 0x20000;
/// TVA word 1: `'OFF!'` stops auto start, 0 re-arms it.
pub const AUTO_START_WORD: u32 = 0x8F72F004;
/// B6 arm byte.
pub const ARM: u8 = 0x5A;

/// Windows B1 may read from: flash (P1), RAM, the B3 staging RAM (P2) and
/// the boot ROM's restart state.
const READ_WINDOWS: &[(u32, u32)] = &[
    (0x8000_0000, 0x8400_0000),
    (0x8C00_0000, 0x9000_0000),
    (0xACE0_0000, 0xACE8_0000),
    (0xB800_0000, 0xB800_0040),
];

/// RAM words B9 may write.
const POKE_WORDS: &[u32] = &[AUTO_START_WORD];

/// B9 addresses words from here.
const POKE_BASE: u32 = 0x8C00_0000;

/// What a frame needs from the gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    /// A read: allowed with or without the lease.
    Read,
    /// A write: only under the lease.
    Lease,
}

/// Checks one 0x7B0 payload. `Err` names what is wrong with it.
pub fn classify(d: &[u8; 8]) -> Result<Need, String> {
    match d[0] {
        0xB1 => {
            let addr = u32::from_be_bytes([d[1], d[2], d[3], d[4]]);
            let len = d[5] as u32;
            if !(1..=64).contains(&len) || d[6..] != [0, 0] {
                return Err(format!("B1 read of {len} bytes"));
            }
            let end = addr.checked_add(len).ok_or("B1 read wraps")?;
            if !READ_WINDOWS.iter().any(|&(a, b)| a <= addr && end <= b) {
                return Err(format!(
                    "B1 read at {addr:08X} is outside the allowed windows"
                ));
            }
            Ok(Need::Read)
        }
        0xB3 => {
            if d[1] == 0xFF && d[2] == 0xFF {
                let n = u32::from_be_bytes([0, d[3], d[4], d[5]]);
                if n == 0 || n > SECTOR_SIZE || d[6..] != [0, 0] {
                    return Err(format!("B3 END over {n} bytes"));
                }
            }
            Ok(Need::Lease)
        }
        0xB7 => {
            let sidx = u16::from_be_bytes([d[2], d[3]]);
            if d[1] != 0 || sidx != SECTOR {
                return Err(format!(
                    "B7 slot {} sector {sidx}: only slot 0, sector {SECTOR}",
                    d[1]
                ));
            }
            Ok(Need::Lease)
        }
        0xB6 => {
            if d[1] != 1 || d[2..7] != [0; 5] || d[7] != ARM {
                return Err("B6: only one target, armed".into());
            }
            Ok(Need::Lease)
        }
        0xB9 => {
            let idx = u32::from_be_bytes([0, d[1], d[2], d[3]]);
            let addr = POKE_BASE + (idx << 2);
            if !POKE_WORDS.contains(&addr) {
                return Err(format!("B9 write to {addr:08X} is not allowed"));
            }
            Ok(Need::Lease)
        }
        op => Err(format!(
            "0x7B0 opcode {op:02X} is not a service command carpid sends"
        )),
    }
}

/// B1 read request.
pub fn read(addr: u32, len: u8) -> [u8; 8] {
    let a = addr.to_be_bytes();
    [0xB1, a[0], a[1], a[2], a[3], len, 0, 0]
}

/// B9 write of one RAM word.
pub fn poke(addr: u32, value: u32) -> [u8; 8] {
    let i = ((addr - POKE_BASE) >> 2).to_be_bytes();
    let v = value.to_be_bytes();
    [0xB9, i[1], i[2], i[3], v[0], v[1], v[2], v[3]]
}

/// B3 data frame `seq`: bytes `seq*5 ..` of the staged image.
pub fn stage(seq: u16, chunk: [u8; 5]) -> [u8; 8] {
    let s = seq.to_be_bytes();
    [
        0xB3, s[0], s[1], chunk[0], chunk[1], chunk[2], chunk[3], chunk[4],
    ]
}

/// B3 END over `n` staged bytes: the unit CRCs them and records `n`.
pub fn stage_end(n: u32) -> [u8; 8] {
    let b = n.to_be_bytes();
    [0xB3, 0xFF, 0xFF, b[1], b[2], b[3], 0, 0]
}

/// B7 slot 0 -> sector 107 with the image CRC.
pub fn set_target(crc: u32) -> [u8; 8] {
    let s = SECTOR.to_be_bytes();
    let c = crc.to_be_bytes();
    [0xB7, 0, s[0], s[1], c[0], c[1], c[2], c[3]]
}

/// B6: commit one staged sector, then one cold reset.
pub fn commit() -> [u8; 8] {
    [0xB6, 1, 0, 0, 0, 0, 0, ARM]
}

pub fn frame(d: &[u8; 8]) -> Frame {
    Frame::new(carpi_signals::ids::RNSE_CMD, d).expect("8 bytes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builders_pass_and_name_their_need() {
        assert_eq!(classify(&read(SECTOR_XIP, 64)), Ok(Need::Read));
        assert_eq!(classify(&read(0xB800001A, 2)), Ok(Need::Read));
        assert_eq!(classify(&read(0xACE1FFD0, 48)), Ok(Need::Read));
        assert_eq!(
            classify(&poke(AUTO_START_WORD, 0x4F464621)),
            Ok(Need::Lease)
        );
        assert_eq!(classify(&stage(0, [1; 5])), Ok(Need::Lease));
        assert_eq!(classify(&stage_end(SECTOR_SIZE)), Ok(Need::Lease));
        assert_eq!(classify(&set_target(0x1234_5678)), Ok(Need::Lease));
        assert_eq!(classify(&commit()), Ok(Need::Lease));
        assert_eq!(
            poke(AUTO_START_WORD, 0x4F464621),
            [0xB9, 0xDC, 0xBC, 0x01, 0x4F, 0x46, 0x46, 0x21]
        );
        assert_eq!(
            set_target(0xAABBCCDD),
            [0xB7, 0, 0, 107, 0xAA, 0xBB, 0xCC, 0xDD]
        );
    }

    #[test]
    fn anything_else_is_refused() {
        // Other sectors, slots, counts, unarmed commits.
        assert!(classify(&[0xB7, 0, 0, 106, 0, 0, 0, 0]).is_err());
        assert!(classify(&[0xB7, 1, 0, 107, 0, 0, 0, 0]).is_err());
        assert!(classify(&[0xB6, 2, 0, 0, 0, 0, 0, ARM]).is_err());
        assert!(classify(&[0xB6, 1, 0, 0, 0, 0, 0, 0]).is_err());
        // B9 anywhere but the one word.
        assert!(classify(&poke(0x8F72F000, 0)).is_err());
        assert!(classify(&poke(0x8F72F008, 0)).is_err());
        // Reads outside the windows, too long, wrapping.
        assert!(classify(&read(0x7FFF_FFF0, 32)).is_err());
        assert!(classify(&read(0xB800_0030, 32)).is_err());
        assert!(classify(&read(SECTOR_XIP, 65)).is_err());
        assert!(classify(&read(0xFFFF_FFF0, 32)).is_err());
        // Oversized END; every other opcode (B0 boot, B4/B5 single commits, B8 task start).
        assert!(classify(&stage_end(SECTOR_SIZE + 4)).is_err());
        for op in [0xB0, 0xB2, 0xB4, 0xB5, 0xB8, 0xBA, 0x00] {
            assert!(classify(&[op, 0, 0, 0, 0, 0, 0, 0]).is_err(), "{op:02X}");
        }
    }
}
