//! LIVI's default key bindings as Linux key codes.
//!
//! LIVI binds DOM-style codes (`livi-core-proto/src/config.rs:582-623`) and
//! turns evdev scancodes into them (`livi-ui-model/src/scancode.rs`). Left and
//! Right become AA rotary steps; Up/Down are D-pad; Enter is select. LIVI has
//! no long press, so a long gesture must map to a different key here.

/// One LIVI binding. `Code` is any other Linux key code, for bindings set in
/// LIVI's Settings → Key Bindings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LiviKey {
    /// `up` = ArrowUp.
    Up,
    /// `down` = ArrowDown.
    Down,
    /// `left` = ArrowLeft: AA rotary -1, menu left.
    Left,
    /// `right` = ArrowRight: AA rotary +1, menu right.
    Right,
    /// `selectDown` = Enter.
    Select,
    /// `back` = Backspace.
    Back,
    /// `home` = KeyH.
    Home,
    /// `playPause` = KeyP.
    PlayPause,
    /// `next` = KeyN.
    Next,
    /// `prev` = KeyB.
    Prev,
    /// `acceptPhone` = KeyA.
    AcceptPhone,
    /// `rejectPhone` = KeyR.
    RejectPhone,
    /// `voiceAssistant` = KeyV.
    Voice,
    /// `cycleSession` = KeyS.
    CycleSession,
    /// D-pad left/right (Prev/Next long). On LIVI's own screens the same
    /// arrow keys as the knob; in AA/CarPlay a real D-pad press, where the
    /// knob's Left/Right are rotary steps.
    DpadLeft,
    DpadRight,
    Code(u16),
}

impl LiviKey {
    /// Every named key, for registering with uinput.
    pub const ALL: [LiviKey; 14] = [
        LiviKey::Up,
        LiviKey::Down,
        LiviKey::Left,
        LiviKey::Right,
        LiviKey::Select,
        LiviKey::Back,
        LiviKey::Home,
        LiviKey::PlayPause,
        LiviKey::Next,
        LiviKey::Prev,
        LiviKey::AcceptPhone,
        LiviKey::RejectPhone,
        LiviKey::Voice,
        LiviKey::CycleSession,
    ];

    /// Linux `KEY_*` code (input-event-codes.h).
    pub const fn code(self) -> u16 {
        match self {
            LiviKey::Up => 103,          // KEY_UP
            LiviKey::Down => 108,        // KEY_DOWN
            LiviKey::Left => 105,        // KEY_LEFT
            LiviKey::Right => 106,       // KEY_RIGHT
            LiviKey::Select => 28,       // KEY_ENTER
            LiviKey::Back => 14,         // KEY_BACKSPACE
            LiviKey::Home => 35,         // KEY_H
            LiviKey::PlayPause => 25,    // KEY_P
            LiviKey::Next => 49,         // KEY_N
            LiviKey::Prev => 48,         // KEY_B
            LiviKey::AcceptPhone => 30,  // KEY_A
            LiviKey::RejectPhone => 19,  // KEY_R
            LiviKey::Voice => 47,        // KEY_V
            LiviKey::CycleSession => 31, // KEY_S
            // Not in ALL: the arrows are registered through Left/Right.
            LiviKey::DpadLeft => 105,
            LiviKey::DpadRight => 106,
            LiviKey::Code(c) => c,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_unique() {
        let mut c: Vec<u16> = LiviKey::ALL.iter().map(|k| k.code()).collect();
        c.sort();
        c.dedup();
        assert_eq!(c.len(), LiviKey::ALL.len());
    }
}
