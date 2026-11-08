use std::fmt;
use std::time::{Instant, SystemTime};

/// A classic CAN data frame (up to 8 bytes).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Frame {
    pub id: u32,
    pub extended: bool,
    len: u8,
    data: [u8; 8],
}

impl Frame {
    /// Standard (11-bit) frame. `None` if the ID or length is out of range.
    pub fn new(id: u32, data: &[u8]) -> Option<Self> {
        if id > 0x7FF {
            return None;
        }
        Self::build(id, false, data)
    }

    /// Extended (29-bit) frame.
    pub fn new_extended(id: u32, data: &[u8]) -> Option<Self> {
        if id > 0x1FFF_FFFF {
            return None;
        }
        Self::build(id, true, data)
    }

    fn build(id: u32, extended: bool, data: &[u8]) -> Option<Self> {
        if data.len() > 8 {
            return None;
        }
        let mut buf = [0u8; 8];
        buf[..data.len()].copy_from_slice(data);
        Some(Frame {
            id,
            extended,
            len: data.len() as u8,
            data: buf,
        })
    }

    pub fn data(&self) -> &[u8] {
        &self.data[..self.len as usize]
    }

    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl fmt::Debug for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.extended {
            write!(f, "{:08X} [{}]", self.id, self.len)?;
        } else {
            write!(f, "{:03X} [{}]", self.id, self.len)?;
        }
        for b in self.data() {
            write!(f, " {b:02X}")?;
        }
        Ok(())
    }
}

/// A received frame with its receive times. `at` drives all timing logic;
/// `wall` is only for logs and captures.
#[derive(Clone, Copy, Debug)]
pub struct RxFrame {
    pub frame: Frame,
    pub at: Instant,
    pub wall: SystemTime,
}
