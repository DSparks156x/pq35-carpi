//! uinput keyboard for LIVI. labwc (host mode) or libinput (DRM mode) picks it
//! up like any keyboard. Needs write access to /dev/uinput (udev rule, not
//! chmod at runtime).

use crate::LiviKey;
use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, EventType, InputEvent, KeyCode};
use std::io;

pub struct LiviKeyboard {
    dev: VirtualDevice,
}

impl LiviKeyboard {
    pub const NAME: &'static str = "carpi-keys";

    /// Creates the device with LIVI's default keys plus `extra` codes.
    pub fn open(extra: impl IntoIterator<Item = u16>) -> io::Result<Self> {
        let mut keys = AttributeSet::<KeyCode>::new();
        for k in LiviKey::ALL {
            keys.insert(KeyCode::new(k.code()));
        }
        for c in extra {
            keys.insert(KeyCode::new(c));
        }
        let dev = VirtualDevice::builder()?
            .name(Self::NAME)
            .with_keys(&keys)?
            .build()?;
        Ok(LiviKeyboard { dev })
    }

    /// Down then up, each with its own SYN. LIVI acts on the down edge.
    pub fn press(&mut self, key: LiviKey) -> io::Result<()> {
        let code = key.code();
        self.dev
            .emit(&[InputEvent::new(EventType::KEY.0, code, 1)])?;
        self.dev.emit(&[InputEvent::new(EventType::KEY.0, code, 0)])
    }
}
