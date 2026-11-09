use std::time::Instant;

/// A physical key. Wheel keys come from 0x5C3, the stalk rocker from 0x2C1,
/// the rest from 0x461.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Key {
    // --- Steering wheel (0x5C3) ---
    WheelUp,
    WheelDown,
    WheelClick,
    Mode,
    Ptt,
    /// Volume roller. The RNS-E handles volume; decoded, never routed by default.
    VolUp,
    VolDown,
    VolClick,

    // --- Wiper-stalk rocker for the cluster display (0x2C1) ---
    StalkUp,
    StalkDown,

    // --- RNS-E faceplate (0x461, only while on TV) ---
    /// Track back (`01 00`).
    Prev,
    /// Track forward (`02 00`).
    Next,
    /// `40 00`.
    Up,
    /// `80 00`.
    Down,
    /// Knob push (`00 10`).
    Enter,
    /// `00 02`.
    Return,
    /// `00 01`.
    Setup,
    /// `08 00`.
    Nav,
    /// `00 08`.
    Tel,
    /// `00 04`.
    Media,
    /// `04 00`.
    Name,
    /// `00 0C`.
    Info,
    /// `0C 00`.
    Car,

    /// Knob rotation in detents; positive is clockwise (`00 20`), assumed.
    Knob(i8),
}

impl Key {
    pub fn is_wheel(self) -> bool {
        matches!(
            self,
            Key::WheelUp
                | Key::WheelDown
                | Key::WheelClick
                | Key::Mode
                | Key::Ptt
                | Key::VolUp
                | Key::VolDown
                | Key::VolClick
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Down,
    /// Another press frame for a key that is already down.
    Repeat,
    Up,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: Key,
    pub edge: Edge,
    pub at: Instant,
}
