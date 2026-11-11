//! Modules by VCDS address, and the store names of arbitrary measuring-block
//! fields (docs/PAGES.md 5c, Module groups).
//!
//! The table maps VCDS address to TP2 logical ID the way Drew's gateway
//! installation list does (the same table as the LIVI fork's
//! `livi-carpi-ui/src/diag.rs`). A live install list overrides it in
//! [`crate::Diag`].

/// `(VCDS address, TP2 ID, name)`, by address.
pub const MODULES: &[(u8, u8, &str)] = &[
    (0x01, 0x01, "Engine"),
    (0x02, 0x02, "Auto trans"),
    (0x03, 0x03, "ABS brakes"),
    (0x08, 0x2C, "Auto HVAC"),
    (0x09, 0x20, "Central electrics"),
    (0x0F, 0x4F, "Digital radio"),
    (0x15, 0x05, "Airbags"),
    (0x16, 0x2A, "Steering wheel"),
    (0x17, 0x07, "Instruments"),
    (0x19, 0x1F, "CAN gateway"),
    (0x22, 0x0A, "AWD"),
    (0x25, 0x14, "Immobilizer"),
    (0x37, 0x5B, "Navigation"),
    (0x42, 0x22, "Door electronics, driver"),
    (0x44, 0x09, "Steering assist"),
    (0x46, 0x21, "Central convenience"),
    (0x52, 0x23, "Door electronics, passenger"),
    (0x55, 0x06, "Headlight range"),
    (0x56, 0x52, "Radio"),
    (0x65, 0x29, "Tyre pressure"),
];

/// VCDS address of ABS. Never polled (polling it upsets ESP and the Haldex).
pub const ABS_ADDRESS: u8 = 0x03;

/// A module's name by VCDS address; `Module` when the table doesn't know it.
pub fn name_of(address: u8) -> &'static str {
    MODULES
        .iter()
        .find(|m| m.0 == address)
        .map_or("Module", |m| m.2)
}

/// TP2 ID by VCDS address, from the table.
pub fn default_tp2(address: u8) -> Option<u8> {
    MODULES.iter().find(|m| m.0 == address).map(|m| m.1)
}

/// VCDS address by TP2 ID, from the table.
pub fn default_address(tp2: u8) -> Option<u8> {
    MODULES.iter().find(|m| m.1 == tp2).map(|m| m.0)
}

/// One field of a measuring block, by VCDS address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct GroupField {
    pub address: u8,
    pub group: u8,
    /// 1-4.
    pub field: u8,
}

/// Store names start with this.
pub const PREFIX: &str = "diag.";

impl GroupField {
    /// `diag.01.103.2`: address as 2 hex digits, group decimal 0-255, field
    /// 1-4. Exactly that spelling; anything else is `None`.
    pub fn parse(name: &str) -> Option<Self> {
        let rest = name.strip_prefix(PREFIX)?;
        let mut it = rest.split('.');
        let (a, g, f) = (it.next()?, it.next()?, it.next()?);
        if it.next().is_some() || a.len() != 2 || !a.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let canonical = |s: &str| {
            !s.is_empty()
                && s.bytes().all(|b| b.is_ascii_digit())
                && (s == "0" || !s.starts_with('0'))
        };
        if !canonical(g) || !canonical(f) {
            return None;
        }
        let field: u8 = f.parse().ok()?;
        if !(1..=4).contains(&field) {
            return None;
        }
        Some(GroupField {
            address: u8::from_str_radix(a, 16).ok()?,
            group: g.parse().ok()?,
            field,
        })
    }

    /// The store name, upper-case hex address: `diag.01.103.2`.
    pub fn name(&self) -> String {
        format!("{PREFIX}{:02X}.{}.{}", self.address, self.group, self.field)
    }

    /// What a picker shows until it's named: `01 · 103 · 2`.
    pub fn label(&self) -> String {
        format!("{:02X} · {} · {}", self.address, self.group, self.field)
    }

    /// The four fields of a whole block, for a prefix `diag.01.103` or
    /// `diag.01.103.`; one field for a full name. `None` otherwise.
    pub fn for_prefix(prefix: &str) -> Option<Vec<GroupField>> {
        if let Some(f) = GroupField::parse(prefix) {
            return Some(vec![f]);
        }
        let block = prefix.strip_suffix('.').unwrap_or(prefix);
        let f = GroupField::parse(&format!("{block}.1"))?;
        Some((1..=4).map(|field| GroupField { field, ..f }).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        let f = GroupField::parse("diag.01.103.2").unwrap();
        assert_eq!(
            f,
            GroupField {
                address: 1,
                group: 103,
                field: 2
            }
        );
        assert_eq!(f.name(), "diag.01.103.2");
        assert_eq!(f.label(), "01 · 103 · 2");
        assert_eq!(
            GroupField::parse("diag.2a.0.4").unwrap().name(),
            "diag.2A.0.4"
        );
        for bad in [
            "diag.01.103",
            "diag.01.103.5",
            "diag.01.103.0",
            "diag.1.103.2",
            "diag.001.3.2",
            "diag.01.256.1",
            "diag.01.03.1",
            "diag.01.103.2.1",
            "diag.mode",
            "engine.rpm",
            "diag.0g.1.1",
        ] {
            assert_eq!(GroupField::parse(bad), None, "{bad}");
        }
        assert_eq!(GroupField::for_prefix("diag.01.115").unwrap().len(), 4);
        assert_eq!(GroupField::for_prefix("diag.01.115.").unwrap()[3].field, 4);
        assert_eq!(GroupField::for_prefix("diag.01.115.3").unwrap().len(), 1);
        assert_eq!(
            GroupField::for_prefix("diag.01.1"),
            Some(GroupField::for_prefix("diag.01.1.").unwrap())
        );
        assert_eq!(GroupField::for_prefix("diag."), None);
        assert_eq!(GroupField::for_prefix("diag.01"), None);
    }

    #[test]
    fn table() {
        assert_eq!(default_tp2(0x22), Some(0x0A));
        assert_eq!(default_address(0x20), Some(0x09));
        assert_eq!(name_of(0x19), "CAN gateway");
        assert_eq!(name_of(0x7E), "Module");
        assert_eq!(default_tp2(ABS_ADDRESS), Some(carpi_kwp::ABS));
    }
}
