//! Bit extraction. Everything on ICAN we use is Intel (little-endian) byte
//! order with DBC-style start bits: bit `n` is bit `n % 8` of byte `n / 8`.

/// Unsigned field. `None` if the frame is too short to hold it.
pub fn get(data: &[u8], start: u32, len: u32) -> Option<u64> {
    debug_assert!((1..=64).contains(&len));
    let end = start + len;
    if end as usize > data.len() * 8 {
        return None;
    }
    let mut word = 0u64;
    for (i, b) in data.iter().enumerate().take(8) {
        word |= (*b as u64) << (8 * i);
    }
    let v = word >> start;
    Some(if len == 64 {
        v
    } else {
        v & ((1u64 << len) - 1)
    })
}

pub fn bit(data: &[u8], n: u32) -> Option<bool> {
    get(data, n, 1).map(|v| v == 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intel_fields() {
        let d = [0x34, 0x12, 0xF0];
        assert_eq!(get(&d, 0, 16), Some(0x1234));
        assert_eq!(get(&d, 4, 8), Some(0x23));
        assert_eq!(get(&d, 20, 4), Some(0xF));
        assert_eq!(bit(&d, 2), Some(true));
        assert_eq!(bit(&d, 0), Some(false));
        assert_eq!(get(&d, 20, 8), None);
    }
}
